//! Pure: whether a program's output asks the terminal something that wants an answer, and that output without its
//! questions (rule 347).
//!
//! A warm spare (a shell started ahead of its tab, `SessionManager::prewarm`) has no emulator: what it prints is
//! kept for the tab that adopts it, and nobody answers its questions until then. A program that gave up waiting
//! (.NET's pwsh asks for the cursor at its first prompt and moves on) then reads the late answer as typed keys:
//! on the Mac runner a pwsh adopted a minute after it started took every cursor report as input, echoed it as
//! characters no key sends, asked again, and never stopped. The session manager therefore hands a tab the spare's
//! kept output without its questions (outside Windows, where ConPTY stands between the shell and Flare and nothing
//! changes): the stale question is never answered, and the spare is adopted as before.

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// True when `bytes` hold a question for the terminal: a device status report (`CSI n`, `CSI ? n`), a device
/// attributes request (`CSI c`, `CSI > c`, `CSI = c`), a mode request (`CSI ? Ps $ p`), a window report request
/// (`CSI 14 t`, `CSI 16 t`, `CSI 18 t`, `CSI 19 t`, `CSI 21 t`), the keyboard protocol query (`CSI ? u`), a colour
/// query (`OSC 4/10/11/12/17/19 ... ; ?`) or a capability or setting request (`DCS + q`, `DCS $ q`). A question
/// cut off at the end is not one yet.
pub fn asks_terminal(bytes: &[u8]) -> bool {
    !questions(bytes).is_empty()
}

/// `bytes` with every whole question (as `asks_terminal` reads them) taken out; everything else, a question cut
/// off at the end included, stays byte for byte.
pub fn without_questions(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    for (from, to) in questions(bytes) {
        out.extend_from_slice(&bytes[at..from]);
        at = to;
    }
    out.extend_from_slice(&bytes[at..]);
    out
}

/// Where the whole questions in `bytes` lie, as `(start, end)` with `end` exclusive, in order.
fn questions(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != ESC {
            i += 1;
            continue;
        }
        let (asks, next) = match bytes[i + 1] {
            b'[' => csi_asks(bytes, i + 2),
            b']' => osc_asks(bytes, i + 2),
            b'P' => dcs_asks(bytes, i + 2),
            _ => (false, i + 1),
        };
        if asks {
            found.push((i, next));
        }
        i = next.max(i + 1);
    }
    found
}

/// A CSI from `start` (just past `ESC [`): whether it is a whole question, and where it ends (exclusive).
fn csi_asks(bytes: &[u8], start: usize) -> (bool, usize) {
    let mut j = start;
    while j < bytes.len() && (0x30..=0x3f).contains(&bytes[j]) {
        j += 1;
    }
    let params = &bytes[start..j];
    let inter_start = j;
    while j < bytes.len() && (0x20..=0x2f).contains(&bytes[j]) {
        j += 1;
    }
    let inter = &bytes[inter_start..j];
    let Some(&fin) = bytes.get(j) else {
        return (false, j);
    };
    let asks = match fin {
        b'n' | b'c' => inter.is_empty(),
        b'p' => inter == b"$" && params.first() == Some(&b'?'),
        b't' => inter.is_empty() && matches!(params, b"14" | b"16" | b"18" | b"19" | b"21"),
        b'u' => inter.is_empty() && params == b"?",
        _ => false,
    };
    (asks, j + 1)
}

/// Where a string sequence from `start` ends (exclusive, past its BEL or ST), or `None` when it is cut off.
fn string_end(bytes: &[u8], start: usize) -> (usize, Option<usize>) {
    let mut j = start;
    while j < bytes.len() {
        if bytes[j] == BEL {
            return (j, Some(j + 1));
        }
        if bytes[j] == ESC && bytes.get(j + 1) == Some(&b'\\') {
            return (j, Some(j + 2));
        }
        j += 1;
    }
    (j, None)
}

/// An OSC from `start` (just past `ESC ]`): whether it is a whole colour query, and where scanning goes on.
fn osc_asks(bytes: &[u8], start: usize) -> (bool, usize) {
    let (body_end, end) = string_end(bytes, start);
    let Some(end) = end else {
        return (false, body_end);
    };
    let body = &bytes[start..body_end];
    let code_end = body.iter().position(|&b| b == b';').unwrap_or(body.len());
    let code = &body[..code_end];
    let asks =
        matches!(code, b"4" | b"10" | b"11" | b"12" | b"17" | b"19") && body.ends_with(b";?");
    (asks, end)
}

/// A DCS from `start` (just past `ESC P`): whether it is a whole capability or setting request.
fn dcs_asks(bytes: &[u8], start: usize) -> (bool, usize) {
    let body = &bytes[start..];
    if !(body.starts_with(b"+q") || body.starts_with(b"$q")) {
        return (false, start);
    }
    match string_end(bytes, start) {
        (_, Some(end)) => (true, end),
        (body_end, None) => (false, body_end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_report_request_and_the_device_questions_ask() {
        assert!(asks_terminal(b"PS /home> \x1b[6n"));
        assert!(asks_terminal(b"\x1b[5n"));
        assert!(asks_terminal(b"\x1b[?6n"));
        assert!(asks_terminal(b"\x1b[c"));
        assert!(asks_terminal(b"\x1b[0c"));
        assert!(asks_terminal(b"\x1b[>c"));
        assert!(asks_terminal(b"\x1b[?2031$p"));
        assert!(asks_terminal(b"\x1b[18t"));
        assert!(asks_terminal(b"\x1b[?u"));
        assert!(asks_terminal(b"\x1b]11;?\x07"));
        assert!(asks_terminal(b"\x1b]4;1;?\x1b\\"));
        assert!(asks_terminal(b"\x1bP+q544e\x1b\\"));
    }

    #[test]
    fn drawing_moving_and_setting_ask_nothing() {
        assert!(!asks_terminal(b"plain text\r\n"));
        assert!(!asks_terminal(
            b"\x1b[1;32mgreen\x1b[0m \x1b[2J\x1b[H\x1b[?25l\x1b[?2004h"
        ));
        assert!(!asks_terminal(
            b"\x1b]0;title\x07\x1b]11;#101010\x07\x1b]777;flare-agent;state=working\x07"
        ));
        assert!(!asks_terminal(b"\x1b[8;24;80t\x1b[>1u\x1b[=1;1u"));
        assert!(!asks_terminal(b"\x1b[?1049h\x1b[3J"));
    }

    #[test]
    fn a_question_cut_at_the_end_or_a_lone_escape_asks_nothing_and_never_panics() {
        assert!(!asks_terminal(b"\x1b"));
        assert!(!asks_terminal(b"\x1b["));
        assert!(!asks_terminal(b"\x1b[6"));
        assert!(!asks_terminal(b"\x1b]11;"));
        assert!(!asks_terminal(b"\x1bP"));
        assert!(!asks_terminal(b"\x1bP+q544e"));
    }

    #[test]
    fn without_questions_takes_out_only_the_questions() {
        assert_eq!(
            without_questions(b"PS /home> \x1b[6n\x1b[?2004h"),
            b"PS /home> \x1b[?2004h"
        );
        assert_eq!(
            without_questions(b"a\x1b]11;?\x07b\x1b]0;t\x07c\x1bP+q544e\x1b\\d\x1b[c\x1b[1;2H"),
            b"ab\x1b]0;t\x07cd\x1b[1;2H"
        );
        let plain = b"\x1b[38;5;1mred\x1b[0m\r\n\x1b[8;24;80t";
        assert_eq!(without_questions(plain), plain);
    }

    #[test]
    fn a_question_cut_at_the_end_stays_for_the_live_output_to_finish() {
        assert_eq!(without_questions(b"x\x1b[6"), b"x\x1b[6");
        assert_eq!(without_questions(b"x\x1b]11;?"), b"x\x1b]11;?");
        assert_eq!(without_questions(b"\x1b"), b"\x1b");
    }
}
