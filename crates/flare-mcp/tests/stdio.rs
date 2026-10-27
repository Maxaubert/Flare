//! The real `flare-mcp.exe` over its stdio, against a real in-process control-channel server.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

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
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 20);
    assert_eq!(replies[2]["result"]["isError"], true);
    let text = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Flare is not running this session"), "{text}");
}

#[test]
fn with_a_tab_but_no_server_it_is_still_not_running() {
    let replies = drive(
        &[
            ("FLARE_TAB_TOKEN", "secret-7"),
            ("FLARE_CTL_PIPE", "\\\\.\\pipe\\flare-mcp-test-nobody"),
        ],
        &[],
        &session(json!({"name":"close_panel"})),
    );
    assert_eq!(replies[2]["result"]["isError"], true);
}

#[test]
fn a_tool_call_reaches_the_server_with_the_tab() {
    let pipe = format!("\\\\.\\pipe\\flare-mcp-test-{}", std::process::id());
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
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"show_image","arguments":{"path":"C:\\pics\\cat.png"}}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"show_file","arguments":{"path":"C:\\pics\\missing.png"}}}),
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
    assert_eq!(calls[0].1["path"], "C:\\pics\\cat.png");
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
    let pipe = format!("\\\\.\\pipe\\flare-mcp-test-badtab-{}", std::process::id());
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
    let pipe = format!("\\\\.\\pipe\\flare-mcp-test-squat-{}", std::process::id());
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
    let pipe = format!(r"\\.\pipe\flare-mcp-test-mods-{}", std::process::id());
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
        21,
        "the built-in tools and one mod tool: {names:?}"
    );
    assert_eq!(names[20], "mod_notes_append_note");
    assert!(!names[..20].contains(&"mod_bad"));

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
