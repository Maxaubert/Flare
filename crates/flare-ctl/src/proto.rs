//! Wire types of the control channel, protocol v1: requests, responses, events, error codes and the
//! parse step. JSON, one object per line. The contract is `docs/surfaces/protocol.md`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The protocol version this crate speaks.
pub const PROTOCOL: u32 = 1;
/// The longest line (one JSON message, without its newline) either side accepts: 16 MB.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// Machine-readable error codes. Strings on the wire; new ones may be added within v1, so a client
/// treats an unknown code as a plain failure.
pub mod code {
    /// The line was not valid JSON. Closes the client.
    pub const PARSE_ERROR: &str = "parse_error";
    /// Valid JSON that is not a request (no id, no method, wrong types). Closes the client.
    pub const INVALID_REQUEST: &str = "invalid_request";
    /// The line passed the size cap. Closes the client.
    pub const TOO_LARGE: &str = "too_large";
    /// The server already serves its client cap. Closes the client.
    pub const TOO_MANY_CLIENTS: &str = "too_many_clients";
    /// The method name is not in the protocol.
    pub const UNKNOWN_METHOD: &str = "unknown_method";
    /// The grant does not allow the method (rule S3 and friends).
    pub const FORBIDDEN: &str = "forbidden";
    /// A request other than `hello` came first.
    pub const HELLO_REQUIRED: &str = "hello_required";
    /// `hello` was sent twice on one connection.
    pub const ALREADY_HELLO: &str = "already_hello";
    /// The token is unknown, or its mod was disabled. Closes the client.
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// The client asked for a protocol version the server does not speak. Closes the client.
    pub const UNSUPPORTED_PROTOCOL: &str = "unsupported_protocol";
    /// The params do not fit the method.
    pub const BAD_PARAMS: &str = "bad_params";
    /// The method needs a tab and the connection has none (or it is gone).
    pub const NO_TAB: &str = "no_tab";
    /// A path, element or panel the call named does not exist.
    pub const NOT_FOUND: &str = "not_found";
    /// The method is in the protocol but this build or this moment cannot do it (no WebView2, no
    /// panel open, a reserved method).
    pub const UNAVAILABLE: &str = "unavailable";
    /// The request was not answered within its timeout.
    pub const TIMEOUT: &str = "timeout";
    /// The client sent too many requests too fast. The connection stays open.
    pub const RATE_LIMITED: &str = "rate_limited";
    /// The server failed while handling the request.
    pub const INTERNAL: &str = "internal";
    /// The user took over the page (a click or a key of their own in it) while the call ran. The
    /// page was left as it is; look again before going on.
    pub const INTERRUPTED: &str = "interrupted";

    /// Errors after which the server closes the connection.
    pub fn closes_client(code: &str) -> bool {
        matches!(
            code,
            PARSE_ERROR
                | INVALID_REQUEST
                | TOO_LARGE
                | TOO_MANY_CLIENTS
                | UNAUTHORIZED
                | UNSUPPORTED_PROTOCOL
        )
    }
}

/// `{code, message}` as it travels, and as a Rust error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CtlError {
    pub code: String,
    pub message: String,
}

impl CtlError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CtlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for CtlError {}

/// A request line. `id` is echoed untouched (a number or a string).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Request {
    pub id: Value,
    pub method: String,
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, method: &str, params: Value) -> Self {
        Self {
            id: Value::from(id),
            method: method.into(),
            params,
        }
    }
}

/// A response line: exactly one of `result` and `error`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<CtlError>,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, error: CtlError) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A server push: `{event, params}`, no id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub event: String,
    #[serde(default)]
    pub params: Value,
}

/// One line from the server, told apart.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Response(Response),
    Event(Event),
}

/// Reads a server line: an object with `event` is an event, one with `id` a response.
pub fn parse_incoming(line: &[u8]) -> Result<Incoming, String> {
    let value: Value = serde_json::from_slice(line).map_err(|e| format!("not JSON: {e}"))?;
    let obj = value.as_object().ok_or("not an object")?;
    if obj.contains_key("event") {
        return serde_json::from_value(value)
            .map(Incoming::Event)
            .map_err(|e| format!("bad event: {e}"));
    }
    if obj.contains_key("id") {
        let resp: Response =
            serde_json::from_value(value).map_err(|e| format!("bad response: {e}"))?;
        if resp.result.is_some() == resp.error.is_some() {
            return Err("a response carries exactly one of result and error".into());
        }
        return Ok(Incoming::Response(resp));
    }
    Err("neither an event nor a response".into())
}

/// Why a client line is refused. Carries what the server answers with (`id` when it could be read).
#[derive(Clone, Debug, PartialEq)]
pub struct ParseFailure {
    pub id: Value,
    pub error: CtlError,
}

/// Parses a request line. Malformed JSON is `parse_error`; JSON that is not a request is
/// `invalid_request` (the id is echoed when it was readable, else null). `params` defaults to `{}`
/// and must be an object.
pub fn parse_request(line: &[u8]) -> Result<Request, ParseFailure> {
    let fail = |id: Value, c: &str, m: &str| ParseFailure {
        id,
        error: CtlError::new(c, m),
    };
    let value: Value = serde_json::from_slice(line)
        .map_err(|e| fail(Value::Null, code::PARSE_ERROR, &format!("not JSON: {e}")))?;
    let Value::Object(mut obj) = value else {
        return Err(fail(
            Value::Null,
            code::INVALID_REQUEST,
            "a request is a JSON object",
        ));
    };
    let id = obj.remove("id").unwrap_or(Value::Null);
    if !(id.is_number() || id.is_string()) {
        return Err(fail(
            Value::Null,
            code::INVALID_REQUEST,
            "id must be a number or a string",
        ));
    }
    let Some(Value::String(method)) = obj.remove("method") else {
        return Err(fail(id, code::INVALID_REQUEST, "method must be a string"));
    };
    let params = match obj.remove("params") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(p @ Value::Object(_)) => p,
        Some(_) => return Err(fail(id, code::INVALID_REQUEST, "params must be an object")),
    };
    Ok(Request { id, method, params })
}

/// Serialises a message as one line (with the trailing newline).
pub fn to_line<T: Serialize>(msg: &T) -> Vec<u8> {
    let mut out = serde_json::to_vec(msg).unwrap_or_else(|_| b"{}".to_vec());
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_round_trips() {
        let r = parse_request(br#"{"id":7,"method":"panel.close","params":{"a":1}}"#).unwrap();
        assert_eq!(r.id, json!(7));
        assert_eq!(r.method, "panel.close");
        assert_eq!(r.params, json!({"a":1}));
    }

    #[test]
    fn string_ids_are_kept() {
        let r = parse_request(br#"{"id":"abc","method":"hello"}"#).unwrap();
        assert_eq!(r.id, json!("abc"));
        assert_eq!(r.params, json!({}));
    }

    #[test]
    fn null_params_become_empty() {
        let r = parse_request(br#"{"id":1,"method":"x","params":null}"#).unwrap();
        assert_eq!(r.params, json!({}));
    }

    #[test]
    fn malformed_json_is_parse_error_with_null_id() {
        let f = parse_request(b"{nope").unwrap_err();
        assert_eq!(f.error.code, code::PARSE_ERROR);
        assert_eq!(f.id, Value::Null);
        assert!(code::closes_client(&f.error.code));
    }

    #[test]
    fn non_object_is_invalid_request() {
        for line in [&b"[1,2]"[..], b"42", b"\"x\"", b"null"] {
            let f = parse_request(line).unwrap_err();
            assert_eq!(f.error.code, code::INVALID_REQUEST, "{line:?}");
        }
    }

    #[test]
    fn missing_or_odd_id_is_invalid_request() {
        for line in [
            &br#"{"method":"x"}"#[..],
            br#"{"id":null,"method":"x"}"#,
            br#"{"id":true,"method":"x"}"#,
            br#"{"id":[1],"method":"x"}"#,
        ] {
            let f = parse_request(line).unwrap_err();
            assert_eq!(f.error.code, code::INVALID_REQUEST);
            assert_eq!(f.id, Value::Null);
        }
    }

    #[test]
    fn bad_method_or_params_echo_the_id() {
        let f = parse_request(br#"{"id":3,"method":5}"#).unwrap_err();
        assert_eq!(f.error.code, code::INVALID_REQUEST);
        assert_eq!(f.id, json!(3));
        let f = parse_request(br#"{"id":3,"method":"x","params":[1]}"#).unwrap_err();
        assert_eq!(f.error.code, code::INVALID_REQUEST);
        assert_eq!(f.id, json!(3));
        let f = parse_request(br#"{"id":3}"#).unwrap_err();
        assert_eq!(f.error.code, code::INVALID_REQUEST);
    }

    #[test]
    fn response_serialises_one_side_only() {
        let ok = serde_json::to_string(&Response::ok(json!(1), json!({"a":1}))).unwrap();
        assert_eq!(ok, r#"{"id":1,"result":{"a":1}}"#);
        let bad = serde_json::to_string(&Response::err(json!(2), CtlError::new("x", "y"))).unwrap();
        assert_eq!(bad, r#"{"id":2,"error":{"code":"x","message":"y"}}"#);
    }

    #[test]
    fn incoming_tells_events_from_responses() {
        assert!(matches!(
            parse_incoming(br#"{"event":"panel.changed","params":{}}"#),
            Ok(Incoming::Event(_))
        ));
        assert!(matches!(
            parse_incoming(br#"{"id":1,"result":{}}"#),
            Ok(Incoming::Response(_))
        ));
        assert!(parse_incoming(br#"{"id":1}"#).is_err());
        assert!(
            parse_incoming(br#"{"id":1,"result":1,"error":{"code":"a","message":"b"}}"#).is_err()
        );
        assert!(parse_incoming(b"{}").is_err());
        assert!(parse_incoming(b"nope").is_err());
        assert!(parse_incoming(b"[]").is_err());
    }

    #[test]
    fn to_line_ends_with_one_newline() {
        let l = to_line(&Response::ok(json!(1), json!({})));
        assert_eq!(l.last(), Some(&b'\n'));
        assert_eq!(l.iter().filter(|b| **b == b'\n').count(), 1);
    }
}
