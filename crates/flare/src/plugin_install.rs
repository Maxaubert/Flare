//! The workers of Settings > Plugins' Install, Update and Remove (plugins phase 7). The rules are in `flare_mods`
//! (`install`, `fetch`, the hub's lifecycle); here each step runs on a thread of its own and answers the window with a
//! `ModsMsg`, so the UI thread never touches the disk or the network.
//!
//! An address is downloaded under phase 6's wall (`flare_surfaces::net`): `https` only, a public host on every hop,
//! and no resolved address on this PC or its network.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use flare_mods::Hub;
use flare_mods::fetch::{self, Rules};
use flare_mods::helper::Host;

use crate::appctl::{Post, UiMsg};
use crate::mods_ctl::{ModsMsg, ModsMsgBox};

/// The download rules: phase 6's address and resolver walls, the zip's size limit and the minute.
pub fn rules() -> Rules {
    Rules {
        address: Arc::new(|u: &str| flare_surfaces::net::refusal(u).map(str::to_string)),
        ip: Arc::new(|ip| flare_surfaces::net::ip_refusal(ip).map(str::to_string)),
        max_bytes: flare_mods::install::MAX_ZIP_BYTES,
        timeout: fetch::TIMEOUT,
    }
}

fn post_mods(post: &Post, m: ModsMsg) {
    post(UiMsg::Mods(ModsMsgBox(m)));
}

fn spawn(name: &str, job: impl FnOnce() + Send + 'static) -> bool {
    match std::thread::Builder::new().name(name.into()).spawn(job) {
        Ok(_) => true,
        Err(e) => {
            log::warn!("{name} did not start: {e}");
            false
        }
    }
}

/// The sentence for a zip that holds another plugin than the one being updated.
fn not_the_one(found: &str, expect: &str) -> String {
    format!("That zip holds the plugin \"{found}\", not \"{expect}\".")
}

/// Unpacks and checks `zip` on a worker; `expect` is the id an update must hold. Answers `ModsMsg::Staged`.
pub fn stage_file(hub: Arc<Hub>, zip: PathBuf, expect: Option<String>, post: Post) -> bool {
    let answer = Arc::clone(&post);
    spawn("flare-plugin-stage", move || {
        let result = hub
            .stage_install(&zip, None)
            .and_then(|s| check_expected(s, expect.as_deref()));
        post_mods(&answer, ModsMsg::Staged(result.map(Box::new)));
    })
}

fn check_expected(
    s: flare_mods::hub::StagedInstall,
    expect: Option<&str>,
) -> Result<flare_mods::hub::StagedInstall, String> {
    match expect {
        Some(want) if s.id() != want => Err(not_the_one(s.id(), want)),
        _ => Ok(s),
    }
}

/// Downloads `url` into the staging folder, then stages it as a file would be, on a worker. Answers
/// `ModsMsg::Staged`.
pub fn stage_url(hub: Arc<Hub>, url: String, expect: Option<String>, post: Post) -> bool {
    let answer = Arc::clone(&post);
    spawn("flare-plugin-download", move || {
        let result = download_and_stage(&hub, &url, &rules())
            .and_then(|s| check_expected(s, expect.as_deref()));
        post_mods(&answer, ModsMsg::Staged(result.map(Box::new)));
    })
}

/// The download's file, in the staging folder (cleared at start if a run ends half way).
fn download_target(plugins: &Path) -> Result<PathBuf, String> {
    let staging = plugins.join(flare_mods::install::STAGING);
    std::fs::create_dir_all(&staging) // sync-ok: the download worker
        .map_err(|e| format!("The plugins folder cannot be written: {e}."))?;
    let name: String = flare_mods::tokens::new_token()?.chars().take(16).collect();
    Ok(staging.join(format!("{name}.zip")))
}

fn download_and_stage(
    hub: &Hub,
    url: &str,
    rules: &Rules,
) -> Result<flare_mods::hub::StagedInstall, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("Paste the https address of a plugin's .zip first.".to_string());
    }
    let dest = download_target(&hub.config().dir)?;
    let got = fetch::download(url, &dest, rules).and_then(|()| hub.stage_install(&dest, Some(url)));
    let _ = flare_mods::install::delete_path(&dest);
    got
}

/// Waits for the plugin's stopped helper to end (it holds the plugin folder until then), on the worker.
fn wait_helper(helpers: Option<&Host>, id: &str) {
    if let Some(h) = helpers {
        h.wait_stopped(id, flare_mods::helper::STOP_WAIT);
    }
}

/// Puts the staged plugin in place and scans, on a worker, once its stopped helper has ended. Answers
/// `ModsMsg::Installed`.
pub fn commit(
    hub: Arc<Hub>,
    staged: Box<flare_mods::hub::StagedInstall>,
    helpers: Option<Host>,
    post: Post,
) -> bool {
    let answer = Arc::clone(&post);
    spawn("flare-plugin-install", move || {
        wait_helper(helpers.as_ref(), staged.id());
        let result = hub.commit_install(*staged);
        post_mods(&answer, ModsMsg::Installed(result));
    })
}

/// Removes the plugin and scans, on a worker, once its stopped helper has ended. Answers `ModsMsg::Removed`.
pub fn remove(hub: Arc<Hub>, id: String, helpers: Option<Host>, post: Post) -> bool {
    let answer = Arc::clone(&post);
    spawn("flare-plugin-remove", move || {
        wait_helper(helpers.as_ref(), &id);
        let result = hub.remove_plugin(&id);
        post_mods(&answer, ModsMsg::Removed { id, result });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub_in(dir: &Path) -> Hub {
        Hub::new(flare_mods::hub::Config {
            dir: dir.join("plugins"),
            old_mods_dir: None,
            state_file: dir.join("mods.json"),
        })
    }

    #[test]
    fn the_download_takes_https_on_a_public_host_only() {
        let r = rules();
        for bad in [
            "http://example.com/p.zip",
            "ftp://example.com/p.zip",
            "file:///C:/p.zip",
            "https://localhost/p.zip",
            "https://127.0.0.1/p.zip",
            "https://10.0.0.5/p.zip",
            "https://192.168.1.2/p.zip",
            "https://[::1]/p.zip",
            "https://nas.local/p.zip",
            "https://intranet/p.zip",
            "https://user:pw@example.com/p.zip",
        ] {
            assert!((r.address)(bad).is_some(), "{bad}");
        }
        assert_eq!((r.address)("https://example.com/p.zip"), None);
        for bad in [
            "127.0.0.1",
            "10.1.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "::1",
            "fe80::1",
        ] {
            assert!((r.ip)(bad.parse().unwrap()).is_some(), "{bad}");
        }
        assert_eq!((r.ip)("93.184.216.34".parse().unwrap()), None);
        assert_eq!(r.max_bytes, 50 * 1024 * 1024);
    }

    #[test]
    fn a_refused_address_is_one_sentence_and_nothing_is_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub_in(dir.path());
        for (url, want) in [
            ("http://example.com/p.zip", "only https"),
            ("https://127.0.0.1/p.zip", "local or private"),
            ("https://localhost/p.zip", "local host name"),
            ("   ", "Paste the https address"),
        ] {
            let e = download_and_stage(&hub, url, &rules()).unwrap_err();
            assert!(e.contains(want), "{url}: {e}");
        }
        let staging = dir
            .path()
            .join("plugins")
            .join(flare_mods::install::STAGING);
        let left = std::fs::read_dir(&staging).map(|d| d.count()).unwrap_or(0); // sync-ok: test code only
        assert_eq!(left, 0, "no download file is left");
    }

    #[test]
    fn an_update_must_hold_the_same_plugin() {
        assert_eq!(
            not_the_one("other", "demo"),
            "That zip holds the plugin \"other\", not \"demo\"."
        );
    }
}
