//! Real named-pipe round trips, in process, against a fake dispatcher. Each test uses its own pipe
//! name, so they run in parallel without meeting each other (or a running Flare).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use flare_ctl::framing::Wire;
use flare_ctl::{
    CallContext, Client, ClientError, CtlError, Dispatcher, Grant, HelloParams, Method, Permission,
    Server, ServerConfig, code, connect, connect_to_server, pipe_name,
};
use serde_json::{Value, json};

struct Fake;

impl Dispatcher for Fake {
    fn resolve_token(&self, token: &str) -> Option<Grant> {
        (token == "good").then(|| Grant::Mod {
            id: "m".into(),
            permissions: vec![Permission::View],
        })
    }
    fn resolve_tab_token(&self, tab_token: &str) -> Option<String> {
        (tab_token == "tok1").then(|| "t1".to_string())
    }
    fn call(&self, ctx: &CallContext, m: Method, p: Value) -> Result<Value, CtlError> {
        match m {
            Method::ViewFile if p["path"] == "missing" => {
                Err(CtlError::new(code::NOT_FOUND, "no such file"))
            }
            _ => Ok(json!({"method": m.name(), "tab": ctx.tab, "echo": p})),
        }
    }
}

fn unique_name() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "\\\\.\\pipe\\flare-ctl-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

fn start(cfg: ServerConfig) -> (Server, String) {
    let name = unique_name();
    let server = Server::start(&name, cfg, Arc::new(Fake)).expect("server starts");
    (server, name)
}

const WAIT: Duration = Duration::from_secs(10);

fn hello(name: &str, tab_token: Option<&str>, token: Option<&str>) -> Client<flare_ctl::PipeEnd> {
    let mut c = connect(name, WAIT).expect("connect");
    c.hello(
        &HelloParams {
            client: "test".into(),
            version: "1".into(),
            token: token.map(str::to_string),
            tab_token: tab_token.map(str::to_string),
        },
        WAIT,
    )
    .expect("hello");
    c
}

fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..500 {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn hello_and_a_call_round_trip() {
    let (server, name) = start(ServerConfig::default());
    let mut c = connect(&name, WAIT).unwrap();
    let info = c
        .hello(
            &HelloParams {
                client: "test".into(),
                version: "1".into(),
                token: None,
                tab_token: Some("tok1".into()),
            },
            WAIT,
        )
        .unwrap();
    assert_eq!(info.protocol, 1);
    assert_eq!(info.grant_kind, "agent");
    assert_eq!(info.tab.as_deref(), Some("t1"));
    assert!(info.can(Method::ViewImage));
    assert!(!info.can(Method::ShellInput));
    let v = c
        .call(Method::ViewImage, json!({"path": "C:\\x.png"}), WAIT)
        .unwrap();
    assert_eq!(v["method"], "view.image");
    assert_eq!(v["tab"], "t1");
    assert_eq!(v["echo"]["path"], "C:\\x.png");
    server.stop();
}

#[test]
fn remote_errors_and_grants_travel_over_the_pipe() {
    let (server, name) = start(ServerConfig::default());
    let mut c = hello(&name, Some("tok1"), None);
    match c.call(Method::ViewFile, json!({"path": "missing"}), WAIT) {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("{other:?}"),
    }
    match c.call(Method::SettingsSet, json!({}), WAIT) {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, code::FORBIDDEN),
        other => panic!("{other:?}"),
    }
    // The connection is still good after both.
    assert!(c.call(Method::PanelState, json!({}), WAIT).is_ok());
    server.stop();
}

#[test]
fn a_mod_token_works_and_a_bad_one_is_refused() {
    let (server, name) = start(ServerConfig::default());
    let mut c = hello(&name, None, Some("good"));
    assert!(c.call(Method::ViewText, json!({"text": "x"}), WAIT).is_ok());
    assert!(matches!(
        c.call(Method::PanelClose, json!({}), WAIT),
        Err(ClientError::Remote(e)) if e.code == code::FORBIDDEN
    ));
    let mut bad = connect(&name, WAIT).unwrap();
    let r = bad.hello(
        &HelloParams {
            client: "x".into(),
            version: "1".into(),
            token: Some("nope".into()),
            tab_token: None,
        },
        WAIT,
    );
    assert!(matches!(r, Err(ClientError::Remote(e)) if e.code == code::UNAUTHORIZED));
    server.stop();
}

#[test]
fn the_65th_client_is_refused_and_the_first_64_keep_working() {
    let (server, name) = start(ServerConfig::default());
    let mut clients: Vec<_> = (0..64).map(|_| hello(&name, None, None)).collect();
    assert!(wait_for(|| server.client_count() == 64));
    let mut extra = connect(&name, WAIT).unwrap();
    let r = extra.hello(
        &HelloParams {
            client: "x".into(),
            version: "1".into(),
            token: None,
            tab_token: None,
        },
        WAIT,
    );
    assert!(
        matches!(&r, Err(ClientError::Remote(e)) if e.code == code::TOO_MANY_CLIENTS),
        "{r:?}"
    );
    assert!(clients[7].call(Method::PanelState, json!({}), WAIT).is_ok());
    // A slot frees up when a client leaves.
    clients.pop();
    assert!(wait_for(|| server.client_count() == 63));
    let mut again = hello(&name, None, None);
    assert!(again.call(Method::PanelState, json!({}), WAIT).is_ok());
    drop(clients);
    server.stop();
}

#[test]
fn a_malformed_line_closes_only_that_client() {
    let (server, name) = start(ServerConfig::default());
    let mut good = hello(&name, None, None);
    let mut bad = hello(&name, None, None);
    bad.wire().write_all(b"{this is not json\n").unwrap();
    // The refusal arrives as the answer to the next call, then the connection is gone.
    match bad.call(Method::PanelState, json!({}), WAIT) {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, code::PARSE_ERROR),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        bad.call(Method::PanelState, json!({}), WAIT),
        Err(ClientError::Closed | ClientError::Io(_))
    ));
    assert!(good.call(Method::PanelState, json!({}), WAIT).is_ok());
    assert!(wait_for(|| server.client_count() == 1));
    server.stop();
}

#[test]
fn an_oversize_line_closes_only_that_client() {
    let (server, name) = start(ServerConfig {
        max_line: 4096,
        ..ServerConfig::default()
    });
    let mut good = hello(&name, None, None);
    let mut bad = hello(&name, None, None);
    let big = vec![b'x'; 10_000];
    bad.wire().write_all(&big).unwrap();
    bad.wire().write_all(b"\n").unwrap();
    match bad.call(Method::PanelState, json!({}), WAIT) {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, code::TOO_LARGE),
        other => panic!("{other:?}"),
    }
    assert!(good.call(Method::PanelState, json!({}), WAIT).is_ok());
    server.stop();
}

#[test]
fn a_second_server_on_the_same_name_fails_to_start() {
    let (server, name) = start(ServerConfig::default());
    assert!(Server::start(&name, ServerConfig::default(), Arc::new(Fake)).is_err());
    server.stop();
}

#[test]
fn a_client_told_which_process_serves_the_pipe_speaks_to_no_other() {
    // A process of the same user that made the pipe name first (a squatter) is not the Flare the shell
    // was told about: the client leaves before it says a word, so the tab's secret is never handed over.
    let (server, name) = start(ServerConfig::default());
    let me = std::process::id();
    assert!(
        connect_to_server(&name, WAIT, me).is_ok(),
        "the named process serves it"
    );
    for wrong in [me.wrapping_add(4), 4, 0] {
        match connect_to_server(&name, WAIT, wrong) {
            Err(ClientError::Protocol(m)) => assert!(m.contains("not served by the Flare"), "{m}"),
            Err(other) => panic!("{other:?}"),
            Ok(_) => panic!("connected to a server that is not process {wrong}"),
        }
    }
    server.stop();
}

#[test]
fn connecting_to_no_server_is_an_error_not_a_hang() {
    let started = std::time::Instant::now();
    let r = connect(&unique_name(), WAIT);
    assert!(matches!(r, Err(ClientError::Io(_))));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn published_events_reach_the_matching_client() {
    let (server, name) = start(ServerConfig::default());
    let mut a = hello(&name, Some("tok1"), None);
    let mut b = hello(&name, None, None);
    assert_eq!(
        server.publish("panel.changed", json!({"open": true}), Some("t1")),
        1
    );
    let ev = a.wait_event(WAIT).unwrap().expect("an event");
    assert_eq!(ev.event, "panel.changed");
    assert_eq!(ev.params["open"], true);
    assert!(b.wait_event(Duration::from_millis(100)).unwrap().is_none());
    server.stop();
}

#[test]
fn stopping_the_server_disconnects_clients_and_frees_the_name() {
    let (server, name) = start(ServerConfig::default());
    let mut c = hello(&name, None, None);
    server.stop();
    assert!(
        c.call(Method::PanelState, json!({}), Duration::from_secs(2))
            .is_err()
    );
    assert!(connect(&name, Duration::from_millis(200)).is_err());
    // The name can be taken again.
    let again = Server::start(&name, ServerConfig::default(), Arc::new(Fake)).unwrap();
    again.stop();
}

#[test]
fn the_pipe_name_is_per_user_and_per_scope() {
    let a = pipe_name("S-1-5-21-1", None);
    assert!(a.starts_with("\\\\.\\pipe\\flare-ctl-"));
    assert_eq!(a, pipe_name("S-1-5-21-1", None));
    assert_ne!(a, pipe_name("S-1-5-21-2", None));
    assert_ne!(a, pipe_name("S-1-5-21-1", Some("e2e")));
    assert_ne!(
        pipe_name("S-1-5-21-1", Some("a")),
        pipe_name("S-1-5-21-1", Some("b"))
    );
    assert!(flare_ctl::pipe::user_pipe_name(None).is_some());
}

/// Known answers, copied into docs/surfaces/protocol.md section 1 for the Node port to check its
/// FNV-1a 64 against. FNV-1a 64 of "a" is af63dc4c8601ec8c and of "" is the offset basis.
#[test]
fn the_pipe_name_has_known_answers() {
    assert_eq!(
        pipe_name("S-1-5-21-1", None),
        "\\\\.\\pipe\\flare-ctl-81be4daa4a464916"
    );
    assert_eq!(
        pipe_name("S-1-5-21-1", Some("e2e")),
        "\\\\.\\pipe\\flare-ctl-81be4daa4a464916-f1179b09"
    );
    assert_eq!(
        pipe_name("S-1-5-21-1111111111-2222222222-3333333333-1001", None),
        "\\\\.\\pipe\\flare-ctl-df660900cac14fc2"
    );
    // Both the SID and the scope are lower-cased before hashing.
    assert_eq!(pipe_name("s-1-5-21-1", None), pipe_name("S-1-5-21-1", None));
    assert_eq!(
        pipe_name("S-1-5-21-1", Some("E2E")),
        pipe_name("S-1-5-21-1", Some("e2e"))
    );
}

#[test]
fn the_pipe_is_closed_to_other_users() {
    // One protected ACE for the current user. Reading the live pipe's descriptor back would need the
    // security APIs; this pins the SDDL the server builds from.
    let sid = flare_ctl::pipe::user_sid().unwrap();
    assert_eq!(
        flare_ctl::pipe::dacl_sddl(&sid),
        format!("D:P(A;;GA;;;{sid})")
    );
    assert!(flare_ctl::pipe::dacl_sddl(&sid).starts_with("D:P(A;;GA;;;S-1-"));
}

/// The descriptor on the LIVE pipe, read back through the security APIs: one protected ACE for the
/// current user. (A server that forgot to pass its attributes would leave the default DACL.)
#[test]
fn the_live_pipe_carries_the_owner_only_descriptor() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
        SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    let (server, name) = start(ServerConfig::default());
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .expect("open the pipe");
    let mut sd = std::ptr::null_mut();
    // SAFETY: the handle is the open pipe; `sd` receives a descriptor freed below.
    let rc = unsafe {
        GetSecurityInfo(
            file.as_raw_handle() as _,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    assert_eq!(rc, 0, "GetSecurityInfo failed");
    let mut text: *mut u16 = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: `sd` is the descriptor just read; `text` receives a LocalAlloc'd string.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            &mut len,
        )
    };
    assert_ne!(ok, 0);
    // SAFETY: `text` is `len` UTF-16 units including the NUL.
    let sddl = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len as usize) })
        .trim_end_matches('\0')
        .to_string();
    // SAFETY: both came from the calls above and are freed once.
    unsafe {
        LocalFree(text as _);
        LocalFree(sd);
    }
    let sid = flare_ctl::pipe::user_sid().unwrap();
    // Generic-all reads back as file-all; there is exactly one ACE and the DACL is protected.
    assert_eq!(sddl, format!("D:P(A;;FA;;;{sid})"));
    drop(file);
    server.stop();
}

/// A client that never reads, with more queued events than the pipe holds: the worker is parked in
/// a write. Stopping must still finish well inside its wait, with nothing left running.
#[test]
fn stop_frees_a_worker_blocked_writing_to_a_client_that_never_reads() {
    let (server, name) = start(ServerConfig::default());
    let c = hello(&name, Some("tok1"), None);
    assert!(wait_for(|| server.client_count() == 1));
    let big = "x".repeat(200_000);
    for _ in 0..20 {
        server.publish("panel.changed", json!({"pad": big}), None);
    }
    std::thread::sleep(Duration::from_millis(300));
    let t = std::time::Instant::now();
    let left = server.stop();
    assert!(
        t.elapsed() < Duration::from_millis(1500),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(left, 0);
    drop(c);
}
