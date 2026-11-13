//! Tests of the viewer's link (remote sessions PR 3), against an in-process host that keeps a tab's output numbered as
//! the real one does (a ring with a start, `gap` when a viewer asks for what fell out of it). No ssh, no other machine.

use super::*;
use flare_ctl::remote::parse_intents;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct HostTab {
    /// Every byte the tab printed; the ring holds `out[start..]`.
    out: Vec<u8>,
    start: u64,
    typed: Vec<Intent>,
    sizes: Vec<(u64, u64)>,
}

#[derive(Default)]
struct HostState {
    tabs: HashMap<String, HostTab>,
    next: u32,
    calls: Vec<String>,
    attached: HashSet<(u64, String)>,
    /// Links made before this epoch are broken.
    epoch: u64,
    refuse: bool,
    closed: Vec<String>,
    /// The host's state word of each tab (remote sessions PR 4); None: an older host with no remote.status.
    states: Option<HashMap<String, String>>,
    /// A passing refusal of `remote.status` (the host's UI wait timed out, a rate refusal), while set.
    status_refusal: Option<&'static str>,
    /// The host's files a fetch may read, by the path a link names (remote sessions PR 6).
    files: HashMap<String, Vec<u8>>,
    /// The files put on the host, whole: (name kept, bytes).
    put: Vec<(String, Vec<u8>)>,
    /// A put being written: (name kept, bytes so far).
    putting: Option<(String, Vec<u8>)>,
}

#[derive(Clone, Default)]
struct FakeHost(Arc<Mutex<HostState>>);

fn lk(h: &FakeHost) -> std::sync::MutexGuard<'_, HostState> {
    h.0.lock().unwrap()
}

fn no_tab() -> CtlError {
    CtlError::new(code::NO_TAB, "that tab is closed")
}

fn enc(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

impl FakeHost {
    fn answer(&self, conn: u64, m: Method, p: &Value) -> Result<Value, CtlError> {
        let mut st = lk(self);
        st.calls.push(m.name().to_string());
        let tab = p["tab"].as_str().unwrap_or_default().to_string();
        match m {
            Method::RemoteOpen => {
                st.next += 1;
                let id = format!("h{}", st.next);
                st.tabs.insert(id.clone(), HostTab::default());
                Ok(json!({"tab": id}))
            }
            Method::RemoteAttach => {
                let seq = st.tabs.get(&tab).ok_or_else(no_tab)?.out.len();
                st.attached.insert((conn, tab.clone()));
                Ok(json!({"tab": tab, "cols": 80, "rows": 24, "seq": seq}))
            }
            Method::RemoteOutput => {
                if !st.attached.contains(&(conn, tab.clone())) {
                    return Err(no_tab());
                }
                let t = st.tabs.get(&tab).ok_or_else(no_tab)?;
                let since = p["since"].as_u64().unwrap_or(0);
                if since < t.start {
                    return Ok(json!({"from": t.start, "to": t.start, "data": "", "gap": true}));
                }
                let max = p["max"].as_u64().unwrap_or(256 * 1024) as usize;
                let from = (since as usize).min(t.out.len());
                let to = (from + max).min(t.out.len());
                Ok(json!({"from": from, "to": to, "data": enc(&t.out[from..to]), "gap": false}))
            }
            Method::RemoteSnapshot => {
                let t = st.tabs.get(&tab).ok_or_else(no_tab)?;
                let mut data = b"\x1b[H\x1b[2J\x1b[3J".to_vec();
                data.extend_from_slice(&t.out[t.start as usize..]);
                Ok(json!({"seq": t.out.len(), "cols": 80, "rows": 24, "data": enc(&data)}))
            }
            Method::RemoteInput => {
                let intents = parse_intents(p.get("intents"))?;
                let n = intents.len();
                st.tabs
                    .get_mut(&tab)
                    .ok_or_else(no_tab)?
                    .typed
                    .extend(intents);
                Ok(json!({"typed": n}))
            }
            Method::RemoteResize => {
                let (c, r) = (p["cols"].as_u64().unwrap(), p["rows"].as_u64().unwrap());
                st.tabs.get_mut(&tab).ok_or_else(no_tab)?.sizes.push((c, r));
                Ok(json!({"cols": c, "rows": r}))
            }
            Method::RemoteDetach => {
                st.attached.remove(&(conn, tab));
                Ok(json!({"detached": true}))
            }
            Method::RemoteClose => {
                st.tabs.remove(&tab).ok_or_else(no_tab)?;
                st.closed.push(tab);
                Ok(json!({"closed": true}))
            }
            Method::RemoteStatus => {
                if let Some(c) = st.status_refusal {
                    return Err(CtlError::new(c, "not now"));
                }
                let states = st
                    .states
                    .as_ref()
                    .ok_or_else(|| CtlError::new(code::UNKNOWN_METHOD, "an older host"))?;
                let tabs: Vec<Value> = states
                    .iter()
                    .map(|(t, s)| json!({"tab": t, "state": s, "kind": "claude"}))
                    .collect();
                Ok(json!({"tabs": tabs, "seq": 1}))
            }
            Method::RemoteFetch => {
                st.tabs.get(&tab).ok_or_else(no_tab)?;
                let path = p["path"].as_str().unwrap_or_default();
                let data = st
                    .files
                    .get(path)
                    .ok_or_else(|| CtlError::new(code::NOT_FOUND, "no such file"))?;
                let chunk = p["chunk"].as_u64().unwrap_or(0) as usize;
                let chunks = flare_ctl::remote::chunks_of(data.len() as u64);
                let from = (chunk * FILE_CHUNK).min(data.len());
                let to = (from + FILE_CHUNK).min(data.len());
                let name = path.rsplit('/').next().unwrap_or(path);
                Ok(json!({
                    "tab": tab, "name": name, "size": data.len(), "chunk": chunk, "chunks": chunks,
                    "data": enc(&data[from..to]),
                }))
            }
            Method::RemotePut => {
                st.tabs.get(&tab).ok_or_else(no_tab)?;
                let chunk = p["chunk"].as_u64().unwrap();
                let chunks = p["chunks"].as_u64().unwrap();
                let data = base64::engine::general_purpose::STANDARD
                    .decode(p["data"].as_str().unwrap())
                    .unwrap();
                let name = if chunk == 0 {
                    let n = flare_ctl::remote::put_name(p["name"].as_str().unwrap());
                    st.putting = Some((n.clone(), data));
                    n
                } else {
                    let (n, so_far) = st.putting.as_mut().expect("a put under way");
                    assert_eq!(p["name"].as_str(), Some(n.as_str()), "the kept name");
                    so_far.extend(data);
                    n.clone()
                };
                if chunk + 1 < chunks {
                    return Ok(json!({"tab": tab, "name": name, "done": false}));
                }
                let whole = st.putting.take().unwrap();
                st.put.push(whole);
                let host = format!("/tmp/up/{name}");
                Ok(
                    json!({"tab": tab, "name": name, "done": true, "host_path": host, "paste": format!("'{host}'")}),
                )
            }
            _ => Err(CtlError::new(code::FORBIDDEN, "not here")),
        }
    }

    fn print(&self, tab: &str, b: &[u8]) {
        lk(self).tabs.get_mut(tab).unwrap().out.extend_from_slice(b);
    }

    /// Every link breaks (ssh died, the network went).
    fn cut(&self) {
        lk(self).epoch += 1;
    }

    fn calls(&self, name: &str) -> usize {
        lk(self).calls.iter().filter(|c| *c == name).count()
    }

    fn typed(&self, tab: &str) -> Vec<Intent> {
        lk(self)
            .tabs
            .get(tab)
            .map(|t| t.typed.clone())
            .unwrap_or_default()
    }
}

struct FakeLink {
    host: FakeHost,
    conn: u64,
    epoch: u64,
}

impl Link for FakeLink {
    fn call(&mut self, m: Method, p: Value, _t: Duration) -> Result<Value, LinkError> {
        if lk(&self.host).epoch != self.epoch {
            return Err(LinkError::Down("the link broke".into()));
        }
        self.host
            .answer(self.conn, m, &p)
            .map_err(LinkError::Refused)
    }
}

struct FakeConnector {
    host: FakeHost,
    conns: AtomicU64,
}

impl Connector for FakeConnector {
    fn connect(&self, _machine: &str) -> Result<Box<dyn Link>, LinkError> {
        let (refuse, epoch) = {
            let st = lk(&self.host);
            (st.refuse, st.epoch)
        };
        if refuse {
            return Err(LinkError::Down("no route to host".into()));
        }
        Ok(Box::new(FakeLink {
            host: self.host.clone(),
            conn: self.conns.fetch_add(1, Ordering::SeqCst),
            epoch,
        }))
    }
}

fn fast() -> Timing {
    Timing {
        poll_min: Duration::from_millis(5),
        poll_max: Duration::from_millis(20),
        backoff_min: Duration::from_millis(20),
        backoff_max: Duration::from_millis(80),
        call: Duration::from_secs(5),
        long_call: Duration::from_secs(5),
        end_call: Duration::from_secs(1),
        status_every: Duration::from_millis(10),
    }
}

/// A remote tab's pty over the fake host, its output read into a buffer on a thread.
struct Rig {
    host: FakeHost,
    live: flare_term::remote_pty::RemoteLive,
    pty: Box<dyn PtyProcess>,
    seen: Arc<Mutex<Vec<u8>>>,
    writer: Box<dyn Write + Send>,
}

fn rig_with(host: FakeHost, host_tab: Option<&str>) -> Rig {
    let live = flare_term::remote_pty::RemoteLive::new(host_tab.map(str::to_string), 0);
    let sp = RemoteSpawner::new(
        Arc::new(FakeConnector {
            host: host.clone(),
            conns: AtomicU64::new(1),
        }),
        fast(),
    );
    let spec = SpawnSpec {
        exe: String::new(),
        args: Vec::new(),
        cwd: "/Users/max/proj".into(),
        env: Vec::new(),
        cols: 80,
        rows: 24,
        remote: Some(RemoteSpec {
            machine: "mac".into(),
            folder: "/Users/max/proj".into(),
            resume: None,
            live: live.clone(),
        }),
    };
    let mut pty = sp.spawn(&spec).unwrap();
    let mut reader = pty.take_reader().unwrap();
    let writer = pty.take_writer().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            s2.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    Rig {
        host,
        live,
        pty,
        seen,
        writer,
    }
}

fn rig() -> Rig {
    rig_with(FakeHost::default(), None)
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5)); // sync-ok: test code only, never the app
    }
}

impl Rig {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.seen.lock().unwrap()).into_owned()
    }

    fn live_on(&self) {
        wait_for("the link", || self.live.link() == LinkState::Live);
    }
}

/// A new tab opens on the host, attaches with this tab's size, starts from the host's screen and then shows what the
/// host prints.
#[test]
fn a_new_remote_tab_opens_on_the_host_and_shows_its_output() {
    let r = rig();
    r.live_on();
    assert_eq!(r.live.host_tab().as_deref(), Some("h1"));
    assert_eq!(r.host.calls("remote.open"), 1);
    assert_eq!(r.host.calls("remote.snapshot"), 1);
    assert_eq!(lk(&r.host).tabs["h1"].sizes, vec![(80, 24)]);
    r.host.print("h1", b"PS C:\\> ");
    wait_for("the prompt", || r.text().contains("PS C:\\> "));
    let seq = r.live.seq();
    assert_eq!(seq, 8);
}

/// Rule 279: typing reaches the host as keys and text; a resize as the last size asked.
#[test]
fn typing_reaches_the_host_as_intents_and_sizes_as_the_last_one() {
    let mut r = rig();
    r.live_on();
    r.writer.write_all(b"echo hi").unwrap();
    r.writer.write_all(b"\r").unwrap();
    wait_for("the typing", || r.host.typed("h1").len() >= 2);
    assert_eq!(
        r.host.typed("h1"),
        vec![
            Intent::Text("echo hi".into()),
            Intent::Key(KeyIntent {
                key: "Enter".into(),
                code: "Enter".into(),
                ..KeyIntent::default()
            }),
        ]
    );
    r.pty.resize(100, 30).unwrap();
    wait_for("the size", || {
        lk(&r.host).tabs["h1"].sizes.last() == Some(&(100, 30))
    });
}

/// Rule 288 and the reconnect: the link breaks, the host prints meanwhile, the tab attaches again and goes on from
/// the number it read: every byte once, no snapshot (the number is still in the ring).
#[test]
fn a_reconnect_goes_on_from_the_number_read_with_no_byte_twice() {
    let r = rig();
    r.live_on();
    r.host.print("h1", b"one\r\n");
    wait_for("one", || r.text().contains("one"));
    lk(&r.host).refuse = true;
    r.host.cut();
    wait_for("the link down", || r.live.link() != LinkState::Live);
    r.host.print("h1", b"two\r\n");
    lk(&r.host).refuse = false;
    wait_for("two", || r.text().contains("two"));
    r.host.print("h1", b"three\r\n");
    wait_for("three", || r.text().contains("three"));
    let t = r.text();
    for w in ["one", "two", "three"] {
        assert_eq!(t.matches(w).count(), 1, "{w} in {t:?}");
    }
    assert_eq!(r.host.calls("remote.attach"), 2);
    assert_eq!(
        r.host.calls("remote.snapshot"),
        1,
        "no snapshot on a reconnect in the ring"
    );
    assert!(
        lk(&r.host).tabs.contains_key("h1"),
        "the host session lives on"
    );
}

/// The number fell out of the host's ring while the link was down: the tab starts again from a snapshot.
#[test]
fn a_gap_after_a_reconnect_takes_a_snapshot() {
    let r = rig();
    r.live_on();
    r.host.print("h1", b"old\r\n");
    wait_for("old", || r.text().contains("old"));
    lk(&r.host).refuse = true;
    r.host.cut();
    wait_for("the link down", || r.live.link() != LinkState::Live);
    {
        let mut st = lk(&r.host);
        let t = st.tabs.get_mut("h1").unwrap();
        t.out.extend_from_slice(b"lost\r\nkept\r\n");
        t.start = t.out.len() as u64 - 6;
        st.refuse = false;
    }
    wait_for("kept", || r.text().contains("kept"));
    assert!(!r.text().contains("lost"));
    assert_eq!(r.host.calls("remote.snapshot"), 2);
}

/// Rule 289: what a reconnect catches up on is replayed without the agent's signals; live output keeps them.
#[test]
fn a_catch_up_carries_no_agent_signal_and_live_output_does() {
    let r = rig();
    r.live_on();
    lk(&r.host).refuse = true;
    r.host.cut();
    wait_for("the link down", || r.live.link() != LinkState::Live);
    r.host.print(
        "h1",
        b"a\x1b]777;flare-agent;state=done\x07b\x1b]0;title\x07",
    );
    lk(&r.host).refuse = false;
    wait_for("the catch-up", || r.text().contains("title"));
    assert!(!r.text().contains("777"), "{:?}", r.text());
    assert!(r.text().contains("ab\x1b]0;title\x07"));
    r.host
        .print("h1", b"\x1b]777;flare-agent;state=working\x07");
    wait_for("the live signal", || r.text().contains("state=working"));
}

/// Rule 288: a restored tab attaches to its host tab (no new one is opened) and starts from a snapshot, since this
/// machine's screen is empty after the restart.
#[test]
fn a_restored_tab_attaches_to_its_host_tab_and_opens_none() {
    let host = FakeHost::default();
    lk(&host).tabs.insert(
        "h7".into(),
        HostTab {
            out: b"before the restart\r\n".to_vec(),
            ..HostTab::default()
        },
    );
    let r = rig_with(host, Some("h7"));
    r.live_on();
    wait_for("the screen", || r.text().contains("before the restart"));
    assert_eq!(r.host.calls("remote.open"), 0);
    assert_eq!(r.host.calls("remote.snapshot"), 1);
}

/// The host's tab is gone (closed there, or the host started again): the tab ends as a shell that exits.
#[test]
fn a_host_tab_that_is_gone_ends_the_tab() {
    let mut r = rig_with(FakeHost::default(), Some("h404"));
    let (tx, rx) = std::sync::mpsc::channel();
    r.pty.on_exit(Box::new(move |c| {
        let _ = tx.send(c);
    }));
    assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok()); // sync-ok: test code only, never the app
    wait_for("the end", || r.live.link() == LinkState::Ended);
}

/// Closing the tab closes the host session; the quit only detaches and the session lives on.
#[test]
fn a_close_ends_the_host_session_and_a_leave_only_detaches() {
    let mut r = rig();
    r.live_on();
    r.pty.kill();
    wait_for("the close", || r.host.calls("remote.close") == 1);
    assert_eq!(lk(&r.host).closed, vec!["h1".to_string()]);

    let mut r = rig();
    r.live_on();
    r.pty.leave();
    wait_for("the detach", || r.host.calls("remote.detach") == 1);
    assert!(
        lk(&r.host).tabs.contains_key("h1"),
        "the host session lives on"
    );
    assert_eq!(r.host.calls("remote.close"), 0);
}

/// Rule 28: a dead link never holds a close: the pty ends at once, and the close is still taken to the host when it
/// answers again.
#[test]
fn a_dead_link_never_holds_a_close() {
    let host = FakeHost::default();
    lk(&host).tabs.insert("h3".into(), HostTab::default());
    lk(&host).refuse = true;
    let mut r = rig_with(host, Some("h3"));
    wait_for("the first try", || r.live.link() == LinkState::Down);
    let t = Instant::now();
    r.pty.kill();
    assert!(t.elapsed() < Duration::from_millis(100));
    assert!(r.pty.exit_code().is_some());
    wait_for("the worker's end", || r.live.link() == LinkState::Ended);
    assert!(lk(&r.host).closed.is_empty(), "the host never answered");

    // Closed while the link is down and the host answers again: the close still reaches it.
    let mut r = rig();
    r.live_on();
    lk(&r.host).refuse = true;
    r.host.cut();
    wait_for("the link down", || r.live.link() == LinkState::Down);
    lk(&r.host).refuse = false;
    r.pty.kill();
    wait_for("the close", || !lk(&r.host).closed.is_empty());
    assert_eq!(lk(&r.host).closed, vec!["h1".to_string()]);
}

#[test]
fn the_back_off_doubles_from_one_second_to_thirty() {
    let max = Duration::from_secs(30);
    let mut d = Timing::default().backoff_min;
    let mut seen = vec![d.as_secs()];
    for _ in 0..7 {
        d = next_backoff(d, max);
        seen.push(d.as_secs());
    }
    assert_eq!(seen, vec![1, 2, 4, 8, 16, 30, 30, 30]);
}

/// A paste larger than one call goes in calls the host takes, in order, and nothing is lost.
#[test]
fn a_big_paste_goes_in_calls_within_the_caps() {
    let big = "x".repeat(200 * 1024) + "é";
    let typed = vec![
        RemoteIntent::Text("a".into()),
        RemoteIntent::Paste(big.clone()),
    ];
    let batches = input_batches(&typed);
    assert!(batches.len() > 1);
    let mut back = String::new();
    for b in &batches {
        let parsed = parse_intents(Some(&intents_json(b))).expect("the host takes each call");
        for i in parsed {
            if let Intent::Paste(p) = i {
                back.push_str(&p);
            }
        }
    }
    assert_eq!(back, big);
}

/// Rules 290 and R-new-24's base: the host and the user are argv items of their own, the remote command is quoted for
/// the host's shell, and nothing that could be read as an option or a second command gets through.
#[test]
fn the_ssh_command_is_argv_items_quoted_for_the_hosts_shell() {
    let m = |kind, ssh: &str, user: Option<&str>, flare: &str| Machine {
        id: "m".into(),
        name: "M".into(),
        kind,
        ssh: ssh.into(),
        user: user.map(str::to_string),
        flare: flare.into(),
    };
    let mac = ssh_argv(
        "/usr/bin/ssh",
        &m(
            Some(Os::Mac),
            "mac-mini",
            Some("max"),
            "/Applications/Fl are.app/flare",
        ),
    )
    .unwrap();
    assert_eq!(
        mac,
        [
            "/usr/bin/ssh",
            "-o",
            "BatchMode=yes",
            "-T",
            "-l",
            "max",
            "--",
            "mac-mini",
            "'/Applications/Fl are.app/flare' --serve-remote",
        ]
    );
    let pc = ssh_argv(
        "ssh.exe",
        &m(
            Some(Os::Pc),
            "pc",
            None,
            r"C:\Program Files\Flare\flare.exe",
        ),
    )
    .unwrap();
    assert_eq!(
        pc.last().unwrap(),
        r#"cmd /c "C:\Program Files\Flare\flare.exe" --serve-remote"#
    );
    assert!(!pc.iter().any(|a| a.contains("&&")));
    assert_eq!(
        remote_command(None, "it's/flare").as_deref(),
        Some(r"'it'\''s/flare' --serve-remote")
    );
    for (kind, ssh, user, flare) in [
        (Some(Os::Mac), "-oProxyCommand=x", None, "flare"),
        (Some(Os::Mac), "a b", None, "flare"),
        (Some(Os::Mac), "mac", Some("-x"), "flare"),
        (Some(Os::Pc), "pc", None, r#"C:\a"b\flare.exe"#),
        (Some(Os::Pc), "pc", None, r"C:\%PATH%\flare.exe"),
        (Some(Os::Mac), "mac", None, "fla\nre"),
    ] {
        assert_eq!(
            ssh_argv("ssh", &m(kind, ssh, user, flare)),
            None,
            "{ssh} {flare}"
        );
    }
}

#[test]
fn machines_json_keeps_good_machines_and_reads_an_unknown_kind_as_none() {
    let raw = r#"{"machines":[
        {"id":"mac","name":"Mac Mini","kind":"mac","ssh":"mac-mini","user":"max","flare":"/A/flare"},
        {"id":"box","kind":"linux","ssh":"box"},
        {"id":"bad","ssh":"-oX"},
        {"id":"","ssh":"x"}
    ]}"#;
    let ms = parse_machines(raw);
    assert_eq!(ms.len(), 2);
    assert_eq!(ms[0].kind, Some(Os::Mac));
    assert_eq!(ms[0].user.as_deref(), Some("max"));
    assert_eq!(ms[1].kind, None);
    assert_eq!(ms[1].name, "box");
    assert_eq!(ms[1].flare, "flare");
    assert!(parse_machines("not json").is_empty());
}

#[test]
fn the_e2e_seams_read_only_well_formed_values() {
    assert_eq!(
        e2e_loopback(Some(r#"["flare.exe","--serve-remote"]"#)),
        Some(vec!["flare.exe".to_string(), "--serve-remote".to_string()])
    );
    assert_eq!(e2e_loopback(Some("[]")), None);
    assert_eq!(e2e_loopback(None), None);
    assert_eq!(
        e2e_open(Some(r#"{"machine":"loop","folder":"C:\\w"}"#)),
        Some(("loop".to_string(), "C:\\w".to_string()))
    );
    assert_eq!(e2e_open(Some(r#"{"machine":"-x","folder":"C:\\w"}"#)), None);
}

// ---- over the wire ----

struct WireHost(FakeHost);

impl flare_ctl::Dispatcher for WireHost {
    fn resolve_token(&self, _t: &str) -> Option<flare_ctl::Grant> {
        None
    }
    fn resolve_tab_token(&self, _t: &str) -> Option<String> {
        None
    }
    fn remote_grant(&self, _o: flare_ctl::ConnOrigin) -> Option<flare_ctl::Grant> {
        Some(flare_ctl::Grant::Remote)
    }
    fn call(&self, ctx: &flare_ctl::CallContext, m: Method, p: Value) -> Result<Value, CtlError> {
        self.0.answer(ctx.conn, m, &p)
    }
}

/// The worker's calls are the protocol's: a `ClientLink` over an in-memory stdio pair to a served host says hello,
/// opens, attaches, types and reads output, and the host's checks (the intents among them) take every call.
#[test]
fn the_link_speaks_the_remote_protocol_over_a_stdio_wire() {
    use flare_ctl::framing::testwire::pair;
    use flare_ctl::session::Shared;
    let host = FakeHost::default();
    let shared = Shared::new_stdio(
        Arc::new(WireHost(host.clone())),
        flare_ctl::ServerConfig::stdio(),
    );
    let (server_end, viewer) = pair();
    let served = std::thread::spawn(move || {
        flare_ctl::serve_stdio(&server_end, &shared);
        server_end.close();
    });
    let mut c = Client::new(viewer);
    hello(&mut c).expect("remote.hello");
    let mut link = ClientLink::new(c, None);
    let opened = link
        .call(
            Method::RemoteOpen,
            json!({"folder": "/p"}),
            Duration::from_secs(5),
        )
        .unwrap();
    let tab = opened["tab"].as_str().unwrap().to_string();
    link.call(
        Method::RemoteAttach,
        json!({"tab": tab}),
        Duration::from_secs(5),
    )
    .unwrap();
    let batches = input_batches(&[
        RemoteIntent::Text("ls".into()),
        RemoteIntent::Key(flare_term::keys::KeyEvent::new("Enter", "Enter")),
    ]);
    link.call(
        Method::RemoteInput,
        json!({"tab": tab, "intents": intents_json(&batches[0])}),
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(host.typed(&tab).len(), 2);
    host.print(&tab, b"out");
    let v = link
        .call(
            Method::RemoteOutput,
            json!({"tab": tab, "since": 0}),
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(b64(&v), b"out");
    let gone = link.call(
        Method::RemoteOutput,
        json!({"tab": "nope", "since": 0}),
        Duration::from_secs(5),
    );
    assert!(matches!(gone, Err(LinkError::Refused(e)) if e.code == code::NO_TAB));
    link.client.wire().close();
    drop(link);
    served.join().unwrap(); // sync-ok: test code only, never the app
}

// ---- remote sessions PR 4: the host's status of the tab ----

/// Rule 294: the link polls `remote.status`, hands the window a change of its own host tab's words and wakes it; the
/// same words again are no news, and a reconnect hands them on once more (the hub applies only a new word).
#[test]
fn the_hosts_status_of_the_tab_reaches_the_window_on_change_and_wakes_it() {
    let host = FakeHost::default();
    lk(&host).states = Some(HashMap::new());
    let woke = Arc::new(AtomicU64::new(0));
    let w2 = Arc::clone(&woke);
    let live = flare_term::remote_pty::RemoteLive::new(None, 0);
    let sp = RemoteSpawner::new(
        Arc::new(FakeConnector {
            host: host.clone(),
            conns: AtomicU64::new(1),
        }),
        fast(),
    )
    .with_wake(Arc::new(move || {
        w2.fetch_add(1, Ordering::SeqCst);
    }));
    let spec = SpawnSpec {
        exe: String::new(),
        args: Vec::new(),
        cwd: "/Users/max/proj".into(),
        env: Vec::new(),
        cols: 80,
        rows: 24,
        remote: Some(RemoteSpec {
            machine: "mac".into(),
            folder: "/Users/max/proj".into(),
            resume: None,
            live: live.clone(),
        }),
    };
    let _pty = sp.spawn(&spec).unwrap();
    wait_for("the link", || live.link() == LinkState::Live);
    lk(&host)
        .states
        .as_mut()
        .unwrap()
        .insert("h1".into(), "working".into());
    wait_for("the working status", || {
        live.take_status().is_some_and(|s| s.state == "working")
    });
    assert!(woke.load(Ordering::SeqCst) >= 1, "the window was woken");
    // Polled again with nothing changed: no news.
    let polls = host.calls("remote.status");
    wait_for("more polls", || host.calls("remote.status") > polls + 3);
    assert_eq!(live.take_status(), None);
    lk(&host)
        .states
        .as_mut()
        .unwrap()
        .insert("h1".into(), "question".into());
    wait_for("the question status", || {
        live.take_status().is_some_and(|s| s.state == "question")
    });
    // A reconnect: the first answer after the attach is handed on again.
    host.cut();
    wait_for("the status after the reconnect", || {
        live.take_status().is_some_and(|s| s.state == "question")
    });
    assert_eq!(host.calls("remote.attach"), 2);
}

/// A host with no `remote.status` (an older Flare) keeps its link: the tab shows its output and the status is not
/// asked again on that link.
#[test]
fn an_older_host_without_remote_status_keeps_the_link_and_is_asked_once() {
    let r = rig();
    r.live_on();
    r.host.print("h1", b"still here\r\n");
    wait_for("the output", || r.text().contains("still here"));
    wait_for("the status asked", || r.host.calls("remote.status") >= 1);
    let polls = r.host.calls("remote.output");
    wait_for("more polls", || r.host.calls("remote.output") > polls + 3);
    assert_eq!(
        r.host.calls("remote.status"),
        1,
        "asked once, refused, not again"
    );
    assert_eq!(r.live.link(), LinkState::Live);
    assert_eq!(r.live.take_status(), None);
}

/// Rule 298: only `unknown_method` stops the status polls. A passing refusal (the host's UI wait timed out, a rate
/// refusal) is asked again at the next poll, so a question the host shows later still reaches the viewer.
#[test]
fn a_passing_status_refusal_keeps_the_polls_and_a_later_question_still_arrives() {
    let host = FakeHost::default();
    {
        let mut st = lk(&host);
        st.states = Some(HashMap::new());
        st.status_refusal = Some(code::TIMEOUT);
    }
    let r = rig_with(host, None);
    r.live_on();
    wait_for("the status asked again after a refusal", || {
        r.host.calls("remote.status") >= 3
    });
    {
        let mut st = lk(&r.host);
        st.status_refusal = Some(code::RATE_LIMITED);
    }
    let polls = r.host.calls("remote.status");
    wait_for("the status asked again after a rate refusal", || {
        r.host.calls("remote.status") > polls + 2
    });
    {
        let mut st = lk(&r.host);
        st.status_refusal = None;
        st.states
            .as_mut()
            .unwrap()
            .insert("h1".into(), "question".into());
    }
    wait_for("the later question", || {
        r.live.take_status().is_some_and(|s| s.state == "question")
    });
    assert_eq!(r.host.calls("remote.attach"), 1, "the link never broke");
}

// ---- files (remote sessions PR 6) ----

/// An errand's answer, caught for the test.
type Caught<T> = Arc<Mutex<Option<Result<T, ErrandFail>>>>;
/// The `done` an errand is given.
type Done<T> = Box<dyn FnOnce(Result<T, ErrandFail>) + Send>;

fn caught<T: Send + 'static>() -> (Caught<T>, Done<T>) {
    let slot: Caught<T> = Arc::default();
    let s = Arc::clone(&slot);
    (slot, Box::new(move |r| *s.lock().unwrap() = Some(r)))
}

fn answer<T: Clone>(slot: &Caught<T>) -> Result<T, ErrandFail> {
    wait_for("the errand's answer", || slot.lock().unwrap().is_some());
    slot.lock().unwrap().clone().unwrap()
}

/// Rule 303: a click's spellings are asked of the host in turn; the first file it has comes back whole, read in
/// chunks; a link the host has no file for is `NoSuchFile`.
#[test]
fn a_file_link_is_fetched_by_the_first_spelling_the_host_has_in_chunks() {
    let r = rig();
    r.live_on();
    let big: Vec<u8> = (0..(FILE_CHUNK * 2 + 7)).map(|i| (i % 253) as u8).collect();
    lk(&r.host)
        .files
        .insert("docs/big file.bin".into(), big.clone());
    let (slot, done) = caught();
    r.live.send_errand(Errand::Fetch {
        texts: vec!["docs/big".into(), "docs/big file.bin".into()],
        done,
    });
    let got = answer(&slot).unwrap();
    assert_eq!(got.name, "big file.bin");
    assert_eq!(got.data, big);
    assert_eq!(
        r.host.calls("remote.fetch"),
        4,
        "one miss, then three chunks"
    );
    let (slot, done) = caught();
    r.live.send_errand(Errand::Fetch {
        texts: vec!["gone.txt".into()],
        done,
    });
    assert_eq!(answer(&slot), Err(ErrandFail::NoSuchFile));
}

/// Rule 304: files go to the host in chunks under the name it keeps, then their host paths, as the host quoted them,
/// are pasted as one paste; this machine's path and bytes are never typed.
#[test]
fn a_put_copies_the_files_then_pastes_the_hosts_quoted_paths_as_one_paste() {
    let r = rig();
    r.live_on();
    let big = vec![9u8; FILE_CHUNK + 1];
    let (slot, done) = caught();
    r.live.send_errand(Errand::Put {
        files: vec![
            Upload {
                name: "C:\\Users\\max\\shot 1.png".into(),
                data: b"png".to_vec(),
            },
            Upload {
                name: "big.bin".into(),
                data: big.clone(),
            },
        ],
        done,
    });
    assert_eq!(
        answer(&slot).unwrap(),
        ["/tmp/up/shot 1.png", "/tmp/up/big.bin"]
    );
    assert_eq!(
        lk(&r.host).put,
        vec![
            ("shot 1.png".to_string(), b"png".to_vec()),
            ("big.bin".to_string(), big)
        ]
    );
    assert_eq!(r.host.calls("remote.put"), 3);
    assert_eq!(
        r.host.typed("h1"),
        vec![Intent::Paste(
            "'/tmp/up/shot 1.png' '/tmp/up/big.bin'".into()
        )],
        "one paste of the host's paths, nothing else"
    );
}

/// Rule 304: a file over the cap is never sent, and the person is told.
#[test]
fn a_file_over_the_cap_is_never_sent() {
    let r = rig();
    r.live_on();
    let (slot, done) = caught();
    r.live.send_errand(Errand::Put {
        files: vec![Upload {
            name: "huge.bin".into(),
            data: vec![0; FILE_MAX as usize + 1],
        }],
        done,
    });
    let Err(ErrandFail::Failed(why)) = answer(&slot) else {
        panic!("refused");
    };
    assert!(why.contains("over 32 MB"), "{why}");
    assert_eq!(r.host.calls("remote.put"), 0);
    assert!(r.host.typed("h1").is_empty());
}

/// Rule 28's spirit for files: work asked while the link is down is told so at once, and a tab whose link ended
/// tells at once too; nothing waits for a link that may take 30 s to come back.
#[test]
fn file_work_asked_while_the_link_is_down_or_ended_is_refused_at_once() {
    let r = rig();
    r.live_on();
    lk(&r.host).refuse = true;
    r.host.cut();
    wait_for("the link down", || r.live.link() == LinkState::Down);
    let (slot, done) = caught::<Fetched>();
    r.live.send_errand(Errand::Fetch {
        texts: vec!["a.txt".into()],
        done,
    });
    let Err(ErrandFail::Failed(why)) = answer(&slot) else {
        panic!("refused");
    };
    assert!(why.contains("down"), "{why}");
    let mut pty = r.pty;
    pty.kill();
    wait_for("the link ended", || r.live.link() == LinkState::Ended);
    let (slot, done) = caught::<Vec<String>>();
    r.live.send_errand(Errand::Put {
        files: Vec::new(),
        done,
    });
    assert!(matches!(answer(&slot), Err(ErrandFail::Failed(_))));
}
/// Remote sessions PR 6, rule 1207: the serve command for each host kind, as a table. A PC gets
/// `cmd /c "<path>" --serve-remote`, the one form both cmd and PowerShell login shells run and wait for; a path cmd
/// would strip the quotes of (`& < > ( ) @ ^ |`) or that either shell expands (`" % ! $` and a backquote) is refused,
/// except a leading `%LOCALAPPDATA%`. A Mac's sh (and an unknown kind) takes single quotes, where nothing expands.
#[test]
fn the_remote_command_table_quotes_each_hosts_shell() {
    let pc = Some(Os::Pc);
    let mac = Some(Os::Mac);
    for (kind, flare, want) in [
        (
            pc,
            r"C:\Users\max\AppData\Local\Flare\current\flare.exe",
            Some(r#"cmd /c "C:\Users\max\AppData\Local\Flare\current\flare.exe" --serve-remote"#),
        ),
        (
            pc,
            r"C:\Program Files\Flare\flare.exe",
            Some(r#"cmd /c "C:\Program Files\Flare\flare.exe" --serve-remote"#),
        ),
        (
            pc,
            r"D:\Tools\it's\flare.exe",
            Some(r#"cmd /c "D:\Tools\it's\flare.exe" --serve-remote"#),
        ),
        (
            pc,
            r"%LOCALAPPDATA%\StableFlare\current\FlareStable.exe",
            Some(r#"cmd /c "%LOCALAPPDATA%\StableFlare\current\FlareStable.exe" --serve-remote"#),
        ),
        (pc, "flare", Some(r#"cmd /c "flare" --serve-remote"#)),
        // cmd /c drops the quotes of a path with one of its special characters: the command would split.
        (pc, r"C:\Program Files (x86)\Flare\flare.exe", None),
        (pc, r"D:\Tools & Apps\flare.exe", None),
        (pc, r"C:\a@b\flare.exe", None),
        (pc, r"C:\a|b\flare.exe", None),
        (pc, r"C:\a<b>\flare.exe", None),
        (pc, r"C:\a!b\flare.exe", None),
        (pc, r"C:\a^b\flare.exe", None),
        (pc, r"C:\%USERPROFILE%\flare.exe", None),
        (pc, r"%USERPROFILE%\flare.exe", None),
        (pc, r"%LOCALAPPDATA%\%PATH%\flare.exe", None),
        (pc, r"C:\$env\flare.exe", None),
        (pc, r"C:\a`b\flare.exe", None),
        (pc, r#"C:\a"b\flare.exe"#, None),
        (
            mac,
            "/Applications/Flare.app/Contents/MacOS/flare",
            Some("'/Applications/Flare.app/Contents/MacOS/flare' --serve-remote"),
        ),
        (
            mac,
            "/Users/max/My Apps/$HOME/`x`/flare",
            Some("'/Users/max/My Apps/$HOME/`x`/flare' --serve-remote"),
        ),
        (
            None,
            "/opt/it's/flare",
            Some(r"'/opt/it'\''s/flare' --serve-remote"),
        ),
        (mac, "", None),
        (pc, "C:\\a\tb\\flare.exe", None),
    ] {
        assert_eq!(
            remote_command(kind, flare).as_deref(),
            want,
            "{kind:?} {flare}"
        );
    }
}
