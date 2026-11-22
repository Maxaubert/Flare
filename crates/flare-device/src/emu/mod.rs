//! The Android emulator's pure parts (Android plan PR A2), shared by Flare's side (`emu_client`, `EmuSource`) and the
//! emu host (`flare-emu-host`, its own crate, so the gRPC stack never enters Flare's process):
//!
//! - [`sdk`]: where an Android SDK is (the person's, or Flare's own folder), its emulator's version and the floors.
//!   Flare only reads the person's SDK, never writes it (rule 1361).
//! - [`discovery`]: the running emulators' discovery files (`pid_<pid>.ini`): gRPC port, token, serial. The token is
//!   held in a type whose `Debug` hides it (rule 1362).
//! - [`avds`]: the virtual devices (`<name>.ini` and `config.ini`) in the person's and Flare's AVD folders.
//! - [`boot`]: the command line Flare boots an emulator with (rule 1362) and the free console port.
//! - [`accel`]: `emulator -accel-check` read back (Windows Hypervisor Platform, or Hypervisor.framework).
//! - [`region`]: the frame region's place (a `flare` folder in the discovery folder, rule 1363), its URL and size.
//! - [`keys`]: the panel's buttons and text as the emulator's key events (rule 1368).
//! - [`setup`]: Google's package lists read back, what Flare's own SDK folder needs, the licences (PR A2b, rule 1361).
//! - [`flare_avd`]: Flare's own device, "Flare Pixel", written by hand (PR A2b).
//! - [`agent`]: the agent helper `flare-agent.jar` (PR A3, rule 1369): its hash, its lines, its tree in the tree model,
//!   and the `uiautomator dump` fallback.

pub mod accel;
pub mod agent;
pub mod avds;
pub mod boot;
pub mod discovery;
pub mod flare_avd;
pub mod keys;
pub mod region;
pub mod sdk;
pub mod setup;

use std::path::PathBuf;

/// The places and settings the Android parts read from the environment, gathered once so the parsers stay pure and
/// the tests can give their own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Places {
    /// `ANDROID_HOME`.
    pub android_home: Option<PathBuf>,
    /// `ANDROID_SDK_ROOT` (older name).
    pub android_sdk_root: Option<PathBuf>,
    /// `ANDROID_AVD_HOME`: the person's AVD folder, when set.
    pub android_avd_home: Option<PathBuf>,
    /// `ANDROID_USER_HOME` (its `avd` folder), then `ANDROID_EMULATOR_HOME` (older).
    pub android_user_home: Option<PathBuf>,
    pub android_emulator_home: Option<PathBuf>,
    /// `%LOCALAPPDATA%` (Windows).
    pub local_app_data: Option<PathBuf>,
    /// The home folder.
    pub home: Option<PathBuf>,
}

impl Places {
    /// From this process's environment. Reads no file.
    pub fn from_env() -> Places {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Places {
            android_home: var("ANDROID_HOME"),
            android_sdk_root: var("ANDROID_SDK_ROOT"),
            android_avd_home: var("ANDROID_AVD_HOME"),
            android_user_home: var("ANDROID_USER_HOME"),
            android_emulator_home: var("ANDROID_EMULATOR_HOME"),
            local_app_data: var("LOCALAPPDATA"),
            home: var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }),
        }
    }

    /// Flare's own folder for what it downloads (`%LOCALAPPDATA%\Flare` on Windows, `~/Library/Application
    /// Support/Flare` on macOS). Flare's own SDK and AVDs go in it, never in the person's (rule 1361).
    pub fn flare_dir(&self) -> Option<PathBuf> {
        if cfg!(target_os = "macos") {
            Some(
                self.home
                    .as_ref()?
                    .join("Library/Application Support/Flare"),
            )
        } else {
            Some(self.local_app_data.as_ref()?.join("Flare"))
        }
    }

    /// Flare's own SDK folder (`android-sdk` in [`Places::flare_dir`]).
    pub fn flare_sdk(&self) -> Option<PathBuf> {
        Some(self.flare_dir()?.join("android-sdk"))
    }

    /// Flare's own AVD folder (`avd` beside its SDK); `ANDROID_AVD_HOME` for the emulators that run its devices.
    pub fn flare_avds(&self) -> Option<PathBuf> {
        Some(self.flare_dir()?.join("avd"))
    }

    /// The emulator's discovery folder, where each running emulator writes `pid_<pid>.ini`:
    /// `%LOCALAPPDATA%\Temp\avd\running` (Windows, as `emulator -help-grpc-use-token` names it) or
    /// `~/Library/Caches/TemporaryItems/avd/running` (macOS). Spike S5, 2026-11-22.
    pub fn discovery_dir(&self) -> Option<PathBuf> {
        if cfg!(target_os = "macos") {
            Some(
                self.home
                    .as_ref()?
                    .join("Library/Caches/TemporaryItems/avd/running"),
            )
        } else {
            Some(self.local_app_data.as_ref()?.join("Temp/avd/running"))
        }
    }
}

/// `key=value` lines (the emulator's `.ini` and `.properties` files): trimmed, blank lines and `#` comments skipped, a
/// line without `=` skipped. Later keys win.
pub fn parse_ini(text: &str) -> std::collections::BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with(';'))
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn ini_lines_read_as_keys_and_values() {
        let m = parse_ini("# c\n\n a = b \nc=d=e\nnot a pair\n;x\nc=f\n");
        assert_eq!(m.get("a").map(String::as_str), Some("b"));
        assert_eq!(m.get("c").map(String::as_str), Some("f"), "later wins");
        assert_eq!(m.len(), 2);
        assert_eq!(parse_ini("k=v=w")["k"], "v=w", "only the first = splits");
    }

    #[test]
    fn flares_own_folders_and_the_discovery_folder_are_per_os() {
        let p = Places {
            local_app_data: Some("C:/Users/a/AppData/Local".into()),
            home: Some("/Users/a".into()),
            ..Places::default()
        };
        if cfg!(target_os = "macos") {
            assert_eq!(
                p.flare_sdk().unwrap(),
                Path::new("/Users/a/Library/Application Support/Flare/android-sdk")
            );
            assert_eq!(
                p.discovery_dir().unwrap(),
                Path::new("/Users/a/Library/Caches/TemporaryItems/avd/running")
            );
        } else {
            assert_eq!(
                p.flare_sdk().unwrap(),
                Path::new("C:/Users/a/AppData/Local/Flare/android-sdk")
            );
            assert_eq!(
                p.flare_avds().unwrap(),
                Path::new("C:/Users/a/AppData/Local/Flare/avd")
            );
            assert_eq!(
                p.discovery_dir().unwrap(),
                Path::new("C:/Users/a/AppData/Local/Temp/avd/running")
            );
        }
        assert_eq!(Places::default().flare_dir(), None);
        assert_eq!(Places::default().discovery_dir(), None);
    }
}
