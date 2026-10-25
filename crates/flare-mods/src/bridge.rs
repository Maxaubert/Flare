//! The `window.flare` bridge of a mod's page (stage 8, the app part): the script the host injects before the
//! page's own scripts, and the messages that cross between the page and the host.
//!
//! The page never holds the token. It talks to the host over the browser's web-message channel (WebView2's
//! `window.chrome.webview`; Electron's preload and `ipcRenderer`), and the host adds the mod's grant to every
//! call it forwards. All messages are JSON text:
//!
//! - page to host: `{"t":"call","id":<n>,"method":"view.text","params":{...}}` and
//!   `{"t":"respond","call_id":"c1","result":<any>}` or `{"t":"respond","call_id":"c1","error":"why"}`;
//! - host to page: `{"t":"reply","id":<n>,"result":<any>}` or `{"t":"reply","id":<n>,"error":{"code","message"}}`
//!   and `{"t":"tool","callId":"c1","tool":"append_note","input":{...}}`.
//! - plugins (phase 3): the page says which events it listens to, `{"t":"listen","name":"tabs"}`, the host hands
//!   it `{"t":"event","name":"tabs","data":{...}}`, and the host's `{"t":"ping","id":<n>}` is answered by the
//!   bridge itself with `{"t":"pong","id":<n>}` (a background page that stops answering is closed).
//!
//! Everything the page sends is untrusted: [`parse_page_message`] checks the shape and the size before the host
//! looks at a method name.

use serde_json::{Value, json};

/// The most a message from the page may be (bytes). A bigger one is dropped with an error to the page.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// The longest `call_id` the page may answer with.
pub const MAX_CALL_ID: usize = 80;

/// The script injected into a mod's page before any of its own, in every frame. It defines
/// `window.flare` (read-only, frozen) and nothing else global.
pub const BRIDGE_JS: &str = r#"(function () {
  'use strict';
  if (window.flare) { return; }
  var wv = window.chrome && window.chrome.webview;
  if (!wv) { return; }
  var names = ['tool', 'tabs', 'agent', 'plugin'];
  var handlers = {};
  var seq = 0;
  var waiting = new Map();
  function send(o) { wv.postMessage(JSON.stringify(o)); }
  wv.addEventListener('message', function (e) {
    var m = e.data;
    if (typeof m === 'string') { try { m = JSON.parse(m); } catch (x) { return; } }
    if (!m || typeof m !== 'object') { return; }
    if (m.t === 'reply') {
      var p = waiting.get(m.id);
      if (!p) { return; }
      waiting.delete(m.id);
      if (m.error) { p.reject(m.error); } else { p.resolve(m.result); }
    } else if (m.t === 'tool') {
      var id = String(m.callId);
      var handler = handlers.tool;
      if (!handler) {
        send({ t: 'respond', call_id: id, error: 'the mod is not listening for tool calls' });
        return;
      }
      try {
        handler({ callId: id, tool: String(m.tool), input: m.input });
      } catch (x) {
        send({ t: 'respond', call_id: id, error: 'the tool handler threw: ' + String((x && x.message) || x) });
      }
    } else if (m.t === 'event') {
      var fn = names.indexOf(m.name) > 0 ? handlers[m.name] : null;
      if (!fn) { return; }
      try { fn(m.data); } catch (x) { if (window.console) { console.error(x); } }
    } else if (m.t === 'ping') {
      send({ t: 'pong', id: m.id });
    }
  });
  var flare = {
    on: function (name, fn) {
      if (names.indexOf(name) < 0) { throw new TypeError("window.flare.on: the events are 'tool', 'tabs', 'agent' and 'plugin'"); }
      if (typeof fn !== 'function') { throw new TypeError('window.flare.on: the handler must be a function'); }
      if (handlers[name]) { throw new Error("window.flare.on: a '" + name + "' handler is already registered"); }
      handlers[name] = fn;
      if (name !== 'tool') { send({ t: 'listen', name: name }); }
    },
    respond: function (callId, result, error) {
      if (typeof callId !== 'string' || !callId) { throw new TypeError('window.flare.respond: callId must be the call\'s callId'); }
      if (typeof error === 'string') {
        send({ t: 'respond', call_id: callId, error: error });
        return;
      }
      var text;
      try {
        text = JSON.stringify({ t: 'respond', call_id: callId, result: result === undefined ? null : result });
      } catch (x) {
        send({ t: 'respond', call_id: callId, error: 'the result could not be sent: ' + String((x && x.message) || x) });
        return;
      }
      wv.postMessage(text);
    },
    call: function (method, params) {
      return new Promise(function (resolve, reject) {
        var id = ++seq;
        waiting.set(id, { resolve: resolve, reject: reject });
        try {
          send({ t: 'call', id: id, method: String(method), params: params === undefined ? {} : params });
        } catch (x) {
          waiting.delete(id);
          reject({ code: 'bad_params', message: String((x && x.message) || x) });
        }
      });
    }
  };
  Object.defineProperty(window, 'flare', { value: Object.freeze(flare), writable: false, configurable: false, enumerable: true });
})();"#;

/// The events a page may listen to besides `tool` (plugins phase 3), each with one handler per page.
pub const EVENTS: [&str; 3] = ["tabs", "agent", "plugin"];

/// What the page sent.
#[derive(Debug, PartialEq)]
pub enum PageMsg {
    /// `window.flare.call(method, params)`.
    Call {
        id: u64,
        method: String,
        params: Value,
    },
    /// `window.flare.respond(callId, result, error)`: the outcome is the result, or the reason.
    Respond {
        call_id: String,
        outcome: Result<Value, String>,
    },
    /// `window.flare.on(name, fn)` for an event other than `tool`: the page listens to `name` (one of [`EVENTS`]).
    Listen { name: String },
    /// The bridge's answer to the host's ping.
    Pong { id: u64 },
}

/// Reads a message from the page. The reason is one sentence, for the page's own console.
pub fn parse_page_message(text: &str) -> Result<PageMsg, String> {
    if text.len() > MAX_MESSAGE_BYTES {
        return Err("the message is larger than 1 MB".into());
    }
    let v: Value = serde_json::from_str(text).map_err(|_| "the message is not JSON".to_string())?;
    let obj = v
        .as_object()
        .ok_or_else(|| "the message is not an object".to_string())?;
    match obj.get("t").and_then(Value::as_str) {
        Some("call") => {
            let id = obj
                .get("id")
                .and_then(Value::as_u64)
                .ok_or_else(|| "a call needs a numeric id".to_string())?;
            let method = obj
                .get("method")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty() && m.len() <= 64)
                .ok_or_else(|| "a call needs a method name".to_string())?
                .to_string();
            let params = match obj.get("params") {
                None | Some(Value::Null) => json!({}),
                Some(p @ Value::Object(_)) => p.clone(),
                Some(_) => return Err("params must be an object".into()),
            };
            Ok(PageMsg::Call { id, method, params })
        }
        Some("respond") => {
            let call_id = obj
                .get("call_id")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty() && c.len() <= MAX_CALL_ID)
                .ok_or_else(|| "a response needs the call's id".to_string())?
                .to_string();
            let outcome = match (obj.get("error"), obj.get("result")) {
                (Some(Value::String(e)), None) => Err(e.clone()),
                (None, Some(r)) => Ok(r.clone()),
                _ => return Err("a response has either a result or an error, not both".into()),
            };
            Ok(PageMsg::Respond { call_id, outcome })
        }
        Some("listen") => {
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| EVENTS.contains(n))
                .ok_or_else(|| "a page listens to tabs, agent or plugin".to_string())?;
            Ok(PageMsg::Listen {
                name: name.to_string(),
            })
        }
        Some("pong") => {
            let id = obj
                .get("id")
                .and_then(Value::as_u64)
                .ok_or_else(|| "a pong needs the ping's id".to_string())?;
            Ok(PageMsg::Pong { id })
        }
        _ => Err("the message type is not call, respond, listen or pong".into()),
    }
}

/// The reply to a page's call that succeeded.
pub fn reply_ok(id: u64, result: &Value) -> String {
    json!({"t": "reply", "id": id, "result": result}).to_string()
}

/// The reply to a page's call that failed: the channel's code and one sentence.
pub fn reply_err(id: u64, code: &str, message: &str) -> String {
    json!({"t": "reply", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// The event that hands the agent's tool call to the page.
pub fn tool_event(call_id: &str, tool: &str, input: &Value) -> String {
    json!({"t": "tool", "callId": call_id, "tool": tool, "input": input}).to_string()
}

/// An event for a page that listens to `name` (one of [`EVENTS`]).
pub fn event_message(name: &str, data: &Value) -> String {
    json!({"t": "event", "name": name, "data": data}).to_string()
}

/// The host's ping; the bridge answers it with a pong of the same id.
pub fn ping_message(id: u64) -> String {
    json!({"t": "ping", "id": id}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_and_both_responses_parse() {
        let c =
            parse_page_message(r#"{"t":"call","id":3,"method":"view.text","params":{"text":"x"}}"#)
                .unwrap();
        assert_eq!(
            c,
            PageMsg::Call {
                id: 3,
                method: "view.text".into(),
                params: json!({"text": "x"})
            }
        );
        let ok =
            parse_page_message(r#"{"t":"respond","call_id":"c1","result":{"count":2}}"#).unwrap();
        assert_eq!(
            ok,
            PageMsg::Respond {
                call_id: "c1".into(),
                outcome: Ok(json!({"count": 2}))
            }
        );
        let null = parse_page_message(r#"{"t":"respond","call_id":"c1","result":null}"#).unwrap();
        assert_eq!(
            null,
            PageMsg::Respond {
                call_id: "c1".into(),
                outcome: Ok(Value::Null)
            },
            "a present null is a success"
        );
        let bad = parse_page_message(r#"{"t":"respond","call_id":"c1","error":"no"}"#).unwrap();
        assert_eq!(
            bad,
            PageMsg::Respond {
                call_id: "c1".into(),
                outcome: Err("no".into())
            }
        );
    }

    #[test]
    fn a_listen_names_one_of_the_events_and_a_pong_its_ping() {
        for name in EVENTS {
            let m = parse_page_message(&format!(r#"{{"t":"listen","name":"{name}"}}"#)).unwrap();
            assert_eq!(m, PageMsg::Listen { name: name.into() });
        }
        for bad in [
            r#"{"t":"listen","name":"tool"}"#,
            r#"{"t":"listen","name":"session"}"#,
            r#"{"t":"listen"}"#,
            r#"{"t":"pong"}"#,
            r#"{"t":"pong","id":"1"}"#,
        ] {
            assert!(parse_page_message(bad).is_err(), "{bad}");
        }
        assert_eq!(
            parse_page_message(r#"{"t":"pong","id":4}"#).unwrap(),
            PageMsg::Pong { id: 4 }
        );
        let e: Value =
            serde_json::from_str(&event_message("tabs", &json!({"kind": "opened"}))).unwrap();
        assert_eq!(
            e,
            json!({"t":"event","name":"tabs","data":{"kind":"opened"}})
        );
        let p: Value = serde_json::from_str(&ping_message(9)).unwrap();
        assert_eq!(p, json!({"t":"ping","id":9}));
    }

    #[test]
    fn the_script_takes_one_handler_per_event_and_answers_a_ping_itself() {
        assert!(BRIDGE_JS.contains("['tool', 'tabs', 'agent', 'plugin']"));
        assert!(BRIDGE_JS.contains("t: 'listen'"));
        assert!(BRIDGE_JS.contains("t: 'pong'"));
        assert!(BRIDGE_JS.contains("is already registered"));
    }

    #[test]
    fn a_call_without_params_gets_an_empty_object() {
        let c = parse_page_message(r#"{"t":"call","id":1,"method":"tabs.list"}"#).unwrap();
        assert!(matches!(c, PageMsg::Call { params, .. } if params == json!({})));
    }

    #[test]
    fn what_is_not_a_message_is_refused_with_a_reason() {
        for bad in [
            "not json",
            "[]",
            "{}",
            r#"{"t":"other"}"#,
            r#"{"t":"call","id":"x","method":"a"}"#,
            r#"{"t":"call","id":-1,"method":"a"}"#,
            r#"{"t":"call","id":1}"#,
            r#"{"t":"call","id":1,"method":""}"#,
            r#"{"t":"call","id":1,"method":"a","params":[1]}"#,
            r#"{"t":"respond","call_id":"c1"}"#,
            r#"{"t":"respond","call_id":"c1","result":1,"error":"x"}"#,
            r#"{"t":"respond","call_id":"c1","error":5}"#,
            r#"{"t":"respond","call_id":"","result":1}"#,
            r#"{"t":"respond","call_id":5,"result":1}"#,
        ] {
            assert!(parse_page_message(bad).is_err(), "{bad}");
        }
        let big = format!(
            r#"{{"t":"respond","call_id":"c","result":"{}"}}"#,
            "x".repeat(MAX_MESSAGE_BYTES)
        );
        assert!(parse_page_message(&big).unwrap_err().contains("1 MB"));
    }

    #[test]
    fn host_messages_are_json_the_script_reads() {
        let r: Value = serde_json::from_str(&reply_ok(7, &json!([1]))).unwrap();
        assert_eq!(r, json!({"t":"reply","id":7,"result":[1]}));
        let e: Value =
            serde_json::from_str(&reply_err(7, "forbidden", "no browser permission")).unwrap();
        assert_eq!(e["error"]["code"], "forbidden");
        let t: Value =
            serde_json::from_str(&tool_event("c9", "append_note", &json!({"text":"a"}))).unwrap();
        assert_eq!(
            t,
            json!({"t":"tool","callId":"c9","tool":"append_note","input":{"text":"a"}})
        );
    }

    #[test]
    fn the_script_exposes_exactly_on_respond_and_call_and_never_a_token() {
        assert!(BRIDGE_JS.contains("on: function"));
        assert!(BRIDGE_JS.contains("respond: function"));
        assert!(BRIDGE_JS.contains("call: function"));
        assert!(BRIDGE_JS.contains("Object.freeze(flare)"));
        assert!(!BRIDGE_JS.to_ascii_lowercase().contains("token"));
        assert!(!BRIDGE_JS.contains("eval("));
    }
}
