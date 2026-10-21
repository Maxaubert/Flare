//! Terminal prefs (Task 3): the getter for the saved shell, and the agent hotkeys switch. M7 owns the
//! rest of the terminal settings.
//!
//! Keys `flare.term.shell` (the shell id; unset until chosen) and `flare.window.agentKeys` (a bool,
//! default ON: the agent hotkeys of the chord table are the app's while it is on).

use crate::kv::{PrefStore, bool_pref_get, bool_pref_set};

const SHELL_KEY: &str = "flare.term.shell";
const AGENT_KEYS: &str = "flare.window.agentKeys";

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

#[cfg(test)]
mod tests {
    use super::*;

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
