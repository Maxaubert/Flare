//! ClickGate, caret_click_allowed, caret_delta, arrow_keys (Prism Terminal's termClickCaret.ts).
//! A plain click on the prompt line moves the caret there, by sending arrow keys.
//! Positions are grid cells and char indices; a wide character counts once.

/// How the program in the pty asks for mouse reports (xterm's modes 1000 to 1003).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseTracking {
    None,
    X10,
    Vt200,
    Drag,
    Any,
}

/// Everything a click needs to know about the screen it lands on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClickGate {
    pub button: u8,
    pub detail: u32,
    pub modifiers: bool,
    pub dragged: bool,
    pub mouse_tracking: MouseTracking,
    pub alternate: bool,
    pub scrolled_back: bool,
    pub cursor_hidden: bool,
    pub text_below: bool,
    pub off_line: bool,
    pub on_link: bool,
}

impl ClickGate {
    /// Button 0, detail 1, everything else false or None: the plain click on the prompt line.
    pub fn plain() -> Self {
        Self {
            button: 0,
            detail: 1,
            modifiers: false,
            dragged: false,
            mouse_tracking: MouseTracking::None,
            alternate: false,
            scrolled_back: false,
            cursor_hidden: false,
            text_below: false,
            off_line: false,
            on_link: false,
        }
    }
}

/// True for a plain click that may move the caret: left button, single click, no modifier,
/// no drag, no program owning the mouse, not the alternate screen, not scrolled back, cursor
/// visible, nothing printed below the line, on the line and not on a link.
pub fn caret_click_allowed(g: &ClickGate) -> bool {
    g.button == 0
        && g.detail < 2
        && !g.modifiers
        && !g.dragged
        && g.mouse_tracking == MouseTracking::None
        && !g.alternate
        && !g.scrolled_back
        && !g.cursor_hidden
        && !g.text_below
        && !g.off_line
        && !g.on_link
}

/// The signed number of cells the caret moves from `cursor` to `target`: positive to the right,
/// negative to the left. Each character counts once (a wide character's spacer does not), and
/// a target past the text is the end of the text.
pub fn caret_delta(widths: &[u8], cursor: usize, target: usize, text_end: usize) -> i32 {
    let start = |i: usize| {
        let mut j = i;
        while j > 0 && widths.get(j) == Some(&0) {
            j -= 1;
        }
        j
    };
    let from = start(cursor);
    let to = start(target.min(cursor.max(text_end)));
    if from == to {
        return 0;
    }
    let (a, b) = if from < to { (from, to) } else { (to, from) };
    let n = (a..b)
        .filter(|&i| widths.get(i).copied().unwrap_or(1) != 0)
        .count();
    let n = i32::try_from(n).unwrap_or(i32::MAX);
    if from < to { n } else { -n }
}

/// The arrow keys that move the caret `delta` cells: Right for positive, Left for negative,
/// in the application form (ESC O) when the program asked for it. Zero sends nothing.
pub fn arrow_keys(delta: i32, application_cursor: bool) -> String {
    let (key, count) = if delta > 0 {
        (
            if application_cursor {
                "\x1bOC"
            } else {
                "\x1b[C"
            },
            delta,
        )
    } else if delta < 0 {
        (
            if application_cursor {
                "\x1bOD"
            } else {
                "\x1b[D"
            },
            -delta,
        )
    } else {
        return String::new();
    };
    key.repeat(usize::try_from(count).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ones(n: usize) -> Vec<u8> {
        vec![1u8; n]
    }

    #[test]
    fn caret_click_plain_click_is_allowed() {
        assert!(caret_click_allowed(&ClickGate::plain()));
    }

    #[test]
    fn caret_click_refuses_a_right_click() {
        let g = ClickGate {
            button: 2,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_click_refuses_a_double_click() {
        let g = ClickGate {
            detail: 2,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_click_refuses_a_modified_click() {
        let g = ClickGate {
            modifiers: true,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_click_refuses_a_drag_or_selection() {
        let g = ClickGate {
            dragged: true,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_click_refuses_a_program_that_owns_the_mouse() {
        let g = ClickGate {
            mouse_tracking: MouseTracking::Vt200,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_click_refuses_the_alternate_screen() {
        let g = ClickGate {
            alternate: true,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_click_refuses_a_view_scrolled_back() {
        let g = ClickGate {
            scrolled_back: true,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_click_refuses_a_hidden_cursor() {
        let g = ClickGate {
            cursor_hidden: true,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_click_refuses_a_program_printing_below_the_line() {
        let g = ClickGate {
            text_below: true,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_click_refuses_a_click_off_the_line() {
        let g = ClickGate {
            off_line: true,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_click_refuses_a_click_on_a_link() {
        let g = ClickGate {
            on_link: true,
            ..ClickGate::plain()
        };
        assert!(!caret_click_allowed(&g));
    }

    #[test]
    fn caret_delta_counts_both_ways() {
        assert_eq!(caret_delta(&ones(20), 12, 5, 12), -7);
        assert_eq!(caret_delta(&ones(20), 5, 9, 12), 4);
        assert_eq!(caret_delta(&ones(20), 5, 5, 12), 0);
    }

    #[test]
    fn caret_delta_wide_counts_once() {
        let w = [1u8, 2, 0, 1];
        assert_eq!(caret_delta(&w, 4, 0, 4), -3);
        assert_eq!(caret_delta(&w, 4, 2, 4), -2);
        assert_eq!(caret_delta(&w, 0, 3, 4), 2);
    }

    #[test]
    fn caret_delta_crosses_wraps() {
        assert_eq!(caret_delta(&ones(20), 15, 3, 15), -12);
    }

    #[test]
    fn caret_delta_past_text_is_the_end() {
        assert_eq!(caret_delta(&ones(80), 3, 70, 10), 7);
        assert_eq!(caret_delta(&ones(80), 11, 70, 10), 0);
    }

    #[test]
    fn arrow_keys_right_left_application() {
        assert_eq!(arrow_keys(3, false), "\x1b[C\x1b[C\x1b[C");
        assert_eq!(arrow_keys(-2, false), "\x1b[D\x1b[D");
        assert_eq!(arrow_keys(1, true), "\x1bOC");
        assert_eq!(arrow_keys(0, false), "");
    }
}
