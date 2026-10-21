//! The hotkey reducer: hold or toggle, the solo rules (Task 4). Ported from Prism Terminal's
//! `core/renderer/lib/dictationKey.ts` at e5848cb.
//!
//! WHY IT IS NOT A PLAIN KEYDOWN. Right Alt is AltGr on Norwegian and most European layouts, the key
//! that types @ { } [ ] \ | ~. Windows reports AltGr as ControlLeft DOWN immediately followed by
//! AltRight DOWN, so a hotkey that fired on the down would start a recording every time somebody typed
//! an e-mail address. A BARE MODIFIER (or a chord of modifiers, the default Ctrl+Win) is therefore read
//! as a SOLO HOLD (hold mode) or a SOLO TAP (toggle mode): the key alone, with nothing else pressed
//! while it decides. A chord or a plain key (Ctrl+Shift+D, F9) acts on its down.
//!
//! The reducer keeps no clock of its own: every event carries `at` (milliseconds on the host's clock),
//! and the host sends a `Tick` when the hold time is up.
//!
//! Codes are W3C `KeyboardEvent.code` strings (`AltRight`, `ControlLeft`, `MetaLeft`, `KeyR`, `Digit2`,
//! `F9`, `Escape`): the binding is by position and does not move with the layout (O2).

use serde::{Deserialize, Serialize};

/// A key and the modifiers held with it. The stored form is `{"code":..,"ctrl":..,"alt":..,"shift":..,
/// "meta":..}`, in that field order, as Prism Terminal writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    pub code: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

/// Ctrl+Win (owner, 2026-10-16): right Alt is AltGr on a Norwegian keyboard. A chord of modifiers:
/// either key may go first, and it follows the solo rules, so Ctrl+Win+Left (switch desktops) never
/// dictates. A user's own saved key is kept; only the default moved.
pub fn default_hotkey() -> Hotkey {
    Hotkey {
        code: "MetaLeft".into(),
        ctrl: true,
        alt: false,
        shift: false,
        meta: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictMode {
    Hold,
    Toggle,
}

/// One key going down or up, with the modifiers held and the host's clock.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyChord {
    pub code: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub repeat: bool,
    pub at: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeyEvt {
    Down(KeyChord),
    Up(KeyChord),
    Tick { at: f64 },
    Blur { at: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing is recording.
    Idle,
    /// The bare-modifier hotkey is down and still alone, and nothing has been started yet.
    Arming,
    /// `Start` has been given and neither `Stop` nor `Cancel` has.
    Listening,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyState {
    pub phase: Phase,
    /// The hotkey is physically down as far as the reducer knows. It outlives the phase on purpose.
    held: bool,
    /// When the press that set `held` went down.
    down_at: f64,
    /// Another key went down during this press, so it is not a solo tap.
    polluted: bool,
    /// This press arrived carrying Ctrl, which is how AltGr arrives.
    alt_gr: bool,
    /// The Escape that cancelled is still down; its up is swallowed with it.
    esc_held: bool,
}

impl KeyState {
    pub fn initial() -> Self {
        KeyState {
            phase: Phase::Idle,
            held: false,
            down_at: 0.0,
            polluted: false,
            alt_gr: false,
            esc_held: false,
        }
    }
}

impl Default for KeyState {
    fn default() -> Self {
        Self::initial()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Start,
    Stop,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct KeyConfig {
    pub hotkey: Hotkey,
    pub mode: DictMode,
    /// How long a bare modifier must be held alone before it records. Default 200.
    pub hold_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyResult {
    pub state: KeyState,
    pub action: Option<KeyAction>,
    /// The host must stop the event reaching anything else.
    pub swallow: bool,
}

pub const DEFAULT_HOLD_MS: f64 = 200.0;
/// The host's tick: one tick a little after the hold time is up.
pub const HOST_TICK_MS: u64 = 210;
pub const ALTGR_PAIR_MS: f64 = 50.0;
pub const TAP_MAX_MS: f64 = 1000.0;

const FLAGS: [Flag; 4] = [Flag::Ctrl, Flag::Alt, Flag::Shift, Flag::Meta];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Ctrl,
    Alt,
    Shift,
    Meta,
}

/// The flag a modifier key sets for itself.
fn modifier_flag(code: &str) -> Option<Flag> {
    match code {
        "ControlLeft" | "ControlRight" => Some(Flag::Ctrl),
        "AltLeft" | "AltRight" => Some(Flag::Alt),
        "ShiftLeft" | "ShiftRight" => Some(Flag::Shift),
        "MetaLeft" | "MetaRight" => Some(Flag::Meta),
        _ => None,
    }
}

fn hotkey_flag(h: &Hotkey, f: Flag) -> bool {
    match f {
        Flag::Ctrl => h.ctrl,
        Flag::Alt => h.alt,
        Flag::Shift => h.shift,
        Flag::Meta => h.meta,
    }
}

fn chord_flag(c: &KeyChord, f: Flag) -> bool {
    match f {
        Flag::Ctrl => c.ctrl,
        Flag::Alt => c.alt,
        Flag::Shift => c.shift,
        Flag::Meta => c.meta,
    }
}

/// A modifier bound on its own.
fn is_bare_modifier(h: &Hotkey) -> bool {
    modifier_flag(&h.code).is_some() && !h.ctrl && !h.alt && !h.shift && !h.meta
}

/// The modifiers a binding made only of modifiers holds down: its own key's flag and every flag it
/// names (Ctrl+Win: ctrl and meta).
fn chord_set(h: &Hotkey) -> [bool; 4] {
    let mut set = FLAGS.map(|f| hotkey_flag(h, f));
    if let Some(own) = modifier_flag(&h.code) {
        set[own as usize] = true;
    }
    set
}

/// Only modifiers: a bare one (Right Alt) or a chord of them (Ctrl+Win). The SOLO rules apply to
/// both: it decides while nothing else is pressed, so a key typed with it (AltGr's @,
/// Ctrl+Win+Left) is never dictation.
fn is_solo(h: &Hotkey) -> bool {
    modifier_flag(&h.code).is_some()
}

/// A key of this solo binding: the bare key itself, or any key of a chord's modifiers, left or right.
fn is_solo_key(code: &str, h: &Hotkey) -> bool {
    if is_bare_modifier(h) {
        return code == h.code;
    }
    match modifier_flag(code) {
        Some(f) => chord_set(h)[f as usize],
        None => false,
    }
}

impl KeyEvt {
    fn at(&self) -> f64 {
        match self {
            KeyEvt::Down(c) | KeyEvt::Up(c) => c.at,
            KeyEvt::Tick { at } | KeyEvt::Blur { at } => *at,
        }
    }
}

/// Is this event the hotkey?
///
/// An UP matches on the code alone: by the time the main key of a chord comes up its modifiers may
/// already have, and the up must still be recognised. A DOWN of a bare modifier ignores the flag the
/// key sets for itself and wants every OTHER modifier up; the one exception is Ctrl on AltRight, which
/// is AltGr. A DOWN of a chord or a plain key matches its modifiers exactly.
pub fn is_hotkey_event(e: &KeyEvt, h: &Hotkey) -> bool {
    let (c, is_up) = match e {
        KeyEvt::Down(c) => (c, false),
        KeyEvt::Up(c) => (c, true),
        KeyEvt::Tick { .. } | KeyEvt::Blur { .. } => return false,
    };
    chord_matches(c, is_up, h)
}

fn chord_matches(c: &KeyChord, is_up: bool, h: &Hotkey) -> bool {
    // A chord of modifiers: either of its keys, the moment the held set is exactly the chord.
    if is_solo(h) && !is_bare_modifier(h) {
        if !is_solo_key(&c.code, h) {
            return false;
        }
        if is_up {
            return true;
        }
        let set = chord_set(h);
        return FLAGS.iter().all(|&f| chord_flag(c, f) == set[f as usize]);
    }
    if c.code != h.code {
        return false;
    }
    if is_up {
        return true;
    }
    if !is_bare_modifier(h) {
        return FLAGS.iter().all(|&f| chord_flag(c, f) == hotkey_flag(h, f));
    }
    let own = modifier_flag(&h.code);
    FLAGS
        .iter()
        .all(|&f| Some(f) == own || !chord_flag(c, f) || (f == Flag::Ctrl && h.code == "AltRight"))
}

/// One event in, the next state out, plus what the host must DO about it.
///
/// What the host owes the reducer: every key down and up (before anything else sees it), a `Blur`
/// when the window loses focus, and one `Tick` at `down_at + hold_ms` while the phase is `Arming`.
/// Hold mode starts ONLY on an event at or after `hold_ms` while the key is still down; a release that
/// no tick came before is only a release.
pub fn reduce_key(state: &KeyState, evt: &KeyEvt, cfg: &KeyConfig) -> KeyResult {
    let hold_ms = cfg.hold_ms.unwrap_or(DEFAULT_HOLD_MS);
    // Solo: a bare modifier or a chord of modifiers (Ctrl+Win) decides alone.
    let bare = is_solo(&cfg.hotkey);
    let stay = |swallow: bool| KeyResult {
        state: *state,
        action: None,
        swallow,
    };
    let with = |phase: Phase, action: Option<KeyAction>, swallow: bool| KeyResult {
        state: KeyState { phase, ..*state },
        action,
        swallow,
    };
    // Hold mode's wait is over: the key has been down, alone, for hold_ms.
    let ripe = bare
        && cfg.mode == DictMode::Hold
        && state.phase == Phase::Arming
        && evt.at() - state.down_at >= hold_ms;

    let (k, is_up) = match evt {
        KeyEvt::Blur { .. } => {
            // The up will never arrive in a window that is not looking, so nothing may be left held.
            // A recording that loses its window is cancelled rather than pasted.
            return KeyResult {
                state: KeyState::initial(),
                action: (state.phase == Phase::Listening).then_some(KeyAction::Cancel),
                swallow: false,
            };
        }
        KeyEvt::Tick { at } => {
            if ripe {
                return with(Phase::Listening, Some(KeyAction::Start), false);
            }
            // Toggle mode: held past a tap is not a tap. The key stays `held`, so its up is still ours.
            if state.phase == Phase::Arming
                && cfg.mode == DictMode::Toggle
                && *at - state.down_at >= TAP_MAX_MS
            {
                return KeyResult {
                    state: KeyState {
                        phase: Phase::Idle,
                        polluted: true,
                        ..*state
                    },
                    action: None,
                    swallow: false,
                };
            }
            return stay(false);
        }
        KeyEvt::Down(k) => (k, false),
        KeyEvt::Up(k) => (k, true),
    };

    let own = if bare {
        is_solo_key(&k.code, &cfg.hotkey)
    } else {
        k.code == cfg.hotkey.code
    };

    if is_up {
        if own && state.held {
            return release(state, k.at, cfg.mode, bare);
        }
        if k.code == "Escape" && state.esc_held {
            return KeyResult {
                state: KeyState {
                    esc_held: false,
                    ..*state
                },
                action: None,
                swallow: true,
            };
        }
        // Any other up, the synthetic ControlLeft's included, says nothing: only a DOWN is somebody
        // pressing a second key.
        return stay(false);
    }

    // ---- a key going down ----

    if own && state.held {
        // Auto-repeat of the hotkey (either key of a chord): part of the same press. It never arms
        // again, but it can carry the start if the host's tick is late.
        if ripe {
            return with(Phase::Listening, Some(KeyAction::Start), true);
        }
        return stay(true);
    }

    if !k.repeat && chord_matches(k, false, &cfg.hotkey) {
        return press(state, k, cfg.mode, bare);
    }

    if k.code == "Escape" && state.phase == Phase::Listening {
        // `polluted` so that in toggle mode an Escape pressed MID-TAP does not let the tap's own up
        // start a fresh recording a moment after the cancel.
        return KeyResult {
            state: KeyState {
                phase: Phase::Idle,
                polluted: true,
                esc_held: true,
                ..*state
            },
            action: Some(KeyAction::Cancel),
            swallow: true,
        };
    }

    if state.held && bare {
        // AltGr's other half is not a second key. On those layouts it precedes AltRight, repeats
        // beside it for as long as the pair is held, and is forgiven a moment after the down as well
        // in case the order ever flips.
        let alt_gr_ctrl = cfg.hotkey.code == "AltRight"
            && k.code == "ControlLeft"
            && (state.alt_gr || k.at - state.down_at <= ALTGR_PAIR_MS);
        if alt_gr_ctrl {
            return stay(false);
        }
        if state.phase == Phase::Arming {
            // Escape means "no" wherever it lands: a late tick must not turn it into a start.
            if k.code == "Escape" {
                return KeyResult {
                    state: KeyState {
                        phase: Phase::Idle,
                        polluted: true,
                        ..*state
                    },
                    action: None,
                    swallow: false,
                };
            }
            // Held alone for the whole of hold_ms, THEN a key: that is dictation that began, not
            // AltGr typing. Before it: AltGr typing, cancelled without a word, and the key that gave
            // it away goes on untouched.
            if ripe {
                return KeyResult {
                    state: KeyState {
                        phase: Phase::Listening,
                        polluted: true,
                        ..*state
                    },
                    action: Some(KeyAction::Start),
                    swallow: false,
                };
            }
            return KeyResult {
                state: KeyState {
                    phase: Phase::Idle,
                    polluted: true,
                    ..*state
                },
                action: None,
                swallow: false,
            };
        }
        // While LISTENING a brushed key cancels nothing. It only spoils a toggle tap in progress.
        if !state.polluted {
            return KeyResult {
                state: KeyState {
                    polluted: true,
                    ..*state
                },
                action: None,
                swallow: false,
            };
        }
    }

    stay(false)
}

/// A fresh, matching, non-repeat down of the hotkey.
fn press(state: &KeyState, k: &KeyChord, mode: DictMode, bare: bool) -> KeyResult {
    let held = KeyState {
        held: true,
        down_at: k.at,
        polluted: false,
        alt_gr: bare && k.code == "AltRight" && k.ctrl,
        ..*state
    };
    if bare {
        // Nothing is decided yet in either mode. From idle the press is arming; while listening (a
        // toggle's second tap) the phase holds until the up.
        let phase = if state.phase == Phase::Idle {
            Phase::Arming
        } else {
            state.phase
        };
        return KeyResult {
            state: KeyState { phase, ..held },
            action: None,
            swallow: true,
        };
    }
    if state.phase == Phase::Listening {
        // Toggle: the second press stops. Hold: a press while already listening can only follow an up
        // that was never seen; keep listening and let this up stop it.
        if mode == DictMode::Toggle {
            return KeyResult {
                state: KeyState {
                    phase: Phase::Idle,
                    ..held
                },
                action: Some(KeyAction::Stop),
                swallow: true,
            };
        }
        return KeyResult {
            state: held,
            action: None,
            swallow: true,
        };
    }
    KeyResult {
        state: KeyState {
            phase: Phase::Listening,
            ..held
        },
        action: Some(KeyAction::Start),
        swallow: true,
    }
}

/// The up of a hotkey the reducer saw go down. Always swallowed: an unswallowed Alt keyup is what
/// opens the window's system menu.
fn release(state: &KeyState, at: f64, mode: DictMode, bare: bool) -> KeyResult {
    let next = KeyState {
        held: false,
        polluted: false,
        alt_gr: false,
        ..*state
    };
    let idle = KeyState {
        phase: Phase::Idle,
        ..next
    };
    if mode == DictMode::Hold {
        // Arming: let go before hold_ms. Idle: the press was cancelled by Escape or by AltGr typing.
        // Neither stops anything.
        if state.phase == Phase::Listening {
            return KeyResult {
                state: idle,
                action: Some(KeyAction::Stop),
                swallow: true,
            };
        }
        return KeyResult {
            state: idle,
            action: None,
            swallow: true,
        };
    }
    if !bare {
        return KeyResult {
            state: next,
            action: None,
            swallow: true,
        };
    }
    let tap = !state.polluted && at - state.down_at < TAP_MAX_MS;
    if state.phase == Phase::Arming {
        return if tap {
            KeyResult {
                state: KeyState {
                    phase: Phase::Listening,
                    ..next
                },
                action: Some(KeyAction::Start),
                swallow: true,
            }
        } else {
            KeyResult {
                state: idle,
                action: None,
                swallow: true,
            }
        };
    }
    if state.phase == Phase::Listening && tap {
        return KeyResult {
            state: idle,
            action: Some(KeyAction::Stop),
            swallow: true,
        };
    }
    KeyResult {
        state: next,
        action: None,
        swallow: true,
    }
}

// ---- naming and binding ----

fn side_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "ControlLeft" => "Left Ctrl",
        "ControlRight" => "Right Ctrl",
        "AltLeft" => "Left Alt",
        "AltRight" => "Right Alt",
        "ShiftLeft" => "Left Shift",
        "ShiftRight" => "Right Shift",
        "MetaLeft" => "Left Win",
        "MetaRight" => "Right Win",
        _ => return None,
    })
}

fn punctuation_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "Backquote" => "`",
        "Minus" => "-",
        "Equal" => "=",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Backslash" | "IntlBackslash" => "\\",
        "Semicolon" => ";",
        "Quote" => "'",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "ArrowUp" => "Up",
        "ArrowDown" => "Down",
        "ArrowLeft" => "Left",
        "ArrowRight" => "Right",
        "PageUp" => "Page Up",
        "PageDown" => "Page Down",
        "PrintScreen" => "Print Screen",
        "ScrollLock" => "Scroll Lock",
        "CapsLock" => "Caps Lock",
        "NumLock" => "Num Lock",
        "ContextMenu" => "Menu",
        "NumpadAdd" => "Num +",
        "NumpadSubtract" => "Num -",
        "NumpadMultiply" => "Num *",
        "NumpadDivide" => "Num /",
        "NumpadDecimal" => "Num .",
        "NumpadEnter" => "Num Enter",
        _ => return None,
    })
}

/// The key's name by POSITION (a US legend): the binding is by position, so it cannot know what the
/// user's layout prints on that cap, and must not guess.
fn key_name(code: &str) -> String {
    if let Some(n) = side_name(code).or_else(|| punctuation_name(code)) {
        return n.to_string();
    }
    for (prefix, label) in [("Key", ""), ("Digit", ""), ("Numpad", "Num ")] {
        if let Some(rest) = code.strip_prefix(prefix) {
            if !rest.is_empty() {
                return format!("{label}{rest}");
            }
        }
    }
    code.to_string()
}

/// 'Right Alt', 'Ctrl+Shift+D': what Settings shows for a binding.
pub fn format_hotkey(h: &Hotkey) -> String {
    // A chord of modifiers is its modifiers: 'Ctrl+Win', not 'Ctrl+Win+Left Win'.
    if is_solo(h) && !is_bare_modifier(h) {
        let set = chord_set(h);
        return ["Ctrl", "Alt", "Shift", "Win"]
            .iter()
            .enumerate()
            .filter(|(i, _)| set[*i])
            .map(|(_, n)| *n)
            .collect::<Vec<_>>()
            .join("+");
    }
    let mut parts: Vec<String> = Vec::new();
    if h.ctrl {
        parts.push("Ctrl".into());
    }
    if h.alt {
        parts.push("Alt".into());
    }
    if h.shift {
        parts.push("Shift".into());
    }
    if h.meta {
        parts.push("Win".into());
    }
    parts.push(key_name(&h.code));
    parts.join("+")
}

/// What this key event would be as a binding, for the rebind control.
///
/// A modifier's own event is that modifier BARE, its flags dropped: its own flag is the key itself,
/// and the Ctrl that rides on AltRight is AltGr, so the Norwegian Right Alt binds as Right Alt rather
/// than as Ctrl+Right Alt. A modifier with OTHER modifiers held is a chord of them (Ctrl+Win). Escape
/// is `None`: it is how the user leaves the control, and as a binding it would take away the one key
/// that cancels a recording.
pub fn parse_hotkey_from_event(e: &KeyChord) -> Option<Hotkey> {
    if e.code.is_empty() || e.code == "Unidentified" || e.code == "Escape" {
        return None;
    }
    if let Some(own) = modifier_flag(&e.code) {
        let others = FLAGS
            .iter()
            .any(|&f| f != own && chord_flag(e, f) && !(f == Flag::Ctrl && e.code == "AltRight"));
        if !others {
            return Some(Hotkey {
                code: e.code.clone(),
                ctrl: false,
                alt: false,
                shift: false,
                meta: false,
            });
        }
        return Some(Hotkey {
            code: e.code.clone(),
            ctrl: e.ctrl || own == Flag::Ctrl,
            alt: e.alt || own == Flag::Alt,
            shift: e.shift || own == Flag::Shift,
            meta: e.meta || own == Flag::Meta,
        });
    }
    Some(Hotkey {
        code: e.code.clone(),
        ctrl: e.ctrl,
        alt: e.alt,
        shift: e.shift,
        meta: e.meta,
    })
}

/// Whether a key may be the dictation key at all. Dictation swallows its key in every shell, so a key
/// that TYPES (a letter, Enter, Space, a Shift+letter) would stop the terminal from typing it, and a
/// chord the terminal owns (Ctrl+C, Ctrl+V, the tab chords) would take that job away. Allowed: a bare
/// modifier, a chord of modifiers, a function key or Pause / Scroll Lock alone, or a chord with Ctrl,
/// Alt or Win that the terminal does not already use. `app_keys` are Flare's own chords: none of them
/// may be the dictation key either.
pub fn usable_hotkey(h: &Hotkey, app_keys: &[Hotkey]) -> bool {
    // Modifiers alone type nothing: a bare one, or a chord of two or more.
    if is_solo(h) {
        return true;
    }
    if app_keys.iter().any(|k| k == h) {
        return false;
    }
    if !h.ctrl && !h.alt && !h.meta {
        return is_function_key(&h.code) || h.code == "Pause" || h.code == "ScrollLock";
    }
    if h.ctrl && !h.alt && !h.meta && is_terminal_chord_key(&h.code) {
        return false;
    }
    true
}

/// `F1` to `F24`.
fn is_function_key(code: &str) -> bool {
    match code.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        Some(n) => (1..=24).contains(&n) && code[1..].chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// The keys whose Ctrl chord the terminal owns: Ctrl+C V W T F, Tab, 1 to 9, Comma.
fn is_terminal_chord_key(code: &str) -> bool {
    matches!(
        code,
        "KeyC" | "KeyV" | "KeyW" | "KeyT" | "KeyF" | "Tab" | "Comma"
    ) || matches!(
        code.strip_prefix("Digit").map(str::as_bytes),
        Some([b'1'..=b'9'])
    )
}
#[cfg(test)]
mod tests {
    use super::*;

    /// The bare Right Alt the solo rules were written for (the default until 2026-10-16).
    fn right_alt() -> Hotkey {
        hk("AltRight", "")
    }

    /// A hotkey from a code and flags: c ctrl, a alt, s shift, m meta.
    fn hk(code: &str, f: &str) -> Hotkey {
        Hotkey {
            code: code.into(),
            ctrl: f.contains('c'),
            alt: f.contains('a'),
            shift: f.contains('s'),
            meta: f.contains('m'),
        }
    }

    fn chord(code: &str, at: f64, f: &str) -> KeyChord {
        KeyChord {
            code: code.into(),
            ctrl: f.contains('c'),
            alt: f.contains('a'),
            shift: f.contains('s'),
            meta: f.contains('m'),
            repeat: f.contains('r'),
            at,
        }
    }

    /// flags: c ctrl, a alt, s shift, m meta, r repeat.
    fn down(code: &str, at: f64, f: &str) -> KeyEvt {
        KeyEvt::Down(chord(code, at, f))
    }
    fn up(code: &str, at: f64, f: &str) -> KeyEvt {
        KeyEvt::Up(chord(code, at, f))
    }
    fn tick(at: f64) -> KeyEvt {
        KeyEvt::Tick { at }
    }
    fn blur(at: f64) -> KeyEvt {
        KeyEvt::Blur { at }
    }
    /// Right Alt as a US layout reports it: the key carries its own alt flag.
    fn ralt_down(at: f64, f: &str) -> KeyEvt {
        down("AltRight", at, &format!("a{f}"))
    }
    fn ralt_up(at: f64) -> KeyEvt {
        up("AltRight", at, "")
    }

    #[derive(Debug, PartialEq)]
    struct Step {
        action: Option<KeyAction>,
        swallow: bool,
        phase: Phase,
    }

    struct Run {
        steps: Vec<Step>,
        state: KeyState,
    }

    impl Run {
        fn actions(&self) -> Vec<KeyAction> {
            self.steps.iter().filter_map(|s| s.action).collect()
        }
        fn step_actions(&self) -> Vec<Option<KeyAction>> {
            self.steps.iter().map(|s| s.action).collect()
        }
        fn phases(&self) -> Vec<Phase> {
            self.steps.iter().map(|s| s.phase).collect()
        }
        fn swallows(&self) -> Vec<bool> {
            self.steps.iter().map(|s| s.swallow).collect()
        }
    }

    /// Feed a whole sequence through the reducer and keep what each event answered.
    fn run_with(events: &[KeyEvt], mode: DictMode, hotkey: Hotkey, hold_ms: Option<f64>) -> Run {
        let cfg = KeyConfig {
            hotkey,
            mode,
            hold_ms,
        };
        let mut state = KeyState::initial();
        let mut steps = Vec::new();
        for e in events {
            let r = reduce_key(&state, e, &cfg);
            state = r.state;
            steps.push(Step {
                action: r.action,
                swallow: r.swallow,
                phase: r.state.phase,
            });
        }
        Run { steps, state }
    }

    fn run(events: &[KeyEvt], mode: DictMode) -> Run {
        run_with(events, mode, right_alt(), None)
    }

    use DictMode::{Hold, Toggle};
    use KeyAction::{Cancel, Start, Stop};
    use Phase::{Arming, Idle, Listening};

    fn chord_kd() -> Hotkey {
        hk("KeyD", "cs")
    }
    fn f9() -> Hotkey {
        hk("F9", "")
    }

    // ---- the default ----

    #[test]
    fn is_ctrl_win() {
        assert_eq!(default_hotkey(), hk("MetaLeft", "cm"));
        assert_eq!(format_hotkey(&default_hotkey()), "Ctrl+Win");
        assert!(usable_hotkey(&default_hotkey(), &[]));
    }

    #[test]
    fn starts_idle() {
        assert_eq!(KeyState::initial().phase, Idle);
    }

    // ---- hold mode, bare modifier ----

    #[test]
    fn a_clean_250_ms_hold_starts_then_stops_on_release() {
        let r = run(
            &[ralt_down(0.0, ""), tick(100.0), tick(200.0), ralt_up(250.0)],
            Hold,
        );
        assert_eq!(r.phases(), vec![Arming, Arming, Listening, Idle]);
        assert_eq!(r.actions(), vec![Start, Stop]);
    }

    #[test]
    fn starts_on_the_tick_at_exactly_hold_ms_and_not_one_millisecond_sooner() {
        let r = run(&[ralt_down(1000.0, ""), tick(1199.0), tick(1200.0)], Hold);
        assert_eq!(r.step_actions(), vec![None, None, Some(Start)]);
    }

    #[test]
    fn honours_a_hold_ms_of_its_own() {
        let r = run_with(
            &[ralt_down(0.0, ""), tick(200.0), tick(350.0)],
            Hold,
            right_alt(),
            Some(350.0),
        );
        assert_eq!(r.step_actions(), vec![None, None, Some(Start)]);
    }

    #[test]
    fn the_norwegian_altgr_sequence_types_an_at_and_never_starts() {
        // Windows reports AltGr as ControlLeft then AltRight, both flags up on the second.
        let t = 5000.0;
        let r = run(
            &[
                down("ControlLeft", t, "c"),
                down("AltRight", t + 1.0, "ca"),
                tick(t + 50.0),
                down("Digit2", t + 80.0, "ca"),
                up("Digit2", t + 140.0, "ca"),
                tick(t + 300.0),
                up("ControlLeft", t + 320.0, "a"),
                up("AltRight", t + 321.0, ""),
                tick(t + 600.0),
            ],
            Hold,
        );
        assert_eq!(r.actions(), vec![]);
        assert_eq!(r.state.phase, Idle);
        // The @ itself must reach the shell: neither half of Digit2 is swallowed.
        assert!(!r.steps[3].swallow);
        assert!(!r.steps[4].swallow);
        // The synthetic Ctrl is not the hotkey either.
        assert!(!r.steps[0].swallow);
    }

    #[test]
    fn altgr_held_alone_on_that_layout_still_dictates_its_own_ctrl_is_not_another_key() {
        let r = run(
            &[
                down("ControlLeft", 0.0, "c"),
                down("AltRight", 1.0, "ca"),
                // Auto-repeat of the pair while it is held.
                down("ControlLeft", 40.0, "car"),
                down("AltRight", 41.0, "car"),
                down("ControlLeft", 120.0, "car"),
                tick(201.0),
                up("ControlLeft", 900.0, "a"),
                up("AltRight", 901.0, ""),
            ],
            Hold,
        );
        assert_eq!(r.actions(), vec![Start, Stop]);
    }

    #[test]
    fn a_synthetic_ctrl_landing_just_after_right_alt_is_forgiven_too() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                down("ControlLeft", 3.0, "ca"),
                tick(200.0),
            ],
            Hold,
        );
        assert_eq!(r.actions(), vec![Start]);
    }

    #[test]
    fn a_real_left_ctrl_pressed_well_into_the_arming_cancels_it() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                down("ControlLeft", 120.0, "ca"),
                tick(200.0),
                tick(400.0),
            ],
            Hold,
        );
        assert_eq!(r.actions(), vec![]);
        assert_eq!(r.state.phase, Idle);
    }

    #[test]
    fn another_key_during_arming_cancels_silently_and_that_key_is_not_swallowed() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                down("KeyQ", 90.0, "a"),
                tick(200.0),
                tick(500.0),
                ralt_up(600.0),
            ],
            Hold,
        );
        assert_eq!(r.actions(), vec![]);
        assert_eq!(
            r.steps[1],
            Step {
                action: None,
                swallow: false,
                phase: Idle
            }
        );
        // The hotkey's own up is still ours, cancelled or not.
        assert!(r.steps[4].swallow);
    }

    #[test]
    fn does_not_re_arm_from_the_auto_repeat_of_a_press_that_was_cancelled() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                down("KeyQ", 50.0, "a"),
                ralt_down(80.0, "r"),
                ralt_down(110.0, ""),
                tick(400.0),
            ],
            Hold,
        );
        assert_eq!(r.actions(), vec![]);
        assert_eq!(r.state.phase, Idle);
        assert!(r.steps[2].swallow);
    }

    #[test]
    fn release_during_arming_goes_back_to_idle_with_no_action() {
        let r = run(&[ralt_down(0.0, ""), ralt_up(120.0), tick(300.0)], Hold);
        assert_eq!(r.actions(), vec![]);
        assert_eq!(
            r.steps[1],
            Step {
                action: None,
                swallow: true,
                phase: Idle
            }
        );
    }

    #[test]
    fn a_release_nobody_ticked_before_is_still_only_a_release() {
        // The host's timer never fired, so nothing was started and nothing may stop.
        let r = run(&[ralt_down(0.0, ""), ralt_up(400.0)], Hold);
        assert_eq!(r.actions(), vec![]);
    }

    #[test]
    fn auto_repeat_of_the_hotkey_is_swallowed_changes_nothing_and_can_carry_the_start() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                ralt_down(30.0, "r"),
                ralt_down(230.0, "r"),
                ralt_up(500.0),
            ],
            Hold,
        );
        assert_eq!(r.step_actions(), vec![None, None, Some(Start), Some(Stop)]);
        assert!(r.steps.iter().all(|s| s.swallow));
    }

    #[test]
    fn another_key_while_listening_does_not_cancel_and_is_not_swallowed() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                tick(200.0),
                down("KeyJ", 900.0, "a"),
                up("KeyJ", 950.0, ""),
                ralt_up(1500.0),
            ],
            Hold,
        );
        assert_eq!(r.actions(), vec![Start, Stop]);
        assert_eq!(
            r.steps[2],
            Step {
                action: None,
                swallow: false,
                phase: Listening
            }
        );
    }

    #[test]
    fn escape_while_listening_cancels_and_is_swallowed_down_and_up() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                tick(200.0),
                down("Escape", 700.0, "a"),
                up("Escape", 760.0, ""),
                ralt_up(900.0),
            ],
            Hold,
        );
        assert_eq!(r.actions(), vec![Start, Cancel]);
        assert_eq!(
            r.steps[2],
            Step {
                action: Some(Cancel),
                swallow: true,
                phase: Idle
            }
        );
        assert!(r.steps[3].swallow);
        // The key is still down after the cancel: its up is ours and stops nothing.
        assert_eq!(
            r.steps[4],
            Step {
                action: None,
                swallow: true,
                phase: Idle
            }
        );
    }

    #[test]
    fn escape_when_nothing_is_listening_is_an_ordinary_key() {
        let r = run(&[down("Escape", 0.0, ""), up("Escape", 50.0, "")], Hold);
        assert!(r.steps.iter().all(|s| !s.swallow && s.action.is_none()));
    }

    #[test]
    fn blur_while_listening_cancels() {
        let r = run(&[ralt_down(0.0, ""), tick(200.0), blur(800.0)], Hold);
        assert_eq!(r.actions(), vec![Start, Cancel]);
        assert_eq!(r.state.phase, Idle);
    }

    #[test]
    fn blur_while_arming_goes_to_idle_without_a_word() {
        let r = run(&[ralt_down(0.0, ""), blur(100.0), tick(300.0)], Hold);
        assert_eq!(r.actions(), vec![]);
        assert_eq!(r.state.phase, Idle);
    }

    #[test]
    fn an_up_the_reducer_never_saw_the_down_of_is_not_swallowed() {
        let r = run(&[ralt_up(10.0)], Hold);
        assert_eq!(
            r.steps[0],
            Step {
                action: None,
                swallow: false,
                phase: Idle
            }
        );
    }

    #[test]
    fn shift_already_down_means_it_is_not_the_solo_hotkey() {
        let r = run(
            &[
                down("ShiftLeft", 0.0, "s"),
                ralt_down(20.0, "s"),
                tick(400.0),
            ],
            Hold,
        );
        assert_eq!(r.actions(), vec![]);
        assert!(!r.steps[1].swallow);
    }

    #[test]
    fn ticks_while_idle_do_nothing() {
        let r = run(&[tick(0.0), tick(1000.0)], Hold);
        assert!(r
            .steps
            .iter()
            .all(|s| s.action.is_none() && !s.swallow && s.phase == Idle));
    }

    // ---- toggle mode, bare modifier ----

    #[test]
    fn a_solo_tap_starts_and_the_next_solo_tap_stops() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                ralt_up(90.0),
                down("KeyA", 500.0, ""),
                up("KeyA", 560.0, ""),
                ralt_down(3000.0, ""),
                ralt_up(3120.0),
            ],
            Toggle,
        );
        assert_eq!(
            r.step_actions(),
            vec![None, Some(Start), None, None, None, Some(Stop)]
        );
        assert_eq!(
            r.phases(),
            vec![Arming, Listening, Listening, Listening, Listening, Idle]
        );
        // Typing while it listens goes to the shell.
        assert!(!r.steps[2].swallow);
        assert!([0, 1, 4, 5].iter().all(|&i| r.steps[i].swallow));
    }

    #[test]
    fn a_slow_tap_still_counts_up_to_a_second() {
        assert_eq!(
            run(&[ralt_down(0.0, ""), ralt_up(999.0)], Toggle).actions(),
            vec![Start]
        );
        assert_eq!(
            run(&[ralt_down(0.0, ""), ralt_up(1000.0)], Toggle).actions(),
            vec![]
        );
    }

    #[test]
    fn a_tap_polluted_by_another_key_does_nothing_altgr_typing() {
        let r = run(
            &[
                down("ControlLeft", 0.0, "c"),
                down("AltRight", 1.0, "ca"),
                down("Digit2", 60.0, "ca"),
                up("Digit2", 110.0, "ca"),
                up("ControlLeft", 150.0, "a"),
                up("AltRight", 151.0, ""),
            ],
            Toggle,
        );
        assert_eq!(r.actions(), vec![]);
        assert_eq!(r.state.phase, Idle);
    }

    #[test]
    fn a_solo_altgr_tap_on_that_layout_is_a_tap() {
        let r = run(
            &[
                down("ControlLeft", 0.0, "c"),
                down("AltRight", 1.0, "ca"),
                up("ControlLeft", 80.0, "a"),
                up("AltRight", 81.0, ""),
            ],
            Toggle,
        );
        assert_eq!(r.actions(), vec![Start]);
    }

    #[test]
    fn a_polluted_tap_while_listening_does_not_stop_it() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                ralt_up(80.0),
                ralt_down(1000.0, ""),
                down("KeyQ", 1050.0, "a"),
                up("KeyQ", 1100.0, ""),
                ralt_up(1150.0),
            ],
            Toggle,
        );
        assert_eq!(r.actions(), vec![Start]);
        assert_eq!(r.state.phase, Listening);
    }

    #[test]
    fn never_starts_from_holding_the_key_down() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                tick(200.0),
                tick(800.0),
                tick(1200.0),
                ralt_up(1500.0),
            ],
            Toggle,
        );
        assert_eq!(r.actions(), vec![]);
        assert_eq!(r.state.phase, Idle);
    }

    #[test]
    fn escape_cancels_and_an_escape_pressed_mid_tap_does_not_let_the_tap_start_again() {
        let r = run(
            &[
                ralt_down(0.0, ""),
                ralt_up(50.0),
                ralt_down(900.0, ""),
                down("Escape", 950.0, "a"),
                ralt_up(1000.0),
            ],
            Toggle,
        );
        assert_eq!(r.actions(), vec![Start, Cancel]);
        assert_eq!(r.state.phase, Idle);
    }

    #[test]
    fn blur_cancels_a_listening_toggle() {
        let r = run(&[ralt_down(0.0, ""), ralt_up(50.0), blur(400.0)], Toggle);
        assert_eq!(r.actions(), vec![Start, Cancel]);
    }

    // ---- a chord or a plain key ----

    #[test]
    fn hold_starts_on_the_down_at_once_and_stops_on_the_up_of_the_main_key() {
        let r = run_with(
            &[
                down("ControlLeft", 0.0, "c"),
                down("ShiftLeft", 10.0, "cs"),
                down("KeyD", 20.0, "cs"),
                down("KeyD", 60.0, "csr"),
                // The modifiers may let go first: the main key is what is being held.
                up("ShiftLeft", 400.0, "c"),
                down("KeyD", 420.0, "cr"),
                up("KeyD", 900.0, "c"),
                up("ControlLeft", 950.0, ""),
            ],
            Hold,
            chord_kd(),
            None,
        );
        assert_eq!(
            r.step_actions(),
            vec![None, None, Some(Start), None, None, None, Some(Stop), None]
        );
        assert_eq!(
            r.swallows(),
            vec![false, false, true, true, false, true, true, false]
        );
    }

    #[test]
    fn hold_a_plain_f9_needs_no_hold_ms_and_no_solo_rule() {
        let r = run_with(
            &[
                down("F9", 0.0, ""),
                down("KeyA", 5.0, ""),
                up("F9", 40.0, ""),
            ],
            Hold,
            f9(),
            None,
        );
        assert_eq!(r.actions(), vec![Start, Stop]);
    }

    #[test]
    fn toggle_each_non_repeat_down_toggles_repeats_and_ups_do_nothing() {
        let r = run_with(
            &[
                down("F9", 0.0, ""),
                down("F9", 40.0, "r"),
                up("F9", 90.0, ""),
                down("F9", 2000.0, ""),
                up("F9", 2050.0, ""),
            ],
            Toggle,
            f9(),
            None,
        );
        assert_eq!(
            r.step_actions(),
            vec![Some(Start), None, None, Some(Stop), None]
        );
        assert!(r.steps.iter().all(|s| s.swallow));
    }

    #[test]
    fn the_main_key_with_the_wrong_modifiers_is_somebody_elses_key() {
        let r = run_with(
            &[down("KeyD", 0.0, "c"), down("F9", 10.0, "s")],
            Hold,
            chord_kd(),
            None,
        );
        assert!(r.steps.iter().all(|s| s.action.is_none() && !s.swallow));
    }

    #[test]
    fn escape_and_blur_cancel_here_too() {
        assert_eq!(
            run_with(
                &[down("F9", 0.0, ""), down("Escape", 300.0, "")],
                Hold,
                f9(),
                None
            )
            .actions(),
            vec![Start, Cancel]
        );
        assert_eq!(
            run_with(
                &[down("F9", 0.0, ""), up("F9", 40.0, ""), blur(300.0)],
                Toggle,
                f9(),
                None
            )
            .actions(),
            vec![Start, Cancel]
        );
    }

    #[test]
    fn hold_after_an_escape_the_still_held_key_stops_nothing_on_its_way_up() {
        let r = run_with(
            &[
                down("F9", 0.0, ""),
                down("Escape", 300.0, ""),
                down("F9", 330.0, "r"),
                up("F9", 600.0, ""),
            ],
            Hold,
            f9(),
            None,
        );
        assert_eq!(r.actions(), vec![Start, Cancel]);
        assert_eq!(
            r.steps[3],
            Step {
                action: None,
                swallow: true,
                phase: Idle
            }
        );
    }

    // ---- is_hotkey_event ----

    #[test]
    fn a_bare_modifier_matches_itself_its_own_flag_notwithstanding() {
        let ra = right_alt();
        assert!(is_hotkey_event(&ralt_down(0.0, ""), &ra));
        assert!(is_hotkey_event(&down("AltRight", 0.0, "ca"), &ra));
        assert!(!is_hotkey_event(&down("AltLeft", 0.0, "a"), &ra));
        assert!(!is_hotkey_event(&ralt_down(0.0, "s"), &ra));
    }

    #[test]
    fn only_right_alt_is_forgiven_a_ctrl_that_is_altgr_and_no_other_key_has_one() {
        let lalt = hk("AltLeft", "");
        assert!(!is_hotkey_event(&down("AltLeft", 0.0, "ca"), &lalt));
    }

    #[test]
    fn an_up_matches_on_the_code_alone_whatever_is_still_held() {
        assert!(is_hotkey_event(&up("AltRight", 0.0, "c"), &right_alt()));
        assert!(is_hotkey_event(&up("KeyD", 0.0, ""), &chord_kd()));
    }

    #[test]
    fn a_chord_matches_its_modifiers_exactly() {
        let c = chord_kd();
        assert!(is_hotkey_event(&down("KeyD", 0.0, "cs"), &c));
        assert!(!is_hotkey_event(&down("KeyD", 0.0, "c"), &c));
        assert!(!is_hotkey_event(&down("KeyD", 0.0, "csa"), &c));
    }

    #[test]
    fn a_tick_and_a_blur_are_nobodys_key() {
        assert!(!is_hotkey_event(&tick(0.0), &right_alt()));
        assert!(!is_hotkey_event(&blur(0.0), &right_alt()));
    }

    // ---- format_hotkey ----

    #[test]
    fn names_a_bare_modifier_by_its_side() {
        assert_eq!(format_hotkey(&right_alt()), "Right Alt");
        assert_eq!(format_hotkey(&hk("ControlLeft", "")), "Left Ctrl");
        assert_eq!(format_hotkey(&hk("ShiftRight", "")), "Right Shift");
        assert_eq!(format_hotkey(&hk("MetaLeft", "")), "Left Win");
    }

    #[test]
    fn writes_a_chord_the_way_windows_does() {
        assert_eq!(format_hotkey(&chord_kd()), "Ctrl+Shift+D");
        assert_eq!(format_hotkey(&hk("Digit2", "ca")), "Ctrl+Alt+2");
        assert_eq!(format_hotkey(&hk("F9", "m")), "Win+F9");
    }

    #[test]
    fn names_the_keys_that_are_not_letters() {
        assert_eq!(format_hotkey(&f9()), "F9");
        assert_eq!(format_hotkey(&hk("Space", "c")), "Ctrl+Space");
        assert_eq!(format_hotkey(&hk("Backquote", "c")), "Ctrl+`");
        assert_eq!(format_hotkey(&hk("ArrowUp", "")), "Up");
        assert_eq!(format_hotkey(&hk("Numpad5", "")), "Num 5");
        assert_eq!(format_hotkey(&hk("NumpadAdd", "")), "Num +");
        assert_eq!(format_hotkey(&hk("Pause", "")), "Pause");
    }

    // ---- parse_hotkey_from_event ----

    #[test]
    fn a_bare_modifier_press_is_that_modifier_with_its_own_flag_dropped() {
        assert_eq!(
            parse_hotkey_from_event(&chord("AltRight", 0.0, "a")),
            Some(right_alt())
        );
        assert_eq!(
            parse_hotkey_from_event(&chord("ControlLeft", 0.0, "c")),
            Some(hk("ControlLeft", ""))
        );
    }

    #[test]
    fn altgr_is_read_as_right_alt_not_as_ctrl_right_alt() {
        assert_eq!(
            parse_hotkey_from_event(&chord("AltRight", 0.0, "ca")),
            Some(right_alt())
        );
    }

    #[test]
    fn a_chord_keeps_its_modifiers() {
        assert_eq!(
            parse_hotkey_from_event(&chord("KeyD", 0.0, "cs")),
            Some(chord_kd())
        );
        assert_eq!(parse_hotkey_from_event(&chord("F9", 0.0, "")), Some(f9()));
    }

    #[test]
    fn escape_is_the_way_out_of_the_rebind_control_never_a_binding() {
        assert_eq!(parse_hotkey_from_event(&chord("Escape", 0.0, "")), None);
        assert_eq!(parse_hotkey_from_event(&chord("Escape", 0.0, "c")), None);
    }

    #[test]
    fn a_key_the_system_could_not_name_is_no_binding() {
        assert_eq!(parse_hotkey_from_event(&chord("", 0.0, "")), None);
        assert_eq!(
            parse_hotkey_from_event(&chord("Unidentified", 0.0, "")),
            None
        );
    }

    #[test]
    fn takes_a_key_event_as_it_comes() {
        // Prism Terminal took a DOM event here; Flare's chord is the same fields.
        assert_eq!(
            parse_hotkey_from_event(&chord("KeyD", 0.0, "cs")),
            Some(chord_kd())
        );
        assert_eq!(
            parse_hotkey_from_event(&chord("AltRight", 0.0, "ca")),
            Some(right_alt())
        );
        assert_eq!(parse_hotkey_from_event(&chord("Escape", 0.0, "cs")), None);
    }

    #[test]
    fn round_trips_through_the_reducer_what_was_bound_is_what_is_recognised() {
        let bound = parse_hotkey_from_event(&chord("KeyD", 0.0, "cs"));
        assert!(bound.is_some());
        let r = run_with(&[down("KeyD", 0.0, "cs")], Toggle, bound.unwrap(), None);
        assert_eq!(r.actions(), vec![Start]);
    }

    #[test]
    fn copies_what_the_reducer_reads_off_a_key_event() {
        let c = chord("AltRight", 42.0, "car");
        assert_eq!(
            c,
            KeyChord {
                code: "AltRight".into(),
                ctrl: true,
                alt: true,
                shift: false,
                meta: false,
                repeat: true,
                at: 42.0
            }
        );
    }

    // ---- usable_hotkey ----

    #[test]
    fn takes_the_default_a_bare_modifier_a_function_key_and_a_free_chord() {
        assert!(usable_hotkey(&right_alt(), &[]));
        assert!(usable_hotkey(&hk("ControlLeft", ""), &[]));
        assert!(usable_hotkey(&hk("F9", ""), &[]));
        assert!(usable_hotkey(&hk("Pause", ""), &[]));
        assert!(usable_hotkey(&hk("KeyD", "cs"), &[]));
        assert!(usable_hotkey(&hk("Space", "a"), &[]));
    }

    #[test]
    fn refuses_a_key_that_types_with_or_without_shift() {
        for code in ["KeyA", "Enter", "Space", "Tab", "Backspace", "Digit1"] {
            assert!(!usable_hotkey(&hk(code, ""), &[]), "{code}");
        }
        assert!(!usable_hotkey(&hk("KeyA", "s"), &[]));
    }

    #[test]
    fn refuses_the_chords_the_terminal_owns() {
        for code in [
            "KeyC", "KeyV", "KeyW", "KeyT", "KeyF", "Tab", "Digit2", "Comma",
        ] {
            assert!(!usable_hotkey(&hk(code, "c"), &[]), "{code}");
        }
        assert!(!usable_hotkey(&hk("KeyC", "cs"), &[]));
    }

    // ---- a chord of modifiers (Ctrl+Win) ----

    fn ctrl_down(at: f64, f: &str) -> KeyEvt {
        down("ControlLeft", at, &format!("c{f}"))
    }
    fn win_down(at: f64, f: &str) -> KeyEvt {
        down("MetaLeft", at, &format!("m{f}"))
    }

    #[test]
    fn records_on_a_hold_ctrl_first_or_win_first_and_stops_on_either_key_going_up() {
        let cw = default_hotkey();
        let a = run_with(
            &[
                ctrl_down(0.0, ""),
                win_down(10.0, "c"),
                tick(210.0),
                up("MetaLeft", 900.0, "c"),
            ],
            Hold,
            cw.clone(),
            None,
        );
        assert_eq!(a.actions(), vec![Start, Stop]);
        let b = run_with(
            &[
                win_down(0.0, ""),
                ctrl_down(10.0, "m"),
                tick(210.0),
                up("ControlLeft", 900.0, "m"),
            ],
            Hold,
            cw,
            None,
        );
        assert_eq!(b.actions(), vec![Start, Stop]);
    }

    #[test]
    fn never_dictates_ctrl_win_left_switching_desktops_the_arrow_cancels_it_unswallowed() {
        let r = run_with(
            &[
                ctrl_down(0.0, ""),
                win_down(10.0, "c"),
                down("ArrowLeft", 60.0, "cm"),
                tick(210.0),
            ],
            Hold,
            default_hotkey(),
            None,
        );
        assert_eq!(r.actions(), vec![]);
        assert!(!r.steps[2].swallow);
    }

    #[test]
    fn is_not_ctrl_alone_nor_win_alone_nor_ctrl_shift_win() {
        let cw = default_hotkey();
        assert!(!is_hotkey_event(&ctrl_down(0.0, ""), &cw));
        assert!(!is_hotkey_event(&win_down(0.0, ""), &cw));
        assert!(is_hotkey_event(&win_down(0.0, "c"), &cw));
        assert!(!is_hotkey_event(&down("MetaLeft", 0.0, "cms"), &cw));
    }

    #[test]
    fn binds_from_the_keys_as_pressed_and_names_itself_by_its_modifiers() {
        let cw = default_hotkey();
        assert_eq!(
            parse_hotkey_from_event(&chord("MetaLeft", 0.0, "mc")),
            Some(cw)
        );
        assert_eq!(
            parse_hotkey_from_event(&chord("ControlLeft", 0.0, "cm")),
            Some(hk("ControlLeft", "cm"))
        );
        assert_eq!(format_hotkey(&hk("ControlLeft", "cm")), "Ctrl+Win");
        // Right Alt with the Ctrl AltGr brings is still the bare key.
        assert_eq!(
            parse_hotkey_from_event(&chord("AltRight", 0.0, "ca")),
            Some(right_alt())
        );
    }

    // ---- extra (Flare): the stored form and the app's own chords ----

    #[test]
    fn stored_key_json_round_trips_in_pt_field_order() {
        let s = serde_json::to_string(&default_hotkey()).unwrap();
        assert_eq!(
            s,
            r#"{"code":"MetaLeft","ctrl":true,"alt":false,"shift":false,"meta":true}"#
        );
        let back: Hotkey = serde_json::from_str(&s).unwrap();
        assert_eq!(back, default_hotkey());
    }

    #[test]
    fn a_chord_the_app_owns_is_not_usable() {
        let own = [hk("KeyK", "cs")];
        assert!(!usable_hotkey(&hk("KeyK", "cs"), &own));
        assert!(usable_hotkey(&hk("KeyD", "cs"), &own));
    }
}
