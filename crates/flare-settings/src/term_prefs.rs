//! Terminal prefs (Task 3): the getter for the saved shell, and the agent hotkeys switch. M7 owns the
//! rest of the terminal settings.
//!
//! Keys `flare.term.shell` (the shell id; unset until chosen) and `flare.window.agentKeys` (a bool,
//! default ON: the agent hotkeys of the chord table are the app's while it is on).

use crate::kv::{PrefStore, Subscription, bool_pref_get, bool_pref_set};

const SHELL_KEY: &str = "flare.term.shell";
const AGENT_KEYS: &str = "flare.window.agentKeys";
const OPEN_PREVIEWS: &str = "flare.window.openPreviews";

/// The shell the user chose. None until chosen; an empty stored value reads as None too.
pub fn saved_shell_id(s: &PrefStore) -> Option<String> {
    s.get(SHELL_KEY).filter(|v| !v.is_empty())
}

pub fn agent_keys(s: &PrefStore) -> bool {
    bool_pref_get(s, AGENT_KEYS, true)
}

pub fn set_agent_keys(s: &PrefStore, on: bool) {
    bool_pref_set(s, AGENT_KEYS, on);
}

/// Open previews in Flare (Surfaces stage 1): a left click on a path to a picture or a text file
/// shows it in the tab's panel instead of handing it to Windows. Default ON; only a stored "0" is off.
pub fn open_previews(s: &PrefStore) -> bool {
    bool_pref_get(s, OPEN_PREVIEWS, true)
}

pub fn set_open_previews(s: &PrefStore, on: bool) {
    bool_pref_set(s, OPEN_PREVIEWS, on);
}

/// Stores the shell id; an empty id removes the key, so the saved shell reads as None again.
pub fn save_shell_id(s: &PrefStore, id: &str) {
    if id.is_empty() {
        s.remove(SHELL_KEY);
    } else {
        s.set(SHELL_KEY, id);
    }
}

/// Calls `cb` when the saved shell changes; dropping the subscription stops it.
pub fn on_shell_change(s: &PrefStore, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
    s.on_change(SHELL_KEY, cb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn previews_are_on_until_a_stored_zero() {
        let s = PrefStore::memory();
        assert!(open_previews(&s));
        set_open_previews(&s, false);
        assert!(!open_previews(&s));
        assert_eq!(s.get("flare.window.openPreviews").as_deref(), Some("0"));
        set_open_previews(&s, true);
        assert!(open_previews(&s));
        s.set("flare.window.openPreviews", "junk");
        assert!(open_previews(&s), "an unknown value reads as the default");
    }

    #[test]
    fn saved_shell_id_is_none_until_chosen() {
        let s = PrefStore::memory();
        assert_eq!(saved_shell_id(&s), None);
        s.set("flare.term.shell", "pwsh");
        assert_eq!(saved_shell_id(&s), Some("pwsh".to_string()));
        s.set("flare.term.shell", "");
        assert_eq!(saved_shell_id(&s), None);
    }

    #[test]
    fn the_saved_shell_is_none_until_chosen_and_never_empty() {
        let s = PrefStore::memory();
        assert_eq!(saved_shell_id(&s), None);
        save_shell_id(&s, "");
        assert_eq!(saved_shell_id(&s), None);
        assert_eq!(s.get(SHELL_KEY), None);
        save_shell_id(&s, "wsl-Ubuntu");
        save_shell_id(&s, "");
        assert_eq!(s.get(SHELL_KEY), None);
        assert_eq!(saved_shell_id(&s), None);
    }

    #[test]
    fn a_saved_shell_round_trips_and_tells_its_listeners() {
        let s = PrefStore::memory();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let sub = on_shell_change(&s, move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        save_shell_id(&s, "wsl-Ubuntu");
        assert_eq!(saved_shell_id(&s), Some("wsl-Ubuntu".to_string()));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        sub.cancel();
        save_shell_id(&s, "pwsh");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn agent_keys_are_on_until_turned_off() {
        let s = PrefStore::memory();
        assert!(agent_keys(&s));
        set_agent_keys(&s, false);
        assert_eq!(s.get("flare.window.agentKeys"), Some("0".to_string()));
        assert!(!agent_keys(&s));
        s.set("flare.window.agentKeys", "x");
        assert!(agent_keys(&s));
    }
}
