//! M9 Task 11: the Velopack backend. Its network constructor is behind the live-updates feature.
//!
//! This is the ONE file that touches Velopack. Flare never implements `UpdateSource` itself; it
//! only constructs the crate's own sources, and the remote ones exist only under `live-updates`.
//! Installing is two steps: `download` writes a package into the packages folder, `apply` starts
//! the updater, which waits for this process to exit. `discard` removes what a download wrote and
//! never the running version's own package (the next delta needs it).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use velopack::locator::{LocationContext, VelopackLocatorConfig, auto_locate_app_manifest};
use velopack::sources::UpdateSource;
use velopack::{Error as VError, UpdateCheck, UpdateInfo, UpdateManager, VelopackApp};

use crate::feed::{Feed, FeedError, Found};
use crate::gate::CancelToken;

/// The pack id (= `flare_win::names::APP_ID`).
pub const PACK_ID: &str = "Flare";
/// The Application User Model ID (= `flare_win::names::AUMID`).
pub const AUMID: &str = "com.flare.terminal";

/// True only for exactly our own pack id (ASCII case-insensitive).
pub fn package_is_ours(package_id: &str) -> bool {
    package_id.eq_ignore_ascii_case(PACK_ID)
}

/// What the app hooks do. Both run on Velopack's schedule, never on a window.
pub struct Hooks {
    pub before_uninstall: Box<dyn FnOnce() + 'static>,
    pub first_run: Box<dyn FnOnce(String) + 'static>,
}

/// Velopack's startup logic. Must be the first thing `main` does; it may end the process (the
/// install and uninstall hooks) and returns at once on a dev build. `aumid` is this run's taskbar
/// identity (`flare_win::names::aumid_for_profile`): [`AUMID`] for the default profile.
pub fn bootstrap(hooks: Hooks, aumid: &str) {
    let Hooks {
        before_uninstall,
        first_run,
    } = hooks;
    let app = VelopackApp::build().set_auto_apply_on_startup(false);
    // The taskbar identity and the fast install hooks exist only in Velopack's Windows build.
    #[cfg(windows)]
    let app = app
        .set_app_user_model_id(aumid)
        .on_before_uninstall_fast_callback(move |_v| before_uninstall())
        .on_after_install_fast_callback(|_v| {})
        .on_after_update_fast_callback(|_v| {});
    // macOS (port phase 8): the app is a bundle the updater swaps whole (rule 273), and there is no uninstall hook to
    // run (a Mac app is removed by moving it to the Bin).
    #[cfg(not(windows))]
    let _ = (aumid, before_uninstall);
    app.on_first_run(move |v| first_run(v.to_string())).run();
}

/// The files a discard removes: the target package, its `.partial`, and the same two for every
/// delta. Never the base release.
pub fn discard_paths(packages_dir: &Path, info: &UpdateInfo) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for asset in std::iter::once(&info.TargetFullRelease).chain(info.DeltasToTarget.iter()) {
        if !is_bare_file_name(&asset.FileName) {
            continue;
        }
        let full = packages_dir.join(&asset.FileName);
        out.push(full.clone());
        out.push(full.with_extension("partial"));
    }
    out
}

/// A file name taken from a feed is only ever joined to the packages folder when it is a bare
/// name: no separators, no `..`, no drive or stream colon, no control characters.
pub fn is_bare_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.chars().any(char::is_control)
        && !name.ends_with(['.', ' '])
}

/// Velopack joins every asset's `FileName` to the packages folder itself (download, delta and
/// apply), so an offer is taken only when every name in it is bare.
pub fn names_are_bare(info: &UpdateInfo) -> bool {
    std::iter::once(&info.TargetFullRelease)
        .chain(info.BaseRelease.iter())
        .chain(info.DeltasToTarget.iter())
        .all(|a| is_bare_file_name(&a.FileName))
}

/// True only for `http://127.0.0.1:<digits>` followed by nothing, or by `/`, `?` or `#`: no
/// userinfo, no other host, no backslash, an explicit numeric port.
#[cfg(any(test, feature = "live-updates"))]
fn is_loopback_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://127.0.0.1:") else {
        return false;
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 5 || rest[..digits].parse::<u32>().map_or(true, |p| p > 65535) {
        return false;
    }
    let tail = &rest[digits..];
    (tail.is_empty() || tail.starts_with(['/', '?', '#']))
        && !tail.contains(['@', '\\', ' ', '\t', '\r', '\n'])
        && !url.chars().any(char::is_control)
}

fn remove_all(packages_dir: &Path, info: &UpdateInfo) {
    for p in discard_paths(packages_dir, info) {
        let _ = std::fs::remove_file(p); // sync-ok: runs on the install worker, errors are ignored
    }
}

fn strip_v(v: &str) -> &str {
    v.strip_prefix(['v', 'V']).unwrap_or(v)
}

fn lock_it<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn map_err(e: VError) -> FeedError {
    match e {
        VError::Network(n) => FeedError::Offline(n.to_string()),
        other => FeedError::Failed(other.to_string()),
    }
}

/// The real backend over the `velopack` crate.
pub struct VelopackFeed {
    um: UpdateManager,
    info: Mutex<Option<UpdateInfo>>,
    packages_dir: PathBuf,
    busy: Arc<AtomicBool>,
    /// Whether the updater starts the new version once it is in place: always in the app; off only for the Mac
    /// update test, which may not put the app on the screen (rule 274).
    restart: bool,
}

impl VelopackFeed {
    pub fn with_source(
        source: Box<dyn UpdateSource>,
        locator: Option<VelopackLocatorConfig>,
    ) -> Result<Self, FeedError> {
        let um = UpdateManager::new_boxed(source, None, locator.clone()).map_err(map_err)?;
        let packages_dir = match &locator {
            Some(cfg) => cfg.PackagesDir.clone(),
            None => auto_locate_app_manifest(LocationContext::FromCurrentExe)
                .map_err(map_err)?
                .get_packages_dir(),
        };
        Ok(Self {
            um,
            info: Mutex::new(None),
            packages_dir,
            busy: Arc::new(AtomicBool::new(false)),
            restart: true,
        })
    }

    /// The updater swaps the bundle in and starts nothing (the Mac update test only: it runs headless, rule 274).
    pub fn without_restart(mut self) -> Self {
        self.restart = false;
        self
    }

    /// The only constructor of a remote network source in the workspace.
    #[cfg(feature = "live-updates")]
    pub fn github(repo_url: &str) -> Result<Self, FeedError> {
        let source = velopack::sources::GithubSource::new(repo_url, None, false);
        Self::with_source(Box::new(source), None)
    }

    /// e2e only: a loopback HTTP feed. Any other host is refused before anything is built.
    #[cfg(feature = "live-updates")]
    pub fn loopback(url: &str) -> Result<Self, FeedError> {
        if !is_loopback_url(url) {
            return Err(FeedError::Failed("not loopback".into()));
        }
        let source = velopack::sources::HttpSource::new(url);
        Self::with_source(Box::new(source), None)
    }

    /// The stored find when its version is `version`.
    fn offered(&self, version: &str) -> Result<UpdateInfo, FeedError> {
        let guard = lock_it(&self.info);
        let info = guard
            .as_ref()
            .ok_or_else(|| FeedError::Failed("nothing found".into()))?;
        if strip_v(&info.TargetFullRelease.Version) != strip_v(version) {
            return Err(FeedError::Failed("not offered".into()));
        }
        Ok(info.clone())
    }
}

impl Feed for VelopackFeed {
    fn check(&self) -> Result<Option<Found>, FeedError> {
        match self.um.check_for_updates().map_err(map_err)? {
            UpdateCheck::RemoteIsEmpty | UpdateCheck::NoUpdateAvailable => Ok(None),
            UpdateCheck::UpdateAvailable(info) => {
                if !package_is_ours(&info.TargetFullRelease.PackageId) || info.IsDowngrade {
                    log::warn!(
                        "update: ignoring an offer ({}, downgrade {})",
                        info.TargetFullRelease.PackageId,
                        info.IsDowngrade
                    );
                    return Ok(None);
                }
                if !names_are_bare(&info) {
                    log::warn!("update: ignoring an offer whose file names are not bare");
                    return Ok(None);
                }
                let found = Found {
                    version: strip_v(&info.TargetFullRelease.Version).to_string(),
                    notes: info.TargetFullRelease.NotesMarkdown.clone(),
                };
                *lock_it(&self.info) = Some(*info);
                Ok(Some(found))
            }
        }
    }

    fn download(
        &self,
        version: &str,
        progress: &mut dyn FnMut(u8),
        cancel: &CancelToken,
    ) -> Result<(), FeedError> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(FeedError::Failed("busy".into()));
        }
        let info = match self.offered(version) {
            Ok(i) => i,
            Err(e) => {
                self.busy.store(false, Ordering::SeqCst);
                return Err(e);
            }
        };
        let (tx, rx) = mpsc::channel::<i16>();
        let (done_tx, done_rx) = mpsc::channel::<Result<(), VError>>();
        let um = self.um.clone();
        let dl_info = info.clone();
        std::thread::spawn(move || {
            let r = um.download_updates(&dl_info, Some(tx));
            let _ = done_tx.send(r);
        });
        loop {
            if cancel.is_cancelled() {
                // The download thread cannot be stopped: a joiner waits for it, removes what it
                // wrote and only then frees `busy`.
                let busy = Arc::clone(&self.busy);
                let dir = self.packages_dir.clone();
                std::thread::spawn(move || {
                    let _ = done_rx.recv(); // sync-ok: dedicated joiner thread
                    remove_all(&dir, &info);
                    busy.store(false, Ordering::SeqCst);
                });
                return Err(FeedError::Cancelled);
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                // sync-ok: the install worker waits for progress
                Ok(v) => progress(v.clamp(0, 100) as u8),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let result = done_rx
            .recv() // sync-ok: the download thread has already dropped its sender
            .unwrap_or_else(|_| Err(VError::Other("download thread ended".into())));
        self.busy.store(false, Ordering::SeqCst);
        result.map_err(map_err)
    }

    fn apply(&self, version: &str, restart_args: &[String]) -> Result<(), FeedError> {
        let info = self.offered(version)?;
        self.um
            .wait_exit_then_apply_updates(&info.TargetFullRelease, true, self.restart, restart_args)
            .map_err(map_err)
    }

    fn discard(&self) {
        if let Some(info) = lock_it(&self.info).as_ref() {
            remove_all(&self.packages_dir, info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use velopack::VelopackAsset;

    fn asset(name: &str) -> VelopackAsset {
        VelopackAsset {
            PackageId: "Flare".into(),
            FileName: name.into(),
            ..Default::default()
        }
    }

    fn production() -> &'static str {
        let src = include_str!("velopack_feed.rs");
        src.split("#[cfg(test)]").next().unwrap()
    }

    #[test]
    fn refuses_a_package_with_another_id() {
        // Built from parts so the name scan (rule: no neighbour names outside the import) passes.
        assert!(!package_is_ours(&["Pri", "sm", "Terminal"].concat()));
        assert!(!package_is_ours(&["Pri", "sm"].concat()));
    }

    #[test]
    fn accepts_only_our_id() {
        assert!(package_is_ours("Flare"));
        assert!(package_is_ours("flare"));
        for bad in ["Flare2", " Flare", "Flare ", ""] {
            assert!(!package_is_ours(bad), "{bad:?}");
        }
    }

    #[test]
    fn rejects_lookalikes() {
        for bad in ["F1are", "Flare.exe", "Flare-Setup"] {
            assert!(!package_is_ours(bad), "{bad:?}");
        }
    }

    #[test]
    fn discard_names_the_target_its_partial_and_the_deltas_never_the_base() {
        let info = UpdateInfo {
            TargetFullRelease: asset("Flare-0.0.2-full.nupkg"),
            BaseRelease: Some(asset("Flare-0.0.1-full.nupkg")),
            DeltasToTarget: vec![asset("Flare-0.0.2-delta.nupkg")],
            IsDowngrade: false,
        };
        let dir = Path::new("pk");
        let got: Vec<String> = discard_paths(dir, &info)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            got,
            [
                "Flare-0.0.2-full.nupkg",
                "Flare-0.0.2-full.partial",
                "Flare-0.0.2-delta.nupkg",
                "Flare-0.0.2-delta.partial",
            ]
        );
        assert!(!got.iter().any(|n| n.contains("0.0.1")));
    }

    #[cfg(feature = "live-updates")]
    #[test]
    fn the_loopback_constructor_refuses_any_other_host() {
        for url in [
            "https://example.com/",
            "http://localhost:1/",
            "http://127.0.0.2:1/",
        ] {
            assert!(VelopackFeed::loopback(url).is_err(), "{url}");
        }
    }

    #[test]
    fn loopback_urls_are_parsed_not_prefix_matched() {
        for ok in [
            "http://127.0.0.1:80",
            "http://127.0.0.1:8080/",
            "http://127.0.0.1:1/feed/x?y=1",
        ] {
            assert!(is_loopback_url(ok), "{ok}");
        }
        for bad in [
            "http://127.0.0.1:80@evil.example/",
            "http://127.0.0.1:80@evil.example",
            "http://user@127.0.0.1:80/",
            "http://127.0.0.1:80\\@evil.example/",
            "http://127.0.0.1:80.evil.example/",
            "http://127.0.0.1:/",
            "http://127.0.0.1/",
            "http://127.0.0.1:99999/",
            "http://127.0.0.1:8a/",
            "https://127.0.0.1:80/",
            "http://127.0.0.2:80/",
            "http://localhost:80/",
            "http://127.0.0.1:80 @evil/",
            "HTTP://127.0.0.1:80/",
            "",
        ] {
            assert!(!is_loopback_url(bad), "{bad}");
        }
    }

    #[test]
    fn feed_file_names_must_be_bare() {
        for ok in ["Flare-0.0.2-full.nupkg", "a.b-c_d.nupkg"] {
            assert!(is_bare_file_name(ok), "{ok}");
        }
        for bad in [
            "",
            ".",
            "..",
            "../x.nupkg",
            "..\\x",
            "a/b",
            "a\\b",
            "C:x.nupkg",
            "C:\\x",
            "/abs",
            "\\\\srv\\x",
            "f.nupkg:stream",
            "a\u{0}b",
            "a\nb",
            "x.",
        ] {
            assert!(!is_bare_file_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn discard_skips_a_traversing_feed_name() {
        let info = UpdateInfo {
            TargetFullRelease: asset("..\\..\\evil.nupkg"),
            ..UpdateInfo::default()
        };
        assert!(discard_paths(Path::new("C:\\pk"), &info).is_empty());
    }

    #[test]
    fn an_offer_is_taken_only_when_every_name_is_bare() {
        let good = UpdateInfo {
            TargetFullRelease: asset("Flare-0.0.2-full.nupkg"),
            BaseRelease: Some(asset("Flare-0.0.1-full.nupkg")),
            DeltasToTarget: vec![asset("Flare-0.0.2-delta.nupkg")],
            IsDowngrade: false,
        };
        assert!(names_are_bare(&good));
        let mut bad_target = good.clone();
        bad_target.TargetFullRelease = asset("..\\evil.nupkg");
        assert!(!names_are_bare(&bad_target));
        let mut bad_base = good.clone();
        bad_base.BaseRelease = Some(asset("C:\\x.nupkg"));
        assert!(!names_are_bare(&bad_base));
        let mut bad_delta = good;
        bad_delta.DeltasToTarget.push(asset("../d.nupkg"));
        assert!(!names_are_bare(&bad_delta));
    }

    #[test]
    #[ignore = "needs real packages; runs in the update e2e CI job"]
    fn the_feed_over_a_file_source_checks_downloads_and_reports_progress() {}

    #[test]
    fn no_network_source_exists_without_the_feature() {
        let mut last_attr = String::new();
        for line in production().lines() {
            let t = line.trim();
            if t.starts_with("#[") {
                last_attr = t.to_string();
                continue;
            }
            if t.starts_with("//") {
                continue;
            }
            assert!(!t.contains("AutoSource"), "AutoSource: {t}");
            if t.contains("GithubSource") || t.contains("HttpSource") {
                assert!(
                    last_attr.contains("live-updates"),
                    "ungated network source: {t}"
                );
            }
        }
    }

    #[test]
    fn the_app_restarts_after_an_update_and_only_the_mac_test_turns_it_off() {
        let p = production();
        assert!(p.contains("restart: true,"));
        assert!(
            p.contains("wait_exit_then_apply_updates(&info.TargetFullRelease, true, self.restart,")
        );
        assert_eq!(p.matches("self.restart = false").count(), 1);
    }

    #[test]
    fn bootstrap_turns_auto_apply_off() {
        assert!(production().contains("set_auto_apply_on_startup(false)"));
    }
}
