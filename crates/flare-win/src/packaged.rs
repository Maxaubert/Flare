//! M9 Task 11: whether this run is a Velopack install (and the verb executable path).
//!
//! A packaged install is `<root>\current\Flare.exe` with `<root>\Update.exe` beside `current`. A
//! portable copy carries `<root>\.portable` and is not packaged for our purposes (no verbs, no
//! updater). The check only looks at file names, so it works on any path and never starts anything.

use std::path::{Path, PathBuf};

/// True when `exe` sits in a folder named `current` (ASCII case-insensitive) whose parent holds
/// `Update.exe` and does not hold `.portable`.
pub fn is_packaged(exe: &Path) -> bool {
    install_root(exe).is_some()
}

/// The folder that holds `Update.exe` and `current`, when `exe` is a packaged install's exe.
pub fn install_root(exe: &Path) -> Option<PathBuf> {
    let current = exe.parent()?;
    let name = current.file_name()?.to_str()?;
    if !name.eq_ignore_ascii_case("current") {
        return None;
    }
    let root = current.parent()?;
    // sync-ok: startup-only existence probes of two marker files
    if root.join("Update.exe").is_file() && !root.join(".portable").exists() {
        Some(root.to_path_buf())
    } else {
        None
    }
}

/// The path the Explorer verbs point at: the exe as it is, when packaged.
pub fn verb_exe(exe: &Path) -> Option<String> {
    install_root(exe)?;
    Some(exe.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree(current_name: &str, update: bool, portable: bool) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Flare");
        let cur = root.join(current_name);
        fs::create_dir_all(&cur).unwrap();
        let exe = cur.join("Flare.exe");
        fs::write(&exe, b"x").unwrap();
        if update {
            fs::write(root.join("Update.exe"), b"x").unwrap();
        }
        if portable {
            fs::write(root.join(".portable"), b"").unwrap();
        }
        (dir, exe)
    }

    #[test]
    fn is_packaged_wants_current_and_update_exe() {
        let (_d, exe) = tree("current", true, false);
        assert!(is_packaged(&exe));
        let (_d, exe) = tree("current", false, false);
        assert!(!is_packaged(&exe));
        let (_d, exe) = tree("app", true, false);
        assert!(!is_packaged(&exe));
        let (_d, exe) = tree("Current", true, false);
        assert!(is_packaged(&exe));
        let (_d, exe) = tree("current", true, true);
        assert!(!is_packaged(&exe));
    }

    #[test]
    fn verb_exe_is_the_current_folder_exe() {
        let (_d, exe) = tree("current", true, false);
        assert_eq!(verb_exe(&exe), Some(exe.to_string_lossy().into_owned()));
        assert_eq!(
            install_root(&exe),
            exe.parent().unwrap().parent().map(Path::to_path_buf)
        );
        assert_eq!(verb_exe(Path::new(r"C:\dev\target\debug\flare.exe")), None);
    }
}
