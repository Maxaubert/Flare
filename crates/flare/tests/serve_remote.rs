//! Process tests of `flare --serve-remote` (remote sessions PR 2): the built binary run the way ssh runs it, its stdin
//! and stdout piped to a test that plays the viewer. Every run has its own temp profile with no Flare running in it, so
//! nothing here can reach the owner's Flare, and no ssh to any machine is made. Every serve process here has
//! `--no-start`, so none starts a Flare hidden (that is the e2e's `remoteOpen`, on the hidden desktop).
//! Windows-only for now, like `instance.rs` (the macOS run is on the by-hand list of remote sessions PR 6).
#![cfg(windows)]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use flare_ctl::{Client, ClientError, Method, RemoteHelloParams, StdioWire, code};
use serde_json::json;

const WAIT: Duration = Duration::from_secs(20);

fn serve(profile: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_flare"))
        .arg("--serve-remote")
        // Never a hidden start of a real Flare from a unit test (remote sessions PR 5): the relay stays `unavailable`.
        .arg("--no-start")
        .arg(format!("--user-data-dir={}", profile.display()))
        .env_remove("FLARE_CTL_PIPE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the serve process")
}

fn exited_within(child: &mut Child, wait: Duration) -> Option<std::process::ExitStatus> {
    let end = Instant::now() + wait;
    while Instant::now() < end {
        if let Ok(Some(s)) = child.try_wait() {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Rules 277 and 283 as a process: the serve process answers the link's hello on its stdout with protocol lines only,
/// relays nothing while no Flare runs (one `unavailable` line), refuses the pipe's greeting, and ends with exit code 0
/// when the link closes.
#[test]
fn the_serve_process_answers_its_own_stdio_and_ends_with_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = serve(dir.path());
    let wire = StdioWire::from_child(child.stdin.take().unwrap(), child.stdout.take().unwrap())
        .expect("a wire over the child's pipes");
    let mut c = Client::new(wire);
    let info = c
        .remote_hello(
            &RemoteHelloParams {
                machine: "viewer".into(),
                flare_version: "9".into(),
            },
            WAIT,
        )
        .expect("remote.hello answers with no Flare running");
    assert_eq!(info.flare_version, env!("CARGO_PKG_VERSION"));
    assert!(info.capabilities.iter().any(|m| m == "remote.output"));
    assert!(info.capabilities.iter().all(|m| m.starts_with("remote.")));
    match c.call(Method::RemoteTabs, json!({}), WAIT) {
        Err(ClientError::Remote(e)) => {
            assert_eq!(e.code, code::UNAVAILABLE);
            assert!(e.message.contains("not running"), "{}", e.message);
        }
        other => panic!("{other:?}"),
    }
    match c.call(Method::Hello, json!({"client": "x", "version": "1"}), WAIT) {
        Err(ClientError::Remote(e)) => assert_eq!(e.code, code::FORBIDDEN),
        other => panic!("{other:?}"),
    }
    drop(c);
    let status = exited_within(&mut child, Duration::from_secs(10))
        .expect("the serve process ends with its link");
    assert!(status.success(), "{status:?}");
}

/// Rule 283: a serve process holds no single-instance lock: a launch of the same profile while it runs is the first
/// instance (the `--instance-probe` mode says so on its stdout).
#[test]
fn a_serve_process_takes_no_single_instance_lock() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = serve(dir.path());
    // The serve process is up once it answered its hello.
    let wire =
        StdioWire::from_child(server.stdin.take().unwrap(), server.stdout.take().unwrap()).unwrap();
    let mut c = Client::new(wire);
    c.remote_hello(
        &RemoteHelloParams {
            machine: "viewer".into(),
            flare_version: "9".into(),
        },
        WAIT,
    )
    .unwrap();
    let mut probe = Command::new(env!("CARGO_BIN_EXE_flare"))
        .arg("--instance-probe")
        .arg(format!("--user-data-dir={}", dir.path().display()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the probe");
    let out = probe.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let end = Instant::now() + WAIT;
    let mut first = false;
    while let Ok(line) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
        if line == "first" {
            first = true;
            break;
        }
        assert_ne!(line, "second", "the serve process must not hold the lock");
    }
    drop(probe.stdin.take());
    let _ = exited_within(&mut probe, Duration::from_secs(10)).or_else(|| {
        let _ = probe.kill();
        None
    });
    drop(c);
    let _ = exited_within(&mut server, Duration::from_secs(10)).or_else(|| {
        let _ = server.kill();
        None
    });
    assert!(
        first,
        "the probe is the first instance while a serve process runs"
    );
}
