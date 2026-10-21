//! Background, accent, secondary and the edges preference (Task 15).
//!
//! The window colours are this app's own picks over a theme: unset means the theme's own, set is a
//! six or eight digit hex, anything else in the store reads as unset. The edges setting is the
//! window's own chrome (hairline, faint, solid or none). Keys are `flare.window.*` (decision O10).

use flare_theme::colour::canonical_stored;
use flare_theme::edges::{WindowEdges, valid_window_edges};

use crate::kv::{PrefStore, Subscription};

/// One window colour setting: one key, unset means the theme's own.
pub struct ColourPref {
    pub key: &'static str,
}

pub const ACCENT: ColourPref = ColourPref {
    key: "flare.window.accent",
};
pub const BACKGROUND: ColourPref = ColourPref {
    key: "flare.window.background",
};
pub const SECONDARY: ColourPref = ColourPref {
    key: "flare.window.secondary",
};

impl ColourPref {
    /// The stored colour in its canonical form, or None to follow the theme.
    pub fn get(&self, s: &PrefStore) -> Option<String> {
        canonical_stored(s.get(self.key).as_deref())
    }

    /// Some(valid hex) stores the canonical form. None, or anything invalid, REMOVES the key. Either
    /// way every listener of the key is told.
    pub fn set(&self, s: &PrefStore, hex: Option<&str>) {
        match hex.and_then(|h| canonical_stored(Some(h))) {
            Some(v) => s.set(self.key, &v),
            None => s.remove(self.key),
        }
    }

    pub fn on_change(&self, s: &PrefStore, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
        s.on_change(self.key, cb)
    }
}

/// The three window colours; each None follows the theme.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowColours {
    pub background: Option<String>,
    pub accent: Option<String>,
    pub secondary: Option<String>,
}

pub const NO_COLOURS: WindowColours = WindowColours {
    background: None,
    accent: None,
    secondary: None,
};

pub fn read_window_colours(s: &PrefStore) -> WindowColours {
    WindowColours {
        background: BACKGROUND.get(s),
        accent: ACCENT.get(s),
        secondary: SECONDARY.get(s),
    }
}

pub fn set_window_colours(s: &PrefStore, c: &WindowColours) {
    BACKGROUND.set(s, c.background.as_deref());
    ACCENT.set(s, c.accent.as_deref());
    SECONDARY.set(s, c.secondary.as_deref());
}

/// Subscribes one callback to all three colour keys; the returned subscriptions stop them.
pub fn on_any_window_colour_change(
    s: &PrefStore,
    cb: impl Fn() + Send + Sync + Clone + 'static,
) -> Vec<Subscription> {
    [&ACCENT, &BACKGROUND, &SECONDARY]
        .iter()
        .map(|p| p.on_change(s, cb.clone()))
        .collect()
}

/// The key Custom keeps its window colours under.
pub const CUSTOM_COLOURS_KEY: &str = "flare.window.customColours";

/// The edges key.
pub const EDGES_KEY: &str = "flare.window.edges";

/// Whether a value is a six or eight digit hex with the `#`; returned lower-cased.
fn valid_hex(v: &str) -> Option<String> {
    let h = v.strip_prefix('#')?;
    let ok = (h.len() == 6 || h.len() == 8) && h.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| v.to_ascii_lowercase())
}

fn field(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(serde_json::Value::as_str).and_then(valid_hex)
}

/// Each field a valid hex (lower-cased), else None. A value that is not an object reads as none.
pub fn valid_colours(v: &serde_json::Value) -> WindowColours {
    WindowColours {
        background: field(v.get("background")),
        accent: field(v.get("accent")),
        secondary: field(v.get("secondary")),
    }
}

/// What Custom keeps. Junk or a missing value reads as none.
pub fn custom_window_colours(s: &PrefStore) -> WindowColours {
    match s
        .get(CUSTOM_COLOURS_KEY)
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    {
        Some(v) => valid_colours(&v),
        None => NO_COLOURS,
    }
}

/// Stores what Custom keeps, validated first; None removes it.
pub fn set_custom_window_colours(s: &PrefStore, c: Option<&WindowColours>) {
    match c {
        Some(c) => {
            let v = valid_colours(&serde_json::json!({
                "background": c.background,
                "accent": c.accent,
                "secondary": c.secondary,
            }));
            let json = serde_json::json!({
                "background": v.background,
                "accent": v.accent,
                "secondary": v.secondary,
            });
            s.set(CUSTOM_COLOURS_KEY, &json.to_string());
        }
        None => s.remove(CUSTOM_COLOURS_KEY),
    }
}

/// Whether the picks in force differ from what the theme in front keeps, both validated first.
pub fn colours_differ(now: &WindowColours, kept: &WindowColours) -> bool {
    let norm = |c: &WindowColours| WindowColours {
        background: c.background.as_deref().and_then(valid_hex),
        accent: c.accent.as_deref().and_then(valid_hex),
        secondary: c.secondary.as_deref().and_then(valid_hex),
    };
    norm(now) != norm(kept)
}

pub fn window_edges(s: &PrefStore) -> WindowEdges {
    valid_window_edges(s.get(EDGES_KEY).as_deref())
}

pub fn set_window_edges(s: &PrefStore, e: WindowEdges) {
    s.set(EDGES_KEY, e.word());
}

/// Stores the edge for a word; anything else stores the default, so junk is never stored.
pub fn set_window_edges_word(s: &PrefStore, word: &str) {
    set_window_edges(s, valid_window_edges(Some(word)));
}

pub fn on_window_edges_change(
    s: &PrefStore,
    cb: impl Fn() + Send + Sync + 'static,
) -> Subscription {
    s.on_change(EDGES_KEY, cb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn counter(s: &PrefStore, key: &str) -> (Arc<AtomicUsize>, Subscription) {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = Arc::clone(&n);
        let sub = s.on_change(key, move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        (n, sub)
    }

    #[test]
    fn accent_follows_the_theme_until_chosen() {
        let s = PrefStore::memory();
        assert_eq!(ACCENT.get(&s), None);
        ACCENT.set(&s, Some("#E07A2F"));
        assert_eq!(ACCENT.get(&s), Some("#e07a2f".to_string()));
    }

    #[test]
    fn accent_remembers_a_colour_in_lower_case() {
        let s = PrefStore::memory();
        ACCENT.set(&s, Some("#E07A2F"));
        assert_eq!(s.get(ACCENT.key), Some("#e07a2f".to_string()));
    }

    #[test]
    fn accent_reads_junk_as_follow_the_theme() {
        let s = PrefStore::memory();
        for junk in [
            "orange",
            "#fff",
            "#12345g",
            "null",
            "#1234567",
            "#123456789",
        ] {
            s.set(ACCENT.key, junk);
            assert_eq!(ACCENT.get(&s), None, "junk {junk}");
        }
    }

    #[test]
    fn accent_remembers_an_alpha_and_stores_an_opaque_pick_as_six_digits() {
        let s = PrefStore::memory();
        ACCENT.set(&s, Some("#E07A2F80"));
        assert_eq!(ACCENT.get(&s), Some("#e07a2f80".to_string()));
        ACCENT.set(&s, Some("#e07a2fff"));
        assert_eq!(s.get(ACCENT.key), Some("#e07a2f".to_string()));
        assert_eq!(ACCENT.get(&s), Some("#e07a2f".to_string()));
    }

    #[test]
    fn accent_forgets_when_handed_none_and_tells_the_painter_both_times() {
        let s = PrefStore::memory();
        let (n, sub) = counter(&s, ACCENT.key);
        ACCENT.set(&s, Some("#112233"));
        ACCENT.set(&s, None);
        assert_eq!(ACCENT.get(&s), None);
        assert_eq!(s.get(ACCENT.key), None);
        assert_eq!(n.load(Ordering::SeqCst), 2);
        sub.cancel();
        ACCENT.set(&s, Some("#112233"));
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn background_follows_the_theme_and_forgets_on_none() {
        let s = PrefStore::memory();
        assert_eq!(BACKGROUND.get(&s), None);
        BACKGROUND.set(&s, Some("#1A1B26"));
        assert_eq!(BACKGROUND.get(&s), Some("#1a1b26".to_string()));
        BACKGROUND.set(&s, None);
        assert_eq!(BACKGROUND.get(&s), None);
        assert_eq!(s.get(BACKGROUND.key), None);
    }

    #[test]
    fn background_is_its_own_key() {
        let s = PrefStore::memory();
        BACKGROUND.set(&s, Some("#1a1b26"));
        assert_eq!(ACCENT.get(&s), None);
        assert_eq!(SECONDARY.get(&s), None);
        assert_eq!(BACKGROUND.key, "flare.window.background");
    }

    #[test]
    fn background_reads_junk_as_follow_the_theme() {
        let s = PrefStore::memory();
        for junk in ["black", "#1234567", "#123456789", "rgba(0,0,0,0.5)"] {
            s.set(BACKGROUND.key, junk);
            assert_eq!(BACKGROUND.get(&s), None, "junk {junk}");
        }
    }

    #[test]
    fn background_keeps_an_alpha_in_lower_case_and_an_opaque_one_as_six_digits() {
        let s = PrefStore::memory();
        BACKGROUND.set(&s, Some("#1A1B2699"));
        assert_eq!(BACKGROUND.get(&s), Some("#1a1b2699".to_string()));
        BACKGROUND.set(&s, Some("#1a1b26FF"));
        assert_eq!(BACKGROUND.get(&s), Some("#1a1b26".to_string()));
    }

    #[test]
    fn secondary_is_its_own_key_and_reads_like_the_others() {
        let s = PrefStore::memory();
        assert_eq!(SECONDARY.key, "flare.window.secondary");
        SECONDARY.set(&s, Some("#2A2A2A"));
        assert_eq!(SECONDARY.get(&s), Some("#2a2a2a".to_string()));
        s.set(SECONDARY.key, "nope");
        assert_eq!(SECONDARY.get(&s), None);
        assert_eq!(BACKGROUND.get(&s), None);
    }

    #[test]
    fn read_and_set_window_colours_go_through_each_pref() {
        let s = PrefStore::memory();
        let c = WindowColours {
            background: Some("#101010".into()),
            accent: None,
            secondary: Some("#2a2a2a".into()),
        };
        set_window_colours(&s, &c);
        assert_eq!(read_window_colours(&s), c);
        set_window_colours(&s, &NO_COLOURS);
        assert_eq!(read_window_colours(&s), NO_COLOURS);
    }

    #[test]
    fn any_window_colour_change_reaches_the_callback_from_each_key() {
        let s = PrefStore::memory();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = Arc::clone(&n);
        let subs = on_any_window_colour_change(&s, move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        ACCENT.set(&s, Some("#112233"));
        BACKGROUND.set(&s, Some("#112233"));
        SECONDARY.set(&s, None);
        assert_eq!(n.load(Ordering::SeqCst), 3);
        drop(subs);
        ACCENT.set(&s, None);
        assert_eq!(n.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn custom_keeps_no_window_colours_until_save_keeps_some() {
        let s = PrefStore::memory();
        assert_eq!(custom_window_colours(&s), NO_COLOURS);
        let kept = WindowColours {
            background: Some("#101010".into()),
            accent: None,
            secondary: Some("#2A2A2A".into()),
        };
        set_custom_window_colours(&s, Some(&kept));
        assert_eq!(
            custom_window_colours(&s),
            WindowColours {
                background: Some("#101010".into()),
                accent: None,
                secondary: Some("#2a2a2a".into()),
            }
        );
        set_custom_window_colours(&s, None);
        assert_eq!(custom_window_colours(&s), NO_COLOURS);
        assert_eq!(s.get(CUSTOM_COLOURS_KEY), None);
    }

    #[test]
    fn custom_window_colours_read_junk_as_none() {
        let s = PrefStore::memory();
        s.set(CUSTOM_COLOURS_KEY, "{nope");
        assert_eq!(custom_window_colours(&s), NO_COLOURS);
        let v = json!({"background": "red", "accent": 12, "secondary": "#abcdef"});
        assert_eq!(
            valid_colours(&v),
            WindowColours {
                background: None,
                accent: None,
                secondary: Some("#abcdef".into()),
            }
        );
        assert_eq!(valid_colours(&json!("not an object")), NO_COLOURS);
    }

    #[test]
    fn a_pick_that_differs_from_what_is_kept_is_a_change_to_save() {
        assert!(!colours_differ(&NO_COLOURS, &NO_COLOURS));
        let accent_only = WindowColours {
            accent: Some("#FF8800".into()),
            ..NO_COLOURS
        };
        assert!(colours_differ(&accent_only, &NO_COLOURS));
        let kept = WindowColours {
            accent: Some("#ff8800".into()),
            ..NO_COLOURS
        };
        assert!(!colours_differ(&accent_only, &kept));
    }

    #[test]
    fn edges_are_a_hairline_until_chosen() {
        let s = PrefStore::memory();
        assert_eq!(window_edges(&s), WindowEdges::Hairline);
    }

    #[test]
    fn edges_remember_each_of_the_four_choices_under_their_own_key() {
        let s = PrefStore::memory();
        for (e, word) in [
            (WindowEdges::None, "none"),
            (WindowEdges::Faint, "faint"),
            (WindowEdges::Hairline, "hairline"),
            (WindowEdges::Solid, "solid"),
        ] {
            set_window_edges(&s, e);
            assert_eq!(s.get(EDGES_KEY), Some(word.to_string()));
            assert_eq!(window_edges(&s), e);
        }
    }

    #[test]
    fn edges_read_an_unknown_word_as_the_default_and_never_store_one() {
        let s = PrefStore::memory();
        s.set(EDGES_KEY, "strong");
        assert_eq!(window_edges(&s), WindowEdges::Hairline);
        set_window_edges_word(&s, "dotted");
        assert_eq!(s.get(EDGES_KEY), Some("hairline".to_string()));
        set_window_edges_word(&s, "solid");
        assert_eq!(s.get(EDGES_KEY), Some("solid".to_string()));
    }

    #[test]
    fn edges_tell_their_listeners_and_stop_when_asked() {
        let s = PrefStore::memory();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = Arc::clone(&n);
        let sub = on_window_edges_change(&s, move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        set_window_edges(&s, WindowEdges::Solid);
        set_window_edges_word(&s, "faint");
        assert_eq!(n.load(Ordering::SeqCst), 2);
        sub.cancel();
        set_window_edges(&s, WindowEdges::None);
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }
}
