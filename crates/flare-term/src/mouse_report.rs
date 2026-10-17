//! Mouse reports for programs that own the mouse (the rules of Prism Terminal and xterm's
//! MouseStateService). encode_mouse turns one pointer event into the bytes a program asked for,
//! or nothing when its tracking mode does not report that event.
//! Positions are 0-based grid cells.

use crate::click_caret::MouseTracking;

/// A pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// What the pointer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Release,
    Move,
}

/// Modifier keys held with the event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseMods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// One pointer event over the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvt {
    pub kind: MouseKind,
    /// None for a Move with no button held.
    pub button: Option<MouseButton>,
    /// 0-based cell column.
    pub col: usize,
    /// 0-based cell row.
    pub row: usize,
    pub mods: MouseMods,
}

fn button_code(button: Option<MouseButton>) -> u8 {
    match button {
        Some(MouseButton::Left) => 0,
        Some(MouseButton::Middle) => 1,
        Some(MouseButton::Right) => 2,
        None => 3,
    }
}

fn mods_code(mods: MouseMods) -> u8 {
    let mut code = 0;
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.ctrl {
        code += 16;
    }
    code
}

/// Whether the tracking mode reports this event at all.
fn reported(tracking: MouseTracking, e: &MouseEvt) -> bool {
    match (tracking, e.kind) {
        (MouseTracking::None, _) => false,
        (MouseTracking::X10, MouseKind::Press) => true,
        (MouseTracking::X10, _) => false,
        (MouseTracking::Vt200, MouseKind::Move) => false,
        (MouseTracking::Vt200, _) => true,
        (MouseTracking::Drag, MouseKind::Move) => e.button.is_some(),
        (MouseTracking::Drag, _) => true,
        (MouseTracking::Any, _) => true,
    }
}

/// The bytes to send, or None when the protocol does not report this event or the default
/// encoding cannot address the cell (col or row above 222).
/// Protocols: X10 = Press only, modifiers cleared; Vt200 = Press and Release; Drag = those, and
/// Move while a button is held; Any = everything; None = nothing.
/// code = button (Left 0, Middle 1, Right 2, none 3) + shift 4 + alt 8 + ctrl 16, + 32 for a Move;
/// a Release in the default encoding is button 3 (so code = 3 + mods), in SGR it keeps the button.
/// Default: ESC [ M, code+32, col+33, row+33. SGR: ESC [ < code ; col+1 ; row+1, then 'M' (Press,
/// Move) or 'm' (Release).
pub fn encode_mouse(e: &MouseEvt, tracking: MouseTracking, sgr: bool) -> Option<Vec<u8>> {
    if !reported(tracking, e) {
        return None;
    }
    let mods = if tracking == MouseTracking::X10 {
        MouseMods::default()
    } else {
        e.mods
    };
    let motion = if e.kind == MouseKind::Move { 32 } else { 0 };
    let button = if e.kind == MouseKind::Release && !sgr {
        3
    } else {
        button_code(e.button)
    };
    let code = button + mods_code(mods) + motion;
    if sgr {
        let end = if e.kind == MouseKind::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{};{};{}{}", code, e.col + 1, e.row + 1, end).into_bytes());
    }
    let cx = e.col + 33;
    let cy = e.row + 33;
    if cx > 255 || cy > 255 {
        return None;
    }
    Some(vec![0x1b, b'[', b'M', code + 32, cx as u8, cy as u8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wheel::{WheelAction, WheelContext, wheel_action};

    fn evt(kind: MouseKind, button: Option<MouseButton>, col: usize, row: usize) -> MouseEvt {
        MouseEvt {
            kind,
            button,
            col,
            row,
            mods: MouseMods::default(),
        }
    }

    fn ctx_for(tracking: MouseTracking) -> WheelContext {
        WheelContext {
            mouse: tracking,
            sgr_mouse: false,
            alt_screen: false,
            alternate_scroll: false,
            app_cursor: false,
        }
    }

    #[test]
    fn a_left_press_in_the_default_encoding() {
        let e = evt(MouseKind::Press, Some(MouseButton::Left), 4, 2);
        let bytes = encode_mouse(&e, MouseTracking::Vt200, false).unwrap();
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 37, 35]);
    }

    #[test]
    fn a_release_is_button_three_in_the_default_encoding() {
        let e = evt(MouseKind::Release, Some(MouseButton::Left), 0, 0);
        let bytes = encode_mouse(&e, MouseTracking::Vt200, false).unwrap();
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 35, 33, 33]);
    }

    #[test]
    fn sgr_keeps_the_button_and_ends_in_lowercase_m_on_release() {
        let press = evt(MouseKind::Press, Some(MouseButton::Right), 9, 19);
        let bytes = encode_mouse(&press, MouseTracking::Vt200, true).unwrap();
        assert_eq!(bytes, b"\x1b[<2;10;20M".to_vec());
        let release = evt(MouseKind::Release, Some(MouseButton::Right), 9, 19);
        let bytes = encode_mouse(&release, MouseTracking::Vt200, true).unwrap();
        assert_eq!(bytes, b"\x1b[<2;10;20m".to_vec());
    }

    #[test]
    fn modifiers_add_4_8_16() {
        let mut e = evt(MouseKind::Press, Some(MouseButton::Left), 0, 0);
        e.mods = MouseMods {
            shift: true,
            alt: true,
            ctrl: true,
        };
        let bytes = encode_mouse(&e, MouseTracking::Vt200, true).unwrap();
        assert_eq!(bytes, b"\x1b[<28;1;1M".to_vec());
    }

    #[test]
    fn x10_reports_presses_only_and_drops_modifiers() {
        let mut press = evt(MouseKind::Press, Some(MouseButton::Left), 0, 0);
        press.mods.ctrl = true;
        let bytes = encode_mouse(&press, MouseTracking::X10, false).unwrap();
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 33, 33]);
        let release = evt(MouseKind::Release, Some(MouseButton::Left), 0, 0);
        assert_eq!(encode_mouse(&release, MouseTracking::X10, false), None);
        let motion = evt(MouseKind::Move, Some(MouseButton::Left), 0, 0);
        assert_eq!(encode_mouse(&motion, MouseTracking::X10, false), None);
    }

    #[test]
    fn vt200_has_no_motion() {
        let motion = evt(MouseKind::Move, Some(MouseButton::Left), 1, 1);
        assert_eq!(encode_mouse(&motion, MouseTracking::Vt200, true), None);
    }

    #[test]
    fn drag_reports_motion_only_with_a_button() {
        let held = evt(MouseKind::Move, Some(MouseButton::Left), 1, 1);
        assert_eq!(
            encode_mouse(&held, MouseTracking::Drag, true),
            Some(b"\x1b[<32;2;2M".to_vec())
        );
        let none = evt(MouseKind::Move, None, 1, 1);
        assert_eq!(encode_mouse(&none, MouseTracking::Drag, true), None);
    }

    #[test]
    fn any_reports_buttonless_motion_as_button_three() {
        let motion = evt(MouseKind::Move, None, 0, 0);
        assert_eq!(
            encode_mouse(&motion, MouseTracking::Any, true),
            Some(b"\x1b[<35;1;1M".to_vec())
        );
    }

    #[test]
    fn tracking_none_reports_nothing() {
        let e = evt(MouseKind::Press, Some(MouseButton::Left), 0, 0);
        assert_eq!(encode_mouse(&e, MouseTracking::None, true), None);
        assert_eq!(encode_mouse(&e, MouseTracking::None, false), None);
    }

    #[test]
    fn an_unaddressable_cell_is_not_reported_in_the_default_encoding() {
        let far = evt(MouseKind::Press, Some(MouseButton::Left), 223, 0);
        assert_eq!(encode_mouse(&far, MouseTracking::Vt200, false), None);
        let edge = evt(MouseKind::Press, Some(MouseButton::Left), 222, 0);
        assert!(encode_mouse(&edge, MouseTracking::Vt200, false).is_some());
        let wide = evt(MouseKind::Press, Some(MouseButton::Left), 500, 0);
        assert!(encode_mouse(&wide, MouseTracking::Vt200, true).is_some());
    }

    #[test]
    fn the_wheel_layout_matches_m2s() {
        let press = evt(MouseKind::Press, Some(MouseButton::Left), 5, 7);
        let bytes = encode_mouse(&press, MouseTracking::Vt200, false).unwrap();
        assert_eq!(&bytes[..3], &[0x1b, b'[', b'M']);
        assert_eq!(bytes.len(), 6);
        assert_eq!(&bytes[4..], &[38, 40]);
        match wheel_action(1, &ctx_for(MouseTracking::Vt200), 5, 7) {
            WheelAction::Bytes(wheel) => {
                assert_eq!(&wheel[..3], &bytes[..3]);
                assert_eq!(&wheel[4..], &bytes[4..]);
            }
            other => panic!("expected bytes, got {other:?}"),
        }
    }
}
