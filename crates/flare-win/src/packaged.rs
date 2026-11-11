//! M9 Task 11: whether this run is a Velopack install (and the verb executable path).
//!
//! A packaged install is `<root>\current\Flare.exe` with `<root>\Update.exe` beside `current`. A
//! portable copy carries `<root>\.portable` and is not packaged for our purposes (no verbs, no
//! updater). The check only looks at file names, so it works on any path and never starts anything.
//!
//! macOS (port phase 8, rule 1047): the installed app is the bundle `vpk pack` made,
//! `<X>.app/Contents/MacOS/<exe>` with Velopack's `UpdateMac` beside it and its `sq.version` in
//! `Contents/Resources`; the root is the `.app`, and there are no Explorer verbs.

use std::path::{Path, PathBuf};

/// True when `exe` sits in a folder named `current` (ASCII case-insensitive) whose parent holds
/// `Update.exe` and does not hold `.portable` (on macOS also: a Velopack bundle, see [`bundle_root`]).
pub fn is_packaged(exe: &Path) -> bool {
    install_root(exe).is_some()
}

/// The folder that holds `Update.exe` and `current`, when `exe` is a packaged install's exe (on
/// macOS also: the `.app` of a Velopack bundle).
pub fn install_root(exe: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    if let Some(app) = bundle_root(exe) {
        return Some(app);
    }
    windows_root(exe)
}

/// The `.app` that `exe` is the program of, when it is a bundle `vpk pack` made: `exe` in
/// `<X>.app/Contents/MacOS` with `UpdateMac` beside it and `Contents/Resources/sq.version`.
pub fn bundle_root(exe: &Path) -> Option<PathBuf> {
    let app = app_of(exe)?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    // sync-ok: startup-only existence probes of two marker files
    (exe.file_name()? != "UpdateMac"
        && macos.join("UpdateMac").is_file()
        && contents.join("Resources").join("sq.version").is_file())
    .then(|| app.to_path_buf())
}

/// True when `exe` is the program of some `.app` bundle by its path alone (`<X>.app/Contents/MacOS/<exe>`),
/// Velopack's or not. Looks at names only and touches no file (rule 1128).
pub fn in_app_bundle(exe: &Path) -> bool {
    app_of(exe).is_some()
}

/// The `<X>.app` of `exe` when its path is `<X>.app/Contents/MacOS/<exe>` (names only).
fn app_of(exe: &Path) -> Option<&Path> {
    exe.file_name()?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app")
        .then_some(app)
}

fn windows_root(exe: &Path) -> Option<PathBuf> {
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

/// The path the Explorer verbs point at: the exe as it is, when packaged (never a Mac bundle: the
/// verbs are Explorer's).
pub fn verb_exe(exe: &Path) -> Option<String> {
    windows_root(exe)?;
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

    fn bundle(dir: &Path, update: bool, version: bool) -> PathBuf {
        let app = dir.join("Flare.app");
        let macos = app.join("Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        fs::write(macos.join("Flare"), b"x").unwrap();
        if update {
            fs::write(macos.join("UpdateMac"), b"x").unwrap();
        }
        if version {
            fs::write(app.join("Contents/Resources/sq.version"), b"<package/>").unwrap();
        }
        macos.join("Flare")
    }

    #[test]
    fn a_mac_bundle_vpk_made_is_its_own_root_and_has_no_verb() {
        let dir = tempfile::tempdir().unwrap();
        let exe = bundle(dir.path(), true, true);
        let app = dir.path().join("Flare.app");
        assert_eq!(bundle_root(&exe), Some(app.clone()));
        assert_eq!(bundle_root(&app.join("Contents/MacOS/UpdateMac")), None);
        assert_eq!(verb_exe(&exe), None);
        #[cfg(target_os = "macos")]
        {
            assert!(is_packaged(&exe));
            assert_eq!(install_root(&exe), Some(app));
        }
        #[cfg(not(target_os = "macos"))]
        assert!(!is_packaged(&exe));
        for (update, version) in [(false, true), (true, false), (false, false)] {
            let d = tempfile::tempdir().unwrap();
            let exe = bundle(d.path(), update, version);
            assert_eq!(bundle_root(&exe), None, "{update} {version}");
            assert!(!is_packaged(&exe));
        }
        // A dev bundle (no UpdateMac) and a folder that is not an app.
        let d = tempfile::tempdir().unwrap();
        let plain = d.path().join("Flare/Contents/MacOS");
        fs::create_dir_all(&plain).unwrap();
        fs::write(plain.join("UpdateMac"), b"x").unwrap();
        assert_eq!(bundle_root(&plain.join("Flare")), None);
    }

    #[test]
    fn in_app_bundle_reads_the_path_alone() {
        // Rule 1128: an ad hoc bundle (no UpdateMac, no sq.version, nothing on disk) is in a bundle.
        assert!(in_app_bundle(Path::new(
            "/Applications/Flare.app/Contents/MacOS/Flare"
        )));
        assert!(in_app_bundle(Path::new(
            "/tmp/x/Other.app/Contents/MacOS/flare"
        )));
        assert!(!is_packaged(Path::new(
            "/Applications/Flare.app/Contents/MacOS/Flare"
        )));
        // cargo run, a plain folder, the wrong level, a folder that is not an app.
        for p in [
            "/Users/me/Flare/target/debug/flare",
            "/Applications/Flare.app/Contents/Resources/Flare",
            "/Applications/Flare.app/Contents/MacOS",
            "/Applications/Flare/Contents/MacOS/Flare",
            "/Applications/Flare.app/Other/MacOS/Flare",
            r"C:\Flare\current\Flare.exe",
            "Flare",
            "",
        ] {
            assert!(!in_app_bundle(Path::new(p)), "{p}");
        }
        // A real Velopack bundle is in a bundle too.
        let dir = tempfile::tempdir().unwrap();
        assert!(in_app_bundle(&bundle(dir.path(), true, true)));
    }
}
