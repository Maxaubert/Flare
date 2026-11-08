//! The OSC 7501 program-status parser (agy, owner decision 2026-10-26). A program reports its state as
//! `OSC 7501 ; state=<word>[:kind=<word>][:app=<id>][:msg=<base64>] ST` (key order free, unknown keys
//! ignored), and `state=clear` takes its records away. Grammar and limits:
//! https://www.superlogical.com/rex/docs/build/program-status (read 2026-10-26). The scanner in
//! `flare-term` joins a sequence split across chunks and takes either terminator (ESC \ or BEL); this
//! file sees the payload after `7501;`.
//!
//! Flare keeps only the root record (no `id`): a record with an `id` is a part of the work (one
//! deploy region, one subagent), and its "done" is not the program's. The feature query (`?`) is not
//! a report; Flare turns the reports on through `AGY_CLI_OSC7501=on` instead of answering it.

use base64::Engine as _;

/// The longest sequence the spec allows; longer is refused whole.
const MAX_LEN: usize = 4096;
/// The longest key.
const MAX_KEY: usize = 16;
/// The longest `msg`, encoded and decoded.
const MAX_MSG_ENCODED: usize = 2732;
const MAX_MSG_DECODED: usize = 2048;
/// The longest `app`.
const MAX_APP: usize = 32;

/// The state a program reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    /// Waiting for the person: a permission, a question or a sign-in.
    Blocked,
    Error,
}

impl ProgramState {
    /// The word on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            ProgramState::Idle => "idle",
            ProgramState::Working => "working",
            ProgramState::Done => "done",
            ProgramState::Blocked => "blocked",
            ProgramState::Error => "error",
        }
    }
}

/// What a blocked program waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}

impl BlockedKind {
    /// The word on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            BlockedKind::Permission => "permission",
            BlockedKind::Question => "question",
            BlockedKind::Auth => "auth",
        }
    }
}

impl ProgramReport {
    /// What the log may say about a report beyond its state: the kind, the app and the message's
    /// length. Never the message itself: a program's own text can carry the person's work.
    pub fn log_note(&self) -> String {
        let mut parts = Vec::new();
        if self.state == ProgramState::Blocked {
            parts.push(format!(
                "kind {}",
                self.kind.map_or("none", BlockedKind::as_str)
            ));
        }
        if let Some(app) = &self.app {
            parts.push(format!("app {app}"));
        }
        if let Some(msg) = &self.msg {
            parts.push(format!("msg {} chars", msg.chars().count()));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        }
    }
}

/// One root-record report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramReport {
    pub state: ProgramState,
    /// Only with `Blocked`; an unknown kind is dropped, the state stands.
    pub kind: Option<BlockedKind>,
    /// The program's own stable name (`[A-Za-z0-9_.+-]{1,32}`), when it gave a valid one.
    pub app: Option<String>,
    /// The one human line, decoded, only when it is clean text (no control or bidi character).
    pub msg: Option<String>,
}

/// One OSC 7501 payload, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramStatus {
    Report(ProgramReport),
    /// `state=clear` with no `id`: every record of the terminal goes.
    Clear,
}

fn is_value(v: &str) -> bool {
    v.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
}

fn is_app(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= MAX_APP
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
}

/// A character that must never reach the screen from a program's text: a control, or a bidi
/// override or isolate that could reorder what is shown around it.
fn is_unclean(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Standard base64 of UTF-8 text, padding optional, clean text only.
fn decode_msg(v: &str) -> Option<String> {
    if v.is_empty() || v.len() > MAX_MSG_ENCODED {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(v.trim_end_matches('='))
        .ok()?;
    if bytes.len() > MAX_MSG_DECODED {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    if text.chars().any(is_unclean) {
        return None;
    }
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// One OSC 7501 payload (everything after `7501;`). None: not a report Flare reads (the feature
/// query, a broken pair, an unknown state, a record with an `id`, too long).
pub fn parse_program_status(data: &str) -> Option<ProgramStatus> {
    if data.len() > MAX_LEN || data.starts_with('?') {
        return None;
    }
    let mut state: Option<&str> = None;
    let mut kind: Option<&str> = None;
    let mut app: Option<&str> = None;
    let mut msg: Option<&str> = None;
    for pair in data.split(':') {
        let (key, value) = pair.split_once('=')?;
        if key.is_empty()
            || key.len() > MAX_KEY
            || !key.bytes().all(|b| b.is_ascii_lowercase())
            || !is_value(value)
        {
            return None;
        }
        match key {
            "state" => state = Some(value),
            "kind" => kind = Some(value),
            "app" => app = Some(value),
            "msg" => msg = Some(value),
            // A part of the work, not the program: Flare reads the root record only.
            "id" => return None,
            _ => {}
        }
    }
    let state = match state? {
        "idle" => ProgramState::Idle,
        "working" => ProgramState::Working,
        "done" => ProgramState::Done,
        "blocked" => ProgramState::Blocked,
        "error" => ProgramState::Error,
        "clear" => return Some(ProgramStatus::Clear),
        _ => return None,
    };
    let kind = match (state, kind) {
        (ProgramState::Blocked, Some("permission")) => Some(BlockedKind::Permission),
        (ProgramState::Blocked, Some("question")) => Some(BlockedKind::Question),
        (ProgramState::Blocked, Some("auth")) => Some(BlockedKind::Auth),
        _ => None,
    };
    Some(ProgramStatus::Report(ProgramReport {
        state,
        kind,
        app: app.filter(|a| is_app(a)).map(str::to_string),
        msg: msg.and_then(decode_msg),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(data: &str) -> ProgramReport {
        match parse_program_status(data) {
            Some(ProgramStatus::Report(r)) => r,
            other => panic!("{data:?} gave {other:?}"),
        }
    }

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn every_state_is_read() {
        for (word, state) in [
            ("idle", ProgramState::Idle),
            ("working", ProgramState::Working),
            ("done", ProgramState::Done),
            ("blocked", ProgramState::Blocked),
            ("error", ProgramState::Error),
        ] {
            let r = report(&format!("state={word}"));
            assert_eq!(r.state, state, "{word}");
            assert_eq!(r.state.as_str(), word);
            assert_eq!((r.kind, r.app, r.msg), (None, None, None));
        }
        assert_eq!(
            parse_program_status("state=clear"),
            Some(ProgramStatus::Clear)
        );
    }

    #[test]
    fn every_kind_is_read_with_blocked_only() {
        for (word, kind) in [
            ("permission", BlockedKind::Permission),
            ("question", BlockedKind::Question),
            ("auth", BlockedKind::Auth),
        ] {
            assert_eq!(
                report(&format!("state=blocked:kind={word}")).kind,
                Some(kind)
            );
            assert_eq!(report(&format!("state=working:kind={word}")).kind, None);
        }
        let r = report("state=blocked:kind=coffee");
        assert_eq!((r.state, r.kind), (ProgramState::Blocked, None));
    }

    #[test]
    fn key_order_is_free_and_unknown_keys_are_ignored() {
        let msg = b64("Run git status?");
        let a = report(&format!("state=blocked:kind=permission:app=agy:msg={msg}"));
        let b = report(&format!(
            "msg={msg}:progress=40:app=agy:title={}:kind=permission:state=blocked",
            b64("x")
        ));
        assert_eq!(a, b);
        assert_eq!(a.app.as_deref(), Some("agy"));
        assert_eq!(a.msg.as_deref(), Some("Run git status?"));
    }

    #[test]
    fn a_broken_or_unknown_report_is_none() {
        for data in [
            "",
            "?",
            "?:states=idle,working",
            "state=sleeping",
            "kind=question",
            "state=working:junk",
            "state=working::app=agy",
            "=working",
            "State=working",
            "state=working:app=a;b",
            "state=work ing",
            "state=working:id=us-east",
            "state=clear:id=us-east",
            "averyveryverylongkey=1:state=idle",
        ] {
            assert_eq!(parse_program_status(data), None, "{data:?}");
        }
        let long = format!("state=idle:x={}", "a".repeat(MAX_LEN));
        assert_eq!(parse_program_status(&long), None);
    }

    #[test]
    fn a_bad_message_is_dropped_and_the_state_stands() {
        for msg in [
            "a,b.".to_string(),
            "a".to_string(),
            b64("bell\x07"),
            b64("esc\x1b[2J"),
            b64("line\nbreak"),
            b64("rtl\u{202e}txt"),
            b64("   "),
            base64::engine::general_purpose::STANDARD.encode([0xff, 0xfe, 0x00]),
            b64(&"x".repeat(MAX_MSG_DECODED + 1)),
        ] {
            let r = report(&format!("state=working:msg={msg}"));
            assert_eq!(r.state, ProgramState::Working, "{msg}");
            assert_eq!(r.msg, None, "{msg}");
        }
        // Padding is optional.
        let bare = b64("Upgraded 12 packages")
            .trim_end_matches('=')
            .to_string();
        assert_eq!(
            report(&format!("state=done:msg={bare}")).msg.as_deref(),
            Some("Upgraded 12 packages")
        );
        assert_eq!(
            report("state=done:msg=VXBncmFkZWQgMTIgcGFja2FnZXM=")
                .msg
                .as_deref(),
            Some("Upgraded 12 packages")
        );
        assert_eq!(
            report(&format!("state=done:msg={}", b64("héllo")))
                .msg
                .as_deref(),
            Some("héllo")
        );
    }

    #[test]
    fn a_bad_app_is_dropped_and_the_state_stands() {
        for app in ["", "a/b", "a=b", &"x".repeat(MAX_APP + 1)] {
            let r = report(&format!("state=idle:app={app}"));
            assert_eq!((r.state, r.app), (ProgramState::Idle, None), "{app:?}");
        }
        assert_eq!(
            report("state=idle:app=claude-code").app.as_deref(),
            Some("claude-code")
        );
    }
}
