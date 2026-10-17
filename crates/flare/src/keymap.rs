//! winit key events -> flare_term KeyEvent (decision O2: physical keys decide the byte).
//! Slint 1.18 key events carry no scan code, so the window's winit event filter feeds the
//! parts of a winit `KeyboardInput` through `key_event_from_parts`. Names follow Prism
//! Terminal's W3C convention: `code` is the Debug name of the physical `KeyCode`
//! ("KeyC", "Digit1", "NumpadEnter"), which is the W3C code name for that key.

use flare_term::keys::KeyEvent;
use slint::winit_030::winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};

/// The W3C code name of a physical key, or "Unidentified" when winit cannot name it.
pub fn code_name(p: &PhysicalKey) -> String {
    match p {
        PhysicalKey::Code(code) => format!("{code:?}"),
        PhysicalKey::Unidentified(_) => "Unidentified".to_string(),
    }
}

/// The logical key's name: the typed character, or the named key's name (Space is " ").
pub fn key_name(k: &Key) -> String {
    match k {
        Key::Character(s) => s.to_string(),
        Key::Named(NamedKey::Space) => " ".to_string(),
        Key::Named(n) => format!("{n:?}"),
        Key::Dead(_) => "Dead".to_string(),
        Key::Unidentified(_) => "Unidentified".to_string(),
    }
}

/// Builds the terminal key event from the parts of a winit keyboard event.
/// `text` is dropped when Ctrl is down without Alt: winit then reports the control byte
/// ("\u{3}" for Ctrl+C), and the encoder decides from the physical key instead.
pub fn key_event_from_parts(
    pressed: bool,
    physical: &PhysicalKey,
    logical: &Key,
    text: Option<&str>,
    mods: ModifiersState,
) -> KeyEvent {
    let ctrl = mods.control_key();
    let alt = mods.alt_key();
    let text = text
        .filter(|t| !t.is_empty())
        .filter(|_| !(ctrl && !alt))
        .map(str::to_string);
    KeyEvent {
        pressed,
        key: key_name(logical),
        code: code_name(physical),
        text,
        ctrl,
        shift: mods.shift_key(),
        alt,
        meta: mods.super_key(),
    }
}

/// Keys Slint keeps for itself: F12 (the debug overlay). Every chord is `AppCtl`'s.
pub fn reserved_for_slint(code: &str) -> bool {
    code == "F12"
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_term::key_encode::encode_key;
    use slint::winit_030::winit::keyboard::{KeyCode, NativeKey, NativeKeyCode};

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    fn code(c: KeyCode) -> PhysicalKey {
        PhysicalKey::Code(c)
    }

    #[test]
    fn codes_use_the_w3c_names() {
        assert_eq!(code_name(&code(KeyCode::KeyC)), "KeyC");
        assert_eq!(code_name(&code(KeyCode::Digit1)), "Digit1");
        assert_eq!(code_name(&code(KeyCode::Enter)), "Enter");
        assert_eq!(code_name(&code(KeyCode::ArrowUp)), "ArrowUp");
        assert_eq!(code_name(&code(KeyCode::F5)), "F5");
        assert_eq!(code_name(&code(KeyCode::Space)), "Space");
        assert_eq!(code_name(&code(KeyCode::NumpadEnter)), "NumpadEnter");
        assert_eq!(
            code_name(&PhysicalKey::Unidentified(NativeKeyCode::Unidentified)),
            "Unidentified"
        );
    }

    #[test]
    fn logical_key_names() {
        assert_eq!(key_name(&ch("c")), "c");
        assert_eq!(key_name(&Key::Named(NamedKey::Enter)), "Enter");
        assert_eq!(key_name(&Key::Named(NamedKey::Space)), " ");
        assert_eq!(key_name(&Key::Named(NamedKey::ArrowLeft)), "ArrowLeft");
        assert_eq!(key_name(&Key::Named(NamedKey::F1)), "F1");
        assert_eq!(key_name(&Key::Dead(None)), "Dead");
    }

    #[test]
    fn ctrl_c_on_a_russian_layout_keeps_the_physical_key() {
        let ev = key_event_from_parts(
            true,
            &code(KeyCode::KeyC),
            &ch("с"),
            Some("\u{3}"),
            ModifiersState::CONTROL,
        );
        assert_eq!(
            ev,
            KeyEvent {
                pressed: true,
                key: "с".to_string(),
                code: "KeyC".to_string(),
                text: None,
                ctrl: true,
                shift: false,
                alt: false,
                meta: false,
            }
        );
        assert_eq!(encode_key(&ev, false), Some(vec![3]));
    }

    #[test]
    fn alt_keeps_its_text() {
        let ev = key_event_from_parts(
            true,
            &code(KeyCode::KeyB),
            &ch("b"),
            Some("b"),
            ModifiersState::ALT,
        );
        assert_eq!(ev.text, Some("b".to_string()));
        assert!(ev.alt);
        assert_eq!(encode_key(&ev, false), Some(b"\x1bb".to_vec()));
    }

    #[test]
    fn altgr_is_ctrl_plus_alt_with_text() {
        let ev = key_event_from_parts(
            true,
            &code(KeyCode::Digit2),
            &ch("@"),
            Some("@"),
            ModifiersState::CONTROL | ModifiersState::ALT,
        );
        assert_eq!(ev.text, Some("@".to_string()));
        assert_eq!(encode_key(&ev, false), Some(b"@".to_vec()));
    }

    #[test]
    fn a_release_is_not_pressed() {
        let ev = key_event_from_parts(
            false,
            &code(KeyCode::KeyA),
            &ch("a"),
            Some("a"),
            ModifiersState::empty(),
        );
        assert!(!ev.pressed);
        assert_eq!(encode_key(&ev, false), None);
    }

    #[test]
    fn empty_text_is_none() {
        let ev = key_event_from_parts(
            true,
            &code(KeyCode::KeyA),
            &ch("a"),
            Some(""),
            ModifiersState::empty(),
        );
        assert_eq!(ev.text, None);
    }

    #[test]
    fn shift_and_super_map() {
        let ev = key_event_from_parts(
            true,
            &code(KeyCode::KeyA),
            &ch("A"),
            Some("A"),
            ModifiersState::SHIFT | ModifiersState::SUPER,
        );
        assert!(ev.shift);
        assert!(ev.meta);
    }

    #[test]
    fn f12_is_reserved_and_nothing_else() {
        assert!(reserved_for_slint("F12"));
        assert!(!reserved_for_slint("F11"));
        assert!(!reserved_for_slint("KeyC"));
    }

    #[test]
    fn unidentified_logical_key_is_named() {
        assert_eq!(
            key_name(&Key::Unidentified(NativeKey::Unidentified)),
            "Unidentified"
        );
    }
}
