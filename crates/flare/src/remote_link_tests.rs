//! Tests of the viewer's link (remote sessions PR 3), against an in-process host that keeps a tab's output numbered as
//! the real one does (a ring with a start, `gap` when a viewer asks for what fell out of it). No ssh, no other machine.

use super::*;
use flare_ctl::remote::parse_intents;
use flare_term::remote_pty::FolderFile;
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
    /// The links made (rule 1210: an attach not taken yet is tried again on the same link).
    connects: u32,
    /// The next this many attaches are answered `unavailable` (a new tab's shell still starting).
    attach_not_yet: u32,
    /// The next this many opens open their tab and then the link breaks before the answer reaches the viewer.
    lose_open_answers: u32,
    /// An older host: `remote.open` refuses `open_id` as an unknown field.
    no_open_id: bool,
    /// The tab each open id opened, and every open's params as sent.
    open_ids: HashMap<String, String>,
    opens: Vec<Value>,
    /// Rule 1213: every snapshot's params, an older host that refuses `styled`, the folders made by a put and an older
    /// host that refuses `folder` and `under`.
    snapshots: Vec<Value>,
    no_styled: bool,
    dirs: Vec<String>,
    no_folder: bool,
    /// Rule 1215: the files put with `exec` (where they went), and an older host that refuses `exec`.
    runnable: Vec<String>,
    no_exec: bool,
    /// Rule 1295: every attach's params as sent, and a host from before rule 1289 that refuses `ring`.
    attaches: Vec<Value>,
    no_ring: bool,
    /// Every call in order, with its params (the order of a size and the typing, rule 1292).
    log: Vec<(String, Value)>,
    /// Rule 1292: the size the host's window gives its tabs (None: 80x24), and an older host whose attach answer
    /// names no size.
    tab_size: Option<(u64, u64)>,
    no_attach_size: bool,
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
        st.log.push((m.name().to_string(), p.clone()));
        let tab = p["tab"].as_str().unwrap_or_default().to_string();
        match m {
            Method::RemoteOpen => {
                st.opens.push(p.clone());
                let open_id = p["open_id"].as_str().map(str::to_string);
                if open_id.is_some() && st.no_open_id {
                    return Err(CtlError::new(
                        code::BAD_PARAMS,
                        "open_id is not a field of remote.open",
                    ));
                }
                if let Some(t) = open_id.as_ref().and_then(|o| st.open_ids.get(o)) {
                    return Ok(json!({"tab": t}));
                }
                st.next += 1;
                let id = format!("h{}", st.next);
                st.tabs.insert(id.clone(), HostTab::default());
                if let Some(o) = open_id {
                    st.open_ids.insert(o, id.clone());
                }
                Ok(json!({"tab": id}))
            }
            Method::RemoteAttach => {
                st.attaches.push(p.clone());
                if p.get("ring").is_some() && st.no_ring {
                    return Err(CtlError::new(
                        code::BAD_PARAMS,
                        "ring is not a field of remote.attach",
                    ));
                }
                if st.attach_not_yet > 0 {
                    st.attach_not_yet -= 1;
                    return Err(CtlError::new(
                        code::UNAVAILABLE,
                        "that tab's shell is not running",
                    ));
                }
                let seq = st.tabs.get(&tab).ok_or_else(no_tab)?.out.len();
                st.attached.insert((conn, tab.clone()));
                if st.no_attach_size {
                    return Ok(json!({"tab": tab, "seq": seq}));
                }
                let (c, r) = st.tab_size.unwrap_or((80, 24));
                Ok(json!({"tab": tab, "cols": c, "rows": r, "seq": seq}))
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
                st.snapshots.push(p.clone());
                let styled = p.get("styled").is_some();
                if styled && st.no_styled {
                    return Err(CtlError::new(
                        code::BAD_PARAMS,
                        "styled is not a field of remote.snapshot",
                    ));
                }
                let t = st.tabs.get(&tab).ok_or_else(no_tab)?;
                let mut data = b"\x1b[H\x1b[2J\x1b[3J".to_vec();
                data.extend_from_slice(&t.out[t.start as usize..]);
                let (c, r) = st.tab_size.unwrap_or((80, 24));
                let mut v = json!({"seq": t.out.len(), "cols": c, "rows": r, "data": enc(&data)});
                if styled {
                    v["styled"] = json!(true);
                }
                Ok(v)
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
                let under = p["under"].as_str().map(str::to_string);
                if (p.get("folder").is_some() || under.is_some()) && st.no_folder {
                    return Err(CtlError::new(
                        code::BAD_PARAMS,
                        "folder is not a field of remote.put",
                    ));
                }
                if p.get("exec").is_some() && st.no_exec {
                    return Err(CtlError::new(
                        code::BAD_PARAMS,
                        "exec is not a field of remote.put",
                    ));
                }
                let place = |n: &str| match &under {
                    Some(u) => format!("{u}/{n}"),
                    None => n.to_string(),
                };
                if p["folder"] == json!(true) {
                    let n = flare_ctl::remote::put_inner_name(p["name"].as_str().unwrap());
                    st.dirs.push(place(&n));
                    let host = format!("/tmp/up/{}", place(&n));
                    return Ok(
                        json!({"tab": tab, "name": n, "done": true, "host_path": host, "paste": format!("'{host}'")}),
                    );
                }
                let chunk = p["chunk"].as_u64().unwrap();
                let chunks = p["chunks"].as_u64().unwrap();
                let data = base64::engine::general_purpose::STANDARD
                    .decode(p["data"].as_str().unwrap())
                    .unwrap();
                let name = if chunk == 0 {
                    let raw = p["name"].as_str().unwrap();
                    let n = if under.is_some() {
                        flare_ctl::remote::put_inner_name(raw)
                    } else {
                        flare_ctl::remote::put_name(raw)
                    };
                    st.putting = Some((n.clone(), data));
                    n
                } else {
                    let (n, so_far) = st.putting.as_mut().expect("a put under way");
                    assert_eq!(p["name"].as_str(), Some(n.as_str()), "the kept name");
                    so_far.extend(data);
                    n.clone()
                };
                if chunk + 1 < chunks {
                    assert!(p.get("exec").is_none(), "exec rides on the last chunk only");
                    return Ok(json!({"tab": tab, "name": name, "done": false}));
                }
                let (n, bytes) = st.putting.take().unwrap();
                if p["exec"] == json!(true) {
                    st.runnable.push(place(&n));
                }
                st.put.push((place(&n), bytes));
                let host = format!("/tmp/up/{}", place(&name));
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
        let answer = self.host.answer(self.conn, m, &p);
        let mut st = lk(&self.host);
        if m == Method::RemoteOpen && answer.is_ok() && st.lose_open_answers > 0 {
            // The host opened the tab; the link broke before its answer came back.
            st.lose_open_answers -= 1;
            st.epoch += 1;
            return Err(LinkError::Down("the link broke under the open".into()));
        }
        answer.map_err(LinkError::Refused)
    }
}

struct FakeConnector {
    host: FakeHost,
    conns: AtomicU64,
}

impl Connector for FakeConnector {
    fn connect(&self, _machine: &str) -> Result<Box<dyn Link>, LinkError> {
        let (refuse, epoch) = {
            let mut st = lk(&self.host);
            st.connects += 1;
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
        attach_retry: Duration::from_millis(5),
        attach_patience: Duration::from_millis(200),
        typing_kept: Duration::from_secs(30),
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
    rig_timed(host, host_tab, fast())
}

fn rig_timed(host: FakeHost, host_tab: Option<&str>, timing: Timing) -> Rig {
    rig_flags(host, host_tab, timing, false, false)
}

/// A rig whose spec is an imported row's: `host_sized_start` (rule 1292) and `ring_both` (rule 1295).
fn rig_flags(
    host: FakeHost,
    host_tab: Option<&str>,
    timing: Timing,
    host_sized_start: bool,
    ring_both: bool,
) -> Rig {
    let live = flare_term::remote_pty::RemoteLive::new(host_tab.map(str::to_string), 0);
    let sp = RemoteSpawner::new(
        Arc::new(FakeConnector {
            host: host.clone(),
            conns: AtomicU64::new(1),
        }),
        timing,
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
            host_sized_start,
            ring_both,
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
        ..Default::default()
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
            "-o",
            "ConnectTimeout=5",
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

/// Rule 1291: the watch link's ssh is a tab link's with one fixed word more, `--no-start` after `--serve-remote`, so a
/// poll never starts Flare on the host; a path refused for a tab link is refused for it too, and the e2e's loopback
/// gets the same word at its end.
#[test]
fn the_watch_link_runs_serve_remote_no_start_and_nothing_else_new() {
    let m = |kind, flare: &str| Machine {
        id: "m".into(),
        name: "M".into(),
        kind,
        ssh: "mac-mini".into(),
        user: Some("max".into()),
        flare: flare.into(),
        ..Default::default()
    };
    for (kind, flare) in [
        (Some(Os::Mac), "/Applications/Fl are.app/flare"),
        (Some(Os::Pc), r"C:\Program Files\Flare\flare.exe"),
        (None, "it's/flare"),
    ] {
        let tab = ssh_argv("ssh", &m(kind, flare)).unwrap();
        let watch = ssh_argv_watch("ssh", &m(kind, flare)).unwrap();
        assert_eq!(watch.len(), tab.len(), "{flare}");
        assert_eq!(watch[..tab.len() - 1], tab[..tab.len() - 1], "{flare}");
        assert_eq!(
            watch.last().unwrap(),
            &format!("{} --no-start", tab.last().unwrap()),
            "{flare}"
        );
        assert!(
            !tab.last().unwrap().contains("--no-start"),
            "a tab's link may start Flare"
        );
    }
    assert_eq!(
        watch_command(Some(Os::Pc), r"C:\Flare\flare.exe").as_deref(),
        Some(r#"cmd /c "C:\Flare\flare.exe" --serve-remote --no-start"#)
    );
    assert_eq!(watch_command(Some(Os::Pc), r#"C:\a"b\flare.exe"#), None);
    assert_eq!(ssh_argv_watch("ssh", &m(Some(Os::Mac), "fla\nre")), None);
    let profile = tempfile::tempdir().unwrap();
    let loop_argv = vec!["flare".to_string(), "--serve-remote".to_string()];
    assert_eq!(
        SshConnector::watching(profile.path(), Some(loop_argv.clone()))
            .argv("mac")
            .unwrap(),
        ["flare", "--serve-remote", "--no-start"]
    );
    assert_eq!(
        SshConnector::new(profile.path(), Some(loop_argv.clone()))
            .argv("mac")
            .unwrap(),
        loop_argv
    );
}

/// Rule 1249: a machine with an address, a port, Flare's key and Flare's known-hosts file runs ssh at that address with
/// that key only and that file only (strict checking, no question), each value one argv item; a path with a space is
/// quoted inside its option's value; a field that could be an option, or break ssh's parsing, refuses the command.
#[test]
fn a_paired_machines_ssh_uses_its_address_port_key_and_known_hosts_as_argv_items() {
    let paired = Machine {
        id: "pc".into(),
        name: "Max's PC".into(),
        kind: Some(Os::Pc),
        ssh: "192.168.0.139".into(),
        user: Some("max".into()),
        flare: "flare".into(),
        host: Some("192.168.0.139".into()),
        port: Some(2222),
        key: Some("/Users/max/Library/Flare/id_flare_ed25519".into()),
        known_hosts: Some(r"C:\Users\John Doe\AppData\Flare\known_hosts".into()),
        host_key: None,
    };
    assert_eq!(
        ssh_argv("ssh", &paired).unwrap(),
        [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "IdentitiesOnly=yes",
            "-i",
            "/Users/max/Library/Flare/id_flare_ed25519",
            "-o",
            r#"UserKnownHostsFile="C:\Users\John Doe\AppData\Flare\known_hosts""#,
            "-o",
            "StrictHostKeyChecking=yes",
            "-p",
            "2222",
            "-T",
            "-l",
            "max",
            "--",
            "192.168.0.139",
            r#"cmd /c "flare" --serve-remote"#,
        ]
    );
    // The address wins over the alias; a machine with no key and no file has neither option.
    let typed = Machine {
        ssh: "old-alias".into(),
        host: Some("studio.local".into()),
        ..Machine::default()
    };
    let argv = machine_opts(&typed).unwrap();
    assert_eq!(argv, ["-o", "ConnectTimeout=5"]);
    assert_eq!(
        machine_dest(&typed),
        ["--".to_string(), "studio.local".to_string()]
    );
    let with = |f: fn(&mut Machine)| {
        let mut m = paired.clone();
        f(&mut m);
        ssh_argv("ssh", &m)
    };
    assert_eq!(with(|m| m.host = Some("-oProxyCommand=x".into())), None);
    assert_eq!(with(|m| m.key = Some("-i/etc/passwd".into())), None);
    assert_eq!(with(|m| m.key = Some("C:\\a%h\\key".into())), None);
    assert_eq!(with(|m| m.key = Some("${HOME}/key".into())), None);
    assert!(
        with(|m| m.key = Some(r"C:\Users\a$b\key".into())).is_some(),
        "a lone $ is a path's own"
    );
    assert_eq!(with(|m| m.known_hosts = Some("a\"b".into())), None);
    assert_eq!(with(|m| m.known_hosts = Some("a\nb".into())), None);
}

/// Rule 1277: before ssh reads Flare's key, the key is made private again when others may read it (Windows' OpenSSH
/// refuses it otherwise: "UNPROTECTED PRIVATE KEY FILE", then "Permission denied (publickey)"). A tab's link builds its
/// argv through it, as does the check of Settings > Machines and the Preview tunnel; the e2e's loopback has no key.
#[test]
fn flares_key_is_made_private_before_ssh_runs_with_it() {
    let profile = tempfile::tempdir().unwrap();
    let files = crate::pair_keys::PairFiles::new(profile.path());
    files.ensure_key("pc").unwrap();
    let key = files.key_path();
    let private = || flare_win::private_file::is_private(&key).unwrap();
    let m = Machine {
        id: "mac".into(),
        name: "Mac".into(),
        kind: Some(Os::Mac),
        ssh: "192.168.0.120".into(),
        flare: "flare".into(),
        host: Some("192.168.0.120".into()),
        key: Some(key.to_string_lossy().into_owned()),
        ..Machine::default()
    };
    // The argv a check or a tunnel builds.
    crate::pair_keys::tests::loosen(&key);
    assert!(!private());
    let argv = ssh_argv_ready("ssh", &m).unwrap();
    assert!(argv.contains(&key.to_string_lossy().into_owned()));
    assert!(private(), "made private before ssh runs");
    // A tab's link: the connector reads machines.json and builds the same argv.
    flare_settings::atomic::write_atomic(
        &machines_file(profile.path()),
        &crate::machines::machines_json(std::slice::from_ref(&m)),
    )
    .unwrap();
    crate::pair_keys::tests::loosen(&key);
    let real = SshConnector::new(profile.path(), None).argv("mac").unwrap();
    assert_eq!(real[1..], argv[1..]);
    assert!(private(), "a tab's link made it private too");
    // The e2e's loopback runs no ssh and reads no key.
    crate::pair_keys::tests::loosen(&key);
    let loop_argv = vec!["flare".to_string(), "--serve-remote".to_string()];
    let e2e = SshConnector::new(profile.path(), Some(loop_argv.clone()));
    assert_eq!(e2e.argv("mac").unwrap(), loop_argv);
    assert!(!private(), "the loopback leaves the key alone");
    // The tunnel and the check call it before their ssh too.
    let tunnel = include_str!("remote_tunnel.rs");
    let open = &tunnel[tunnel.find("pub fn open_ssh(").unwrap()..];
    assert!(
        open.find("keep_key_private(&ask.machine)").unwrap() < open.find("tunnel_argv(").unwrap()
    );
    assert!(include_str!("machines_page.rs").contains("remote_link::ssh_argv_ready("));
}

/// Rule 1249: `machines.json` reads and writes the optional fields; a machine with an address needs no alias; a field
/// that is there but not well formed leaves the machine out, never half used.
#[test]
fn machines_json_reads_and_writes_the_address_port_key_and_known_hosts() {
    let raw = r#"{"machines":[
        {"id":"pc","name":"Max's PC","kind":"pc","host":"192.168.0.139","port":2222,"user":"max",
         "key":"C:/Flare/id_flare_ed25519","known_hosts":"C:/Flare/known_hosts"},
        {"id":"bad-port","ssh":"x","port":70000},
        {"id":"zero-port","ssh":"x","port":0},
        {"id":"bad-host","ssh":"x","host":"-oX"},
        {"id":"bad-key","ssh":"x","key":"%d/key"},
        {"id":"no-where"}
    ]}"#;
    let ms = parse_machines(raw);
    assert_eq!(ms.len(), 1, "{ms:?}");
    let pc = &ms[0];
    assert_eq!(
        pc.ssh, "192.168.0.139",
        "the address stands in for the alias"
    );
    assert_eq!(pc.target(), "192.168.0.139");
    assert_eq!(pc.port, Some(2222));
    assert_eq!(pc.key.as_deref(), Some("C:/Flare/id_flare_ed25519"));
    assert_eq!(pc.known_hosts.as_deref(), Some("C:/Flare/known_hosts"));
    let back = parse_machines(&crate::machines::machines_json(&ms));
    assert_eq!(back, ms, "written and read back the same");
    // A machine with none of them is written as before.
    let plain = parse_machines(r#"{"machines":[{"id":"m","ssh":"m"}]}"#);
    let text = crate::machines::machines_json(&plain);
    for k in ["host", "port", "key", "known_hosts"] {
        assert!(!text.contains(&format!("\"{k}\"")), "{k}: {text}");
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
            host_sized_start: false,
            ring_both: false,
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
                exec: false,
            },
            Upload {
                name: "big.bin".into(),
                data: big.clone(),
                exec: false,
            },
        ],
        folders: Vec::new(),
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
            exec: false,
        }],
        folders: Vec::new(),
        done,
    });
    let Err(ErrandFail::Failed(why)) = answer(&slot) else {
        panic!("refused");
    };
    assert!(why.contains("over 32 MB"), "{why}");
    assert_eq!(r.host.calls("remote.put"), 0);
    assert!(r.host.typed("h1").is_empty());
}

fn coloured_tab(host: &FakeHost) {
    lk(host).tabs.insert(
        "h1".into(),
        HostTab {
            out: b"\x1b[31mred\x1b[0m\x1b]0;title\x07 ok".to_vec(),
            ..HostTab::default()
        },
    );
}

/// Rule 1213: a fresh tab asks its snapshot styled and shows the host's colours at once (a title still never rides on
/// it); an older host refuses the param, the snapshot goes again plain on the same link, and the tab's later snapshots
/// leave it out.
#[test]
fn a_snapshot_is_asked_styled_and_an_older_hosts_refusal_falls_back_to_plain() {
    let host = FakeHost::default();
    coloured_tab(&host);
    let r = rig_with(host, Some("h1"));
    r.live_on();
    wait_for("the colour", || r.text().contains("\x1b[31mred"));
    assert!(!r.text().contains("\x1b]0;"), "{:?}", r.text());
    assert_eq!(
        lk(&r.host).snapshots,
        vec![json!({"tab": "h1", "styled": true})]
    );

    let old = FakeHost::default();
    coloured_tab(&old);
    lk(&old).no_styled = true;
    let r = rig_with(old, Some("h1"));
    r.live_on();
    wait_for("the text", || r.text().contains("red"));
    assert!(!r.text().contains("\x1b[31m"), "{:?}", r.text());
    assert_eq!(r.host.calls("remote.attach"), 1, "the same link");
    // The output falls out of the ring: the next snapshot is asked plain at once.
    {
        let mut st = lk(&r.host);
        let t = st.tabs.get_mut("h1").unwrap();
        t.out.extend_from_slice(b" more");
        t.start = t.out.len() as u64;
    }
    wait_for("a second snapshot", || lk(&r.host).snapshots.len() >= 3);
    assert_eq!(
        lk(&r.host).snapshots[..3],
        [
            json!({"tab": "h1", "styled": true}),
            json!({"tab": "h1"}),
            json!({"tab": "h1"})
        ]
    );
}

fn folder_file(dir: &[&str], name: &str, path: &Path, size: u64) -> FolderFile {
    FolderFile {
        dir: dir.iter().map(|s| s.to_string()).collect(),
        name: name.into(),
        path: path.to_path_buf(),
        size,
        exec: false,
    }
}

/// Rule 1213: a dropped folder goes after the files: the folder, the folders in it parents first, then each file, read
/// here as it goes, into its own folder there (a dot file keeps its name). Only the folder's host path is pasted, with
/// the files', as one paste.
#[test]
fn a_dropped_folder_is_copied_whole_and_only_its_path_is_pasted() {
    let r = rig();
    r.live_on();
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a.txt");
    std::fs::write(&a, b"A").unwrap(); // sync-ok: test code only
    let big = vec![7u8; FILE_CHUNK + 3];
    let b = d.path().join("b.bin");
    std::fs::write(&b, &big).unwrap(); // sync-ok: test code only
    let folder = UploadFolder {
        name: "my proj".into(),
        dirs: vec![vec!["src".into()], vec!["src".into(), ".git".into()]],
        files: vec![
            folder_file(&[], ".env", &a, 1),
            folder_file(&["src"], "b.bin", &b, big.len() as u64),
        ],
    };
    let (slot, done) = caught();
    r.live.send_errand(Errand::Put {
        files: vec![Upload {
            name: "one.txt".into(),
            data: b"1".to_vec(),
            exec: false,
        }],
        folders: vec![folder],
        done,
    });
    assert_eq!(
        answer(&slot).unwrap(),
        ["/tmp/up/one.txt", "/tmp/up/my proj"]
    );
    {
        let st = lk(&r.host);
        assert_eq!(st.dirs, ["my proj", "my proj/src", "my proj/src/.git"]);
        assert_eq!(
            st.put,
            vec![
                ("one.txt".to_string(), b"1".to_vec()),
                ("my proj/.env".to_string(), b"A".to_vec()),
                ("my proj/src/b.bin".to_string(), big),
            ]
        );
    }
    assert_eq!(
        r.host.typed("h1"),
        vec![Intent::Paste("'/tmp/up/one.txt' '/tmp/up/my proj'".into())]
    );
}

/// Rule 1215: a runnable file says `exec` on its last chunk only, alone or in a dropped folder; an older host refuses the
/// unknown param once, the chunk goes again without it, and the rest of the errand leaves it out, so every file still
/// lands.
#[test]
fn a_runnable_file_says_exec_and_an_older_host_takes_it_without() {
    let d = tempfile::tempdir().unwrap();
    let run = d.path().join("gradlew");
    std::fs::write(&run, b"#!/bin/sh\n").unwrap(); // sync-ok: test code only
    let big = vec![5u8; FILE_CHUNK + 2];
    let errand = |slot_done: Done<Vec<String>>| {
        let mut file = folder_file(&[], "gradlew", &run, 10);
        file.exec = true;
        Errand::Put {
            files: vec![
                Upload {
                    name: "build.sh".into(),
                    data: big.clone(),
                    exec: true,
                },
                Upload {
                    name: "a.txt".into(),
                    data: b"a".to_vec(),
                    exec: false,
                },
            ],
            folders: vec![UploadFolder {
                name: "proj".into(),
                dirs: Vec::new(),
                files: vec![file],
            }],
            done: slot_done,
        }
    };
    let r = rig();
    r.live_on();
    let (slot, done) = caught();
    r.live.send_errand(errand(done));
    assert!(answer(&slot).is_ok());
    assert_eq!(lk(&r.host).runnable, ["build.sh", "proj/gradlew"]);
    assert_eq!(r.host.calls("remote.put"), 5);

    let old = rig();
    old.live_on();
    lk(&old.host).no_exec = true;
    let (slot, done) = caught();
    old.live.send_errand(errand(done));
    assert_eq!(
        answer(&slot).unwrap(),
        ["/tmp/up/build.sh", "/tmp/up/a.txt", "/tmp/up/proj"]
    );
    let st = lk(&old.host);
    assert!(st.runnable.is_empty());
    let names: Vec<&str> = st.put.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["build.sh", "a.txt", "proj/gradlew"]);
    assert_eq!(st.put[0].1, big, "the refused chunk went again whole");
    drop(st);
    assert_eq!(
        old.host.calls("remote.put"),
        6,
        "one refusal, then never exec again"
    );
}

/// Rule 1213: a folder over the caps copies nothing and says so in one line; so does a host that takes single files
/// only (an older Flare refuses `folder`), after its one refusal.
#[test]
fn a_folder_over_the_caps_or_to_an_older_host_copies_nothing() {
    let r = rig();
    r.live_on();
    let nowhere = Path::new("nowhere");
    let refused_with = |folder: UploadFolder| {
        let (slot, done) = caught::<Vec<String>>();
        r.live.send_errand(Errand::Put {
            files: Vec::new(),
            folders: vec![folder],
            done,
        });
        let Err(ErrandFail::Failed(why)) = answer(&slot) else {
            panic!("refused");
        };
        why
    };
    let many = UploadFolder {
        name: "many".into(),
        dirs: Vec::new(),
        files: (0..=FOLDER_FILES_MAX)
            .map(|i| folder_file(&[], &format!("f{i}"), nowhere, 1))
            .collect(),
    };
    let why = refused_with(many);
    assert!(
        why.contains("5000 files") || why.contains("over 5000"),
        "{why}"
    );
    let heavy = UploadFolder {
        name: "heavy".into(),
        dirs: Vec::new(),
        files: vec![
            folder_file(&[], "a", nowhere, FOLDER_MAX / 2 + 1),
            folder_file(&[], "b", nowhere, FOLDER_MAX / 2 + 1),
        ],
    };
    assert!(refused_with(heavy).contains("over 200 MB"));
    assert_eq!(r.host.calls("remote.put"), 0, "nothing was copied");
    lk(&r.host).no_folder = true;
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("x.txt");
    std::fs::write(&f, b"x").unwrap(); // sync-ok: test code only
    let why = refused_with(UploadFolder {
        name: "x".into(),
        dirs: Vec::new(),
        files: vec![folder_file(&[], "x.txt", &f, 1)],
    });
    assert!(why.contains("update it there"), "{why}");
    assert_eq!(r.host.calls("remote.put"), 1, "the one refused call");
    assert!(lk(&r.host).put.is_empty() && lk(&r.host).dirs.is_empty());
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
        folders: Vec::new(),
        done,
    });
    assert!(matches!(answer(&slot), Err(ErrandFail::Failed(_))));
}
/// Remote sessions PR 6, rule 1207: the serve command for each host kind, as a table. A PC gets
/// `cmd /c "<path>" --serve-remote`, the one form both cmd and PowerShell login shells run and wait for; a path cmd
/// would strip the quotes of (`& < > ( ) @ ^ |`) or that either shell expands (`" % ! $` and a backquote) is refused,
/// except a leading `%LOCALAPPDATA%` or `$env:LOCALAPPDATA`, and so is a curly quote (rule 1208). A Mac's sh (and an
/// unknown kind) takes single quotes, where nothing expands.
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
        // Rule 1208: PowerShell's name for the same folder, expanded and quoted by a PowerShell login shell.
        (
            pc,
            r"$env:LOCALAPPDATA\Flare\current\Flare.exe",
            Some(r#"cmd /c "$env:LOCALAPPDATA\Flare\current\Flare.exe" --serve-remote"#),
        ),
        (pc, "flare", Some(r#"cmd /c "flare" --serve-remote"#)),
        (pc, r"$env:LOCALAPPDATA\$x\flare.exe", None),
        (pc, r"C:\x$env:LOCALAPPDATA\flare.exe", None),
        // PowerShell ends a double-quoted string at a curly quote too (rule 1208): `calc` would run on its own.
        (pc, "C:\\x\u{201D}; calc; \u{201D}\\flare.exe", None),
        (pc, "C:\\x\u{201C}\\flare.exe", None),
        (pc, "C:\\x\u{201E}\\flare.exe", None),
        (pc, "C:\\x\u{2018}\\flare.exe", None),
        (pc, "%LOCALAPPDATA%\\x\u{201F}\\flare.exe", None),
        // Other letters outside ASCII are a path's own (a user named Jose with an accent).
        (
            pc,
            "C:\\Users\\Jos\u{E9}\\flare.exe",
            Some("cmd /c \"C:\\Users\\Jos\u{E9}\\flare.exe\" --serve-remote"),
        ),
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

// ---- the link's edge cases (rule 1210) ----

/// Rule 1210: an attach the host answers `unavailable` right after the open (the new tab's shell is still starting) is
/// tried again on the same link, with no new connect.
#[test]
fn an_attach_the_host_cannot_take_yet_is_tried_again_on_the_same_link() {
    let host = FakeHost::default();
    lk(&host).attach_not_yet = 3;
    let r = rig_with(host, None);
    r.live_on();
    assert_eq!(r.host.calls("remote.attach"), 4);
    assert_eq!(lk(&r.host).connects, 1, "no second connect");
    assert_eq!(r.host.calls("remote.open"), 1);
    r.host.print("h1", b"ready\r\n");
    wait_for("the output", || r.text().contains("ready"));
}

/// Rule 1210: an attach still not taken after the patience drops the link, and the next link attaches.
#[test]
fn an_attach_still_unavailable_after_the_patience_drops_the_link_and_the_next_one_attaches() {
    let host = FakeHost::default();
    lk(&host).attach_not_yet = u32::MAX;
    let r = rig_with(host, None);
    wait_for("a second connect", || lk(&r.host).connects >= 2);
    lk(&r.host).attach_not_yet = 0;
    r.live_on();
    assert_eq!(r.host.calls("remote.open"), 1, "the tab is opened once");
    assert_eq!(lk(&r.host).tabs.len(), 1);
}

/// Rule 1210: the link breaks after the host opened the tab and before its answer came back. The open goes again with
/// the same open id on the next link, and the host answers with the tab it opened: one tab, not two.
#[test]
fn an_open_whose_answer_was_lost_opens_no_second_tab() {
    let host = FakeHost::default();
    lk(&host).lose_open_answers = 1;
    let r = rig_with(host, None);
    r.live_on();
    assert_eq!(r.live.host_tab().as_deref(), Some("h1"));
    assert_eq!(
        r.host.calls("remote.open"),
        2,
        "sent again after the lost answer"
    );
    assert_eq!(lk(&r.host).tabs.len(), 1, "no second tab on the host");
    let opens = lk(&r.host).opens.clone();
    let id = opens[0]["open_id"].as_str().expect("an open id");
    assert!(flare_ctl::remote::valid_open_id(id), "{id}");
    assert_eq!(
        opens[1]["open_id"], opens[0]["open_id"],
        "the same id again"
    );
    // Another tab has an id of its own.
    let other = rig();
    other.live_on();
    assert_ne!(lk(&other.host).opens[0]["open_id"], opens[0]["open_id"]);
}

/// Rule 1210: an older host refuses `open_id` as an unknown field; the viewer opens again without it, and the tab works.
#[test]
fn an_older_host_that_refuses_the_open_id_still_opens_the_tab() {
    let host = FakeHost::default();
    lk(&host).no_open_id = true;
    let r = rig_with(host, None);
    r.live_on();
    assert_eq!(r.live.host_tab().as_deref(), Some("h1"));
    let opens = lk(&r.host).opens.clone();
    assert_eq!(opens.len(), 2);
    assert!(opens[0].get("open_id").is_some());
    assert_eq!(opens[1], json!({"folder": "/Users/max/proj"}));
    assert_eq!(lk(&r.host).tabs.len(), 1);
}

/// Rule 1210: what is typed while the link is down is kept and reaches the host in order after the next attach, before
/// what is typed once the link is back.
#[test]
fn typing_while_the_link_is_down_is_sent_in_order_after_the_next_attach() {
    let mut r = rig();
    r.live_on();
    lk(&r.host).refuse = true;
    r.host.cut();
    wait_for("the link down", || r.live.link() == LinkState::Down);
    r.writer.write_all(b"one ").unwrap();
    std::thread::sleep(Duration::from_millis(30)); // sync-ok: test code only, never the app
    r.writer.write_all(b"two").unwrap();
    std::thread::sleep(Duration::from_millis(30)); // sync-ok: test code only, never the app
    lk(&r.host).refuse = false;
    wait_for("the kept typing", || r.host.typed("h1").len() >= 2);
    r.writer.write_all(b" three").unwrap();
    wait_for("the live typing", || r.host.typed("h1").len() >= 3);
    assert_eq!(
        r.host.typed("h1"),
        vec![
            Intent::Text("one ".into()),
            Intent::Text("two".into()),
            Intent::Text(" three".into()),
        ]
    );
    assert!(!r.text().contains("not sent"), "{:?}", r.text());
}

/// Rule 1210: typing older than the kept age when the link comes back is dropped, the rest is sent, and the tab says
/// in one line that some typing was not sent.
#[test]
fn typing_older_than_the_kept_age_is_dropped_and_the_tab_says_so() {
    let t = Timing {
        typing_kept: Duration::from_millis(300),
        ..fast()
    };
    let mut r = rig_timed(FakeHost::default(), None, t);
    r.live_on();
    lk(&r.host).refuse = true;
    r.host.cut();
    wait_for("the link down", || r.live.link() == LinkState::Down);
    r.writer.write_all(b"stale").unwrap();
    std::thread::sleep(Duration::from_millis(600)); // sync-ok: test code only, never the app
    r.writer.write_all(b"fresh").unwrap();
    lk(&r.host).refuse = false;
    wait_for("the fresh typing", || !r.host.typed("h1").is_empty());
    wait_for("the note", || r.text().contains("was not sent"));
    assert_eq!(r.host.typed("h1"), vec![Intent::Text("fresh".into())]);
    assert_eq!(r.text().matches("was not sent").count(), 1);
}

/// Rule 1210: typing over the cap while the link is down is dropped, with everything typed after it, and the tab says
/// so; nothing of it reaches the host.
#[test]
fn typing_over_the_cap_while_the_link_is_down_is_dropped_and_the_tab_says_so() {
    let mut r = rig();
    r.live_on();
    lk(&r.host).refuse = true;
    r.host.cut();
    wait_for("the link down", || r.live.link() == LinkState::Down);
    r.writer.write_all(b"kept").unwrap();
    std::thread::sleep(Duration::from_millis(30)); // sync-ok: test code only, never the app
    r.writer
        .write_all("x".repeat(KEPT_TYPING_MAX).as_bytes())
        .unwrap();
    std::thread::sleep(Duration::from_millis(30)); // sync-ok: test code only, never the app
    r.writer.write_all(b"after").unwrap();
    std::thread::sleep(Duration::from_millis(30)); // sync-ok: test code only, never the app
    lk(&r.host).refuse = false;
    wait_for("the note", || r.text().contains("was not sent"));
    wait_for("the kept typing", || !r.host.typed("h1").is_empty());
    assert_eq!(r.host.typed("h1"), vec![Intent::Text("kept".into())]);
}

/// Rule 1210: the host tab is gone when the link comes back: the tab ends, and what was typed meanwhile is typed
/// nowhere.
#[test]
fn typing_kept_for_a_host_tab_that_is_gone_is_typed_nowhere() {
    let mut r = rig();
    r.live_on();
    lk(&r.host).refuse = true;
    r.host.cut();
    wait_for("the link down", || r.live.link() == LinkState::Down);
    r.writer.write_all(b"rm -rf build").unwrap();
    std::thread::sleep(Duration::from_millis(30)); // sync-ok: test code only, never the app
    {
        let mut st = lk(&r.host);
        st.tabs.remove("h1");
        st.refuse = false;
    }
    wait_for("the end", || r.live.link() == LinkState::Ended);
    assert_eq!(r.host.calls("remote.input"), 0);
    assert_eq!(r.host.calls("remote.open"), 1, "no new tab to type into");
}

/// Rule 1210, the keep itself: order, the age dropping the oldest, the cap dropping the rest of the outage, one note.
#[test]
fn the_kept_typing_keeps_order_and_drops_old_and_over_the_cap_with_no_hole() {
    let t0 = Instant::now();
    let age = Duration::from_secs(30);
    let text = |s: &str| RemoteIntent::Text(s.into());
    let mut k = KeptTyping::default();
    k.keep(vec![text("a"), text("b")], &[t0, t0], t0, age);
    k.keep(vec![text("c")], &[], t0, age);
    assert_eq!(
        k.take(t0, age),
        (vec![text("a"), text("b"), text("c")], false)
    );
    assert_eq!(k.take(t0, age), (vec![], false), "taken once");

    let later = t0 + Duration::from_secs(20);
    k.keep(vec![text("old")], &[t0], t0, age);
    k.keep(vec![text("new")], &[later], later, age);
    assert_eq!(
        k.take(t0 + Duration::from_secs(40), age),
        (vec![text("new")], true)
    );

    k.keep(vec![text("first")], &[t0], t0, age);
    k.keep(vec![text(&"x".repeat(KEPT_TYPING_MAX))], &[t0], t0, age);
    k.keep(vec![text("then")], &[t0], t0, age);
    assert_eq!(k.take(t0, age), (vec![text("first")], true));
    k.keep(vec![text("next outage")], &[t0], t0, age);
    assert_eq!(k.take(t0, age), (vec![text("next outage")], false));

    k.keep(vec![text("gone")], &[t0], t0, age);
    k.clear();
    assert_eq!(k.take(t0, age), (vec![], false));
}

// ---- import remote sessions PR 3 (rules 1292, 1295) ----

/// A host with one tab of its own, `tab`, as a linked machine's Flare lists it.
fn host_with(tab: &str) -> FakeHost {
    let host = FakeHost::default();
    lk(&host).tabs.insert(tab.into(), HostTab::default());
    host
}

/// The sizes the host's tab was sent, in order.
fn sizes(host: &FakeHost, tab: &str) -> Vec<(u64, u64)> {
    lk(host)
        .tabs
        .get(tab)
        .map(|t| t.sizes.clone())
        .unwrap_or_default()
}

/// Lets the worker run a few polls, so a call it should not make had its chance.
fn settle() {
    std::thread::sleep(Duration::from_millis(120)); // sync-ok: test code only, never the app
}

/// Breaks every link once, and waits for the tab to attach again.
fn reconnect(r: &Rig) {
    let attaches = r.host.calls("remote.attach");
    lk(&r.host).refuse = true;
    r.host.cut();
    wait_for("the link down", || r.live.link() != LinkState::Live);
    lk(&r.host).refuse = false;
    wait_for("the attach again", || {
        r.host.calls("remote.attach") > attaches
    });
    r.live_on();
}

/// Rules 1292 and 1193: an imported row attaches to its host tab (no `remote.open`, no resume) and, while the host's
/// window sizes it, sends no size on attach, a refit, a tab switch or a reconnect. The first key here ends the hold: the
/// newest size goes first, then the key.
#[test]
fn an_imported_claude_tab_sends_no_size_on_attach_refit_switch_or_reconnect_and_one_before_the_first_key()
 {
    let mut r = rig_flags(host_with("h7"), Some("h7"), fast(), true, true);
    r.live_on();
    assert_eq!(r.host.calls("remote.open"), 0, "an import never opens");
    assert!(lk(&r.host).opens.is_empty(), "nor resumes");
    r.pty.resize(100, 30).unwrap();
    settle();
    assert!(
        sizes(&r.host, "h7").is_empty(),
        "no size on attach or a refit"
    );
    reconnect(&r);
    r.pty.resize(110, 32).unwrap();
    settle();
    assert!(
        sizes(&r.host, "h7").is_empty(),
        "none on a reconnect either"
    );
    r.writer.write_all(b"x").unwrap();
    wait_for("the key", || !r.host.typed("h7").is_empty());
    assert_eq!(sizes(&r.host, "h7"), [(110, 32)], "the newest size, once");
    let log = lk(&r.host).log.clone();
    let at = |m: &str| log.iter().position(|(n, _)| n == m).unwrap();
    assert!(
        at("remote.resize") < at("remote.input"),
        "the size goes before the key"
    );
    // From then on the tab sizes as any followed tab does.
    r.pty.resize(90, 20).unwrap();
    wait_for("the next size", || {
        sizes(&r.host, "h7").last() == Some(&(90, 20))
    });
}

/// Rule 1292: a click on the fitted view (a retake) ends the hold too, and sends the size it kept.
#[test]
fn a_retake_ends_an_imported_tabs_size_hold() {
    let r = rig_flags(host_with("h7"), Some("h7"), fast(), true, false);
    r.live_on();
    settle();
    assert!(sizes(&r.host, "h7").is_empty());
    r.live.retake_size();
    wait_for("the size", || sizes(&r.host, "h7") == [(80, 24)]);
}

/// Rule 1292: an imported plain shell (no session, so no fitted view) sends its size on attach as a followed tab does.
#[test]
fn an_imported_plain_shell_sends_its_size_on_attach_as_a_followed_tab_does() {
    let r = rig_flags(host_with("h7"), Some("h7"), fast(), false, true);
    r.live_on();
    wait_for("the size", || sizes(&r.host, "h7") == [(80, 24)]);
    assert_eq!(r.host.calls("remote.open"), 0);
}

/// Rule 1295: every attach of an imported row asks `ring: "both"`, reconnects too; a tab opened from here never does.
#[test]
fn an_imported_rows_attach_sends_ring_both_and_a_tab_opened_from_here_does_not() {
    let r = rig_flags(host_with("h7"), Some("h7"), fast(), true, true);
    r.live_on();
    reconnect(&r);
    let attaches = lk(&r.host).attaches.clone();
    assert_eq!(attaches.len(), 2);
    for a in &attaches {
        assert_eq!(a, &json!({"tab": "h7", "ring": "both"}));
    }
    let opened = rig();
    opened.live_on();
    reconnect(&opened);
    for a in lk(&opened.host).attaches.iter() {
        assert_eq!(a, &json!({"tab": "h1"}), "rule 295 as before");
    }
}

/// Rule 1295: a host from before rule 1289 refuses `ring`; the attach goes again without it on the same link, and the
/// reconnects leave it out.
#[test]
fn an_older_host_that_refuses_ring_is_attached_without_it() {
    let host = host_with("h7");
    lk(&host).no_ring = true;
    let r = rig_flags(host, Some("h7"), fast(), false, true);
    r.live_on();
    assert_eq!(lk(&r.host).connects, 1, "on the same link");
    reconnect(&r);
    let attaches = lk(&r.host).attaches.clone();
    assert_eq!(
        attaches,
        [
            json!({"tab": "h7", "ring": "both"}),
            json!({"tab": "h7"}),
            json!({"tab": "h7"}),
        ]
    );
}

// ---- rule 1292: a held tab shows the host's grid ----

/// A viewer tab's real terminal (its `TermHandle` and session) over the fake host, at this window's size, attached to
/// the host's `h7` as an imported row (`held`: a Claude tab the host's window sizes).
fn viewer(
    host: &FakeHost,
    held: bool,
    cols: usize,
    rows: usize,
) -> Arc<flare_term::handle::TermHandle> {
    let spawner = Arc::new(flare_term::pty::RoutingSpawner {
        local: flare_term::fake::FakeSpawner::new(),
        remote: Arc::new(RemoteSpawner::new(
            Arc::new(FakeConnector {
                host: host.clone(),
                conns: AtomicU64::new(1),
            }),
            fast(),
        )),
    });
    let mgr = flare_term::session::SessionManager::new(
        spawner,
        flare_term::fake::FakeShells::new(),
        flare_term::session::SessionOptions::default(),
    );
    let h = flare_term::handle::TermHandle::new("v1", mgr, cols, rows);
    h.start(flare_term::handle::StartOptions {
        remote: Some(RemoteSpec {
            machine: "mac".into(),
            folder: "/Users/max/proj".into(),
            resume: None,
            live: flare_term::remote_pty::RemoteLive::new(Some("h7".into()), 0),
            host_sized_start: held,
            ring_both: true,
        }),
        ..Default::default()
    });
    h
}

/// A frame a Claude-like screen draws for its own 120 columns: a line longer than 80, text placed past column 80, and
/// the prompt on the row below.
fn wide_frame() -> Vec<u8> {
    let mut f = b"\x1b[H\x1b[2J".to_vec();
    f.extend(std::iter::repeat_n(b'a', 100));
    f.extend_from_slice(b"\x1b[3;101HZ\x1b[4;1H> ok");
    f
}

fn rows_of(e: &flare_term::emulator::Emulator) -> Vec<String> {
    (0..4).map(|i| e.row_text(i)).collect()
}

fn grid(h: &flare_term::handle::TermHandle) -> (usize, usize) {
    h.size()
}

/// Rule 1292 (Max, 2026-11-19: a tab typed in on the Mac, then fronted here, drew its lines over each other): while the
/// size is held, the host draws for its own size, so this tab's grid takes the host's size from the attach answer and
/// shows the host's screen cell for cell, whatever this window's size. A refit here sends nothing and keeps that grid.
/// The first key ends the hold: this window's size goes to the host (never the host's own), and the grid follows it.
#[test]
fn a_held_imported_tab_draws_at_the_hosts_size_until_the_hold_ends() {
    let host = host_with("h7");
    lk(&host).tab_size = Some((120, 30));
    let h = viewer(&host, true, 80, 24);
    wait_for("the attach", || host.calls("remote.snapshot") == 1);
    host.print("h7", &wide_frame());
    wait_for("the frame", || {
        h.screen_tail(4).iter().any(|l| l.contains("> ok"))
    });
    settle();
    let mut want = flare_term::emulator::Emulator::new(120, 30);
    want.feed(&wide_frame());
    assert_eq!(grid(&h), (120, 30), "the host's grid");
    assert_eq!(
        h.with_emu_quiet(rows_of),
        rows_of(&want),
        "the host's screen, cell for cell"
    );
    h.resize(100, 28);
    settle();
    assert_eq!(
        grid(&h),
        (120, 30),
        "a refit keeps the host's grid while held"
    );
    assert!(sizes(&host, "h7").is_empty(), "and sends nothing");
    h.send_text("x");
    wait_for("the key", || !host.typed("h7").is_empty());
    assert_eq!(
        sizes(&host, "h7"),
        [(100, 28)],
        "this window's size, never the host's"
    );
    wait_for("the grid back at this window's size", || {
        grid(&h) == (100, 28)
    });
}

/// Rule 1292: an older host's attach answer names no size: the held tab keeps this window's grid, as before. A tab that
/// is not held (one opened from here, a plain shell) never takes the host's grid; nor does the host's own output pin it.
#[test]
fn a_tab_keeps_its_own_grid_when_the_host_names_no_size_or_it_is_not_held() {
    let old = host_with("h7");
    lk(&old).no_attach_size = true;
    let h = viewer(&old, true, 80, 24);
    wait_for("the attach", || old.calls("remote.snapshot") == 1);
    settle();
    assert_eq!(grid(&h), (80, 24));
    let host = host_with("h7");
    lk(&host).tab_size = Some((120, 30));
    let free = viewer(&host, false, 80, 24);
    wait_for("the size", || sizes(&host, "h7").first() == Some(&(80, 24)));
    host.print("h7", b"\x1b]7502;40;10\x07ok");
    wait_for("the output", || {
        free.screen_tail(2).iter().any(|l| l.contains("ok"))
    });
    assert_eq!(grid(&free), (80, 24), "host output never sets the grid");
}
