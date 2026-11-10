//! The macOS terminal keys (port phase 3): what a key the app did not take sends to the shell on a
//! Mac, before the xterm encoder. Pure, so it is tested on every system.
//!
//! The Mac conventions, as Terminal.app and iTerm2 keep them:
//! - Cmd never types: a Cmd key that is no shortcut sends nothing, except Cmd+Left and Cmd+Right
//!   (line start and end: ^A and ^E) and Cmd+Backspace (delete to line start: ^U).
//! - Option+Left and Option+Right jump a word (ESC b, ESC f); Option+Backspace deletes the word
//!   before the caret (ESC DEL) and Option+Delete the one after it (ESC d).
//! - Option types the layout's character (Norwegian Option+2 is @, Option+7 is |) unless "Use
//!   Option as Meta" is on; then Option+key sends ESC and the key as it is without Option.
//! - Ctrl+letter is the control character, as everywhere.

use crate::keys::KeyEvent;

/// What a Mac key sends to the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PtyKey {
    /// Encode this event as usual (`key_encode::encode_key`).
    Event(KeyEvent),
    /// Send exactly this text.
    Text(String),
    /// Send nothing.
    Nothing,
}

/// The macOS translation of a key for the shell. `option_as_meta` is the "Use Option as Meta"
/// setting (default off, like Terminal.app).
pub fn mac_pty_key(e: &KeyEvent, option_as_meta: bool) -> PtyKey {
    if !e.pressed {
        return PtyKey::Event(e.clone());
    }
    let text = |s: &str| PtyKey::Text(s.to_string());
    // Cmd: the line keys, else nothing.
    if e.meta {
        if e.ctrl || e.alt || e.shift {
            return PtyKey::Nothing;
        }
        return match e.key.as_str() {
            "ArrowLeft" => text("\x01"),
            "ArrowRight" => text("\x05"),
            "Backspace" => text("\x15"),
            _ => PtyKey::Nothing,
        };
    }
    if !e.alt {
        return PtyKey::Event(e.clone());
    }
    // Option, with or without Ctrl and Shift: the word keys come first, the setting aside.
    if !e.ctrl && !e.shift {
        match e.key.as_str() {
            "ArrowLeft" => return text("\x1bb"),
            "ArrowRight" => return text("\x1bf"),
            "Backspace" => return text("\x1b\x7f"),
            "Delete" => return text("\x1bd"),
            _ => {}
        }
    }
    if !option_as_meta {
        // Option is the layout's: the typed character, or the plain key with Ctrl and Shift.
        let mut out = e.clone();
        out.alt = false;
        return PtyKey::Event(out);
    }
    if e.ctrl {
        // Ctrl+Option+key as Meta: ESC and the control character (no AltGr on a Mac).
        let mut out = e.clone();
        out.text = None;
        return PtyKey::Event(out);
    }
    match base_char(e) {
        Some(c) => PtyKey::Text(format!("\x1b{c}")),
        // A named key (Enter, an arrow, a function key): the xterm encoder's Alt forms.
        None => {
            let mut out = e.clone();
            out.text = None;
            PtyKey::Event(out)
        }
    }
}

/// The character the key makes without Option: the physical letter or digit (Shift makes the
/// letter upper case), else the key's own single character, else none.
fn base_char(e: &KeyEvent) -> Option<String> {
    if let Some(l) = e.code.strip_prefix("Key")
        && l.len() == 1
        && l.as_bytes()[0].is_ascii_uppercase()
    {
        let c = l.as_bytes()[0] as char;
        return Some(if e.shift { c } else { c.to_ascii_lowercase() }.to_string());
    }
    if let Some(d) = e.code.strip_prefix("Digit")
        && d.len() == 1
        && d.as_bytes()[0].is_ascii_digit()
        && !e.shift
    {
        return Some(d.to_string());
    }
    let mut chars = e.key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_encode::encode_key;

    fn k(key: &str, code: &str) -> KeyEvent {
        KeyEvent::new(key, code)
    }

    /// The bytes the shell gets on a Mac, through the encoder where the translation says so.
    fn bytes(e: &KeyEvent, meta: bool) -> Option<Vec<u8>> {
        match mac_pty_key(e, meta) {
            PtyKey::Event(ev) => encode_key(&ev, false),
            PtyKey::Text(t) => Some(t.into_bytes()),
            PtyKey::Nothing => None,
        }
    }

    #[test]
    fn cmd_left_and_right_go_to_the_line_start_and_end() {
        assert_eq!(
            bytes(&k("ArrowLeft", "ArrowLeft").meta(), false),
            Some(vec![1])
        );
        assert_eq!(
            bytes(&k("ArrowRight", "ArrowRight").meta(), false),
            Some(vec![5])
        );
        assert_eq!(
            bytes(&k("Backspace", "Backspace").meta(), false),
            Some(vec![0x15])
        );
    }

    #[test]
    fn option_left_and_right_jump_a_word_whatever_the_setting() {
        for meta in [false, true] {
            assert_eq!(
                bytes(&k("ArrowLeft", "ArrowLeft").alt(), meta),
                Some(b"\x1bb".to_vec())
            );
            assert_eq!(
                bytes(&k("ArrowRight", "ArrowRight").alt(), meta),
                Some(b"\x1bf".to_vec())
            );
            assert_eq!(
                bytes(&k("Backspace", "Backspace").alt(), meta),
                Some(b"\x1b\x7f".to_vec())
            );
            assert_eq!(
                bytes(&k("Delete", "Delete").alt(), meta),
                Some(b"\x1bd".to_vec())
            );
        }
    }

    #[test]
    fn a_cmd_key_that_is_no_shortcut_never_types() {
        for e in [
            k("q", "KeyQ").meta().text("q"),
            k("k", "KeyK").meta(),
            k("ArrowUp", "ArrowUp").meta(),
            k("ArrowLeft", "ArrowLeft").meta().shift(),
            k("f", "KeyF").meta().ctrl(),
            k("e", "KeyE").meta().alt(),
        ] {
            assert_eq!(mac_pty_key(&e, false), PtyKey::Nothing, "{e:?}");
            assert_eq!(mac_pty_key(&e, true), PtyKey::Nothing, "{e:?}");
        }
    }

    #[test]
    fn ctrl_letters_are_control_characters_on_the_mac_too() {
        for (key, byte) in [
            ("c", 3u8),
            ("d", 4),
            ("a", 1),
            ("e", 5),
            ("t", 0x14),
            ("w", 0x17),
        ] {
            let code = format!("Key{}", key.to_uppercase());
            assert_eq!(
                bytes(&k(key, &code).ctrl(), false),
                Some(vec![byte]),
                "{key}"
            );
        }
    }

    #[test]
    fn option_types_the_layouts_character_while_option_as_meta_is_off() {
        // Norwegian: Option+2 is @, Option+Shift+7 is \\; US: Option+e is a dead key, Option+a is å.
        assert_eq!(
            bytes(&k("@", "Digit2").alt().text("@"), false),
            Some(b"@".to_vec())
        );
        assert_eq!(
            bytes(&k("\\", "Digit7").alt().shift().text("\\"), false),
            Some(b"\\".to_vec())
        );
        assert_eq!(
            bytes(&k("\u{e5}", "KeyA").alt().text("\u{e5}"), false),
            Some("\u{e5}".as_bytes().to_vec())
        );
        // A named key with Option and no word meaning: the plain key.
        assert_eq!(bytes(&k("Enter", "Enter").alt(), false), Some(vec![0x0d]));
    }

    #[test]
    fn option_as_meta_sends_escape_and_the_key_without_option() {
        assert_eq!(
            bytes(&k("\u{e5}", "KeyA").alt().text("\u{e5}"), true),
            Some(b"\x1ba".to_vec())
        );
        assert_eq!(
            bytes(&k("\u{2202}", "KeyD").alt().text("\u{2202}"), true),
            Some(b"\x1bd".to_vec())
        );
        assert_eq!(
            bytes(&k("B", "KeyB").alt().shift().text("B"), true),
            Some(b"\x1bB".to_vec())
        );
        assert_eq!(
            bytes(&k("@", "Digit2").alt().text("@"), true),
            Some(b"\x1b2".to_vec())
        );
        assert_eq!(
            bytes(&k(".", "Period").alt().text("."), true),
            Some(b"\x1b.".to_vec())
        );
        // Named keys keep the xterm Alt forms: Option+Enter is ESC CR.
        assert_eq!(
            bytes(&k("Enter", "Enter").alt(), true),
            Some(b"\x1b\r".to_vec())
        );
        // Ctrl+Option+x as Meta: ESC ^X.
        assert_eq!(
            bytes(&k("x", "KeyX").alt().ctrl().text("x"), true),
            Some(vec![0x1b, 0x18])
        );
        // Off: Ctrl+Option+x is the control character alone.
        assert_eq!(
            bytes(&k("x", "KeyX").alt().ctrl().text("x"), false),
            Some(vec![0x18])
        );
    }

    #[test]
    fn plain_keys_and_releases_pass_through() {
        let a = k("a", "KeyA").text("a");
        assert_eq!(mac_pty_key(&a, false), PtyKey::Event(a.clone()));
        let up = k("a", "KeyA").meta().released();
        assert_eq!(mac_pty_key(&up, false), PtyKey::Event(up.clone()));
        assert_eq!(bytes(&k("Enter", "Enter"), false), Some(vec![0x0d]));
    }
}
