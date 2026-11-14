use super::*;
use crate::machines_page::MachinesPage;
use crate::machines_page::fake::FakeIo;
use crate::pair_keys::{ED25519, KNOWN_HOSTS_FILE};
use crate::pair_wire::pipe;
use crate::sharing::Platform;
use crate::sharing::fake::{FakeOs, host_key};
use fake::{FakeNet, Hub};
use flare_settings::kv::PrefStore;
use flare_settings::page::machines::{INCOMING_PREFIX, LinkStatus, NEARBY_PREFIX};

fn mac() -> Me {
    Me {
        name: "Max's Mac mini".into(),
        os: Some(Os::Mac),
        version: "0.4.2".into(),
    }
}

fn pc() -> Me {
    Me {
        name: "Max's PC".into(),
        os: Some(Os::Pc),
        version: "0.4.2".into(),
    }
}

fn pc_nearby(port: u16) -> Nearby {
    Nearby {
        id: format!("Max's PC.{SERVICE}"),
        name: "Max's PC".into(),
        os: Some(Os::Pc),
        version: "0.4.2".into(),
        addrs: vec!["192.168.0.139".parse().unwrap()],
        port,
    }
}

/// The host's side on a thread over a pipe; its events come back through a channel.
fn host_thread(
    os: Arc<FakeOs>,
    io: Arc<FakeIo>,
) -> (
    pipe::End,
    std::sync::mpsc::Receiver<HostEvent>,
    std::thread::JoinHandle<()>,
) {
    let (v, mut h) = pipe::pair();
    let (tx, rx) = channel();
    let t = std::thread::spawn(move || {
        let ctx = HostCtx {
            me: pc(),
            os,
            io,
            flare: r"C:\Users\Max\AppData\Local\Flare\current\Flare.exe".into(),
        };
        let tx = Mutex::new(tx);
        host_session(&mut h, "192.168.0.120", &ctx, 1, &|e| {
            let _ = lock(&tx).send(e);
        });
    });
    (v, rx, t)
}

/// The viewer's side on a thread; the codes go in through `codes`, its events come back.
fn viewer_thread(
    mut s: pipe::End,
    io: Arc<FakeIo>,
    files: PairFiles,
) -> (
    Sender<String>,
    std::sync::mpsc::Receiver<ViewerEvent>,
    std::thread::JoinHandle<()>,
) {
    let (codes_tx, codes_rx) = channel();
    let (tx, rx) = channel();
    let t = std::thread::spawn(move || {
        let ctx = ViewerCtx {
            me: mac(),
            io,
            files,
        };
        let tx = Mutex::new(tx);
        viewer_session(
            &mut s,
            "192.168.0.139",
            &pc_nearby(1),
            &ctx,
            &codes_rx,
            &|e| {
                let _ = lock(&tx).send(e);
            },
        );
    });
    (codes_tx, rx, t)
}

fn next<T>(rx: &std::sync::mpsc::Receiver<T>) -> T {
    rx.recv_timeout(Duration::from_secs(10)).expect("an event") // sync-ok: a test waits for its threads and reads its own files
}

/// Rules 1243 to 1246: the whole exchange over an in-memory pipe. The host shows a code; the right code gives the
/// channel; the host adds the viewer's key (a standard account: its own file, no prompt) and keeps it as a computer
/// that reaches it; the viewer pins the host key in Flare's own file and saves the machine with its address, user,
/// key and known hosts.
#[test]
fn the_right_code_pairs_and_both_sides_keep_the_other() {
    let dir = tempfile::tempdir().unwrap();
    let files = PairFiles::new(dir.path());
    let os = Arc::new(FakeOs::new(Platform::Windows));
    let (host_io, view_io) = (Arc::new(FakeIo::default()), Arc::new(FakeIo::default()));
    let (v, host_events, ht) = host_thread(Arc::clone(&os), Arc::clone(&host_io));
    let (codes, view_events, vt) = viewer_thread(v, Arc::clone(&view_io), files.clone());
    let HostEvent::Ask { viewer, code, .. } = next(&host_events) else {
        panic!("the host shows a code first");
    };
    assert_eq!(viewer, "Max's Mac mini");
    assert_eq!(code.len(), 6);
    assert_eq!(next(&view_events), ViewerEvent::Shown);
    codes.send(code).unwrap();
    let ViewerEvent::Linked { list, id } = next(&view_events) else {
        panic!("linked");
    };
    vt.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    ht.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    // The host: one key line, in the user's own file (no prompt on a standard account), tagged with the name.
    let lines = os.user_keys.lock().unwrap().clone();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5"));
    assert!(lines[0].ends_with(" flare-pair:Max's-Mac-mini"));
    assert!(os.scripts.lock().unwrap().is_empty(), "no UAC prompt");
    let HostEvent::Done {
        linked: Some((incoming, admin)),
        too_many: false,
        ..
    } = next(&host_events)
    else {
        panic!("the host's request ends linked");
    };
    assert!(!admin);
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].name, "Max's Mac mini");
    assert_eq!(incoming[0].address, "192.168.0.120");
    assert_eq!(incoming[0].kind, Some(Os::Mac));
    assert_eq!(incoming[0].version, "0.4.2");
    assert!(lines[0].contains(&incoming[0].key));
    assert_eq!(host_io.list_in(), incoming);
    // The viewer: the machine with every field ssh needs, and the host key pinned for its address.
    assert_eq!(view_io.list(), list);
    let m = &list[0];
    assert_eq!(m.id, id);
    assert_eq!(m.name, "Max's PC");
    assert_eq!(m.kind, Some(Os::Pc));
    assert_eq!(m.host.as_deref(), Some("192.168.0.139"));
    assert_eq!(m.user.as_deref(), Some("max"));
    assert_eq!(m.port, None);
    assert_eq!(
        m.flare,
        r"C:\Users\Max\AppData\Local\Flare\current\Flare.exe"
    );
    assert_eq!(m.key.as_deref(), Some(&*files.key_path().to_string_lossy()));
    let known = dir.path().join(KNOWN_HOSTS_FILE);
    assert_eq!(m.known_hosts.as_deref(), Some(&*known.to_string_lossy()));
    let pinned = std::fs::read_to_string(&known).unwrap(); // sync-ok: a test waits for its threads and reads its own files
    let hk = parse_public_key(&host_key(), &HOST_KEY_TYPES).unwrap();
    assert_eq!(pinned, format!("192.168.0.139 {}\n", hk.line()));
    // The key the host took is Flare's own key on the viewer.
    let public = std::fs::read_to_string(files.key_path().with_extension("pub")).unwrap(); // sync-ok: a test waits for its threads and reads its own files
    assert!(public.contains(&incoming[0].key));
    // ssh runs with that key, that known-hosts file and strict checking, at the address, as the user.
    let argv = crate::remote_link::ssh_argv("ssh", m).unwrap();
    for want in [
        "IdentitiesOnly=yes",
        "StrictHostKeyChecking=yes",
        "192.168.0.139",
        "max",
    ] {
        assert!(argv.iter().any(|a| a == want), "{want}: {argv:?}");
    }
}

/// Rule 1244: on a Windows account in Administrators the key goes into `administrators_authorized_keys` through one
/// elevated script that keeps only Administrators and SYSTEM on the file (by SID); a declined prompt adds nothing and
/// the viewer hears why.
#[test]
fn an_admin_account_adds_the_key_with_one_elevated_script() {
    let dir = tempfile::tempdir().unwrap();
    let os = Arc::new(FakeOs::new(Platform::Windows));
    os.admin_account.store(true, Ordering::SeqCst);
    *os.elevated.lock().unwrap() = Ok("ok=1\n".into());
    let (v, host_events, ht) = host_thread(Arc::clone(&os), Arc::new(FakeIo::default()));
    let (codes, view_events, vt) =
        viewer_thread(v, Arc::new(FakeIo::default()), PairFiles::new(dir.path()));
    let HostEvent::Ask { code, .. } = next(&host_events) else {
        panic!("ask");
    };
    next(&view_events);
    codes.send(code).unwrap();
    assert!(matches!(next(&view_events), ViewerEvent::Linked { .. }));
    vt.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    ht.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    assert!(os.user_keys.lock().unwrap().is_empty());
    let scripts = os.scripts.lock().unwrap().clone();
    assert_eq!(scripts.len(), 1);
    let s = &scripts[0];
    assert!(s.contains(r"ssh\administrators_authorized_keys"));
    assert!(s.contains("/inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F'"));
    assert!(
        s.contains("flare-pair:Max''s-Mac-mini"),
        "the name's quote is doubled"
    );
    let Some(HostEvent::Done {
        linked: Some((_, true)),
        ..
    }) = host_events
        .iter()
        .find(|e| matches!(e, HostEvent::Done { .. }))
    else {
        panic!("linked through the admin file");
    };
    // Declined at the prompt: the viewer gets the host's one line, nothing is saved.
    *os.elevated.lock().unwrap() = Err("declined".into());
    let view_io = Arc::new(FakeIo::default());
    let (v, host_events, ht) = host_thread(Arc::clone(&os), Arc::new(FakeIo::default()));
    let (codes, view_events, vt) =
        viewer_thread(v, Arc::clone(&view_io), PairFiles::new(dir.path()));
    let HostEvent::Ask { code, .. } = next(&host_events) else {
        panic!("ask");
    };
    next(&view_events);
    codes.send(code).unwrap();
    assert_eq!(
        next(&view_events),
        ViewerEvent::Failed(
            "Windows did not allow the change, so the computer was not added.".into()
        )
    );
    vt.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    ht.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    assert!(view_io.list().is_empty());
}

/// Rule 1243: a wrong code is "Wrong code" and the request stays; the third wrong code closes the host's window.
#[test]
fn wrong_codes_say_so_and_the_third_closes_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let os = Arc::new(FakeOs::new(Platform::Mac));
    let view_io = Arc::new(FakeIo::default());
    let (v, host_events, ht) = host_thread(Arc::clone(&os), Arc::new(FakeIo::default()));
    let (codes, view_events, vt) =
        viewer_thread(v, Arc::clone(&view_io), PairFiles::new(dir.path()));
    let HostEvent::Ask { code, .. } = next(&host_events) else {
        panic!("ask");
    };
    next(&view_events);
    let wrong = if code == "000000" { "000001" } else { "000000" };
    codes.send(wrong.into()).unwrap();
    assert_eq!(next(&view_events), ViewerEvent::Wrong);
    assert_eq!(next(&host_events), HostEvent::Strike { id: 1, left: 2 });
    codes.send(wrong.into()).unwrap();
    assert_eq!(next(&view_events), ViewerEvent::Wrong);
    codes.send(wrong.into()).unwrap();
    assert_eq!(
        next(&view_events),
        ViewerEvent::Failed("Wrong code three times. Pick Max's PC again for a new code.".into())
    );
    vt.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    ht.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    let rest: Vec<HostEvent> = host_events.try_iter().collect();
    assert_eq!(
        rest.last(),
        Some(&HostEvent::Done {
            id: 1,
            linked: None,
            too_many: true
        })
    );
    assert!(os.user_keys.lock().unwrap().is_empty() && view_io.list().is_empty());
    assert!(
        !dir.path().join(KNOWN_HOSTS_FILE).exists(),
        "nothing pinned"
    );
}

/// Rule 1243: a host that speaks another pairing protocol is refused in one line.
#[test]
fn another_protocol_is_refused_in_one_line() {
    let (mut v, mut h) = pipe::pair();
    let t = std::thread::spawn(move || {
        let _ = wire::recv(&mut h);
        wire::send(&mut h, &json!({"t": "proto", "proto": 2})).unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let (_codes, rx) = channel();
    let got = Mutex::new(Vec::new());
    let ctx = ViewerCtx {
        me: mac(),
        io: Arc::new(FakeIo::default()),
        files: PairFiles::new(dir.path()),
    };
    viewer_session(&mut v, "10.0.0.2", &pc_nearby(1), &ctx, &rx, &|e| {
        lock(&got).push(e)
    });
    t.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    assert_eq!(
        *lock(&got),
        [ViewerEvent::Failed(
            "Update Flare on both computers, then try again.".into()
        )]
    );
}

/// Rule 1242: an announcement is a nearby computer only with the pairing protocol and a port; its TXT name wins over
/// the instance name, IPv4 comes first and loopback goes.
#[test]
fn an_announcement_is_a_nearby_computer_with_its_name_system_and_addresses() {
    let txt = |m: &'static [(&'static str, &'static str)]| {
        move |k: &str| m.iter().find(|(a, _)| *a == k).map(|(_, v)| v.to_string())
    };
    let addrs: Vec<IpAddr> = vec![
        "fe80::1".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
        "192.168.0.139".parse().unwrap(),
    ];
    let full = format!("Max's PC.{SERVICE}");
    let n = nearby_of(
        &full,
        &txt(&[
            ("proto", "1"),
            ("name", "Max's PC"),
            ("os", "pc"),
            ("version", "0.4.2"),
        ]),
        addrs.clone(),
        41234,
    )
    .unwrap();
    assert_eq!(n.name, "Max's PC");
    assert_eq!(n.os, Some(Os::Pc));
    assert_eq!(n.version, "0.4.2");
    assert_eq!(n.port, 41234);
    let shown: Vec<String> = n.addrs.iter().map(ToString::to_string).collect();
    assert_eq!(shown, ["192.168.0.139", "fe80::1"]);
    let bare = nearby_of(&full, &txt(&[("proto", "1")]), addrs.clone(), 1).unwrap();
    assert_eq!(bare.name, "Max's PC", "the instance name");
    assert!(nearby_of(&full, &txt(&[("proto", "2")]), addrs.clone(), 1).is_none());
    assert!(nearby_of(&full, &txt(&[("proto", "1")]), addrs, 0).is_none());
}

/// Rule 1242: the announcement says the name, the system, Flare's version and the protocol, and nothing else.
#[test]
fn the_announcement_carries_no_secret() {
    let keys: Vec<String> = txt_of(&pc()).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["name", "os", "version", "proto"]);
    assert_eq!(mdns_host("Max's PC"), "Max-s-PC.local.");
    assert_eq!(mdns_host("..."), "flare.local.");
    assert_eq!(SERVICE, "_flare-pair._tcp.local.");
}

// ---- two pages over the fake network ----------------------------------------------------------------------------

struct Page {
    page: MachinesPage,
    io: Arc<FakeIo>,
    os: Arc<FakeOs>,
    net: Arc<FakeNet>,
    got: Arc<Mutex<Vec<UiMsg>>>,
    _dir: tempfile::TempDir,
}

fn page(hub: &Arc<Hub>, address: &str, me: Me, sharing: bool) -> Page {
    let io = Arc::new(FakeIo::default());
    let got: Arc<Mutex<Vec<UiMsg>>> = Arc::default();
    let g = Arc::clone(&got);
    let post: Post = Arc::new(move |m| lock(&g).push(m));
    let os = Arc::new(FakeOs::new(Platform::Windows));
    let net = Arc::new(FakeNet::new(hub, address));
    let dir = tempfile::tempdir().unwrap();
    let prefs = PrefStore::memory();
    flare_settings::machine_prefs::set_sharing_on(&prefs, sharing);
    let mut page = MachinesPage::new(io.clone(), post)
        .with_sharing_os(os.clone())
        .with_pairing(net.clone(), PairFiles::new(dir.path()), me);
    page.use_prefs(prefs);
    Page {
        page,
        io,
        os,
        net,
        got,
        _dir: dir,
    }
}

impl Page {
    fn pump(&mut self) {
        let msgs: Vec<UiMsg> = lock(&self.got).drain(..).collect();
        for m in msgs {
            if let UiMsg::Settings(SettingsMsg::Machines(m)) = m {
                self.page.on_msg(m);
            }
        }
    }
}

fn until(pages: &mut [&mut Page], what: &str, cond: impl Fn(&[&mut Page]) -> bool) {
    let t = Instant::now();
    loop {
        for p in pages.iter_mut() {
            p.pump();
        }
        if cond(pages) {
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(3)); // sync-ok: a test waits for its workers
    }
}

/// Rules 1242 to 1246, on two pages: the PC with Computer sharing on listens and is announced; the Mac's dialog finds
/// it; picking it makes the PC show its code; a wrong code is "Wrong code"; the right one links; both list the other.
#[test]
fn two_computers_pair_from_the_dialog_and_list_each_other() {
    let hub = Arc::new(Hub::default());
    let mut host = page(&hub, "192.168.0.139", pc(), true);
    assert!(host.page.pairing().hosting(), "sharing on: listening");
    let port = host.net.port().expect("announced with its port");
    assert_eq!(
        *host.net.announced.lock().unwrap(),
        [Some(("Max's PC".into(), port))]
    );
    let mut view = page(&hub, "192.168.0.120", mac(), false);
    assert!(!view.page.pairing().hosting(), "sharing off: no port");
    *view.net.nearby.lock().unwrap() = vec![pc_nearby(port)];
    view.page.page_shown();
    view.page.open_form();
    until(&mut [&mut view], "the nearby computer", |p| {
        !p[0].page.info().nearby.is_empty()
    });
    let near = view.page.info().nearby[0].clone();
    assert_eq!(near.name, "Max's PC");
    view.page.set_pick(&format!("{NEARBY_PREFIX}{}", near.id));
    assert_eq!(view.page.info().add.code_for, "Max's PC");
    assert!(!view.page.info().add.can_add(), "no code shown yet");
    until(&mut [&mut host, &mut view], "the code shows", |p| {
        p[0].page.pairing().prompt().is_some() && p[1].page.info().add.ready
    });
    let prompt = host.page.pairing().prompt().cloned().unwrap();
    let words = prompt_words(&prompt);
    assert_eq!(words.title, "Max's Mac mini wants to connect");
    assert_eq!(words.line, "Enter this code on Max's Mac mini");
    assert_eq!(words.code.len(), 7, "482 913");
    // A wrong code: the red line, the request stays.
    let wrong = if prompt.code == "111111" {
        "222 222"
    } else {
        "111 111"
    };
    view.page.set_code(wrong);
    assert!(view.page.info().add.can_add());
    view.page.submit();
    assert!(view.page.info().add.busy);
    until(&mut [&mut host, &mut view], "wrong code", |p| {
        p[1].page.info().add.error.is_some()
    });
    assert_eq!(view.page.info().add.error.as_deref(), Some("Wrong code"));
    assert!(host.page.pairing().prompt().is_some(), "the code stays up");
    // The right code: both sides list the other.
    view.page.set_code(&words.code);
    view.page.submit();
    until(&mut [&mut host, &mut view], "linked", |p| {
        !p[1].page.info().add.open && !p[0].page.info().machines.is_empty()
    });
    assert!(
        host.page.pairing().prompt().is_none(),
        "the code dialog closed"
    );
    let on_host = host.page.info().machines;
    assert_eq!(on_host.len(), 1);
    assert_eq!(on_host[0].id, format!("{INCOMING_PREFIX}max-s-mac-mini"));
    assert_eq!(on_host[0].name, "Max's Mac mini");
    assert_eq!(on_host[0].address, "192.168.0.120");
    assert_eq!(on_host[0].status, LinkStatus::Linked);
    let saved = view.io.list();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].name, "Max's PC");
    assert_eq!(saved[0].host.as_deref(), Some("192.168.0.139"));
    assert_eq!(view.page.info().machines[0].address, "192.168.0.139");
    assert_eq!(host.os.user_keys.lock().unwrap().len(), 1);
}

/// Rule 1243: Decline on the host ends the request: the viewer's dialog says so and its Add waits for a new pick.
#[test]
fn decline_on_the_host_ends_the_request() {
    let hub = Arc::new(Hub::default());
    let mut host = page(&hub, "192.168.0.139", pc(), true);
    let port = host.net.port().unwrap();
    let mut view = page(&hub, "192.168.0.120", mac(), false);
    *view.net.nearby.lock().unwrap() = vec![pc_nearby(port)];
    view.page.open_form();
    until(&mut [&mut view], "nearby", |p| {
        !p[0].page.info().nearby.is_empty()
    });
    let id = view.page.info().nearby[0].id.clone();
    view.page.set_pick(&format!("{NEARBY_PREFIX}{id}"));
    until(&mut [&mut host, &mut view], "the code shows", |p| {
        p[0].page.pairing().prompt().is_some() && p[1].page.info().add.ready
    });
    host.page.decline();
    assert!(host.page.pairing().prompt().is_none());
    view.page.set_code("123 456");
    view.page.submit();
    until(&mut [&mut host, &mut view], "declined", |p| {
        p[1].page.info().add.error.is_some()
    });
    assert_eq!(
        view.page.info().add.error.as_deref(),
        Some("Max's PC declined.")
    );
    assert!(!view.page.info().add.can_add());
    assert!(view.io.list().is_empty());
}

/// Rule 1243: one request at a time: a second computer asking while the code shows hears that the host is busy.
#[test]
fn a_second_request_while_one_shows_its_code_is_told_busy() {
    let hub = Arc::new(Hub::default());
    let mut host = page(&hub, "192.168.0.139", pc(), true);
    let port = host.net.port().unwrap();
    let mut first = page(&hub, "192.168.0.120", mac(), false);
    let mut second = page(&hub, "192.168.0.121", mac(), false);
    for v in [&mut first, &mut second] {
        *v.net.nearby.lock().unwrap() = vec![pc_nearby(port)];
        v.page.open_form();
    }
    until(&mut [&mut first, &mut second], "nearby", |p| {
        p.iter().all(|p| !p.page.info().nearby.is_empty())
    });
    let id = first.page.info().nearby[0].id.clone();
    first.page.set_pick(&format!("{NEARBY_PREFIX}{id}"));
    until(&mut [&mut host, &mut first], "the code shows", |p| {
        p[0].page.pairing().prompt().is_some()
    });
    second.page.set_pick(&format!("{NEARBY_PREFIX}{id}"));
    until(&mut [&mut host, &mut second], "busy", |p| {
        p[1].page.info().add.error.is_some()
    });
    assert_eq!(
        second.page.info().add.error.as_deref(),
        Some("Max's PC is busy with another request. Try again in a minute.")
    );
    // Cancel on the first viewer ends its request: the host's code dialog closes.
    first.page.cancel_form();
    until(&mut [&mut host], "the code dialog closes", |p| {
        p[0].page.pairing().prompt().is_none()
    });
}

/// Rules 1241 and 1242: turning Computer sharing off stops the announcement and the listener.
#[test]
fn turning_sharing_off_stops_the_announcement() {
    let hub = Arc::new(Hub::default());
    let mut host = page(&hub, "192.168.0.139", pc(), true);
    assert!(host.page.pairing().hosting());
    host.page.sharing_switch(false);
    assert!(!host.page.pairing().hosting());
    assert_eq!(host.net.announced.lock().unwrap().last(), Some(&None));
    host.pump();
}

/// Rule 1246: Remove on a computer that reaches this one removes its key and its row.
#[test]
fn removing_a_computer_that_reaches_this_one_removes_its_key() {
    let hub = Arc::new(Hub::default());
    let mut host = page(&hub, "192.168.0.139", pc(), true);
    let key = parse_public_key(&host_key(), &[ED25519]).unwrap().key;
    *host.io.incoming.lock().unwrap() = vec![crate::machines::Incoming {
        id: "mini".into(),
        name: "Mini".into(),
        kind: Some(Os::Mac),
        address: "192.168.0.120".into(),
        version: "0.4.2".into(),
        key: key.clone(),
    }];
    host.page.page_shown();
    until(&mut [&mut host], "read", |p| {
        !p[0].page.info().machines.is_empty()
    });
    host.page.remove(&format!("{INCOMING_PREFIX}mini"));
    until(&mut [&mut host], "removed", |p| {
        p[0].page.info().machines.is_empty()
    });
    assert_eq!(*host.os.removed_keys.lock().unwrap(), [key]);
    assert!(host.io.list_in().is_empty());
    assert!(
        host.os.scripts.lock().unwrap().is_empty(),
        "no admin key: no prompt"
    );
}
