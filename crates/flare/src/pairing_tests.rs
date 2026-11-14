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
    host_thread_with(os, io, PrefStore::memory())
}

/// [`host_thread`] with the settings store an admin key is noted in. A test that set `os.stop_on_add` turns sharing
/// off the moment the key goes in.
fn host_thread_with(
    os: Arc<FakeOs>,
    io: Arc<FakeIo>,
    prefs: PrefStore,
) -> (
    pipe::End,
    std::sync::mpsc::Receiver<HostEvent>,
    std::thread::JoinHandle<()>,
) {
    host_thread_cleaned(os, io, prefs, CleanUps::default())
}

/// [`host_thread_with`] whose host started when `clean_ups` had passed none: a test that passes one first plays a
/// sharing clean up that already went past its wait.
fn host_thread_cleaned(
    os: Arc<FakeOs>,
    io: Arc<FakeIo>,
    prefs: PrefStore,
    clean_ups: CleanUps,
) -> (
    pipe::End,
    std::sync::mpsc::Receiver<HostEvent>,
    std::thread::JoinHandle<()>,
) {
    let (v, mut h) = pipe::pair();
    let (tx, rx) = channel();
    let stop = lock(&os.stop_on_add).clone().unwrap_or_default();
    let t = std::thread::spawn(move || {
        let ctx = HostCtx {
            me: pc(),
            os,
            io,
            flare: r"C:\Users\Max\AppData\Local\Flare\current\Flare.exe".into(),
            stop,
            prefs: Arc::new(Mutex::new(Some(prefs))),
            clean_ups,
            clean_ups_at_start: 0,
        };
        let tx = Mutex::new(tx);
        let Some(hello) = read_hello(&mut h) else {
            return;
        };
        host_session(&mut h, "192.168.0.120", &hello, &ctx, 1, &|e| {
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

/// Rules 1257 to 1260: the whole exchange over an in-memory pipe. The host shows a code; the right code gives the
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
        linked: Some(incoming),
        too_many: false,
        ..
    } = next(&host_events)
    else {
        panic!("the host's request ends linked");
    };
    assert_eq!(incoming[0].admin, Some(false), "the user's own file");
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

/// Rule 1258: on a Windows account in Administrators the key goes into `administrators_authorized_keys` through one
/// elevated script that keeps only Administrators and SYSTEM on the file (by SID); a declined prompt adds nothing and
/// the viewer hears why.
#[test]
fn an_admin_account_adds_the_key_with_one_elevated_script() {
    let dir = tempfile::tempdir().unwrap();
    let os = Arc::new(FakeOs::new(Platform::Windows));
    os.admin_account.store(true, Ordering::SeqCst);
    *os.elevated.lock().unwrap() = Ok(crate::sharing::fake::ok_outcome(false, false, false));
    let prefs = PrefStore::memory();
    let (v, host_events, ht) =
        host_thread_with(Arc::clone(&os), Arc::new(FakeIo::default()), prefs.clone());
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
    assert!(s.contains("$f = Join-Path $dir 'administrators_authorized_keys'"));
    assert!(s.contains("/inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F'"));
    // The folder and an existing file are checked before Flare writes there as an administrator (rule 1253's check).
    let at = |want: &str| s.find(want).unwrap_or_else(|| panic!("{want}\n{s}"));
    assert!(at("Assert-AdminOnly $dir") < at("$step = 'addkey'"));
    assert!(at("Assert-AdminOnly $f") < at("Set-Content -LiteralPath $f"));
    assert!(
        s.contains("flare-pair:Max''s-Mac-mini"),
        "the name's quote is doubled"
    );
    let Some(HostEvent::Done {
        linked: Some(list), ..
    }) = host_events
        .iter()
        .find(|e| matches!(e, HostEvent::Done { .. }))
    else {
        panic!("linked");
    };
    assert_eq!(
        list[0].admin,
        Some(true),
        "the entry says the key is in the admin file"
    );
    assert!(
        flare_settings::machine_prefs::admin_keys(&prefs),
        "noted for the clean up"
    );
    // Declined at the prompt: the viewer gets the host's one line, nothing is saved.
    *os.elevated.lock().unwrap() = Ok("flare-sharing=declined\r\n".into());
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

/// Rule 1257: a wrong code is "Wrong code" and the request stays; the third wrong code closes the host's window.
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

/// Rule 1257: a host that speaks another pairing protocol is refused in one line.
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

/// Rule 1256: an announcement is a nearby computer only with the pairing protocol and a port; its TXT name wins over
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

/// Rule 1256: the announcement says the name, the system, Flare's version and the protocol, and nothing else.
#[test]
fn the_announcement_carries_no_secret() {
    let keys: Vec<String> = txt_of(&pc()).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["name", "os", "version", "proto"]);
    assert_eq!(SERVICE, "_flare-pair._tcp.local.");
}

/// Rule 1256: the announcement's address records never go under the computer's own `.local` name, which macOS'
/// mDNSResponder (and Windows' responder) answers already: a second answer there is a conflict, and macOS renames the
/// Mac. The readable name stays in the instance name and the TXT.
#[test]
fn the_announcement_never_takes_the_computers_own_local_name() {
    // gethostname on a Mac says "Maxs-Mac-mini.local"; `Me` leaves the `.local` off.
    for (name, own) in [
        ("Maxs-Mac-mini", "Maxs-Mac-mini.local."),
        ("DESKTOP-7Q2K", "DESKTOP-7Q2K.local."),
        ("Max's PC", "Max-s-PC.local."),
    ] {
        let host = mdns_host(name);
        assert_ne!(host.to_ascii_lowercase(), own.to_ascii_lowercase());
        assert!(host.ends_with("-flare.local."), "{host}");
    }
    assert_eq!(mdns_host("Maxs-Mac-mini"), "Maxs-Mac-mini-flare.local.");
    assert_eq!(mdns_host("Max's PC"), "Max-s-PC-flare.local.");
    assert_eq!(mdns_host("..."), "computer-flare.local.");
    // One DNS label stays under 63 letters.
    assert!(mdns_host(&"x".repeat(200)).len() <= 40 + "-flare.local.".len());
    let txt = txt_of(&mac());
    assert_eq!(txt[0], ("name".to_string(), "Max's Mac mini".to_string()));
}

/// Rule 1256: the listener takes the first free port of the fixed range Flare's firewall rule names, in order; when
/// every one is taken, it says so in one plain line.
#[test]
fn the_listener_takes_the_first_free_pairing_port_or_says_none_was_free() {
    assert_eq!(PAIRING_PORTS, 47_790..=47_794);
    let mut tried = Vec::new();
    let got = bind_first(|p| {
        tried.push(p);
        if p < 47_792 {
            Err("in use".to_string())
        } else {
            Ok(u32::from(p) * 2)
        }
    });
    assert_eq!(got, Ok((47_792_u32 * 2, 47_792)));
    assert_eq!(tried, [47_790, 47_791, 47_792]);
    let mut tried = Vec::new();
    let none = bind_first::<()>(|p| {
        tried.push(p);
        Err("in use".to_string())
    });
    assert_eq!(tried, [47_790, 47_791, 47_792, 47_793, 47_794]);
    assert_eq!(
        none,
        Err("Other programs use ports 47790 to 47794, so other computers cannot pair with this one. Close them, then \
             turn Computer sharing off and on."
            .to_string())
    );
}

/// Rule 1259: a Mac copy that runs from a disk image or from macOS' moved-aside copy answers with the installed app's
/// path when there is one, else with none (the viewer uses its own default); any other path goes as it is.
#[test]
fn a_mac_copy_from_a_disk_image_names_the_installed_flare_or_none() {
    let installed = |p: &str| p == MAC_APP_FLARE;
    let nothing = |_: &str| false;
    for exe in [
        "/Volumes/Flare/Flare.app/Contents/MacOS/flare",
        "/private/var/folders/x1/T/AppTranslocation/0A1B/d/Flare.app/Contents/MacOS/flare",
    ] {
        assert_eq!(
            reply_flare_path(exe, true, &installed).as_deref(),
            Some(MAC_APP_FLARE),
            "{exe}"
        );
        assert_eq!(reply_flare_path(exe, true, &nothing), None, "{exe}");
    }
    assert_eq!(
        reply_flare_path(
            "/Applications/Flare.app/Contents/MacOS/flare",
            true,
            &nothing
        )
        .as_deref(),
        Some("/Applications/Flare.app/Contents/MacOS/flare")
    );
    let pc = r"C:\Users\Max\AppData\Local\Flare\current\Flare.exe";
    assert_eq!(reply_flare_path(pc, false, &nothing).as_deref(), Some(pc));
    assert_eq!(reply_flare_path("", false, &installed), None);
    assert_eq!(fallback_flare(Some(Os::Mac)), MAC_APP_FLARE);
    assert_eq!(fallback_flare(Some(Os::Pc)), "flare");
}

/// Rule 1259: a Mac host that runs from a disk image, with no installed app, answers with no Flare path; the viewer
/// still pairs and saves the Mac app's own place.
#[test]
fn a_host_that_names_no_flare_path_pairs_with_the_viewers_default() {
    let dir = tempfile::tempdir().unwrap();
    let os = Arc::new(FakeOs::new(Platform::Mac));
    let view_io = Arc::new(FakeIo::default());
    let (v, mut h) = pipe::pair();
    let (tx, host_events) = channel();
    let ht = std::thread::spawn(move || {
        let ctx = HostCtx {
            me: mac(),
            os,
            io: Arc::new(FakeIo::default()),
            // No such file on the test machine: the installed app is missing.
            flare: "/Volumes/Flare 0.4.2/Flare.app/Contents/MacOS/flare".into(),
            stop: Arc::default(),
            prefs: Arc::default(),
            clean_ups: CleanUps::default(),
            clean_ups_at_start: 0,
        };
        let tx = Mutex::new(tx);
        let Some(hello) = read_hello(&mut h) else {
            return;
        };
        host_session(&mut h, "192.168.0.139", &hello, &ctx, 1, &|e| {
            let _ = lock(&tx).send(e);
        });
    });
    let (codes, view_events, vt) =
        viewer_thread(v, Arc::clone(&view_io), PairFiles::new(dir.path()));
    let HostEvent::Ask { code, .. } = next(&host_events) else {
        panic!("ask");
    };
    next(&view_events);
    codes.send(code).unwrap();
    let ViewerEvent::Linked { list, .. } = next(&view_events) else {
        panic!("linked");
    };
    vt.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    ht.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    assert_eq!(list[0].kind, Some(Os::Mac));
    assert_eq!(list[0].flare, MAC_APP_FLARE);
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

/// Rule 1256: when every pairing port is taken, the sharing dialog says so once, nothing is announced, and turning
/// sharing off and on tries again.
#[test]
fn no_free_pairing_port_says_so_once_and_announces_nothing() {
    let hub = Arc::new(Hub::default());
    let mut p = page(&hub, "192.168.0.139", pc(), false);
    *p.net.ports_taken.lock().unwrap() = true;
    let prefs = PrefStore::memory();
    flare_settings::machine_prefs::set_sharing_on(&prefs, true);
    p.page.use_prefs(prefs.clone());
    assert!(!p.page.pairing().hosting());
    assert!(p.net.port().is_none(), "nothing announced");
    let Some(crate::sharing::SharingAsk::Notice(line)) = p.page.sharing().ask().cloned() else {
        panic!("one line");
    };
    assert_eq!(line, ports_taken_line());
    // Said once: the next look at the switch does not say it again.
    p.page
        .sharing_button(flare_settings::page::machines::SHARING_PRIMARY);
    p.page.use_prefs(prefs.clone());
    assert!(p.page.sharing().ask().is_none(), "said once");
    // Off, then on again with a port free: it listens.
    flare_settings::machine_prefs::set_sharing_on(&prefs, false);
    p.page.use_prefs(prefs.clone());
    *p.net.ports_taken.lock().unwrap() = false;
    flare_settings::machine_prefs::set_sharing_on(&prefs, true);
    p.page.use_prefs(prefs);
    assert!(p.page.pairing().hosting());
    assert!(p.net.port().is_some());
}

/// Rules 1256 to 1260, on two pages: the PC with Computer sharing on listens and is announced; the Mac's dialog finds
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

/// Rule 1257: Decline on the host ends the request: the viewer's dialog says so and its Add waits for a new pick.
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

/// Rule 1257: one request at a time: a second computer asking while the code shows hears that the host is busy.
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

/// Rule 1261: a request that ended unpaired once its code showed locks the host; a request that paired (a typo, then
/// the right code), or that ended before its code showed, does not.
#[test]
fn a_request_that_ends_after_a_wrong_code_locks_the_host() {
    let t = Instant::now();
    let mut g = Guard::default();
    g.ended("10.0.0.9", t, false, false);
    assert!(g.admits("10.0.0.9", t), "no code showed: no lock");
    g.strike("10.0.0.9", t);
    g.ended("10.0.0.9", t, true, true);
    assert!(g.admits("10.0.0.9", t), "a typo, then the right code");
    g.strike("10.0.0.9", t);
    g.ended("10.0.0.9", t, true, false);
    assert!(!g.admits("10.0.0.9", t), "a wrong code, then hung up");
    assert!(!g.admits("10.0.0.7", t), "the lock holds for every address");
    assert!(g.admits("10.0.0.7", t + LOCKOUTS[0]));
}

/// Rule 1261: the count holds across connections. Each request that ended unpaired after its code showed locks for
/// longer (30 s, 2 min, 10 min); one address past its cap per hour, or all addresses past theirs, get no request until
/// the oldest strike is an hour old, so new connections never mean new guesses.
#[test]
fn wrong_codes_count_across_connections_and_the_lockout_grows() {
    let t = Instant::now();
    let mut g = Guard::default();
    let mut now = t;
    for (i, want) in [LOCKOUTS[0], LOCKOUTS[1], LOCKOUTS[2], LOCKOUTS[2]]
        .into_iter()
        .enumerate()
    {
        assert!(g.admits("10.0.0.9", now), "request {i}");
        g.ended("10.0.0.9", now, true, false);
        assert!(!g.admits("10.0.0.9", now + want - Duration::from_secs(1)));
        now += want;
    }
    // One address: its cap per hour, whatever the lockouts.
    let mut g = Guard::default();
    for _ in 0..PEER_STRIKES_MAX {
        g.strike("10.0.0.9", t);
    }
    assert!(!g.admits("10.0.0.9", t + LOCKOUTS[2]));
    assert!(g.admits("10.0.0.8", t + LOCKOUTS[2]));
    assert!(g.admits("10.0.0.9", t + STRIKE_WINDOW));
    // All addresses together: a stranger who changes address still meets the cap.
    let mut g = Guard::default();
    for i in 0..ALL_STRIKES_MAX {
        g.strike(&format!("10.0.1.{i}"), t);
    }
    assert!(!g.admits("10.0.2.1", t));
    assert!(g.admits("10.0.2.1", t + STRIKE_WINDOW));
    const { assert!(PEER_STRIKES_MAX < ALL_STRIKES_MAX) };
}

/// Rule 1261, on two pages: a viewer that types one wrong code and hangs up cannot ask again at once for a fresh code:
/// the next request is told busy (else a stranger gets unlimited guesses, a few per code).
#[test]
fn hanging_up_after_a_wrong_code_does_not_give_a_fresh_code_at_once() {
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
    let pick = format!("{NEARBY_PREFIX}{id}");
    view.page.set_pick(&pick);
    until(&mut [&mut host, &mut view], "the code shows", |p| {
        p[0].page.pairing().prompt().is_some() && p[1].page.info().add.ready
    });
    let code = host.page.pairing().prompt().unwrap().code.clone();
    let wrong = if code == "111111" {
        "222 222"
    } else {
        "111 111"
    };
    view.page.set_code(wrong);
    view.page.submit();
    until(&mut [&mut host, &mut view], "wrong code", |p| {
        p[1].page.info().add.error.is_some()
    });
    assert_eq!(view.page.info().add.error.as_deref(), Some("Wrong code"));
    // Hang up and ask again at once.
    view.page.cancel_form();
    until(&mut [&mut host], "the code dialog closes", |p| {
        p[0].page.pairing().prompt().is_none()
    });
    view.page.open_form();
    until(&mut [&mut view], "nearby again", |p| {
        !p[0].page.info().nearby.is_empty()
    });
    // The host's dialog closed on its Done, which set the lock first.
    view.page.set_pick(&pick);
    until(&mut [&mut host, &mut view], "an answer", |p| {
        p[1].page.info().add.error.is_some() || p[0].page.pairing().prompt().is_some()
    });
    assert!(host.page.pairing().prompt().is_none(), "no fresh code");
    assert_eq!(
        view.page.info().add.error.as_deref(),
        Some("Max's PC is busy with another request. Try again in a minute.")
    );
}

/// Rules 1255 and 1256: turning Computer sharing off stops the announcement and the listener.
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

/// Rule 1260: Remove on a computer that reaches this one removes its key and its row.
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
        admin: Some(false),
    }];
    // Another key once went into the admin file: Flare noted that for the whole computer, yet removing this one,
    // which is in the user's own file, needs no prompt (rule 1260).
    flare_settings::machine_prefs::set_admin_keys(&host.page.sharing().prefs(), true);
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
        "a key in the user's own file: no prompt"
    );
}

/// Rule 1260: a computer whose key is in `administrators_authorized_keys` is removed with one elevated script.
#[test]
fn removing_a_computer_whose_key_is_in_the_admin_file_asks_once() {
    let hub = Arc::new(Hub::default());
    let mut host = page(&hub, "192.168.0.139", pc(), true);
    *host.os.elevated.lock().unwrap() = Ok(crate::sharing::fake::ok_outcome(false, false, false));
    let key = parse_public_key(&host_key(), &[ED25519]).unwrap().key;
    *host.io.incoming.lock().unwrap() = vec![crate::machines::Incoming {
        id: "mini".into(),
        name: "Mini".into(),
        key: key.clone(),
        admin: Some(true),
        ..Default::default()
    }];
    host.page.page_shown();
    until(&mut [&mut host], "read", |p| {
        !p[0].page.info().machines.is_empty()
    });
    host.page.remove(&format!("{INCOMING_PREFIX}mini"));
    until(&mut [&mut host], "removed", |p| {
        p[0].page.info().machines.is_empty()
    });
    let scripts = host.os.scripts.lock().unwrap().clone();
    assert_eq!(scripts.len(), 1);
    assert!(scripts[0].contains("remove a paired computer's key"));
}

/// Rule 1260: an entry written before Flare kept where its key went says nothing. Remove then never takes it as the
/// user's own file: when the switch noted an admin key, or the account is a Windows administrator, the admin file's
/// line is removed too (one elevated script); else no prompt.
#[test]
fn removing_an_older_entry_that_does_not_say_where_its_key_went_asks_the_switch_and_the_account() {
    for (noted, admin_account, asks) in [
        (true, false, true),
        (false, true, true),
        (false, false, false),
    ] {
        let hub = Arc::new(Hub::default());
        let mut host = page(&hub, "192.168.0.139", pc(), true);
        *host.os.elevated.lock().unwrap() =
            Ok(crate::sharing::fake::ok_outcome(false, false, false));
        host.os.admin_account.store(admin_account, Ordering::SeqCst);
        flare_settings::machine_prefs::set_admin_keys(&host.page.sharing().prefs(), noted);
        let key = parse_public_key(&host_key(), &[ED25519]).unwrap().key;
        *host.io.incoming.lock().unwrap() = vec![crate::machines::Incoming {
            id: "mini".into(),
            name: "Mini".into(),
            key: key.clone(),
            admin: None,
            ..Default::default()
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
        let scripts = host.os.scripts.lock().unwrap().clone();
        assert_eq!(
            scripts
                .iter()
                .filter(|s| s.contains("remove a paired computer's key"))
                .count(),
            usize::from(asks),
            "noted {noted}, admin account {admin_account}"
        );
    }
}

/// Rule 1258: everything that can fail is read before the key goes in: a host key that cannot be read, or a user
/// name ssh cannot take, refuses the request with nothing added.
#[test]
fn nothing_goes_in_when_the_answer_cannot_be_made() {
    let dir = tempfile::tempdir().unwrap();
    for broken in ["host key", "user"] {
        let os = Arc::new(FakeOs::new(Platform::Windows));
        match broken {
            "host key" => *os.host_key.lock().unwrap() = Err("no file".into()),
            _ => *os.user.lock().unwrap() = "Jo O'Neil".into(),
        }
        let host_io = Arc::new(FakeIo::default());
        let (v, host_events, ht) = host_thread(Arc::clone(&os), Arc::clone(&host_io));
        let (codes, view_events, vt) =
            viewer_thread(v, Arc::new(FakeIo::default()), PairFiles::new(dir.path()));
        let HostEvent::Ask { code, .. } = next(&host_events) else {
            panic!("ask");
        };
        next(&view_events);
        codes.send(code).unwrap();
        assert!(
            matches!(next(&view_events), ViewerEvent::Failed(_)),
            "{broken}"
        );
        vt.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
        ht.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
        assert!(os.user_keys.lock().unwrap().is_empty(), "{broken}");
        assert!(os.scripts.lock().unwrap().is_empty(), "{broken}");
        assert!(host_io.list_in().is_empty(), "{broken}");
    }
}

/// Rule 1258: when sharing turns off while a request adds its key, the key comes out again and no entry stays; an
/// admin key is noted for the clean up at once, and the clean up (not a second prompt) removes it.
#[test]
fn a_key_that_went_in_as_sharing_turned_off_comes_out_again() {
    let dir = tempfile::tempdir().unwrap();
    for admin in [false, true] {
        let os = Arc::new(FakeOs::new(Platform::Windows));
        os.admin_account.store(admin, Ordering::SeqCst);
        *os.elevated.lock().unwrap() = Ok(crate::sharing::fake::ok_outcome(false, false, false));
        *os.stop_on_add.lock().unwrap() = Some(Arc::default());
        let (host_io, prefs) = (Arc::new(FakeIo::default()), PrefStore::memory());
        let (v, host_events, ht) =
            host_thread_with(Arc::clone(&os), Arc::clone(&host_io), prefs.clone());
        let (codes, view_events, vt) =
            viewer_thread(v, Arc::new(FakeIo::default()), PairFiles::new(dir.path()));
        let HostEvent::Ask { code, .. } = next(&host_events) else {
            panic!("ask");
        };
        next(&view_events);
        codes.send(code).unwrap();
        assert!(matches!(next(&view_events), ViewerEvent::Failed(_)));
        vt.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
        ht.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
        let done: Vec<HostEvent> = host_events.try_iter().collect();
        assert!(
            matches!(done.last(), Some(HostEvent::Done { linked: None, .. })),
            "{done:?}"
        );
        assert!(host_io.list_in().is_empty(), "no entry stays");
        assert_eq!(os.removed_keys.lock().unwrap().len(), 1, "taken out");
        assert_eq!(
            flare_settings::machine_prefs::admin_keys(&prefs),
            admin,
            "an admin key is noted the moment it went in"
        );
        assert_eq!(
            os.scripts.lock().unwrap().len(),
            usize::from(admin),
            "no second prompt"
        );
    }
}

/// Rule 1258: when the sharing clean up already went past its wait for this request (it gave up after `DRAIN_WAIT`),
/// it may have cleaned the admin file before the key went in. The request then removes its own admin key, elevated,
/// so no key in `administrators_authorized_keys` outlives sharing.
#[test]
fn an_admin_key_that_went_in_after_the_clean_up_comes_out_with_its_own_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let os = Arc::new(FakeOs::new(Platform::Windows));
    os.admin_account.store(true, Ordering::SeqCst);
    *os.elevated.lock().unwrap() = Ok(crate::sharing::fake::ok_outcome(false, false, false));
    *os.stop_on_add.lock().unwrap() = Some(Arc::default());
    let clean_ups = CleanUps::default();
    clean_ups.pass();
    let host_io = Arc::new(FakeIo::default());
    let (v, host_events, ht) = host_thread_cleaned(
        Arc::clone(&os),
        Arc::clone(&host_io),
        PrefStore::memory(),
        clean_ups,
    );
    let (codes, view_events, vt) =
        viewer_thread(v, Arc::new(FakeIo::default()), PairFiles::new(dir.path()));
    let HostEvent::Ask { code, .. } = next(&host_events) else {
        panic!("ask");
    };
    next(&view_events);
    codes.send(code).unwrap();
    assert!(matches!(next(&view_events), ViewerEvent::Failed(_)));
    vt.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    ht.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    assert!(host_io.list_in().is_empty(), "no entry stays");
    let scripts = os.scripts.lock().unwrap().clone();
    assert_eq!(scripts.len(), 2, "the key went in, then out: {scripts:?}");
    assert!(scripts[0].contains("let a paired computer in"));
    assert!(scripts[1].contains("remove a paired computer's key"));
}

/// Rule 1258: turning sharing off waits for the host's requests that still run, at most a while.
#[test]
fn turning_sharing_off_waits_for_a_request_that_runs() {
    let running = Running::default();
    assert!(running.wait_idle(Duration::ZERO));
    let one = running.enter();
    assert!(!running.wait_idle(Duration::from_millis(20)));
    let r = running.clone();
    let t = std::thread::spawn(move || r.wait_idle(Duration::from_secs(10)));
    std::thread::sleep(Duration::from_millis(30)); // sync-ok: a test waits for its workers
    drop(one);
    assert!(t.join().unwrap()); // sync-ok: a test waits for its threads and reads its own files
}

/// Rules 1255 and 1258: turning sharing off while a request adds its key: the request takes its key out and writes the
/// list back as it was, and only then is the list of computers that reach this one emptied, so none whose key the
/// clean up removes stays listed.
#[test]
fn turning_sharing_off_mid_request_leaves_no_computer_listed() {
    let hub = Arc::new(Hub::default());
    let mut host = page(&hub, "192.168.0.139", pc(), true);
    *host.io.incoming.lock().unwrap() = vec![crate::machines::Incoming {
        id: "mini".into(),
        name: "Mini".into(),
        address: "192.168.0.120".into(),
        key: parse_public_key(&host_key(), &[ED25519]).unwrap().key,
        ..Default::default()
    }];
    let (release, hold) = std::sync::mpsc::channel::<()>();
    *host.os.hold_add.lock().unwrap() = Some(hold);
    let port = host.net.port().unwrap();
    let mut view = page(&hub, "192.168.0.110", mac(), false);
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
    let code = host.page.pairing().prompt().unwrap().code.clone();
    view.page.set_code(&code);
    view.page.submit();
    until(&mut [&mut host, &mut view], "the key goes in", |p| {
        !p[0].os.user_keys.lock().unwrap().is_empty()
    });
    host.page.sharing_switch(false);
    drop(release);
    let t = Instant::now();
    while host.os.removed_keys.lock().unwrap().is_empty() || !host.io.list_in().is_empty() {
        host.pump();
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "timed out: the list empties"
        );
        std::thread::sleep(Duration::from_millis(3)); // sync-ok: a test waits for its workers
    }
    // The clean up's worker ran after the request: nothing writes the old list back later.
    std::thread::sleep(Duration::from_millis(50)); // sync-ok: a test waits for its workers
    assert!(host.io.list_in().is_empty(), "{:?}", host.io.list_in());
    assert_eq!(host.os.removed_keys.lock().unwrap().len(), 1);
}

/// Rule 1261: a connection that says nothing holds no request: another computer still gets its code. The slot is
/// taken only after a valid hello.
#[test]
fn a_quiet_connection_holds_no_request_slot() {
    let hub = Arc::new(Hub::default());
    let mut host = page(&hub, "192.168.0.139", pc(), true);
    let port = host.net.port().unwrap();
    let quiet = FakeNet::new(&hub, "192.168.0.66");
    let _held = quiet.connect(&pc_nearby(port)).unwrap();
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
    assert_eq!(
        host.page.pairing().prompt().unwrap().viewer,
        "Max's Mac mini"
    );
    view.page.cancel_form();
}

/// Rules 1258 and 1259: a Windows user name with a space pairs, and goes to ssh as one argv item after `-l`.
#[test]
fn a_windows_user_with_a_space_pairs_and_is_one_ssh_argument() {
    let dir = tempfile::tempdir().unwrap();
    let os = Arc::new(FakeOs::new(Platform::Windows));
    *os.user.lock().unwrap() = "Max Aubert".into();
    let view_io = Arc::new(FakeIo::default());
    let (v, host_events, ht) = host_thread(Arc::clone(&os), Arc::new(FakeIo::default()));
    let (codes, view_events, vt) =
        viewer_thread(v, Arc::clone(&view_io), PairFiles::new(dir.path()));
    let HostEvent::Ask { code, .. } = next(&host_events) else {
        panic!("ask");
    };
    next(&view_events);
    codes.send(code).unwrap();
    let ViewerEvent::Linked { list, .. } = next(&view_events) else {
        panic!("linked");
    };
    vt.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    ht.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
    assert_eq!(list[0].user.as_deref(), Some("Max Aubert"));
    let argv = crate::remote_link::ssh_argv("ssh", &list[0]).unwrap();
    let l = argv.iter().position(|a| a == "-l").unwrap();
    assert_eq!(argv[l + 1], "Max Aubert");
    // Read back from the file, it is the same one user.
    let text = crate::machines::machines_json(&list);
    assert_eq!(crate::machines::parse_machines(&text), list);
}

/// Rule 1259: the viewer never picks the entry to replace by a name the other computer says: a computer that calls
/// itself like a saved machine is a new entry with an id of its own. Pairing the same computer again (the same pinned
/// SSH host key) keeps its entry and id.
#[test]
fn pairing_never_replaces_a_machine_by_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let view_io = Arc::new(FakeIo::default());
    let mine = Machine {
        id: "max-s-pc".into(),
        name: "Max's PC".into(),
        ssh: "studio".into(),
        flare: "flare".into(),
        host: Some("192.168.0.139".into()),
        ..Machine::default()
    };
    *view_io.list.lock().unwrap() = vec![mine.clone()];
    let pair_once = || {
        let os = Arc::new(FakeOs::new(Platform::Windows));
        let (v, host_events, ht) = host_thread(os, Arc::new(FakeIo::default()));
        let (codes, view_events, vt) =
            viewer_thread(v, Arc::clone(&view_io), PairFiles::new(dir.path()));
        let HostEvent::Ask { code, .. } = next(&host_events) else {
            panic!("ask");
        };
        next(&view_events);
        codes.send(code).unwrap();
        let ViewerEvent::Linked { list, id } = next(&view_events) else {
            panic!("linked");
        };
        vt.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
        ht.join().unwrap(); // sync-ok: a test waits for its threads and reads its own files
        (list, id)
    };
    let (list, id) = pair_once();
    assert_eq!(list.len(), 2, "a new entry");
    assert_eq!(list[0], mine, "the saved machine is untouched");
    assert_ne!(id, mine.id);
    let hk = parse_public_key(&host_key(), &HOST_KEY_TYPES).unwrap().key;
    assert_eq!(list[1].host_key.as_deref(), Some(hk.as_str()));
    let (again, id2) = pair_once();
    assert_eq!(again.len(), 2, "the same host key: the same entry");
    assert_eq!(id2, id);
    assert_eq!(again[0], mine);
}
