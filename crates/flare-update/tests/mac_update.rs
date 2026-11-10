//! Port phase 8, step 3 (rules 1046 and 1047): a Velopack update applied for real on a Mac, headless.
//!
//! Version A of `Flare.app` (from its portable zip, as a person unzips it) is installed into a temp folder, never
//! `/Applications`; a child process (this test binary again) runs the real [`VelopackFeed`] over a folder feed that
//! holds B: check, download, apply without a restart. Velopack's `UpdateMac` waits for the child to end, then swaps
//! the whole bundle in one `renamex_np` (Velopack's `apply_osx_impl`), so every seal is B's own. The parent samples
//! the window server the whole time (no window of anything under the temp folder may appear), waits for the swap,
//! and checks the bundle: B's `sq.version` and `Info.plist`, `UpdateMac` in place, `codesign --verify --deep
//! --strict`, nothing started (no restart), and Gatekeeper's verdict: "Notarized Developer ID" for a signed,
//! notarized pack (`FLARE_MAC_EXPECT_NOTARIZED=1`, plus `stapler validate`), the same verdict as A's for an ad hoc
//! one (ad hoc is never accepted; what matters is that the swap did not change what Gatekeeper sees).
//!
//! Run by `tools/release/mac/update-e2e.sh`, which packs the throwaway versions 0.0.1 and 0.0.2:
//! `FLARE_MAC_RELEASES=<Releases with 0.0.1 and 0.0.2> FLARE_MAC_INSTALL_ZIP=<0.0.1's Flare-osx-Portable.zip>
//! cargo test -p flare-update --test mac_update -- --ignored --test-threads=1`.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use flare_update::feed::Feed;
use flare_update::gate::CancelToken;
use flare_update::velopack_feed::VelopackFeed;
use velopack::locator::VelopackLocatorConfig;
use velopack::sources::FileSource;

/// In the child: the installed `Flare.app` to update.
const CHILD_APP: &str = "FLARE_MAC_UPDATE_CHILD_APP";
/// In the child: the packages folder (under the temp folder, never `~/Library/Caches/velopack`).
const CHILD_PACKAGES: &str = "FLARE_MAC_UPDATE_CHILD_PACKAGES";
const RELEASES: &str = "FLARE_MAC_RELEASES";
const INSTALL_ZIP: &str = "FLARE_MAC_INSTALL_ZIP";
const EXPECT_NOTARIZED: &str = "FLARE_MAC_EXPECT_NOTARIZED";
const FROM: &str = "0.0.1";
const TO: &str = "0.0.2";

fn need(var: &str) -> PathBuf {
    PathBuf::from(std::env::var(var).unwrap_or_else(|_| {
        panic!("{var} is not set: run tools/release/mac/update-e2e.sh (it packs 0.0.1 and 0.0.2)")
    }))
}

fn run(cmd: &mut Command) -> (bool, String) {
    let out = cmd.output().expect("start");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// The version in a bundle's `sq.version` (Velopack's manifest, in `Contents/Resources`).
fn sq_version(app: &Path) -> Option<String> {
    let text = std::fs::read_to_string(app.join("Contents/Resources/sq.version")).ok()?;
    let start = text.find("<version>")? + "<version>".len();
    let end = text[start..].find("</version>")? + start;
    Some(text[start..end].to_string())
}

fn plist_version(app: &Path) -> String {
    let (ok, out) = run(Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist")));
    assert!(ok, "{out}");
    out.trim().to_string()
}

fn codesign_verify(app: &Path) {
    let (ok, out) = run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "--verbose=2"])
        .arg(app));
    assert!(
        ok,
        "codesign --verify --deep --strict {}:\n{out}",
        app.display()
    );
}

/// Gatekeeper's verdict on the bundle (`spctl -a -t exec`): accepted or not, and its `source=` line.
fn spctl(app: &Path) -> (bool, String) {
    let (ok, out) = run(Command::new("/usr/sbin/spctl")
        .args(["-a", "-t", "exec", "-vv"])
        .arg(app));
    let source = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("source="))
        .unwrap_or("")
        .to_string();
    (ok, source)
}

/// The pids of every process whose command line names `dir` (the installed app's programs, `UpdateMac` included).
fn pids_under(dir: &Path) -> Vec<i64> {
    let (_, out) = run(Command::new("/usr/bin/pgrep")
        .arg("-f")
        .arg(dir.as_os_str()));
    out.lines().filter_map(|l| l.trim().parse().ok()).collect()
}

/// Every on-screen window's owner pid and name (the window server's list, as `flare_cef::macos::screen` reads it).
fn on_screen() -> Vec<(i64, String)> {
    use objc2::rc::Retained;
    use objc2_core_foundation::CFRetained;
    use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
    let Some(arr) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0) else {
        return Vec::new();
    };
    let ptr = CFRetained::as_ptr(&arr).as_ptr();
    // SAFETY: CFArray and NSArray are toll-free bridged; `arr` stays alive (and retained) for the whole borrow.
    let list: &NSArray = unsafe { &*(ptr.cast::<NSArray>()) };
    let pid_key = NSString::from_str("kCGWindowOwnerPID");
    let name_key = NSString::from_str("kCGWindowOwnerName");
    list.iter()
        .filter_map(|d| {
            let d: Retained<NSDictionary> = d.downcast().ok()?;
            let pid = d
                .objectForKey(&pid_key)
                .and_then(|p| p.downcast::<NSNumber>().ok())?
                .longLongValue();
            let owner = d
                .objectForKey(&name_key)
                .and_then(|p| p.downcast::<NSString>().ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            Some((pid, owner))
        })
        .collect()
}

/// The windows on screen now that belong to `pids` or to an owner named `UpdateMac` (its progress window).
fn shown_by(pids: &[i64]) -> Vec<(i64, String)> {
    on_screen()
        .into_iter()
        .filter(|(pid, owner)| pids.contains(pid) || owner == "UpdateMac")
        .collect()
}

#[test]
#[ignore = "needs packed 0.0.1 and 0.0.2: tools/release/mac/update-e2e.sh"]
fn a_packed_flare_app_updates_itself_headless_and_the_swapped_bundle_still_verifies() {
    let releases = need(RELEASES);
    let zip = need(INSTALL_ZIP);
    let notarized = std::env::var(EXPECT_NOTARIZED).is_ok_and(|v| v == "1");
    let tmp = tempfile::tempdir().expect("temp");
    let apps = tmp.path().join("Applications");
    std::fs::create_dir_all(&apps).unwrap();
    let app = apps.join("Flare.app");
    let packages = tmp.path().join("packages");

    // Install A as a person does from the portable zip (ditto keeps the links and the signatures).
    let (ok, out) = run(Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(&zip)
        .arg(&apps));
    assert!(ok, "ditto -x: {out}");
    assert_eq!(sq_version(&app).as_deref(), Some(FROM));
    assert_eq!(plist_version(&app), FROM);
    assert!(app.join("Contents/MacOS/UpdateMac").is_file());
    codesign_verify(&app);
    let before = spctl(&app);
    if notarized {
        assert!(
            before.0 && before.1 == "Notarized Developer ID",
            "A: {before:?}"
        );
    }
    assert!(shown_by(&pids_under(tmp.path())).is_empty());

    // The child runs the feed and hands off to UpdateMac; its output (and UpdateMac's, which inherits it) is a file.
    let log = tmp.path().join("child.log");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "the_child_checks_downloads_and_applies",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD_APP, &app)
        .env(CHILD_PACKAGES, &packages)
        .env(RELEASES, &releases)
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(Stdio::from(
            std::fs::File::options().append(true).open(&log).unwrap(),
        ))
        .spawn()
        .expect("child");

    // Sample the window server until the swap is done and nothing under the temp folder runs any more.
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut child_ok = None;
    let mut samples = 0u32;
    loop {
        let pids = pids_under(tmp.path());
        let shown = shown_by(&pids);
        assert!(shown.is_empty(), "a window appeared: {shown:?}");
        samples += 1;
        if child_ok.is_none()
            && let Some(st) = child.try_wait().unwrap()
        {
            child_ok = Some(st.success());
        }
        if child_ok.is_some() && pids.is_empty() && sq_version(&app).as_deref() == Some(TO) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no swap in 3 minutes (child {child_ok:?}, pids {pids:?}):\n{}",
            std::fs::read_to_string(&log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let child_log = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(child_ok, Some(true), "{child_log}");
    assert!(child_log.contains(&format!("applied {TO}")), "{child_log}");

    // B is in place, whole and sealed; nothing was started.
    assert_eq!(sq_version(&app).as_deref(), Some(TO));
    assert_eq!(plist_version(&app), TO);
    assert!(app.join("Contents/MacOS/UpdateMac").is_file());
    assert_eq!(
        std::fs::read_link(app.join("Contents/MacOS/sq.version")).ok(),
        Some(PathBuf::from("../Resources/sq.version"))
    );
    codesign_verify(&app);
    let after = spctl(&app);
    assert_eq!(
        after, before,
        "Gatekeeper sees the swapped bundle as it saw A"
    );
    if notarized {
        let (ok, out) = run(Command::new("/usr/bin/xcrun")
            .args(["stapler", "validate"])
            .arg(&app));
        assert!(ok, "stapler validate: {out}");
    }
    let left: Vec<String> = std::fs::read_dir(&apps)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        left,
        ["Flare.app"],
        "the old bundle is gone, nothing beside it"
    );
    assert!(
        pids_under(tmp.path()).is_empty(),
        "no restart: nothing of the app runs"
    );
    assert!(
        samples > 1,
        "the window server was sampled during the update"
    );
    eprintln!(
        "updated {FROM} -> {TO} in {} (Gatekeeper: {after:?}; {samples} window samples, none shown)",
        app.display()
    );
}

/// The child: the real feed over the folder, as the app's update engine drives it (check, download, apply), with
/// the packages folder under the temp folder and no restart. Does nothing outside the parent's run.
#[test]
#[ignore = "the child of the Mac update test"]
fn the_child_checks_downloads_and_applies() {
    let Ok(app) = std::env::var(CHILD_APP) else {
        return;
    };
    let app = PathBuf::from(app);
    let packages = need(CHILD_PACKAGES);
    std::fs::create_dir_all(&packages).unwrap();
    let macos = app.join("Contents/MacOS");
    let cfg = VelopackLocatorConfig {
        RootAppDir: app.clone(),
        UpdateExePath: macos.join("UpdateMac"),
        PackagesDir: packages.clone(),
        // The link vpk puts in Contents/MacOS, as Velopack's own locator reads it first.
        ManifestPath: macos.join("sq.version"),
        CurrentBinaryDir: macos,
        IsPortable: true,
    };
    let feed = VelopackFeed::with_source(Box::new(FileSource::new(need(RELEASES))), Some(cfg))
        .expect("feed")
        .without_restart();
    let found = feed.check().expect("check").expect("an offer");
    assert_eq!(found.version, TO);
    let mut seen = Vec::new();
    feed.download(TO, &mut |p| seen.push(p), &CancelToken::default())
        .expect("download");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert!(
        packages
            .join(format!("Flare-{TO}-osx-full.nupkg"))
            .is_file()
    );
    feed.apply(TO, &[]).expect("apply");
    println!("applied {TO}");
}
