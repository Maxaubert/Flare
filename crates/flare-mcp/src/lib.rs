//! `flare-mcp`: a stdio Model Context Protocol server that gives an agent (Claude Code, any MCP client)
//! tools to show things in Flare and, later, to drive its browser panel. It speaks JSON-RPC 2.0 on
//! stdin/stdout (one message per line, nothing else on stdout) and the Flare control channel
//! (`flare-ctl`) on the other side. Hand-rolled on `serde_json`: the surface is four methods.
//!
//! The server answers `initialize`, `tools/list` and `ping` with or without Flare. Only a tool call
//! needs the channel, and without Flare (no pipe, no `FLARE_TAB_ID`) it answers `isError` text that
//! says so, so the agent can carry on without the tool.

use std::io::{BufRead, Write};
use std::time::Duration;

use flare_ctl::{ClientError, Method};
use serde_json::{Map, Value, json};

/// Protocol versions this server can speak, newest first. The client's is echoed when listed.
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
pub const SERVER_NAME: &str = "flare-mcp";

/// How long a tool call waits for Flare (a connect, then the answer).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// JSON-RPC error codes.
mod rpc {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
}

/// One piece of a tool result.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    /// Base64 data and its MIME type. Used by `browser_look` (stage 3); the type is here so the
    /// result shape does not change when it lands.
    Image {
        data: String,
        mime_type: String,
    },
}

impl Content {
    fn to_json(&self) -> Value {
        match self {
            Content::Text(t) => json!({"type": "text", "text": t}),
            Content::Image { data, mime_type } => {
                json!({"type": "image", "data": data, "mimeType": mime_type})
            }
        }
    }
}

/// What a tool call returns.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub content: Vec<Content>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(t: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(t.into())],
            is_error: false,
        }
    }

    pub fn error(t: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(t.into())],
            is_error: true,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "content": self.content.iter().map(Content::to_json).collect::<Vec<_>>(),
            "isError": self.is_error,
        })
    }
}

/// Why a call to Flare did not produce a result.
#[derive(Debug)]
pub enum ChannelError {
    /// Flare is not there: no pipe, no tab, not running inside Flare.
    NotRunning(String),
    /// Flare answered with an error.
    Remote { code: String, message: String },
    /// The connection broke or timed out.
    Failed(String),
}

/// The way tools reach Flare. The real one is [`FlareChannel`]; tests use a fake.
pub trait Channel {
    fn call(&mut self, method: Method, params: Value) -> Result<Value, ChannelError>;
}

/// A tool as `tools/list` shows it.
#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

fn path_schema(what: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": what}
        },
        "required": ["path"],
        "additionalProperties": false
    })
}

/// The tools of v1.
pub fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "show_image",
            description: "Show an image file in a panel beside this terminal tab in Flare, so the user can look at it without leaving the terminal. Takes an absolute path, or one relative to the current folder.",
            schema: path_schema("Path of the image file (png, jpg, gif, webp, bmp, svg)."),
        },
        ToolDef {
            name: "show_file",
            description: "Show a text or code file in a panel beside this terminal tab in Flare. The file is only displayed, never run or changed.",
            schema: path_schema("Path of the file to show."),
        },
        ToolDef {
            name: "close_panel",
            description: "Close the panel Flare shows beside this terminal tab, if one is open.",
            schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
    ]
}

/// An absolute form of `path` (relative paths resolve against this process's folder, which is the
/// agent's). Flare resolves it again on its side (rule S2); this only spares it guessing a cwd.
fn absolute(path: &str) -> String {
    match std::path::absolute(path) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(_) => path.to_string(),
    }
}

fn map_channel_error(e: ChannelError) -> ToolResult {
    match e {
        ChannelError::NotRunning(why) => ToolResult::error(format!(
            "Flare is not running this session ({why}). This tool only works inside a Flare terminal tab; carry on without it."
        )),
        ChannelError::Remote { code, message } => {
            ToolResult::error(format!("Flare refused: {message} ({code})."))
        }
        ChannelError::Failed(why) => ToolResult::error(format!("Could not reach Flare: {why}.")),
    }
}

/// Runs one tool. Argument mistakes are tool errors (`isError`), not protocol errors, so the model
/// sees them and can fix its call.
pub fn call_tool(
    name: &str,
    args: &Map<String, Value>,
    channel: &mut dyn Channel,
) -> Option<ToolResult> {
    let path_arg = || -> Result<String, ToolResult> {
        match args.get("path") {
            Some(Value::String(p)) if !p.trim().is_empty() => Ok(absolute(p)),
            _ => Err(ToolResult::error("path is required: a non-empty string.")),
        }
    };
    let result = match name {
        "show_image" | "show_file" => {
            let path = match path_arg() {
                Ok(p) => p,
                Err(e) => return Some(e),
            };
            let (method, what) = if name == "show_image" {
                (Method::ViewImage, "image")
            } else {
                (Method::ViewFile, "file")
            };
            channel
                .call(method, json!({"path": path}))
                .map(|_| ToolResult::text(format!("Showing the {what} {path} in Flare.")))
        }
        "close_panel" => channel.call(Method::PanelClose, json!({})).map(|v| {
            match v.get("closed").and_then(Value::as_bool) {
                Some(false) => ToolResult::text("No panel was open."),
                _ => ToolResult::text("Closed the panel."),
            }
        }),
        _ => return None,
    };
    Some(result.unwrap_or_else(map_channel_error))
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

/// Handles one JSON-RPC message. `None` means no reply (a notification, or a response from the
/// client that we never asked for).
pub fn handle_message(msg: &Value, channel: &mut dyn Channel) -> Option<Value> {
    let Some(obj) = msg.as_object() else {
        // Batches were removed from MCP in 2025-06-18; anything but an object is not a message.
        return Some(rpc_error(
            &Value::Null,
            rpc::INVALID_REQUEST,
            "expected a JSON-RPC object",
        ));
    };
    let id = obj.get("id");
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // A response to something we never sent: ignore it. With an id and no method it is
        // otherwise malformed.
        return match (id, obj.contains_key("result") || obj.contains_key("error")) {
            (Some(_), true) | (None, _) => None,
            (Some(id), false) => Some(rpc_error(id, rpc::INVALID_REQUEST, "missing method")),
        };
    };
    let Some(id) = id.filter(|i| i.is_string() || i.is_number()) else {
        return None; // a notification (initialized, cancelled, ...): never answered
    };
    let params = obj.get("params");
    Some(match method {
        "initialize" => {
            let asked = params
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str);
            let version = asked
                .filter(|v| SUPPORTED_VERSIONS.contains(v))
                .unwrap_or(SUPPORTED_VERSIONS[0]);
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "Tools to show files and images in the Flare terminal tab this agent runs in."
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => {
            let list: Vec<Value> = tools()
                .into_iter()
                .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": t.schema}))
                .collect();
            rpc_result(id, json!({"tools": list}))
        }
        "tools/call" => {
            let Some(name) = params.and_then(|p| p.get("name")).and_then(Value::as_str) else {
                return Some(rpc_error(
                    id,
                    rpc::INVALID_PARAMS,
                    "tools/call needs a tool name",
                ));
            };
            let args = match params.and_then(|p| p.get("arguments")) {
                None | Some(Value::Null) => Map::new(),
                Some(Value::Object(m)) => m.clone(),
                Some(_) => {
                    return Some(rpc_error(
                        id,
                        rpc::INVALID_PARAMS,
                        "arguments must be an object",
                    ));
                }
            };
            match call_tool(name, &args, channel) {
                Some(r) => rpc_result(id, r.to_json()),
                None => rpc_error(id, rpc::INVALID_PARAMS, format!("unknown tool: {name}")),
            }
        }
        other => rpc_error(
            id,
            rpc::METHOD_NOT_FOUND,
            format!("method not found: {other}"),
        ),
    })
}

/// Reads JSON-RPC lines from `input` until it ends, answering on `output`. Blank lines are skipped;
/// a line that is not JSON gets a parse error with a null id.
pub fn run(
    input: impl BufRead,
    mut output: impl Write,
    channel: &mut dyn Channel,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle_message(&msg, channel),
            Err(e) => Some(rpc_error(
                &Value::Null,
                rpc::PARSE_ERROR,
                format!("parse error: {e}"),
            )),
        };
        if let Some(r) = reply {
            // One line, no embedded newlines (serde_json escapes them).
            writeln!(output, "{r}")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// The real channel: connects to Flare's pipe on the first call, says hello with the tab from
/// `FLARE_TAB_ID`, and reconnects once if the connection has gone stale.
pub struct FlareChannel {
    tab: Option<String>,
    pipe: Option<String>,
    client: Option<flare_ctl::Client<flare_ctl::PipeEnd>>,
}

impl FlareChannel {
    /// Reads `FLARE_TAB_ID` and `FLARE_CTL_PIPE` (or the user's default pipe).
    pub fn from_env() -> Self {
        let tab = std::env::var("FLARE_TAB_ID").ok().filter(|t| !t.is_empty());
        Self::new(tab, flare_ctl::default_pipe_name())
    }

    pub fn new(tab: Option<String>, pipe: Option<String>) -> Self {
        Self {
            tab,
            pipe,
            client: None,
        }
    }

    fn connect(&mut self) -> Result<(), ChannelError> {
        if self.client.is_some() {
            return Ok(());
        }
        let Some(tab) = self.tab.clone() else {
            return Err(ChannelError::NotRunning("FLARE_TAB_ID is not set".into()));
        };
        let Some(pipe) = self.pipe.clone() else {
            return Err(ChannelError::NotRunning(
                "no Flare pipe to connect to".into(),
            ));
        };
        let mut client = flare_ctl::connect(&pipe, CALL_TIMEOUT)
            .map_err(|e| ChannelError::NotRunning(format!("could not connect: {e}")))?;
        client
            .hello(
                &flare_ctl::HelloParams {
                    client: SERVER_NAME.into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    token: None,
                    tab: Some(tab),
                },
                CALL_TIMEOUT,
            )
            .map_err(client_error)?;
        self.client = Some(client);
        Ok(())
    }
}

fn client_error(e: ClientError) -> ChannelError {
    match e {
        ClientError::Remote(r) => ChannelError::Remote {
            code: r.code,
            message: r.message,
        },
        other => ChannelError::Failed(other.to_string()),
    }
}

impl Channel for FlareChannel {
    fn call(&mut self, method: Method, params: Value) -> Result<Value, ChannelError> {
        for attempt in 0..2 {
            self.connect()?;
            let client = self.client.as_mut().expect("connected above");
            match client.call(method, params.clone(), CALL_TIMEOUT) {
                Ok(v) => return Ok(v),
                Err(ClientError::Remote(r)) => {
                    return Err(ChannelError::Remote {
                        code: r.code,
                        message: r.message,
                    });
                }
                // A closed or broken connection (Flare restarted): drop it and try once more.
                Err(ClientError::Closed | ClientError::Io(_)) if attempt == 0 => {
                    self.client = None;
                }
                Err(e) => {
                    self.client = None;
                    return Err(client_error(e));
                }
            }
        }
        Err(ChannelError::Failed("Flare did not answer".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records calls and answers from a script.
    #[derive(Default)]
    struct Fake {
        calls: Vec<(Method, Value)>,
        answer: Option<Result<Value, ChannelError>>,
    }

    impl Channel for Fake {
        fn call(&mut self, method: Method, params: Value) -> Result<Value, ChannelError> {
            self.calls.push((method, params));
            self.answer.take().unwrap_or(Ok(json!({})))
        }
    }

    fn req(id: i64, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    fn text_of(reply: &Value) -> String {
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn initialize_echoes_a_supported_version() {
        let mut ch = Fake::default();
        let r = handle_message(
            &req(1, "initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}})),
            &mut ch,
        )
        .unwrap();
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "flare-mcp");
        assert_eq!(r["result"]["capabilities"]["tools"]["listChanged"], false);
        assert!(ch.calls.is_empty());
    }

    #[test]
    fn initialize_offers_the_newest_version_for_an_unknown_one() {
        let mut ch = Fake::default();
        for asked in [json!("1999-01-01"), json!(7), Value::Null] {
            let r = handle_message(
                &req(1, "initialize", json!({"protocolVersion": asked})),
                &mut ch,
            )
            .unwrap();
            assert_eq!(r["result"]["protocolVersion"], SUPPORTED_VERSIONS[0]);
        }
        let r = handle_message(&req(2, "initialize", json!({})), &mut ch).unwrap();
        assert_eq!(r["result"]["protocolVersion"], SUPPORTED_VERSIONS[0]);
    }

    #[test]
    fn notifications_get_no_reply() {
        let mut ch = Fake::default();
        let n = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert!(handle_message(&n, &mut ch).is_none());
        let n = json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 3}});
        assert!(handle_message(&n, &mut ch).is_none());
        // A stray client response is ignored too.
        assert!(
            handle_message(&json!({"jsonrpc": "2.0", "id": 5, "result": {}}), &mut ch).is_none()
        );
    }

    #[test]
    fn ping_answers_an_empty_result() {
        let r = handle_message(&req(9, "ping", json!({})), &mut Fake::default()).unwrap();
        assert_eq!(r["result"], json!({}));
        assert_eq!(r["id"], 9);
    }

    #[test]
    fn string_ids_are_echoed() {
        let m = json!({"jsonrpc": "2.0", "id": "abc", "method": "ping"});
        assert_eq!(
            handle_message(&m, &mut Fake::default()).unwrap()["id"],
            "abc"
        );
    }

    #[test]
    fn tools_list_has_the_three_tools_with_object_schemas() {
        let r = handle_message(&req(1, "tools/list", json!({})), &mut Fake::default()).unwrap();
        let list = r["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = list.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["show_image", "show_file", "close_panel"]);
        for t in list {
            assert!(!t["description"].as_str().unwrap().is_empty());
            assert_eq!(t["inputSchema"]["type"], "object");
        }
        assert_eq!(list[0]["inputSchema"]["required"], json!(["path"]));
        assert_eq!(list[2]["inputSchema"]["properties"], json!({}));
    }

    #[test]
    fn show_image_routes_to_view_image_with_an_absolute_path() {
        let mut ch = Fake::default();
        let r = handle_message(
            &req(
                1,
                "tools/call",
                json!({"name": "show_image", "arguments": {"path": "C:\\pics\\a.png"}}),
            ),
            &mut ch,
        )
        .unwrap();
        assert_eq!(
            ch.calls,
            vec![(Method::ViewImage, json!({"path": "C:\\pics\\a.png"}))]
        );
        assert_eq!(r["result"]["isError"], false);
        assert!(text_of(&r).contains("a.png"));
    }

    #[test]
    fn a_relative_path_is_made_absolute() {
        let mut ch = Fake::default();
        handle_message(
            &req(
                1,
                "tools/call",
                json!({"name": "show_file", "arguments": {"path": "src\\main.rs"}}),
            ),
            &mut ch,
        );
        assert_eq!(ch.calls[0].0, Method::ViewFile);
        let sent = ch.calls[0].1["path"].as_str().unwrap();
        assert!(std::path::Path::new(sent).is_absolute(), "{sent}");
        assert!(sent.ends_with("main.rs"));
    }

    #[test]
    fn close_panel_routes_to_panel_close() {
        let mut ch = Fake {
            answer: Some(Ok(json!({"closed": false}))),
            ..Fake::default()
        };
        let r = handle_message(
            &req(1, "tools/call", json!({"name": "close_panel"})),
            &mut ch,
        )
        .unwrap();
        assert_eq!(ch.calls, vec![(Method::PanelClose, json!({}))]);
        assert_eq!(text_of(&r), "No panel was open.");
        assert_eq!(r["result"]["isError"], false);
    }

    #[test]
    fn bad_arguments_are_tool_errors_and_never_reach_flare() {
        let mut ch = Fake::default();
        for args in [
            json!({}),
            json!({"path": ""}),
            json!({"path": "  "}),
            json!({"path": 5}),
        ] {
            let r = handle_message(
                &req(
                    1,
                    "tools/call",
                    json!({"name": "show_image", "arguments": args}),
                ),
                &mut ch,
            )
            .unwrap();
            assert_eq!(r["result"]["isError"], true);
            assert!(text_of(&r).contains("path"));
        }
        assert!(ch.calls.is_empty());
    }

    #[test]
    fn protocol_level_mistakes_are_rpc_errors() {
        let mut ch = Fake::default();
        let r = handle_message(&req(1, "tools/call", json!({"name": "nope"})), &mut ch).unwrap();
        assert_eq!(r["error"]["code"], rpc::INVALID_PARAMS);
        let r = handle_message(&req(2, "tools/call", json!({})), &mut ch).unwrap();
        assert_eq!(r["error"]["code"], rpc::INVALID_PARAMS);
        let r = handle_message(
            &req(
                3,
                "tools/call",
                json!({"name": "show_image", "arguments": [1]}),
            ),
            &mut ch,
        )
        .unwrap();
        assert_eq!(r["error"]["code"], rpc::INVALID_PARAMS);
        let r = handle_message(&req(4, "resources/list", json!({})), &mut ch).unwrap();
        assert_eq!(r["error"]["code"], rpc::METHOD_NOT_FOUND);
        let r = handle_message(&json!([1, 2]), &mut ch).unwrap();
        assert_eq!(r["error"]["code"], rpc::INVALID_REQUEST);
        assert!(ch.calls.is_empty());
    }

    #[test]
    fn channel_errors_map_to_is_error_text() {
        let cases = [
            (
                ChannelError::NotRunning("FLARE_TAB_ID is not set".into()),
                "Flare is not running this session",
            ),
            (
                ChannelError::Remote {
                    code: "not_found".into(),
                    message: "no such file".into(),
                },
                "Flare refused: no such file (not_found)",
            ),
            (
                ChannelError::Failed("timed out".into()),
                "Could not reach Flare: timed out",
            ),
        ];
        for (err, want) in cases {
            let mut ch = Fake {
                answer: Some(Err(err)),
                ..Fake::default()
            };
            let r = handle_message(
                &req(
                    1,
                    "tools/call",
                    json!({"name": "show_file", "arguments": {"path": "C:\\a"}}),
                ),
                &mut ch,
            )
            .unwrap();
            assert_eq!(r["result"]["isError"], true);
            assert!(text_of(&r).contains(want), "{}", text_of(&r));
        }
    }

    #[test]
    fn image_content_serialises_as_mcp_image() {
        let c = Content::Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
        };
        assert_eq!(
            c.to_json(),
            json!({"type": "image", "data": "AAAA", "mimeType": "image/png"})
        );
        let r = ToolResult {
            content: vec![Content::Text("t".into()), c],
            is_error: false,
        };
        assert_eq!(r.to_json()["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn run_answers_line_by_line_and_survives_garbage() {
        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "",
            "this is not json",
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        ]
        .join("\n");
        let mut out = Vec::new();
        run(input.as_bytes(), &mut out, &mut Fake::default()).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[1]["error"]["code"], rpc::PARSE_ERROR);
        assert_eq!(lines[1]["id"], Value::Null);
        assert_eq!(lines[2]["id"], 2);
    }

    #[test]
    fn without_a_tab_the_real_channel_says_flare_is_not_running() {
        let mut ch = FlareChannel::new(None, Some("\\\\.\\pipe\\flare-ctl-nobody-home".into()));
        match ch.call(Method::PanelClose, json!({})) {
            Err(ChannelError::NotRunning(m)) => assert!(m.contains("FLARE_TAB_ID")),
            other => panic!("{other:?}"),
        }
        let mut ch = FlareChannel::new(
            Some("t1".into()),
            Some("\\\\.\\pipe\\flare-ctl-nobody-home".into()),
        );
        assert!(matches!(
            ch.call(Method::PanelClose, json!({})),
            Err(ChannelError::NotRunning(_))
        ));
    }
}
