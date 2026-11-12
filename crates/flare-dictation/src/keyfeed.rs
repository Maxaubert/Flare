//! The key feed: helper lines to reducer events (Task 10). Ported from Prism Terminal's
//! `src/main/globalDictKey.ts` at e5848cb: the virtual keys a binding watches (N13), the Start-menu
//! mask rule (N14), and the feed that keeps the modifier flags the hook reports and turns every
//! down, up, "another key", Esc and tick into the reducer's events (N16).

use crate::hotkey::{
    extra_cancel, reduce_key, Hotkey, KeyAction, KeyChord, KeyConfig, KeyEvt, KeyState, Phase,
    EXTRA_MODIFIER,
};
use std::collections::HashSet;

/// One line of the hook's output, or a key event the hook has already decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// A binding key went down or up. `front` is the process id of the window in front, on a down.
    Key {
        down: bool,
        code: String,
        front: Option<u32>,
    },
    /// Another key went down while a binding key is held (its identity is never reported).
    Other,
    /// A modifier the binding does not name is down: it went down while a binding key is held, or it was
    /// already down when a binding key went down (Shift before Ctrl+Cmd). Never which one.
    Extra,
    /// A digit 1 to 9 while the picker is open.
    Digit(u8),
    /// Escape while the picker is open, or as a key of the binding.
    Esc,
    Ready,
    Failed,
    Waiting,
}

/// Prism Terminal's `VK` table, verbatim: the named keys, then F1 to F24, A to Z and 0 to 9.
const NAMED: [(&str, u16); 12] = [
    ("ControlLeft", 0xa2),
    ("ControlRight", 0xa3),
    ("AltLeft", 0xa4),
    ("AltRight", 0xa5),
    ("ShiftLeft", 0xa0),
    ("ShiftRight", 0xa1),
    ("MetaLeft", 0x5b),
    ("MetaRight", 0x5c),
    ("Pause", 0x13),
    ("ScrollLock", 0x91),
    ("Space", 0x20),
    ("Backquote", 0xc0),
];

/// The Windows virtual key of a `KeyboardEvent.code`, or None when it has none.
pub fn vk_of_code(code: &str) -> Option<u16> {
    if let Some((_, vk)) = NAMED.iter().find(|(c, _)| *c == code) {
        return Some(*vk);
    }
    if let Some(n) = code.strip_prefix('F').and_then(|r| r.parse::<u16>().ok()) {
        if (1..=24).contains(&n) && code == format!("F{n}") {
            return Some(0x6f + n);
        }
    }
    let bytes = code.as_bytes();
    if bytes.len() == 4 && code.starts_with("Key") && bytes[3].is_ascii_uppercase() {
        return Some(u16::from(bytes[3]));
    }
    if bytes.len() == 6 && code.starts_with("Digit") && bytes[5].is_ascii_digit() {
        return Some(0x30 + u16::from(bytes[5] - b'0'));
    }
    None
}

/// The `KeyboardEvent.code` of a Windows virtual key, or None when the table has none.
pub fn code_of_vk(vk: u16) -> Option<String> {
    match vk {
        0x70..=0x87 => Some(format!("F{}", vk - 0x6f)),
        0x41..=0x5a => Some(format!("Key{}", char::from(vk as u8))),
        0x30..=0x39 => Some(format!("Digit{}", vk - 0x30)),
        _ => NAMED
            .iter()
            .find(|(_, v)| *v == vk)
            .map(|(c, _)| (*c).to_string()),
    }
}

/// Which modifier flag a modifier key sets for itself.
fn own_flag(code: &str) -> Option<&'static str> {
    match code {
        "ControlLeft" | "ControlRight" => Some("ctrl"),
        "AltLeft" | "AltRight" => Some("alt"),
        "ShiftLeft" | "ShiftRight" => Some("shift"),
        "MetaLeft" | "MetaRight" => Some("meta"),
        _ => None,
    }
}

/// The virtual keys the hook must watch for a binding: its key, and both sides of every modifier it
/// names (and of its own key's, for a chord of modifiers). None when the key has no Windows equivalent.
pub fn keys_for(h: &Hotkey) -> Option<Vec<u16>> {
    let main = vk_of_code(&h.code)?;
    let own = own_flag(&h.code);
    let any = h.ctrl || h.alt || h.shift || h.meta;
    let groups: [(&str, bool, [u16; 2]); 4] = [
        ("ctrl", h.ctrl, [0xa2, 0xa3]),
        ("alt", h.alt, [0xa4, 0xa5]),
        ("shift", h.shift, [0xa0, 0xa1]),
        ("meta", h.meta, [0x5b, 0x5c]),
    ];
    let mut out: Vec<u16> = vec![main];
    for (name, on, keys) in groups {
        if on || (own == Some(name) && any) {
            out.extend(keys);
        }
    }
    out.sort_unstable();
    out.dedup();
    Some(out)
}

/// The Start menu needs masking only when the binding holds Win with Ctrl.
pub fn masks_start(h: &Hotkey) -> bool {
    (h.meta || own_flag(&h.code) == Some("meta")) && (h.ctrl || own_flag(&h.code) == Some("ctrl"))
}

/// One line of the hook's text form, or None for anything else. `d 5B` is a down of MetaLeft,
/// `u A2` an up of ControlLeft, `d 5B f 4242` a down with the front window's process id, `o` another
/// key, `p 3` digit 3, `x` Escape, and `ready`, `failed`, `waiting`.
pub fn parse_helper_line(line: &str) -> Option<HookEvent> {
    let t = line.trim();
    match t {
        "ready" => return Some(HookEvent::Ready),
        "failed" => return Some(HookEvent::Failed),
        "waiting" => return Some(HookEvent::Waiting),
        "o" => return Some(HookEvent::Other),
        "m" => return Some(HookEvent::Extra),
        "x" => return Some(HookEvent::Esc),
        _ => {}
    }
    if let Some(d) = t.strip_prefix("p ") {
        let b = d.as_bytes();
        return match b {
            [c @ b'1'..=b'9'] => Some(HookEvent::Digit(c - b'0')),
            _ => None,
        };
    }
    let (down, rest) = match t.strip_prefix("d ") {
        Some(r) => (true, r),
        None => (false, t.strip_prefix("u ")?),
    };
    let (hex, tail) = match rest.split_once(' ') {
        Some((h, tl)) => (h, Some(tl)),
        None => (rest, None),
    };
    if hex.len() != 2
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
    {
        return None;
    }
    let vk = u16::from_str_radix(hex, 16).ok()?;
    let front = match tail {
        None => None,
        Some(tl) => {
            let digits = tl.strip_prefix("f ")?;
            if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            digits
                .parse::<u64>()
                .ok()
                .and_then(|v| u32::try_from(v).ok())
        }
    };
    let code = code_of_vk(vk)?;
    Some(HookEvent::Key {
        down,
        code,
        front: if down { front } else { None },
    })
}

/// The binding's key events as the reducer reads them. The modifier flags are kept here from the downs
/// and ups seen (the hook reports only the binding's keys), and "another key" is a key named `Other`
/// carrying those flags (N16).
#[derive(Debug, Default)]
pub struct KeyFeed {
    state: KeyState,
    held: HashSet<String>,
    /// The last event cost a press to another key or modifier: `"key"` or `"modifier"` (for the diag log).
    cancel: Option<&'static str>,
}

impl KeyFeed {
    pub fn new() -> Self {
        KeyFeed {
            state: KeyState::initial(),
            held: HashSet::new(),
            cancel: None,
        }
    }

    fn held_any(&self, keys: &[&str]) -> bool {
        keys.iter().any(|k| self.held.contains(*k))
    }

    fn chord(&self, code: &str, repeat: bool, at: f64) -> KeyChord {
        KeyChord {
            code: code.to_string(),
            ctrl: self.held_any(&["ControlLeft", "ControlRight"]),
            alt: self.held_any(&["AltLeft", "AltRight"]),
            shift: self.held_any(&["ShiftLeft", "ShiftRight"]),
            meta: self.held_any(&["MetaLeft", "MetaRight"]),
            repeat,
            at,
        }
    }

    fn run(&mut self, evt: KeyEvt, c: &KeyConfig) -> Option<KeyAction> {
        let r = reduce_key(&self.state, &evt, c);
        self.cancel = extra_cancel(&self.state, &r.state, &evt, c);
        self.state = r.state;
        r.action
    }

    /// One hook line's event, at the host's clock. Digits and lines that are not keys do nothing here.
    pub fn feed(&mut self, e: &HookEvent, at: f64, c: &KeyConfig) -> Option<KeyAction> {
        self.cancel = None;
        match e {
            HookEvent::Other => {
                let chord = self.chord("Other", false, at);
                self.run(KeyEvt::Down(chord), c)
            }
            HookEvent::Extra => {
                let chord = self.chord(EXTRA_MODIFIER, false, at);
                self.run(KeyEvt::Down(chord), c)
            }
            HookEvent::Esc => {
                let chord = self.chord("Escape", false, at);
                self.run(KeyEvt::Down(chord), c)
            }
            HookEvent::Key { down, code, .. } => {
                let repeat = *down && self.held.contains(code);
                if *down {
                    self.held.insert(code.clone());
                }
                let chord = self.chord(code, repeat, at);
                if !*down {
                    self.held.remove(code);
                }
                let evt = if *down {
                    KeyEvt::Down(chord)
                } else {
                    KeyEvt::Up(chord)
                };
                self.run(evt, c)
            }
            HookEvent::Digit(_) | HookEvent::Ready | HookEvent::Failed | HookEvent::Waiting => None,
        }
    }

    /// Whether the last event fed cancelled a press because another key or modifier was down: `"key"` or
    /// `"modifier"`, never which. Taken once.
    pub fn take_cancel(&mut self) -> Option<&'static str> {
        self.cancel.take()
    }

    /// The host's tick, one a little after the hold time is up (the reducer decides on it).
    pub fn tick(&mut self, at: f64, c: &KeyConfig) -> Option<KeyAction> {
        self.run(KeyEvt::Tick { at }, c)
    }

    /// Is a bare-modifier press still deciding (it may yet become a dictation)?
    pub fn arming(&self) -> bool {
        self.state.phase == Phase::Arming
    }

    /// Does the feed think a recording is under way (`Start` given, no `Stop` or `Cancel` yet)?
    pub fn listening(&self) -> bool {
        self.state.phase == Phase::Listening
    }

    /// Is any of the binding's keys down, as far as the feed knows (a chord is in progress)?
    pub fn holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// The recording ended without the key (the service refused a start, or the 300 s cap stopped it): the
    /// next press starts again. The keys held stay held, so the chord's own key-up is still the binding's.
    pub fn to_idle(&mut self) {
        self.state.phase = Phase::Idle;
    }

    pub fn reset(&mut self) {
        self.state = KeyState::initial();
        self.held.clear();
        self.cancel = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::{default_hotkey, DictMode};

    fn f9() -> Hotkey {
        Hotkey {
            code: "F9".into(),
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
        }
    }

    fn cfg(hotkey: Hotkey, mode: DictMode) -> KeyConfig {
        KeyConfig {
            hotkey,
            mode,
            hold_ms: None,
        }
    }

    fn key(down: bool, code: &str) -> HookEvent {
        HookEvent::Key {
            down,
            code: code.into(),
            front: None,
        }
    }

    #[test]
    fn keys_for_watches_both_sides_of_each_modifier_of_ctrl_win_and_masks_start() {
        assert_eq!(
            keys_for(&default_hotkey()),
            Some(vec![0x5b, 0x5c, 0xa2, 0xa3])
        );
        assert!(masks_start(&default_hotkey()));
    }

    #[test]
    fn keys_for_watches_a_plain_key_alone_and_a_chord_with_its_modifiers() {
        assert_eq!(keys_for(&f9()), Some(vec![0x78]));
        assert!(!masks_start(&f9()));
        let ctrl_shift_d = Hotkey {
            code: "KeyD".into(),
            ctrl: true,
            alt: false,
            shift: true,
            meta: false,
        };
        assert_eq!(
            keys_for(&ctrl_shift_d),
            Some(vec![0x44, 0xa0, 0xa1, 0xa2, 0xa3])
        );
        let bare_right_alt = Hotkey {
            code: "AltRight".into(),
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
        };
        assert_eq!(keys_for(&bare_right_alt), Some(vec![0xa5]));
    }

    #[test]
    fn keys_for_names_no_key_without_a_virtual_key() {
        let unknown = Hotkey {
            code: "Unidentified".into(),
            ..f9()
        };
        assert_eq!(keys_for(&unknown), None);
    }

    #[test]
    fn parse_helper_line_reads_keys_another_key_digits_and_esc_and_nothing_else() {
        assert_eq!(parse_helper_line("d 5B"), Some(key(true, "MetaLeft")));
        assert_eq!(parse_helper_line("u A2\r"), Some(key(false, "ControlLeft")));
        assert_eq!(parse_helper_line("o"), Some(HookEvent::Other));
        assert_eq!(parse_helper_line("p 3"), Some(HookEvent::Digit(3)));
        assert_eq!(parse_helper_line("x"), Some(HookEvent::Esc));
        assert_eq!(parse_helper_line("p 0"), None);
        assert_eq!(parse_helper_line("d ZZ"), None);
        assert_eq!(
            parse_helper_line("d 5B f 4242"),
            Some(HookEvent::Key {
                down: true,
                code: "MetaLeft".into(),
                front: Some(4242)
            })
        );
        assert_eq!(parse_helper_line("d 5B f"), None);
        assert_eq!(parse_helper_line("waiting"), Some(HookEvent::Waiting));
        assert_eq!(parse_helper_line("hello"), None);
        assert_eq!(parse_helper_line("d 01"), None);
    }

    #[test]
    fn create_key_feed_starts_a_hold_of_ctrl_win_after_the_solo_wait_and_stops_on_release() {
        let c = cfg(default_hotkey(), DictMode::Hold);
        let mut f = KeyFeed::new();
        assert_eq!(f.feed(&key(true, "ControlLeft"), 0.0, &c), None);
        assert_eq!(f.feed(&key(true, "MetaLeft"), 10.0, &c), None);
        assert!(f.arming());
        assert_eq!(f.tick(220.0, &c), Some(KeyAction::Start));
        assert_eq!(
            f.feed(&key(false, "MetaLeft"), 900.0, &c),
            Some(KeyAction::Stop)
        );
    }

    #[test]
    fn create_key_feed_lets_ctrl_win_left_switch_desktops_another_key_while_it_decides_is_no_dictation(
    ) {
        let c = cfg(default_hotkey(), DictMode::Hold);
        let mut f = KeyFeed::new();
        f.feed(&key(true, "ControlLeft"), 0.0, &c);
        f.feed(&key(true, "MetaLeft"), 10.0, &c);
        assert_eq!(f.feed(&HookEvent::Other, 50.0, &c), None);
        assert_eq!(f.tick(220.0, &c), None);
    }

    #[test]
    fn to_idle_makes_the_next_tap_a_start_and_still_swallows_the_held_chords_key_up() {
        let c = cfg(default_hotkey(), DictMode::Toggle);
        let mut f = KeyFeed::new();
        f.feed(&key(true, "ControlLeft"), 0.0, &c);
        f.feed(&key(true, "MetaLeft"), 10.0, &c);
        assert_eq!(
            f.feed(&key(false, "MetaLeft"), 100.0, &c),
            Some(KeyAction::Start)
        );
        assert!(f.listening());
        // A second press is under way (Ctrl+Win down) when the recording ends by itself.
        f.feed(&key(true, "MetaLeft"), 500.0, &c);
        assert!(f.holding());
        f.to_idle();
        assert!(!f.listening());
        assert!(f.holding(), "the keys held stay held");
        // The press's own key-up is still the binding's: swallowed, and it stops nothing.
        let up = KeyEvt::Up(f.chord("MetaLeft", false, 600.0));
        assert!(reduce_key(&f.state, &up, &c).swallow);
        assert_eq!(f.feed(&key(false, "MetaLeft"), 600.0, &c), None);
        assert_eq!(f.feed(&key(false, "ControlLeft"), 610.0, &c), None);
        // The next tap starts.
        f.feed(&key(true, "ControlLeft"), 2000.0, &c);
        f.feed(&key(true, "MetaLeft"), 2010.0, &c);
        assert_eq!(
            f.feed(&key(false, "MetaLeft"), 2100.0, &c),
            Some(KeyAction::Start)
        );
    }

    #[test]
    fn create_key_feed_cancels_a_recording_on_esc() {
        let c = cfg(f9(), DictMode::Toggle);
        let mut f = KeyFeed::new();
        assert_eq!(f.feed(&key(true, "F9"), 0.0, &c), Some(KeyAction::Start));
        f.feed(&key(false, "F9"), 50.0, &c);
        assert_eq!(f.feed(&HookEvent::Esc, 400.0, &c), Some(KeyAction::Cancel));
    }

    #[test]
    fn a_modifier_the_binding_does_not_name_cancels_a_ctrl_win_press_in_hold_and_toggle() {
        // The hook's lines for Cmd+Shift+S as Karabiner's Cmd+Ctrl+Shift+4 (Ctrl+Win+Shift+S on Windows):
        // Win down, Ctrl down, then "a modifier the binding does not name is down"; the S or 4 may never come.
        for mode in [DictMode::Hold, DictMode::Toggle] {
            let c = cfg(default_hotkey(), mode);
            let mut f = KeyFeed::new();
            assert_eq!(f.feed(&key(true, "MetaLeft"), 0.0, &c), None);
            assert_eq!(f.take_cancel(), None);
            assert_eq!(f.feed(&key(true, "ControlLeft"), 10.0, &c), None);
            assert_eq!(f.feed(&HookEvent::Extra, 10.0, &c), None);
            assert_eq!(f.take_cancel(), Some("modifier"));
            assert_eq!(f.take_cancel(), None, "taken once");
            assert!(!f.arming() && !f.listening(), "{mode:?}");
            assert_eq!(f.tick(220.0, &c), None);
            assert_eq!(f.feed(&key(false, "ControlLeft"), 300.0, &c), None);
            assert_eq!(f.feed(&key(false, "MetaLeft"), 310.0, &c), None);
            // Ctrl+Win alone afterwards still works.
            f.feed(&key(true, "ControlLeft"), 2000.0, &c);
            f.feed(&key(true, "MetaLeft"), 2010.0, &c);
            let started = if mode == DictMode::Hold {
                f.tick(2220.0, &c)
            } else {
                f.feed(&key(false, "MetaLeft"), 2100.0, &c)
            };
            assert_eq!(started, Some(KeyAction::Start), "{mode:?}");
        }
        // Another key is a key.
        let c = cfg(default_hotkey(), DictMode::Toggle);
        let mut f = KeyFeed::new();
        f.feed(&key(true, "ControlLeft"), 0.0, &c);
        f.feed(&key(true, "MetaLeft"), 10.0, &c);
        f.feed(&HookEvent::Other, 30.0, &c);
        assert_eq!(f.take_cancel(), Some("key"));
        assert_eq!(f.feed(&key(false, "MetaLeft"), 100.0, &c), None);
        assert_eq!(parse_helper_line("m"), Some(HookEvent::Extra));
    }
}
