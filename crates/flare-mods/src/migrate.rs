//! The one-time move of the old mods folder into the plugins folder (plugins phase 2c): Flare reads only
//! `<profile>\plugins\`, so at start, before the first scan, every folder of `<profile>\mods\` whose name is a legal
//! id and that the plugins folder does not hold already is moved there (a rename, on the same volume). A name the
//! plugins folder already holds stays where it is and is reported in Settings; a name that is not an id is left
//! alone. An emptied mods folder is removed; one that still holds something is left with it.
//!
//! A mod's switch (`mods.json`), its approval and its storage (the profile `flare-mod-<id>`) are tied to its id,
//! never to its folder, so a moved mod keeps all three.
//!
//! Worker-only: it renames folders. It never fails as a whole and never overwrites: a folder it cannot move is one
//! [`FolderError`] and every other folder is still moved.

use crate::discover::{FolderError, MAX_ENTRIES};
use crate::manifest::valid_id;
use std::path::Path;

/// What the move did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// The folders moved into the plugins folder, sorted.
    pub moved: Vec<String>,
    /// The folders that stayed in the mods folder, each with one sentence for Settings, sorted by name.
    pub kept: Vec<FolderError>,
    /// The mods folder was emptied and removed.
    pub removed: bool,
}

/// Moves the folders of `mods` into `plugins` (made when there is something to move). A missing `mods` is nothing
/// to do.
pub fn move_into_plugins(mods: &Path, plugins: &Path) -> Moved {
    let mut out = Moved::default();
    let entries = match std::fs::read_dir(mods) {
        // sync-ok: the scan worker
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            out.kept.push(FolderError {
                folder: "mods".to_string(),
                reason: format!(
                    "The old mods folder ({}) cannot be read, so nothing in it was moved to the plugins folder: {e}.",
                    mods.display()
                ),
            });
            return out;
        }
    };
    let mut names: Vec<String> = entries
        .take(MAX_ENTRIES)
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| valid_id(n))
        .collect();
    names.sort();
    for name in names {
        let to = plugins.join(&name);
        // sync-ok: the scan worker
        if std::fs::symlink_metadata(&to).is_ok() {
            out.kept.push(FolderError {
                reason: format!(
                    "\"{name}\" stayed in the old mods folder ({}) because the plugins folder already has a folder named \"{name}\"; move or remove one of them.",
                    mods.display()
                ),
                folder: name,
            });
            continue;
        }
        let moved = std::fs::create_dir_all(plugins) // sync-ok: the scan worker
            .and_then(|()| std::fs::rename(mods.join(&name), &to)); // sync-ok: the scan worker
        match moved {
            Ok(()) => out.moved.push(name),
            Err(e) => out.kept.push(FolderError {
                reason: format!(
                    "\"{name}\" stayed in the old mods folder ({}) because it could not be moved to the plugins folder: {e}.",
                    mods.display()
                ),
                folder: name,
            }),
        }
    }
    // Only an empty folder can be removed, so whatever could not move (or is not a plugin) keeps it.
    out.removed = std::fs::remove_dir(mods).is_ok(); // sync-ok: the scan worker
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A folder `name` under `root` with one file, so a move can be seen to carry its content.
    fn folder(root: &Path, name: &str, text: &str) {
        let f = root.join(name);
        fs::create_dir_all(&f).unwrap(); // sync-ok: test code only
        fs::write(f.join("mod.json"), text).unwrap(); // sync-ok: test code only
    }

    fn read(p: &Path) -> String {
        fs::read_to_string(p.join("mod.json")).unwrap() // sync-ok: test code only
    }

    #[test]
    fn every_id_folder_is_moved_and_the_empty_mods_folder_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (mods, plugins) = (dir.path().join("mods"), dir.path().join("plugins"));
        folder(&mods, "notes", "n");
        folder(&mods, "zeta", "z");
        let m = move_into_plugins(&mods, &plugins);
        assert_eq!(m.moved, ["notes", "zeta"]);
        assert!(m.kept.is_empty(), "{:?}", m.kept);
        assert!(m.removed);
        assert!(!mods.exists(), "the empty mods folder is gone");
        assert_eq!(read(&plugins.join("notes")), "n", "the content came along");
        assert_eq!(read(&plugins.join("zeta")), "z");
    }

    #[test]
    fn a_name_the_plugins_folder_holds_stays_and_is_named_in_one_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let (mods, plugins) = (dir.path().join("mods"), dir.path().join("plugins"));
        folder(&mods, "notes", "old");
        folder(&mods, "other", "o");
        folder(&plugins, "notes", "new");
        let m = move_into_plugins(&mods, &plugins);
        assert_eq!(m.moved, ["other"]);
        assert_eq!(m.kept.len(), 1);
        assert_eq!(m.kept[0].folder, "notes");
        let why = &m.kept[0].reason;
        assert!(
            why.contains("\"notes\"") && why.contains("already has a folder"),
            "{why}"
        );
        assert_eq!(why.matches(". ").count(), 0, "one sentence: {why}");
        assert!(why.ends_with('.'), "{why}");
        assert!(!why.contains('\u{2014}'));
        // Neither side was touched, and the mods folder stays with what could not move.
        assert_eq!(read(&plugins.join("notes")), "new");
        assert_eq!(read(&mods.join("notes")), "old");
        assert!(!m.removed && mods.is_dir());
    }

    #[test]
    fn a_folder_that_is_not_an_id_and_loose_files_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (mods, plugins) = (dir.path().join("mods"), dir.path().join("plugins"));
        folder(&mods, "Backup_Old", "b");
        folder(&mods, ".git", "g");
        fs::write(mods.join("readme.txt"), "x").unwrap(); // sync-ok: test code only
        folder(&mods, "notes", "n");
        let m = move_into_plugins(&mods, &plugins);
        assert_eq!(m.moved, ["notes"]);
        assert!(
            m.kept.is_empty(),
            "a non-id folder is not reported: {:?}",
            m.kept
        );
        assert!(mods.join("Backup_Old").is_dir() && mods.join(".git").is_dir());
        assert!(mods.join("readme.txt").is_file());
        assert!(!plugins.join("Backup_Old").exists());
        assert!(!m.removed);
    }

    #[test]
    fn nothing_is_moved_twice() {
        let dir = tempfile::tempdir().unwrap();
        let (mods, plugins) = (dir.path().join("mods"), dir.path().join("plugins"));
        folder(&mods, "notes", "n");
        folder(&mods, "Keep_Me", "k");
        let first = move_into_plugins(&mods, &plugins);
        assert_eq!(first.moved, ["notes"]);
        let again = move_into_plugins(&mods, &plugins);
        assert_eq!(
            again,
            Moved::default(),
            "the second run finds nothing to move"
        );
        assert_eq!(read(&plugins.join("notes")), "n");
        // Once the mods folder is gone, a run is nothing at all.
        fs::remove_dir_all(&mods).unwrap(); // sync-ok: test code only
        assert_eq!(move_into_plugins(&mods, &plugins), Moved::default());
        assert!(!mods.exists(), "a missing mods folder is not made");
    }

    #[test]
    fn a_missing_plugins_folder_is_made_only_when_something_moves() {
        let dir = tempfile::tempdir().unwrap();
        let (mods, plugins) = (dir.path().join("mods"), dir.path().join("plugins"));
        fs::create_dir_all(mods.join("Not_An_Id")).unwrap(); // sync-ok: test code only
        let m = move_into_plugins(&mods, &plugins);
        assert!(m.moved.is_empty() && !plugins.exists());
        folder(&mods, "notes", "n");
        move_into_plugins(&mods, &plugins);
        assert!(plugins.join("notes").is_dir());
    }
}
