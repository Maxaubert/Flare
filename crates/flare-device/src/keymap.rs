//! The US keymap: a character to a USB HID keyboard usage (page 0x07) plus shift, which is what the simulator's
//! keyboard events take (rule 1314). Text the keymap cannot type (anything outside printable ASCII, Return and Tab)
//! goes through the simulator's own pasteboard and Cmd-V instead; the Mac's clipboard is never touched.
//!
//! Ported from idb, `Tools/SimScope/SimScope/Keymap.swift` (facebook/idb, commit be342ac):
//!
//! Copyright (c) Meta Platforms, Inc. and affiliates.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
//! documentation files (the "Software"), to deal in the Software without restriction, including without limitation
//! the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and
//! to permit persons to whom the Software is furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all copies or substantial portions of
//! the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO
//! THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
//! CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
//! IN THE SOFTWARE.

use crate::source::Key;

/// Named usages (modifier and control keys).
pub mod usage {
    pub const RETURN: u32 = 0x28;
    pub const ESCAPE: u32 = 0x29;
    /// Backspace.
    pub const DELETE: u32 = 0x2A;
    pub const TAB: u32 = 0x2B;
    pub const SPACE: u32 = 0x2C;
    pub const FORWARD_DELETE: u32 = 0x4C;
    pub const RIGHT: u32 = 0x4F;
    pub const LEFT: u32 = 0x50;
    pub const DOWN: u32 = 0x51;
    pub const UP: u32 = 0x52;
    pub const LEFT_SHIFT: u32 = 0xE1;
    pub const LEFT_GUI: u32 = 0xE3;
    /// The `v` key (Cmd-V pastes the simulator's pasteboard).
    pub const V: u32 = 0x19;
}

/// One character's key: its usage and whether shift is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HidKey {
    pub usage: u32,
    pub shift: bool,
}

/// The number row's usages, 1 to 9 then 0.
const DIGITS: &str = "1234567890";
const DIGIT_SHIFTED: &str = "!@#$%^&*()";
/// Punctuation: unshifted, shifted, usage.
const PUNCTUATION: [(char, char, u32); 11] = [
    ('-', '_', 0x2D),
    ('=', '+', 0x2E),
    ('[', '{', 0x2F),
    (']', '}', 0x30),
    ('\\', '|', 0x31),
    (';', ':', 0x33),
    ('\'', '"', 0x34),
    ('`', '~', 0x35),
    (',', '<', 0x36),
    ('.', '>', 0x37),
    ('/', '?', 0x38),
];

/// The key that types `c` on a US keyboard, or None when the keymap has none (it is pasted instead).
pub fn key_for(c: char) -> Option<HidKey> {
    let plain = |usage| {
        Some(HidKey {
            usage,
            shift: false,
        })
    };
    let shifted = |usage| Some(HidKey { usage, shift: true });
    match c {
        'a'..='z' => plain(0x04 + (c as u32 - 'a' as u32)),
        'A'..='Z' => shifted(0x04 + (c as u32 - 'A' as u32)),
        ' ' => plain(usage::SPACE),
        '\n' => plain(usage::RETURN),
        '\t' => plain(usage::TAB),
        _ => {
            if let Some(i) = DIGITS.find(c) {
                return plain(0x1E + i as u32);
            }
            if let Some(i) = DIGIT_SHIFTED.find(c) {
                return shifted(0x1E + i as u32);
            }
            PUNCTUATION.iter().find_map(|&(base, shift, u)| {
                if c == base {
                    plain(u)
                } else if c == shift {
                    shifted(u)
                } else {
                    None
                }
            })
        }
    }
}

/// The key events that type `k`: shift down, key down, key up, shift up.
pub fn events(k: HidKey) -> Vec<Key> {
    let key = |usage, down| Key { usage, down };
    let mut out = Vec::with_capacity(4);
    if k.shift {
        out.push(key(usage::LEFT_SHIFT, true));
    }
    out.push(key(k.usage, true));
    out.push(key(k.usage, false));
    if k.shift {
        out.push(key(usage::LEFT_SHIFT, false));
    }
    out
}

/// How a piece of text is typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typing {
    /// Every character has a key: these, in order.
    Keys(Vec<HidKey>),
    /// Some character has none: the whole text goes through the simulator's pasteboard and Cmd-V, so its order is
    /// kept.
    Paste(String),
}

/// How `text` is typed.
pub fn typing(text: &str) -> Typing {
    match text.chars().map(key_for).collect::<Option<Vec<_>>>() {
        Some(keys) => Typing::Keys(keys),
        None => Typing::Paste(text.to_string()),
    }
}

/// The key `device_press` names (return, delete, escape, tab, the arrows), or None.
pub fn named(name: &str) -> Option<u32> {
    Some(match name {
        "return" | "enter" => usage::RETURN,
        "delete" | "backspace" => usage::DELETE,
        "forward_delete" => usage::FORWARD_DELETE,
        "escape" => usage::ESCAPE,
        "tab" => usage::TAB,
        "space" => usage::SPACE,
        "left" => usage::LEFT,
        "right" => usage::RIGHT,
        "up" => usage::UP,
        "down" => usage::DOWN,
        _ => return None,
    })
}

/// The name a key is shown by on the keyboard badge.
pub fn label(name: &str) -> Option<&'static str> {
    Some(match named(name)? {
        usage::RETURN => "Return",
        usage::DELETE => "Delete",
        usage::FORWARD_DELETE => "Forward Delete",
        usage::ESCAPE => "Escape",
        usage::TAB => "Tab",
        usage::SPACE => "Space",
        usage::LEFT => "Left",
        usage::RIGHT => "Right",
        usage::UP => "Up",
        usage::DOWN => "Down",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(usage: u32, shift: bool) -> Option<HidKey> {
        Some(HidKey { usage, shift })
    }

    #[test]
    fn ascii_types_with_shift_where_needed_and_other_text_needs_paste() {
        assert_eq!(key_for('a'), k(0x04, false));
        assert_eq!(key_for('z'), k(0x1D, false));
        assert_eq!(key_for('A'), k(0x04, true));
        assert_eq!(key_for('Z'), k(0x1D, true));
        assert_eq!(key_for('1'), k(0x1E, false));
        assert_eq!(key_for('0'), k(0x27, false));
        assert_eq!(key_for('!'), k(0x1E, true));
        assert_eq!(key_for(')'), k(0x27, true));
        assert_eq!(key_for(' '), k(0x2C, false));
        assert_eq!(key_for('\n'), k(0x28, false));
        assert_eq!(key_for('\t'), k(0x2B, false));
        assert_eq!(key_for('-'), k(0x2D, false));
        assert_eq!(key_for('_'), k(0x2D, true));
        assert_eq!(key_for('"'), k(0x34, true));
        assert_eq!(key_for('?'), k(0x38, true));
        assert_eq!(key_for('~'), k(0x35, true));
        // Every printable ASCII character has a key, and only those (plus Return and Tab).
        for c in (0x20u8..0x7F).map(char::from) {
            assert!(key_for(c).is_some(), "{c:?}");
        }
        for c in [
            '\u{e9}',
            '\u{fc}',
            '\u{2014}',
            '\u{1F600}',
            '\r',
            '\0',
            '\u{7f}',
        ] {
            assert_eq!(key_for(c), None, "{c:?}");
        }
        // Shift wraps the key.
        assert_eq!(
            events(key_for('A').unwrap()),
            vec![
                Key {
                    usage: 0xE1,
                    down: true
                },
                Key {
                    usage: 0x04,
                    down: true
                },
                Key {
                    usage: 0x04,
                    down: false
                },
                Key {
                    usage: 0xE1,
                    down: false
                },
            ]
        );
        assert_eq!(events(key_for('a').unwrap()).len(), 2);
        // Whole texts.
        assert_eq!(
            typing("Hi!"),
            Typing::Keys(vec![
                HidKey {
                    usage: 0x0B,
                    shift: true
                },
                HidKey {
                    usage: 0x0C,
                    shift: false
                },
                HidKey {
                    usage: 0x1E,
                    shift: true
                },
            ])
        );
        assert_eq!(typing(""), Typing::Keys(Vec::new()));
        assert_eq!(typing("caf\u{e9} ok"), Typing::Paste("caf\u{e9} ok".into()));
        assert_eq!(named("return"), Some(usage::RETURN));
        assert_eq!(named("up"), Some(usage::UP));
        assert_eq!(named("f13"), None);
        assert_eq!(label("return"), Some("Return"));
        assert_eq!(label("f13"), None);
    }
}
