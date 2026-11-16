//! The Start menu shortcut wears the app icon pick (rule 1272, Windows only; macOS is unchanged).
//!
//! On every pick, and once at start, a worker finds THIS profile's Start menu shortcut by its taskbar id (the
//! shortcut's `PKEY_AppUserModel_ID` equals `ship_ctl::effective_aumid`), never by its name, and touches only that one.
//! A custom pick is written as a multi-size `.ico` into the profile's data folder under a content-hash name
//! (`app-icon-<16 hex>.ico`, so the shell's icon cache never shows an old picture for it), the shortcut's icon is
//! pointed at it, the shell is told, and older `app-icon-*.ico` files are deleted. The original points the shortcut
//! back at the exe's own icon (index 0), but only when it wears one of Flare's files: an icon the person set by hand
//! stays. No shortcut means nothing is done, and Flare never makes one. The run at start puts the pick back after an
//! update rewrote the shortcut.
//!
//! The file and shell calls sit behind [`IconFiles`] and [`LinkShell`], so the tests run in a temp folder with a fake
//! shell and never touch the real Start menu.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use flare_settings::app_icon::ORIGINAL;
pub use flare_win::shell_link::LinkFacts;
use sha2::{Digest, Sha256};

/// The start of every icon file Flare writes into the profile's data folder.
pub const ICON_PREFIX: &str = "app-icon-";

/// The sizes the `.ico` holds: what the Start menu, the taskbar and Explorer ask for at every scale.
pub const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// The shortcuts' side: read one, point its icon, tell the shell.
pub trait LinkShell: Send + Sync {
    fn read(&self, lnk: &Path) -> Result<LinkFacts, String>;
    fn set_icon(&self, lnk: &Path, icon: &Path, index: i32) -> Result<(), String>;
    fn notify(&self, lnk: &Path);
}

/// The files' side: the shortcuts of a folder, and Flare's icon files in the data folder.
pub trait IconFiles: Send + Sync {
    /// Every `.lnk` directly in `dir`, by name.
    fn links(&self, dir: &Path) -> Vec<PathBuf>;
    /// Every `app-icon-*.ico` directly in `dir`.
    fn icons(&self, dir: &Path) -> Vec<PathBuf>;
    fn exists(&self, path: &Path) -> bool;
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String>;
    fn remove(&self, path: &Path);
}

/// Where the worker looks and writes.
#[derive(Clone, Debug)]
pub struct Places {
    /// The Start menu Programs folder.
    pub programs: PathBuf,
    /// The profile's data folder, which keeps the `.ico`.
    pub data: PathBuf,
    /// The running exe, whose resource 1 is the original icon.
    pub exe: PathBuf,
    /// This profile's taskbar id.
    pub aumid: String,
}

/// What one run did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No shortcut carries this profile's id: nothing was done.
    NoShortcut,
    /// The shortcut already wore the pick (or wears an icon the person set, for the original).
    Unchanged(PathBuf),
    /// The shortcut now points at `icon`.
    Pointed {
        lnk: PathBuf,
        icon: PathBuf,
    },
    Failed(String),
}

/// The `.ico` file name for these bytes: a hash of the content, so a new picture gets a new path.
pub fn ico_name(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{ICON_PREFIX}{hex}.ico")
}

/// The first shortcut in `programs` (by name) whose taskbar id is `aumid` (ASCII case ignored), with what it says.
pub fn find_link(
    files: &dyn IconFiles,
    shell: &dyn LinkShell,
    programs: &Path,
    aumid: &str,
) -> Option<(PathBuf, LinkFacts)> {
    files.links(programs).into_iter().find_map(|lnk| {
        let facts = shell.read(&lnk).ok()?;
        facts
            .app_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(aumid))
            .then_some((lnk, facts))
    })
}

/// Two Windows paths name the same file: case and slash direction ignored.
fn same_path(a: &str, b: &Path) -> bool {
    let norm = |s: &str| s.replace('/', "\\").to_lowercase();
    norm(a) == norm(&b.to_string_lossy())
}

/// The icon is one of Flare's files in the data folder.
fn is_ours(icon: &str, data: &Path) -> bool {
    let p = Path::new(icon);
    let named = p
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .is_some_and(|n| n.starts_with(ICON_PREFIX) && n.ends_with(".ico"));
    named
        && p.parent()
            .is_some_and(|d| same_path(&d.to_string_lossy(), data))
}

/// Puts the pick `id` on this profile's Start menu shortcut. `ico` draws a custom pick's `.ico` bytes; it is only
/// called when a shortcut was found.
pub fn apply(
    files: &dyn IconFiles,
    shell: &dyn LinkShell,
    places: &Places,
    id: &str,
    ico: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Outcome {
    let Some((lnk, facts)) = find_link(files, shell, &places.programs, &places.aumid) else {
        return Outcome::NoShortcut;
    };
    let mut wrote = false;
    let (icon, keep) = if id == ORIGINAL {
        if !is_ours(&facts.icon, &places.data) {
            // The installer's icon, or one the person set by hand: it stays. Flare's leftover files (an update
            // rewrote the shortcut while a pick was worn) go, but never the file the shortcut names.
            let worn_name = Path::new(&facts.icon).file_name();
            for old in files.icons(&places.data) {
                let named = old.file_name().zip(worn_name).is_some_and(|(a, b)| {
                    a.to_string_lossy()
                        .eq_ignore_ascii_case(&b.to_string_lossy())
                });
                if !named {
                    files.remove(&old);
                }
            }
            return Outcome::Unchanged(PathBuf::from(&facts.icon));
        }
        (places.exe.clone(), None)
    } else {
        let Some(bytes) = ico(id) else {
            return Outcome::Failed(format!("{id}: not drawn"));
        };
        let path = places.data.join(ico_name(&bytes));
        if !files.exists(&path) {
            if let Err(e) = files.write(&path, &bytes) {
                return Outcome::Failed(e);
            }
            wrote = true;
        }
        (path.clone(), Some(path))
    };
    let worn = same_path(&facts.icon, &icon) && facts.index == 0;
    if !worn {
        if let Err(e) = shell.set_icon(&lnk, &icon, 0) {
            // The shortcut still names its old file; the one just written for it is not left behind.
            if wrote {
                files.remove(&icon);
            }
            return Outcome::Failed(e);
        }
        shell.notify(&lnk);
    }
    // The shortcut no longer names an older file: those go.
    for old in files.icons(&places.data) {
        if keep.as_ref() != Some(&old) {
            files.remove(&old);
        }
    }
    if worn {
        Outcome::Unchanged(icon)
    } else {
        Outcome::Pointed { lnk, icon }
    }
}

// ---- the real sides ----------------------------------------------------------------------------------------------

/// The disk (the worker's thread only).
pub struct DiskFiles;

impl DiskFiles {
    fn entries(dir: &Path, keep: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            // sync-ok: the start menu icon worker
            return Vec::new();
        };
        let mut out: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| keep(&n.to_string_lossy().to_lowercase()))
            })
            .collect();
        out.sort();
        out
    }
}

impl IconFiles for DiskFiles {
    fn links(&self, dir: &Path) -> Vec<PathBuf> {
        Self::entries(dir, &|n| n.ends_with(".lnk"))
    }

    fn icons(&self, dir: &Path) -> Vec<PathBuf> {
        Self::entries(dir, &|n| n.starts_with(ICON_PREFIX) && n.ends_with(".ico"))
    }

    fn exists(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        // Written beside it, then renamed: the shell never reads half an icon.
        let tmp = path.with_extension("ico.tmp");
        std::fs::write(&tmp, bytes) // sync-ok: the start menu icon worker
            .and_then(|()| std::fs::rename(&tmp, path)) // sync-ok: the start menu icon worker
            .map_err(|e| format!("icon file: {e}"))
    }

    fn remove(&self, path: &Path) {
        let _ = std::fs::remove_file(path); // sync-ok: the start menu icon worker
    }
}

/// The shell (`flare_win::shell_link`, COM on the worker's thread).
pub struct SystemShell;

impl LinkShell for SystemShell {
    fn read(&self, lnk: &Path) -> Result<LinkFacts, String> {
        flare_win::shell_link::read(lnk)
    }

    fn set_icon(&self, lnk: &Path, icon: &Path, index: i32) -> Result<(), String> {
        flare_win::shell_link::set_icon(lnk, icon, index)
    }

    fn notify(&self, lnk: &Path) {
        flare_win::shell_link::notify_changed(lnk);
    }
}

/// The worker: takes picks, keeps only the newest of a burst, and applies it off the UI thread.
pub struct Worker {
    tx: std::sync::mpsc::Sender<&'static str>,
}

impl Worker {
    /// Starts the `flare-start-menu-icon` thread. `places` is found on it (the Programs folder is a shell call).
    pub fn spawn(
        places: Box<dyn FnOnce() -> Option<Places> + Send>,
        files: Arc<dyn IconFiles>,
        shell: Arc<dyn LinkShell>,
    ) -> Option<Worker> {
        let (tx, rx) = std::sync::mpsc::channel::<&'static str>();
        let spawned = std::thread::Builder::new()
            .name("flare-start-menu-icon".into())
            .spawn(move || {
                let Some(places) = places() else {
                    log::warn!("start menu icon: no Programs folder");
                    return;
                };
                // sync-ok: the worker waits for the next pick
                while let Ok(mut id) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        id = newer;
                    }
                    let out = apply(&*files, &*shell, &places, id, &|i| {
                        crate::app_icon::render_ico(i)
                    });
                    match out {
                        Outcome::Failed(why) => log::warn!("start menu icon {id}: {why}"),
                        Outcome::Pointed { .. } => log::info!("start menu icon: {id}"),
                        Outcome::NoShortcut | Outcome::Unchanged(_) => {}
                    }
                }
            });
        match spawned {
            Ok(_) => Some(Worker { tx }),
            Err(e) => {
                log::warn!("start menu icon: no worker: {e}");
                None
            }
        }
    }

    /// Asks the worker to wear `id`.
    pub fn wear(&self, id: &'static str) {
        let _ = self.tx.send(id);
    }
}

/// The worker for this run, when it may touch the Start menu: an installed Flare on Windows, never a dev build (its
/// exe is not the shortcut's) or an `--e2e` run.
#[cfg(windows)]
pub fn for_run(ship: &crate::ship_ctl::Ship, profile: &Path) -> Option<Worker> {
    if !ship.packaged || ship.e2e {
        return None;
    }
    // A relative `--user-data-dir` would leave the shortcut naming a path that depends on the shell's folder.
    let data = std::path::absolute(profile).unwrap_or_else(|_| profile.to_path_buf());
    let exe = ship.exe.clone();
    let aumid = crate::ship_ctl::effective_aumid();
    Worker::spawn(
        Box::new(move || {
            Some(Places {
                programs: flare_win::shell_link::programs_dir()?,
                data,
                exe,
                aumid,
            })
        }),
        Arc::new(DiskFiles),
        Arc::new(SystemShell),
    )
}

/// macOS keeps its Dock icon only (rule 1231); there is no Start menu.
#[cfg(not(windows))]
pub fn for_run(_ship: &crate::ship_ctl::Ship, _profile: &Path) -> Option<Worker> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A shell over a map of shortcut facts; it records every write and notice.
    #[derive(Default)]
    struct FakeShell {
        links: Mutex<HashMap<PathBuf, LinkFacts>>,
        sets: Mutex<Vec<(PathBuf, PathBuf, i32)>>,
        notified: Mutex<Vec<PathBuf>>,
    }

    impl FakeShell {
        fn with(&self, lnk: &Path, app_id: Option<&str>, icon: &str) {
            self.links.lock().unwrap().insert(
                lnk.to_path_buf(),
                LinkFacts {
                    app_id: app_id.map(str::to_string),
                    icon: icon.to_string(),
                    index: 0,
                },
            );
        }
        fn icon_of(&self, lnk: &Path) -> String {
            self.links.lock().unwrap()[lnk].icon.clone()
        }
    }

    impl LinkShell for FakeShell {
        fn read(&self, lnk: &Path) -> Result<LinkFacts, String> {
            self.links
                .lock()
                .unwrap()
                .get(lnk)
                .cloned()
                .ok_or_else(|| "not a shortcut".to_string())
        }
        fn set_icon(&self, lnk: &Path, icon: &Path, index: i32) -> Result<(), String> {
            let mut links = self.links.lock().unwrap();
            let f = links.get_mut(lnk).ok_or("not a shortcut")?;
            f.icon = icon.to_string_lossy().into_owned();
            f.index = index;
            self.sets
                .lock()
                .unwrap()
                .push((lnk.to_path_buf(), icon.to_path_buf(), index));
            Ok(())
        }
        fn notify(&self, lnk: &Path) {
            self.notified.lock().unwrap().push(lnk.to_path_buf());
        }
    }

    const AUMID: &str = "com.flare.terminal.p0123456789abcdef";

    /// A temp Start menu with three shortcuts (each an empty file; the fake shell says what they carry): another
    /// app, the other Flare profile and this one, under a name that is not "Flare".
    fn setup() -> (tempfile::TempDir, Places, FakeShell, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let programs = root.path().join("Programs");
        let data = root.path().join("Profile");
        std::fs::create_dir_all(&programs).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let exe = root.path().join("current").join("FlareStable.exe");
        let shell = FakeShell::default();
        let other = programs.join("Another App.lnk");
        let dev = programs.join("Flare.lnk");
        let mine = programs.join("Zed Renamed.lnk");
        for l in [&other, &dev, &mine] {
            std::fs::write(l, b"").unwrap();
        }
        std::fs::write(programs.join("readme.txt"), b"").unwrap();
        shell.with(&other, None, r"C:\Other\other.exe");
        shell.with(
            &dev,
            Some("com.flare.terminal"),
            r"C:\Flare\current\Flare.exe",
        );
        let exe_text = exe.to_string_lossy().into_owned();
        shell.with(&mine, Some(&AUMID.to_uppercase()), &exe_text);
        let places = Places {
            programs,
            data,
            exe,
            aumid: AUMID.to_string(),
        };
        (root, places, shell, mine)
    }

    fn fake_ico(id: &str) -> Option<Vec<u8>> {
        Some(format!("ico of {id}").into_bytes())
    }

    /// Rule 1272: the shortcut is found by this profile's id, not its name, and only it is touched; the `.ico` is
    /// written under its content hash and the shell is told.
    #[test]
    fn the_shortcut_is_matched_by_its_id_and_only_it_is_touched() {
        let (_root, places, shell, mine) = setup();
        let found = find_link(&DiskFiles, &shell, &places.programs, AUMID).expect("found");
        assert_eq!(found.0, mine);
        let out = apply(&DiskFiles, &shell, &places, "daybreak", &fake_ico);
        let ico = places.data.join(ico_name(b"ico of daybreak"));
        assert_eq!(
            out,
            Outcome::Pointed {
                lnk: mine.clone(),
                icon: ico.clone()
            }
        );
        assert_eq!(std::fs::read(&ico).unwrap(), b"ico of daybreak");
        assert_eq!(*shell.sets.lock().unwrap(), [(mine.clone(), ico, 0)]);
        assert_eq!(*shell.notified.lock().unwrap(), [mine]);
        assert_eq!(
            shell.icon_of(&places.programs.join("Flare.lnk")),
            r"C:\Flare\current\Flare.exe",
            "the other profile's shortcut is untouched"
        );
    }

    /// A new pick writes a new file and deletes the old one; the same pick again writes nothing and tells nobody.
    #[test]
    fn a_new_pick_replaces_the_old_file_and_the_same_pick_is_a_no_op() {
        let (_root, places, shell, mine) = setup();
        apply(&DiskFiles, &shell, &places, "daybreak", &fake_ico);
        let first = places.data.join(ico_name(b"ico of daybreak"));
        apply(&DiskFiles, &shell, &places, "jade-glass", &fake_ico);
        let second = places.data.join(ico_name(b"ico of jade-glass"));
        assert!(!first.exists(), "the old icon is deleted");
        assert!(second.exists());
        assert_eq!(shell.icon_of(&mine), second.to_string_lossy());
        let out = apply(&DiskFiles, &shell, &places, "jade-glass", &fake_ico);
        assert_eq!(out, Outcome::Unchanged(second));
        assert_eq!(shell.sets.lock().unwrap().len(), 2);
        assert_eq!(shell.notified.lock().unwrap().len(), 2);
    }

    /// The original points the shortcut back at the exe's icon 0 and deletes Flare's files.
    #[test]
    fn the_original_restores_the_exes_icon_zero() {
        let (_root, places, shell, mine) = setup();
        apply(&DiskFiles, &shell, &places, "safety-orange", &fake_ico);
        let out = apply(&DiskFiles, &shell, &places, ORIGINAL, &fake_ico);
        assert_eq!(
            out,
            Outcome::Pointed {
                lnk: mine.clone(),
                icon: places.exe.clone()
            }
        );
        let last = shell.sets.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last, (mine, places.exe.clone(), 0));
        assert!(DiskFiles.icons(&places.data).is_empty());
        assert_eq!(shell.notified.lock().unwrap().len(), 2);
    }

    /// The original at start leaves an icon Flare did not set (the installer's, or one set by hand).
    #[test]
    fn the_original_leaves_an_icon_flare_did_not_set() {
        let (_root, places, shell, mine) = setup();
        shell.with(&mine, Some(AUMID), r"D:\Icons\mine.ico");
        let out = apply(&DiskFiles, &shell, &places, ORIGINAL, &fake_ico);
        assert_eq!(out, Outcome::Unchanged(PathBuf::from(r"D:\Icons\mine.ico")));
        assert!(shell.sets.lock().unwrap().is_empty());
        assert!(shell.notified.lock().unwrap().is_empty());
    }

    /// An update that rewrote the shortcut back to the exe's icon gets the pick again at the next start.
    #[test]
    fn a_rewritten_shortcut_gets_the_pick_again() {
        let (_root, places, shell, mine) = setup();
        apply(&DiskFiles, &shell, &places, "daybreak", &fake_ico);
        shell.with(&mine, Some(AUMID), &places.exe.to_string_lossy());
        let out = apply(&DiskFiles, &shell, &places, "daybreak", &fake_ico);
        assert!(matches!(out, Outcome::Pointed { .. }), "{out:?}");
        assert_eq!(shell.notified.lock().unwrap().len(), 2);
    }

    /// No shortcut with this id: nothing is drawn, written, set or told.
    #[test]
    fn no_shortcut_is_a_no_op() {
        let (_root, mut places, shell, _mine) = setup();
        places.aumid = "com.flare.terminal.pffffffffffffffff".into();
        let drawn = std::cell::Cell::new(false);
        let out = apply(&DiskFiles, &shell, &places, "daybreak", &|i| {
            drawn.set(true);
            fake_ico(i)
        });
        assert_eq!(out, Outcome::NoShortcut);
        assert!(!drawn.get());
        assert!(DiskFiles.icons(&places.data).is_empty());
        assert!(shell.sets.lock().unwrap().is_empty());
        assert!(shell.notified.lock().unwrap().is_empty());
    }

    /// A failed write of the shortcut keeps the old file (the shortcut still names it) and tells nobody.
    #[test]
    fn a_failed_set_keeps_the_old_file() {
        struct Refusing(FakeShell);
        impl LinkShell for Refusing {
            fn read(&self, lnk: &Path) -> Result<LinkFacts, String> {
                self.0.read(lnk)
            }
            fn set_icon(&self, _: &Path, _: &Path, _: i32) -> Result<(), String> {
                Err("denied".into())
            }
            fn notify(&self, lnk: &Path) {
                self.0.notify(lnk);
            }
        }
        let (_root, places, shell, _mine) = setup();
        apply(&DiskFiles, &shell, &places, "daybreak", &fake_ico);
        let refusing = Refusing(shell);
        let out = apply(&DiskFiles, &refusing, &places, "lemon-ink", &fake_ico);
        assert_eq!(out, Outcome::Failed("denied".into()));
        assert!(places.data.join(ico_name(b"ico of daybreak")).exists());
        assert!(
            !places.data.join(ico_name(b"ico of lemon-ink")).exists(),
            "the file written for the failed pick is not left behind"
        );
        assert_eq!(refusing.0.notified.lock().unwrap().len(), 1);
    }

    /// An update rewrote the shortcut while a pick was worn, then the original is picked: the shortcut is left alone
    /// and Flare's leftover files go, but never a file the shortcut still names.
    #[test]
    fn the_original_on_a_rewritten_shortcut_clears_leftover_files() {
        let (_root, places, shell, mine) = setup();
        apply(&DiskFiles, &shell, &places, "daybreak", &fake_ico);
        std::fs::write(places.data.join("app-icon-mine.ico"), b"x").unwrap();
        shell.with(
            &mine,
            Some(AUMID),
            r"D:\Elsewhere\APP-ICON-MINE.ICO", // a hand-set copy that shares the name: kept
        );
        let out = apply(&DiskFiles, &shell, &places, ORIGINAL, &fake_ico);
        assert!(matches!(out, Outcome::Unchanged(_)), "{out:?}");
        assert!(!places.data.join(ico_name(b"ico of daybreak")).exists());
        assert!(places.data.join("app-icon-mine.ico").exists());
        assert_eq!(shell.sets.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_file_name_is_a_content_hash() {
        let a = ico_name(b"one");
        assert_eq!(a, ico_name(b"one"));
        assert_ne!(a, ico_name(b"two"));
        assert!(a.starts_with(ICON_PREFIX) && a.ends_with(".ico"));
        assert_eq!(a.len(), ICON_PREFIX.len() + 16 + 4);
    }

    /// Rule 1272: the worker thread is the only caller of the shell here, and Flare never makes a shortcut.
    #[test]
    fn the_shell_runs_on_the_worker_and_no_shortcut_is_made() {
        let src = include_str!("start_menu_icon.rs");
        let body = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(body.contains("flare-start-menu-icon"));
        assert!(!body.contains(concat!("Set", "Path(")));
        assert!(!body.contains(concat!("CoCreate", "Instance")));
    }
}
