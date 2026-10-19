//! Path helpers shared by the e2e and the installer (Task 42).
//!
//! `%LOCALAPPDATA%` is asked of Windows' known-folder table first (the variable can be unset or
//! redirected by a sandbox), then the variable, then `<home>\AppData\Local`, so a machine with no
//! variable still finds the folder Velopack and the dictation models live in.

use std::path::PathBuf;

use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

/// The local application data folder: the known folder, else `LOCALAPPDATA`, else
/// `<USERPROFILE>\AppData\Local`. None when none of the three is known.
pub fn local_app_data() -> Option<PathBuf> {
    resolve(
        known_local_app_data(),
        std::env::var("LOCALAPPDATA").ok(),
        std::env::var_os("USERPROFILE").map(PathBuf::from),
    )
}

/// The pure rule behind [`local_app_data`]: the first of the three that is set and not empty.
fn resolve(known: Option<PathBuf>, env: Option<String>, home: Option<PathBuf>) -> Option<PathBuf> {
    known
        .or_else(|| env.filter(|s| !s.is_empty()).map(PathBuf::from))
        .or_else(|| home.map(|h| h.join("AppData").join("Local")))
}

/// Windows' answer for the LocalAppData known folder, or None when the call fails.
fn known_local_app_data() -> Option<PathBuf> {
    let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
    // sync-ok: a shell lookup of a known-folder path that reads the profile's record, no file I/O
    // SAFETY: the out-pointer is a local, the GUID is a static, and a null token means the current user.
    let hr = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_LocalAppData,
            KF_FLAG_DEFAULT as u32,
            std::ptr::null_mut(),
            &mut raw,
        )
    };
    if hr < 0 || raw.is_null() {
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

    #[test]
    fn nothing_known_gives_none() {
        assert_eq!(resolve(None, None, None), None);
    }

    #[test]
    fn the_real_known_folder_is_the_local_app_data_folder() {
        let got = known_local_app_data().expect("Windows knows its LocalAppData folder");
        let text = got.to_string_lossy().to_ascii_lowercase();
        assert!(text.ends_with(r"appdata\local"), "got {text}");
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
