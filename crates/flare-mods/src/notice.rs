//! A plugin's short notice (`notice.show`, rule 1275, 2026-11-17): one line under the title bar, with the plugin's logo
//! and a close button, gone after a few seconds. This is the params' parse and the limits; the window draws it.
//!
//! The text is one line a person reads: at most [`MAX_TEXT_CHARS`] characters as sent, with every control, line break
//! and hidden formatting character (zero-width, bidi, the BOM, tag characters) taken out and runs of white space made
//! one space, as `flare_ctl::text::clean_text` does for every other text from outside. A text that is blank after that
//! is refused. `kind` is `"error"` (a red edge) or `"info"` (the default).

use std::time::Duration;

use flare_ctl::{CtlError, code};
use serde_json::Value;

/// The longest text a notice takes, in characters, as the plugin sent it.
pub const MAX_TEXT_CHARS: usize = 120;
/// How long a notice shows while the pointer is not on it.
pub const SHOW_FOR: Duration = Duration::from_secs(6);
/// Notices one plugin may show in [`RATE_WINDOW`]: a fourth in that time is `rate_limited`.
pub const RATE_BURST: f64 = 3.0;
pub const RATE_WINDOW: Duration = Duration::from_secs(10);

/// The keys `notice.show` takes. Any other is refused, so a misspelt one is never dropped quietly.
const KEYS: [&str; 2] = ["text", "kind"];

/// What the notice says about itself: an error gets a red edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Error,
}

impl NoticeKind {
    pub fn name(self) -> &'static str {
        match self {
            NoticeKind::Info => "info",
            NoticeKind::Error => "error",
        }
    }
}

/// A checked `notice.show`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeRequest {
    pub text: String,
    pub kind: NoticeKind,
}

fn bad(m: impl Into<String>) -> CtlError {
    CtlError::new(code::BAD_PARAMS, m.into())
}

/// The rate a plugin's notices refill at, per second (three in ten seconds).
pub fn rate_per_second() -> f64 {
    RATE_BURST / RATE_WINDOW.as_secs_f64()
}

/// The `rate_limited` message.
pub fn rate_message() -> String {
    format!(
        "at most {} notices in {} seconds for one plugin",
        RATE_BURST as u32,
        RATE_WINDOW.as_secs()
    )
}

/// Reads `notice.show`'s params: `{text, kind?}`.
pub fn parse_notice(params: &Value) -> Result<NoticeRequest, CtlError> {
    let obj = params
        .as_object()
        .ok_or_else(|| bad("params must be an object"))?;
    if let Some(k) = obj.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return Err(bad(format!(
            "unknown param \"{}\"; notice.show takes {}",
            k.chars().take(40).collect::<String>(),
            KEYS.join(", ")
        )));
    }
    let raw = match obj.get("text") {
        None | Some(Value::Null) => return Err(bad("text is required")),
        Some(Value::String(s)) => s,
        Some(_) => return Err(bad("text must be text")),
    };
    if raw.chars().count() > MAX_TEXT_CHARS {
        return Err(bad(format!(
            "text is longer than {MAX_TEXT_CHARS} characters"
        )));
    }
    let text = clean(raw);
    if text.is_empty() {
        return Err(bad("text must not be blank"));
    }
    let kind = match obj.get("kind") {
        None | Some(Value::Null) => NoticeKind::Info,
        Some(Value::String(s)) if s == "info" => NoticeKind::Info,
        Some(Value::String(s)) if s == "error" => NoticeKind::Error,
        Some(_) => return Err(bad("kind is \"error\" or \"info\"")),
    };
    Ok(NoticeRequest { text, kind })
}

/// One clean line: no control, line break or hidden formatting character, one space between words, no cut (the
/// length was checked before).
fn clean(s: &str) -> String {
    let no_hidden: String = s
        .chars()
        .filter(|c| !crate::manifest::is_format_char(*c))
        .collect();
    flare_ctl::text::clean_text(&no_hidden, MAX_TEXT_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn code_of(v: Value) -> String {
        parse_notice(&v).unwrap_err().code.to_string()
    }

    #[test]
    fn a_notice_is_one_line_of_text_and_a_kind() {
        let r = parse_notice(&json!({"text": "Bitwarden could not lock the vault. It is still unlocked.", "kind": "error"}))
            .unwrap();
        assert_eq!(
            r.text,
            "Bitwarden could not lock the vault. It is still unlocked."
        );
        assert_eq!(r.kind, NoticeKind::Error);
        let r = parse_notice(&json!({"text": "Synced"})).unwrap();
        assert_eq!(r.kind, NoticeKind::Info, "info is the default");
        let r = parse_notice(&json!({"text": "Synced", "kind": "info"})).unwrap();
        assert_eq!(r.kind, NoticeKind::Info);
        let r = parse_notice(&json!({"text": "Synced", "kind": null})).unwrap();
        assert_eq!(r.kind, NoticeKind::Info);
        assert_eq!(NoticeKind::Error.name(), "error");
    }

    #[test]
    fn hidden_characters_and_line_breaks_are_taken_out() {
        let r = parse_notice(
            &json!({"text": "  Vault\u{202E}kcol \u{200B}failed\r\n\tagain\u{FEFF}\u{2066} "}),
        )
        .unwrap();
        assert_eq!(r.text, "Vaultkcol failed again");
        let r = parse_notice(&json!({"text": "a\u{0007}b\u{061C}c\u{E0041}d"})).unwrap();
        assert_eq!(r.text, "a bcd");
        for (c, name) in [
            ('\u{200E}', "LRM"),
            ('\u{2069}', "PDI"),
            ('\u{00AD}', "soft hyphen"),
        ] {
            let r = parse_notice(&json!({ "text": format!("x{c}y") })).unwrap();
            assert_eq!(r.text, "xy", "{name}");
        }
    }

    #[test]
    fn a_notice_that_is_too_long_blank_or_not_text_is_refused() {
        let most = "e".repeat(MAX_TEXT_CHARS);
        assert_eq!(
            parse_notice(&json!({ "text": most })).unwrap().text.len(),
            MAX_TEXT_CHARS
        );
        let long = "e".repeat(MAX_TEXT_CHARS + 1);
        let e = parse_notice(&json!({ "text": long })).unwrap_err();
        assert_eq!(e.code, code::BAD_PARAMS);
        assert!(e.message.contains("120"), "{}", e.message);
        for v in [
            json!({}),
            json!({"text": null}),
            json!({"text": ""}),
            json!({"text": " \n\t "}),
            json!({"text": "\u{200B}\u{202E}"}),
            json!({"text": 5}),
            json!({"text": ["a"]}),
            json!({"text": "a", "kind": "warning"}),
            json!({"text": "a", "kind": 1}),
            json!({"text": "a", "button": "Retry"}),
            json!("a"),
            json!(null),
        ] {
            assert_eq!(code_of(v.clone()), code::BAD_PARAMS, "{v}");
        }
        let e = parse_notice(&json!({"text": "a", "title": "x"})).unwrap_err();
        assert!(e.message.contains("title"), "{}", e.message);
    }

    #[test]
    fn three_notices_in_ten_seconds() {
        assert!((rate_per_second() - 0.3).abs() < 1e-9);
        assert_eq!(
            rate_message(),
            "at most 3 notices in 10 seconds for one plugin"
        );
        assert_eq!(SHOW_FOR, Duration::from_secs(6));
    }
}
