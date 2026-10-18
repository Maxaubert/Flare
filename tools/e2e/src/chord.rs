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
