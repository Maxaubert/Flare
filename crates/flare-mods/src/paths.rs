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

/// Syntax of a relative path inside a mod folder. The reason is a phrase that reads after the field name.
pub fn check_relative(path: &str) -> Result<(), String> {
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
    let lower = path.to_ascii_lowercase();
    if !(lower.ends_with(".html") || lower.ends_with(".htm")) {
        return Err("must name an .html file".into());
    }
    Ok(())
}

/// The file `rel` names inside the canonical mod folder `folder`, or why not. `folder` must come from
/// `std::fs::canonicalize`; `rel` must already pass [`check_relative`].
pub fn inside(folder: &Path, rel: &str) -> Result<PathBuf, String> {
    check_relative(rel)?;
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
        if std::os::windows::fs::symlink_file(&secret, &link).is_err() {
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
