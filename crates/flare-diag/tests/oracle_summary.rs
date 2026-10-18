//! M9 Task 17: the oracle. Prism Terminal's own answers (`fixtures/summary.json`, `fixtures/gate.json`,
//! written by `tools/oracle-update`) must be agreed with, never edited to match Flare.
//!
//! Two encodings are read here. A value JSON cannot hold is written as a marker object (`{"number":"NaN"}`,
//! `{"undefined":true}`, `{"$bytes":n}` and so on). A `null` output means Prism Terminal's cut left a lone
//! UTF-16 surrogate, which JSON cannot hold: the port never leaves one, so such a vector is checked against
//! the pair-safe cut written out below.
//!
//! serde_json's `Map` sorts keys unless the `preserve_order` feature is on. The vectors whose answer depends
//! on key order (the first twelve keys, the first forty) are skipped while it is off, and the test says so.

use flare_diag::gate::{DEFAULT_BUDGET, DEFAULT_EVERY_MS, DiagGate};
use flare_diag::summary::{self, Arg, STRING_MAX};
use serde_json::{Value, json};

const SUMMARY: &str = include_str!("fixtures/summary.json");
const GATE: &str = include_str!("fixtures/gate.json");

/// True when serde_json keeps the order keys were written in (the `preserve_order` feature).
fn order_kept() -> bool {
    let v: Value = serde_json::from_str(r#"{"b":1,"a":2}"#).expect("json");
    serde_json::to_string(&v)
        .expect("json")
        .starts_with(r#"{"b""#)
}

/// True when some object in the value has more keys than a cut keeps, so its answer depends on order.
fn order_sensitive(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.len() > 12 || m.values().any(order_sensitive),
        Value::Array(items) => items.iter().any(order_sensitive),
        _ => false,
    }
}

/// A fixture value as the port sees it: the marker encodings replaced by the value they stand for.
fn to_value(v: &Value) -> Value {
    if let Value::Object(m) = v {
        if m.len() == 1 {
            let (k, x) = m.iter().next().expect("one key");
            match (k.as_str(), x) {
                ("undefined", Value::Bool(true)) => return Value::Null,
                ("number", Value::String(_)) => return Value::Null, // NaN, Infinity: non-finite is null
                ("bigint", Value::String(s)) => return Value::String(s.clone()),
                ("fn", Value::Bool(true)) => return Value::String("function".into()),
                ("symbol", Value::String(_)) => return Value::String("symbol".into()),
                ("$bytes" | "$arraybuffer", Value::Number(n)) => {
                    return Value::String(format!("bytes:{n}"));
                }
                _ => {}
            }
        }
        return Value::Object(m.iter().map(|(k, x)| (k.clone(), to_value(x))).collect());
    }
    match v {
        Value::Array(items) => Value::Array(items.iter().map(to_value).collect()),
        other => other.clone(),
    }
}

/// An IPC argument as the port sees it.
fn arg_of(v: &Value) -> Arg<'_> {
    match v {
        Value::String(s) => Arg::Str(s),
        Value::Object(m) if m.contains_key("$bytes") => {
            Arg::Bytes(m["$bytes"].as_u64().expect("bytes") as usize)
        }
        Value::Array(items) => Arg::List(items.len()),
        other => Arg::Json(other),
    }
}

/// A thrown value as `(message, stack)`: a string, an Error (`$error`), a plain object, or anything else.
fn thrown_of(v: &Value) -> (String, Option<String>) {
    match v {
        Value::String(s) => (s.clone(), None),
        Value::Object(m) if m.contains_key("$error") => {
            let e = &m["$error"];
            (
                e["message"].as_str().expect("message").into(),
                e.get("stack").and_then(Value::as_str).map(String::from),
            )
        }
        Value::Object(m) => (
            m["message"].as_str().expect("message").into(),
            m.get("stack").and_then(Value::as_str).map(String::from),
        ),
        Value::Number(n) => (n.to_string(), None),
        Value::Bool(b) => (b.to_string(), None),
        Value::Null => ("null".into(), None),
        Value::Array(_) => panic!("no array is thrown in the fixtures"),
    }
}

/// What a cut at `max` units reads as when it would split a surrogate pair: one unit shorter.
fn pair_safe_cut(input: &str, max: usize) -> String {
    let units: Vec<u16> = input.encode_utf16().collect();
    let mut kept = units.len().min(max);
    if kept > 0 && (0xD800..=0xDBFF).contains(&units[kept - 1]) && kept < units.len() {
        kept -= 1;
    }
    let prefix = String::from_utf16(&units[..kept]).expect("no lone surrogate kept");
    format!("{prefix}...(+{})", units.len() - kept)
}

fn with_repeats(line: Value, repeats: Option<u32>) -> Value {
    let mut m = line.as_object().cloned().expect("a line is an object");
    if let Some(n) = repeats {
        m.insert("repeats".into(), json!(n));
    }
    Value::Object(m)
}

#[test]
fn the_oracle_agrees_on_every_summary_vector() {
    let fx: Value = serde_json::from_str(SUMMARY).expect("summary.json");
    let ordered = order_kept();
    let (mut checked, mut skipped) = (0usize, 0usize);
    for (name, cases) in fx.as_object().expect("object") {
        for case in cases.as_array().expect("array") {
            let raw = &case["input"];
            let expected = &case["output"];
            if !ordered && order_sensitive(raw) {
                skipped += 1;
                continue;
            }
            let actual = match name.as_str() {
                "capString" => {
                    let s = raw.as_str().expect("string");
                    if expected.is_null() {
                        assert_eq!(
                            summary::cap_string(s, STRING_MAX),
                            pair_safe_cut(s, STRING_MAX),
                            "{name}"
                        );
                        checked += 1;
                        continue;
                    }
                    Value::String(summary::cap_string(s, STRING_MAX))
                }
                "summariseValue" => {
                    let input = to_value(raw);
                    if expected.is_null() {
                        if let Value::String(s) = raw {
                            assert_eq!(
                                summary::summarise_value(&input, 0),
                                Value::String(pair_safe_cut(s, STRING_MAX)),
                                "{name}"
                            );
                            checked += 1;
                            continue;
                        }
                    }
                    summary::summarise_value(&input, 0)
                }
                "summariseArgs" => {
                    let args_raw = raw["args"].as_array().expect("args");
                    let args: Vec<Arg> = args_raw.iter().map(arg_of).collect();
                    Value::Array(summary::summarise_args(
                        &args,
                        raw["opaque"].as_bool().expect("opaque"),
                    ))
                }
                "errorFields" => {
                    let (msg, stack) = thrown_of(raw);
                    Value::Object(summary::error_fields(&msg, stack.as_deref()))
                }
                "cleanFields" => {
                    let Value::Object(map) = to_value(raw) else {
                        panic!("an object")
                    };
                    let out = Value::Object(summary::clean_fields(&map, 0));
                    if expected.is_null() {
                        let text = raw["text"].as_str().expect("text");
                        assert_eq!(
                            out["text"],
                            Value::String(pair_safe_cut(text, STRING_MAX)),
                            "{name}"
                        );
                        checked += 1;
                        continue;
                    }
                    out
                }
                other => panic!("unknown vector kind {other}"),
            };
            assert_eq!(&actual, expected, "{name}: input {raw}");
            checked += 1;
        }
    }
    assert!(checked > 0);
    if skipped > 0 {
        eprintln!(
            "oracle_summary: {skipped} key-order vectors skipped; serde_json's Map is sorted here, enable the \
             preserve_order feature to check them"
        );
    }
}

#[test]
fn the_oracle_agrees_on_60_gate_sequences() {
    let seqs: Vec<Value> = serde_json::from_str(GATE).expect("gate.json");
    assert_eq!(seqs.len(), 60);
    for (si, seq) in seqs.iter().enumerate() {
        let every = seq["everyMs"].as_i64().expect("everyMs");
        let budget = u32::try_from(seq["budget"].as_u64().expect("budget")).expect("budget");
        let mut gate = DiagGate::<Value>::new(every, budget);
        let ops = seq["ops"].as_array().expect("ops");
        let answers = seq["answers"].as_array().expect("answers");
        assert_eq!(ops.len(), answers.len(), "sequence {si}");
        for (i, op) in ops.iter().enumerate() {
            let now = op["now"].as_i64().expect("now");
            let got = match op["op"].as_str().expect("op") {
                "sweep" => Value::Array(
                    gate.sweep(now)
                        .into_iter()
                        .map(|(line, n)| with_repeats(line, Some(n)))
                        .collect(),
                ),
                "offer" => {
                    let line = op["line"].clone();
                    match gate.offer(
                        op["k"].as_str().expect("k"),
                        op["key"].as_str().expect("key"),
                        line,
                        now,
                    ) {
                        None => Value::Null,
                        Some((line, repeats)) => with_repeats(line, repeats),
                    }
                }
                other => panic!("unknown op {other}"),
            };
            assert_eq!(got, answers[i], "sequence {si} op {i}");
        }
    }
}

#[test]
fn the_port_uses_the_callers_defaults() {
    assert_eq!(DEFAULT_EVERY_MS, 10_000);
    assert_eq!(DEFAULT_BUDGET, 10);
}
