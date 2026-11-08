//! Path helpers shared by the e2e and the installer (Task 42), and the new-tab menu's starter folders.
//!
//! `%LOCALAPPDATA%` is asked of Windows' known-folder table first (the variable can be unset or
//! redirected by a sandbox), then the variable, then `<home>\AppData\Local`, so a machine with no
//! variable still finds the folder Velopack and the dictation models live in.

use std::path::PathBuf;

#[cfg(windows)]
use windows_sys::Win32::System::Com::CoTaskMemFree;
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData,
    FOLDERID_Profile, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
#[cfg(windows)]
use windows_sys::core::GUID;

/// The local application data folder: the known folder, else `LOCALAPPDATA`, else
/// `<USERPROFILE>\AppData\Local`. None when none of the three is known.
#[cfg(windows)]
pub fn local_app_data() -> Option<PathBuf> {
    resolve(
        known_local_app_data(),
        std::env::var("LOCALAPPDATA").ok(),
        std::env::var_os("USERPROFILE").map(PathBuf::from),
    )
}

/// The pure rule behind [`local_app_data`]: the first of the three that is set and not empty.
#[cfg(any(windows, test))]
fn resolve(known: Option<PathBuf>, env: Option<String>, home: Option<PathBuf>) -> Option<PathBuf> {
    known
        .or_else(|| env.filter(|s| !s.is_empty()).map(PathBuf::from))
        .or_else(|| home.map(|h| h.join("AppData").join("Local")))
}

/// The folders the new-tab menu offers while it has few recents (2026-10-27): Documents, Downloads, Desktop and
/// the user folder, in that order, those that exist. Each is Windows' known folder, so a redirected Documents
/// (OneDrive) is the one Explorer shows. It looks at the disk: call it on a worker, never on the UI thread (rule 34).
#[cfg(windows)]
pub fn starter_folders() -> Vec<PathBuf> {
    [
        &FOLDERID_Documents,
        &FOLDERID_Downloads,
        &FOLDERID_Desktop,
        &FOLDERID_Profile,
    ]
    .into_iter()
    .filter_map(known_folder)
    // sync-ok: the existence check runs on the caller's worker thread, never on the UI thread
    .filter(|p| p.is_dir())
    .collect()
}

/// Windows' answer for the LocalAppData known folder, or None when the call fails.
#[cfg(windows)]
fn known_local_app_data() -> Option<PathBuf> {
    known_folder(&FOLDERID_LocalAppData)
}

/// Windows' answer for a known folder, or None when the call fails.
#[cfg(windows)]
fn known_folder(id: &GUID) -> Option<PathBuf> {
    let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
    // sync-ok: a shell lookup of a known-folder path that reads the profile's record, no file I/O
    // SAFETY: the out-pointer is a local, the GUID is a static, and a null token means the current user.
    let hr =
        unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT as u32, std::ptr::null_mut(), &mut raw) };
    if hr < 0 || raw.is_null() {
        if !raw.is_null() {
            // SAFETY: a failed call may still hand back an allocation; it is freed once.
            unsafe { CoTaskMemFree(raw.cast()) };
        }
        return None;
    }
    let mut len = 0usize;
    // SAFETY: SHGetKnownFolderPath returned a NUL-terminated UTF-16 string we now own.
    unsafe {
        while *raw.add(len) != 0 {
            len += 1;
        }
    }
    // SAFETY: `len` code units were read above and stay inside the allocation.
    let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, len) });
    // SAFETY: the caller frees the string SHGetKnownFolderPath allocated, exactly once.
    unsafe { CoTaskMemFree(raw.cast()) };
    Some(PathBuf::from(text))
}

/// The roaming application data folder Flare's default profile lives under: `%APPDATA%` on Windows
/// (the variable, as before), `~/Library/Application Support` on macOS. None when unknown.
#[cfg(windows)]
pub fn roaming_app_data() -> Option<String> {
    std::env::var("APPDATA").ok()
}

/// macOS: `~/Library/Application Support`, where the default profile (`.../Flare`) lives.
#[cfg(not(windows))]
pub fn roaming_app_data() -> Option<String> {
    local_app_data().map(|p| p.to_string_lossy().into_owned())
}

/// macOS: Flare's log folder, `~/Library/Logs/Flare` (Console.app lists it under Log Reports).
#[cfg(not(windows))]
pub fn app_logs_dir() -> Option<PathBuf> {
    home().map(|h| h.join("Library").join("Logs").join("Flare"))
}

/// macOS: Flare's cache folder, `~/Library/Caches/Flare`: what may be thrown away and made again
/// (the system may clear it when the disk is low). Nothing is cached on disk yet; the web panel's
/// caches (port phase 5) go here.
#[cfg(not(windows))]
pub fn app_caches_dir() -> Option<PathBuf> {
    home().map(|h| h.join("Library").join("Caches").join("Flare"))
}

/// The user's home folder off Windows: `$HOME`, None when unset or empty.
#[cfg(not(windows))]
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// The per-user application data folder on macOS: `~/Library/Application Support`.
#[cfg(not(windows))]
pub fn local_app_data() -> Option<PathBuf> {
    home().map(|h| h.join("Library").join("Application Support"))
}

/// Documents, Downloads, Desktop and the home folder, those that exist (macOS).
#[cfg(not(windows))]
pub fn starter_folders() -> Vec<PathBuf> {
    let Some(home) = home() else {
        return Vec::new();
    };
    [
        home.join("Documents"),
        home.join("Downloads"),
        home.join("Desktop"),
        home,
    ]
    .into_iter()
    // sync-ok: the existence check runs on the caller's worker thread, never on the UI thread
    .filter(|p| p.is_dir())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_known_folder_wins_over_the_variable_and_the_home() {
        let got = resolve(
            Some(PathBuf::from(r"C:\Known\Local")),
            Some(r"C:\Env".into()),
            Some(PathBuf::from(r"C:\Home")),
        );
        assert_eq!(got, Some(PathBuf::from(r"C:\Known\Local")));
    }

    #[test]
    fn the_variable_is_used_when_the_known_folder_is_missing() {
        let got = resolve(
            None,
            Some(r"C:\Env\Local".into()),
            Some(PathBuf::from(r"C:\Home")),
        );
        assert_eq!(got, Some(PathBuf::from(r"C:\Env\Local")));
    }

    #[test]
    fn an_empty_variable_falls_through_to_the_home() {
        let got = resolve(None, Some(String::new()), Some(PathBuf::from(r"C:\Home")));
        assert_eq!(
            got,
            Some(PathBuf::from(r"C:\Home").join("AppData").join("Local"))
        );
    }

    #[test]
    fn the_home_fallback_is_appdata_local_under_it() {
        let got = resolve(None, None, Some(PathBuf::from(r"C:\Users\me")));
        assert_eq!(
            got,
            Some(Path::new(r"C:\Users\me").join("AppData").join("Local"))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn the_mac_folders_are_the_library_ones() {
        let h = home().expect("a home folder");
        assert_eq!(
            roaming_app_data().map(PathBuf::from),
            Some(h.join("Library/Application Support"))
        );
        assert_eq!(app_logs_dir(), Some(h.join("Library/Logs/Flare")));
        assert_eq!(app_caches_dir(), Some(h.join("Library/Caches/Flare")));
    }

    #[test]
    fn nothing_known_gives_none() {
        assert_eq!(resolve(None, None, None), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_real_known_folder_is_the_local_app_data_folder() {
        let got = known_local_app_data().expect("Windows knows its LocalAppData folder");
        let text = got.to_string_lossy().to_ascii_lowercase();
        assert!(text.ends_with(r"appdata\local"), "got {text}");
    }

    #[cfg(windows)]
    #[test]
    fn the_starter_folders_exist_and_end_with_the_user_folder() {
        let got = starter_folders();
        assert!(got.iter().all(|p| p.is_dir()), "{got:?}");
        let home = known_folder(&FOLDERID_Profile).expect("Windows knows the user folder");
        assert_eq!(got.last(), Some(&home), "{got:?}");
    }

    #[test]
    fn no_crate_reads_localappdata_directly() {
        // The scan covers flare and flare-win source (not this file, which holds the pattern).
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let needles = [
            concat!("env::var(\"", "LOCALAPPDATA", "\")"),
            concat!("env::var_os(\"", "LOCALAPPDATA", "\")"),
        ];
        let mut hits = Vec::new();
        for krate in ["flare", "flare-win"] {
            let mut stack = vec![root.join(krate).join("src")];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().is_some_and(|x| x == "rs")
                        && p.file_name().is_some_and(|n| n != "paths.rs")
                    {
                        let text = std::fs::read_to_string(&p).unwrap_or_default();
                        if needles.iter().any(|n| text.contains(n)) {
                            hits.push(p.display().to_string());
                        }
                    }
                }
            }
        }
        assert!(hits.is_empty(), "read LOCALAPPDATA directly in {hits:?}");
    }
}
