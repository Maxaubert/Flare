//! Real named-pipe (Windows) and Unix socket (macOS) round trips, in process, against a fake
//! dispatcher. Each test uses its own name, so they run in parallel without meeting each other (or a
//! running Flare). The tests of one system's naming and access control sit behind its cfg, each with
//! a twin on the other system.
#![cfg(any(windows, target_os = "macos"))]

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
            plugin: false,
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

#[cfg(windows)]
fn unique_name() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "\\\\.\\pipe\\flare-ctl-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

/// A socket of this user's own folder, under a name no Flare uses.
#[cfg(target_os = "macos")]
fn unique_name() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    // The folder the server would make, made now for the tests that put a file there first.
    let me = flare_ctl::pipe::own_uid();
    flare_ctl::pipe::ensure_private_dir(&flare_ctl::pipe::socket_dir(), me).unwrap();
    pipe_name(
        &format!(
            "flare-ctl-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ),
        None,
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
            no_events: false,
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
                no_events: false,
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
            no_events: false,
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
            no_events: false,
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
    let mut big = vec![b'x'; 10_000];
    big.push(b'\n');
    // The server may answer and hang up as soon as it has read past the limit, before the rest of the
    // line is written: a broken pipe here is that, not a failure (the refusal is checked below). A Unix socket
    // whose peer has already shut it says so as ENOTCONN on macOS when the write lands after the shutdown, rather
    // than EPIPE (seen on the CI runner), which is the same hang-up.
    if let Err(e) = bad.wire().write_all(&big) {
        assert!(
            matches!(
                e.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::NotConnected
            ),
            "{e}"
        );
    }
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

#[cfg(windows)]
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

#[cfg(windows)]
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

#[cfg(windows)]
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

#[cfg(windows)]
/// The descriptor on the LIVE pipe, read back through the security APIs: one protected ACE for the
/// current user. (A server that forgot to pass its attributes would leave the default DACL.)
#[test]
fn the_live_pipe_carries_the_owner_only_descriptor() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
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
    let sddl = dacl_text(sd);
    // SAFETY: `sd` came from GetSecurityInfo above and is freed once.
    unsafe { LocalFree(sd) };
    let sid = flare_ctl::pipe::user_sid().unwrap();
    // Generic-all reads back as file-all; there is exactly one ACE and the DACL is protected. The
    // expected text goes through the same conversion, because Windows writes some SIDs as an alias:
    // the built-in Administrator (the CI runner's account) reads back as "LA", not its S-1-5-21 SID.
    assert_eq!(sddl, canonical_dacl(&format!("D:P(A;;FA;;;{sid})")));
    drop(file);
    server.stop();
}

#[cfg(windows)]
/// The DACL of `sd` as SDDL text, the way Windows writes it.
fn dacl_text(sd: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR) -> String {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
    let mut text: *mut u16 = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: `sd` is a valid descriptor; `text` receives a LocalAlloc'd string.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            &mut len,
        )
    };
    assert_ne!(ok, 0, "the descriptor did not convert to SDDL");
    // SAFETY: `text` is `len` UTF-16 units including the NUL.
    let sddl = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len as usize) })
        .trim_end_matches('\0')
        .to_string();
    // SAFETY: `text` came from the call above and is freed once.
    unsafe { LocalFree(text as _) };
    sddl
}

#[cfg(windows)]
/// `sddl` parsed and written back by Windows, so a SID it writes as an alias compares equal.
fn canonical_dacl(sddl: &str) -> String {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut sd = std::ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated; `sd` receives a LocalAlloc'd descriptor freed below.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(ok, 0, "not valid SDDL: {sddl}");
    let text = dacl_text(sd);
    // SAFETY: `sd` came from the call above and is freed once.
    unsafe { LocalFree(sd) };
    text
}

#[cfg(windows)]
/// The account domain of this machine (its local users' SID without the RID), looked up from the
/// computer name. A domain or Entra ID account has a different prefix, so the user's SID cannot
/// stand in for it.
fn machine_account_domain() -> String {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{LookupAccountNameW, SID_NAME_USE, SidTypeDomain};
    use windows_sys::Win32::System::SystemInformation::{ComputerNameNetBIOS, GetComputerNameExW};
    let mut name = [0u16; 64];
    let mut name_len = name.len() as u32;
    // SAFETY: `name` holds `name_len` units; the call writes the NetBIOS name and its length.
    let ok = unsafe { GetComputerNameExW(ComputerNameNetBIOS, name.as_mut_ptr(), &mut name_len) };
    assert_ne!(ok, 0, "GetComputerNameExW failed");
    let name: Vec<u16> = name[..name_len as usize]
        .iter()
        .copied()
        .chain(Some(0))
        .collect();
    let mut sid = [0u8; 128];
    let mut sid_len = sid.len() as u32;
    let mut domain = [0u16; 256];
    let mut domain_len = domain.len() as u32;
    let mut kind: SID_NAME_USE = 0;
    // SAFETY: every buffer is passed with its size; `name` is NUL-terminated.
    let ok = unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            name.as_ptr(),
            sid.as_mut_ptr() as _,
            &mut sid_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        )
    };
    assert_ne!(ok, 0, "LookupAccountNameW failed for the computer name");
    assert_eq!(
        kind, SidTypeDomain,
        "the computer name is the account domain"
    );
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` holds a valid SID; `text` receives a LocalAlloc'd string freed below.
    let ok = unsafe { ConvertSidToStringSidW(sid.as_mut_ptr() as _, &mut text) };
    assert_ne!(ok, 0, "ConvertSidToStringSidW failed");
    // SAFETY: `text` is NUL-terminated.
    let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    // SAFETY: `text` holds `len` units before the NUL.
    let out = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    // SAFETY: `text` came from the call above and is freed once.
    unsafe { LocalFree(text as _) };
    out
}

#[cfg(windows)]
/// The DACL text Windows writes for one file-all ACE for `sid`: "LA" for RID 500 of this machine's
/// account domain `machine`, the SID itself for anything else (a domain or Entra ID account too).
fn expected_owner_dacl(sid: &str, machine: &str) -> String {
    if sid == format!("{machine}-500") {
        "D:P(A;;FA;;;LA)".to_string()
    } else {
        format!("D:P(A;;FA;;;{sid})")
    }
}

#[cfg(windows)]
/// The CI runner's account is the built-in Administrator (RID 500), whose SID Windows writes as
/// "LA". Only the local machine's RID 500 gets that alias; another domain's RID 500 and an Entra ID
/// SID keep their own text. So the live-pipe check above holds on a local, domain or Entra account.
#[test]
fn the_built_in_administrator_reads_back_as_its_alias() {
    let machine = machine_account_domain();
    assert!(
        machine.starts_with("S-1-5-21-"),
        "an account domain: {machine}"
    );
    assert_eq!(
        canonical_dacl(&format!("D:P(A;;FA;;;{machine}-500)")),
        "D:P(A;;FA;;;LA)"
    );
    // RID 500 of some other domain, and an Entra ID user: neither is this machine's Administrator.
    for other in ["S-1-5-21-1-2-3-500", "S-1-12-1-1111-2222-3333-4444"] {
        assert_ne!(other, format!("{machine}-500"));
        assert_eq!(
            canonical_dacl(&format!("D:P(A;;FA;;;{other})")),
            expected_owner_dacl(other, &machine),
            "{other}"
        );
    }
    let sid = flare_ctl::pipe::user_sid().unwrap();
    assert_eq!(
        canonical_dacl(&format!("D:P(A;;FA;;;{sid})")),
        expected_owner_dacl(&sid, &machine)
    );
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

// ---- macOS twins of the Windows naming and access tests ------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Path;

    use super::*;
    use flare_ctl::pipe::{
        check_private_dir, lock_path, own_uid, peer_allowed, server_alive, socket_dir,
        socket_file_name, verify_server,
    };

    #[test]
    fn the_socket_name_is_per_user_and_per_scope() {
        let a = pipe_name("501", None);
        assert!(a.starts_with(&*socket_dir().to_string_lossy()), "{a}");
        assert!(a.ends_with(".sock"));
        assert_eq!(a, pipe_name("501", None));
        assert_ne!(a, pipe_name("502", None));
        assert_ne!(a, pipe_name("501", Some("e2e")));
        assert_ne!(pipe_name("501", Some("a")), pipe_name("501", Some("b")));
        let mine = flare_ctl::pipe::user_pipe_name(None).unwrap();
        assert_eq!(mine, pipe_name(&own_uid().to_string(), None));
        assert!(mine.len() <= flare_ctl::pipe::MAX_PATH, "{mine}");
    }

    /// The same hashes as the Windows pipe name (docs/surfaces/protocol.md section 1): the file name is
    /// the Windows name's last part, with `.sock`.
    #[test]
    fn the_socket_name_has_the_pipe_names_known_answers() {
        assert_eq!(
            socket_file_name("S-1-5-21-1", None),
            "81be4daa4a464916.sock"
        );
        assert_eq!(
            socket_file_name("S-1-5-21-1", Some("e2e")),
            "81be4daa4a464916-f1179b09.sock"
        );
        assert_eq!(
            socket_file_name("S-1-5-21-1111111111-2222222222-3333333333-1001", None),
            "df660900cac14fc2.sock"
        );
        assert_eq!(
            socket_file_name("s-1-5-21-1", None),
            socket_file_name("S-1-5-21-1", None)
        );
        assert_eq!(
            socket_file_name("S-1-5-21-1", Some("E2E")),
            socket_file_name("S-1-5-21-1", Some("e2e"))
        );
    }

    /// The owner-only descriptor's twin: the LIVE socket is mode 0600 and this user's, in a folder of
    /// this user's with no group or other bits.
    #[test]
    fn the_live_socket_is_mode_0600_in_a_0700_folder_of_this_user() {
        let (server, name) = start(ServerConfig::default());
        let meta = std::fs::symlink_metadata(&name).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600, "{:o}", meta.mode());
        assert_eq!(meta.uid(), own_uid());
        let dir = std::fs::symlink_metadata(Path::new(&name).parent().unwrap()).unwrap();
        assert!(dir.is_dir());
        assert_eq!(dir.mode() & 0o777, 0o700, "{:o}", dir.mode());
        assert_eq!(dir.uid(), own_uid());
        server.stop();
    }

    /// The closed-to-other-users twin. A test cannot switch users, so the uid rule is checked as the
    /// pure function both ends use, and then on the live socket with each end believing it runs as
    /// another uid.
    #[test]
    fn the_socket_is_closed_to_other_users() {
        let me = own_uid();
        assert!(peer_allowed(me, Some(me)));
        assert!(!peer_allowed(me, Some(me + 1)));
        assert!(!peer_allowed(me, Some(0)));
        assert!(!peer_allowed(me, None), "an unreadable uid is refused");
        assert!(verify_server(me, Some(me), Some(7), None).is_ok());
        assert!(verify_server(me, Some(me), Some(7), Some(7)).is_ok());
        for (uid, pid, want) in [
            (Some(me + 1), Some(7), None),
            (None, Some(7), None),
            (Some(me), None, None),
            (Some(me), Some(7), Some(8)),
        ] {
            assert!(
                matches!(
                    verify_server(me, uid, pid, want),
                    Err(ClientError::Protocol(_))
                ),
                "{uid:?} {pid:?} {want:?}"
            );
        }
        match verify_server(me, Some(me + 1), Some(7), None) {
            Err(ClientError::Protocol(m)) => assert!(m.contains("another user"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    /// A server that believes it is another user meets this process as a stranger: the connection is
    /// dropped before a byte is read, and the dispatcher never hears of it.
    #[test]
    fn the_live_server_refuses_a_client_of_another_uid() {
        let name = unique_name();
        let server = Server::start_as(
            &name,
            ServerConfig::default(),
            Arc::new(Fake),
            own_uid() + 1,
        )
        .unwrap();
        // The server drops a stranger as soon as it accepts it. Under load that can come before the
        // client has asked who serves the socket, and the client then refuses at the connect: the same
        // refusal, seen one step earlier. Either way no hello gets through.
        match connect(&name, WAIT) {
            Err(ClientError::Protocol(m)) => assert!(m.contains("who serves"), "{m}"),
            Err(e) => panic!("{e:?}"),
            Ok(mut c) => {
                let r = c.hello(
                    &HelloParams {
                        client: "x".into(),
                        version: "1".into(),
                        token: None,
                        tab_token: Some("tok1".into()),
                        no_events: false,
                    },
                    WAIT,
                );
                assert!(
                    matches!(r, Err(ClientError::Closed | ClientError::Io(_))),
                    "{r:?}"
                );
            }
        }
        assert!(wait_for(|| server.client_count() == 0));
        server.stop();
    }

    /// A client that believes it is another user meets the real server as a stranger and leaves before
    /// it says a word.
    #[test]
    fn the_live_client_refuses_a_server_of_another_uid() {
        let (server, name) = start(ServerConfig::default());
        match flare_ctl::pipe::connect_as(&name, WAIT, own_uid() + 1) {
            Err(ClientError::Protocol(m)) => assert!(m.contains("another user"), "{m}"),
            Err(other) => panic!("{other:?}"),
            Ok(_) => panic!("spoke to a server of another user"),
        }
        server.stop();
    }

    #[test]
    fn a_folder_open_to_others_or_of_another_user_is_refused() {
        assert!(check_private_dir(true, 501, 0o700, 501).is_ok());
        assert!(check_private_dir(true, 501, 0o750, 501).is_err());
        assert!(check_private_dir(true, 501, 0o707, 501).is_err());
        assert!(check_private_dir(true, 0, 0o700, 501).is_err());
        assert!(check_private_dir(false, 501, 0o700, 501).is_err());
        // On disk: a socket asked for in a folder open to the group is never bound.
        let dir = socket_dir().join(format!("open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o770)).unwrap();
        let name = dir.join("x.sock");
        let r = Server::start(
            &name.to_string_lossy(),
            ServerConfig::default(),
            Arc::new(Fake),
        );
        assert!(r.is_err());
        assert!(!name.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A Flare that crashed leaves its socket behind: the next start removes it and serves.
    #[test]
    fn a_dead_socket_is_replaced_and_a_file_in_the_way_is_not() {
        let name = unique_name();
        {
            let l = std::os::unix::net::UnixListener::bind(&name).unwrap();
            drop(l); // the socket file stays, nobody listens
        }
        assert!(Path::new(&name).exists());
        let server = Server::start(&name, ServerConfig::default(), Arc::new(Fake)).unwrap();
        let mut c = hello(&name, None, None);
        assert!(c.call(Method::PanelState, json!({}), WAIT).is_ok());
        server.stop();
        assert!(!Path::new(&name).exists(), "stop removes the socket");

        let other = unique_name();
        std::fs::write(&other, b"not a socket").unwrap();
        assert!(Server::start(&other, ServerConfig::default(), Arc::new(Fake)).is_err());
        assert_eq!(std::fs::read(&other).unwrap(), b"not a socket");
        std::fs::remove_file(&other).unwrap();
    }

    /// Stopping removes only the socket this server bound: one put there since (a later Flare) stays.
    #[test]
    fn stop_never_removes_a_socket_it_did_not_bind() {
        let (server, name) = start(ServerConfig::default());
        std::fs::remove_file(&name).unwrap();
        let later = std::os::unix::net::UnixListener::bind(&name).unwrap();
        server.stop();
        assert!(Path::new(&name).exists());
        drop(later);
        std::fs::remove_file(&name).unwrap();
    }

    /// A listener on `name` with a backlog of one, its queue filled by connects it never accepts: what a
    /// live server that is busy looks like from outside (macOS then refuses a connect, ECONNREFUSED,
    /// exactly as for a dead socket). Returns the listener and the queued clients.
    fn full_backlog(
        name: &str,
    ) -> (
        std::os::unix::net::UnixListener,
        Vec<std::os::unix::net::UnixStream>,
    ) {
        let l = std::os::unix::net::UnixListener::bind(name).unwrap();
        // SAFETY: the listener's own fd; a smaller backlog on a socket that already listens.
        let r = unsafe { libc::listen(std::os::fd::AsRawFd::as_raw_fd(&l), 1) };
        assert_eq!(r, 0);
        let mut queued = Vec::new();
        for _ in 0..16 {
            match std::os::unix::net::UnixStream::connect(name) {
                Ok(s) => queued.push(s),
                Err(e) => {
                    assert_eq!(e.kind(), std::io::ErrorKind::ConnectionRefused, "{e}");
                    return (l, queued);
                }
            }
        }
        panic!("the backlog never filled");
    }

    /// Holds the server lock of `name`, as a running Flare does.
    fn hold_lock(name: &str) -> std::fs::File {
        use std::os::fd::AsRawFd;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path(Path::new(name)))
            .unwrap();
        // SAFETY: a live fd.
        let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(r, 0);
        f
    }

    /// The reviewer's probe: a live server with a full backlog was taken for a dead one, unlinked and
    /// replaced. Now the lock it holds fails the start, and its socket stays where it is.
    #[test]
    fn a_live_server_with_a_full_backlog_is_never_replaced() {
        let name = unique_name();
        let lock = hold_lock(&name);
        let (l, queued) = full_backlog(&name);
        let before = std::fs::symlink_metadata(&name).unwrap().ino();
        match Server::start(&name, ServerConfig::default(), Arc::new(Fake)) {
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse, "{e}"),
            Ok(_) => panic!("started over a live server"),
        }
        assert_eq!(std::fs::symlink_metadata(&name).unwrap().ino(), before);
        // Two real servers on one name: the second fails on the first's lock, not on a connect.
        drop((l, queued, lock));
        std::fs::remove_file(&name).unwrap();
        std::fs::remove_file(lock_path(Path::new(&name))).unwrap();
        let (first, name2) = start(ServerConfig::default());
        match Server::start(&name2, ServerConfig::default(), Arc::new(Fake)) {
            Err(e) => assert!(e.to_string().contains("another Flare"), "{e}"),
            Ok(_) => panic!("a second server on one name"),
        }
        first.stop();
        assert!(
            !lock_path(Path::new(&name2)).exists(),
            "stop removes its lock file"
        );
    }

    /// A busy server (full backlog, lock held) is waited for within the client's timeout, as a busy pipe
    /// is on Windows; once it takes the queued clients the connect goes through.
    #[test]
    fn a_client_of_a_busy_server_waits_and_then_gets_in() {
        let name = unique_name();
        let lock = hold_lock(&name);
        let (l, queued) = full_backlog(&name);
        assert_eq!(server_alive(Path::new(&name)), Some(true));
        // Busy for the whole timeout: a timeout, not an instant refusal.
        let t = std::time::Instant::now();
        match connect(&name, Duration::from_millis(300)) {
            Err(ClientError::Timeout) => {}
            Err(other) => panic!("{other:?}"),
            Ok(_) => panic!("connected to a full backlog"),
        }
        assert!(
            t.elapsed() >= Duration::from_millis(300),
            "{:?}",
            t.elapsed()
        );
        // The server takes its queue a moment later: the waiting client gets in.
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let drain = {
            let done = done.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                drop(queued);
                l.set_nonblocking(true).unwrap();
                let mut taken = Vec::new();
                while !done.load(Ordering::SeqCst) {
                    match l.accept() {
                        Ok((s, _)) => taken.push(s),
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
                (l, taken)
            })
        };
        let r = connect(&name, WAIT);
        done.store(true, Ordering::SeqCst);
        let _ = drain.join();
        assert!(r.is_ok(), "the busy server was waited for: {:?}", r.err());
        drop(lock);
        std::fs::remove_file(&name).unwrap();
        std::fs::remove_file(lock_path(Path::new(&name))).unwrap();
    }

    /// A dead socket (its lock free, or no lock at all) fails a client at once: no waiting on a Flare that
    /// is gone.
    #[test]
    fn a_client_of_a_dead_socket_fails_at_once() {
        let name = unique_name();
        drop(std::os::unix::net::UnixListener::bind(&name).unwrap());
        assert_eq!(server_alive(Path::new(&name)), Some(false), "no lock file");
        let free = hold_lock(&name);
        drop(free);
        assert_eq!(server_alive(Path::new(&name)), Some(false), "a free lock");
        let t = std::time::Instant::now();
        assert!(matches!(connect(&name, WAIT), Err(ClientError::Io(_))));
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        std::fs::remove_file(&name).unwrap();
        let _ = std::fs::remove_file(lock_path(Path::new(&name)));
    }

    #[test]
    fn a_path_too_long_for_a_socket_is_an_error() {
        let name = format!("{}/{}.sock", socket_dir().display(), "x".repeat(120));
        assert!(Server::start(&name, ServerConfig::default(), Arc::new(Fake)).is_err());
    }
}
