//! The scan of the two plugin folders: `<profile>\mods\<id>\` and `<profile>\plugins\<id>\`, each folder holding
//! a `mod.json` (protocol 1) or a `plugin.json` (protocol 2).
//!
//! Worker-callable: it reads the disk, so the app runs it on a worker and posts the answer back. It
//! never fails as a whole. A folder that is not a mod becomes one [`FolderError`] with its reason, and
//! every other folder is still read. The bounds, per folder: at most [`MAX_MODS`] folders are read (sorted by
//! name; the rest are one error), at most [`MAX_ENTRIES`] directory entries are looked at, and a manifest
//! over 64 KB is refused before it is parsed.
//!
//! The folder check: the folder name must be the manifest's id (lower-case, so a Windows folder cannot
//! be two mods), the folder must canonicalise under its canonical root (a junction out is refused), the
//! manifest must resolve inside the folder, and every file the manifest names (the panel, the background page,
//! the bar, the helper) must resolve inside it too. The ids of both folders are one space: `mods\` is read
//! first, and an id it holds is refused in `plugins\` (design section 13).

use crate::manifest::{
    Kind, MANIFEST_FILE, MAX_MANIFEST_BYTES, Manifest, PLUGIN_FILE, parse_as, valid_id,
};
use crate::paths;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Mod folders read in one scan of one root.
pub const MAX_MODS: usize = 64;
/// Directory entries looked at in one scan of one root.
pub const MAX_ENTRIES: usize = 4096;

/// Which of the two folders something was found in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Root {
    /// `<profile>\mods`, today's folder.
    #[default]
    Mods,
    /// `<profile>\plugins`, the second root, read after `mods`.
    Plugins,
}

impl Root {
    /// The folder as the refusals and Settings name it.
    pub fn words(self) -> &'static str {
        match self {
            Root::Mods => "the mods folder",
            Root::Plugins => "the plugins folder",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Root::Mods => "mod",
            Root::Plugins => "plugin",
        }
    }
}

/// A mod that passed every check.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub manifest: Manifest,
    /// The canonical mod folder.
    pub folder: PathBuf,
    /// The folder it was found in.
    pub root: Root,
    /// The canonical panel file, when the mod has a panel.
    pub entry: Option<PathBuf>,
    /// The canonical background page, when a plugin has one.
    pub background: Option<PathBuf>,
    /// The canonical bar page, when a plugin has a bar.
    pub bar: Option<PathBuf>,
    /// The canonical helper program, when a plugin has one.
    pub helper: Option<PathBuf>,
}

/// A folder that was not read as a mod, and why (one sentence for Settings > Plugins).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderError {
    /// The folder's name, or empty for a problem with the root folder itself.
    pub folder: String,
    pub reason: String,
    /// The root it is in.
    pub root: Root,
}

/// The result of one scan: the valid mods (sorted by id) and the folders that were refused.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Discovery {
    pub mods: Vec<Found>,
    pub errors: Vec<FolderError>,
}

/// Scans one mods folder `root`. A missing `root` is simply no mods.
pub fn discover(root: &Path) -> Discovery {
    discover_all(root, None)
}

/// Scans the mods folder and then, when given, the plugins folder. A missing folder is simply nothing found in it.
pub fn discover_all(mods: &Path, plugins: Option<&Path>) -> Discovery {
    let mut out = Discovery::default();
    let mut taken = BTreeSet::new();
    scan_root(Root::Mods, mods, MAX_ENTRIES, &mut taken, &mut out);
    if let Some(plugins) = plugins {
        scan_root(Root::Plugins, plugins, MAX_ENTRIES, &mut taken, &mut out);
    }
    out.mods.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    out
}

/// [`discover`] with the entry bound as a parameter, so a test can reach the bound with a few files.
#[cfg(test)]
fn discover_bounded(root: &Path, max_entries: usize) -> Discovery {
    let mut out = Discovery::default();
    scan_root(
        Root::Mods,
        root,
        max_entries,
        &mut BTreeSet::new(),
        &mut out,
    );
    out.mods.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    out
}

/// Reads one root into `out`. `taken` holds the ids an earlier root looked at; a folder named one of them is
/// refused here, and this root's own ids are added to it.
fn scan_root(
    which: Root,
    root: &Path,
    max_entries: usize,
    taken: &mut BTreeSet<String>,
    out: &mut Discovery,
) {
    let words = which.words();
    match std::fs::metadata(root) {
        // sync-ok: runs on the discovery worker
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return refuse(out, which, "", format!("{words} is not a folder"));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            return refuse(out, which, "", format!("{words} cannot be read: {e}"));
        }
    }
    let canon_root = match std::fs::canonicalize(root) {
        // sync-ok: discovery worker
        Ok(p) => p,
        Err(e) => {
            return refuse(out, which, "", format!("{words} cannot be read: {e}"));
        }
    };
    let folders = folder_names(which, root, max_entries, out);
    // Only names that are legal mod ids count toward the cap, so a junk folder sorted first (a backup, a
    // `.git`) cannot push a real mod out of the scan. Each junk name is still reported.
    let (ids, junk): (Vec<String>, Vec<String>) = folders.into_iter().partition(|n| valid_id(n));
    for name in &junk {
        refuse(
            out,
            which,
            name,
            format!(
                "the folder name is not a {} id (3 to 40 characters: a-z, 0-9, '-')",
                which.noun()
            ),
        );
    }
    // One id space: a name the mods folder already holds is refused here, and does not count toward the cap.
    let (dup, ids): (Vec<String>, Vec<String>) = ids.into_iter().partition(|n| taken.contains(n));
    for name in &dup {
        refuse(
            out,
            which,
            name,
            format!(
                "the id \"{name}\" is already in {}; an id must be unique across mods and plugins",
                Root::Mods.words()
            ),
        );
    }
    let extra = ids.len().saturating_sub(MAX_MODS);
    if extra > 0 {
        refuse(
            out,
            which,
            "",
            format!("more than {MAX_MODS} folders; {extra} were not read"),
        );
    }
    for name in ids.iter().take(MAX_MODS) {
        load_one(which, &canon_root, root, name, out);
    }
    taken.extend(ids);
}

/// The names of the subfolders, sorted. Looks at most `max_entries` directory entries; one more than that is
/// an error, and the rest are not looked at.
fn folder_names(which: Root, root: &Path, max_entries: usize, out: &mut Discovery) -> Vec<String> {
    let entries = match std::fs::read_dir(root) {
        // sync-ok: discovery worker
        Ok(e) => e,
        Err(e) => {
            refuse(
                out,
                which,
                "",
                format!("{} cannot be listed: {e}", which.words()),
            );
            return Vec::new();
        }
    };
    let mut names = Vec::new();
    for (i, entry) in entries.enumerate() {
        if i == max_entries {
            refuse(
                out,
                which,
                "",
                format!("more than {max_entries} entries; the rest were not looked at"),
            );
            break;
        }
        if let Ok(e) = entry
            && e.path().is_dir()
        {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}

fn refuse(out: &mut Discovery, root: Root, folder: &str, reason: String) {
    out.errors.push(FolderError {
        folder: folder.to_string(),
        reason,
        root,
    });
}

fn load_one(which: Root, canon_root: &Path, root: &Path, name: &str, out: &mut Discovery) {
    let folder = match std::fs::canonicalize(root.join(name)) {
        // sync-ok: discovery worker
        Ok(p) => p,
        Err(e) => return refuse(out, which, name, format!("the folder cannot be read: {e}")),
    };
    if !folder.starts_with(canon_root) {
        return refuse(
            out,
            which,
            name,
            format!(
                "the folder is a link to somewhere outside {}",
                which.words()
            ),
        );
    }
    let kind = match manifest_kind(&folder) {
        Ok(k) => k,
        Err(reason) => return refuse(out, which, name, reason),
    };
    let text = match read_manifest(&folder, kind.file()) {
        Ok(t) => t,
        Err(reason) => return refuse(out, which, name, reason),
    };
    let manifest = match parse_as(kind, &text) {
        Ok(m) => m,
        Err(reason) => return refuse(out, which, name, reason),
    };
    if manifest.id != name {
        return refuse(
            out,
            which,
            name,
            format!(
                "the folder is named \"{name}\" but {} says id \"{}\"; the folder name must be the id",
                kind.file(),
                manifest.id
            ),
        );
    }
    // Every file the manifest names must be a real file inside the folder.
    let page = |rel: Option<&str>, field: &str| -> Result<Option<PathBuf>, String> {
        rel.map(|r| paths::inside(&folder, r).map_err(|reason| format!("{field} {reason}")))
            .transpose()
    };
    let files = (|| {
        let entry = page(
            manifest.panel.as_ref().map(|p| p.entry.as_str()),
            "panel.entry",
        )?;
        let background = page(manifest.background.as_deref(), "background")?;
        let bar = page(manifest.bar.as_ref().map(|b| b.entry.as_str()), "bar.entry")?;
        let helper = manifest
            .helper
            .as_ref()
            .map(|h| {
                paths::program_inside(&folder, &h.command)
                    .map_err(|reason| format!("helper.command {reason}"))
            })
            .transpose()?;
        Ok::<_, String>((entry, background, bar, helper))
    })();
    let (entry, background, bar, helper) = match files {
        Ok(f) => f,
        Err(reason) => return refuse(out, which, name, reason),
    };
    out.mods.push(Found {
        manifest,
        folder,
        root: which,
        entry,
        background,
        bar,
        helper,
    });
}

/// Which manifest the folder holds: exactly one of `plugin.json` and `mod.json`.
fn manifest_kind(folder: &Path) -> Result<Kind, String> {
    let has = |f: &str| std::fs::symlink_metadata(folder.join(f)).is_ok(); // sync-ok: discovery worker
    match (has(PLUGIN_FILE), has(MANIFEST_FILE)) {
        (true, true) => Err(format!(
            "the folder holds both {PLUGIN_FILE} and {MANIFEST_FILE}; keep the one for its protocol"
        )),
        (true, false) => Ok(Kind::Plugin),
        (false, true) => Ok(Kind::Mod),
        (false, false) => Err(format!(
            "there is no {MANIFEST_FILE} or {PLUGIN_FILE} in this folder"
        )),
    }
}

/// The text of the manifest `file` in a canonical mod folder: it must resolve inside the folder, be at most 64 KB
/// and be UTF-8.
fn read_manifest(folder: &Path, file: &str) -> Result<String, String> {
    let path = folder.join(file);
    let real = std::fs::canonicalize(&path) // sync-ok: discovery worker
        .map_err(|_| format!("{file} cannot be opened"))?;
    if !real.starts_with(folder) {
        return Err(format!("{file} is a link to a file outside the mod folder"));
    }
    let mut raw = Vec::new();
    std::fs::File::open(&real) // sync-ok: discovery worker
        .and_then(|f| f.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut raw))
        .map_err(|e| format!("{file} cannot be read: {e}"))?;
    if raw.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{file} is larger than {} KB",
            MAX_MANIFEST_BYTES / 1024
        ));
    }
    String::from_utf8(raw).map_err(|_| format!("{file} is not UTF-8 text"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const GOOD: &str = r#"{
        "id": "notes", "name": "Notes", "version": "1.0.0", "protocol": 1,
        "description": "A notes panel.", "permissions": ["panel", "tools"],
        "panel": {"entry": "index.html", "title": "Notes"},
        "tools": [{"name": "append_note", "description": "Adds a line.",
                   "input_schema": {"type": "object"}}]
    }"#;

    /// A mods folder with the named mod folders, each made by `make`.
    fn root_with(mods: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, manifest) in mods {
            let folder = dir.path().join(name);
            fs::create_dir_all(&folder).unwrap(); // sync-ok: test code only
            fs::write(folder.join("index.html"), "<p>hi</p>").unwrap(); // sync-ok: test code only
            fs::write(folder.join("mod.json"), manifest).unwrap(); // sync-ok: test code only
        }
        dir
    }

    fn reasons(d: &Discovery) -> Vec<(String, String)> {
        d.errors
            .iter()
            .map(|e| (e.folder.clone(), e.reason.clone()))
            .collect()
    }

    #[test]
    fn a_good_mod_is_found_with_its_panel_file() {
        let dir = root_with(&[("notes", GOOD)]);
        let d = discover(dir.path());
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        assert_eq!(d.mods.len(), 1);
        let m = &d.mods[0];
        assert_eq!(m.manifest.id, "notes");
        assert!(m.entry.as_ref().unwrap().ends_with("index.html"));
    }

    #[test]
    fn a_missing_root_is_no_mods_and_no_errors() {
        let dir = tempfile::tempdir().unwrap();
        let d = discover(&dir.path().join("absent"));
        assert_eq!(d, Discovery::default());
    }

    #[test]
    fn a_root_that_is_a_file_is_one_error() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("file");
        fs::write(&f, "x").unwrap(); // sync-ok: test code only
        let d = discover(&f);
        assert!(d.mods.is_empty());
        assert_eq!(d.errors.len(), 1);
        assert!(d.errors[0].reason.contains("not a folder"));
    }

    #[test]
    fn a_bad_mod_is_an_error_and_the_good_ones_still_load() {
        let bad = GOOD.replace("\"protocol\": 1", "\"protocol\": 9");
        let dir = root_with(&[("notes", GOOD), ("broke", &bad)]);
        let d = discover(dir.path());
        assert_eq!(d.mods.len(), 1);
        assert_eq!(d.mods[0].manifest.id, "notes");
        let r = reasons(&d);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "broke");
        assert!(r[0].1.contains("protocol must be the number 1"));
    }

    #[test]
    fn the_folder_name_must_be_the_id() {
        let dir = root_with(&[("other", GOOD)]);
        let d = discover(dir.path());
        assert!(d.mods.is_empty());
        assert!(
            d.errors[0]
                .reason
                .contains("the folder name must be the id"),
            "{:?}",
            d.errors
        );
    }

    #[test]
    fn a_folder_that_is_not_an_id_is_refused_by_name() {
        let dir = root_with(&[("Bad_Name", GOOD)]);
        let d = discover(dir.path());
        assert!(
            d.errors[0].reason.contains("not a mod id"),
            "{:?}",
            d.errors
        );
    }

    #[test]
    fn a_folder_without_a_manifest_says_so() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("empty-one")).unwrap(); // sync-ok: test code only
        let d = discover(dir.path());
        assert!(d.errors[0].reason.contains("no mod.json"), "{:?}", d.errors);
    }

    #[test]
    fn a_manifest_over_sixty_four_kilobytes_is_refused_before_parsing() {
        let big = format!("{{\"id\":\"notes\",\"pad\":\"{}\"}}", "x".repeat(70 * 1024));
        let dir = root_with(&[("notes", &big)]);
        let d = discover(dir.path());
        assert!(
            d.errors[0].reason.contains("larger than 64 KB"),
            "{:?}",
            d.errors
        );
    }

    #[test]
    fn a_manifest_that_is_not_utf8_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("notes");
        fs::create_dir(&folder).unwrap(); // sync-ok: test code only
        fs::write(folder.join("mod.json"), [0xff, 0xfe, 0x7b]).unwrap(); // sync-ok: test code only
        let d = discover(dir.path());
        assert!(d.errors[0].reason.contains("not UTF-8"), "{:?}", d.errors);
    }

    #[test]
    fn a_missing_panel_file_is_refused() {
        let dir = root_with(&[("notes", GOOD)]);
        fs::remove_file(dir.path().join("notes").join("index.html")).unwrap(); // sync-ok: test code only
        let d = discover(dir.path());
        assert!(d.mods.is_empty());
        assert!(
            d.errors[0].reason.contains("does not exist"),
            "{:?}",
            d.errors
        );
    }

    #[test]
    fn a_panel_entry_that_climbs_out_is_refused_by_the_manifest() {
        let escape = GOOD.replace("\"index.html\"", "\"../index.html\"");
        let dir = root_with(&[("notes", &escape)]);
        let d = discover(dir.path());
        assert!(d.errors[0].reason.contains("'..'"), "{:?}", d.errors);
    }

    #[test]
    fn more_than_sixty_four_folders_are_bounded_and_the_rest_named() {
        let names: Vec<String> = (0..65).map(|i| format!("mod-{i:03}")).collect();
        let mods: Vec<(&str, String)> = names
            .iter()
            .map(|n| (n.as_str(), GOOD.replace("\"notes\"", &format!("\"{n}\""))))
            .collect();
        let refs: Vec<(&str, &str)> = mods.iter().map(|(n, m)| (*n, m.as_str())).collect();
        let dir = root_with(&refs);
        let d = discover(dir.path());
        assert_eq!(d.mods.len(), MAX_MODS);
        assert!(
            d.errors
                .iter()
                .any(|e| e.folder.is_empty() && e.reason.contains("1 were not read")),
            "{:?}",
            d.errors
        );
    }

    #[test]
    fn a_junction_out_of_the_mods_folder_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("mod.json"), GOOD).unwrap(); // sync-ok: test code only
        fs::write(outside.path().join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("notes");
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let made = std::process::Command::new(format!(r"{sys}\System32\cmd.exe"))
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output();
        if !matches!(made, Ok(ref o) if o.status.success()) {
            eprintln!("skipped: could not make a junction");
            return;
        }
        let d = discover(dir.path());
        assert!(d.mods.is_empty());
        assert!(
            d.errors[0].reason.contains("outside the mods folder"),
            "{:?}",
            d.errors
        );
    }

    #[test]
    fn a_manifest_file_that_links_outside_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        let real = outside.path().join("mod.json");
        fs::write(&real, GOOD).unwrap(); // sync-ok: test code only
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("notes");
        fs::create_dir(&folder).unwrap(); // sync-ok: test code only
        fs::write(folder.join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
        if std::os::windows::fs::symlink_file(&real, folder.join("mod.json")).is_err() {
            eprintln!("skipped: this account cannot make file symlinks");
            return;
        }
        let d = discover(dir.path());
        assert!(
            d.errors[0].reason.contains("outside the mod folder"),
            "{:?}",
            d.errors
        );
    }

    /// Junk folders sorted before a real mod (`Junk-00`, uppercase, sorts first) do not push it out of the cap,
    /// and each junk name is still reported.
    #[test]
    fn junk_folders_do_not_use_up_the_mod_cap() {
        let junk: Vec<String> = (0..70).map(|i| format!("Junk-{i:02}")).collect();
        let mut dirs: Vec<(String, String)> =
            junk.iter().map(|n| (n.clone(), GOOD.to_string())).collect();
        dirs.push(("zzz-mod".into(), GOOD.replace("\"notes\"", "\"zzz-mod\"")));
        let refs: Vec<(&str, &str)> = dirs.iter().map(|(n, m)| (n.as_str(), m.as_str())).collect();
        let dir = root_with(&refs);
        let d = discover(dir.path());
        assert_eq!(d.mods.len(), 1, "the real mod is found");
        assert_eq!(d.mods[0].manifest.id, "zzz-mod");
        assert_eq!(d.errors.len(), 70, "each junk folder is reported");
        assert!(d.errors.iter().all(|e| e.reason.contains("not a mod id")));
        assert!(!d.errors.iter().any(|e| e.reason.contains("were not read")));
    }

    /// The entry bound: one entry past the bound is an error and the rest are not looked at. The bound is a
    /// parameter here so the test does not make 4096 folders.
    #[test]
    fn entries_past_the_bound_are_reported_not_dropped_silently() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            fs::create_dir(dir.path().join(format!("mod-{i}"))).unwrap(); // sync-ok: test code only
        }
        let at_bound = discover_bounded(dir.path(), 5);
        assert!(
            !at_bound
                .errors
                .iter()
                .any(|e| e.reason.contains("not looked at")),
            "exactly the bound is not an overflow: {:?}",
            at_bound.errors
        );
        fs::write(dir.path().join("one-more.txt"), "x").unwrap(); // sync-ok: test code only
        let over = discover_bounded(dir.path(), 5);
        assert!(
            over.errors.iter().any(|e| e.folder.is_empty()
                && e.reason == "more than 5 entries; the rest were not looked at"),
            "{:?}",
            over.errors
        );
    }

    #[test]
    fn scans_are_sorted_by_id() {
        let other = GOOD.replace("\"notes\"", "\"zeta\"");
        let dir = root_with(&[("zeta", &other), ("notes", GOOD)]);
        let ids: Vec<String> = discover(dir.path())
            .mods
            .into_iter()
            .map(|m| m.manifest.id)
            .collect();
        assert_eq!(ids, ["notes", "zeta"]);
    }

    /// The sample mod in `docs/surfaces/sample-mod` is a valid mod: it is the one the mod author docs use.
    #[test]
    fn the_sample_mod_is_valid() {
        let sample = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/surfaces/sample-mod"
        ));
        let d = discover(sample);
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        assert_eq!(d.mods.len(), 1);
        let m = &d.mods[0].manifest;
        assert_eq!(m.id, "notes");
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.tools[0].name, "append_note");
        assert!(d.mods[0].entry.is_some());
        let page = fs::read_to_string(sample.join("notes").join("index.html")).unwrap(); // sync-ok: test code only
        assert!(
            page.contains("window.flare.on('tool'"),
            "the page answers tool calls through the bridge"
        );
        assert!(page.contains("window.flare.respond("));
        assert!(
            !page.contains("http://") && !page.contains("https://") && !page.contains("//cdn"),
            "the sample makes no network reference"
        );
    }

    // ---- the plugins folder and protocol 2 ----

    const PLUGIN: &str = r#"{
        "id": "todo", "name": "To-dos", "version": "1.0.0", "protocol": 2,
        "description": "Things to check.", "author": "someone",
        "permissions": ["panel", "bar", "process"],
        "background": "background.html",
        "panel": {"entry": "panel.html", "title": "To-dos"},
        "bar": {"entry": "ui/bar.html", "height": 40},
        "helper": {"command": "bin/helper.exe"}
    }"#;

    /// A plugin folder `name` under `root` with the files PLUGIN names.
    fn plugin_at(root: &Path, name: &str, manifest: &str) {
        let f = root.join(name);
        fs::create_dir_all(f.join("ui")).unwrap(); // sync-ok: test code only
        fs::create_dir_all(f.join("bin")).unwrap(); // sync-ok: test code only
        for page in ["background.html", "panel.html", "ui/bar.html"] {
            fs::write(f.join(page), "<p>x</p>").unwrap(); // sync-ok: test code only
        }
        fs::write(f.join("bin").join("helper.exe"), "MZ").unwrap(); // sync-ok: test code only
        fs::write(f.join("plugin.json"), manifest).unwrap(); // sync-ok: test code only
    }

    #[test]
    fn a_plugin_in_the_plugins_folder_is_found_with_every_file_it_names() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("plugins");
        plugin_at(&plugins, "todo", PLUGIN);
        let d = discover_all(&dir.path().join("mods"), Some(&plugins));
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        let f = &d.mods[0];
        assert_eq!(f.root, Root::Plugins);
        assert!(f.manifest.is_plugin());
        assert!(f.entry.as_ref().unwrap().ends_with("panel.html"));
        assert!(f.background.as_ref().unwrap().ends_with("background.html"));
        assert!(f.bar.as_ref().unwrap().ends_with("bar.html"));
        assert!(f.helper.as_ref().unwrap().ends_with("helper.exe"));
    }

    #[test]
    fn both_roots_are_read_and_the_result_is_sorted_by_id() {
        let mods = root_with(&[("notes", GOOD)]);
        let plugins = tempfile::tempdir().unwrap();
        plugin_at(plugins.path(), "todo", PLUGIN);
        let early = PLUGIN.replace("\"todo\"", "\"alpha\"");
        plugin_at(plugins.path(), "alpha", &early);
        let d = discover_all(mods.path(), Some(plugins.path()));
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        let got: Vec<(&str, Root)> = d
            .mods
            .iter()
            .map(|f| (f.manifest.id.as_str(), f.root))
            .collect();
        assert_eq!(
            got,
            [
                ("alpha", Root::Plugins),
                ("notes", Root::Mods),
                ("todo", Root::Plugins)
            ]
        );
    }

    #[test]
    fn an_id_in_both_folders_is_refused_in_the_plugins_folder() {
        let mods = root_with(&[("notes", GOOD)]);
        let plugins = tempfile::tempdir().unwrap();
        plugin_at(
            plugins.path(),
            "notes",
            &PLUGIN.replace("\"todo\"", "\"notes\""),
        );
        let d = discover_all(mods.path(), Some(plugins.path()));
        assert_eq!(d.mods.len(), 1);
        assert_eq!(d.mods[0].root, Root::Mods, "the mods folder wins");
        assert_eq!(d.errors.len(), 1);
        assert_eq!(d.errors[0].root, Root::Plugins);
        assert_eq!(d.errors[0].folder, "notes");
        assert!(
            d.errors[0]
                .reason
                .contains("unique across mods and plugins"),
            "{:?}",
            d.errors
        );
        // A broken folder in the mods folder still holds its id.
        fs::write(mods.path().join("notes").join("mod.json"), "{").unwrap(); // sync-ok: test code only
        let d = discover_all(mods.path(), Some(plugins.path()));
        assert!(d.mods.is_empty());
        assert_eq!(d.errors.len(), 2, "{:?}", d.errors);
    }

    #[test]
    fn either_manifest_is_read_in_either_folder_and_both_at_once_is_refused() {
        let plugins = tempfile::tempdir().unwrap();
        // A protocol 1 mod dropped in the plugins folder by hand is still a mod.
        let f = plugins.path().join("notes");
        fs::create_dir_all(&f).unwrap(); // sync-ok: test code only
        fs::write(f.join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
        fs::write(f.join("mod.json"), GOOD).unwrap(); // sync-ok: test code only
        let mods = tempfile::tempdir().unwrap();
        plugin_at(mods.path(), "todo", PLUGIN);
        let d = discover_all(mods.path(), Some(plugins.path()));
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        assert_eq!(d.mods.len(), 2);
        assert!(!d.mods[0].manifest.is_plugin() && d.mods[0].root == Root::Plugins);
        assert!(d.mods[1].manifest.is_plugin() && d.mods[1].root == Root::Mods);
        fs::write(f.join("plugin.json"), PLUGIN).unwrap(); // sync-ok: test code only
        let d = discover_all(mods.path(), Some(plugins.path()));
        assert!(
            d.errors[0].reason.contains("both plugin.json and mod.json"),
            "{:?}",
            d.errors
        );
    }

    #[test]
    fn a_plugin_json_must_say_protocol_2_and_its_refusals_name_plugin_json() {
        let plugins = tempfile::tempdir().unwrap();
        plugin_at(
            plugins.path(),
            "todo",
            &PLUGIN.replace("\"protocol\": 2", "\"protocol\": 1"),
        );
        let d = discover_all(&plugins.path().join("none"), Some(plugins.path()));
        assert!(
            d.errors[0]
                .reason
                .contains("protocol must be the number 2 in plugin.json"),
            "{:?}",
            d.errors
        );
        plugin_at(
            plugins.path(),
            "todo",
            &PLUGIN.replace("\"todo\"", "\"other\""),
        );
        let d = discover_all(&plugins.path().join("none"), Some(plugins.path()));
        assert!(
            d.errors[0].reason.contains("plugin.json says id \"other\""),
            "{:?}",
            d.errors
        );
    }

    #[test]
    fn every_file_a_plugin_names_must_be_in_its_folder() {
        for (gone, want) in [
            ("background.html", "background does not exist"),
            ("ui/bar.html", "bar.entry does not exist"),
            ("bin/helper.exe", "helper.command does not exist"),
            ("panel.html", "panel.entry does not exist"),
        ] {
            let plugins = tempfile::tempdir().unwrap();
            plugin_at(plugins.path(), "todo", PLUGIN);
            fs::remove_file(plugins.path().join("todo").join(gone)).unwrap(); // sync-ok: test code only
            let d = discover_all(&plugins.path().join("none"), Some(plugins.path()));
            assert!(d.mods.is_empty(), "{gone}");
            assert!(d.errors[0].reason.contains(want), "{gone}: {:?}", d.errors);
        }
    }

    #[test]
    fn problems_of_the_plugins_folder_name_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plugins");
        fs::write(&file, "x").unwrap(); // sync-ok: test code only
        let d = discover_all(&dir.path().join("mods"), Some(&file));
        assert_eq!(d.errors[0].root, Root::Plugins);
        assert_eq!(d.errors[0].reason, "the plugins folder is not a folder");
        let plugins = tempfile::tempdir().unwrap();
        fs::create_dir(plugins.path().join("Bad_Name")).unwrap(); // sync-ok: test code only
        let d = discover_all(&dir.path().join("mods"), Some(plugins.path()));
        assert!(
            d.errors[0].reason.contains("not a plugin id"),
            "{:?}",
            d.errors
        );
        // A missing plugins folder is nothing found, as a missing mods folder is.
        let d = discover_all(&dir.path().join("mods"), Some(&dir.path().join("absent")));
        assert_eq!(d, Discovery::default());
    }

    /// Every folder in `sample-plugins/` is a valid protocol 2 plugin: they are the examples the plugin author docs use.
    #[test]
    fn the_sample_plugins_are_valid() {
        let samples = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../sample-plugins"));
        let d = discover_all(&samples.join("no-mods-here"), Some(samples));
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        assert!(!d.mods.is_empty());
        for f in &d.mods {
            assert!(f.manifest.is_plugin(), "{}", f.manifest.id);
            assert_eq!(f.root, Root::Plugins);
            for page in [&f.entry, &f.background, &f.bar].into_iter().flatten() {
                let text = fs::read_to_string(page).unwrap(); // sync-ok: test code only
                assert!(
                    !text.contains("http://") && !text.contains("//cdn"),
                    "{} makes no network reference",
                    page.display()
                );
            }
        }
        let tour = d
            .mods
            .iter()
            .find(|f| f.manifest.id == "manifest-tour")
            .expect("the manifest tour");
        let m = &tour.manifest;
        assert!(m.author.is_some() && m.homepage.is_some());
        assert!(tour.background.is_some() && tour.bar.is_some() && tour.entry.is_some());
        assert!(!m.contributes.sidebar.is_empty());
        assert!(!m.contributes.actions.is_empty());
        assert!(!m.contributes.icons.is_empty());
        assert_eq!(m.tools[0].name, "add_item");
    }
}
