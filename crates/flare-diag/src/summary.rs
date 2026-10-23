//! M9 Task 17: the field cleaning of the diagnostics log (no typed text, no clipboard, no titles).
//!
//! Port of Prism Terminal's `core/main/diagSummary.ts` and the sink rules of `core/main/diagIpc.ts`.
//! Every string length counts UTF-16 units, as JavaScript does, so the caps agree with the oracle.

use serde_json::{Map, Value};

/// A string field's cap, in UTF-16 units.
pub const STRING_MAX: usize = 300;
/// A stack's cap, in UTF-16 units.
pub const STACK_MAX: usize = 2000;
/// Keys kept of an object (one level deep, and inside a record).
const MAX_KEYS: usize = 12;
/// Keys kept at the top of a line's fields.
const TOP_KEYS: usize = 40;
/// Items kept of a list.
const MAX_ITEMS: usize = 10;
/// IPC arguments kept.
const MAX_ARGS: usize = 4;
/// The one field name that takes the longer cap.
const STACK_KEY: &str = "stack";
/// The kinds only main writes itself, never through the sink.
const MAIN_ONLY_SLOW: &[&str] = &["fs-slow"];
/// The kinds the sink takes at every level (the agent indicator's, the crumbs, a frame over budget).
const SINK_KINDS: &[&str] = &[
    "crumb",
    "agent-hook",
    "agent-title",
    "agent-mark",
    "agent-restore",
    "agent-poll-failing",
    "frame-slow",
];
const DAY_MS: u64 = 86_400_000;
/// A time this far in the future is taken as the clock's own time, not as a stamp.
const FUTURE_SLACK_MS: u64 = 60_000;

/// UTF-16 units in a string, as JavaScript counts them.
fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

/// A string at most `max` UTF-16 units, a cut one ending in how much was cut. A cut never splits a
/// surrogate pair: it stops before the pair, so the kept part may be one unit short of `max`.
pub fn cap_string(s: &str, max: usize) -> String {
    let total = units(s);
    if total <= max {
        return s.to_owned();
    }
    let mut kept = 0usize;
    let mut end = 0usize;
    for ch in s.chars() {
        let w = ch.len_utf16();
        if kept + w > max {
            break;
        }
        kept += w;
        end += ch.len_utf8();
    }
    format!("{}...(+{})", &s[..end], total - kept)
}

/// One value as an IPC argument is logged: primitives as they are, a string capped, an array as `[n]`,
/// an object one level deep (its keys as a count below that).
pub fn summarise_value(v: &Value, depth: usize) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::String(s) => Value::String(cap_string(s, STRING_MAX)),
        Value::Number(_) | Value::Bool(_) => v.clone(),
        Value::Array(items) => Value::String(format!("[{}]", items.len())),
        Value::Object(map) => {
            if depth > 0 {
                return Value::String(format!("{{{}}}", map.len()));
            }
            Value::Object(
                map.iter()
                    .take(MAX_KEYS)
                    .map(|(k, x)| (k.clone(), summarise_value(x, depth + 1)))
                    .collect(),
            )
        }
    }
}

/// One IPC argument as the caller holds it. `Bytes` and `List` carry only the size, never the content.
#[derive(Debug, Clone, Copy)]
pub enum Arg<'a> {
    /// A JSON value (a number, a boolean, null, an object).
    Json(&'a Value),
    /// A string.
    Str(&'a str),
    /// A byte buffer of this many bytes.
    Bytes(usize),
    /// A list of this many items.
    List(usize),
    /// Anything else, by its type name.
    Other(&'static str),
}

/// The arguments of a call as the log holds them: the first four. An opaque channel (typed text, the
/// clipboard, audio) says only each argument's kind and size.
pub fn summarise_args(args: &[Arg], opaque: bool) -> Vec<Value> {
    args.iter()
        .take(MAX_ARGS)
        .map(|a| if opaque { opaque_arg(a) } else { plain_arg(a) })
        .collect()
}

fn plain_arg(a: &Arg) -> Value {
    match a {
        Arg::Json(v) => summarise_value(v, 0),
        Arg::Str(s) => Value::String(cap_string(s, STRING_MAX)),
        Arg::Bytes(n) => Value::String(format!("bytes:{n}")),
        Arg::List(n) => Value::String(format!("[{n}]")),
        Arg::Other(name) => Value::String((*name).to_owned()),
    }
}

fn opaque_arg(a: &Arg) -> Value {
    let text = match a {
        Arg::Str(s) => format!("string:{}", units(s)),
        Arg::Json(Value::String(s)) => format!("string:{}", units(s)),
        Arg::Json(Value::Number(_)) => "number".to_owned(),
        Arg::Json(Value::Bool(_)) => "boolean".to_owned(),
        Arg::Json(Value::Array(items)) => format!("[{}]", items.len()),
        Arg::Json(Value::Null | Value::Object(_)) => "object".to_owned(),
        Arg::Bytes(n) => format!("bytes:{n}"),
        Arg::List(n) => format!("[{n}]"),
        Arg::Other(name) => (*name).to_owned(),
    };
    Value::String(text)
}

/// Anything thrown, as `msg` and (when there is one) a longer `stack`. The caller reads the thrown value
/// into a message and an optional stack.
pub fn error_fields(msg: &str, stack: Option<&str>) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("msg".to_owned(), Value::String(cap_string(msg, STRING_MAX)));
    if let Some(st) = stack.filter(|s| !s.is_empty()) {
        out.insert("stack".to_owned(), Value::String(cap_string(st, STACK_MAX)));
    }
    out
}

/// A line's fields made safe to write: every string capped (a `stack` at its own cap), a list kept to
/// ten items, records inside a list cleaned the same way one level down, and anything at depth two or
/// more summarised.
pub fn clean_fields(fields: &Map<String, Value>, depth: usize) -> Map<String, Value> {
    let limit = if depth == 0 { TOP_KEYS } else { MAX_KEYS };
    let mut out = Map::new();
    for (k, v) in fields.iter().take(limit) {
        if let Some(c) = clean_value(k, v, depth) {
            out.insert(k.clone(), c);
        }
    }
    out
}

fn clean_value(key: &str, v: &Value, depth: usize) -> Option<Value> {
    match v {
        Value::Null => Some(Value::Null),
        Value::String(s) => {
            let max = if key == STACK_KEY {
                STACK_MAX
            } else {
                STRING_MAX
            };
            Some(Value::String(cap_string(s, max)))
        }
        Value::Number(_) | Value::Bool(_) => Some(v.clone()),
        _ if depth >= 2 => Some(summarise_value(v, 1)),
        Value::Array(items) => Some(Value::Array(
            items
                .iter()
                .take(MAX_ITEMS)
                .map(|x| clean_value(key, x, depth + 1).unwrap_or(Value::Null))
                .collect(),
        )),
        Value::Object(map) => Some(Value::Object(clean_fields(map, depth + 1))),
    }
}

/// A kind is lowercase letters and hyphens, two to thirty-one characters, starting with a letter.
pub fn kind_ok(kind: &str) -> bool {
    let b = kind.as_bytes();
    (2..=31).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || *c == b'-')
}

/// Whether the sink takes a line of this kind. `frame-task` is the high-rate line, kept for verbose only.
pub fn sink_allows(kind: &str, verbose: bool) -> bool {
    if !kind_ok(kind) {
        return false;
    }
    if kind == "frame-task" {
        return verbose;
    }
    if SINK_KINDS.contains(&kind) {
        return true;
    }
    kind.ends_with("-slow") && !MAIN_ONLY_SLOW.contains(&kind)
}

/// A line's time: a missing time (0), one more than a day before `now`, or one after `now` plus a minute
/// reads as `now`.
pub fn clamp_time(at_ms: u64, now_ms: u64) -> u64 {
    if at_ms == 0
        || at_ms.saturating_add(DAY_MS) < now_ms
        || at_ms > now_ms.saturating_add(FUTURE_SLACK_MS)
    {
        return now_ms;
    }
    at_ms
}

/// A mark carries no note: Flare's mark takes no text, so no typed text can reach the log through it (D12).
pub fn mark_fields() -> Map<String, Value> {
    Map::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 1_800_000_000_000;

    #[test]
    fn leaves_a_short_string_alone_and_cuts_a_long_one_saying_so() {
        assert_eq!(cap_string("abc", STRING_MAX), "abc");
        let cut = cap_string(&"x".repeat(STRING_MAX + 50), STRING_MAX);
        assert!(cut.ends_with("...(+50)"), "{cut}");
        assert_eq!(cut.len(), STRING_MAX + "...(+50)".len());
    }

    #[test]
    fn keeps_primitives_caps_strings_and_logs_an_array_as_its_length() {
        assert_eq!(summarise_value(&json!(5), 0), json!(5));
        assert_eq!(summarise_value(&json!(true), 0), json!(true));
        assert_eq!(summarise_value(&Value::Null, 0), Value::Null);
        assert_eq!(summarise_value(&json!([1, 2, 3]), 0), json!("[3]"));
        let long = summarise_value(&json!("y".repeat(400)), 0);
        assert!(long.as_str().expect("string").ends_with("(+100)"));
    }

    #[test]
    fn logs_bytes_by_size_never_by_content() {
        assert_eq!(
            summarise_args(&[Arg::Bytes(1234)], false),
            vec![json!("bytes:1234")]
        );
    }

    #[test]
    fn flattens_an_object_one_level_deeper_ones_as_their_key_count() {
        assert_eq!(
            summarise_value(&json!({"a": "x", "b": [1, 2], "c": {"d": 1, "e": 2}}), 0),
            json!({"a": "x", "b": "[2]", "c": "{2}"})
        );
    }

    #[test]
    fn stops_at_twelve_keys() {
        let big: Map<String, Value> = (0..20).map(|i| (format!("k{i}"), json!(i))).collect();
        let Value::Object(out) = summarise_value(&Value::Object(big), 0) else {
            panic!("an object")
        };
        assert_eq!(out.len(), MAX_KEYS);
    }

    #[test]
    fn summarises_the_first_four_arguments() {
        let obj = json!({"x": 1});
        let two = json!(2);
        let args = [
            Arg::Str("C:\\a"),
            Arg::Json(&two),
            Arg::List(1),
            Arg::Json(&obj),
            Arg::Str("dropped"),
        ];
        assert_eq!(
            summarise_args(&args, false),
            vec![json!("C:\\a"), json!(2), json!("[1]"), json!({"x": 1})]
        );
    }

    #[test]
    fn logs_an_opaque_channel_argument_as_its_kind_and_size_alone() {
        // Typed text and the clipboard are what somebody would not want written down.
        let args = [Arg::Str("id-1"), Arg::Str("secret typed text")];
        assert_eq!(
            summarise_args(&args, true),
            vec![json!("string:4"), json!("string:17")]
        );
    }

    #[test]
    fn takes_the_message_and_a_longer_stack_from_an_error() {
        let f = error_fields("nope", Some("frame one\nframe two"));
        assert_eq!(f.get("msg"), Some(&json!("nope")));
        assert_eq!(f.get("stack"), Some(&json!("frame one\nframe two")));
        assert_eq!(error_fields("nope", None).len(), 1);
    }

    #[test]
    fn reads_anything_thrown_a_string_or_a_plain_object() {
        assert_eq!(
            Value::Object(error_fields("bad", None)),
            json!({"msg": "bad"})
        );
        assert_eq!(
            Value::Object(error_fields("42", None)),
            json!({"msg": "42"})
        );
    }

    #[test]
    fn caps_a_stack_at_its_own_longer_cap() {
        let f = error_fields("x", Some(&"s".repeat(STACK_MAX * 2)));
        let stack = f.get("stack").and_then(Value::as_str).expect("stack");
        assert!(stack.ends_with("...(+2000)"), "{stack}");
        assert!(stack.len() <= STACK_MAX + 12);
    }

    #[test]
    fn caps_strings_keeps_small_lists_of_records_and_drops_what_json_cannot_say() {
        let fields = json!({
            "s": "z".repeat(400),
            "stack": "q".repeat(1000),
            "scripts": [{"src": "a", "ms": 1}],
            "n": null
        });
        let Value::Object(map) = fields else {
            panic!("an object")
        };
        let out = clean_fields(&map, 0);
        assert!(out["s"].as_str().expect("string").len() < 330);
        assert_eq!(out["stack"].as_str().expect("string").len(), 1000);
        assert_eq!(out["scripts"], json!([{"src": "a", "ms": 1}]));
        assert_eq!(out["n"], Value::Null);
    }

    #[test]
    fn keeps_at_most_ten_items_of_a_list() {
        let list: Vec<i32> = (0..30).collect();
        let Value::Object(map) = json!({"l": list}) else {
            panic!("an object")
        };
        let out = clean_fields(&map, 0);
        assert_eq!(out["l"].as_array().expect("array").len(), MAX_ITEMS);
    }

    #[test]
    fn takes_an_allowed_kind_and_refuses_one_only_main_may_write() {
        assert!(sink_allows("crumb", false));
        assert!(!sink_allows("session", false));
        assert!(!sink_allows("main-lag", false));
        assert!(!sink_allows("fs-slow", false));
        assert!(!sink_allows("logger-error", false));
        assert!(!sink_allows("panic", false));
        assert!(!sink_allows("Crumb", false));
    }

    #[test]
    fn takes_the_agent_kinds_at_the_quiet_level_and_no_other_agent_kind() {
        for k in ["agent-hook", "agent-title", "agent-mark", "agent-restore"] {
            assert!(sink_allows(k, false), "{k}");
        }
        // The poll's own failure: why agent marks went missing (review #103).
        assert!(sink_allows("agent-poll-failing", false));
        assert!(!sink_allows("agent-anything", false));
        assert!(!sink_allows("agent-bogus", false));
    }

    #[test]
    fn takes_an_apps_own_slow_kind() {
        assert!(sink_allows("sort-slow", false));
    }

    #[test]
    fn reads_a_time_that_is_missing_or_far_off_as_now() {
        let day = 86_400_000;
        assert_eq!(clamp_time(0, NOW), NOW);
        assert_eq!(clamp_time(NOW - 3 * day, NOW), NOW);
        assert_eq!(clamp_time(NOW - 300, NOW), NOW - 300);
        assert_eq!(clamp_time(NOW + 120_000, NOW), NOW);
    }

    #[test]
    fn keeps_the_high_rate_lines_for_verbose_only() {
        assert!(!sink_allows("frame-task", false));
        assert!(sink_allows("frame-task", true));
    }

    #[test]
    fn a_mark_carries_no_note() {
        assert!(mark_fields().is_empty());
    }

    #[test]
    fn a_string_with_an_emoji_at_the_cut_is_cut_before_the_pair() {
        // 299 letters, then an emoji (two UTF-16 units) and a z: 302 units. The cut at 300 would
        // split the pair, so the cut stops at 299 and says it cut three.
        let input = format!("{}\u{1F600}z", "a".repeat(299));
        assert_eq!(
            cap_string(&input, STRING_MAX),
            format!("{}...(+3)", "a".repeat(299))
        );
    }
}
