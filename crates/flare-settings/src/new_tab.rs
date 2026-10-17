//! NewTabMode and folder (Task 3): what the + button does. Keys `flare.newtab.mode` ("folder" | "ask",
//! fallback folder) and `flare.newtab.folder` (the folder a new tab opens in; "" is the user's own folder).

use crate::kv::{EnumPref, PrefStore, Subscription, on_any_change};

const MODE: EnumPref = EnumPref {
    key: "flare.newtab.mode",
    allowed: &["folder", "ask"],
    fallback: "folder",
};
const FOLDER_KEY: &str = "flare.newtab.folder";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewTabMode {
    Folder,
    Ask,
}

pub fn new_tab_mode(s: &PrefStore) -> NewTabMode {
    match MODE.get(s) {
        "ask" => NewTabMode::Ask,
        _ => NewTabMode::Folder,
    }
}

/// The folder a new tab opens in; "" means the user's own folder.
pub fn new_tab_folder(s: &PrefStore) -> String {
    s.get(FOLDER_KEY).unwrap_or_default()
}

/// Stores the mode, and the folder when one is given (`None` keeps the stored folder, so a visit to
/// "ask" does not forget the chosen folder).
pub fn set_new_tab_mode(s: &PrefStore, m: NewTabMode, folder: Option<&str>) {
    let word = match m {
        NewTabMode::Folder => "folder",
        NewTabMode::Ask => "ask",
    };
    MODE.set(s, word);
    if let Some(f) = folder {
        s.set(FOLDER_KEY, f);
    }
}

/// Fires for both the mode and the folder.
pub fn on_new_tab_change(s: &PrefStore, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
    on_any_change(s, &[MODE.key, FOLDER_KEY], cb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tab_defaults_to_folder_mode_and_the_users_folder() {
        let s = PrefStore::memory();
        assert_eq!(new_tab_mode(&s), NewTabMode::Folder);
        assert_eq!(new_tab_folder(&s), "");
    }

    #[test]
    fn new_tab_asks_only_when_asked_to() {
        let s = PrefStore::memory();
        set_new_tab_mode(&s, NewTabMode::Ask, None);
        assert_eq!(new_tab_mode(&s), NewTabMode::Ask);
    }

    #[test]
    fn new_tab_keeps_the_folder_across_ask() {
        let s = PrefStore::memory();
        set_new_tab_mode(&s, NewTabMode::Folder, Some("D:/work"));
        set_new_tab_mode(&s, NewTabMode::Ask, None);
        set_new_tab_mode(&s, NewTabMode::Folder, None);
        assert_eq!(new_tab_folder(&s), "D:/work");
    }

    #[test]
    fn new_tab_clearing_the_folder_reads_the_users_own() {
        let s = PrefStore::memory();
        set_new_tab_mode(&s, NewTabMode::Folder, Some("D:/work"));
        set_new_tab_mode(&s, NewTabMode::Folder, Some(""));
        assert_eq!(new_tab_folder(&s), "");
        assert_eq!(new_tab_mode(&s), NewTabMode::Folder);
    }

    #[test]
    fn new_tab_junk_mode_reads_as_folder() {
        let s = PrefStore::memory();
        s.set("flare.newtab.mode", "tabs");
        assert_eq!(new_tab_mode(&s), NewTabMode::Folder);
    }
}
