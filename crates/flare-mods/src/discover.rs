//! The scan of the mods folder: `<root>\<id>\mod.json`.
//!
//! Worker-callable: it reads the disk, so the app runs it on a worker and posts the answer back. It
//! never fails as a whole. A folder that is not a mod becomes one [`FolderError`] with its reason, and
//! every other folder is still read. The bounds: at most [`MAX_MODS`] folders are read (sorted by name;
//! the rest are one error), at most [`MAX_ENTRIES`] directory entries are looked at, and a `mod.json`
//! over 64 KB is refused before it is parsed.
//!
//! The folder check: the folder name must be the manifest's id (lower-case, so a Windows folder cannot
//! be two mods), the folder must canonicalise under the canonical mods folder (a junction out is
//! refused), `mod.json` must resolve inside the folder, and the panel entry must resolve inside it too.

use crate::manifest::{MANIFEST_FILE, MAX_MANIFEST_BYTES, Manifest, valid_id};
use crate::paths;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Mod folders read in one scan.
pub const MAX_MODS: usize = 64;
/// Directory entries looked at in one scan.
pub const MAX_ENTRIES: usize = 4096;

/// A mod that passed every check.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub manifest: Manifest,
    /// The canonical mod folder.
    pub folder: PathBuf,
    /// The canonical panel file, when the mod has a panel.
    pub entry: Option<PathBuf>,
}

/// A folder that was not read as a mod, and why (one sentence for Settings > Mods).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderError {
    /// The folder's name, or empty for a problem with the mods folder itself.
    pub folder: String,
    pub reason: String,
}

/// The result of one scan: the valid mods (sorted by id) and the folders that were refused.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Discovery {
    pub mods: Vec<Found>,
    pub errors: Vec<FolderError>,
}

/// Scans `root` for mods. A missing `root` is simply no mods.
pub fn discover(root: &Path) -> Discovery {
    discover_bounded(root, MAX_ENTRIES)
}

/// [`discover`] with the entry bound as a parameter, so a test can reach the bound with a few files.
fn discover_bounded(root: &Path, max_entries: usize) -> Discovery {
    let mut out = Discovery::default();
    match std::fs::metadata(root) {
        // sync-ok: runs on the discovery worker
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            refuse(&mut out, "", "the mods folder is not a folder".into());
            return out;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            refuse(&mut out, "", format!("the mods folder cannot be read: {e}"));
            return out;
        }
    }
    let canon_root = match std::fs::canonicalize(root) {
        // sync-ok: discovery worker
        Ok(p) => p,
        Err(e) => {
            refuse(&mut out, "", format!("the mods folder cannot be read: {e}"));
            return out;
        }
    };
    let folders = folder_names(root, max_entries, &mut out);
    // Only names that are legal mod ids count toward the cap, so a junk folder sorted first (a backup, a
    // `.git`) cannot push a real mod out of the scan. Each junk name is still reported.
    let (ids, junk): (Vec<String>, Vec<String>) = folders.into_iter().partition(|n| valid_id(n));
    for name in &junk {
        refuse(
            &mut out,
            name,
            "the folder name is not a mod id (3 to 40 characters: a-z, 0-9, '-')".into(),
        );
    }
    let extra = ids.len().saturating_sub(MAX_MODS);
    if extra > 0 {
        refuse(
            &mut out,
            "",
            format!("more than {MAX_MODS} folders; {extra} were not read"),
        );
    }
    for name in ids.iter().take(MAX_MODS) {
        load_one(&canon_root, root, name, &mut out);
    }
    out.mods.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    out
}

/// The names of the subfolders, sorted. Looks at most `max_entries` directory entries; one more than that is
/// an error, and the rest are not looked at.
fn folder_names(root: &Path, max_entries: usize, out: &mut Discovery) -> Vec<String> {
    let entries = match std::fs::read_dir(root) {
        // sync-ok: discovery worker
        Ok(e) => e,
        Err(e) => {
            refuse(out, "", format!("the mods folder cannot be listed: {e}"));
            return Vec::new();
        }
    };
    let mut names = Vec::new();
    for (i, entry) in entries.enumerate() {
        if i == max_entries {
            refuse(
                out,
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

fn refuse(out: &mut Discovery, folder: &str, reason: String) {
    out.errors.push(FolderError {
        folder: folder.to_string(),
        reason,
    });
}

fn load_one(canon_root: &Path, root: &Path, name: &str, out: &mut Discovery) {
    let folder = match std::fs::canonicalize(root.join(name)) {
        // sync-ok: discovery worker
        Ok(p) => p,
        Err(e) => return refuse(out, name, format!("the folder cannot be read: {e}")),
    };
    if !folder.starts_with(canon_root) {
        return refuse(
            out,
            name,
            "the folder is a link to somewhere outside the mods folder".into(),
        );
    }
    let text = match read_manifest(&folder) {
        Ok(t) => t,
        Err(reason) => return refuse(out, name, reason),
    };
    let manifest = match crate::manifest::parse(&text) {
        Ok(m) => m,
        Err(reason) => return refuse(out, name, reason),
    };
    if manifest.id != name {
        return refuse(
            out,
            name,
            format!(
                "the folder is named \"{name}\" but mod.json says id \"{}\"; the folder name must be the id",
                manifest.id
            ),
        );
    }
    let entry = match &manifest.panel {
        None => None,
        Some(p) => match paths::inside(&folder, &p.entry) {
            Ok(path) => Some(path),
            Err(reason) => {
                return refuse(out, name, format!("panel.entry {reason}"));
            }
        },
    };
    out.mods.push(Found {
        manifest,
        folder,
        entry,
    });
}

/// The text of `mod.json` in a canonical mod folder: it must resolve inside the folder, be at most 64 KB
/// and be UTF-8.
fn read_manifest(folder: &Path) -> Result<String, String> {
    let file = folder.join(MANIFEST_FILE);
    let real = std::fs::canonicalize(&file) // sync-ok: discovery worker
        .map_err(|_| "there is no mod.json in this folder".to_string())?;
    if !real.starts_with(folder) {
        return Err("mod.json is a link to a file outside the mod folder".into());
    }
    let mut raw = Vec::new();
    std::fs::File::open(&real) // sync-ok: discovery worker
        .and_then(|f| f.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut raw))
        .map_err(|e| format!("mod.json cannot be read: {e}"))?;
    if raw.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!(
            "mod.json is larger than {} KB",
            MAX_MANIFEST_BYTES / 1024
        ));
    }
    String::from_utf8(raw).map_err(|_| "mod.json is not UTF-8 text".to_string())
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
}
