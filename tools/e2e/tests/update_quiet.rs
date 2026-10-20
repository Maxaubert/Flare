//! M9 Task 39: the update stays quiet without a flag, a second launch previews in the running app,
//! the command file drives the window, and a real-shaped offer is refused before any request.
//!
//! Runs everywhere: nothing is installed, nothing is downloaded and GitHub is never asked
//! (`calls` stays at zero checks for the preview, and one install with no check for the refusal).
//! Every app runs `--e2e` in its own sandbox profile and is reaped when the test ends.
//!
//! The exe is `FLARE_EXE` when set, else the `flare.exe` in the target folder that holds this test
//! binary (`<target>\debug\deps\update_quiet.exe` -> `<target>\debug\flare.exe`). Build it first:
//! `cargo build -p flare --target-dir <TARGET>`.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flare_e2e::proc::Child;
use flare_e2e::sandbox::Sandbox;
use flare_update::flow::{INSTALL_FAILED, PREVIEW_DONE};
use flare_update::preview::preview_version;
use serde_json::Value;

const WAIT: Duration = Duration::from_secs(20);

/// The flare.exe to run: `FLARE_EXE`, else the one in this test binary's target folder.
fn flare_exe() -> PathBuf {
    if let Some(p) = std::env::var_os("FLARE_EXE") {
        return PathBuf::from(p);
    }
    let me = std::env::current_exe().expect("the test binary's path");
    let exe = me
        .parent()
        .and_then(Path::parent)
        .map(|debug| debug.join("flare.exe"))
        .expect("a target folder above deps");
    assert!(
        exe.is_file(),
        "{} is missing: run `cargo build -p flare` first or set FLARE_EXE",
        exe.display()
    );
    exe
}

/// Launches flare with the sandbox's environment plus `extra`, on the sandbox profile.
fn launch(sb: &Sandbox, extra: &[(&str, &str)], args: &[&str]) -> Child {
    let mut env = sb.env();
    for (k, v) in extra {
        env.push(((*k).to_string(), (*v).to_string()));
    }
    Child::launch(&flare_exe(), args, &env, &sb.profile)
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The state file written after every change (`chip`, `version`, `mock`, `open`, `phase`, `pct`,
/// `notice`).
fn state(profile: &Path) -> Option<Value> {
    read_json(&profile.join("update-state.json"))
}

/// `(checks, installs)` from the calls file.
fn calls(profile: &Path) -> Option<(u64, u64)> {
    let v = read_json(&profile.join("update-calls.json"))?;
    Some((v["checks"].as_u64()?, v["installs"].as_u64()?))
}

/// Writes one command word for the app to read and delete.
fn send(profile: &Path, word: &str) {
    let tmp = profile.join("update-command.tmp");
    std::fs::write(&tmp, word).expect("the command file");
    std::fs::rename(&tmp, profile.join("update-command")).expect("the command file");
}

fn chip_on(profile: &Path) -> bool {
    state(profile).is_some_and(|s| s["chip"] == true)
}

#[test]
fn no_flag_means_no_chip_zero_checks_and_zero_installs() {
    let sb = Sandbox::new();
    let app = launch(&sb, &[], &[]);
    assert!(
        app.wait_for(
            || state(&sb.profile).is_some() && calls(&sb.profile).is_some(),
            WAIT
        ),
        "update-state.json never appeared"
    );
    let s = state(&sb.profile).expect("state");
    assert_eq!(s["chip"], false, "{s}");
    assert_eq!(calls(&sb.profile), Some((0, 0)));
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        calls(&sb.profile),
        Some((0, 0)),
        "a check or an install ran"
    );
    assert_eq!(state(&sb.profile).expect("state")["chip"], false);
    assert!(app.exited().is_none(), "the app quit by itself");
}

#[test]
fn a_second_launch_with_the_flag_previews_in_the_running_app_and_exits_0() {
    let sb = Sandbox::new();
    let first = launch(&sb, &[], &[]);
    assert!(
        first.wait_for(
            || state(&sb.profile).is_some() && calls(&sb.profile).is_some(),
            WAIT
        ),
        "the first launch never wrote its state"
    );
    let tabs_before = std::fs::read(sb.profile.join("tabs.json")).ok();

    let second = launch(&sb, &[], &["--preview-update"]);
    assert!(
        second.wait_for(|| second.exited().is_some(), Duration::from_secs(5)),
        "the second launch was still running after 5 s"
    );
    assert_eq!(second.exited(), Some(0), "the second launch must exit 0");

    assert!(
        first.wait_for(
            || state(&sb.profile).is_some_and(|s| s["chip"] == true && s["mock"] == true),
            WAIT
        ),
        "the running app never showed the preview chip"
    );
    assert_eq!(
        std::fs::read(sb.profile.join("tabs.json")).ok(),
        tabs_before,
        "tabs.json changed: the preview opened a tab"
    );
    assert_eq!(calls(&sb.profile), Some((0, 0)), "the preview asked GitHub");
}

#[test]
fn the_preview_flag_in_the_first_launch_shows_the_chip() {
    let sb = Sandbox::new();
    let app = launch(&sb, &[], &["--preview-update"]);
    assert!(
        app.wait_for(|| chip_on(&sb.profile), WAIT),
        "the preview chip never appeared"
    );
    let s = state(&sb.profile).expect("state");
    assert_eq!(s["mock"], true, "{s}");
    assert_eq!(
        s["version"],
        preview_version(env!("CARGO_PKG_VERSION")),
        "{s}"
    );
    assert_eq!(calls(&sb.profile), Some((0, 0)));
}

#[test]
fn the_command_file_opens_the_window_and_the_preview_runs_to_the_end() {
    let sb = Sandbox::new();
    let app = launch(&sb, &[], &["--preview-update"]);
    assert!(app.wait_for(|| chip_on(&sb.profile), WAIT));

    send(&sb.profile, "open");
    assert!(
        app.wait_for(
            || state(&sb.profile).is_some_and(|s| s["open"] == true),
            Duration::from_secs(8)
        ),
        "the open command never opened the window"
    );

    send(&sb.profile, "install");
    let saw_downloading = Cell::new(false);
    let finished = app.wait_for(
        || {
            let Some(s) = state(&sb.profile) else {
                return false;
            };
            if s["phase"] == "downloading" {
                saw_downloading.set(true);
            }
            s["phase"] == "idle" && s["notice"] == PREVIEW_DONE
        },
        Duration::from_secs(8),
    );
    assert!(finished, "the preview did not end with the notice");
    assert!(saw_downloading.get(), "the preview never showed a download");
    assert_eq!(
        calls(&sb.profile),
        Some((0, 0)),
        "the preview installed or asked"
    );
}

#[test]
fn a_real_shaped_offer_is_refused_before_any_request() {
    let sb = Sandbox::new();
    let app = launch(&sb, &[("FLARE_E2E_UPDATE_OFFER", "1")], &[]);
    assert!(
        app.wait_for(|| chip_on(&sb.profile), WAIT),
        "the offered update's chip never appeared"
    );
    let s = state(&sb.profile).expect("state");
    assert_eq!(s["mock"], false, "{s}");
    assert_eq!(s["version"], "99.0.0", "{s}");
    assert_eq!(
        calls(&sb.profile),
        Some((0, 0)),
        "a check ran before the install"
    );

    send(&sb.profile, "install");
    assert!(
        app.wait_for(
            || state(&sb.profile)
                .is_some_and(|s| s["phase"] == "idle" && s["notice"] == INSTALL_FAILED),
            WAIT
        ),
        "the refused install did not end with the notice"
    );
    assert_eq!(
        calls(&sb.profile),
        Some((0, 1)),
        "want checks 0 and installs 1"
    );
}
