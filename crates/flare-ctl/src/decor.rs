//! The words of the decor methods (`tabs.badge.*`, `tabs.progress.*`): the built-in badge icons, the tones, the
//! shape of a key and the length limits. Both sides read them from here: the app checks a call against them on the
//! channel's worker, and `flare-mcp` offers the same icon list to the agent as an enum.

/// The badges Flare draws, by wire name. There is no `check` and no `dot`: those would read as the agent's own
/// finished and working marks.
pub const BADGE_ICONS: [&str; 11] = [
    "phone", "globe", "bell", "mic", "eye", "moon", "lock", "bolt", "star", "plus", "pin",
];

/// The tones a badge may wear. None of them is a state colour of the agent marks.
pub const TONES: [&str; 3] = ["ink", "accent", "muted"];

/// The longest key: `a-z`, `0-9` and `-`.
pub const KEY_MAX: usize = 24;
/// The longest tip of a badge and label of a progress chip, in characters.
pub const TIP_MAX: usize = 80;
/// Decor changes one caller may make to one tab in a second (the rest answer `rate_limited`).
pub const RATE_PER_TAB: f64 = 10.0;
/// The key `flare-mcp` uses for the agent's one progress chip (its tool takes no key).
pub const AGENT_PROGRESS_KEY: &str = "progress";

/// Whether `icon` is a built-in badge.
pub fn is_badge_icon(icon: &str) -> bool {
    BADGE_ICONS.contains(&icon)
}

/// A key is 1 to 24 of `a-z`, `0-9` and `-`.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= KEY_MAX
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badges_are_unique_and_never_a_mark_glyph() {
        let mut seen = std::collections::HashSet::new();
        for i in BADGE_ICONS {
            assert!(seen.insert(i), "{i} twice");
            assert!(is_badge_icon(i));
        }
        for mark in ["check", "dot", "", "Phone", "phone "] {
            assert!(!is_badge_icon(mark), "{mark:?}");
        }
    }

    #[test]
    fn a_key_is_short_lower_case_letters_digits_and_dashes() {
        for k in ["a", "call-me", "x9", "-", &"a".repeat(KEY_MAX)] {
            assert!(valid_key(k), "{k:?}");
        }
        for k in [
            "",
            "A",
            "call me",
            "call_me",
            "\u{e9}",
            "a.b",
            &"a".repeat(KEY_MAX + 1),
        ] {
            assert!(!valid_key(k), "{k:?}");
        }
    }
}
