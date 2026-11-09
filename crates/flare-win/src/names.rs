//! Every Flare name that the outside world sees, in one file (M9 Task 1; the right-hand side of the
//! M9 plan's Appendix B). Nothing else spells these out, so a rename is one edit, and the neutrality
//! scan (M9 Task 35) reads this file.

/// The Velopack pack id (the install folder name under `%LOCALAPPDATA%` and the update key).
pub const APP_ID: &str = "Flare";
/// The product and window title.
pub const PRODUCT_TITLE: &str = "Flare";
/// The app executable.
pub const EXE_NAME: &str = "Flare.exe";
/// The installer file that the release workflow attaches (verified in M9 Task 29).
pub const SETUP_NAME: &str = "Flare-win-Setup.exe";
/// The folder under `%LOCALAPPDATA%` that Velopack installs into.
pub const INSTALL_FOLDER: &str = "Flare";
/// The profile folder under `%APPDATA%`.
pub const PROFILE_FOLDER: &str = "Flare";
/// The log folder, inside the profile folder.
pub const LOG_FOLDER: &str = "logs";
/// The dictation models folder under `%LOCALAPPDATA%`.
pub const DICTATION_FOLDER: &str = "FlareDictation";
/// The dictation hook lock file in `%TEMP%` (one hook per user; never Prism Terminal's lock).
pub const DICTATION_LOCK: &str = "Flare-dictation-key.lock";
/// The prefix of the dictation link pipe (`\\.\pipe\flare-dictation-<hash>`).
pub const DICTATION_PIPE_PREFIX: &str = "flare-dictation-";
/// The file that holds the link key, inside the dictation folder.
pub const DICTATION_LINK_KEY: &str = "link.key";
/// The copy name on the dictation link for the packaged app.
pub const COPY_NAME: &str = "Flare";
/// The copy name on the dictation link for any other build.
pub const COPY_NAME_DEV: &str = "Flare (Dev)";
/// The resident Parakeet host executable.
#[cfg(windows)]
pub const PARAKEET_HOST_EXE: &str = "flare-parakeet-host.exe";
/// The resident Parakeet host executable (macOS: no extension).
#[cfg(not(windows))]
pub const PARAKEET_HOST_EXE: &str = "flare-parakeet-host";
/// The engine folder beside the executable.
pub const ENGINE_FOLDER: &str = "whisper";
/// Env seam (read only through `e2e::global().env`): a WAV played as the microphone.
pub const SEAM_MIC: &str = concat!("FLARE_E2E", "_MIC");
/// Env seam: `1` or `0` instead of the DXGI NVIDIA probe.
pub const SEAM_NVIDIA: &str = concat!("FLARE_E2E", "_NVIDIA");
/// Env seam: the scenario's own link pipe name.
pub const SEAM_DICT_PIPE: &str = concat!("FLARE_E2E", "_DICT_PIPE");
/// Env seam: the scenario's own link key.
pub const SEAM_DICT_KEY: &str = concat!("FLARE_E2E", "_DICT_KEY");
/// Env seam: `stall` makes the store's downloads come from a slow in-memory stand-in.
pub const SEAM_FETCH: &str = concat!("FLARE_E2E", "_FETCH");
/// Env seam: the models folder (else `%LOCALAPPDATA%\FlareDictation`).
pub const SEAM_DICTATION_ROOT: &str = concat!("FLARE_DICTATION", "_ROOT");
/// Env seam: the engine folder (else `<exe dir>\whisper`).
pub const SEAM_WHISPER_DIR: &str = concat!("FLARE_WHISPER", "_DIR");
/// Env seam: the Claude plugin template folder (read only under --e2e).
pub const SEAM_PLUGIN_DIR: &str = concat!("FLARE_CLAUDE", "_PLUGIN_DIR");
/// The name of the Explorer verb key, under `Directory\shell` and `Directory\Background\shell`.
pub const VERB_KEY: &str = "Flare";
/// The Explorer verb label.
pub const VERB_LABEL: &str = "Open in Flare";
/// The Application User Model ID that Flare sets on its windows.
pub const AUMID: &str = "com.flare.terminal";
/// The prefix of the single-instance mutex (`Local\<prefix>.<sid>.<session>.<hash>`).
pub const MUTEX_PREFIX: &str = "Flare.Single";
/// The prefix of the single-instance hand-off pipe (`\\.\pipe\<prefix>.<...>`).
pub const PIPE_PREFIX: &str = "Flare.Handoff";
/// The Claude Code plugin name.
pub const PLUGIN_NAME: &str = "flare-status";
/// The OSC 777 signal name that the agent hooks write.
pub const HOOK_SIGNAL: &str = "flare-agent";
/// The environment variable that carries a tab id into a shell.
pub const TAB_ID_VAR: &str = "FLARE_TAB_ID";
/// The public-release repository (the updater reads it only when live updates are compiled in).
pub const REPO_URL: &str = "https://github.com/Maxaubert/Flare";
/// The registry prefix that the e2e sandbox writes under `HKCU` (never the real `Software\Classes`).
pub const E2E_REG_PREFIX: &str = r"Software\FlareE2E\";
/// The file name of the "verb off" marker, inside the profile folder.
pub const OFF_MARKER_FILE: &str = "shell-verb-off";

/// The taskbar identity of a run (owner, 2026-10-22). The default profile keeps [`AUMID`], so the
/// installed Flare groups with its Start-menu shortcut. A run with its own `--user-data-dir` (the
/// owner's stable copy) gets `com.flare.terminal.p<16 hex>`: the FNV-1a 64 hash of the profile path,
/// lowercased, with `/` read as `\` and trailing separators dropped. Stable across runs, so the
/// shortcut of that copy can carry it; at most 36 characters, letters, digits and dots only (an
/// AUMID may hold 128).
pub fn aumid_for_profile(user_data_dir: Option<&std::path::Path>) -> String {
    let Some(dir) = user_data_dir else {
        return AUMID.to_string();
    };
    // A `\\?\` prefix is dropped and a relative path is read against the cwd, so the spellings of
    // one folder give one identity (no canonicalize: the folder may not exist yet).
    let lossy = dir.to_string_lossy();
    let raw = lossy.strip_prefix(r"\\?\").unwrap_or(&lossy).to_string();
    let abs = if std::path::Path::new(&raw).is_absolute() || raw.starts_with(['/', '\\']) {
        raw
    } else {
        match std::env::current_dir() {
            Ok(c) => format!("{}\\{raw}", c.to_string_lossy().trim_start_matches(r"\\?\")),
            Err(_) => raw,
        }
    };
    let text = abs.to_lowercase().replace('/', "\\");
    let text = text.trim_end_matches('\\');
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{AUMID}.p{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_flare_name_says_the_other_product() {
        let other = ["pri", "sm"].concat();
        let all = [
            APP_ID,
            PRODUCT_TITLE,
            EXE_NAME,
            SETUP_NAME,
            INSTALL_FOLDER,
            PROFILE_FOLDER,
            LOG_FOLDER,
            DICTATION_FOLDER,
            VERB_KEY,
            VERB_LABEL,
            AUMID,
            MUTEX_PREFIX,
            PIPE_PREFIX,
            PLUGIN_NAME,
            HOOK_SIGNAL,
            TAB_ID_VAR,
            REPO_URL,
            E2E_REG_PREFIX,
            OFF_MARKER_FILE,
            DICTATION_LOCK,
            DICTATION_PIPE_PREFIX,
            DICTATION_LINK_KEY,
            COPY_NAME,
            COPY_NAME_DEV,
            PARAKEET_HOST_EXE,
            ENGINE_FOLDER,
            SEAM_MIC,
            SEAM_NVIDIA,
            SEAM_DICT_PIPE,
            SEAM_DICT_KEY,
            SEAM_FETCH,
            SEAM_DICTATION_ROOT,
            SEAM_WHISPER_DIR,
            SEAM_PLUGIN_DIR,
        ];
        for name in all {
            assert!(!name.to_ascii_lowercase().contains(&other), "{name}");
        }
    }

    #[test]
    fn verb_label_and_aumid_are_the_flare_values() {
        assert_eq!(VERB_LABEL, "Open in Flare");
        assert_eq!(AUMID, "com.flare.terminal");
        assert_eq!(E2E_REG_PREFIX, "Software\\FlareE2E\\");
    }
    #[test]
    fn aumid_is_the_plain_one_for_the_default_profile_and_per_profile_otherwise() {
        use std::path::Path;
        assert_eq!(aumid_for_profile(None), AUMID);
        let a = aumid_for_profile(Some(Path::new(
            "C:\\Users\\x\\AppData\\Local\\StableFlare\\profile",
        )));
        assert!(a.starts_with("com.flare.terminal.p"));
        assert_eq!(a.len(), AUMID.len() + 2 + 16);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'));
        // Case, slash form and a trailing separator do not change it.
        let b = aumid_for_profile(Some(Path::new(
            "c:/users/X/appdata/local/stableflare/profile/",
        )));
        assert_eq!(a, b);
        let c = aumid_for_profile(Some(Path::new("C:\\other")));
        assert_ne!(a, c);
        assert!(a.len() <= 128);
    }
}
