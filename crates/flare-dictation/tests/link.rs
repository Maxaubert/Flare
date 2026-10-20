//! The link between copies (Task 20, N26). Ported from Prism Terminal's `dictationLink.test.ts` at e5848cb, plus
//! the caps and the drops its prose promises. Every test uses a pipe named for the test, with its own key.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_dictation::hmac::{hmac_sha256, to_hex};
use flare_dictation::link::*;
use flare_dictation::route::{RemoteCopy, SendDecision, SendTab};

fn tab(over: impl FnOnce(&mut SendTab)) -> SendTab {
    let mut t = SendTab {
        id: "p1".into(),
        tab: "t1".into(),
        label: "Wind".into(),
        cwd: "C:\\src\\wind".into(),
        agent: Some("claude".into()),
        state: None,
        orchestrator: false,
        session: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into()),
        copy: None,
    };
    over(&mut t);
    t
}

const N: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MAC: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn key() -> [u8; 32] {
    let mut k = [0u8; 32];
    assert!(flare_dictation::hmac::random_bytes(&mut k));
    k
}

fn pipe_name() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!(
        "\\\\.\\pipe\\flare-dict-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

fn until(mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    f()
}

// ---- the messages ----

#[test]
fn reads_what_a_client_may_say_and_nothing_else() {
    let hello = format!(r#"{{"t":"hello","pid":42,"name":"Stable","n":"{N}","mac":"{MAC}"}}"#);
    assert_eq!(
        parse_to_service(&hello),
        Some(ToService::Hello {
            pid: 42,
            name: "Stable".into(),
            n: N.into(),
            mac: MAC.into()
        })
    );
    assert_eq!(
        parse_to_service(r#"{"t":"hello","pid":42,"name":"Stable"}"#),
        None
    );
    let tabs = format!(
        r#"{{"t":"tabs","rid":3,"tabs":[{},{{"id":5}}]}}"#,
        serde_json::to_string(&serde_json::json!({
            "id":"p1","tab":"t1","label":"Wind","cwd":"C:\\src\\wind","agent":"claude","state":null,
            "orchestrator":false,"session":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        }))
        .unwrap()
    );
    assert_eq!(
        parse_to_service(&tabs),
        Some(ToService::Tabs {
            rid: 3,
            tabs: vec![tab(|_| {})]
        })
    );
    assert_eq!(
        parse_to_service(r#"{"t":"sent","rid":4,"res":{"ok":false,"reason":"asking"}}"#),
        Some(ToService::Sent {
            rid: 4,
            res: SendDecision::No("asking".into())
        })
    );
    assert_eq!(parse_to_service(r#"{"t":"paste","text":"x"}"#), None);
    assert_eq!(parse_to_service("not json"), None);
    assert_eq!(parse_to_service("[1,2]"), None);
}

#[test]
fn reads_what_the_service_may_say_and_caps_the_text() {
    assert_eq!(
        parse_to_client(&format!(r#"{{"t":"nonce","n":"{N}"}}"#)),
        Some(ToClient::Nonce { n: N.into() })
    );
    assert_eq!(
        parse_to_client(&format!(r#"{{"t":"proof","mac":"{MAC}"}}"#)),
        Some(ToClient::Proof { mac: MAC.into() })
    );
    assert_eq!(
        parse_to_client(r#"{"t":"tabs?","rid":1}"#),
        Some(ToClient::TabsAsk { rid: 1 })
    );
    assert_eq!(
        parse_to_client(r#"{"t":"paste","text":"hi"}"#),
        Some(ToClient::Paste { text: "hi".into() })
    );
    let long = "x".repeat(9000);
    assert_eq!(
        parse_to_client(&format!(
            r#"{{"t":"send","rid":2,"id":"p1","text":"{long}"}}"#
        )),
        None
    );
    let ok = "y".repeat(8000);
    assert!(parse_to_client(&format!(
        r#"{{"t":"send","rid":2,"id":"p1","text":"{ok}"}}"#
    ))
    .is_some());
    // Hex fields are exact: upper case, short and long are refused.
    assert_eq!(parse_to_client(r#"{"t":"nonce","n":"AAAA"}"#), None);
    assert_eq!(
        parse_to_client(&format!(r#"{{"t":"nonce","n":"{}"}}"#, "A".repeat(32))),
        None
    );
}

#[test]
fn the_caps_of_a_tab_list() {
    let many: Vec<serde_json::Value> = (0..100)
        .map(|i| serde_json::json!({"id": format!("p{i}"), "tab": "t", "label": "l", "cwd": "c"}))
        .collect();
    let line = serde_json::json!({"t":"tabs","rid":1,"tabs":many}).to_string();
    let Some(ToService::Tabs { tabs, .. }) = parse_to_service(&line) else {
        panic!("a tab list")
    };
    assert_eq!(tabs.len(), 64);
    // Over a field's cap the tab is left out, the rest stay.
    let line = serde_json::json!({"t":"tabs","rid":1,"tabs":[
        {"id": "i".repeat(129), "tab":"t","label":"l","cwd":"c"},
        {"id": "i".repeat(128), "tab":"t","label":"l".repeat(256),"cwd":"c".repeat(1024)},
    ]})
    .to_string();
    let Some(ToService::Tabs { tabs, .. }) = parse_to_service(&line) else {
        panic!("a tab list")
    };
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].id.len(), 128);
    // A name over 64 and a reason over 200 are refused.
    let hello = format!(
        r#"{{"t":"hello","pid":1,"name":"{}","n":"{N}","mac":"{MAC}"}}"#,
        "n".repeat(65)
    );
    assert_eq!(parse_to_service(&hello), None);
    let sent = format!(
        r#"{{"t":"sent","rid":1,"res":{{"ok":false,"reason":"{}"}}}}"#,
        "r".repeat(201)
    );
    assert_eq!(parse_to_service(&sent), None);
    // A line over 64 KiB is nothing.
    let big = format!(r#"{{"t":"paste","text":"a"}}{}"#, " ".repeat(MAX_LINE + 1));
    assert_eq!(parse_to_client(&big), None);
}

#[test]
fn messages_survive_their_own_encoding() {
    let t = tab(|_| {});
    let msgs = [
        ToService::Hello {
            pid: 7,
            name: "Flare".into(),
            n: N.into(),
            mac: MAC.into(),
        },
        ToService::Tabs {
            rid: 9,
            tabs: vec![t.clone(), tab(|x| x.state = Some("question".into()))],
        },
        ToService::Sent {
            rid: 2,
            res: SendDecision::Ok(t.clone()),
        },
        ToService::Sent {
            rid: 3,
            res: SendDecision::No("no".into()),
        },
    ];
    for m in msgs {
        let line = encode_to_service(&m);
        assert!(line.ends_with('\n') && !line[..line.len() - 1].contains('\n'));
        assert_eq!(parse_to_service(line.trim_end()), Some(m));
    }
    let msgs = [
        ToClient::Nonce { n: N.into() },
        ToClient::Proof { mac: MAC.into() },
        ToClient::TabsAsk { rid: 1 },
        ToClient::Send {
            rid: 2,
            id: "p1".into(),
            text: "line one\nline \"two\"".into(),
        },
        ToClient::Paste {
            text: "tab\tand \u{e5}".into(),
        },
    ];
    for m in msgs {
        let line = encode_to_client(&m);
        assert!(line.ends_with('\n') && !line[..line.len() - 1].contains('\n'));
        assert_eq!(parse_to_client(line.trim_end()), Some(m));
    }
}

#[test]
fn names_a_copy_by_its_exe() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let dev = root.join("target").join("debug").join("flare.exe");
    assert_eq!(copy_name(&dev), "Flare (Dev)");
    // A packaged install: Update.exe beside a `current` folder that holds the exe.
    std::fs::create_dir_all(root.join("app").join("current")).unwrap();
    std::fs::write(root.join("app").join("Update.exe"), b"").unwrap();
    let exe = root.join("app").join("current").join("flare.exe");
    assert_eq!(copy_name(&exe), "Flare");
}

// ---- the key and the pipe's name ----

#[test]
fn the_key_is_made_once_and_the_pipe_is_named_by_its_hash() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sub").join("link.key");
    let a = link_key(&file).expect("a key");
    assert_eq!(link_key(&file), Some(a));
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.len(), 64);
    assert!(text
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    let pipe = link_pipe(&a);
    let tail = pipe
        .strip_prefix("\\\\.\\pipe\\flare-dictation-")
        .expect("Flare's own prefix");
    assert_eq!(tail.len(), 24);
    assert!(tail.bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(!pipe.contains(&to_hex(&a)[..24]));
    // A key file that is not 64 hex digits (cut short or emptied by a crash) is no key: it is replaced
    // atomically by a fresh one, and that one then stays (audit 2026-10-20 L38).
    for junk in ["nope", ""] {
        let bad = dir.path().join("bad.key");
        std::fs::write(&bad, junk).unwrap();
        let fresh = link_key(&bad).expect("a bad key file is replaced by a fresh key");
        assert_eq!(link_key(&bad), Some(fresh));
        let text = std::fs::read_to_string(&bad).unwrap();
        assert_eq!(text.len(), 64);
        assert_eq!(text, to_hex(&fresh));
    }
    // No stray temporary file is left beside the key.
    let strays = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(strays, 0);
}

#[test]
fn two_copies_starting_together_agree_on_one_key() {
    let dir = tempfile::tempdir().unwrap();
    let file = Arc::new(dir.path().join("link.key"));
    let found: Vec<_> = (0..6)
        .map(|_| {
            let f = Arc::clone(&file);
            std::thread::spawn(move || link_key(&f))
        })
        .collect();
    let keys: Vec<_> = found.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(keys[0].is_some());
    assert!(keys.iter().all(|k| *k == keys[0]));
}

// ---- the pipe ----

struct Handlers {
    pasted: Mutex<Vec<String>>,
    sent: Mutex<Vec<String>>,
    delay: Duration,
}

impl Handlers {
    fn new(delay: Duration) -> Arc<Handlers> {
        Arc::new(Handlers {
            pasted: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            delay,
        })
    }
}

impl ClientHandlers for Handlers {
    fn tabs(&self) -> Vec<SendTab> {
        std::thread::sleep(self.delay);
        vec![tab(|t| {
            t.id = "r1".into();
            t.label = "Sonara".into();
        })]
    }
    fn send(&self, id: &str, text: &str) -> SendDecision {
        std::thread::sleep(self.delay);
        self.sent.lock().unwrap().push(format!("{id}:{text}"));
        SendDecision::Ok(tab(|t| {
            t.id = id.into();
            t.label = "Sonara".into();
        }))
    }
    fn paste(&self, text: &str) {
        self.pasted.lock().unwrap().push(text.into());
    }
}

fn copy(pid: u32, name: &str) -> RemoteCopy {
    RemoteCopy {
        pid,
        name: name.into(),
    }
}

fn client_with_state(
    pipe: &str,
    key: [u8; 32],
    me: RemoteCopy,
    h: Arc<Handlers>,
) -> (LinkClient, Arc<Mutex<bool>>) {
    let trusted = Arc::new(Mutex::new(false));
    let t = Arc::clone(&trusted);
    let c = LinkClient::start(pipe, key, me, h, 50, move |on| *t.lock().unwrap() = on);
    (c, trusted)
}

#[test]
fn asks_a_connected_copy_for_its_tabs_sends_into_one_and_forgets_it_when_it_closes() {
    let pipe = pipe_name();
    let k = key();
    let svc = LinkService::start(&pipe, k, |e| panic!("link error: {e}"));
    let h = Handlers::new(Duration::ZERO);
    let (client, trusted) = client_with_state(&pipe, k, copy(777, "Flare"), Arc::clone(&h));
    assert!(until(|| svc.copies().len() == 1 && *trusted.lock().unwrap()));
    assert_eq!(svc.copies(), vec![copy(777, "Flare")]);

    let tabs = svc.tabs(Duration::from_millis(800)).recv().unwrap();
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].0, copy(777, "Flare"));
    assert_eq!(
        tabs[0].1,
        vec![tab(|t| {
            t.id = "r1".into();
            t.label = "Sonara".into();
        })]
    );

    let res = svc
        .send(777, "r1", "hello", Duration::from_secs(5))
        .recv()
        .unwrap();
    assert_eq!(
        res,
        SendDecision::Ok(tab(|t| {
            t.id = "r1".into();
            t.label = "Sonara".into();
        }))
    );
    assert_eq!(*h.sent.lock().unwrap(), vec!["r1:hello".to_string()]);

    assert!(svc.paste(777, "typed"));
    assert!(until(|| h.pasted.lock().unwrap().len() == 1));
    assert_eq!(*h.pasted.lock().unwrap(), vec!["typed".to_string()]);
    assert!(!svc.paste(1, "nobody"));

    client.close();
    assert!(until(|| svc.copies().is_empty()));
    assert_eq!(
        svc.send(777, "r1", "x", Duration::from_secs(5))
            .recv()
            .unwrap(),
        SendDecision::No("that copy of the app has closed".into())
    );
    assert!(!*trusted.lock().unwrap());
    svc.close();
}

#[test]
fn two_copies_are_two_peers_and_text_goes_where_it_was_sent() {
    let pipe = pipe_name();
    let k = key();
    let svc = LinkService::start(&pipe, k, |e| panic!("link error: {e}"));
    let (h1, h2) = (Handlers::new(Duration::ZERO), Handlers::new(Duration::ZERO));
    let (c1, _) = client_with_state(&pipe, k, copy(1, "Flare"), Arc::clone(&h1));
    let (c2, _) = client_with_state(&pipe, k, copy(2, "Flare"), Arc::clone(&h2));
    assert!(until(|| svc.copies().len() == 2));
    let tabs = svc.tabs(Duration::from_secs(2)).recv().unwrap();
    assert_eq!(tabs.len(), 2);
    assert!(tabs.iter().all(|(_, t)| t.len() == 1));
    svc.send(2, "r1", "to two", Duration::from_secs(5))
        .recv()
        .unwrap();
    assert!(h1.sent.lock().unwrap().is_empty());
    assert_eq!(*h2.sent.lock().unwrap(), vec!["r1:to two".to_string()]);
    c1.close();
    c2.close();
    svc.close();
}

#[test]
fn a_copy_that_does_not_answer_is_left_without_tabs_and_a_send_says_so() {
    let pipe = pipe_name();
    let k = key();
    let svc = LinkService::start(&pipe, k, |e| panic!("link error: {e}"));
    let h = Handlers::new(Duration::from_millis(1500));
    let (client, _) = client_with_state(&pipe, k, copy(5, "Flare"), Arc::clone(&h));
    assert!(until(|| svc.copies().len() == 1));
    let started = Instant::now();
    let tabs = svc.tabs(Duration::from_millis(200)).recv().unwrap();
    assert!(started.elapsed() < Duration::from_millis(1200));
    assert_eq!(tabs.len(), 1);
    assert!(tabs[0].1.is_empty());
    let res = svc
        .send(5, "r1", "late", Duration::from_millis(200))
        .recv()
        .unwrap();
    assert_eq!(
        res,
        SendDecision::No("that copy of the app did not answer".into())
    );
    client.close();
    svc.close();
}

#[test]
fn no_copies_means_an_empty_list_at_once() {
    let pipe = pipe_name();
    let svc = LinkService::start(&pipe, key(), |e| panic!("link error: {e}"));
    assert!(svc.copies().is_empty());
    assert!(svc
        .tabs(Duration::from_millis(800))
        .recv_timeout(Duration::from_millis(500))
        .unwrap()
        .is_empty());
    svc.close();
}

#[test]
fn drops_a_client_that_does_not_hold_the_key_it_is_never_listed() {
    let pipe = pipe_name();
    let svc = LinkService::start(&pipe, key(), |e| panic!("link error: {e}"));
    let (client, trusted) = client_with_state(
        &pipe,
        key(),
        copy(9, "Impostor"),
        Handlers::new(Duration::ZERO),
    );
    std::thread::sleep(Duration::from_millis(500));
    assert!(svc.copies().is_empty());
    assert!(!*trusted.lock().unwrap());
    client.close();
    svc.close();
}

#[test]
fn never_answers_types_or_pastes_for_a_service_that_does_not_hold_the_key() {
    let pipe = pipe_name();
    // The impostor took the pipe's name and has a key of its own.
    let fake = LinkService::start(&pipe, key(), |e| panic!("link error: {e}"));
    let h = Handlers::new(Duration::ZERO);
    let (client, trusted) = client_with_state(&pipe, key(), copy(5, "Flare"), Arc::clone(&h));
    std::thread::sleep(Duration::from_millis(500));
    assert!(!*trusted.lock().unwrap());
    // It lists nobody, so it cannot even ask, and a client hangs up on a request before the proof.
    assert!(fake.copies().is_empty());
    assert!(h.sent.lock().unwrap().is_empty());
    assert!(h.pasted.lock().unwrap().is_empty());
    client.close();
    fake.close();
}

#[test]
fn a_second_service_on_the_same_name_is_refused() {
    let pipe = pipe_name();
    let k = key();
    let first = LinkService::start(&pipe, k, |e| panic!("link error: {e}"));
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let e = Arc::clone(&errors);
    let second = LinkService::start(&pipe, k, move |m| e.lock().unwrap().push(m));
    assert_eq!(errors.lock().unwrap().len(), 1);
    assert!(second.copies().is_empty());
    second.close();
    first.close();
}

#[test]
fn a_client_finds_the_service_when_it_starts_later() {
    let pipe = pipe_name();
    let k = key();
    let (client, trusted) =
        client_with_state(&pipe, k, copy(3, "Flare"), Handlers::new(Duration::ZERO));
    std::thread::sleep(Duration::from_millis(200));
    assert!(!*trusted.lock().unwrap());
    let svc = LinkService::start(&pipe, k, |e| panic!("link error: {e}"));
    assert!(until(|| *trusted.lock().unwrap()));
    // The service ends: the client says it is no longer linked, and tries again.
    svc.close();
    assert!(until(|| !*trusted.lock().unwrap()));
    let again = LinkService::start(&pipe, k, |e| panic!("link error: {e}"));
    assert!(until(|| *trusted.lock().unwrap()));
    client.close();
    again.close();
}

// ---- the drops ----

/// A bare pipe client with synchronous I/O, standing in for a peer that speaks the wire by hand.
fn raw(pipe: &str) -> std::fs::File {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe)
        .expect("the pipe")
}

/// Reads until the peer hangs up; the bytes it sent first. On its own thread so a hang fails the test.
fn read_to_end_within(mut f: std::fs::File, ms: u64) -> Option<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut all = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match f.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => all.extend_from_slice(&buf[..n]),
            }
        }
        let _ = tx.send(all);
    });
    rx.recv_timeout(Duration::from_millis(ms)).ok()
}

fn nonce_of(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let first = text.lines().next().expect("a nonce line");
    match parse_to_client(first) {
        Some(ToClient::Nonce { n }) => n,
        other => panic!("not a nonce: {other:?}"),
    }
}

fn read_nonce_line(f: &mut std::fs::File) -> Vec<u8> {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    while f.read(&mut b).unwrap_or(0) == 1 {
        line.push(b[0]);
        if b[0] == b'\n' {
            break;
        }
    }
    line
}

#[test]
fn a_line_over_64_kib_drops_the_peer() {
    let pipe = pipe_name();
    let svc = LinkService::start(&pipe, key(), |e| panic!("link error: {e}"));
    let mut f = raw(&pipe);
    let first = read_nonce_line(&mut f);
    assert!(!first.is_empty());
    let mut line = vec![b'a'; MAX_LINE + 10];
    line.push(b'\n');
    // A synchronous handle does its I/O one call at a time, so: write (the service keeps reading until it
    // hangs up, and the write then fails, which is the point), then read what is left.
    let _ = f.write_all(&line);
    let rest = read_to_end_within(f, 4000).expect("the service hung up");
    assert!(rest.is_empty(), "answered a peer it dropped");
    svc.close();
}

#[test]
fn a_buffer_that_never_ends_a_line_drops_the_peer_too() {
    let pipe = pipe_name();
    let svc = LinkService::start(&pipe, key(), |e| panic!("link error: {e}"));
    let mut f = raw(&pipe);
    read_nonce_line(&mut f);
    let junk = vec![b'z'; 300 * 1024];
    let _ = f.write_all(&junk);
    assert!(read_to_end_within(f, 4000).is_some());
    svc.close();
}

#[test]
fn a_peer_that_fails_the_proof_is_answered_nothing_and_dropped() {
    let pipe = pipe_name();
    let svc = LinkService::start(&pipe, key(), |e| panic!("link error: {e}"));
    let mut f = raw(&pipe);
    let n = nonce_of(&read_nonce_line(&mut f));
    assert_eq!(n.len(), 32);
    // A well-formed hello with a MAC made with the wrong key.
    let mine = "c".repeat(32);
    let wrong = to_hex(&hmac_sha256(
        &[7u8; 32],
        format!("client|{n}|{mine}").as_bytes(),
    ));
    let hello = encode_to_service(&ToService::Hello {
        pid: 1,
        name: "Impostor".into(),
        n: mine,
        mac: wrong,
    });
    f.write_all(hello.as_bytes()).unwrap();
    let rest = read_to_end_within(f, 4000).expect("the service hung up");
    assert!(rest.is_empty(), "a peer that failed was answered: {rest:?}");
    assert!(svc.copies().is_empty());
    svc.close();
}

#[test]
fn a_peer_that_proves_nothing_in_time_is_dropped() {
    let pipe = pipe_name();
    let svc = LinkService::start_with_proof(&pipe, key(), |e| panic!("link error: {e}"), 300);
    let mut f = raw(&pipe);
    read_nonce_line(&mut f);
    let started = Instant::now();
    let rest = read_to_end_within(f, 4000).expect("the service hung up");
    assert!(rest.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(250));
    svc.close();
}

#[test]
fn a_peer_with_the_right_proof_is_answered_with_the_services_own() {
    let pipe = pipe_name();
    let k = key();
    let svc = LinkService::start(&pipe, k, |e| panic!("link error: {e}"));
    let mut f = raw(&pipe);
    let n = nonce_of(&read_nonce_line(&mut f));
    let mine = "d".repeat(32);
    let mac = to_hex(&hmac_sha256(&k, format!("client|{n}|{mine}").as_bytes()));
    let hello = encode_to_service(&ToService::Hello {
        pid: 11,
        name: "Raw".into(),
        n: mine.clone(),
        mac,
    });
    f.write_all(hello.as_bytes()).unwrap();
    let proof = read_nonce_line(&mut f);
    let Some(ToClient::Proof { mac }) = parse_to_client(String::from_utf8_lossy(&proof).trim())
    else {
        panic!("no proof: {proof:?}")
    };
    assert_eq!(
        mac,
        to_hex(&hmac_sha256(&k, format!("service|{mine}|{n}").as_bytes()))
    );
    assert!(until(|| svc.copies() == vec![copy(11, "Raw")]));
    // A second hello on the same connection ends it.
    f.write_all(hello.as_bytes()).unwrap();
    assert!(until(|| svc.copies().is_empty()));
    svc.close();
}

#[test]
fn close_leaves_no_peer_and_frees_the_name() {
    let pipe = pipe_name();
    let k = key();
    let svc = LinkService::start(&pipe, k, |e| panic!("link error: {e}"));
    let (client, _) = client_with_state(&pipe, k, copy(4, "Flare"), Handlers::new(Duration::ZERO));
    assert!(until(|| svc.copies().len() == 1));
    svc.close();
    assert!(svc.copies().is_empty());
    client.close();
    let errors = Arc::new(Mutex::new(0usize));
    let e = Arc::clone(&errors);
    let again = LinkService::start(&pipe, k, move |_| *e.lock().unwrap() += 1);
    assert_eq!(*errors.lock().unwrap(), 0, "the name was not freed");
    again.close();
}

// ---- close, and the client's half of "authenticated both ways" ----

#[test]
fn close_does_not_wait_for_a_copy_that_connects_while_it_runs() {
    let pipe = pipe_name();
    let k = key();
    // The acceptor is held after a connection completes, so close() runs first: the peer is made after the
    // cancel loop, the very window a copy arriving during a quit falls in.
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let go_rx = Mutex::new(go_rx);
    let held_tx = Mutex::new(held_tx);
    let svc = Arc::new(LinkService::start_with_accept_hook(
        &pipe,
        k,
        |e| panic!("link error: {e}"),
        move || {
            let _ = held_tx.lock().unwrap().send(());
            let _ = go_rx.lock().unwrap().recv();
        },
    ));
    // A copy that proves itself and then stays connected, as a live copy does.
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let (stay_tx, stay_rx) = std::sync::mpsc::channel::<()>();
    let p = pipe.clone();
    let raw_copy = std::thread::spawn(move || {
        let mut f = raw(&p);
        let n = nonce_of(&read_nonce_line(&mut f));
        let mine = "e".repeat(32);
        let mac = to_hex(&hmac_sha256(&k, format!("client|{n}|{mine}").as_bytes()));
        let hello = encode_to_service(&ToService::Hello {
            pid: 21,
            name: "Late".into(),
            n: mine,
            mac,
        });
        let _ = f.write_all(hello.as_bytes());
        let _ = done_tx.send(());
        let _ = stay_rx.recv_timeout(Duration::from_secs(20));
    });
    held_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the connection reached the acceptor");
    let (closed_tx, closed_rx) = std::sync::mpsc::channel::<()>();
    let s = Arc::clone(&svc);
    let closer = std::thread::spawn(move || {
        s.close();
        let _ = closed_tx.send(());
    });
    std::thread::sleep(Duration::from_millis(200));
    let _ = go_tx.send(());
    let _ = done_rx.recv_timeout(Duration::from_secs(5));
    let finished = closed_rx.recv_timeout(Duration::from_secs(3)).is_ok();
    // Let the copy go either way, so a hang does not outlive the test.
    let _ = stay_tx.send(());
    let _ = raw_copy.join();
    let _ = closer.join();
    assert!(
        finished,
        "close() waited for a copy that connected during it"
    );
}

/// What the raw service saw of its client.
struct RawServer {
    hello: Option<ToService>,
    file: std::fs::File,
}

/// A service by hand: one instance of the pipe, byte mode, synchronous, connected to the first client.
fn raw_service(pipe: &str) -> std::fs::File {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    let name: Vec<u16> = pipe.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: a NUL-terminated name; default security; the handle is handed to a File that closes it; the
    // connect is blocking (no OVERLAPPED) on a handle we own.
    unsafe {
        let h = CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        );
        assert_ne!(h, INVALID_HANDLE_VALUE, "the raw service's pipe");
        ConnectNamedPipe(h, std::ptr::null_mut());
        std::fs::File::from_raw_handle(h as _)
    }
}
/// Starts a client against a raw service, sends it a nonce and reads the client's hello.
fn raw_service_and_hello(
    pipe: &str,
    k: [u8; 32],
) -> (
    RawServer,
    Arc<Handlers>,
    LinkClient,
    Arc<Mutex<bool>>,
    String,
) {
    // The pipe must exist before the client's first try: its retry would find it anyway.
    let (tx, rx) = std::sync::mpsc::channel();
    let p = pipe.to_string();
    let server = std::thread::spawn(move || {
        let f = raw_service(&p);
        let _ = tx.send(());
        f
    });
    let h = Handlers::new(Duration::ZERO);
    let (client, trusted) = client_with_state(pipe, k, copy(5, "Flare"), Arc::clone(&h));
    rx.recv_timeout(Duration::from_secs(5)).expect("connected");
    let mut file = server.join().unwrap();
    let mine = "f".repeat(32);
    file.write_all(encode_to_client(&ToClient::Nonce { n: mine.clone() }).as_bytes())
        .unwrap();
    let line = read_nonce_line(&mut file);
    let hello = parse_to_service(String::from_utf8_lossy(&line).trim());
    assert!(matches!(hello, Some(ToService::Hello { .. })), "{line:?}");
    (RawServer { hello, file }, h, client, trusted, mine)
}

fn asks() -> Vec<ToClient> {
    vec![
        ToClient::Paste {
            text: "rm -rf".into(),
        },
        ToClient::Send {
            rid: 1,
            id: "p1".into(),
            text: "rm -rf".into(),
        },
        ToClient::TabsAsk { rid: 2 },
    ]
}

#[test]
fn a_client_drops_a_service_that_asks_before_it_has_proved_itself() {
    for ask in asks() {
        let pipe = pipe_name();
        let (mut srv, h, client, trusted, _) = raw_service_and_hello(&pipe, key());
        srv.file
            .write_all(encode_to_client(&ask).as_bytes())
            .unwrap();
        let rest = read_to_end_within(srv.file, 4000).expect("the client hung up");
        assert!(
            rest.is_empty(),
            "the client answered an unproved service: {rest:?}"
        );
        assert!(!*trusted.lock().unwrap());
        assert!(h.pasted.lock().unwrap().is_empty(), "pasted for {ask:?}");
        assert!(h.sent.lock().unwrap().is_empty(), "typed for {ask:?}");
        client.close();
    }
}

#[test]
fn a_client_drops_a_service_whose_proof_has_the_wrong_mac() {
    let pipe = pipe_name();
    let k = key();
    let (mut srv, h, client, trusted, mine) = raw_service_and_hello(&pipe, k);
    let Some(ToService::Hello { n, .. }) = srv.hello.clone() else {
        panic!("a hello")
    };
    // Right shape, made with a key of the impostor's own, then a paste behind it.
    let wrong = to_hex(&hmac_sha256(
        &[9u8; 32],
        format!("service|{n}|{mine}").as_bytes(),
    ));
    let mut out = encode_to_client(&ToClient::Proof { mac: wrong });
    out.push_str(&encode_to_client(&ToClient::Paste {
        text: "rm -rf".into(),
    }));
    srv.file.write_all(out.as_bytes()).unwrap();
    let rest = read_to_end_within(srv.file, 4000).expect("the client hung up");
    assert!(rest.is_empty(), "{rest:?}");
    assert!(!*trusted.lock().unwrap());
    assert!(h.pasted.lock().unwrap().is_empty());
    client.close();
}

#[test]
fn a_client_with_the_right_proof_does_answer_the_same_service() {
    // The control for the two tests above: the raw service is good enough to be trusted when it holds the key.
    let pipe = pipe_name();
    let k = key();
    let (mut srv, h, client, trusted, mine) = raw_service_and_hello(&pipe, k);
    let Some(ToService::Hello { n, .. }) = srv.hello.clone() else {
        panic!("a hello")
    };
    let proof = to_hex(&hmac_sha256(&k, format!("service|{n}|{mine}").as_bytes()));
    let mut out = encode_to_client(&ToClient::Proof { mac: proof });
    out.push_str(&encode_to_client(&ToClient::Paste {
        text: "typed".into(),
    }));
    srv.file.write_all(out.as_bytes()).unwrap();
    assert!(until(|| *trusted.lock().unwrap()));
    assert!(until(|| h.pasted.lock().unwrap().len() == 1));
    client.close();
}

#[test]
fn a_client_hangs_up_on_a_service_that_never_proves_itself() {
    let pipe = pipe_name();
    let (srv, _h, client, trusted, _) = raw_service_and_hello(&pipe, key());
    let started = Instant::now();
    let rest = read_to_end_within(srv.file, 9000).expect("the client hung up");
    assert!(rest.is_empty());
    assert!(
        started.elapsed() >= Duration::from_millis(PROOF_MS - 500),
        "hung up early: {:?}",
        started.elapsed()
    );
    assert!(!*trusted.lock().unwrap());
    client.close();
}
