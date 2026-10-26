//! Serving a mod's page (stage 8, the app part): what one request for a file of the mod folder answers.
//!
//! The browser never maps the folder (a mapping follows junctions and links inside it, `mods.md`, "Serving
//! rule"). Every request the page makes comes here: the path is checked by [`crate::paths::serve`] (syntax,
//! canonical location, a file), the file is read at most [`MAX_FILE_BYTES`], and the answer carries the
//! header policy. This runs on a worker or the request's own thread, never the UI thread (rule 34).

use std::io::Read;
use std::path::Path;

use crate::paths;

/// The most one served file may be (bytes). A mod's page is small; a bigger file is a mistake.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// What every answer of a mod's page carries as its Content-Security-Policy header. A meta tag the page's
/// author writes is theirs; this one is the host's, and a script cannot remove it.
pub const CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:";

/// The policy of a page of a plugin that holds `network` (plugins phase 6, design 13): [`CSP`], and its connections,
/// pictures and sound may come from `https` addresses too. No frame, no form that posts elsewhere, no `<base>`. A
/// policy cannot leave out the local hosts; the request wall refuses those (`flare_surfaces::net`).
pub const NET_CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; connect-src 'self' https:; media-src 'self' data: blob: https:; frame-src 'none'; form-action 'self'; base-uri 'none'";

/// The policy of a plugin's page: [`NET_CSP`] with `network`, else [`CSP`] (as a mod's page always had).
pub fn csp(network: bool) -> &'static str {
    if network { NET_CSP } else { CSP }
}

/// A file ready to answer a request with.
#[derive(Debug, PartialEq, Eq)]
pub struct Served {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

/// Why a request got no file: the HTTP status the browser is told, and a reason for the log.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub reason: String,
}

fn refuse(status: u16, reason: impl Into<String>) -> Refused {
    Refused {
        status,
        reason: reason.into(),
    }
}

/// The file a request for `url_path` (decoded, `/` or empty meaning the page's entry) names inside the
/// canonical mod folder `folder`, read in full. `entry` is the manifest's panel entry.
pub fn serve_file(folder: &Path, entry: &str, url_path: &str) -> Result<Served, Refused> {
    // A manifest may write its entry with `\`; the request syntax takes `/` only.
    let entry = entry.replace('\\', "/");
    let rel = if url_path.is_empty() || url_path == "/" {
        entry.as_str()
    } else {
        url_path
    };
    let real = paths::serve(folder, rel).map_err(|why| {
        // The same answer for a path that is missing and one that tried to leave: a probe learns nothing.
        refuse(404, why)
    })?;
    let mut bytes = Vec::new();
    let file = std::fs::File::open(&real) // sync-ok: runs on the request worker
        .map_err(|e| refuse(404, format!("cannot be opened: {e}")))?;
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes) // sync-ok: runs on the request worker
        .map_err(|e| refuse(404, format!("cannot be read: {e}")))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(refuse(413, "is larger than 16 MB"));
    }
    Ok(Served {
        bytes,
        mime: mime_of(&real),
    })
}

/// The media type a file is served as, by its extension. An unknown one is `application/octet-stream` (and
/// `X-Content-Type-Options: nosniff` keeps the browser from guessing otherwise).
pub fn mime_of(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "txt" | "md" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/ogg; codecs=opus",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "weba" => "audio/webm",
        _ => "application/octet-stream",
    }
}

/// The response headers for a served file, as WebView2 takes them (CRLF separated). `network`: the page is a plugin's
/// that holds `network` ([`csp`]).
pub fn headers_for(mime: &str, network: bool) -> String {
    let policy = csp(network);
    format!(
        "Content-Type: {mime}\r\nContent-Security-Policy: {policy}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nCross-Origin-Resource-Policy: same-origin"
    )
}

/// The headers of a refusal (no file): still carries the policy, so an error page is as sealed as a page.
pub fn refusal_headers() -> String {
    format!(
        "Content-Type: text/plain; charset=utf-8\r\nContent-Security-Policy: {CSP}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(dir.path()).unwrap();
        (dir, canon)
    }

    #[test]
    fn the_root_is_the_entry_and_a_path_is_a_file_with_its_type() {
        let (_d, f) = folder();
        std::fs::write(f.join("index.html"), "<p>hi</p>").unwrap(); // sync-ok: test code only
        std::fs::create_dir(f.join("js")).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("js").join("a.js"), "1").unwrap(); // sync-ok: test code only
        let root = serve_file(&f, "index.html", "/").unwrap();
        assert_eq!(root.bytes, b"<p>hi</p>");
        assert!(root.mime.starts_with("text/html"));
        assert_eq!(serve_file(&f, "index.html", "").unwrap().bytes.len(), 9);
        let js = serve_file(&f, "index.html", "/js/a.js").unwrap();
        assert!(js.mime.starts_with("text/javascript"));
    }

    #[test]
    fn what_is_missing_or_tries_to_leave_is_a_404_with_a_reason() {
        let (_d, f) = folder();
        std::fs::write(f.join("index.html"), "x").unwrap(); // sync-ok: test code only
        for p in ["/nope.js", "/../x", "/a/../../x", "/C:x", "/a\\b"] {
            let e = serve_file(&f, "index.html", p).unwrap_err();
            assert_eq!(e.status, 404, "{p}");
            assert!(!e.reason.is_empty());
        }
    }

    #[test]
    fn a_file_over_the_cap_is_413_and_a_folder_is_not_a_file() {
        let (_d, f) = folder();
        std::fs::create_dir(f.join("sub")).unwrap(); // sync-ok: test code only
        assert_eq!(
            serve_file(&f, "index.html", "/sub").unwrap_err().status,
            404
        );
        let big = std::fs::File::create(f.join("big.bin")).unwrap(); // sync-ok: test code only
        big.set_len(MAX_FILE_BYTES + 1).unwrap(); // sync-ok: test code only
        assert_eq!(
            serve_file(&f, "index.html", "/big.bin").unwrap_err().status,
            413
        );
        let ok = std::fs::File::create(f.join("ok.bin")).unwrap(); // sync-ok: test code only
        ok.set_len(1024).unwrap(); // sync-ok: test code only
        assert_eq!(
            serve_file(&f, "index.html", "/ok.bin").unwrap().bytes.len(),
            1024
        );
    }

    #[test]
    fn a_junction_that_points_out_is_never_served() {
        let (_d, f) = folder();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "key").unwrap(); // sync-ok: test code only
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let made = std::process::Command::new(format!(r"{sys}\System32\cmd.exe"))
            .args(["/C", "mklink", "/J"])
            .arg(f.join("data"))
            .arg(outside.path())
            .output();
        if !matches!(made, Ok(ref o) if o.status.success()) {
            return; // Windows refused to make a junction here: nothing to hold
        }
        let e = serve_file(&f, "index.html", "/data/secret.txt").unwrap_err();
        assert_eq!(e.status, 404);
    }

    #[test]
    fn the_policy_is_on_every_answer_and_names_no_network() {
        let h = headers_for("text/html", false);
        assert!(h.contains("Content-Security-Policy: default-src 'none'"));
        assert!(h.contains("X-Content-Type-Options: nosniff"));
        assert!(refusal_headers().contains("Content-Security-Policy"));
        assert!(!CSP.contains("http"));
        assert!(!CSP.contains("connect-src"), "default-src 'none' covers it");
    }

    /// Design 13: with `network` the page may connect, and load pictures and sound, over https; it still has no
    /// frame, posts no form elsewhere and sets no base, and without `network` the policy is the sealed one.
    #[test]
    fn a_networked_plugins_policy_opens_https_for_connections_pictures_and_sound_only() {
        assert_eq!(csp(false), CSP);
        let p = csp(true);
        for part in [
            "default-src 'none'",
            "connect-src 'self' https:",
            "img-src 'self' data: https:",
            "media-src 'self' data: blob: https:",
            "frame-src 'none'",
            "form-action 'self'",
            "base-uri 'none'",
        ] {
            assert!(p.contains(part), "{part} in {p}");
        }
        assert!(!p.contains("http:"), "never plain http");
        assert!(
            !p.contains("script-src 'self' 'unsafe-inline' https"),
            "no script from the internet"
        );
        assert!(!p.contains('*'));
        let h = headers_for("text/html", true);
        assert!(h.contains(&format!("Content-Security-Policy: {NET_CSP}")));
        assert!(!headers_for("text/html", false).contains("https:"));
    }

    #[test]
    fn types_follow_the_extension() {
        assert_eq!(mime_of(Path::new("a.mp3")), "audio/mpeg");
        assert_eq!(mime_of(Path::new("a.ogg")), "audio/ogg");
        assert_eq!(mime_of(Path::new("a.PNG")), "image/png");
        assert_eq!(mime_of(Path::new("a")), "application/octet-stream");
        assert_eq!(mime_of(Path::new("a.exe")), "application/octet-stream");
    }
}
