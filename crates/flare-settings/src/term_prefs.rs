//! Terminal prefs (Task 3): the getter for the saved shell, and the agent hotkeys switch. M7 owns the
//! rest of the terminal settings.
//!
//! Keys `flare.term.shell` (the shell id; unset until chosen) and `flare.window.agentKeys` (a bool,
//! default ON: the agent hotkeys of the chord table are the app's while it is on).

use crate::kv::{PrefStore, Subscription, bool_pref_get, bool_pref_set};

const SHELL_KEY: &str = "flare.term.shell";
const AGENT_KEYS: &str = "flare.window.agentKeys";
const OPEN_PREVIEWS: &str = "flare.window.openPreviews";
const AGENT_EVAL: &str = "flare.web.agentEval";
const APP_NETWORK: &str = "flare.web.appNetwork";
const DEV_PREVIEW: &str = "flare.window.devPreview";
const WRAP_PREVIEWS: &str = "flare.window.wrapPreviews";

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

/// Wrap long lines in previews (Surfaces, 2026-10-22): the text panel continues a long line on the next row
/// instead of running off the right edge. Default ON; only a stored "0" is off. The panel header has a button
/// for it too; both write this one key.
pub fn wrap_previews(s: &PrefStore) -> bool {
    bool_pref_get(s, WRAP_PREVIEWS, true)
}

pub fn set_wrap_previews(s: &PrefStore, on: bool) {
    bool_pref_set(s, WRAP_PREVIEWS, on);
}

/// Let agents run scripts in pages (Surfaces stage 4): `browser.eval` for the agent. Default OFF; only a
/// stored "1" is on (S4). Read from the control channel's threads at every request.
pub fn agent_eval(s: &PrefStore) -> bool {
    bool_pref_get(s, AGENT_EVAL, false)
}

pub fn set_agent_eval(s: &PrefStore, on: bool) {
    bool_pref_set(s, AGENT_EVAL, on);
}

/// Let mini apps use the network (Surfaces stage 5): a page an agent wrote may load `https:` content only
/// when the agent asks AND this is on. Default OFF; only a stored "1" is on. Read from the control
/// channel's threads at every request.
pub fn app_network(s: &PrefStore) -> bool {
    bool_pref_get(s, APP_NETWORK, false)
}

pub fn set_app_network(s: &PrefStore, on: bool) {
    bool_pref_set(s, APP_NETWORK, on);
}

/// Offer previews of local servers (Surfaces stage 7): a tab whose output names a local dev server gets a
/// "Preview" chip that opens it in the web panel. Default ON; only a stored "0" is off.
pub fn dev_preview(s: &PrefStore) -> bool {
    bool_pref_get(s, DEV_PREVIEW, true)
}

pub fn set_dev_preview(s: &PrefStore, on: bool) {
    bool_pref_set(s, DEV_PREVIEW, on);
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
    fn scripts_for_agents_are_off_until_a_stored_one() {
        let s = PrefStore::memory();
        assert!(!agent_eval(&s));
        set_agent_eval(&s, true);
        assert!(agent_eval(&s));
        assert_eq!(s.get("flare.web.agentEval").as_deref(), Some("1"));
        set_agent_eval(&s, false);
        assert!(!agent_eval(&s));
        s.set("flare.web.agentEval", "junk");
        assert!(!agent_eval(&s), "an unknown value reads as the default");
        s.set("flare.web.agentEval", "true");
        assert!(!agent_eval(&s));
    }

    #[test]
    fn mini_apps_have_no_network_until_a_stored_one() {
        let s = PrefStore::memory();
        assert!(!app_network(&s));
        set_app_network(&s, true);
        assert!(app_network(&s));
        assert_eq!(s.get("flare.web.appNetwork").as_deref(), Some("1"));
        set_app_network(&s, false);
        assert!(!app_network(&s));
        s.set("flare.web.appNetwork", "junk");
        assert!(!app_network(&s), "an unknown value reads as the default");
        s.set("flare.web.appNetwork", "true");
        assert!(!app_network(&s));
    }

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
    fn wrapping_previews_is_on_until_a_stored_zero() {
        let s = PrefStore::memory();
        assert!(wrap_previews(&s));
        set_wrap_previews(&s, false);
        assert!(!wrap_previews(&s));
        assert_eq!(s.get("flare.window.wrapPreviews").as_deref(), Some("0"));
        set_wrap_previews(&s, true);
        assert!(wrap_previews(&s));
        s.set("flare.window.wrapPreviews", "junk");
        assert!(wrap_previews(&s), "an unknown value reads as the default");
    }

    #[test]
    fn local_server_previews_are_on_until_a_stored_zero() {
        let s = PrefStore::memory();
        assert!(dev_preview(&s));
        set_dev_preview(&s, false);
        assert!(!dev_preview(&s));
        assert_eq!(s.get("flare.window.devPreview").as_deref(), Some("0"));
        set_dev_preview(&s, true);
        assert!(dev_preview(&s));
        s.set("flare.window.devPreview", "junk");
        assert!(dev_preview(&s), "an unknown value reads as the default");
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
