//! CaretHoldState and rules (Prism Terminal's termCaretHold.ts). After a key the caret is held
//! on the input line for a short window, so a streaming frame that parks the cursor above the
//! prompt does not pull the caret away. Times are in milliseconds.

/// A caret position: the line and the column (cells).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaretPos {
    pub line: i64,
    pub x: i64,
}

/// The hold state: the lowest caret seen since the last key, and when the window closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaretHoldState {
    pub caret: Option<CaretPos>,
    pub window_until: i64,
}

/// How long after a key the caret hold applies.
pub const KEY_WINDOW_MS: i64 = 250;

/// Nothing typed yet: no caret, no window.
pub fn initial_caret_hold() -> CaretHoldState {
    CaretHoldState {
        caret: None,
        window_until: 0,
    }
}

/// A key opens a fresh window and forgets the caret.
pub fn on_caret_key(_s: &CaretHoldState, now: i64) -> CaretHoldState {
    CaretHoldState {
        caret: None,
        window_until: now + KEY_WINDOW_MS,
    }
}

/// The caret moved to `at` at time `now`. Inside the window the lowest position wins; outside
/// it a caret above the held one is held back, and anything at or below re-anchors. Returns the
/// new state and the position to hold the caret on, if any.
pub fn on_caret_move(
    s: &CaretHoldState,
    at: CaretPos,
    now: i64,
) -> (CaretHoldState, Option<CaretPos>) {
    if now <= s.window_until {
        let caret = match s.caret {
            Some(c) if at.line < c.line => c,
            _ => at,
        };
        let hold = if caret == at { None } else { Some(caret) };
        return (
            CaretHoldState {
                caret: Some(caret),
                window_until: s.window_until,
            },
            hold,
        );
    }
    match s.caret {
        None => (*s, None),
        Some(c) if at.line < c.line => (*s, Some(c)),
        Some(_) => (
            CaretHoldState {
                caret: Some(at),
                window_until: s.window_until,
            },
            None,
        ),
    }
}

/// Lets the hold go: back to the initial state.
pub fn release_caret_hold() -> CaretHoldState {
    initial_caret_hold()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: i64, x: i64) -> CaretPos {
        CaretPos { line, x }
    }

    #[test]
    fn caret_hold_follows_before_typing() {
        let (_, hold) = on_caret_move(&initial_caret_hold(), pos(10, 3), 1000);
        assert_eq!(hold, None);
    }

    #[test]
    fn caret_hold_holds_on_the_input_line() {
        let init = initial_caret_hold();
        let mut s = on_caret_key(&init, 1000);
        s = on_caret_move(&s, pos(24, 4), 1010).0;
        let frame = on_caret_move(&s, pos(10, 3), 1000 + KEY_WINDOW_MS + 50);
        assert_eq!(frame.1, Some(pos(24, 4)));
        assert_eq!(
            on_caret_move(&frame.0, pos(10, 3), 2000).1,
            Some(pos(24, 4))
        );
    }

    #[test]
    fn caret_hold_takes_the_lowest_position() {
        let mut s = on_caret_key(&initial_caret_hold(), 1000);
        s = on_caret_move(&s, pos(24, 5), 1010).0;
        let r = on_caret_move(&s, pos(10, 3), 1050);
        assert_eq!(r.1, Some(pos(24, 5)));
        assert_eq!(r.0.caret, Some(pos(24, 5)));
    }

    #[test]
    fn caret_hold_follows_at_or_below() {
        let mut s = on_caret_key(&initial_caret_hold(), 1000);
        s = on_caret_move(&s, pos(24, 4), 1010).0;
        let down = on_caret_move(&s, pos(25, 0), 2000);
        assert_eq!(down.1, None);
        assert_eq!(down.0.caret, Some(pos(25, 0)));
        assert_eq!(on_caret_move(&down.0, pos(25, 9), 2100).1, None);
    }

    #[test]
    fn caret_hold_new_key_reanchors() {
        let mut s = on_caret_key(&initial_caret_hold(), 1000);
        s = on_caret_move(&s, pos(24, 4), 1010).0;
        s = on_caret_key(&s, 3000);
        let up = on_caret_move(&s, pos(23, 2), 3010);
        assert_eq!(up.1, None);
        assert_eq!(up.0.caret, Some(pos(23, 2)));
    }
}
