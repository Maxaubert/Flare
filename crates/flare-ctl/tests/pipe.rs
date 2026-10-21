//! Real named-pipe round trips, in process, against a fake dispatcher. Each test uses its own pipe
//! name, so they run in parallel without meeting each other (or a running Flare).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use flare_ctl::framing::Wire;
use flare_ctl::{
    CallContext, Client, ClientError, CtlError, Dispatcher, Grant, HelloParams, Method, Permission,
    Server, ServerConfig, code, connect, pipe_name,
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
    fn tab_exists(&self, tab: &str) -> bool {
        tab == "t1"
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

fn hello(name: &str, extra_tab: Option<&str>, token: Option<&str>) -> Client<flare_ctl::PipeEnd> {
    let mut c = connect(name, WAIT).expect("connect");
    c.hello(
        &HelloParams {
            client: "test".into(),
            version: "1".into(),
            token: token.map(str::to_string),
            tab: extra_tab.map(str::to_string),
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
                tab: Some("t1".into()),
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
    let mut c = hello(&name, Some("t1"), None);
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
            tab: None,
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
            tab: None,
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
fn connecting_to_no_server_is_an_error_not_a_hang() {
    let started = std::time::Instant::now();
    let r = connect(&unique_name(), WAIT);
    assert!(matches!(r, Err(ClientError::Io(_))));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn published_events_reach_the_matching_client() {
    let (server, name) = start(ServerConfig::default());
    let mut a = hello(&name, Some("t1"), None);
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
