//! The real `flare-mcp` binary over its stdio, against a real in-process control-channel server (the
//! named pipe on Windows, the Unix socket on macOS).

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_ctl::{CallContext, CtlError, Dispatcher, Method, Server, ServerConfig, code};
use serde_json::{Value, json};

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(Method, Value, Option<String>)>>,
}

impl Dispatcher for Recorder {
    fn resolve_token(&self, _: &str) -> Option<flare_ctl::Grant> {
        None
    }
    fn resolve_tab_token(&self, token: &str) -> Option<String> {
        (token == "secret-7").then(|| "tab-7".to_string())
    }
    fn call(&self, ctx: &CallContext, m: Method, p: Value) -> Result<Value, CtlError> {
        self.calls
            .lock()
            .unwrap()
            .push((m, p.clone(), ctx.tab.clone()));
        if p["path"]
            .as_str()
            .is_some_and(|s| s.ends_with("missing.png"))
        {
            return Err(CtlError::new(code::NOT_FOUND, "no such file"));
        }
        Ok(json!({"closed": true}))
    }
}

/// This test's own channel name: the Windows pipe name the tests always used, or on macOS a socket in
/// this user's socket folder (`flare_ctl::pipe_name`, the server makes the folder).
fn test_pipe(tag: &str) -> String {
    let id = format!("{tag}-{}", std::process::id());
    #[cfg(windows)]
    {
        format!("\\\\.\\pipe\\{id}")
    }
    #[cfg(not(windows))]
    {
        flare_ctl::pipe_name(&id, None)
    }
}

/// An absolute picture path in this system's grammar, and a missing one beside it.
#[cfg(windows)]
const CAT: (&str, &str) = ("C:\\pics\\cat.png", "C:\\pics\\missing.png");
#[cfg(not(windows))]
const CAT: (&str, &str) = ("/pics/cat.png", "/pics/missing.png");

/// A channel name no server listens on.
fn nobody_pipe(tag: &str) -> String {
    #[cfg(windows)]
    {
        format!("\\\\.\\pipe\\{tag}")
    }
    #[cfg(not(windows))]
    {
        flare_ctl::pipe_name(tag, None)
    }
}

/// Starts the binary with the given extra env, sends `lines`, closes stdin, returns its stdout lines.
fn drive(env: &[(&str, &str)], remove: &[&str], lines: &[Value]) -> Vec<Value> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flare-mcp"));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for r in remove {
        cmd.env_remove(r);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for l in lines {
        writeln!(stdin, "{l}").unwrap();
    }
    drop(stdin);
    let out = BufReader::new(child.stdout.take().unwrap());
    let replies: Vec<Value> = out
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).expect("stdout is JSON lines only"))
        .collect();
    assert!(child.wait().unwrap().success());
    replies
}

fn session(call: Value) -> Vec<Value> {
    vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":call}),
    ]
}

#[test]
fn outside_flare_every_tool_says_so_and_the_handshake_still_works() {
    let replies = drive(
        &[],
        &["FLARE_TAB_TOKEN", "FLARE_CTL_PIPE", "FLARE_CTL_PID"],
        &session(json!({"name":"show_image","arguments":{"path":"C:\\a.png"}})),
    );
    assert_eq!(replies.len(), 3, "the notification gets no reply");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 24);
    assert_eq!(replies[2]["result"]["isError"], true);
    let text = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Flare is not running this session"), "{text}");
}

#[test]
fn with_a_tab_but_no_server_it_is_still_not_running() {
    let replies = drive(
        &[
            ("FLARE_TAB_TOKEN", "secret-7"),
            ("FLARE_CTL_PIPE", &nobody_pipe("flare-mcp-test-nobody")),
        ],
        &[],
        &session(json!({"name":"close_panel"})),
    );
    assert_eq!(replies[2]["result"]["isError"], true);
}

#[test]
fn a_tool_call_reaches_the_server_with_the_tab() {
    let pipe = test_pipe("flare-mcp-test");
    let rec = Arc::new(Recorder::default());
    let server = Server::start(&pipe, ServerConfig::default(), rec.clone()).unwrap();
    let pid = std::process::id().to_string();
    let replies = drive(
        &[
            ("FLARE_TAB_TOKEN", "secret-7"),
            ("FLARE_CTL_PIPE", &pipe),
            ("FLARE_CTL_PID", &pid),
        ],
        &[],
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"show_image","arguments":{"path":CAT.0}}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"show_file","arguments":{"path":CAT.1}}}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"close_panel"}}),
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"show_text","arguments":{"text":"hello","title":"T"}}}),
        ],
    );
    assert_eq!(replies[1]["result"]["isError"], false);
    assert_eq!(replies[2]["result"]["isError"], true);
    let refused = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        refused.contains("no such file") && refused.contains("not_found"),
        "{refused}"
    );
    assert_eq!(
        replies[3]["result"]["content"][0]["text"],
        "Closed the panel."
    );
    let calls = rec.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0].0, Method::ViewImage);
    assert_eq!(calls[0].1["path"], CAT.0);
    assert_eq!(calls[0].2.as_deref(), Some("tab-7"));
    assert_eq!(calls[1].0, Method::ViewFile);
    assert_eq!(calls[2].0, Method::PanelClose);
    assert_eq!(calls[3].0, Method::ViewText);
    assert_eq!(calls[3].1["text"], "hello");
    assert_eq!(calls[3].1["title"], "T");
    drop(calls);
    server.stop();
}

#[test]
fn an_unknown_tab_token_is_reported_as_flare_refusing() {
    let pipe = test_pipe("flare-mcp-test-badtab");
    let server = Server::start(
        &pipe,
        ServerConfig::default(),
        Arc::new(Recorder::default()),
    )
    .unwrap();
    let pid = std::process::id().to_string();
    let replies = drive(
        &[
            ("FLARE_TAB_TOKEN", "guess"),
            ("FLARE_CTL_PIPE", &pipe),
            ("FLARE_CTL_PID", &pid),
        ],
        &[],
        &session(json!({"name":"close_panel"})),
    );
    assert_eq!(replies[2]["result"]["isError"], true);
    let t = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(t.contains("unauthorized"), "{t}");
    server.stop();
}

#[test]
fn a_pipe_served_by_another_process_is_never_given_the_secret() {
    // The server here is this process; the shell was told another process serves the pipe. The binary
    // leaves before it says hello, so the recorder never sees a token, and the model is told Flare is
    // not running.
    let pipe = test_pipe("flare-mcp-test-squat");
    let rec = Arc::new(Recorder::default());
    let server = Server::start(&pipe, ServerConfig::default(), rec.clone()).unwrap();
    let other = std::process::id().wrapping_add(4).to_string();
    let replies = drive(
        &[
            ("FLARE_TAB_TOKEN", "secret-7"),
            ("FLARE_CTL_PIPE", &pipe),
            ("FLARE_CTL_PID", &other),
        ],
        &[],
        &session(json!({"name":"close_panel"})),
    );
    server.stop();
    assert_eq!(replies[2]["result"]["isError"], true);
    let text = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Flare is not running this session"), "{text}");
    assert!(
        rec.calls.lock().unwrap().is_empty(),
        "the server saw no call"
    );
}

/// A server that offers one mod tool and answers its calls.
struct ModServer {
    calls: Mutex<Vec<(Method, Value)>>,
}

impl Dispatcher for ModServer {
    fn resolve_token(&self, _: &str) -> Option<flare_ctl::Grant> {
        None
    }
    fn resolve_tab_token(&self, token: &str) -> Option<String> {
        (token == "secret-7").then(|| "tab-7".to_string())
    }
    fn call(&self, _ctx: &CallContext, m: Method, p: Value) -> Result<Value, CtlError> {
        self.calls.lock().unwrap().push((m, p.clone()));
        match m {
            Method::ModsTools => Ok(json!({"tools": [
                {"name": "mod_notes_append_note", "description": "Adds one line.",
                 "input_schema": {"type": "object", "properties": {"text": {"type": "string"}}}},
                // These do not fit the agent's tool space and are left out by the server.
                {"name": "show_image", "description": "x", "input_schema": {"type": "object"}},
                {"name": "mod_bad", "description": "x", "input_schema": {"type": "string"}}
            ]})),
            Method::ModsCall => {
                if p["input"]["text"] == "boom" {
                    Err(CtlError::new("tool_error", "text is empty"))
                } else {
                    Ok(json!({"result": {"count": 1}}))
                }
            }
            _ => Ok(json!({})),
        }
    }
}

#[test]
fn a_mods_tools_are_listed_called_and_announced_when_they_change() {
    let pipe = test_pipe("flare-mcp-test-mods");
    let srv = Arc::new(ModServer {
        calls: Mutex::new(Vec::new()),
    });
    let server = Server::start(&pipe, ServerConfig::default(), srv.clone()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_flare-mcp"))
        .env("FLARE_TAB_TOKEN", "secret-7")
        .env("FLARE_CTL_PIPE", &pipe)
        .env("FLARE_CTL_PID", std::process::id().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut next = move || -> Value {
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).expect("stdout is JSON lines only")
    };
    let mut send = |v: Value| writeln!(stdin, "{v}").unwrap();

    send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    assert_eq!(
        next()["result"]["capabilities"]["tools"]["listChanged"],
        true
    );
    send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let list = next();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names.len(),
        25,
        "the built-in tools and one mod tool: {names:?}"
    );
    assert_eq!(names[24], "mod_notes_append_note");
    assert!(!names[..24].contains(&"mod_bad"));

    // A call goes to the mod through mods.call, and the mod's words come back.
    send(
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"mod_notes_append_note","arguments":{"text":"hi"}}}),
    );
    let r = next();
    assert_eq!(r["result"]["isError"], false);
    assert_eq!(r["result"]["content"][0]["text"], r#"{"count":1}"#);
    send(
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"mod_notes_append_note","arguments":{"text":"boom"}}}),
    );
    let r = next();
    assert_eq!(r["result"]["isError"], true);
    assert_eq!(r["result"]["content"][0]["text"], "text is empty");

    // The mods changed: the server tells the client to list again (the watcher has its own connection, which
    // takes a moment to say hello, so the publish repeats until the notification is heard).
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let v = next();
        let _ = tx.send(v);
    });
    let mut heard = None;
    for _ in 0..100 {
        server.publish("mods.changed", json!({}), None);
        if let Ok(v) = rx.recv_timeout(std::time::Duration::from_millis(100)) {
            heard = Some(v);
            break;
        }
    }
    let heard = heard.expect("the notification was sent");
    assert_eq!(heard["method"], "notifications/tools/list_changed");
    assert!(heard.get("id").is_none());
    drop(send);
    drop(stdin);
    reader.join().unwrap();
    assert!(child.wait().unwrap().success());
    let calls = srv.calls.lock().unwrap();
    assert!(calls.iter().any(|(m, p)| *m == Method::ModsCall
        && p["name"] == "mod_notes_append_note"
        && p["input"]["text"] == "hi"));
    drop(calls);
    server.stop();
}

/// A server whose `panel.close` waits until its caller goes away, as a click or a wait would.
#[derive(Default)]
struct WaitsForCaller {
    started: AtomicU32,
    ended_by_cancel: AtomicU32,
}

impl Dispatcher for WaitsForCaller {
    fn resolve_token(&self, _: &str) -> Option<flare_ctl::Grant> {
        None
    }
    fn resolve_tab_token(&self, token: &str) -> Option<String> {
        (token == "secret-7").then(|| "tab-7".to_string())
    }
    fn call(&self, ctx: &CallContext, m: Method, _p: Value) -> Result<Value, CtlError> {
        if m != Method::PanelClose {
            return Ok(json!({"open": false}));
        }
        self.started.fetch_add(1, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(15);
        while !ctx.cancel.is_set() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        if ctx.cancel.is_set() {
            self.ended_by_cancel.fetch_add(1, Ordering::SeqCst);
        }
        Ok(json!({"closed": true}))
    }
}

fn wait_until(what: &str, ok: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < until, "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_cancel_during_a_call_ends_the_job_on_flares_side_and_the_next_call_still_works() {
    let pipe = test_pipe("flare-mcp-test-cancel");
    let srv = Arc::new(WaitsForCaller::default());
    let server = Server::start(&pipe, ServerConfig::default(), srv.clone()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_flare-mcp"))
        .env("FLARE_TAB_TOKEN", "secret-7")
        .env("FLARE_CTL_PIPE", &pipe)
        .env("FLARE_CTL_PID", std::process::id().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut next = move || -> Value {
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).expect("stdout is JSON lines only")
    };
    let mut send = |v: Value| writeln!(stdin, "{v}").unwrap();

    send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    assert_eq!(next()["id"], 1);
    send(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"close_panel"}}));
    wait_until("the call to reach Flare", || {
        srv.started.load(Ordering::SeqCst) == 1
    });
    send(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"user"}}),
    );
    wait_until("Flare to end the job", || {
        srv.ended_by_cancel.load(Ordering::SeqCst) == 1
    });
    // The server goes on, on a new connection, and request 2 never got a response.
    send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"browser_state"}}));
    let r = next();
    assert_eq!(r["id"], 3, "request 2 got no response: {r}");
    drop(send);
    drop(stdin);
    assert!(child.wait().unwrap().success());
    server.stop();
}

/// Answers `panel.state` from a script that the test changes between prompts.
struct PanelServer {
    state: Mutex<Value>,
    calls: Mutex<Vec<(Method, Option<String>)>>,
}

impl Dispatcher for PanelServer {
    fn resolve_token(&self, _: &str) -> Option<flare_ctl::Grant> {
        None
    }
    fn resolve_tab_token(&self, token: &str) -> Option<String> {
        (token == "secret-9").then(|| "tab-9".to_string())
    }
    fn call(&self, ctx: &CallContext, m: Method, _p: Value) -> Result<Value, CtlError> {
        self.calls.lock().unwrap().push((m, ctx.tab.clone()));
        Ok(self.state.lock().unwrap().clone())
    }
}

/// Runs the binary as Claude Code runs the prompt hook: `--hook-context`, the hook's JSON on stdin. Returns
/// its stdout and how long it took.
fn prompt_hook(
    env: &[(&str, &str)],
    remove: &[&str],
    input: &str,
) -> (String, std::time::Duration) {
    let started = std::time::Instant::now();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flare-mcp"));
    cmd.arg("--hook-context")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for r in remove {
        cmd.env_remove(r);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.as_bytes()).unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "the hook always exits 0");
    (String::from_utf8(out.stdout).unwrap(), started.elapsed())
}

#[test]
fn the_prompt_hook_tells_the_agent_of_a_page_once_and_says_nothing_outside_flare() {
    // Its own temp folder, so the state the hook keeps is this test's alone.
    let temp = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("hook-context-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let temp_text = temp.to_string_lossy().to_string();
    let input = r#"{"session_id":"s-hook","hook_event_name":"UserPromptSubmit","prompt":"hi"}"#;

    // Outside Flare: nothing, at once.
    let (out, took) = prompt_hook(
        &[
            ("TEMP", &temp_text),
            ("TMP", &temp_text),
            ("TMPDIR", &temp_text),
        ],
        &["FLARE_TAB_TOKEN", "FLARE_CTL_PIPE", "FLARE_CTL_PID"],
        input,
    );
    assert_eq!(out, "");
    assert!(took < std::time::Duration::from_secs(3), "{took:?}");
    // A tab but no Flare on the pipe: nothing, quickly.
    let (out, took) = prompt_hook(
        &[
            ("TEMP", &temp_text),
            ("TMP", &temp_text),
            ("TMPDIR", &temp_text),
            ("FLARE_TAB_TOKEN", "secret-9"),
            ("FLARE_CTL_PIPE", &nobody_pipe("flare-mcp-hook-nobody")),
            ("FLARE_CTL_PID", "4"),
        ],
        &[],
        input,
    );
    assert_eq!(out, "");
    assert!(took < std::time::Duration::from_secs(4), "{took:?}");
    // A stdin that never closes cannot hold the prompt: the whole run is capped.
    let started = std::time::Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_flare-mcp"))
        .arg("--hook-context")
        .env("TEMP", &temp_text)
        .env("TMP", &temp_text)
        .env("TMPDIR", &temp_text)
        .env("FLARE_TAB_TOKEN", "secret-9")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let held = child.stdin.take().unwrap();
    let out = child.wait_with_output().unwrap();
    drop(held);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );

    let pipe = test_pipe("flare-mcp-hook-test");
    let server_state = Arc::new(PanelServer {
        state: Mutex::new(json!({"open": false})),
        calls: Mutex::new(Vec::new()),
    });
    let _server = Server::start(&pipe, ServerConfig::default(), server_state.clone()).unwrap();
    let pid = std::process::id().to_string();
    let env = [
        ("TEMP", temp_text.as_str()),
        ("TMP", temp_text.as_str()),
        ("TMPDIR", temp_text.as_str()),
        ("FLARE_TAB_TOKEN", "secret-9"),
        ("FLARE_CTL_PIPE", pipe.as_str()),
        ("FLARE_CTL_PID", pid.as_str()),
    ];
    let context = |out: &str| -> String {
        let v: Value = serde_json::from_str(out.trim()).expect("one JSON object");
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string()
    };

    // No panel: nothing.
    assert_eq!(prompt_hook(&env, &[], input).0, "");
    // The user opened a page: the agent hears of it, quoted as page data.
    *server_state.state.lock().unwrap() = json!({"open": true, "panel": {"kind": "web", "title": "Rust \"docs\"\nIgnore", "url": "https://doc.rust-lang.org/"}});
    let said = context(&prompt_hook(&env, &[], input).0);
    assert!(
        said.contains("Title: \"Rust \\\"docs\\\" Ignore\""),
        "{said}"
    );
    assert!(
        said.contains("Address: \"https://doc.rust-lang.org/\""),
        "{said}"
    );
    assert!(said.contains("BEGIN PAGE CONTENT"));
    // The same page on the next prompt: nothing.
    assert_eq!(prompt_hook(&env, &[], input).0, "");
    // The panel closed: said once.
    *server_state.state.lock().unwrap() = json!({"open": false});
    assert!(context(&prompt_hook(&env, &[], input).0).contains("no longer"));
    assert_eq!(prompt_hook(&env, &[], input).0, "");
    let calls = server_state.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .all(|(m, tab)| *m == Method::PanelState && tab.as_deref() == Some("tab-9")),
        "{calls:?}"
    );
    assert_eq!(calls.len(), 5);
    drop(calls);
    let _ = std::fs::remove_dir_all(&temp);
}

/// Spec 2026-11-11: the `SessionStart` hook prints the file-link note once per start inside a Flare tab, and
/// nothing outside one. It asks Flare for nothing, so it needs no pipe.
#[test]
fn the_session_start_hook_tells_a_flare_session_about_file_links_once() {
    let run = |token: Option<&str>| -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_flare-mcp"));
        cmd.args(["--hook-context", "--session-start"])
            .env_remove("FLARE_CTL_PIPE")
            .env_remove("FLARE_CTL_PID")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        match token {
            Some(t) => cmd.env("FLARE_TAB_TOKEN", t),
            None => cmd.env_remove("FLARE_TAB_TOKEN"),
        };
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "the hook always exits 0");
        String::from_utf8(out.stdout).unwrap()
    };
    assert_eq!(run(None), "", "outside Flare: nothing");
    assert_eq!(run(Some("")), "");
    let out = run(Some("secret-9"));
    assert_eq!(out.lines().count(), 1, "{out}");
    let v: Value = serde_json::from_str(out.trim()).expect("one JSON object");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert_eq!(
        v["hookSpecificOutput"]["additionalContext"].as_str(),
        Some(flare_mcp::FILE_LINK_NOTE)
    );
}
