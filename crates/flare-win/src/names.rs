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
}
