//! The chord parser for the harness: the same grammar the app reads (Task 8's `event_from_chord`).
//! The client only validates and labels; the app builds the real key event.
//!
//! Grammar: tokens joined by `+`, an optional last `=<logical key>`, modifier tokens `Ctrl`,
//! `Shift`, `Alt`, `Meta` (case-insensitive), and a last token that is a W3C code or one
//! printable character.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mod {
    Ctrl,
    Shift,
    Alt,
    Meta,
}

const NAMED: &[&str] = &[
    "Enter",
    "Tab",
    "Space",
    "Escape",
    "Backspace",
    "Delete",
    "Insert",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "PageUp",
    "PageDown",
    "Home",
    "End",
    "Comma",
    "Period",
    "Slash",
    "Semicolon",
    "Quote",
    "BracketLeft",
    "BracketRight",
    "Backslash",
    "Minus",
    "Equal",
    "Backquote",
];

fn is_code(token: &str) -> bool {
    if NAMED.contains(&token) {
        return true;
    }
    if let Some(rest) = token.strip_prefix("Key") {
        return rest.len() == 1 && rest.chars().all(|c| c.is_ascii_uppercase());
    }
    if let Some(rest) = token.strip_prefix("Digit") {
        return rest.len() == 1 && rest.chars().all(|c| c.is_ascii_digit());
    }
    if let Some(rest) = token.strip_prefix("Numpad") {
        return rest.len() == 1 && rest.chars().all(|c| c.is_ascii_digit());
    }
    if let Some(rest) = token.strip_prefix('F') {
        return rest.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n));
    }
    false
}

fn modifier(token: &str) -> Option<Mod> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" => Some(Mod::Ctrl),
        "shift" => Some(Mod::Shift),
        "alt" => Some(Mod::Alt),
        "meta" => Some(Mod::Meta),
        _ => None,
    }
}

/// Split a chord into its modifiers and its key (a W3C code, or one printable character).
/// A trailing `=<logical>` is checked to be non-empty and dropped.
pub fn parse(chord: &str) -> Result<(Vec<Mod>, String), String> {
    if chord.is_empty() {
        return Err("empty chord".into());
    }
    // The logical override follows the last `=` that is not itself the key (`Ctrl+=`).
    let body = match chord.rfind('=') {
        Some(i) if i > 0 && !chord[..i].ends_with('+') => {
            if chord[i + 1..].is_empty() {
                return Err(format!("chord {chord:?} has an empty logical key"));
            }
            &chord[..i]
        }
        _ => chord,
    };
    // `+` itself is a key: "Ctrl++" splits to a trailing empty token after a `+`.
    let (head, key) = if let Some(stripped) = body.strip_suffix("++") {
        (format!("{stripped}+"), Some("+".to_string()))
    } else if body == "+" {
        (String::new(), Some("+".to_string()))
    } else {
        (body.to_string(), None)
    };
    let mut tokens: Vec<&str> = if head.is_empty() {
        Vec::new()
    } else {
        head.split('+').collect()
    };
    let key = match key {
        Some(k) => {
            // `head` ended with the `+` separator we re-added.
            if tokens.last() == Some(&"") {
                tokens.pop();
            }
            k
        }
        None => match tokens.pop() {
            Some(t) => t.to_string(),
            None => return Err(format!("chord {chord:?} has no key")),
        },
    };
    let mut mods = Vec::new();
    for t in tokens {
        match modifier(t) {
            Some(m) if !mods.contains(&m) => mods.push(m),
            Some(_) => return Err(format!("chord {chord:?} repeats a modifier")),
            None => return Err(format!("chord {chord:?}: {t:?} is not a modifier")),
        }
    }
    if key.is_empty() {
        return Err(format!("chord {chord:?} has no key"));
    }
    let mut chars = key.chars();
    let single =
        matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_control() && c != ' ');
    if !single && !is_code(&key) {
        return Err(format!("chord {chord:?}: {key:?} is not a key"));
    }
    Ok((mods, key))
}

/// The chord as a label such as `Ctrl+Shift+T`: fixed modifier order, letters and digits bare.
pub fn label(chord: &str) -> String {
    let Ok((mods, key)) = parse(chord) else {
        return chord.to_string();
    };
    let mut out = String::new();
    for want in [Mod::Ctrl, Mod::Shift, Mod::Alt, Mod::Meta] {
        if mods.contains(&want) {
            out.push_str(match want {
                Mod::Ctrl => "Ctrl+",
                Mod::Shift => "Shift+",
                Mod::Alt => "Alt+",
                Mod::Meta => "Meta+",
            });
        }
    }
    let shown = if let Some(r) = key.strip_prefix("Key").filter(|r| r.len() == 1) {
        r.to_string()
    } else if let Some(r) = key.strip_prefix("Digit").filter(|r| r.len() == 1) {
        r.to_string()
    } else {
        key.to_uppercase_if_letter()
    };
    out.push_str(&shown);
    out
}

trait UpperIfLetter {
    fn to_uppercase_if_letter(&self) -> String;
}

impl UpperIfLetter for String {
    fn to_uppercase_if_letter(&self) -> String {
        if self.chars().count() == 1 && self.chars().all(|c| c.is_alphabetic()) {
            self.to_uppercase()
        } else {
            self.clone()
        }
    }
}

/// The key event a chord stands for, as far as the app's chord table reads it (the logical key, the
/// code and the modifiers). None for a chord that does not parse.
fn table_event(mods: &[Mod], key: &str) -> flare_term::keys::KeyEvent {
    let shift = mods.contains(&Mod::Shift);
    let punct = [
        (',', "Comma"),
        ('.', "Period"),
        ('/', "Slash"),
        (';', "Semicolon"),
        ('\'', "Quote"),
        ('[', "BracketLeft"),
        (']', "BracketRight"),
        ('\\', "Backslash"),
        ('-', "Minus"),
        ('=', "Equal"),
        ('`', "Backquote"),
    ];
    let (name, code) = if let Some(l) = key.strip_prefix("Key").filter(|r| r.len() == 1) {
        (l.to_string(), key.to_string())
    } else if let Some(d) = key.strip_prefix("Digit").filter(|r| r.len() == 1) {
        (d.to_string(), key.to_string())
    } else if let Some((c, code)) = punct.iter().find(|(_, code)| *code == key) {
        (c.to_string(), (*code).to_string())
    } else if key == "Space" {
        (" ".to_string(), key.to_string())
    } else if key.chars().count() == 1 {
        let c = key.chars().next().unwrap_or(' ');
        let code = if c.is_ascii_alphabetic() {
            format!("Key{}", c.to_ascii_uppercase())
        } else if c.is_ascii_digit() {
            format!("Digit{c}")
        } else {
            punct
                .iter()
                .find(|(p, _)| *p == c)
                .map_or_else(String::new, |(_, code)| (*code).to_string())
        };
        (c.to_string(), code)
    } else {
        (key.to_string(), key.to_string())
    };
    let name = if shift && name.len() == 1 {
        name.to_uppercase()
    } else {
        name.to_lowercase_if_single()
    };
    let mut ev = flare_term::keys::KeyEvent::new(&name, &code);
    ev.ctrl = mods.contains(&Mod::Ctrl);
    ev.shift = shift;
    ev.alt = mods.contains(&Mod::Alt);
    ev.meta = mods.contains(&Mod::Meta);
    ev
}

trait LowerIfSingle {
    fn to_lowercase_if_single(&self) -> String;
}

impl LowerIfSingle for String {
    fn to_lowercase_if_single(&self) -> String {
        if self.chars().count() == 1 {
            self.to_lowercase()
        } else {
            self.clone()
        }
    }
}

/// A context where every chord of the table is live (help on, sidebar on, the agent guard on).
fn table_context(os: flare_term::keys::KeyOs) -> flare_tabs::keymap::KeyContext {
    flare_tabs::keymap::KeyContext {
        os,
        help: true,
        vertical: true,
        agent_guard: true,
        ..Default::default()
    }
}

/// `chord` with its first `Ctrl` modifier token spelled `Meta` (Cmd on a Mac).
fn ctrl_as_meta(chord: &str) -> String {
    let mut out = Vec::new();
    let mut swapped = false;
    let parts: Vec<&str> = chord.split('+').collect();
    let last = parts.len().saturating_sub(1);
    for (i, p) in parts.iter().enumerate() {
        if !swapped && i < last && p.eq_ignore_ascii_case("ctrl") {
            out.push("Meta");
            swapped = true;
        } else {
            out.push(p);
        }
    }
    out.join("+")
}

/// macOS (rule 333): the Mac spelling of a scenario's Windows chord. A chord that is an app chord on
/// Windows (the app's own chord table says so) and whose Mac key is the same chord with Cmd in Ctrl's
/// place becomes that; paste (Ctrl+V, Ctrl+Shift+V) becomes Cmd+V and Cmd+Shift+V, as the Mac
/// terminal rules have it. Everything else is unchanged: Ctrl+Tab (the same on both), and the Ctrl
/// keys that are the shell's (Ctrl+C, Ctrl+A), which a Mac also sends to the shell.
pub fn mac_app_chord(chord: &str) -> String {
    use flare_tabs::keymap::chord_of;
    use flare_term::keys::KeyOs;
    let Ok((mods, key)) = parse(chord) else {
        return chord.to_string();
    };
    if !mods.contains(&Mod::Ctrl) || mods.contains(&Mod::Meta) {
        return chord.to_string();
    }
    let ev = table_event(&mods, &key);
    let paste = ev.key.eq_ignore_ascii_case("v") && !ev.alt;
    if paste {
        return ctrl_as_meta(chord);
    }
    let Some(id) = chord_of(&ev, &table_context(KeyOs::Windows)) else {
        return chord.to_string();
    };
    if chord_of(&ev, &table_context(KeyOs::Mac)) == Some(id) {
        return chord.to_string();
    }
    let swapped = ctrl_as_meta(chord);
    let Ok((mods2, key2)) = parse(&swapped) else {
        return chord.to_string();
    };
    if chord_of(&table_event(&mods2, &key2), &table_context(KeyOs::Mac)) == Some(id) {
        swapped
    } else {
        chord.to_string()
    }
}

#[cfg(test)]
mod mac_chord_tests {
    use super::*;

    #[test]
    fn the_app_chords_take_cmd_on_a_mac_and_the_shell_keys_stay() {
        assert_eq!(mac_app_chord("Ctrl+T"), "Meta+T");
        assert_eq!(mac_app_chord("Ctrl+KeyW"), "Meta+KeyW");
        assert_eq!(mac_app_chord("Ctrl+Shift+P"), "Meta+Shift+P");
        assert_eq!(mac_app_chord("Ctrl+Comma"), "Meta+Comma");
        assert_eq!(mac_app_chord("Ctrl+F"), "Meta+F");
        assert_eq!(mac_app_chord("Ctrl+V"), "Meta+V");
        assert_eq!(mac_app_chord("Ctrl+Shift+V"), "Meta+Shift+V");
        // The same on both systems, or the shell's.
        assert_eq!(mac_app_chord("Ctrl+Tab"), "Ctrl+Tab");
        assert_eq!(mac_app_chord("Ctrl+C"), "Ctrl+C");
        assert_eq!(mac_app_chord("Ctrl+A"), "Ctrl+A");
        assert_eq!(mac_app_chord("Enter"), "Enter");
        assert_eq!(mac_app_chord("Shift+Enter"), "Shift+Enter");
        assert_eq!(mac_app_chord("not a chord+"), "not a chord+");
    }
}
