//! The new-tab folder (Task 3): where the + button opens a tab. Key `flare.newtab.folder` (the folder a new
//! tab opens in; "" is the user's own folder).
//!
//! There is no "ask where each time" mode any more (2026-10-27): the folder menu's "Choose a folder…" row took
//! its place. The old key `flare.newtab.mode` is no longer read, so a stored "ask" opens in the folder like
//! "folder" does; Settings' reset still clears it.

use crate::kv::{PrefStore, Subscription, on_any_change};

/// The retired mode key: kept only so the import and the row's reset know its name.
pub const MODE_KEY: &str = "flare.newtab.mode";
const FOLDER_KEY: &str = "flare.newtab.folder";

/// The folder a new tab opens in; "" means the user's own folder.
pub fn new_tab_folder(s: &PrefStore) -> String {
    s.get(FOLDER_KEY).unwrap_or_default()
}

/// Stores the folder a new tab opens in; "" goes back to the user's own folder.
pub fn set_new_tab_folder(s: &PrefStore, folder: &str) {
    s.set(FOLDER_KEY, folder);
}

/// Fires when the folder changes.
pub fn on_new_tab_change(s: &PrefStore, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
    on_any_change(s, &[FOLDER_KEY], cb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tab_defaults_to_the_users_folder() {
        let s = PrefStore::memory();
        assert_eq!(new_tab_folder(&s), "");
    }

    #[test]
    fn new_tab_clearing_the_folder_reads_the_users_own() {
        let s = PrefStore::memory();
        set_new_tab_folder(&s, "D:/work");
        assert_eq!(new_tab_folder(&s), "D:/work");
        set_new_tab_folder(&s, "");
        assert_eq!(new_tab_folder(&s), "");
    }

    #[test]
    fn a_stored_ask_mode_keeps_the_folder() {
        // A profile from before 2026-10-27 may still say "ask": the folder is what counts.
        let s = PrefStore::memory();
        s.set(MODE_KEY, "ask");
        set_new_tab_folder(&s, "D:/work");
        assert_eq!(new_tab_folder(&s), "D:/work");
    }
}
