//! The app icon picker's choice (rule 1230, 2026-11-13): which of the owner's icon designs the Dock, the app
//! switcher, the taskbar and the window wear. Key `flare.window.appIcon`; `original`, the shipped Flare icon, until
//! somebody picks another, and for any stored word that is not one of the choices.
//!
//! The bundle and the exe keep the original (Finder, the installer), so the signed app is never written to. On
//! Windows this profile's Start menu shortcut wears the pick too (rule 1272).

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

/// macOS (rule 1276, Max 2026-11-17): the pick's picture, a PNG under `~/Library/Application Support`, for what draws
/// Flare while it is closed: the Dock tile plugin in `Flare.app/Contents/PlugIns` (a pinned Flare) and Huddle. It is
/// absent while the original is picked, so both show the bundle's icon. The plugin's source names the same path.
pub const SHARED_ICON_FILE: &str = "Flare/app-icon.png";

/// The distributed notification Flare posts after the shared picture changed (rule 1276); the Dock tile plugin and
/// Huddle read the file again when they hear it.
pub const SHARED_ICON_NOTE: &str = "com.flare.terminal.appIconChanged";

/// The App icon row's button on Windows (rule 1263, 2026-11-14): it restarts Explorer, so the taskbar shows the
/// picked icon at once. It stands where the picked icon's name stood. A Mac has no such button (rule 1264).
pub const RESTART_BUTTON: &str = "restart-shell";

/// The button's words. It has words, so it has no tooltip.
pub const RESTART_LABEL: &str = "Restart Explorer";

/// The one plain line of the settings notice (a dialog with OK) after a failed restart. The row's subtext never
/// changes to report it (rule 1263).
pub const RESTART_FAILED: &str = "Explorer did not restart.";

/// The system whose shell the App icon row is for. Any system but macOS reads as Windows, as the rest of the page's
/// `cfg!(target_os = "macos")` switches do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellHost {
    Windows,
    Mac,
}

impl ShellHost {
    /// The system Flare runs on.
    pub const fn current() -> ShellHost {
        if cfg!(target_os = "macos") {
            ShellHost::Mac
        } else {
            ShellHost::Windows
        }
    }
}

/// The restart button's words on `host`, or None when the row offers no restart button (rule 1264, Max 2026-11-15):
/// the Dock shows a picked icon at once through `NSApplication.applicationIconImage`, and restarting the Dock may
/// drop it, so a Mac has no Restart Dock button and Flare never ends the Dock.
pub const fn restart_label(host: ShellHost) -> Option<&'static str> {
    match host {
        ShellHost::Windows => Some(RESTART_LABEL),
        ShellHost::Mac => None,
    }
}

/// Restarts Explorer (rule 1263): Windows ends `explorer.exe` in this user's session and starts it again. It blocks,
/// so it runs on a worker. A Mac never calls it (rule 1264). Tests use `FakeShellRestart`: nothing outside the test
/// restarts. The error is a short reason with no path in it (it reaches the log as a crumb).
pub trait ShellRestart: Send + Sync {
    fn restart(&self) -> Result<(), String>;
}

/// The tests' stand-in: counts the restarts it was asked for and answers as told.
#[derive(Debug, Default)]
pub struct FakeShellRestart {
    calls: std::sync::atomic::AtomicUsize,
    fail: std::sync::atomic::AtomicBool,
}

impl FakeShellRestart {
    pub fn new() -> FakeShellRestart {
        FakeShellRestart::default()
    }

    /// A stand-in whose every restart fails.
    pub fn failing() -> FakeShellRestart {
        let f = FakeShellRestart::default();
        f.set_failing(true);
        f
    }

    pub fn set_failing(&self, fail: bool) {
        self.fail.store(fail, std::sync::atomic::Ordering::SeqCst);
    }

    /// How many restarts were asked for.
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl ShellRestart for FakeShellRestart {
    fn restart(&self) -> Result<(), String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            Err("fake failure".to_string())
        } else {
            Ok(())
        }
    }
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

    /// Rule 1263: the stand-in counts and answers as told; the button's words name the system's shell.
    #[test]
    fn the_fake_restart_counts_and_fails_when_told() {
        let f = FakeShellRestart::new();
        assert_eq!(f.restart(), Ok(()));
        f.set_failing(true);
        assert!(f.restart().is_err());
        assert_eq!(f.calls(), 2);
        assert!(FakeShellRestart::failing().restart().is_err());
        assert_eq!(RESTART_LABEL, "Restart Explorer");
        assert_eq!(RESTART_FAILED, "Explorer did not restart.");
    }

    /// Rule 1264: Windows offers Restart Explorer; a Mac offers no restart button at all.
    #[test]
    fn only_windows_offers_a_restart_and_a_mac_none() {
        assert_eq!(restart_label(ShellHost::Windows), Some("Restart Explorer"));
        assert_eq!(restart_label(ShellHost::Mac), None);
        let want = if cfg!(target_os = "macos") {
            ShellHost::Mac
        } else {
            ShellHost::Windows
        };
        assert_eq!(ShellHost::current(), want);
    }

    #[test]
    fn the_original_tile_says_it_is_the_default() {
        assert_eq!(tile_name(&APP_ICONS[0]), "Original, the default");
        assert_eq!(tile_name(icon_of("daybreak")), "Daybreak");
    }
}
