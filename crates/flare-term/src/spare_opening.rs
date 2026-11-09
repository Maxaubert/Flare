//! The cursor question a warm spare is asked as it opens (regression rule 231).
//!
//! ConPTY opens every shell by asking where the cursor is. A tab answers within milliseconds, but a warm spare
//! has no tab yet, so the question used to wait in its banner and be answered when a tab adopted it, long after
//! PowerShell 7 had reached its prompt. That late answer cost the first key sent afterwards (and a beep), so the
//! first paste into a new tab lost its first letter. The spare answers the question itself instead.

use std::ops::Range;

/// The cursor position question.
const CURSOR_QUERY: &[u8] = b"\x1b[6n";
/// The answer before anything has been drawn: the top left cell.
pub const OPENING_CURSOR_REPLY: &[u8] = b"\x1b[1;1R";

/// What the start of a spare's output says about the opening question.
#[derive(Debug, PartialEq, Eq)]
pub enum Opening {
    /// Only whole control sequences so far: the question may still come.
    Wait,
    /// Something else came first, so the cursor may have moved and only a real grid can answer.
    Over,
    /// The question is at this range.
    Asked(Range<usize>),
}

/// Reads the control sequences at the start of `banner`, looking for the cursor question.
pub fn scan_opening(banner: &[u8]) -> Opening {
    let mut at = 0;
    loop {
        match banner.get(at..at + 2) {
            None if banner.get(at).is_none_or(|b| *b == 0x1b) => return Opening::Wait,
            Some([0x1b, b'[']) => {}
            _ => return Opening::Over,
        }
        let params = banner[at + 2..]
            .iter()
            .take_while(|b| (0x20..=0x3f).contains(*b))
            .count();
        let last = at + 2 + params;
        match banner.get(last) {
            None => return Opening::Wait,
            Some(0x40..=0x7e) => {}
            Some(_) => return Opening::Over,
        }
        if &banner[at..=last] == CURSOR_QUERY {
            return Opening::Asked(at..last + 1);
        }
        at = last + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_is_found_behind_the_sequences_conpty_sends_first() {
        let b = b"\x1b[1t\x1b[6n\x1b[c\x1b[?1004h";
        assert_eq!(scan_opening(b), Opening::Asked(4..8));
    }

    #[test]
    fn output_that_stops_part_way_waits_for_the_rest() {
        for b in [
            &b""[..],
            b"\x1b",
            b"\x1b[",
            b"\x1b[6",
            b"\x1b[1t",
            b"\x1b[1t\x1b",
        ] {
            assert_eq!(scan_opening(b), Opening::Wait, "{b:?}");
        }
    }

    #[test]
    fn anything_drawn_before_the_question_ends_the_opening() {
        for b in [
            &b"PS C:\\>\x1b[6n"[..],
            b"\x1b[1tx\x1b[6n",
            b"\x1b]0;t\x07\x1b[6n",
            b"\x1bc\x1b[6n",
        ] {
            assert_eq!(scan_opening(b), Opening::Over, "{b:?}");
        }
    }

    #[test]
    fn a_broken_sequence_ends_the_opening() {
        assert_eq!(scan_opening(b"\x1b[1\x01\x1b[6n"), Opening::Over);
    }

    #[test]
    fn a_question_with_other_parameters_is_not_the_one_answered() {
        assert_eq!(scan_opening(b"\x1b[?6n"), Opening::Wait);
        assert_eq!(scan_opening(b"\x1b[5n"), Opening::Wait);
    }
}
