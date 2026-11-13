//! The app icon picker's choice (rule 1230, 2026-11-13): which of the owner's icon designs the Dock, the app
//! switcher, the taskbar and the window wear. Key `flare.window.appIcon`; `original`, the shipped Flare icon, until
//! somebody picks another, and for any stored word that is not one of the choices.
//!
//! Only the running app changes: the bundle and the exe keep the original (Finder, shortcuts, the installer), so the
//! signed app is never written to.

use crate::kv::{EnumPref, PrefStore, Subscription};

/// One icon the picker offers: its stored word and the name its tile says (tooltip and accessible label).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppIcon {
    pub id: &'static str,
    pub name: &'static str,
}

/// The shipped icon: first on the picker and the choice until another is picked.
pub const ORIGINAL: &str = "original";

/// The choices in the picker's order: the original first, then #199's Liquid Glass, then the owner's alternates in
/// the order of their files (2026-11-12).
pub const APP_ICONS: [AppIcon; 12] = [
    AppIcon {
        id: ORIGINAL,
        name: "Original",
    },
    AppIcon {
        id: "liquid-glass",
        name: "Liquid Glass",
    },
    AppIcon {
        id: "high-voltage",
        name: "High Voltage",
    },
    AppIcon {
        id: "daybreak",
        name: "Daybreak",
    },
    AppIcon {
        id: "hazard-reverse",
        name: "Hazard Reverse",
    },
    AppIcon {
        id: "safety-orange",
        name: "Safety Orange",
    },
    AppIcon {
        id: "blue-enamel",
        name: "Blue Enamel",
    },
    AppIcon {
        id: "raspberry-lacquer",
        name: "Raspberry Lacquer",
    },
    AppIcon {
        id: "lemon-ink",
        name: "Lemon Ink",
    },
    AppIcon {
        id: "jade-glass",
        name: "Jade Glass",
    },
    AppIcon {
        id: "citrus-punch",
        name: "Citrus Punch",
    },
    AppIcon {
        id: "cobalt-electric",
        name: "Cobalt Electric",
    },
];

/// The stored key.
pub const KEY: &str = "flare.window.appIcon";

const PREF: EnumPref = EnumPref {
    key: KEY,
    allowed: &[
        "original",
        "liquid-glass",
        "high-voltage",
        "daybreak",
        "hazard-reverse",
        "safety-orange",
        "blue-enamel",
        "raspberry-lacquer",
        "lemon-ink",
        "jade-glass",
        "citrus-punch",
        "cobalt-electric",
    ],
    fallback: ORIGINAL,
};

/// The icon of a stored word; the original for one that is not a choice.
pub fn icon_of(word: &str) -> &'static AppIcon {
    APP_ICONS
        .iter()
        .find(|i| i.id == word)
        .unwrap_or(&APP_ICONS[0])
}

/// The icon in force.
pub fn app_icon(s: &PrefStore) -> &'static AppIcon {
    icon_of(PREF.get(s))
}

/// Stores a word as given, or the original when it is not a choice.
pub fn set_app_icon_word(s: &PrefStore, word: &str) {
    PREF.set(s, word);
}

pub fn on_app_icon_change(s: &PrefStore, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
    s.on_change(PREF.key, cb)
}

/// What a tile says: the icon's name, and that the original is the default.
pub fn tile_name(icon: &AppIcon) -> String {
    if icon.id == ORIGINAL {
        format!("{}, the default", icon.name)
    } else {
        icon.name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::FileBackend;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn the_original_is_the_choice_until_another_is_picked() {
        let s = PrefStore::memory();
        assert_eq!(app_icon(&s).id, "original");
        assert_eq!(app_icon(&s).name, "Original");
        assert_eq!(APP_ICONS[0].id, ORIGINAL, "the original comes first");
    }

    #[test]
    fn every_choice_is_allowed_once_and_in_the_picker_order() {
        let ids: Vec<&str> = APP_ICONS.iter().map(|i| i.id).collect();
        assert_eq!(ids, PREF.allowed);
        let mut seen = std::collections::BTreeSet::new();
        for i in APP_ICONS {
            assert!(seen.insert(i.id), "{} twice", i.id);
            assert!(!i.name.is_empty());
        }
        assert_eq!(ids[1], "liquid-glass");
        assert!(ids.contains(&"daybreak"));
    }

    #[test]
    fn an_unknown_stored_word_reads_as_the_original_and_is_never_stored() {
        let s = PrefStore::memory();
        s.set(KEY, "neon");
        assert_eq!(app_icon(&s).id, ORIGINAL);
        set_app_icon_word(&s, "rainbow");
        assert_eq!(s.get(KEY).as_deref(), Some("original"));
        assert_eq!(icon_of("").id, ORIGINAL);
    }

    #[test]
    fn the_pick_round_trips_through_settings_json() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        {
            let s = PrefStore::new(Arc::new(FileBackend::open(
                &file,
                Duration::from_millis(20),
            )));
            set_app_icon_word(&s, "jade-glass");
            s.flush();
        }
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            text.contains(r#""flare.window.appIcon":"jade-glass""#),
            "{text}"
        );
        let again = PrefStore::new(Arc::new(FileBackend::open(
            &file,
            Duration::from_millis(20),
        )));
        assert_eq!(app_icon(&again).id, "jade-glass");
        assert_eq!(app_icon(&again).name, "Jade Glass");
    }

    #[test]
    fn a_pick_is_heard() {
        let s = PrefStore::memory();
        let n = Arc::new(AtomicUsize::new(0));
        let _sub = {
            let n = Arc::clone(&n);
            on_app_icon_change(&s, move || {
                n.fetch_add(1, Ordering::SeqCst);
            })
        };
        set_app_icon_word(&s, "daybreak");
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_original_tile_says_it_is_the_default() {
        assert_eq!(tile_name(&APP_ICONS[0]), "Original, the default");
        assert_eq!(tile_name(icon_of("daybreak")), "Daybreak");
    }
}
