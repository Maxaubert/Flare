//! OSC 52 (Prism Terminal's termOsc52.ts, #176): a program may put text ON the clipboard,
//! never read it. A read is named so it is swallowed and never answered (rule 10).

/// The most text one OSC 52 may put on the clipboard, in UTF-16 units (1 MB).
pub const OSC52_MAX: usize = 1_048_576;

/// What an OSC 52 asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Osc52 {
    /// Put this text on the clipboard.
    Write(String),
    /// A read: swallowed, never answered.
    Read,
}

/// Parses the payload after `52;` ("Pc;Pd"). `None` for anything Flare refuses.
pub fn parse_osc52(data: &str, max: usize) -> Option<Osc52> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    let (pc, pd) = data.split_once(';')?;
    if !pc.is_empty() && pc != "c" {
        return None;
    }
    if pd == "?" {
        return Some(Osc52::Read);
    }
    if pd.is_empty() || pd.len() % 4 != 0 || !is_base64_shape(pd) {
        return None;
    }
    // Every UTF-16 unit is at most 3 UTF-8 bytes; base64 makes that 4 characters per 3 bytes,
    // so anything longer than this cannot decode to `max` units. Refused unread.
    if pd.len() > max * 4 + 4 {
        return None;
    }
    let bytes = STANDARD.decode(pd).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    if text.is_empty() || text.encode_utf16().count() > max {
        return None;
    }
    Some(Osc52::Write(text))
}

/// `[A-Za-z0-9+/]*` followed by at most two `=`.
fn is_base64_shape(s: &str) -> bool {
    let body = s.trim_end_matches('=');
    if s.len() - body.len() > 2 {
        return false;
    }
    body.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    const MAX: usize = OSC52_MAX;

    fn b64(s: &str) -> String {
        STANDARD.encode(s.as_bytes())
    }

    fn write(s: &str) -> Option<Osc52> {
        Some(Osc52::Write(s.to_string()))
    }

    #[test]
    fn osc52_decodes_a_c_write() {
        assert_eq!(parse_osc52("c;aGVsbG8=", MAX), write("hello"));
    }

    #[test]
    fn osc52_empty_selection_is_the_clipboard() {
        assert_eq!(parse_osc52(";aGVsbG8=", MAX), write("hello"));
    }

    #[test]
    fn osc52_refuses_other_selections() {
        for bad in ["p;aGVsbG8=", "cs;aGVsbG8=", "s0;aGVsbG8="] {
            assert_eq!(parse_osc52(bad, MAX), None, "{bad}");
        }
    }

    #[test]
    fn osc52_names_a_read() {
        assert_eq!(parse_osc52("c;?", MAX), Some(Osc52::Read));
        assert_eq!(parse_osc52(";?", MAX), Some(Osc52::Read));
    }

    #[test]
    fn osc52_ignores_a_clear() {
        assert_eq!(parse_osc52("c;", MAX), None);
        assert_eq!(parse_osc52("c", MAX), None);
    }

    #[test]
    fn osc52_refuses_non_base64() {
        for bad in ["c;not base64!", "c;aGVsbG8", "c;a=GV"] {
            assert_eq!(parse_osc52(bad, MAX), None, "{bad}");
        }
    }

    #[test]
    fn osc52_refuses_invalid_utf8() {
        let data = format!("c;{}", STANDARD.encode([0xff, 0xfe, 0x41]));
        assert_eq!(parse_osc52(&data, MAX), None);
    }

    #[test]
    fn osc52_round_trips_utf8() {
        let text = "\u{e6}\u{f8}\u{e5} OSC52 \u{e6}";
        let data = format!("c;{}", b64(text));
        assert_eq!(parse_osc52(&data, MAX), write(text));
    }

    #[test]
    fn osc52_cap_is_exact() {
        let ten = format!("c;{}", b64(&"x".repeat(10)));
        assert_eq!(parse_osc52(&ten, 10), write(&"x".repeat(10)));
        let eleven = format!("c;{}", b64(&"x".repeat(11)));
        assert_eq!(parse_osc52(&eleven, 10), None);
    }

    #[test]
    fn osc52_refuses_a_huge_encoding_unread() {
        let data = format!("c;{}", "A".repeat(4000));
        assert_eq!(parse_osc52(&data, 10), None);
    }

    #[test]
    fn osc52_max_is_one_megabyte() {
        assert_eq!(OSC52_MAX, 1_048_576);
    }
}
