//! Install, update and remove a plugin (plugins phase 7, design 9 and 13), the disk part: no UI, no network.
//!
//! - **Zip rules.** Each entry is written by Flare itself with create-new, never by a library's extract. Refused, with
//!   a sentence: links (a unix link mode or a Windows reparse attribute), encrypted entries, absolute and drive paths,
//!   `:`, `\`, control characters and `<>"|?*`, `.` and `..` parts, parts that end in a dot or a space, Windows device
//!   names with any extension (`CON`, `nul.txt`, `COM1`), names that differ only in case, paths over
//!   [`MAX_PATH_CHARS`] characters or more than [`MAX_DEPTH`] folders deep. The real decompressed bytes are counted
//!   while they are written: [`MAX_ENTRY_BYTES`] an entry, [`MAX_TOTAL_BYTES`] in all, [`MAX_ZIP_BYTES`] for the zip
//!   and [`MAX_ENTRIES`] entries.
//! - **Staging.** A zip is extracted to `plugins\.staging\<random>`, never straight into the plugins folder. Its layout
//!   is `plugin.json` (or `mod.json`) at the top, or exactly one top folder holding it. The manifest and every file it
//!   names are checked on the staged copy (`discover::check_folder`), and the copy's content hash is taken, before
//!   anything else happens. The scan never reads `.staging`, and its leftovers are cleared at start.
//! - **Commit.** The staged folder replaces `plugins\<id>` by rename: the old folder is moved aside first and moved
//!   back if the new one cannot take its place, so the plugins folder never holds half a plugin. The old copy then goes
//!   to the Recycle Bin.
//! - **Remove.** The folder goes to the Recycle Bin (a link is unlinked, never followed).
//!
//! Every call here reads or writes the disk: the app runs them on a worker, one at a time (the hub's disk lock).

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::discover::{Found, check_folder};
use crate::manifest::{MANIFEST_FILE, PLUGIN_FILE, valid_id};

/// The largest zip read (its size on disk, or the bytes downloaded).
pub const MAX_ZIP_BYTES: u64 = 50 * 1024 * 1024;
/// Entries a zip may hold.
pub const MAX_ENTRIES: usize = 2000;
/// Decompressed bytes of one entry.
pub const MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
/// Decompressed bytes of the whole zip.
pub const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024;
/// The longest entry path, in characters.
pub const MAX_PATH_CHARS: usize = 200;
/// Folders an entry may sit in, counted from the zip's top.
pub const MAX_DEPTH: usize = 8;
/// Install's working folder inside the plugins folder.
pub const STAGING: &str = ".staging";

/// The Windows reparse-point attribute, in a zip entry's external attributes (a link or a junction).
const REPARSE_ATTRIBUTE: u32 = 0x400;
/// Characters no Windows file name may hold (`:` and `\` are refused earlier with their own reason).
const BAD_CHARS: [char; 6] = ['<', '>', '"', '|', '?', '*'];

/// The parts of a zip entry's name, or why the entry is refused. `name` uses `/` between parts; a trailing `/` marks a
/// folder entry. A refusal shows the name with each control or hidden formatting character as `?` (Settings shows the
/// sentence), and a name that passes holds none, so the later sentences that name it are clean too.
pub fn check_entry(name: &str) -> Result<Vec<String>, String> {
    let hidden = |c: char| c.is_control() || crate::manifest::is_format_char(c);
    let shown: String = name
        .chars()
        .take(60)
        .map(|c| if hidden(c) { '?' } else { c })
        .collect();
    let refuse = |why: &str| Err(format!("the zip entry \"{shown}\" {why}"));
    if name.is_empty() {
        return refuse("has no name");
    }
    if name.chars().any(char::is_control) {
        return refuse("has a control character in its name");
    }
    if name.chars().any(crate::manifest::is_format_char) {
        return refuse(
            "has a hidden formatting character in its name (a zero-width space or a bidi override)",
        );
    }
    if name.contains('\\') {
        return refuse("uses '\\' in its name");
    }
    if name.contains(':') {
        return refuse("names a drive or a stream (':')");
    }
    if name.starts_with('/') {
        return refuse("is an absolute path");
    }
    if name.contains(BAD_CHARS) {
        return refuse("has a character Windows does not allow in a name");
    }
    if name.chars().count() > MAX_PATH_CHARS {
        return refuse(&format!("is longer than {MAX_PATH_CHARS} characters"));
    }
    let body = name.strip_suffix('/').unwrap_or(name);
    let mut parts = Vec::new();
    for part in body.split('/') {
        if part.is_empty() || part == "." {
            return refuse("has an empty or '.' part");
        }
        if part == ".." {
            return refuse("goes up out of the plugin with '..'");
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return refuse("has a part that ends in a dot or a space");
        }
        if is_device_name(part) {
            return refuse("uses a Windows device name");
        }
        parts.push(part.to_string());
    }
    let folders = if name.ends_with('/') {
        parts.len()
    } else {
        parts.len() - 1
    };
    if folders > MAX_DEPTH {
        return refuse(&format!("is more than {MAX_DEPTH} folders deep"));
    }
    Ok(parts)
}

/// A Windows device name, whatever extension follows (`CON`, `nul.txt`, `Com1.log`, `COM¹`).
fn is_device_name(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .unwrap_or(part)
        .trim_end()
        .to_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) {
        return true;
    }
    let mut chars = stem.chars();
    let head: String = chars.by_ref().take(3).collect();
    let tail: Vec<char> = chars.collect();
    (head == "COM" || head == "LPT")
        && tail.len() == 1
        && (tail[0].is_ascii_digit() || matches!(tail[0], '\u{b9}' | '\u{b2}' | '\u{b3}'))
}

/// What an extraction wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extracted {
    pub files: usize,
    pub bytes: u64,
}

/// Writes the entries of `zip` under `out` (an empty folder Flare made), each checked by the rules of the module doc.
/// On an error what was written stays under `out` for the caller to delete with it.
pub fn extract(zip: &Path, out: &Path) -> Result<Extracted, String> {
    let size = fs::metadata(zip) // sync-ok: install runs on a worker
        .map_err(|e| format!("the zip cannot be read: {e}"))?
        .len();
    if size > MAX_ZIP_BYTES {
        return Err(format!(
            "the zip is larger than {} MB",
            MAX_ZIP_BYTES / (1024 * 1024)
        ));
    }
    let file = File::open(zip).map_err(|e| format!("the zip cannot be read: {e}"))?; // sync-ok: install worker
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("the file is not a zip Flare reads: {e}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(format!("the zip holds more than {MAX_ENTRIES} entries"));
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut done = Extracted::default();
    for i in 0..archive.len() {
        // The entry's facts are read without decompressing it first.
        let (parts, is_dir, declared) = {
            let raw = archive
                .by_index_raw(i)
                .map_err(|e| format!("the zip is damaged: {e}"))?;
            let name = raw
                .name()
                .map_err(|_| "a zip entry's name is not readable text".to_string())?
                .into_owned();
            let parts = check_entry(&name)?;
            if raw.encrypted() {
                return Err(format!("the zip entry \"{name}\" is encrypted"));
            }
            if raw.is_symlink() || raw.external_attributes() & REPARSE_ATTRIBUTE != 0 {
                return Err(format!("the zip entry \"{name}\" is a link"));
            }
            (parts, raw.is_dir(), raw.size())
        };
        let key = parts.join("/").to_lowercase();
        if !seen.insert(key) {
            return Err(format!(
                "the zip holds \"{}\" twice (names that differ only in case count as one)",
                parts.join("/")
            ));
        }
        let target = parts.iter().fold(out.to_path_buf(), |p, part| p.join(part));
        if is_dir {
            fs::create_dir_all(&target) // sync-ok: install worker
                .map_err(|e| format!("a folder of the zip cannot be made: {e}"))?;
            continue;
        }
        if declared > MAX_ENTRY_BYTES {
            return Err(entry_too_large(&parts));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent) // sync-ok: install worker
                .map_err(|e| format!("a folder of the zip cannot be made: {e}"))?;
        }
        let mut member = archive
            .by_index(i)
            .map_err(|e| format!("the zip entry \"{}\" cannot be read: {e}", parts.join("/")))?;
        let mut dest = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target) // sync-ok: install worker
            .map_err(|e| format!("\"{}\" cannot be written: {e}", parts.join("/")))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = member.read(&mut buf).map_err(|e| {
                format!("the zip entry \"{}\" cannot be read: {e}", parts.join("/"))
            })?;
            if n == 0 {
                break;
            }
            written += n as u64;
            done.bytes += n as u64;
            if written > MAX_ENTRY_BYTES {
                return Err(entry_too_large(&parts));
            }
            if done.bytes > MAX_TOTAL_BYTES {
                return Err(format!(
                    "the zip unpacks to more than {} MB",
                    MAX_TOTAL_BYTES / (1024 * 1024)
                ));
            }
            dest.write_all(&buf[..n])
                .map_err(|e| format!("\"{}\" cannot be written: {e}", parts.join("/")))?;
        }
        done.files += 1;
    }
    Ok(done)
}

fn entry_too_large(parts: &[String]) -> String {
    format!(
        "the zip entry \"{}\" unpacks to more than {} MB",
        parts.join("/"),
        MAX_ENTRY_BYTES / (1024 * 1024)
    )
}

/// The folder of an unpacked zip that holds the manifest: the top itself, or its one top folder.
fn plugin_root(unpacked: &Path) -> Result<PathBuf, String> {
    let has_manifest = |d: &Path| {
        [PLUGIN_FILE, MANIFEST_FILE]
            .iter()
            .any(|f| fs::symlink_metadata(d.join(f)).is_ok()) // sync-ok: install worker
    };
    if has_manifest(unpacked) {
        return Ok(unpacked.to_path_buf());
    }
    let entries: Vec<fs::DirEntry> =
        fs::read_dir(unpacked) // sync-ok: install worker
            .map_err(|e| format!("the unpacked zip cannot be listed: {e}"))?
            .flatten()
            .collect();
    if let [one] = entries.as_slice()
        && one.file_type().is_ok_and(|t| t.is_dir())
        && has_manifest(&one.path())
    {
        return Ok(one.path());
    }
    Err(format!(
        "the zip must hold {PLUGIN_FILE} at its top, or one folder that holds it"
    ))
}

/// A plugin unpacked and checked in `plugins\.staging\<random>`, not installed yet. Dropping it deletes the staging
/// folder (whatever is still in it).
#[derive(Debug)]
pub struct Staged {
    /// `plugins\.staging\<random>`.
    root: PathBuf,
    /// The checked plugin, in `<root>\<id>`.
    pub found: Found,
    /// The content hash of the staged folder (`crate::hash`).
    pub hash: String,
}

impl Staged {
    pub fn id(&self) -> &str {
        &self.found.manifest.id
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root); // sync-ok: dropped on the install worker
    }
}

/// Unpacks `zip` into a new staging folder of `plugins` and checks the plugin it holds: the manifest, every file it
/// names and the layout. Nothing reaches the plugins folder itself.
pub fn stage(zip: &Path, plugins: &Path) -> Result<Staged, String> {
    let staging = plugins.join(STAGING);
    fs::create_dir_all(&staging) // sync-ok: install worker
        .map_err(|e| format!("the plugins folder cannot be written: {e}"))?;
    let name: String = crate::tokens::new_token()?.chars().take(16).collect();
    let root = staging.join(name);
    fs::create_dir(&root) // sync-ok: install worker
        .map_err(|e| format!("the plugins folder cannot be written: {e}"))?;
    // From here the staging folder is deleted on any refusal (and, once staged, by the Staged).
    let mut guard = RootGuard(Some(root.clone()));
    let unpacked = root.join("zip");
    fs::create_dir(&unpacked) // sync-ok: install worker
        .map_err(|e| format!("the plugins folder cannot be written: {e}"))?;
    extract(zip, &unpacked)?;
    let top = plugin_root(&unpacked)?;
    let canon = fs::canonicalize(&top) // sync-ok: install worker
        .map_err(|e| format!("the unpacked zip cannot be read: {e}"))?;
    let id = check_folder(&canon)?.manifest.id;
    // The folder takes the id's name, as it will have in the plugins folder, and is checked again there.
    let named = root.join(&id);
    fs::rename(&top, &named) // sync-ok: install worker
        .map_err(|e| format!("the unpacked zip cannot be moved: {e}"))?;
    let canon = fs::canonicalize(&named) // sync-ok: install worker
        .map_err(|e| format!("the unpacked zip cannot be read: {e}"))?;
    let found = check_folder(&canon)?;
    let hash = crate::hash::folder_hash(&canon)?;
    guard.0 = None;
    Ok(Staged { root, found, hash })
}

/// Deletes a staging folder unless it was handed on.
struct RootGuard(Option<PathBuf>);

impl Drop for RootGuard {
    fn drop(&mut self) {
        if let Some(root) = &self.0 {
            let _ = fs::remove_dir_all(root); // sync-ok: dropped on the install worker
        }
    }
}

/// What a commit did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Committed {
    /// A folder of the same id was there and was replaced.
    pub replaced: bool,
}

/// Moves the staged plugin to `plugins\<id>`. A folder already there is moved aside first, put back if the new one
/// cannot take its place, and otherwise handed to `recycle` (the Recycle Bin in the app).
pub fn commit(
    staged: Staged,
    plugins: &Path,
    recycle: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<Committed, String> {
    let id = staged.id().to_string();
    if !valid_id(&id) {
        return Err(format!("\"{id}\" is not a plugin id"));
    }
    let target = plugins.join(&id);
    let aside = staged.root.join("replaced");
    let replaced = fs::symlink_metadata(&target).is_ok(); // sync-ok: install worker
    if replaced {
        fs::rename(&target, &aside) // sync-ok: install worker
            .map_err(|e| {
                format!(
                    "the installed copy of {id} cannot be moved aside (is a file of it open?): {e}"
                )
            })?;
    }
    if let Err(e) = fs::rename(&staged.found.folder, &target) {
        // sync-ok: install worker
        if replaced {
            let _ = fs::rename(&aside, &target); // sync-ok: install worker
        }
        return Err(format!("the new copy of {id} cannot be put in place: {e}"));
    }
    if replaced && let Err(why) = discard(&aside, recycle) {
        log::warn!("the replaced copy of plugin {id} was not removed: {why}");
    }
    Ok(Committed { replaced })
}

/// Removes the plugin folder `plugins\<id>`: to the Recycle Bin (`recycle`), or, for a link, the link alone. A folder
/// that is not there is nothing to do.
pub fn remove(
    plugins: &Path,
    id: &str,
    recycle: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if !valid_id(id) {
        return Err(format!("\"{id}\" is not a plugin id"));
    }
    let target = plugins.join(id);
    match fs::symlink_metadata(&target) {
        // sync-ok: install worker
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("the folder of {id} cannot be read: {e}")),
        Ok(_) => discard(&target, recycle),
    }
}

/// A folder to the Recycle Bin, or a link unlinked (never followed: recycling a junction could reach its target).
fn discard(path: &Path, recycle: &dyn Fn(&Path) -> Result<(), String>) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?; // sync-ok: install worker
    if is_link(&meta) {
        return fs::remove_dir(path) // sync-ok: install worker
            .or_else(|_| fs::remove_file(path)) // sync-ok: install worker
            .map_err(|e| format!("the link cannot be removed: {e}"));
    }
    recycle(path)
}

fn is_link(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_type().is_symlink() || meta.file_attributes() & REPARSE_ATTRIBUTE != 0
}

/// Deletes what an earlier run left in the staging folder (start, on the scan worker).
pub fn clean_staging(plugins: &Path) {
    let staging = plugins.join(STAGING);
    match fs::remove_dir_all(&staging) {
        // sync-ok: the scan worker
        Ok(()) => log::info!("cleared the plugins' staging folder"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("the plugins' staging folder was not cleared: {e}"),
    }
}

/// Moves `path` to the Recycle Bin, silently (no dialog, no progress). A failure is a sentence.
pub fn recycle(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    // The list of paths ends with two NULs.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    // SAFETY: `op` is fully initialised; `from` is a double-NUL-terminated wide string that outlives the call; no
    // window is named, so the shell shows nothing.
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted != 0 {
        return Err(format!(
            "it could not be moved to the Recycle Bin (code {code})"
        ));
    }
    Ok(())
}

/// Deletes `path` for good (a folder or a file); one that is not there is fine.
pub fn delete_path(path: &Path) -> Result<(), String> {
    let meta = match fs::symlink_metadata(path) {
        // sync-ok: install worker
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    if meta.is_dir() && !is_link(&meta) {
        fs::remove_dir_all(path).map_err(|e| e.to_string()) // sync-ok: install worker
    } else if meta.is_dir() {
        fs::remove_dir(path).map_err(|e| e.to_string()) // sync-ok: install worker
    } else {
        fs::remove_file(path).map_err(|e| e.to_string()) // sync-ok: install worker
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    const PLUGIN: &str = r#"{"id":"demo","name":"Demo","version":"1.0.0","protocol":2,"description":"A demo.",
        "permissions":["panel"],"panel":{"entry":"panel.html","title":"Demo"}}"#;

    /// A zip of `(name, bytes)` entries; a name ending in `/` is a folder.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cur = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cur);
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, bytes) in entries {
                if name.ends_with('/') {
                    w.add_directory(*name, opts).expect("a folder");
                } else {
                    w.start_file(*name, opts).expect("a file");
                    w.write_all(bytes).expect("its bytes");
                }
            }
            w.finish().expect("the zip");
        }
        cur.into_inner()
    }

    fn write_zip(dir: &Path, bytes: &[u8]) -> PathBuf {
        let p = dir.join("in.zip");
        fs::write(&p, bytes).expect("the zip file");
        p
    }

    fn noop_recycle(p: &Path) -> Result<(), String> {
        delete_path(p)
    }

    #[test]
    fn traversal_absolute_drive_and_odd_names_are_refused() {
        for (bad, why) in [
            ("../evil.txt", "'..'"),
            ("a/../../evil.txt", "'..'"),
            ("/etc/passwd", "absolute"),
            ("C:/Windows/x.dll", "':'"),
            ("a:stream", "':'"),
            ("a\\b.txt", "'\\'"),
            ("a//b.txt", "empty"),
            ("./a.txt", "'.'"),
            ("a\u{1}b", "control"),
            ("a\u{202e}b.txt", "hidden formatting"),
            ("a\u{200b}b.txt", "hidden formatting"),
            ("CON", "device"),
            ("sub/nul.txt", "device"),
            ("Com1.log", "device"),
            ("LPT\u{b9}", "device"),
            ("conin$", "device"),
            ("a./b", "dot or a space"),
            ("a /b", "dot or a space"),
            ("a?b", "does not allow"),
            ("", "no name"),
        ] {
            let e = check_entry(bad).expect_err(bad);
            assert!(e.contains(why), "{bad}: {e}");
        }
        // The sentence Settings shows holds no control or hidden character of the name.
        for (bad, shown) in [
            ("a\u{1b}[2Jb", "a?[2Jb"),
            ("a\nb", "a?b"),
            ("a\u{202e}b", "a?b"),
        ] {
            let e = check_entry(bad).expect_err(bad);
            assert!(e.contains(&format!("\"{shown}\"")), "{e:?}");
            let hidden = |c: char| c.is_control() || crate::manifest::is_format_char(c);
            assert!(!e.chars().any(hidden), "{e:?}");
        }
        let long = "a".repeat(MAX_PATH_CHARS + 1);
        assert!(check_entry(&long).unwrap_err().contains("longer"));
        let deep = format!("{}f.txt", "d/".repeat(MAX_DEPTH + 1));
        assert!(check_entry(&deep).unwrap_err().contains("deep"));
        let ok_deep = format!("{}f.txt", "d/".repeat(MAX_DEPTH));
        assert!(check_entry(&ok_deep).is_ok());
        for ok in [
            "plugin.json",
            "a/b/c.html",
            "folder/",
            "con-tab.js",
            "COM10.txt",
            "lpt.txt",
        ] {
            assert!(check_entry(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn a_zip_with_a_traversal_entry_writes_nothing_outside() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        let z = write_zip(
            dir.path(),
            &zip_of(&[("ok.txt", b"1"), ("../escape.txt", b"2")]),
        );
        let e = extract(&z, &out).unwrap_err();
        assert!(e.contains("'..'"), "{e}");
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn case_duplicates_links_and_encryption_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        let z = write_zip(dir.path(), &zip_of(&[("A.txt", b"1"), ("a.TXT", b"2")]));
        assert!(extract(&z, &out).unwrap_err().contains("twice"));

        // A unix link entry.
        let mut cur = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cur);
            w.add_symlink("link", "../../outside", SimpleFileOptions::default())
                .unwrap();
            w.finish().unwrap();
        }
        let out2 = dir.path().join("out2");
        fs::create_dir(&out2).unwrap();
        let z = write_zip(dir.path(), &cur.into_inner());
        assert!(extract(&z, &out2).unwrap_err().contains("is a link"));
    }

    #[test]
    fn the_size_and_count_limits_hold() {
        let dir = tempfile::tempdir().unwrap();
        // One entry past 16 MB (zeros compress small, so the zip itself is tiny).
        let big = vec![0u8; (MAX_ENTRY_BYTES + 1) as usize];
        let z = write_zip(dir.path(), &zip_of(&[("big.bin", &big)]));
        let out = dir.path().join("o1");
        fs::create_dir(&out).unwrap();
        assert!(extract(&z, &out).unwrap_err().contains("more than 16 MB"));
        // Seven entries of 15 MB pass each entry's limit and not the total.
        let fifteen = vec![0u8; 15 * 1024 * 1024];
        let names: Vec<String> = (0..7).map(|i| format!("f{i}.bin")).collect();
        let entries: Vec<(&str, &[u8])> = names
            .iter()
            .map(|n| (n.as_str(), fifteen.as_slice()))
            .collect();
        let z = write_zip(dir.path(), &zip_of(&entries));
        let out = dir.path().join("o2");
        fs::create_dir(&out).unwrap();
        assert!(extract(&z, &out).unwrap_err().contains("more than 100 MB"));
        // Too many entries.
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("f{i}")).collect();
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
        let z = write_zip(dir.path(), &zip_of(&entries));
        let out = dir.path().join("o3");
        fs::create_dir(&out).unwrap();
        assert!(extract(&z, &out).unwrap_err().contains("more than 2000"));
        // A file past 50 MB is not opened as a zip at all.
        let huge = dir.path().join("huge.zip");
        let f = File::create(&huge).unwrap();
        f.set_len(MAX_ZIP_BYTES + 1).unwrap();
        assert!(
            extract(&huge, &out)
                .unwrap_err()
                .contains("larger than 50 MB")
        );
        // Not a zip.
        let junk = dir.path().join("junk.zip");
        fs::write(&junk, b"not a zip").unwrap();
        assert!(extract(&junk, &out).unwrap_err().contains("not a zip"));
    }

    #[test]
    fn a_plugin_is_staged_flat_or_in_one_top_folder_and_checked_before_it_lands() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("plugins");
        for layout in [
            vec![
                ("plugin.json", PLUGIN.as_bytes()),
                ("panel.html", &b"<p>hi</p>"[..]),
            ],
            vec![
                ("demo-1.0/", &b""[..]),
                ("demo-1.0/plugin.json", PLUGIN.as_bytes()),
                ("demo-1.0/panel.html", &b"<p>hi</p>"[..]),
            ],
        ] {
            let z = write_zip(dir.path(), &zip_of(&layout));
            let s = stage(&z, &plugins).expect("staged");
            assert_eq!(s.id(), "demo");
            assert!(crate::hash::is_hash(&s.hash));
            assert!(
                !plugins.join("demo").exists(),
                "nothing lands before the commit"
            );
            let staging_root = s.root.clone();
            drop(s);
            assert!(!staging_root.exists(), "a dropped stage is deleted");
        }
        // A manifest that names a missing file is refused on the staged copy.
        let z = write_zip(dir.path(), &zip_of(&[("plugin.json", PLUGIN.as_bytes())]));
        assert!(stage(&z, &plugins).unwrap_err().contains("panel.entry"));
        // Two top folders, or none with a manifest.
        let z = write_zip(
            dir.path(),
            &zip_of(&[("a/plugin.json", PLUGIN.as_bytes()), ("b/x.txt", b"x")]),
        );
        assert!(stage(&z, &plugins).unwrap_err().contains("one folder"));
        // A manifest that is not valid.
        let z = write_zip(dir.path(), &zip_of(&[("plugin.json", b"{}")]));
        assert!(stage(&z, &plugins).is_err());
        // Nothing is left in the plugins folder but the (empty) staging folder.
        let left: Vec<_> = fs::read_dir(plugins.join(STAGING)).unwrap().collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn a_commit_replaces_the_folder_by_rename_and_hands_the_old_copy_over() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("plugins");
        let z = write_zip(
            dir.path(),
            &zip_of(&[("plugin.json", PLUGIN.as_bytes()), ("panel.html", b"v1")]),
        );
        let first = commit(stage(&z, &plugins).unwrap(), &plugins, &noop_recycle).unwrap();
        assert!(!first.replaced);
        assert_eq!(
            fs::read(plugins.join("demo").join("panel.html")).unwrap(),
            b"v1"
        );
        let z = write_zip(
            dir.path(),
            &zip_of(&[("plugin.json", PLUGIN.as_bytes()), ("panel.html", b"v2")]),
        );
        let handed = std::cell::RefCell::new(Vec::new());
        let rec = |p: &Path| {
            handed
                .borrow_mut()
                .push(fs::read(p.join("panel.html")).unwrap());
            delete_path(p)
        };
        let second = commit(stage(&z, &plugins).unwrap(), &plugins, &rec).unwrap();
        assert!(second.replaced);
        assert_eq!(
            fs::read(plugins.join("demo").join("panel.html")).unwrap(),
            b"v2"
        );
        assert_eq!(handed.into_inner(), vec![b"v1".to_vec()]);
        // The scan finds the plugin and never reports the staging folder.
        let d = crate::discover(&plugins);
        assert_eq!(d.mods.len(), 1);
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        // Remove hands the folder to the Recycle Bin's stand-in; a second remove has nothing to do.
        remove(&plugins, "demo", &noop_recycle).unwrap();
        assert!(!plugins.join("demo").exists());
        remove(&plugins, "demo", &noop_recycle).unwrap();
        assert!(remove(&plugins, "../x", &noop_recycle).is_err());
    }

    #[test]
    fn leftovers_of_an_earlier_run_are_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join(STAGING).join("abc").join("zip");
        fs::create_dir_all(&left).unwrap();
        fs::write(left.join("f"), b"x").unwrap();
        clean_staging(dir.path());
        assert!(!dir.path().join(STAGING).exists());
        clean_staging(dir.path());
    }
}
