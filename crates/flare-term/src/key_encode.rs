//! encode_key (xterm byte sequences).
//! The bytes a key sends to the pty, following xterm's ctlseqs. Alacritty has no encoder,
//! so this is new code; the rules are the ones written in the Task 10 plan.

use crate::keys::KeyEvent;

/// The bytes a key sends to the pty, or None when it sends nothing. `app_cursor` is DECCKM.
pub fn encode_key(e: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    if !e.pressed || is_lone_modifier(&e.key) {
        return None;
    }
    let m =
        1 + u8::from(e.shift) + 2 * u8::from(e.alt) + 4 * u8::from(e.ctrl) + 8 * u8::from(e.meta);
    let text = e.text.as_deref().filter(|t| !t.is_empty());
    // AltGr (Ctrl and Alt together) types its text; the physical key is not a chord here.
    if e.ctrl && e.alt && text.is_some() {
        return text.map(|t| t.as_bytes().to_vec());
    }
    if let Some(bytes) = named_key(e, m, app_cursor) {
        return Some(bytes);
    }
    if e.ctrl && !e.alt {
        if let Some(b) = ctrl_for(e) {
            return Some(vec![b]);
        }
        return text.map(|t| t.as_bytes().to_vec());
    }
    if e.alt && !e.ctrl {
        let body = match text {
            Some(t) => t.to_string(),
            None if e.key.chars().count() == 1 => e.key.clone(),
            None => return None,
        };
        let mut out = vec![0x1b];
        out.extend_from_slice(body.as_bytes());
        return Some(out);
    }
    if e.ctrl && e.alt {
        return ctrl_for(e).map(|b| vec![0x1b, b]);
    }
    text.map(|t| t.as_bytes().to_vec())
}

/// Keys that only change the modifier state; they send nothing on their own.
fn is_lone_modifier(key: &str) -> bool {
    matches!(
        key,
        "Shift"
            | "Control"
            | "Alt"
            | "Meta"
            | "AltGraph"
            | "CapsLock"
            | "NumLock"
            | "ScrollLock"
            | "Fn"
            | "FnLock"
            | "Hyper"
            | "Super"
            | "Symbol"
            | "Dead"
            | "Unidentified"
            | "Process"
    )
}

/// Keys the xterm table names by their `key`; None for every other key.
fn named_key(e: &KeyEvent, m: u8, app_cursor: bool) -> Option<Vec<u8>> {
    let csi = |s: &str| format!("\x1b[{s}").into_bytes();
    let tilde = |n: u8| {
        if m == 1 {
            csi(&format!("{n}~"))
        } else {
            csi(&format!("{n};{m}~"))
        }
    };
    let letter_key = |x: char| {
        if m == 1 {
            if app_cursor {
                format!("\x1bO{x}").into_bytes()
            } else {
                csi(&x.to_string())
            }
        } else {
            csi(&format!("1;{m}{x}"))
        }
    };
    let f14 = |x: char| {
        if m == 1 {
            format!("\x1bO{x}").into_bytes()
        } else {
            csi(&format!("1;{m}{x}"))
        }
    };
    let bytes = match e.key.as_str() {
        "Enter" => {
            if e.ctrl {
                vec![0x0a]
            } else if e.alt {
                vec![0x1b, 0x0d]
            } else {
                vec![0x0d]
            }
        }
        "Tab" => {
            if e.shift {
                csi("Z")
            } else if e.ctrl || e.alt {
                return None;
            } else {
                vec![0x09]
            }
        }
        "Backspace" => {
            if e.ctrl {
                vec![0x08]
            } else if e.alt {
                vec![0x1b, 0x7f]
            } else {
                vec![0x7f]
            }
        }
        "Escape" => {
            if e.alt {
                vec![0x1b, 0x1b]
            } else {
                vec![0x1b]
            }
        }
        "ArrowUp" => letter_key('A'),
        "ArrowDown" => letter_key('B'),
        "ArrowRight" => letter_key('C'),
        "ArrowLeft" => letter_key('D'),
        "Home" => letter_key('H'),
        "End" => letter_key('F'),
        "Insert" => tilde(2),
        "Delete" => tilde(3),
        "PageUp" => tilde(5),
        "PageDown" => tilde(6),
        "F1" => f14('P'),
        "F2" => f14('Q'),
        "F3" => f14('R'),
        "F4" => f14('S'),
        "F5" => tilde(15),
        "F6" => tilde(17),
        "F7" => tilde(18),
        "F8" => tilde(19),
        "F9" => tilde(20),
        "F10" => tilde(21),
        "F11" => tilde(23),
        "F12" => tilde(24),
        _ => return None,
    };
    Some(bytes)
}

/// The control byte a physical key makes with Ctrl (the table in the Task 10 plan).
/// The control byte for a Ctrl chord. The key's own ASCII letter decides first (the layout
/// the user types on: Dvorak, AZERTY, QWERTZ); the physical code is the fallback for keys
/// whose name is not ASCII (Cyrillic, Greek) and for digits and punctuation.
fn ctrl_for(e: &KeyEvent) -> Option<u8> {
    let mut chars = e.key.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii_alphabetic()
    {
        return Some(c.to_ascii_uppercase() as u8 - b'A' + 1);
    }
    ctrl_byte(&e.code)
}

fn ctrl_byte(code: &str) -> Option<u8> {
    if let Some(letter) = code.strip_prefix("Key") {
        let mut chars = letter.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && c.is_ascii_uppercase()
        {
            return Some(c as u8 - b'A' + 1);
        }
        return None;
    }
    match code {
        "Space" | "Digit2" => Some(0),
        "Digit3" | "BracketLeft" => Some(0x1b),
        "Digit4" | "Backslash" => Some(0x1c),
        "Digit5" | "BracketRight" => Some(0x1d),
        "Digit6" => Some(0x1e),
        "Digit7" | "Minus" | "Slash" => Some(0x1f),
        "Digit8" => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::KeyEvent;

    fn key(k: &str) -> KeyEvent {
        let code = if k.chars().count() == 1 {
            format!("Key{}", k.to_uppercase())
        } else {
            k.to_string()
        };
        KeyEvent::new(k, &code)
    }
    fn enc(e: &KeyEvent) -> Option<Vec<u8>> {
        encode_key(e, false)
    }
    fn enc_app(e: &KeyEvent) -> Option<Vec<u8>> {
        encode_key(e, true)
    }

    #[test]
    fn plain_text_is_utf8() {
        assert_eq!(enc(&key("a").text("a")), Some(b"a".to_vec()));
        assert_eq!(enc(&key("\u{e6}").text("\u{e6}")), Some(vec![0xc3, 0xa6]));
        assert_eq!(
            enc(&key("\u{1f600}").text("\u{1f600}")),
            Some(vec![0xf0, 0x9f, 0x98, 0x80])
        );
    }

    #[test]
    fn shift_letter_is_just_its_text() {
        assert_eq!(enc(&key("A").shift().text("A")), Some(b"A".to_vec()));
    }

    #[test]
    fn ctrl_letters_use_the_physical_key() {
        assert_eq!(enc(&KeyEvent::new("c", "KeyC").ctrl()), Some(vec![3]));
        assert_eq!(enc(&KeyEvent::new("\u{441}", "KeyC").ctrl()), Some(vec![3]));
        assert_eq!(enc(&KeyEvent::new("z", "KeyZ").ctrl()), Some(vec![26]));
        assert_eq!(
            enc(&KeyEvent::new("a", "KeyA").ctrl().shift()),
            Some(vec![1])
        );
    }

    #[test]
    fn ctrl_punctuation_table() {
        let table: &[(&str, u8)] = &[
            ("Space", 0),
            ("Digit2", 0),
            ("Digit3", 0x1b),
            ("Digit4", 0x1c),
            ("Digit5", 0x1d),
            ("Digit6", 0x1e),
            ("Digit7", 0x1f),
            ("Digit8", 0x7f),
            ("BracketLeft", 0x1b),
            ("Backslash", 0x1c),
            ("BracketRight", 0x1d),
            ("Minus", 0x1f),
            ("Slash", 0x1f),
        ];
        for (code, byte) in table {
            assert_eq!(
                enc(&KeyEvent::new("#", code).ctrl()),
                Some(vec![*byte]),
                "{code}"
            );
        }
    }

    #[test]
    fn ctrl_with_no_table_entry_falls_back_to_text() {
        assert_eq!(
            enc(&KeyEvent::new("1", "Digit1").ctrl().text("1")),
            Some(b"1".to_vec())
        );
        assert_eq!(enc(&KeyEvent::new("1", "Digit1").ctrl()), None);
    }

    #[test]
    fn alt_prefixes_esc() {
        assert_eq!(
            enc(&KeyEvent::new("b", "KeyB").alt().text("b")),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            enc(&KeyEvent::new("x", "KeyX").alt()),
            Some(b"\x1bx".to_vec())
        );
    }

    #[test]
    fn altgr_sends_the_text() {
        let e = KeyEvent::new("@", "Digit2").ctrl().alt().text("@");
        assert_eq!(enc(&e), Some(b"@".to_vec()));
    }

    #[test]
    fn ctrl_alt_letter_is_esc_then_control_byte() {
        assert_eq!(
            enc(&KeyEvent::new("c", "KeyC").ctrl().alt()),
            Some(vec![0x1b, 3])
        );
    }

    #[test]
    fn enter_family() {
        assert_eq!(enc(&key("Enter")), Some(vec![0x0d]));
        assert_eq!(enc(&key("Enter").shift()), Some(vec![0x0d]));
        assert_eq!(enc(&key("Enter").ctrl()), Some(vec![0x0a]));
        assert_eq!(enc(&key("Enter").alt()), Some(vec![0x1b, 0x0d]));
    }

    #[test]
    fn tab_family() {
        assert_eq!(enc(&key("Tab")), Some(vec![9]));
        assert_eq!(enc(&key("Tab").shift()), Some(b"\x1b[Z".to_vec()));
        assert_eq!(enc(&key("Tab").ctrl()), None);
        assert_eq!(enc(&key("Tab").alt()), None);
    }

    #[test]
    fn backspace_family() {
        assert_eq!(enc(&key("Backspace")), Some(vec![0x7f]));
        assert_eq!(enc(&key("Backspace").ctrl()), Some(vec![0x08]));
        assert_eq!(enc(&key("Backspace").alt()), Some(vec![0x1b, 0x7f]));
    }

    #[test]
    fn escape() {
        assert_eq!(enc(&key("Escape")), Some(vec![0x1b]));
        assert_eq!(enc(&key("Escape").alt()), Some(vec![0x1b, 0x1b]));
    }

    #[test]
    fn arrows_normal_and_application() {
        assert_eq!(enc(&key("ArrowUp")), Some(b"\x1b[A".to_vec()));
        assert_eq!(enc(&key("ArrowDown")), Some(b"\x1b[B".to_vec()));
        assert_eq!(enc(&key("ArrowRight")), Some(b"\x1b[C".to_vec()));
        assert_eq!(enc(&key("ArrowLeft")), Some(b"\x1b[D".to_vec()));
        assert_eq!(enc_app(&key("ArrowUp")), Some(b"\x1bOA".to_vec()));
        assert_eq!(enc_app(&key("ArrowDown")), Some(b"\x1bOB".to_vec()));
        assert_eq!(enc_app(&key("ArrowRight")), Some(b"\x1bOC".to_vec()));
        assert_eq!(enc_app(&key("ArrowLeft")), Some(b"\x1bOD".to_vec()));
    }

    #[test]
    fn arrows_with_modifiers_are_csi_one_semicolon_m() {
        assert_eq!(enc(&key("ArrowUp").shift()), Some(b"\x1b[1;2A".to_vec()));
        assert_eq!(enc(&key("ArrowLeft").ctrl()), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(enc(&key("ArrowRight").alt()), Some(b"\x1b[1;3C".to_vec()));
        assert_eq!(
            enc(&key("ArrowDown").ctrl().shift()),
            Some(b"\x1b[1;6B".to_vec())
        );
        assert_eq!(enc_app(&key("ArrowUp").ctrl()), Some(b"\x1b[1;5A".to_vec()));
    }

    #[test]
    fn home_and_end() {
        assert_eq!(enc(&key("Home")), Some(b"\x1b[H".to_vec()));
        assert_eq!(enc_app(&key("Home")), Some(b"\x1bOH".to_vec()));
        assert_eq!(enc(&key("End")), Some(b"\x1b[F".to_vec()));
        assert_eq!(enc_app(&key("End")), Some(b"\x1bOF".to_vec()));
        assert_eq!(enc(&key("Home").ctrl()), Some(b"\x1b[1;5H".to_vec()));
    }

    #[test]
    fn edit_keys() {
        assert_eq!(enc(&key("Insert")), Some(b"\x1b[2~".to_vec()));
        assert_eq!(enc(&key("Delete")), Some(b"\x1b[3~".to_vec()));
        assert_eq!(enc(&key("PageUp")), Some(b"\x1b[5~".to_vec()));
        assert_eq!(enc(&key("PageDown")), Some(b"\x1b[6~".to_vec()));
        assert_eq!(enc(&key("Delete").ctrl()), Some(b"\x1b[3;5~".to_vec()));
        assert_eq!(enc(&key("PageUp").shift()), Some(b"\x1b[5;2~".to_vec()));
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc(&key("F1")), Some(b"\x1bOP".to_vec()));
        assert_eq!(enc(&key("F2")), Some(b"\x1bOQ".to_vec()));
        assert_eq!(enc(&key("F3")), Some(b"\x1bOR".to_vec()));
        assert_eq!(enc(&key("F4")), Some(b"\x1bOS".to_vec()));
        assert_eq!(enc(&key("F1").shift()), Some(b"\x1b[1;2P".to_vec()));
        assert_eq!(enc(&key("F5")), Some(b"\x1b[15~".to_vec()));
        assert_eq!(enc(&key("F6")), Some(b"\x1b[17~".to_vec()));
        assert_eq!(enc(&key("F7")), Some(b"\x1b[18~".to_vec()));
        assert_eq!(enc(&key("F8")), Some(b"\x1b[19~".to_vec()));
        assert_eq!(enc(&key("F9")), Some(b"\x1b[20~".to_vec()));
        assert_eq!(enc(&key("F10")), Some(b"\x1b[21~".to_vec()));
        assert_eq!(enc(&key("F11")), Some(b"\x1b[23~".to_vec()));
        assert_eq!(enc(&key("F12")), Some(b"\x1b[24~".to_vec()));
        assert_eq!(enc(&key("F5").ctrl()), Some(b"\x1b[15;5~".to_vec()));
    }

    #[test]
    fn modifier_params_add_up() {
        assert_eq!(enc(&key("ArrowUp").meta()), Some(b"\x1b[1;9A".to_vec()));
        let all = key("ArrowUp").shift().alt().ctrl().meta();
        assert_eq!(enc(&all), Some(b"\x1b[1;16A".to_vec()));
    }

    #[test]
    fn nothing_is_sent_for_releases_and_lone_modifiers() {
        assert_eq!(enc(&key("a").text("a").released()), None);
        for k in [
            "Shift",
            "Control",
            "Alt",
            "Meta",
            "CapsLock",
            "Dead",
            "Unidentified",
        ] {
            assert_eq!(enc(&KeyEvent::new(k, k)), None, "{k}");
        }
    }

    #[test]
    fn ctrl_letters_follow_the_layout_not_the_position() {
        // Dvorak: the key labelled C sits at physical KeyI.
        assert_eq!(enc(&KeyEvent::new("c", "KeyI").ctrl()), Some(vec![0x03]));
        // AZERTY: the key labelled W sits at physical KeyZ.
        assert_eq!(enc(&KeyEvent::new("w", "KeyZ").ctrl()), Some(vec![0x17]));
        // QWERTZ: Z and Y are swapped.
        assert_eq!(enc(&KeyEvent::new("z", "KeyY").ctrl()), Some(vec![0x1a]));
        assert_eq!(enc(&KeyEvent::new("y", "KeyZ").ctrl()), Some(vec![0x19]));
        // Ctrl+Alt takes the same route.
        assert_eq!(
            enc(&KeyEvent::new("c", "KeyI").ctrl().alt()),
            Some(vec![0x1b, 0x03])
        );
        // Cyrillic still falls back to the physical key.
        assert_eq!(enc(&KeyEvent::new("\u{441}", "KeyC").ctrl()), Some(vec![0x03]));
    }
}
