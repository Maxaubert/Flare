//! adb for what gRPC cannot do: install an `.apk` and launch an app by its package name (spec "UI tree and agent
//! helper"). The path checks (an `.apk` file inside the tab's folder, symlinks resolved, a zip with a manifest) are on
//! Flare's side (`ctl_device::install_ok`, Android plan PR A5);
//! here a package name must be one Android takes, so nothing reaches the device's shell but a checked name.

use std::path::Path;
use std::time::Duration;

use flare_device::job::Fail;

use crate::run::{command, run};

type Failed = (Fail, String);

/// How long an install may take (a big `.apk` on a cold device), and a launch.
const INSTALL_LIMIT: Duration = Duration::from_secs(180);
const LAUNCH_LIMIT: Duration = Duration::from_secs(30);

/// Whether `s` is an Android package name (the check Flare's side makes too). Nothing else reaches `adb shell`.
pub use flare_device::emu::package_ok;

/// The launcher activity in `cmd package resolve-activity --brief` output: its last line, `pkg/.Activity`, None when
/// the package has none.
pub fn component_of(out: &str, package: &str) -> Option<String> {
    let last = out.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let (pkg, activity) = last.split_once('/')?;
    (pkg == package
        && !activity.is_empty()
        && activity
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '$'))
    .then(|| last.to_string())
}

/// BLOCKING: launches `package` on `serial` (its launcher activity, `am start -W`). Arguments are not passed on
/// Android in v1.
pub fn launch(adb: &Path, serial: &str, package: &str, args: &[String]) -> Result<(), Failed> {
    if !package_ok(package) {
        return Err((
            Fail::Unavailable,
            format!("{package:?} is not an Android package name"),
        ));
    }
    if !args.is_empty() {
        return Err((
            Fail::Unavailable,
            "launch arguments are not taken on Android".to_string(),
        ));
    }
    let mut c = command(adb);
    c.args([
        "-s",
        serial,
        "shell",
        "cmd",
        "package",
        "resolve-activity",
        "--brief",
        package,
    ]);
    let r = run(c, LAUNCH_LIMIT).map_err(|e| (Fail::Unavailable, e))?;
    let component = component_of(&r.stdout, package).ok_or_else(|| {
        (
            Fail::NotFound,
            format!("no app {package} to open on this device"),
        )
    })?;
    let mut c = command(adb);
    c.args(["-s", serial, "shell", "am", "start", "-W", "-n", &component]);
    let r = run(c, LAUNCH_LIMIT).map_err(|e| (Fail::Unavailable, e))?;
    if !r.ok() || r.stdout.contains("Error") {
        return Err((
            Fail::Unavailable,
            format!(
                "{package} did not open: {}",
                first_line(&r.stdout, &r.stderr)
            ),
        ));
    }
    Ok(())
}

/// BLOCKING: installs an `.apk` on `serial` (`adb install -r -t`).
pub fn install(adb: &Path, serial: &str, apk: &Path) -> Result<(), Failed> {
    if apk
        .extension()
        .is_none_or(|e| !e.eq_ignore_ascii_case("apk"))
    {
        return Err((
            Fail::Unavailable,
            "an Android app is an .apk file".to_string(),
        ));
    }
    let mut c = command(adb);
    c.args(["-s", serial, "install", "-r", "-t"]).arg(apk);
    let r = run(c, INSTALL_LIMIT).map_err(|e| (Fail::Unavailable, e))?;
    if r.ok() && r.stdout.contains("Success") {
        Ok(())
    } else {
        Err((
            Fail::Unavailable,
            format!("the install failed: {}", first_line(&r.stdout, &r.stderr)),
        ))
    }
}

/// How long a new device's first-boot compile may take (a minute or two in Step 0 S7).
const PREPARE_LIMIT: Duration = Duration::from_secs(600);

/// BLOCKING, up to 10 minutes: `pm bg-dexopt-job` on `serial` (Android plan PR A2b), so a new device compiles its apps
/// now, not in the background while the person uses it.
pub fn prepare(adb: &Path, serial: &str) -> Result<(), String> {
    let mut c = command(adb);
    c.args(["-s", serial, "shell", "pm", "bg-dexopt-job"]);
    let r = run(c, PREPARE_LIMIT)?;
    if r.ok() && !r.stdout.contains("Failure") {
        Ok(())
    } else {
        Err(first_line(&r.stdout, &r.stderr))
    }
}

fn first_line(out: &str, err: &str) -> String {
    let pick = |s: &str| {
        s.lines()
            .map(str::trim)
            .find(|l| l.contains("Error") || l.contains("Failure"))
            .map(str::to_string)
    };
    pick(out)
        .or_else(|| pick(err))
        .or_else(|| {
            err.lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "no reason given".to_string())
        .chars()
        .take(200)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_package_name_reaches_the_devices_shell() {
        for ok in [
            "com.android.settings",
            "dev.flare.probe",
            "a.b",
            "com.x_y.Z9",
        ] {
            assert!(package_ok(ok), "{ok}");
        }
        for bad in [
            "settings",
            "com..x",
            ".com.x",
            "com.x;rm -rf /",
            "com.x y",
            "com.1x",
            "",
            "com.x'",
            "com.x/.A",
        ] {
            assert!(!package_ok(bad), "{bad}");
        }
        assert!(!package_ok(&format!("a.{}", "b".repeat(300))));
    }

    #[test]
    fn the_launcher_activity_is_the_last_line() {
        let out = "priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=false\n\
                   com.android.settings/.Settings\n";
        assert_eq!(
            component_of(out, "com.android.settings").as_deref(),
            Some("com.android.settings/.Settings")
        );
        assert_eq!(component_of("No activity found\n", "a.b"), None);
        assert_eq!(
            component_of("other.app/.Main\n", "a.b"),
            None,
            "another package"
        );
        assert_eq!(component_of("a.b/.Main; reboot\n", "a.b"), None);
        assert_eq!(
            first_line(
                "Performing Streamed Install\nFailure [INSTALL_FAILED_OLDER_SDK]\n",
                ""
            ),
            "Failure [INSTALL_FAILED_OLDER_SDK]"
        );
        assert_eq!(first_line("", ""), "no reason given");
    }

    #[test]
    fn an_install_needs_an_apk_and_a_launch_a_package() {
        let adb = Path::new("/no/adb");
        assert_eq!(
            install(adb, "emulator-5554", Path::new("/x/app.zip"))
                .unwrap_err()
                .0,
            Fail::Unavailable
        );
        assert_eq!(
            launch(adb, "emulator-5554", "not a package", &[])
                .unwrap_err()
                .0,
            Fail::Unavailable
        );
        assert_eq!(
            launch(adb, "emulator-5554", "a.b", &["-x".to_string()])
                .unwrap_err()
                .0,
            Fail::Unavailable
        );
    }
}
