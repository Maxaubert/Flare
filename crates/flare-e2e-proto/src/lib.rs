//! The test hook's request and reply types, shared by the app and by tools/e2e.
//!
//! One JSON object per line, UTF-8, `\n`-terminated, request then reply in lockstep; a line is
//! at most 1 MiB. All `f32` positions are css pixels in window-client coordinates (origin top-left).

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The wire version. Bumped when a request or reply changes shape.
pub const PROTO_VERSION: u32 = 1;

/// The longest line `decode_request` and `decode_reply` accept.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PointerAction {
    Move,
    Down { button: Button },
    Up { button: Button },
    Click { button: Button, count: u8 },
    Wheel { dx: f32, dy: f32 },
    Leave,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapMode {
    Auto,
    Slint,
    PrintWindow,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Hello,
    /// One event; "Ctrl+Shift+T", "Enter", "KeyW", "F11".
    Key {
        chord: String,
        down: bool,
    },
    /// Down then up.
    KeyPress {
        chord: String,
    },
    /// Each char as a key press with its text, to the focused control.
    Text {
        text: String,
    },
    Pointer {
        x: f32,
        y: f32,
        action: PointerAction,
    },
    Drag {
        from: (f32, f32),
        to: (f32, f32),
        steps: u32,
        button: Button,
    },
    /// Hovered-file then dropped-file, as winit does.
    DropFiles {
        paths: Vec<String>,
        x: f32,
        y: f32,
    },
    /// Logical client size.
    Resize {
        w: f32,
        h: f32,
    },
    /// 96 = 100 percent; sends WM_DPICHANGED. `x`, `y` = the new origin in device px (a monitor
    /// move), else unchanged.
    DpiChange {
        dpi: u32,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
    },
    /// The window's focus events, not the OS focus.
    Focus {
        on: bool,
    },
    Probe {
        name: String,
        #[serde(default)]
        arg: serde_json::Value,
    },
    Snapshot {
        path: String,
        #[serde(default)]
        mode: Option<SnapMode>,
    },
    /// Device pixels of the last snapshot's source.
    Pixel {
        x: u32,
        y: u32,
    },
    Frames {
        reset: bool,
    },
    Latency {
        chord: String,
        expect: String,
        tab: Option<String>,
        timeout_ms: u64,
    },
    /// B1: replies Ok at once; the app injects the key when the front grid's cursor row first
    /// ends with the suffix and records prompt_ms / usable_ms (Task 10).
    KeyAtPrompt {
        chord: String,
        prompt_suffix: String,
    },
    /// diagLog: hold the UI thread.
    BlockUi {
        ms: u64,
    },
    /// diagLog: run a named work item of `ms` through the work timer.
    Slow {
        name: String,
        ms: u64,
    },
    /// diagLog: a worker thread panics; the app survives.
    PanicWorker,
    /// "minimise" | "restore" | "maximise" | "fullscreen". The wire key is `what`, because
    /// `op` is the tag.
    Window {
        #[serde(rename = "what")]
        op: String,
    },
    /// Write the in-memory clipboard (only exists under --e2e).
    ClipSet {
        text: Option<String>,
        png_b64: Option<String>,
    },
    Quit {
        force: bool,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "re", rename_all = "snake_case")]
pub enum Reply {
    Ok,
    Err {
        message: String,
    },
    Value {
        value: serde_json::Value,
    },
    Hello {
        proto: u32,
        version: String,
        renderer: String,
        scale: f32,
        pid: u32,
    },
    Png {
        path: String,
        w: u32,
        h: u32,
    },
    Pixel {
        rgba: [u8; 4],
    },
    Latency {
        ms: f64,
    },
    Frames {
        count: u32,
        mean_ms: f64,
        p50_ms: f64,
        p95_ms: f64,
        p99_ms: f64,
        max_ms: f64,
        over_33: u32,
        render_p95_ms: f64,
    },
}

/// The named pipe for a profile: `\\.\pipe\flare-e2e-` plus 16 lower-case hex digits of a 64-bit
/// FNV-1a hash of the lower-cased, slash-normalised profile path. A name, not a secret: the
/// pipe's DACL is the security.
pub fn pipe_name(profile: &Path) -> String {
    let text = profile.to_string_lossy().replace('\\', "/").to_lowercase();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!(r"\\.\pipe\flare-e2e-{hash:016x}")
}

/// One request as a wire line, `\n`-terminated.
pub fn encode(r: &Request) -> String {
    let mut line = serde_json::to_string(r).unwrap_or_else(|_| String::from("{}"));
    line.push('\n');
    line
}

/// Parse one request line (a trailing `\n` or `\r\n` is allowed). Errors, never panics.
pub fn decode_request(line: &str) -> Result<Request, String> {
    decode_line(line)
}

/// One reply as a wire line, `\n`-terminated.
pub fn encode_reply(r: &Reply) -> String {
    let mut line = serde_json::to_string(r).unwrap_or_else(|_| String::from("{}"));
    line.push('\n');
    line
}

/// Parse one reply line. Errors, never panics.
pub fn decode_reply(line: &str) -> Result<Reply, String> {
    decode_line(line)
}

fn decode_line<T: serde::de::DeserializeOwned>(line: &str) -> Result<T, String> {
    if line.len() > MAX_LINE_BYTES {
        return Err(format!(
            "line is {} bytes, over the 1 MiB limit",
            line.len()
        ));
    }
    serde_json::from_str(line.trim_end_matches(['\n', '\r'])).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_requests() -> Vec<Request> {
        vec![
            Request::Hello,
            Request::Key {
                chord: "Ctrl+Shift+T".into(),
                down: true,
            },
            Request::KeyPress {
                chord: "Enter".into(),
            },
            Request::Text {
                text: "héllo".into(),
            },
            Request::Pointer {
                x: 10.5,
                y: 20.0,
                action: PointerAction::Click {
                    button: Button::Left,
                    count: 2,
                },
            },
            Request::Pointer {
                x: 1.0,
                y: 2.0,
                action: PointerAction::Wheel { dx: 0.0, dy: -3.0 },
            },
            Request::Pointer {
                x: 1.0,
                y: 2.0,
                action: PointerAction::Leave,
            },
            Request::Drag {
                from: (1.0, 2.0),
                to: (3.0, 4.0),
                steps: 8,
                button: Button::Right,
            },
            Request::DropFiles {
                paths: vec!["C:/a.txt".into()],
                x: 5.0,
                y: 6.0,
            },
            Request::Resize { w: 800.0, h: 600.0 },
            Request::DpiChange {
                dpi: 144,
                x: Some(-1920),
                y: Some(0),
            },
            Request::DpiChange {
                dpi: 96,
                x: None,
                y: None,
            },
            Request::Focus { on: false },
            Request::Probe {
                name: "chrome".into(),
                arg: serde_json::json!({"k": 1}),
            },
            Request::Snapshot {
                path: "C:/out.png".into(),
                mode: Some(SnapMode::PrintWindow),
            },
            Request::Snapshot {
                path: "C:/out.png".into(),
                mode: None,
            },
            Request::Pixel { x: 3, y: 4 },
            Request::Frames { reset: true },
            Request::Latency {
                chord: "KeyW".into(),
                expect: "w".into(),
                tab: Some("main".into()),
                timeout_ms: 500,
            },
            Request::KeyAtPrompt {
                chord: "Enter".into(),
                prompt_suffix: "$ ".into(),
            },
            Request::BlockUi { ms: 50 },
            Request::Slow {
                name: "scan".into(),
                ms: 20,
            },
            Request::PanicWorker,
            Request::Window {
                op: "maximise".into(),
            },
            Request::ClipSet {
                text: Some("hi".into()),
                png_b64: None,
            },
            Request::Quit { force: false },
        ]
    }

    fn sample_replies() -> Vec<Reply> {
        vec![
            Reply::Ok,
            Reply::Err {
                message: "nope".into(),
            },
            Reply::Value {
                value: serde_json::json!([1, "two"]),
            },
            Reply::Hello {
                proto: PROTO_VERSION,
                version: "0.1.0".into(),
                renderer: "gpu".into(),
                scale: 2.25,
                pid: 4242,
            },
            Reply::Png {
                path: "C:/x.png".into(),
                w: 640,
                h: 480,
            },
            Reply::Pixel {
                rgba: [1, 2, 3, 255],
            },
            Reply::Latency { ms: 12.5 },
            Reply::Frames {
                count: 3,
                mean_ms: 4.0,
                p50_ms: 3.5,
                p95_ms: 8.0,
                p99_ms: 9.0,
                max_ms: 11.0,
                over_33: 0,
                render_p95_ms: 2.0,
            },
        ]
    }

    #[test]
    fn every_request_round_trips() {
        for request in sample_requests() {
            let line = encode(&request);
            assert!(line.ends_with('\n'), "encode terminates the line: {line:?}");
            assert_eq!(decode_request(&line).expect("decodes"), request);
            assert_eq!(
                decode_request(line.trim_end()).expect("decodes bare"),
                request
            );
        }
    }

    #[test]
    fn every_reply_round_trips() {
        for reply in sample_replies() {
            let line = encode_reply(&reply);
            assert!(
                line.ends_with('\n'),
                "encode_reply terminates the line: {line:?}"
            );
            assert_eq!(decode_reply(&line).expect("decodes"), reply);
        }
    }

    #[test]
    fn an_unknown_op_is_an_error_not_a_panic() {
        assert!(decode_request(r#"{"op":"no_such_op"}"#).is_err());
        assert!(decode_request("not json at all").is_err());
        assert!(decode_request("").is_err());
        assert!(decode_reply(r#"{"re":"no_such_reply"}"#).is_err());
    }

    #[test]
    fn a_line_over_one_mib_is_refused() {
        let big = format!(
            r#"{{"op":"text","text":"{}"}}"#,
            "a".repeat(MAX_LINE_BYTES + 16)
        );
        assert!(big.len() > MAX_LINE_BYTES);
        assert!(decode_request(&big).is_err());
        // A line just under the limit still decodes.
        let fits = format!(r#"{{"op":"text","text":"{}"}}"#, "a".repeat(1000));
        assert!(decode_request(&fits).is_ok());
    }

    #[test]
    fn the_pipe_name_is_stable_and_case_and_slash_insensitive() {
        let a = pipe_name(&PathBuf::from(
            r"C:\Users\Admin\AppData\Local\Temp\Flare-E2E\p1",
        ));
        let b = pipe_name(&PathBuf::from(
            "c:/users/admin/appdata/local/temp/flare-e2e/P1",
        ));
        assert_eq!(a, b);
        assert_eq!(
            a,
            pipe_name(&PathBuf::from(
                r"C:\Users\Admin\AppData\Local\Temp\Flare-E2E\p1"
            ))
        );
        let prefix = r"\\.\pipe\flare-e2e-";
        assert!(a.starts_with(prefix), "{a}");
        let hex = &a[prefix.len()..];
        assert_eq!(hex.len(), 16, "{a}");
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{a}"
        );
    }

    #[test]
    fn two_profiles_get_two_names() {
        let one = pipe_name(&PathBuf::from(r"C:\tmp\profile-one"));
        let two = pipe_name(&PathBuf::from(r"C:\tmp\profile-two"));
        assert_ne!(one, two);
    }
}
