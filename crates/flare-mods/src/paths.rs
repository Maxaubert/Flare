//! Paths inside a mod folder (manifest rule and the containment check).
//!
//! Two gates, in order. [`check_relative`] is syntax only and runs when the manifest is read: a path
//! that is absolute, names a drive or a stream (`:`), is a UNC or device path, has a `..` part or is not
//! an `.html` file is refused before anything is opened. [`inside`] runs on disk: the path is resolved
//! with `canonicalize` (links and junctions are followed), and the result must still be under the
//! canonical mod folder and be a file. A link that points out is caught here, not by its name.
//! [`serve`] is the same containment run on every request the mod's page makes: the page is served by the
//! host, never by a folder mapping, so a junction inside the folder cannot expose the rest of the disk.

use std::path::{Path, PathBuf};

/// The longest entry path the manifest may name.
pub const MAX_ENTRY_CHARS: usize = 200;

/// Syntax of a relative path inside a mod folder that names a page (`.html` or `.htm`). The reason is a phrase
/// that reads after the field name.
pub fn check_relative(path: &str) -> Result<(), String> {
    check_inside_syntax(path)?;
    let lower = path.to_ascii_lowercase();
    if !(lower.ends_with(".html") || lower.ends_with(".htm")) {
        return Err("must name an .html file".into());
    }
    Ok(())
}

/// Syntax of a relative path inside a plugin folder that names its helper program: the rules of
/// [`check_relative`], and an `.exe` file (a script would need an interpreter Flare does not choose).
pub fn check_program(path: &str) -> Result<(), String> {
    check_inside_syntax(path)?;
    if !path.to_ascii_lowercase().ends_with(".exe") {
        return Err("must name an .exe file".into());
    }
    Ok(())
}

/// Syntax of `helpers.macos.command` (macOS port phase 6, regression rule 284): the rules every path inside the
/// folder keeps, `/` as the only separator (a `\\` is a letter of a Mac file name, so it would name another file than
/// its author meant on Windows), and no Windows program (`.exe`, `.bat`, `.cmd`, `.com`, `.ps1`): a Mac helper is a
/// Mach-O program or a script with a `#!` line, and has no extension to ask for.
pub fn check_mac_program(path: &str) -> Result<(), String> {
    check_inside_syntax(path)?;
    if path.contains('\\') {
        return Err("contains '\\'; a macOS path separates its folders with '/'".into());
    }
    let lower = path.to_ascii_lowercase();
    if [".exe", ".bat", ".cmd", ".com", ".ps1"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        return Err(
            "names a Windows program; name the macOS program (no extension needed) and put the .exe in helper"
                .into(),
        );
    }
    Ok(())
}

/// The part of the syntax every path inside a mod folder shares, whatever it names.
fn check_inside_syntax(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("is empty".into());
    }
    if path.chars().count() > MAX_ENTRY_CHARS {
        return Err(format!("is longer than {MAX_ENTRY_CHARS} characters"));
    }
    if path.chars().any(char::is_control) {
        return Err("contains a control character".into());
    }
    if path.contains(':') {
        return Err(
            "contains ':', which can name a drive or a stream; use a path inside the mod folder"
                .into(),
        );
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("is absolute or a network path; use a path inside the mod folder".into());
    }
    for part in path.split(['/', '\\']) {
        if part == ".." {
            return Err("goes up out of the mod folder with '..'".into());
        }
        if part.is_empty() || part == "." {
            return Err("has an empty or '.' part".into());
        }
    }
    Ok(())
}

/// The file `rel` names inside the canonical mod folder `folder`, or why not. `folder` must come from
/// `std::fs::canonicalize`; `rel` must already pass [`check_relative`].
pub fn inside(folder: &Path, rel: &str) -> Result<PathBuf, String> {
    check_relative(rel)?;
    resolve_inside(folder, rel)
}

/// [`inside`] for the helper program: `rel` must pass [`check_program`].
pub fn program_inside(folder: &Path, rel: &str) -> Result<PathBuf, String> {
    check_program(rel)?;
    resolve_inside(folder, rel)
}

/// [`inside`] for `helpers.macos.command`, on any system (the scan checks every system's helper is in the folder, so
/// a plugin is valid or not the same way everywhere): `rel` must pass [`check_mac_program`].
pub fn mac_program_inside(folder: &Path, rel: &str) -> Result<PathBuf, String> {
    check_mac_program(rel)?;
    resolve_inside(folder, rel)
}

/// macOS: the helper program `rel` names inside the canonical plugin folder, checked when it starts:
/// the syntax of [`check_mac_program`] (no `..`, no absolute path, no `:`, no `\\`, no Windows program),
/// resolved inside the folder (a link that points out is refused), a file, executable by its owner, and
/// not quarantined ([`quarantined`], rule 285).
#[cfg(not(windows))]
pub fn unix_program_inside(folder: &Path, rel: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    check_mac_program(rel)?;
    let real = resolve_inside(folder, rel)?;
    let mode = std::fs::metadata(&real) // sync-ok: the helper's own thread
        .map_err(|_| "does not exist in the mod folder".to_string())?
        .permissions()
        .mode();
    if mode & 0o100 == 0 {
        return Err("is not executable (chmod +x)".into());
    }
    if quarantined(&real) {
        return Err(format!(
            "is quarantined: macOS marked it as downloaded, and Flare does not start a quarantined program, since \
             Gatekeeper would judge it with a dialog while you may not be there. If you trust this plugin, run \
             xattr -dr {QUARANTINE} on its folder and turn it on again"
        ));
    }
    // A script helper may run another program of the folder: one quarantined file is enough to refuse.
    if let Some(rel) = quarantined_in(folder) {
        return Err(format!(
            "cannot start while {rel} in the plugin folder is quarantined: macOS marked it as downloaded, and the \
             helper could hand it to Gatekeeper, which would judge it with a dialog while you may not be there. If \
             you trust this plugin, run xattr -dr {QUARANTINE} on its folder and turn it on again"
        ));
    }
    Ok(real)
}

/// The most entries [`quarantined_in`] reads (an install holds at most 2000 files).
#[cfg(not(windows))]
const QUARANTINE_SCAN_MAX: usize = 4096;

/// The first file or folder inside `folder` (itself left out) that carries [`QUARANTINE`], as a `/`-separated path
/// relative to it (rule 285). Links are not followed: each is read as itself. Past [`QUARANTINE_SCAN_MAX`] entries or
/// 32 levels the rest is not read.
#[cfg(not(windows))]
pub fn quarantined_in(folder: &Path) -> Option<String> {
    let mut todo = vec![(folder.to_path_buf(), 0usize)];
    let mut seen = 0;
    while let Some((dir, depth)) = todo.pop() {
        let read = std::fs::read_dir(&dir); // sync-ok: the helper's own thread
        let Ok(entries) = read else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > QUARANTINE_SCAN_MAX {
                return None;
            }
            let path = entry.path();
            if quarantined_link(&path) {
                let rel = path.strip_prefix(folder).unwrap_or(&path);
                return Some(rel.to_string_lossy().into_owned());
            }
            if depth < 32 && entry.file_type().is_ok_and(|t| t.is_dir()) {
                todo.push((path, depth + 1));
            }
        }
    }
    None
}

/// The extended attribute macOS puts on a file that came from the internet (a browser's download, a file
/// unpacked from one by Archive Utility).
pub const QUARANTINE: &str = "com.apple.quarantine";

/// Whether `path` carries [`QUARANTINE`] (macOS port phase 6, regression rule 285). Starting such a program from
/// Flare would hand it to Gatekeeper, which judges it with a dialog (or, for an unsigned program, a refusal the
/// person sees only as a dialog): a helper starts in the background, when the plugin turns on or after a crash, so
/// Flare refuses it with a sentence Settings shows instead. Flare's own install from a zip writes every file itself
/// and adds no quarantine; the person approved that install in Settings.
#[cfg(target_os = "macos")]
pub fn quarantined(path: &Path) -> bool {
    has_quarantine(path, 0)
}

/// [`quarantined`] of `path` itself: a link is read as the link.
#[cfg(target_os = "macos")]
fn quarantined_link(path: &Path) -> bool {
    has_quarantine(path, libc::XATTR_NOFOLLOW)
}

#[cfg(target_os = "macos")]
fn has_quarantine(path: &Path, options: libc::c_int) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let name = c"com.apple.quarantine";
    // SAFETY: both strings are NUL-terminated and live for the call; a null buffer of size 0 asks only for the
    // attribute's size. Without XATTR_NOFOLLOW a link (already resolved inside the folder) is read through to its
    // file; with it, the link itself.
    let n = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            name.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            options,
        )
    };
    n >= 0
}

/// Off macOS there is no quarantine attribute to read.
#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn quarantined(_path: &Path) -> bool {
    false
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn quarantined_link(_path: &Path) -> bool {
    false
}

fn resolve_inside(folder: &Path, rel: &str) -> Result<PathBuf, String> {
    let joined = folder.join(rel);
    let real = std::fs::canonicalize(&joined) // sync-ok: runs on the discovery worker
        .map_err(|_| "does not exist in the mod folder".to_string())?;
    if !real.starts_with(folder) {
        return Err("points outside the mod folder (a link)".into());
    }
    if !real.is_file() {
        return Err("is not a file".into());
    }
    Ok(real)
}

/// The file a request for `url_path` names inside the canonical mod folder `folder`, or why not. This is the
/// gate for the mod's virtual host: every request is checked again (the file may have been swapped for a link
/// since the scan), and nothing else in the folder is served. `url_path` is the path after the host, already
/// percent-decoded; its leading `/` is optional. The page's own entry is the caller's to map from `/`.
pub fn serve(folder: &Path, url_path: &str) -> Result<PathBuf, String> {
    check_serve(url_path)?;
    let rel = url_path.strip_prefix('/').unwrap_or(url_path);
    let real = std::fs::canonicalize(folder.join(rel)) // sync-ok: runs on the request worker
        .map_err(|_| "does not exist in the mod folder".to_string())?;
    if !real.starts_with(folder) {
        return Err("points outside the mod folder (a link or a junction)".into());
    }
    if !real.is_file() {
        return Err("is not a file".into());
    }
    Ok(real)
}

/// The syntax of a request path: `/` separators only, no `..`, no empty or `.` part, no drive, no stream,
/// no control character. Any extension is allowed (the page loads its own script, style and image files).
fn check_serve(url_path: &str) -> Result<(), String> {
    let rel = url_path.strip_prefix('/').unwrap_or(url_path);
    if rel.is_empty() {
        return Err("is empty; the page is served at its entry".into());
    }
    if rel.chars().any(|c| c.is_control() || c == '\\' || c == ':') {
        return Err("contains a character a mod path may not have".into());
    }
    for part in rel.split('/') {
        if part == ".." {
            return Err("goes up out of the mod folder with '..'".into());
        }
        if part.is_empty() || part == "." {
            return Err("has an empty or '.' part".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A junction (a directory link) from `link` to `target`, made with `mklink /J`. False when Windows
    /// refuses, so the caller skips the test.
    fn junction(link: &Path, target: &Path) -> bool {
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let made = std::process::Command::new(format!(r"{sys}\System32\cmd.exe"))
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output();
        matches!(made, Ok(ref o) if o.status.success())
    }

    #[test]
    fn serve_finds_a_file_by_its_url_path_with_or_without_the_slash() {
        let dir = tempfile::tempdir().unwrap();
        let folder = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(folder.join("ui")).unwrap(); // sync-ok: test code only
        std::fs::write(folder.join("ui").join("app.js"), "1").unwrap(); // sync-ok: test code only
        assert!(serve(&folder, "/ui/app.js").unwrap().ends_with("app.js"));
        assert!(serve(&folder, "ui/app.js").is_ok());
    }

    #[test]
    fn serve_refuses_every_escape_form_before_touching_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let folder = std::fs::canonicalize(dir.path()).unwrap();
        for (p, want) in [
            ("/../x.html", "'..'"),
            ("/a/../../x.html", "'..'"),
            ("/C:x.html", "character"),
            ("/a\\b.html", "character"),
            ("/a\u{7}.html", "character"),
            ("/", "is empty"),
            ("", "is empty"),
            ("//x.html", "empty or '.' part"),
            ("/x.html/", "empty or '.' part"),
            ("/./x.html", "'.' part"),
        ] {
            let err = serve(&folder, p).unwrap_err();
            assert!(err.contains(want), "{p:?}: {err}");
        }
    }

    #[test]
    fn serve_refuses_a_missing_file_and_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(folder.join("sub")).unwrap(); // sync-ok: test code only
        assert!(
            serve(&folder, "/gone.js")
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(serve(&folder, "/sub").unwrap_err().contains("not a file"));
    }

    /// The hole the review found: a junction inside the mod folder that points at a user's own folder. A
    /// folder mapping of the mod would serve `data/secret.txt`; the per-request gate refuses it.
    #[test]
    fn a_junction_out_of_the_mods_folder_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "not for a mod").unwrap(); // sync-ok: test code only
        let dir = tempfile::tempdir().unwrap();
        let folder = std::fs::canonicalize(dir.path()).unwrap();
        if !junction(&folder.join("data"), outside.path()) {
            eprintln!("skipped: could not make a junction");
            return;
        }
        let err = serve(&folder, "/data/secret.txt").unwrap_err();
        assert!(err.contains("outside the mod folder"), "{err}");
    }

    /// The time-of-check hole: the folder is clean at the scan, and a directory is swapped for a junction
    /// afterwards. The next request is checked again, so it is refused.
    #[test]
    fn a_folder_swapped_for_a_junction_after_the_scan_is_refused_on_the_next_request() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.js"), "1").unwrap(); // sync-ok: test code only
        let dir = tempfile::tempdir().unwrap();
        let folder = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(folder.join("ui")).unwrap(); // sync-ok: test code only
        std::fs::write(folder.join("ui").join("app.js"), "1").unwrap(); // sync-ok: test code only
        assert!(serve(&folder, "/ui/app.js").is_ok(), "the real file serves");
        std::fs::remove_dir_all(folder.join("ui")).unwrap(); // sync-ok: test code only
        if !junction(&folder.join("ui"), outside.path()) {
            eprintln!("skipped: could not make a junction");
            return;
        }
        let err = serve(&folder, "/ui/secret.js").unwrap_err();
        assert!(err.contains("outside the mod folder"), "{err}");
    }

    #[test]
    fn plain_relative_html_paths_pass() {
        for p in ["index.html", "ui/index.html", r"ui\page.HTM", "a/b/c.html"] {
            assert_eq!(check_relative(p), Ok(()), "{p}");
        }
    }

    #[test]
    fn every_escape_form_is_refused_with_a_reason() {
        let cases: [(&str, &str); 8] = [
            ("", "is empty"),
            ("../x.html", "'..'"),
            ("a/../../x.html", "'..'"),
            (r"a\..\x.html", "'..'"),
            ("/etc/x.html", "absolute"),
            (r"\\server\share\x.html", "absolute"),
            ("C:x.html", "':'"),
            ("a/b:stream.html", "':'"),
        ];
        for (p, want) in cases {
            let err = check_relative(p).unwrap_err();
            assert!(err.contains(want), "{p:?}: {err}");
        }
        assert!(check_relative("a//b.html").unwrap_err().contains("empty"));
        assert!(check_relative("./x.html").unwrap_err().contains("'.'"));
        assert!(check_relative("x.html/").unwrap_err().contains("empty"));
    }

    #[test]
    fn the_entry_must_be_html_and_not_too_long_or_control() {
        assert!(check_relative("run.exe").unwrap_err().contains(".html"));
        assert!(check_relative("index").unwrap_err().contains(".html"));
        assert!(
            check_relative("a\u{7}.html")
                .unwrap_err()
                .contains("control")
        );
        let long = format!("{}.html", "a".repeat(MAX_ENTRY_CHARS));
        assert!(check_relative(&long).unwrap_err().contains("longer"));
    }

    #[test]
    fn a_helper_program_is_an_exe_inside_the_folder_by_the_same_rules() {
        for ok in ["helper.exe", r"bin\Helper.EXE", "a/b/run.exe"] {
            assert_eq!(check_program(ok), Ok(()), "{ok}");
        }
        for (bad, want) in [
            ("helper.bat", ".exe"),
            ("helper.cmd", ".exe"),
            ("helper.ps1", ".exe"),
            ("index.html", ".exe"),
            ("../helper.exe", "'..'"),
            ("C:helper.exe", "':'"),
            (r"\\server\x.exe", "absolute"),
            ("", "is empty"),
        ] {
            let err = check_program(bad).unwrap_err();
            assert!(err.contains(want), "{bad:?}: {err}");
        }
        let dir = tempfile::tempdir().unwrap();
        let folder = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(folder.join("helper.exe"), "MZ").unwrap(); // sync-ok: test code only
        assert!(program_inside(&folder, "helper.exe").is_ok());
        assert!(
            program_inside(&folder, "gone.exe")
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(
            program_inside(&folder, "index.html")
                .unwrap_err()
                .contains(".exe")
        );
    }

    #[test]
    fn inside_finds_a_real_file_and_refuses_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        let folder = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(folder.join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
        assert!(inside(&folder, "index.html").is_ok());
        assert!(
            inside(&folder, "gone.html")
                .unwrap_err()
                .contains("does not exist")
        );
        std::fs::create_dir(folder.join("dir.html")).unwrap(); // sync-ok: test code only
        assert!(
            inside(&folder, "dir.html")
                .unwrap_err()
                .contains("not a file")
        );
    }

    #[test]
    fn a_file_link_that_points_outside_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.html");
        std::fs::write(&secret, "<p>outside</p>").unwrap(); // sync-ok: test code only
        let dir = tempfile::tempdir().unwrap();
        let folder = std::fs::canonicalize(dir.path()).unwrap();
        let link = folder.join("link.html");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&secret, &link);
        #[cfg(not(windows))]
        let made = std::os::unix::fs::symlink(&secret, &link);
        if made.is_err() {
            eprintln!("skipped: this account cannot make file symlinks");
            return;
        }
        assert!(
            inside(&folder, "link.html")
                .unwrap_err()
                .contains("outside")
        );
    }
}
