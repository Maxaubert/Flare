//! Which key the profiles on disk were encrypted with (rule 282). Chromium encrypts a profile's cookie values and
//! saved passwords with the key [`crate::macos::engine::Keychain`] names: the mock keychain's fixed key in a dev or
//! ad-hoc build, Flare's own keychain item in the Developer ID app. Both use the same default profile folder, so a
//! switch between them would leave every cookie unreadable and Chromium would drop them, silently. Nothing in a
//! profile says which key wrote it, so the engine's root folder carries a marker ([`MARKER`]); when the key changes,
//! the files written with the old key are set aside under a name of that key (`Cookies.mock-key`) and the ones the
//! new key wrote before, if any, are put back. Local Storage and IndexedDB are not encrypted with the key and stay
//! where they are. Pure file work: no keychain, no CEF.

use std::fs;
use std::path::{Path, PathBuf};

use crate::macos::engine::Keychain;

/// The marker file in the engine's root folder: `mock` or `own`.
pub const MARKER: &str = "flare-profile-key";

/// The files Chromium encrypts with the profile key (with their SQLite journals).
pub const KEYED_FILES: [&str; 4] = [
    "Cookies",
    "Cookies-journal",
    "Login Data",
    "Login Data-journal",
];

/// How deep under the root a keyed file is looked for (`mods/<name>/Network/Cookies` is 4).
const DEPTH: usize = 5;

/// The marker's word for a key.
pub fn word(k: Keychain) -> &'static str {
    match k {
        Keychain::Mock => "mock",
        Keychain::Own => "own",
    }
}

/// The key a marker names. A root without a marker was written with the mock key: every Mac build before the marker
/// used it (no Developer ID build had run Flare's window).
pub fn parse(text: Option<&str>) -> Keychain {
    match text.map(str::trim) {
        Some("own") => Keychain::Own,
        _ => Keychain::Mock,
    }
}

/// The name a keyed file is set aside under while key `k` is not the one in use.
pub fn aside_name(file: &str, k: Keychain) -> String {
    format!("{file}.{}-key", word(k))
}

/// Every folder under `root` (itself included, links not followed, [`DEPTH`] levels) holding a keyed file, or one
/// set aside for `k`.
fn folders_with_keyed_files(root: &Path, k: Keychain) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        let mut here = false;
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name();
            let name = name.to_string_lossy();
            if ft.is_dir() && depth < DEPTH {
                stack.push((e.path(), depth + 1));
            } else if ft.is_file()
                && KEYED_FILES
                    .iter()
                    .any(|f| name == *f || name == aside_name(f, k))
            {
                here = true;
            }
        }
        if here {
            out.push(d);
        }
    }
    out.sort();
    out
}

/// Writes the marker for key `now` into `root` when it does not name it yet. Called after `cef_initialize`: a root
/// folder made before CEF makes it changes how CEF creates the profile folders under it (measured 2026-11-09: a mod's
/// profile folder was then not on disk when the probe looked), so nothing here makes the root.
pub fn mark(root: &Path, now: Keychain) {
    let marker = root.join(MARKER);
    if !root.is_dir() || fs::read_to_string(&marker).ok().as_deref() == Some(word(now)) {
        return;
    }
    let tmp = root.join(format!("{MARKER}.tmp"));
    if fs::write(&tmp, word(now)).is_ok() {
        let _ = fs::rename(&tmp, &marker);
    }
}

/// Settles the profiles under `root` on key `now` before the engine opens them (a root that does not exist yet has
/// nothing to settle and is left for CEF to make; [`mark`] runs after). Returns what was done when the key changed
/// (for the log), else None. A failure to move a file is reported in the text, never fatal: the engine still starts
/// (Chromium then drops what it cannot read, as it would have).
pub fn settle(root: &Path, now: Keychain) -> Option<String> {
    if !root.is_dir() {
        return None;
    }
    let marker = root.join(MARKER);
    let before = parse(fs::read_to_string(&marker).ok().as_deref());
    let write_marker = || mark(root, now);
    if before == now {
        if !marker.is_file() {
            write_marker();
        }
        return None;
    }
    let (mut set_aside, mut restored, mut failed) = (0usize, 0usize, Vec::new());
    // Both kinds are looked for: what `before` wrote is set aside, what `now` wrote earlier comes back.
    let mut dirs = folders_with_keyed_files(root, before);
    dirs.extend(folders_with_keyed_files(root, now));
    dirs.sort();
    dirs.dedup();
    if dirs.is_empty() && !marker.is_file() {
        // A new root (or one with nothing keyed in it): nothing to settle.
        write_marker();
        return None;
    }
    for d in &dirs {
        for f in KEYED_FILES {
            let live = d.join(f);
            if live.is_file() {
                match fs::rename(&live, d.join(aside_name(f, before))) {
                    Ok(()) => set_aside += 1,
                    Err(e) => failed.push(format!("{}: {e}", live.display())),
                }
            }
        }
        for f in KEYED_FILES {
            let kept = d.join(aside_name(f, now));
            if kept.is_file() {
                match fs::rename(&kept, d.join(f)) {
                    Ok(()) => restored += 1,
                    Err(e) => failed.push(format!("{}: {e}", kept.display())),
                }
            }
        }
    }
    write_marker();
    let mut text = format!(
        "the profiles' key changed from {} to {}: {set_aside} file(s) written with the old key set aside as \
         *.{}-key, {restored} written with the new key earlier put back",
        word(before),
        word(now),
        word(before)
    );
    if !failed.is_empty() {
        text.push_str(&format!("; not moved: {}", failed.join(", ")));
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("flare-profile-key-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("dir");
        d
    }

    #[test]
    fn a_change_of_key_sets_the_old_keys_cookies_aside_and_brings_the_new_keys_back() {
        let root = tmp("swap");
        let web = root.join("Default/Network");
        let m = root.join("mods/notes/Network");
        fs::create_dir_all(&web).expect("dir");
        fs::create_dir_all(&m).expect("dir");
        fs::write(web.join("Cookies"), "dev").expect("write");
        fs::write(web.join("Cookies-journal"), "j").expect("write");
        fs::write(m.join("Cookies"), "mod dev").expect("write");
        fs::create_dir_all(root.join("Default/Local Storage")).expect("dir");
        fs::write(root.join("Default/Local Storage/x.ldb"), "kept").expect("write");

        // No marker: written with the mock key. Same key: nothing moves, the marker is written.
        assert_eq!(settle(&root, Keychain::Mock), None);
        assert_eq!(fs::read_to_string(root.join(MARKER)).unwrap(), "mock");
        assert!(web.join("Cookies").is_file());

        // The Developer ID app: the mock key's files go aside, nothing is lost.
        let note = settle(&root, Keychain::Own).expect("a change");
        assert!(note.contains("from mock to own"), "{note}");
        assert!(!web.join("Cookies").exists());
        assert_eq!(
            fs::read_to_string(web.join("Cookies.mock-key")).unwrap(),
            "dev"
        );
        assert!(web.join("Cookies-journal.mock-key").is_file());
        assert_eq!(
            fs::read_to_string(m.join("Cookies.mock-key")).unwrap(),
            "mod dev"
        );
        assert_eq!(fs::read_to_string(root.join(MARKER)).unwrap(), "own");
        // Not encrypted with the key: left alone.
        assert!(root.join("Default/Local Storage/x.ldb").is_file());

        // The signed app writes its own cookies; a dev run comes back and gets its own again.
        fs::write(web.join("Cookies"), "signed").expect("write");
        assert!(settle(&root, Keychain::Own).is_none());
        settle(&root, Keychain::Mock).expect("a change");
        assert_eq!(fs::read_to_string(web.join("Cookies")).unwrap(), "dev");
        assert_eq!(
            fs::read_to_string(web.join("Cookies.own-key")).unwrap(),
            "signed"
        );
        assert_eq!(fs::read_to_string(m.join("Cookies")).unwrap(), "mod dev");
        // And the signed app gets back what it wrote.
        settle(&root, Keychain::Own).expect("a change");
        assert_eq!(fs::read_to_string(web.join("Cookies")).unwrap(), "signed");
        assert_eq!(
            fs::read_to_string(web.join("Cookies.mock-key")).unwrap(),
            "dev"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_new_root_is_left_for_cef_and_marked_after_and_an_unknown_marker_counts_as_the_mock_key() {
        let root = tmp("new").join("Flare");
        assert_eq!(settle(&root, Keychain::Own), None);
        assert!(!root.exists(), "the root is CEF's to make");
        mark(&root, Keychain::Own);
        assert!(!root.exists());
        fs::create_dir_all(&root).expect("dir");
        mark(&root, Keychain::Own);
        assert_eq!(fs::read_to_string(root.join(MARKER)).unwrap(), "own");
        assert!(settle(&root, Keychain::Own).is_none());
        assert_eq!(parse(Some("own\n")), Keychain::Own);
        assert_eq!(parse(Some("garbage")), Keychain::Mock);
        assert_eq!(parse(None), Keychain::Mock);
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }
}
