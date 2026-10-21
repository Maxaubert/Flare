//! hook_line: the OSC 777 agent hook line, checked against flare-agent's parser (Task 15).
//!
//! The frame is the one `flare-agent::signal` reads: `ESC ] 777 ; flare-agent ; state=<word>
//! [; key=value ...] BEL`. The committed parser is the oracle; the plan's JSON body was never the
//! protocol (M6 `signal.rs`), so no JSON is written here.

use serde_json::Value;

/// The OSC 777 hook line a Claude hook writes for `state` (`working`, `question`, `done`,
/// `failed`), with the string fields of `payload` as extra `key=value` pairs (`kind` for a
/// failure). Returns the whole frame, ESC and BEL included.
pub fn hook_line(state: &str, payload: &Value) -> String {
    let mut data = format!("{};state={state}", flare_agent::signal::PREFIX);
    if let Some(fields) = payload.as_object() {
        for (key, value) in fields {
            if let Some(text) = value.as_str() {
                data.push_str(&format!(";{key}={text}"));
            }
        }
    }
    format!("\x1b]777;{data}\x07")
}
