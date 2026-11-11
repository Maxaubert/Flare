//! wheel_action.
//! The mouse wheel's meaning for the view, the alt screen and a program that owns the mouse.
//! Port of the rules in Prism Terminal's TerminalPanel wheel handler; positions are grid cells.

use crate::click_caret::MouseTracking;

/// Lines the view moves per wheel notch (tuned by eye in M10).
pub const WHEEL_LINES_PER_NOTCH: i32 = 3;

/// What a wheel turn does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WheelAction {
    /// Move the view; positive is toward older output.
    ScrollView(i32),
    /// Bytes to write to the pty.
    Bytes(Vec<u8>),
    None,
}

/// The terminal state the wheel rules read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WheelContext {
    pub mouse: MouseTracking,
    pub sgr_mouse: bool,
    pub alt_screen: bool,
    pub alternate_scroll: bool,
    pub app_cursor: bool,
}

/// `notches` > 0 is wheel up. `col` and `row` are the cell under the pointer.
pub fn wheel_action(notches: i32, ctx: &WheelContext, col: usize, row: usize) -> WheelAction {
    step_action(notches, WHEEL_LINES_PER_NOTCH, ctx, col, row)
}

/// A trackpad drag on macOS, in whole lines (`lines` > 0 is up), as Terminal.app scrolls
/// (rule 1129): the view moves `lines` lines, the alternate screen gets one arrow a line, and a
/// program that owns the mouse one wheel report a line.
pub fn wheel_lines_action(lines: i32, ctx: &WheelContext, col: usize, row: usize) -> WheelAction {
    step_action(lines, 1, ctx, col, row)
}

/// `steps` wheel steps of `lines_per_step` lines each: one mouse report a step, `lines_per_step`
/// arrows a step on the alternate screen, `lines_per_step` lines of view a step.
fn step_action(
    notches: i32,
    lines_per_step: i32,
    ctx: &WheelContext,
    col: usize,
    row: usize,
) -> WheelAction {
    if notches == 0 {
        return WheelAction::None;
    }
    let up = notches > 0;
    let count = notches.unsigned_abs() as usize;
    if ctx.mouse != MouseTracking::None {
        if ctx.sgr_mouse {
            let button = if up { 64 } else { 65 };
            let report = format!("\x1b[<{};{};{}M", button, col + 1, row + 1);
            return WheelAction::Bytes(report.repeat(count).into_bytes());
        }
        let button = if up { 96u8 } else { 97u8 };
        let mut one = vec![0x1b, b'[', b'M', button];
        if crate::mouse_report::push_coord(&mut one, col + 33).is_none()
            || crate::mouse_report::push_coord(&mut one, row + 33).is_none()
        {
            return WheelAction::None;
        }
        return WheelAction::Bytes(one.repeat(count));
    }
    if ctx.alt_screen {
        if !ctx.alternate_scroll {
            return WheelAction::None;
        }
        let letter = match (up, ctx.app_cursor) {
            (true, false) => "\x1b[A",
            (false, false) => "\x1b[B",
            (true, true) => "\x1bOA",
            (false, true) => "\x1bOB",
        };
        return WheelAction::Bytes(letter.repeat(lines_per_step as usize * count).into_bytes());
    }
    WheelAction::ScrollView(notches * lines_per_step)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Rule 1129: a Mac trackpad drag moves one line a line: the view, the alternate screen's
    // arrows and a mouse program's reports; a notch stays three lines.
    #[test]
    fn a_line_step_is_one_line_everywhere_and_a_notch_stays_three() {
        assert_eq!(
            wheel_lines_action(2, &ctx(), 0, 0),
            WheelAction::ScrollView(2)
        );
        assert_eq!(wheel_lines_action(0, &ctx(), 0, 0), WheelAction::None);
        assert_eq!(wheel_action(1, &ctx(), 0, 0), WheelAction::ScrollView(3));
        let alt = WheelContext {
            alt_screen: true,
            ..ctx()
        };
        assert_eq!(
            wheel_lines_action(-2, &alt, 0, 0),
            WheelAction::Bytes(b"\x1b[B\x1b[B".to_vec())
        );
        let sgr = WheelContext {
            mouse: MouseTracking::Vt200,
            sgr_mouse: true,
            ..ctx()
        };
        assert_eq!(
            wheel_lines_action(2, &sgr, 4, 1),
            WheelAction::Bytes(b"\x1b[<64;5;2M\x1b[<64;5;2M".to_vec())
        );
    }

    fn ctx() -> WheelContext {
        WheelContext {
            mouse: MouseTracking::None,
            sgr_mouse: false,
            alt_screen: false,
            alternate_scroll: true,
            app_cursor: false,
        }
    }

    #[test]
    fn wheel_scrolls_the_view_three_lines_a_notch() {
        assert_eq!(wheel_action(1, &ctx(), 0, 0), WheelAction::ScrollView(3));
        assert_eq!(wheel_action(-2, &ctx(), 0, 0), WheelAction::ScrollView(-6));
        assert_eq!(wheel_action(0, &ctx(), 0, 0), WheelAction::None);
    }

    #[test]
    fn wheel_on_the_alt_screen_sends_arrows() {
        let alt = WheelContext {
            alt_screen: true,
            ..ctx()
        };
        assert_eq!(
            wheel_action(1, &alt, 0, 0),
            WheelAction::Bytes(b"\x1b[A\x1b[A\x1b[A".to_vec())
        );
        assert_eq!(
            wheel_action(-1, &alt, 0, 0),
            WheelAction::Bytes(b"\x1b[B\x1b[B\x1b[B".to_vec())
        );
        let app = WheelContext {
            app_cursor: true,
            ..alt
        };
        assert_eq!(
            wheel_action(1, &app, 0, 0),
            WheelAction::Bytes(b"\x1bOA\x1bOA\x1bOA".to_vec())
        );
    }

    #[test]
    fn wheel_on_the_alt_screen_without_alternate_scroll_does_nothing() {
        let alt = WheelContext {
            alt_screen: true,
            alternate_scroll: false,
            ..ctx()
        };
        assert_eq!(wheel_action(1, &alt, 0, 0), WheelAction::None);
    }

    #[test]
    fn wheel_reports_to_a_program_that_owns_the_mouse_sgr() {
        let sgr = WheelContext {
            mouse: MouseTracking::Vt200,
            sgr_mouse: true,
            ..ctx()
        };
        assert_eq!(
            wheel_action(1, &sgr, 4, 9),
            WheelAction::Bytes(b"\x1b[<64;5;10M".to_vec())
        );
        assert_eq!(
            wheel_action(-1, &sgr, 4, 9),
            WheelAction::Bytes(b"\x1b[<65;5;10M".to_vec())
        );
        let up2 = b"\x1b[<64;5;10M".repeat(2);
        assert_eq!(wheel_action(2, &sgr, 4, 9), WheelAction::Bytes(up2));
    }

    #[test]
    fn wheel_reports_x10_when_not_sgr() {
        let x10 = WheelContext {
            mouse: MouseTracking::Vt200,
            ..ctx()
        };
        assert_eq!(
            wheel_action(1, &x10, 0, 0),
            WheelAction::Bytes(vec![0x1b, b'[', b'M', 96, 33, 33])
        );
        assert_eq!(wheel_action(1, &x10, 2015, 0), WheelAction::None);
    }

    #[test]
    fn a_program_owning_the_mouse_beats_the_alt_screen() {
        let both = WheelContext {
            mouse: MouseTracking::Any,
            sgr_mouse: true,
            alt_screen: true,
            ..ctx()
        };
        assert_eq!(
            wheel_action(1, &both, 4, 9),
            WheelAction::Bytes(b"\x1b[<64;5;10M".to_vec())
        );
    }

    #[test]
    fn the_default_report_is_utf8_past_column_94() {
        let c = WheelContext {
            mouse: MouseTracking::Vt200,
            ..ctx()
        };
        let WheelAction::Bytes(b) = wheel_action(1, &c, 99, 0) else {
            panic!("bytes")
        };
        assert_eq!(b, "\x1b[M`\u{84}!".as_bytes());
    }
}
