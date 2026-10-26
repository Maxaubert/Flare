//! The hub's part of install, update, remove and the helper program (plugins phase 7, design 9 and 13).
//!
//! - **Install and update** come in two steps, both on a worker, with the window in between: [`Hub::stage_install`]
//!   unpacks and checks the zip in the staging folder (nothing changes yet; a downgrade is refused here), the window
//!   stops the plugin's views and helper when it replaces one that is on ([`Hub::pause`] keeps them from starting
//!   again meanwhile), and [`Hub::commit_install`] puts the folder in place, records where it came from and its hash,
//!   and scans. The scan decides what the plugin may do: a new plugin is off (an install never turns one on); an
//!   update that asks for more permissions, or names another author, homepage or source, is turned off until it is
//!   approved again, which is the existing consent check of `reconcile`. A change of maker also wipes its storage and
//!   its browser profile, so a new owner never reads what the old one kept.
//! - **Remove** ([`Hub::remove_plugin`]): the window turns the plugin off first; here its folder goes to the Recycle
//!   Bin, and its approval, install record, storage, helper log and browser profile are forgotten.
//! - **The helper** ([`Hub::helper_of`]): only a protocol 2 plugin that is on, holds `process` in its live grant and
//!   declares a helper gets one, and not while it is paused or stopped.
//!
//! Each step holds the hub's disk lock, so a scan, an install and a removal never run at once.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flare_ctl::{Grant, Permission};

use super::{Change, Config, Hub, ProfileDirs, Recycle};
use crate::enabled::{EnabledStore, Identity};
use crate::install::{self, Staged};

/// Where the helpers' logs are kept: `plugins-logs` beside the switch file (the profile's folder).
pub fn logs_dir(cfg: &Config) -> PathBuf {
    cfg.state_file
        .parent()
        .map_or_else(|| cfg.dir.join(".logs"), Path::to_path_buf)
        .join("plugins-logs")
}

/// A zip unpacked and checked, waiting for the window to stop what it replaces.
#[derive(Debug)]
pub struct StagedInstall {
    pub staged: Staged,
    /// The `https` address it was downloaded from; None for a file.
    pub source: Option<String>,
    /// A plugin of the same id is installed.
    pub replaces: bool,
    /// That plugin is on (its views and helper are stopped before the commit).
    pub was_on: bool,
}

impl StagedInstall {
    pub fn id(&self) -> &str {
        self.staged.id()
    }
}

/// What an install did, for Settings and the window.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub id: String,
    pub name: String,
    pub version: String,
    /// It replaced a copy of the same id (an update).
    pub replaced: bool,
    /// It was on and the new version must be approved again (more permissions, another maker or source).
    pub needs_approval: bool,
    /// What the scan after it changed.
    pub change: Change,
}

impl Installed {
    /// The sentence Settings shows.
    pub fn sentence(&self) -> String {
        match (self.replaced, self.needs_approval) {
            (true, true) => format!(
                "{} was updated to {}. It asks for more than you allowed, so it is off until you turn it on again.",
                self.name, self.version
            ),
            (true, false) => format!("{} was updated to {}.", self.name, self.version),
            (false, _) => format!(
                "{} {} is installed. It is off: read what it asks for, then turn it on.",
                self.name, self.version
            ),
        }
    }
}

impl Hub {
    /// Where removed and replaced plugin folders go (the app keeps the Recycle Bin; a test deletes them).
    pub fn set_recycle(&self, r: Recycle) {
        *self.recycle.lock().unwrap_or_else(|e| e.into_inner()) = r;
    }

    /// The browser profile folders of a plugin, for a removal to delete (the app's; the WebView2 folders).
    pub fn set_profile_dirs(&self, p: ProfileDirs) {
        *self.profiles.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
    }

    /// Where the helpers' logs are.
    pub fn logs_dir(&self) -> PathBuf {
        logs_dir(&self.cfg)
    }

    /// Unpacks and checks `zip` in the staging folder (a worker). Nothing is installed yet. Refused, with one sentence:
    /// a zip against the rules, a manifest that is not valid, and an older version of an installed plugin.
    pub fn stage_install(&self, zip: &Path, source: Option<&str>) -> Result<StagedInstall, String> {
        let _disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        let staged = install::stage(zip, &self.cfg.dir)?;
        let new = &staged.found.manifest;
        let (replaces, was_on, old_version) = {
            let g = self.lock();
            let old = g.found.iter().find(|f| f.manifest.id == new.id);
            (
                old.is_some() || self.cfg.dir.join(&new.id).exists(),
                g.on.contains(&new.id),
                old.map(|f| f.manifest.version.clone()),
            )
        };
        if let Some(old) = old_version
            && crate::manifest::compare_versions(&new.version, &old) == Ordering::Less
        {
            return Err(format!(
                "This is version {} of {}, older than the installed {old}. Remove it first to install an older one.",
                new.version, new.name
            ));
        }
        Ok(StagedInstall {
            staged,
            source: source.map(str::to_string),
            replaces,
            was_on,
        })
    }

    /// Keeps the plugin's views and helper from starting until the install that replaces it is done.
    pub fn pause(&self, id: &str) {
        self.lock().paused.insert(id.to_string());
    }

    /// The install that paused the plugin did not go ahead.
    pub fn unpause(&self, id: &str) {
        self.lock().paused.remove(id);
    }

    pub fn is_paused(&self, id: &str) -> bool {
        self.lock().paused.contains(id)
    }

    /// Puts the staged plugin in place (a worker; the window stopped what it replaces), records where it came from and
    /// its hash, and scans. A change of maker or source wipes its storage and browser profile.
    pub fn commit_install(&self, s: StagedInstall) -> Result<Installed, String> {
        let _disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        let id = s.id().to_string();
        let (name, version) = (
            s.staged.found.manifest.name.clone(),
            s.staged.found.manifest.version.clone(),
        );
        let maker =
            |author: &Option<String>, homepage: &Option<String>, source: &Option<String>| {
                (author.clone(), homepage.clone(), source.clone())
            };
        let new_maker = maker(
            &s.staged.found.manifest.author,
            &s.staged.found.manifest.homepage,
            &s.source,
        );
        let old_maker = {
            let g = self.lock();
            g.found.iter().find(|f| f.manifest.id == id).map(|f| {
                maker(
                    &f.manifest.author,
                    &f.manifest.homepage,
                    &g.installed.get(&id).and_then(|i| i.source.clone()),
                )
            })
        };
        let hash = s.staged.hash.clone();
        let recycle = Arc::clone(&self.recycle.lock().unwrap_or_else(|e| e.into_inner()));
        let committed = install::commit(s.staged, &self.cfg.dir, &*recycle);
        let committed = match committed {
            Ok(c) => c,
            Err(why) => {
                self.lock().paused.remove(&id);
                return Err(why);
            }
        };
        let record = Identity {
            source: s.source.clone(),
            hash: Some(hash.clone()),
            ..Identity::default()
        };
        if let Err(why) =
            self.with_store(|st| st.set_installed(&id, s.source.as_deref(), Some(&hash)))
        {
            log::warn!("where plugin {id} came from was not saved: {why}");
        }
        {
            let mut g = self.lock();
            g.installed.insert(id.clone(), record);
            // What was started for the old copy is never applied to the new one.
            *g.generations.entry(id.clone()).or_insert(0) += 1;
            g.paused.remove(&id);
        }
        if old_maker.is_some_and(|old| old != new_maker) {
            log::info!(
                "plugin {id}: another maker or source; its storage and browser profile are wiped"
            );
            self.wipe_data(&id);
        }
        let change = self.scan_locked();
        let needs_approval = self.lock().needs.contains(&id);
        log::info!(
            "plugin {id}: {} {version}",
            if committed.replaced {
                "updated to"
            } else {
                "installed"
            }
        );
        Ok(Installed {
            id,
            name,
            version,
            replaced: committed.replaced,
            needs_approval,
            change,
        })
    }

    /// Removes the plugin (a worker; the window turned it off and stopped it first): its folder to the Recycle Bin, and
    /// its approval, install record, storage, helper log and browser profile forgotten. Then scans.
    pub fn remove_plugin(&self, id: &str) -> Result<Change, String> {
        let _disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        if !crate::manifest::valid_id(id) {
            return Err(format!("\"{id}\" is not a plugin id"));
        }
        let recycle = Arc::clone(&self.recycle.lock().unwrap_or_else(|e| e.into_inner()));
        install::remove(&self.cfg.dir, id, &*recycle)
            .map_err(|why| format!("The folder of {id} was not removed: {why}."))?;
        if let Err(why) = self.with_store(|st| st.forget(id)) {
            log::warn!("the approval of plugin {id} was not forgotten on disk: {why}");
        }
        {
            let mut g = self.lock();
            g.on.remove(id);
            g.approved.remove(id);
            g.identity.remove(id);
            g.installed.remove(id);
            g.needs.remove(id);
            g.stopped.remove(id);
            g.helper_stopped.remove(id);
            g.paused.remove(id);
            *g.generations.entry(id.to_string()).or_insert(0) += 1;
        }
        self.wipe_data(id);
        let _ = install::delete_path(&crate::helper::log_file(&self.logs_dir(), id));
        let mut old = crate::helper::log_file(&self.logs_dir(), id).into_os_string();
        old.push(".1");
        let _ = install::delete_path(Path::new(&old));
        log::info!("plugin {id}: removed");
        Ok(self.scan_locked())
    }

    /// Deletes what the plugin kept: its storage and its browser profile folders.
    fn wipe_data(&self, id: &str) {
        if let Err(why) = self.storage.wipe(id) {
            log::warn!("plugin {id}: {why}");
        }
        let dirs = self
            .profiles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .map(|p| p(id))
            .unwrap_or_default();
        for d in dirs {
            if let Err(why) = install::delete_path(&d) {
                log::warn!("plugin {id}: its browser profile was not deleted: {why}");
            }
        }
    }

    /// Runs `f` on the switch file's store (loaded if no scan has yet).
    fn with_store(
        &self,
        f: impl FnOnce(&mut EnabledStore) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let st = store.get_or_insert_with(|| EnabledStore::load(self.cfg.state_file.clone())); // sync-ok: runs on a worker
        f(st)
    }

    // ---- the helper -------------------------------------------------------------------------------------------------

    /// What starting the plugin's helper takes, for a protocol 2 plugin that is on, holds `process` in its live grant
    /// and declares a helper, and is neither paused nor stopped. It carries the plugin's own token: a secret, for the
    /// helper's environment only.
    pub fn helper_of(&self, id: &str) -> Option<crate::helper::Spec> {
        let g = self.lock();
        if g.paused.contains(id) || g.helper_stopped.contains_key(id) {
            return None;
        }
        let token = g.tokens.token_of(id)?;
        let Some(Grant::Mod { permissions, .. }) = g.tokens.grant(token) else {
            return None;
        };
        if !permissions.contains(&Permission::Process) {
            return None;
        }
        let f = g
            .found
            .iter()
            .find(|f| f.manifest.id == id && f.manifest.is_plugin())?;
        let h = f.manifest.helper.as_ref()?;
        Some(crate::helper::Spec {
            id: id.to_string(),
            folder: f.folder.clone(),
            command: h.command.clone(),
            args: h.args.clone(),
            token: token.to_string(),
        })
    }

    /// The plugins whose helper should run now ([`Hub::helper_of`] has one), by id.
    pub fn with_helper(&self) -> Vec<String> {
        let ids: Vec<String> = {
            let g = self.lock();
            g.found
                .iter()
                .filter(|f| f.manifest.is_plugin() && f.manifest.helper.is_some())
                .map(|f| f.manifest.id.clone())
                .collect()
        };
        ids.into_iter()
            .filter(|id| self.helper_of(id).is_some())
            .collect()
    }

    /// The window stopped the plugin's helper (`Some(why)`: it ended too often, or did not start), or let it run again.
    pub fn set_helper_stopped(&self, id: &str, why: Option<&str>) {
        let mut g = self.lock();
        match why {
            Some(w) => g.helper_stopped.insert(id.to_string(), w.to_string()),
            None => g.helper_stopped.remove(id),
        };
    }

    pub fn helper_stopped(&self, id: &str) -> Option<String> {
        self.lock().helper_stopped.get(id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::Shown;
    use std::io::{Cursor, Write};

    fn plugin_json(version: &str, perms: &str, author: &str) -> String {
        format!(
            r#"{{"id":"demo","name":"Demo","version":"{version}","protocol":2,"description":"d","author":"{author}","permissions":{perms},"background":"bg.html"}}"#
        )
    }

    fn zip_file(dir: &Path, name: &str, manifest: &str) -> PathBuf {
        let mut cur = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cur);
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("plugin.json", o).unwrap();
            w.write_all(manifest.as_bytes()).unwrap();
            w.start_file("bg.html", o).unwrap();
            w.write_all(b"<p>bg</p>").unwrap();
            w.finish().unwrap();
        }
        let p = dir.join(name);
        std::fs::write(&p, cur.into_inner()).unwrap(); // sync-ok: test code only
        p
    }

    struct World {
        dir: tempfile::TempDir,
        hub: Hub,
    }

    fn world() -> World {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::new(Config {
            dir: dir.path().join("plugins"),
            old_mods_dir: None,
            state_file: dir.path().join("mods.json"),
        });
        hub.set_recycle(Arc::new(|p: &Path| install::delete_path(p)));
        let profile = dir.path().join("profiles");
        hub.set_profile_dirs(Arc::new(move |id: &str| vec![profile.join(id)]));
        hub.scan();
        World { dir, hub }
    }

    fn install(
        w: &World,
        name: &str,
        manifest: &str,
        source: Option<&str>,
    ) -> Result<Installed, String> {
        let z = zip_file(w.dir.path(), name, manifest);
        let staged = w.hub.stage_install(&z, source)?;
        w.hub.commit_install(staged)
    }

    fn turn_on(w: &World) {
        let f = w.hub.found("demo").unwrap();
        let shown = Shown {
            permissions: f
                .manifest
                .permissions
                .iter()
                .map(|p| crate::hub::permission_words(*p).to_string())
                .collect(),
            author: f.manifest.author.clone().unwrap_or_default(),
            homepage: String::new(),
        };
        w.hub.set_enabled_shown("demo", true, Some(&shown)).unwrap();
        w.hub.persist("demo").unwrap();
    }

    fn state(w: &World) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(w.dir.path().join("mods.json")).unwrap())
            .unwrap() // sync-ok: test code only
    }

    #[test]
    fn an_install_lands_off_with_its_source_and_hash_recorded() {
        let w = world();
        let done = install(
            &w,
            "a.zip",
            &plugin_json("1.0.0", r#"["decorate"]"#, "me"),
            Some("https://example.com/demo.zip"),
        )
        .expect("installed");
        assert!(!done.replaced && !done.needs_approval);
        assert!(
            done.sentence().contains("is installed. It is off"),
            "{}",
            done.sentence()
        );
        assert!(w.hub.found("demo").is_some());
        assert!(
            !w.hub.is_enabled("demo"),
            "an install never turns a plugin on"
        );
        let s = state(&w);
        assert_eq!(s["mods"]["demo"]["on"], false);
        assert_eq!(
            s["mods"]["demo"]["installed"]["source"],
            "https://example.com/demo.zip"
        );
        assert!(crate::hash::is_hash(
            s["mods"]["demo"]["installed"]["hash"].as_str().unwrap()
        ));
        assert_eq!(
            w.hub.snapshot().mods[0].source.as_deref(),
            Some("https://example.com/demo.zip")
        );
        // Turning it on keeps the source with the approval.
        turn_on(&w);
        assert_eq!(
            state(&w)["mods"]["demo"]["source"],
            "https://example.com/demo.zip"
        );
    }

    #[test]
    fn an_update_that_asks_for_more_is_turned_off_and_needs_approval() {
        let w = world();
        install(
            &w,
            "a.zip",
            &plugin_json("1.0.0", r#"["decorate"]"#, "me"),
            None,
        )
        .unwrap();
        turn_on(&w);
        let before = w.hub.generation("demo");
        // The same permissions: it stays on, as a newer document.
        let same = install(
            &w,
            "b.zip",
            &plugin_json("1.1.0", r#"["decorate"]"#, "me"),
            None,
        )
        .unwrap();
        assert!(same.replaced && !same.needs_approval, "{same:?}");
        assert!(w.hub.is_enabled("demo"));
        assert!(
            w.hub.generation("demo") > before,
            "the generation moves on with an update"
        );
        // More: off until approved again.
        let more = install(
            &w,
            "c.zip",
            &plugin_json("1.2.0", r#"["decorate","tabs"]"#, "me"),
            None,
        )
        .unwrap();
        assert!(more.replaced && more.needs_approval, "{more:?}");
        assert!(more.sentence().contains("off until you turn it on again"));
        assert!(!w.hub.is_enabled("demo"));
        assert!(w.hub.snapshot().mods[0].needs_approval);
        assert!(more.change.revoked.contains(&"demo".to_string()));
        assert_eq!(w.hub.found("demo").unwrap().manifest.version, "1.2.0");
    }

    #[test]
    fn an_older_version_is_refused_and_nothing_changes() {
        let w = world();
        install(&w, "a.zip", &plugin_json("2.0.0", "[]", "me"), None).unwrap();
        let e = install(&w, "b.zip", &plugin_json("1.9.0", "[]", "me"), None).unwrap_err();
        assert!(e.contains("older than the installed 2.0.0"), "{e}");
        assert_eq!(w.hub.found("demo").unwrap().manifest.version, "2.0.0");
        let staging = w.dir.path().join("plugins").join(install::STAGING);
        assert_eq!(
            std::fs::read_dir(staging).unwrap().count(),
            0,
            "the refused stage is gone"
        ); // sync-ok: test code only
    }

    #[test]
    fn another_maker_or_source_turns_it_off_and_wipes_its_storage_and_profile() {
        let w = world();
        install(
            &w,
            "a.zip",
            &plugin_json("1.0.0", "[]", "me"),
            Some("https://a.example/x.zip"),
        )
        .unwrap();
        turn_on(&w);
        w.hub
            .storage()
            .set("demo", Some("k"), serde_json::json!(1))
            .unwrap();
        let file = w.hub.storage().file_of("demo");
        let t = std::time::Instant::now();
        while !file.exists() {
            assert!(t.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let profile = w.dir.path().join("profiles").join("demo");
        std::fs::create_dir_all(&profile).unwrap(); // sync-ok: test code only
        let moved = install(
            &w,
            "b.zip",
            &plugin_json("1.0.1", "[]", "me"),
            Some("https://b.example/x.zip"),
        )
        .unwrap();
        assert!(moved.needs_approval, "another source is a new approval");
        assert!(!w.hub.is_enabled("demo"));
        assert!(!file.exists(), "its storage is wiped");
        assert!(!profile.exists(), "its browser profile is wiped");
        assert_eq!(
            w.hub.storage().get("demo", Some("k")).unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn remove_forgets_the_approval_the_record_and_the_storage() {
        let w = world();
        install(
            &w,
            "a.zip",
            &plugin_json("1.0.0", r#"["decorate"]"#, "me"),
            Some("https://a.example/x.zip"),
        )
        .unwrap();
        turn_on(&w);
        w.hub
            .storage()
            .set("demo", Some("k"), serde_json::json!(1))
            .unwrap();
        let logs = w.hub.logs_dir();
        std::fs::create_dir_all(&logs).unwrap(); // sync-ok: test code only
        std::fs::write(crate::helper::log_file(&logs, "demo"), b"x").unwrap(); // sync-ok: test code only
        w.hub.set_enabled("demo", false).unwrap();
        w.hub.remove_plugin("demo").unwrap();
        assert!(w.hub.found("demo").is_none());
        assert!(!w.dir.path().join("plugins").join("demo").exists());
        assert!(
            state(&w)["mods"].get("demo").is_none(),
            "the approval and the record are gone"
        );
        assert!(!w.hub.storage().file_of("demo").exists());
        assert!(!crate::helper::log_file(&logs, "demo").exists());
        assert!(w.hub.snapshot().mods.is_empty());
        // Installed again, it is a new plugin: off, and nothing approved.
        install(
            &w,
            "b.zip",
            &plugin_json("1.0.0", r#"["decorate"]"#, "me"),
            None,
        )
        .unwrap();
        assert!(!w.hub.is_enabled("demo"));
        assert!(!w.hub.snapshot().mods[0].needs_approval);
    }

    #[test]
    fn only_a_plugin_that_is_on_and_holds_process_has_a_helper() {
        let w = world();
        let with_helper = r#"{"id":"demo","name":"Demo","version":"1.0.0","protocol":2,"description":"d","permissions":["process"],"helper":{"command":"bg.html.exe"},"background":"bg.html"}"#;
        // The helper must be a real file: write it beside the manifest by hand after the install fails on it.
        let z = zip_file(w.dir.path(), "a.zip", with_helper);
        assert!(
            w.hub
                .stage_install(&z, None)
                .unwrap_err()
                .contains("helper.command")
        );
        let folder = w.dir.path().join("plugins").join("demo");
        std::fs::create_dir_all(&folder).unwrap(); // sync-ok: test code only
        std::fs::write(folder.join("plugin.json"), with_helper).unwrap(); // sync-ok: test code only
        std::fs::write(folder.join("bg.html"), "x").unwrap(); // sync-ok: test code only
        std::fs::write(folder.join("bg.html.exe"), "x").unwrap(); // sync-ok: test code only
        w.hub.scan();
        assert!(w.hub.helper_of("demo").is_none(), "off: no helper");
        turn_on(&w);
        let spec = w.hub.helper_of("demo").expect("on with process: a helper");
        assert_eq!(spec.command, "bg.html.exe");
        assert_eq!(
            w.hub
                .grant_of_token(&spec.token)
                .map(|g| g.permission_names()),
            Some(vec!["process"])
        );
        assert_eq!(w.hub.with_helper(), vec!["demo".to_string()]);
        w.hub.set_helper_stopped("demo", Some("stopped"));
        assert!(w.hub.helper_of("demo").is_none());
        assert_eq!(w.hub.snapshot().mods[0].stopped.as_deref(), Some("stopped"));
        w.hub.set_helper_stopped("demo", None);
        w.hub.pause("demo");
        assert!(
            w.hub.helper_of("demo").is_none(),
            "paused for an update: no helper"
        );
    }
}
