//! The stand-ins a scenario needs (M10 Task 15): hook lines, FakeClaude, the Sonara stand-in,
//! floods, small programs and the fixture cache.
//!
//! Windows-only: the e2e harness drives the Windows app (job objects, the hidden desktop, named pipes).
//! The Mac E2E is the port plan's "Ongoing" row.
#![cfg(windows)]

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use flare_agent::question_widget::ShownQuestion;
use flare_agent::resume::{claude_sessions, is_interactive_head};
use flare_agent::signal::{agent_hook_events, parse_agent_signal, HookState};
use flare_agent::sonara::{post_question_shown, read_runtime, Runtime};
use flare_e2e::standins::agent_cmd::{FakeClaude, Step};
use flare_e2e::standins::flood::{flood_file, long_output_cmd};
use flare_e2e::standins::hooklines::hook_line;
use flare_e2e::standins::progs;
use flare_e2e::standins::sonara::SonaraStandIn;
use flare_e2e::standins::FixtureCache;
use flare_e2e::world::World;
use serde_json::json;
use sha2::{Digest, Sha256};

/// The OSC 777 frame around a hook payload, as a shell writes it.
fn frame_data(line: &str) -> &str {
    line.strip_prefix("\x1b]777;")
        .and_then(|s| s.strip_suffix('\x07'))
        .unwrap_or_else(|| panic!("not an OSC 777 frame: {line:?}"))
}

#[test]
fn hook_line_parses_with_the_real_agent_parser() {
    let rows = agent_hook_events();
    assert_eq!(rows.len(), 25);
    for row in rows {
        // `+sid`: the turn's own hooks also name the Claude session (N25).
        let sid = "f1f2f3f4-a5a6-4b70-8c9d-0e1f2a3b4c5d";
        let (state, payload) = match row.args.split_once(' ') {
            Some(("failed", kind)) => ("failed", json!({ "kind": kind })),
            Some((state, "+sid")) => (state, json!({ "session": sid })),
            _ => (row.args.as_str(), json!({})),
        };
        let line = hook_line(state, &payload);
        let sig = parse_agent_signal(frame_data(&line))
            .unwrap_or_else(|| panic!("the real parser refuses {line:?}"));
        assert_eq!(sig.state.as_str(), state, "{line:?}");
        assert_eq!(
            sig.kind.as_deref(),
            payload.get("kind").and_then(|k| k.as_str()),
            "{line:?}"
        );
    }
    let question = parse_agent_signal(frame_data(&hook_line("question", &json!({}))));
    assert_eq!(question.map(|s| s.state), Some(HookState::Question));
}

#[test]
fn hook_line_is_the_plugin_frame_byte_for_byte() {
    assert_eq!(
        hook_line("failed", &json!({ "kind": "rate_limit" })),
        "\x1b]777;flare-agent;state=failed;kind=rate_limit\x07"
    );
    assert_eq!(
        hook_line("done", &json!({})),
        "\x1b]777;flare-agent;state=done\x07"
    );
}

#[test]
fn fake_claude_writes_interactive_transcripts_the_resume_reader_accepts() {
    let world = World::new();
    let mut fake = FakeClaude::new(&world);
    let cwd = r"C:\Users\me\repo";
    let mine_id = "5a1d0c2e-1111-4222-8333-94445555a666";
    let mine = fake.transcript(cwd, mine_id, "cli");
    fake.transcript(cwd, "9b2e1d3f-2222-4333-8444-95556666b777", "sdk-cli");

    let head = std::fs::read_to_string(&mine).expect("the transcript was written");
    assert!(is_interactive_head(&head));
    let home = fake.home();
    assert_eq!(claude_sessions(cwd, &home), vec![mine_id.to_string()]);

    fake.script(&[
        Step::Title("claude".into()),
        Step::Sleep(300),
        Step::Title("\u{2733} Claude Code".into()),
        Step::Print("FAKE CLAUDE READY".into()),
        Step::Hook {
            state: "working".into(),
            payload: json!({}),
        },
    ]);
    assert!(fake.dir.join("bin").join("claude.cmd").is_file());
    let env = fake.env();
    let user = env
        .iter()
        .find(|(k, _)| k == "USERPROFILE")
        .expect("USERPROFILE set");
    assert_eq!(user.1, home.display().to_string());
    let path = env.iter().find(|(k, _)| k == "PATH").expect("PATH set");
    assert!(path
        .1
        .starts_with(&fake.dir.join("bin").display().to_string()));

    let dir = fake.dir.clone();
    drop(fake);
    assert!(!dir.exists(), "FakeClaude leaves no folder behind");
}

/// Sends one raw POST with `bearer` and returns the HTTP status code.
fn raw_post(port: u16, bearer: &str, body: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let request = format!(
        "POST /v1/question_shown HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {bearer}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("read");
    let status_line = reply.lines().next().unwrap_or_default();
    status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status in {reply:?}"))
}

#[test]
fn sonara_stand_in_refuses_a_bad_bearer_and_records_a_good_post() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("sonara-home");
    let sonara = SonaraStandIn::start(&home, "e2e-token").expect("start");
    assert_eq!(
        read_runtime(&home),
        Some(Runtime {
            http_port: sonara.port,
            token: "e2e-token".into(),
            pid: Some(std::process::id()),
        }),
        "the real reader takes the stand-in's runtime.json"
    );

    assert_eq!(
        raw_post(sonara.port, "wrong", r#"{"host_tab":"t0","text":"x"}"#),
        401
    );

    let env = |k: &str| (k == "SONARA_HOME").then(|| home.display().to_string());
    let shown = ShownQuestion::Text("Pick a colour?".into());
    assert!(post_question_shown(
        "tab-1",
        &shown,
        &env,
        Duration::from_secs(5)
    ));

    let posts = sonara.posts();
    assert_eq!(posts.len(), 2, "every POST is recorded");
    assert!(!posts[0].auth_ok);
    assert!(posts[1].auth_ok);
    assert_eq!(posts[1].method, "POST");
    assert_eq!(posts[1].path, "/v1/question_shown");
    assert_eq!(posts[1].tab_id.as_deref(), Some("tab-1"));
    assert_eq!(posts[1].body["text"], "Pick a colour?");
}

#[test]
fn flood_file_is_exactly_n_mb_and_ends_with_the_marker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = flood_file(dir.path(), 1, "FLOOD-END");
    let bytes = std::fs::read(&path).expect("read");
    assert_eq!(bytes.len(), 1024 * 1024);
    let text = String::from_utf8(bytes.clone()).expect("ascii");
    assert!(
        text.ends_with("FLOOD-END\n"),
        "{:?}",
        &text[text.len() - 20..]
    );
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    assert!(lines[0].starts_with("LINE 000000001 "));
    for line in &lines[..lines.len() - 1] {
        assert_eq!(
            line.len(),
            100,
            "every line but the last is 100 bytes: {line:?}"
        );
    }
    let again = flood_file(dir.path(), 1, "FLOOD-END");
    assert_eq!(std::fs::read(&again).expect("read"), bytes, "deterministic");
    assert_eq!(long_output_cmd(3), "for /L %i in (1,1,3) do @echo line %i");
}

/// Serves one HTTP response with `body` on 127.0.0.1 and returns its URL.
fn serve_once(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!(
        "http://{}/fixture.bin",
        listener.local_addr().expect("addr")
    );
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.shutdown(Shutdown::Both);
        }
    });
    url
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn the_cache_refuses_a_wrong_hash() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cache = FixtureCache {
        dir: tmp.path().join(".e2e-cache"),
    };
    std::fs::create_dir_all(&cache.dir).expect("dir");
    let good = sha256_hex(b"hello");

    // A cached file with the right hash is used without any download.
    std::fs::write(cache.dir.join("hello.bin"), b"hello").expect("write");
    let hit = cache
        .fetch("hello.bin", "http://127.0.0.1:9/never", &good)
        .expect("cache hit");
    assert_eq!(std::fs::read(hit).expect("read"), b"hello");

    // A download whose bytes do not match the pinned hash is refused and removed.
    let url = serve_once("bye");
    let err = cache
        .fetch("bad.bin", &url, &good)
        .expect_err("a wrong hash is refused");
    assert!(err.contains("SHA-256 mismatch"), "{err}");
    assert!(
        !cache.dir.join("bad.bin").exists(),
        "the refused file is removed"
    );

    // A cached file with a wrong hash is never trusted.
    std::fs::write(cache.dir.join("stale.bin"), b"tampered").expect("write");
    assert!(cache
        .fetch("stale.bin", "http://127.0.0.1:9/never", &good)
        .is_err());
    assert!(!cache.dir.join("stale.bin").exists());
}

/// Runs a `.cmd` once under `cmd /c` with no stdin and returns its stdout.
fn run_prog(cmd: &Path) -> Vec<u8> {
    let out = Command::new("cmd")
        .args(["/d", "/c"])
        .arg(cmd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .expect("cmd runs");
    assert!(out.status.success(), "{} failed", cmd.display());
    out.stdout
}

#[test]
fn progs_print_what_they_are_asked_to() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    let bell = progs::bell(dir, 3);
    let out = run_prog(&bell);
    assert_eq!(out.iter().filter(|&&b| b == 7).count(), 3, "three bells");

    let osc52 = progs::osc52_writer(dir, "aGk=");
    let out = run_prog(&osc52);
    assert!(
        out.windows(b"\x1b]52;c;aGk=\x07".len())
            .any(|w| w == b"\x1b]52;c;aGk=\x07"),
        "OSC 52 carries the base64 text"
    );

    let link = progs::osc8(dir, "https://example.com/x", "link");
    let out = run_prog(&link);
    let want = b"\x1b]8;;https://example.com/x\x07link\x1b]8;;\x07";
    assert!(
        out.windows(want.len()).any(|w| w == want),
        "OSC 8 wraps the label"
    );

    // These block on the console or on replies, so they are checked by what they would write.
    // The functions return the `.cmd`; its body is the `.ps1` beside it.
    let body = |cmd: std::path::PathBuf| {
        std::fs::read_to_string(cmd.with_extension("ps1")).expect("the .ps1 body")
    };
    let alt = body(progs::alt_screen(dir));
    assert!(alt.contains("?1049h") && alt.contains("?1049l"));
    let mouse = body(progs::mouse_reporter(dir));
    assert!(mouse.contains("?1000h") && mouse.contains("?1006h"));
    let da1 = body(progs::da1_dump(dir));
    assert!(da1.contains("[c"));
    let color = body(progs::color_query(dir));
    assert!(color.contains("]10;?") && color.contains("]11;?"));
}
