//! resolve_term_path, StatGate, path_kinds, open_term_path.
//!
//! A path clicked or hovered in the terminal. The page says what text it saw and which folder the
//! shell is in; this module alone decides what that names, whether it exists, and what opening it
//! means. Stat calls block: they live on worker threads only, every blocking line carries
//! `// sync-ok:`.
//!
//! The stats are metered. At most `slots` stats are in flight, a slot is held until its stat itself
//! ends, and a drive whose stat takes longer than `slow_ms` answers "nothing there" for
//! `slow_for_ms` without being asked: a link that lights up a minute late beats a stalled save.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// The most texts asked about in one go.
pub const PATHS_MAX: usize = 300;
pub(crate) const TEXT_MAX: usize = 1024;
const WORKERS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    File,
    Dir,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathHit {
    pub kind: PathKind,
    /// Resolved and normalised: what is opened.
    pub abs: String,
}

/// The only file types a click may open (decided 2026-10-17, safer than Prism Terminal's denylist):
/// documents, text, data, code that no default verb runs, images, audio and video. Everything else
/// (unknown, runnable, scripts, shortcuts, disk images, certificates) is revealed, never opened.
const OPENABLE: &[&str] = &[
    // text and data
    "txt", "md", "markdown", "rst", "log", "json", "jsonc", "yaml", "yml", "toml", "csv", "tsv",
    "ini", "cfg", "conf", "xml", "lock", "diff", "patch", "env", // documents
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "epub",
    // images
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "tif", "tiff", "avif", "heic",
    // audio and video
    "mp3", "wav", "flac", "ogg", "opus", "m4a", "aac", "wma", "mp4", "mkv", "webm", "mov", "avi",
    "m4v", "wmv",
    // source code (never one whose default verb runs it: no py, js, sh, rb, pl, ps1, and no lua
    // or php, whose installers may register a run verb, and no jsx, which Adobe apps run)
    "rs", "ts", "tsx", "c", "h", "cpp", "hpp", "cc", "cs", "go", "java", "kt", "swift", "css",
    "scss", "sql", "slint", "vue", "svelte",
];

/// The extension (lower case) of a file name Windows would read the way it is shown, or None for a
/// name it would read differently (a trailing dot or space on any segment, a `:` stream) or one with
/// no extension.
fn plain_extension(file: &str) -> Option<String> {
    let body = file.strip_prefix(r"\\?\").unwrap_or(file);
    let rest = if has_drive_root(body) {
        &body[2..]
    } else {
        body
    };
    if rest.contains(':') {
        return None;
    }
    if rest
        .split(is_sep)
        .any(|seg| seg.ends_with('.') || seg.ends_with(' '))
    {
        return None;
    }
    let name = rest.rsplit(is_sep).next().unwrap_or(rest);
    name.rfind('.').map(|i| name[i + 1..].to_lowercase())
}

/// True when a click may open a file of this name. Unknown and runnable types answer false, and so
/// does every name Windows would read differently from what is shown (a trailing dot or space on
/// any segment, a `:` stream).
pub fn may_open(file: &str) -> bool {
    plain_extension(file).is_some_and(|e| OPENABLE.contains(&e.as_str()))
}

/// True for a page a mini-app may be made from (`view.html` with a path): `html` or `htm`, under the
/// same name rules as [`may_open`]. It is not on the click allowlist: a click never opens a page, only
/// the agent's `show_html` shows one, inside the mini-app sandbox.
pub fn may_show_as_app(file: &str) -> bool {
    plain_extension(file).is_some_and(|e| e == "html" || e == "htm")
}

/// True for a file a viewer of Flare's own may show when it is one of `exts`, under the same name rules as [`may_open`].
/// It is not on the click allowlist: a click never opens such a file (Windows opens a `.stl` as a certificate list), only
/// the tool for it shows it, inside the mini-app sandbox.
pub fn may_show_as(file: &str, exts: &[&str]) -> bool {
    plain_extension(file).is_some_and(|e| exts.contains(&e.as_str()))
}

/// The path as the file system names it (links and 8.3 names resolved), or None when it cannot be.
fn canonical(abs: &str) -> Option<String> {
    let c = std::fs::canonicalize(abs).ok()?; // sync-ok: runs on the click's own worker only
    let s = c.to_string_lossy().into_owned();
    Some(s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s))
}

pub(crate) fn has_drive_root(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

pub(crate) fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// What `text` names from inside `cwd`, or None when it names nothing this checks: a network path,
/// a NUL, a relative path with no folder to be relative to, an over-long text. The grammar is the
/// platform's (`path_grammar`): drive paths on Windows, POSIX paths on macOS.
pub fn resolve_term_path(cwd: &str, text: &str, home: &str) -> Option<String> {
    crate::path_grammar::resolve_term_path(cwd, text, home)
}

/// A blocking stat. Called on worker threads only.
pub trait StatFn: Send + Sync {
    fn kind(&self, abs: &str) -> Option<PathKind>;
}

/// The real file system.
pub struct FsStat;

impl StatFn for FsStat {
    fn kind(&self, abs: &str) -> Option<PathKind> {
        let m = std::fs::metadata(abs).ok()?; // sync-ok: runs on stat threads only
        if m.is_dir() {
            Some(PathKind::Dir)
        } else if m.is_file() {
            Some(PathKind::File)
        } else {
            None
        }
    }
}

pub struct StatGateOptions {
    pub slots: usize,
    pub slow_ms: u64,
    pub slow_for_ms: u64,
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Default for StatGateOptions {
    fn default() -> Self {
        static START: OnceLock<Instant> = OnceLock::new();
        let start = *START.get_or_init(Instant::now);
        Self {
            slots: 3,
            slow_ms: 750,
            slow_for_ms: 60_000,
            clock: Arc::new(move || start.elapsed().as_millis() as u64),
        }
    }
}

struct Inner {
    stat: Arc<dyn StatFn>,
    opts: StatGateOptions,
    metered: bool,
    active: Mutex<usize>,
    freed: Condvar,
    slow_until: Mutex<HashMap<String, u64>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone)]
pub struct StatGate {
    inner: Arc<Inner>,
}

impl Inner {
    fn is_slow(&self, drive: &str) -> bool {
        let mut map = lock(&self.slow_until);
        match map.get(drive) {
            None => false,
            Some(&until) if (self.opts.clock)() < until => true,
            Some(_) => {
                map.remove(drive);
                false
            }
        }
    }

    fn release(&self) {
        *lock(&self.active) -= 1;
        self.freed.notify_one();
    }
}

impl StatGate {
    fn make(stat: Arc<dyn StatFn>, opts: StatGateOptions, metered: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                stat,
                opts,
                metered,
                active: Mutex::new(0),
                freed: Condvar::new(),
                slow_until: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn new(stat: Arc<dyn StatFn>, opts: StatGateOptions) -> Self {
        Self::make(stat, opts, true)
    }

    /// No meter and no slow-drive rule: for a click, one question the user asked.
    pub fn unmetered(stat: Arc<dyn StatFn>) -> Self {
        let opts = StatGateOptions {
            slots: usize::MAX,
            ..StatGateOptions::default()
        };
        Self::make(stat, opts, false)
    }

    /// What `abs` is, through the meter: None for nothing there, or a drive held slow.
    pub fn kind(&self, abs: &str) -> Option<PathKind> {
        let inner = &self.inner;
        if !inner.metered {
            return inner.stat.kind(abs);
        }
        let drive = crate::path_grammar::volume_key(abs);
        if inner.is_slow(&drive) {
            return None;
        }
        {
            let mut active = lock(&inner.active);
            while *active >= inner.opts.slots {
                active = inner.freed.wait(active).unwrap_or_else(|e| e.into_inner()); // sync-ok: runs on worker threads only
            }
            *active += 1;
        }
        if inner.is_slow(&drive) {
            inner.release();
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let for_thread = Arc::clone(inner);
        let path = abs.to_string();
        std::thread::spawn(move || {
            let got = for_thread.stat.kind(&path);
            // The slot frees when the stat itself ends, even if nobody waits any more.
            for_thread.release();
            let _ = tx.send(got);
        });
        let wait = Duration::from_millis(inner.opts.slow_ms);
        match rx.recv_timeout(wait) {
            // sync-ok: runs on worker threads only
            Ok(got) => got,
            Err(_) => {
                let until = (inner.opts.clock)().saturating_add(inner.opts.slow_for_ms);
                lock(&inner.slow_until).insert(drive, until);
                None
            }
        }
    }
}

/// What each text names, in order: a file, a folder, or None. Only the first PATHS_MAX texts are
/// asked; up to 8 worker threads pull indices, so a long list is not one thread per path.
pub fn path_kinds(
    cwd: &str,
    texts: &[String],
    gate: &StatGate,
    home: &str,
) -> Vec<Option<PathHit>> {
    let list = &texts[..texts.len().min(PATHS_MAX)];
    let results: Mutex<Vec<Option<PathHit>>> = Mutex::new(vec![None; list.len()]);
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..list.len().min(WORKERS) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(text) = list.get(i) else { break };
                    let Some(abs) = resolve_term_path(cwd, text, home) else {
                        continue;
                    };
                    if let Some(kind) = gate.kind(&abs) {
                        lock(&results)[i] = Some(PathHit { kind, abs });
                    }
                }
            });
        }
    });
    results.into_inner().unwrap_or_else(|e| e.into_inner())
}

/// A file a click may show inside the app: where the text names it, and the same file as the file
/// system names it (links and 8.3 names resolved). Resolved again here, in the backend (rule 13), with
/// one unmetered stat. None for a folder, a name that does not exist, a network path, and any file
/// that is not on the click allowlist either as named or as the file system names it (rule 14):
/// the caller then decides by extension what, if anything, the file may be shown as.
pub fn resolve_openable_file(cwd: &str, text: &str, home: &str) -> Option<(String, String)> {
    let gate = StatGate::unmetered(Arc::new(FsStat));
    let hit = path_kinds(cwd, &[text.to_string()], &gate, home)
        .into_iter()
        .next()??;
    if hit.kind != PathKind::File || !may_open(&hit.abs) {
        return None;
    }
    let real = canonical(&hit.abs)?;
    may_open(&real).then_some((hit.abs, real))
}

/// A page for a mini-app (`view.html` with a path): resolved again in the backend like
/// [`resolve_openable_file`] (rule 13), with the same checks on the name as typed and as the file
/// system names it (rule 14), but the extension must be `html` or `htm` ([`may_show_as_app`]).
pub fn resolve_html_file(cwd: &str, text: &str, home: &str) -> Option<(String, String)> {
    let gate = StatGate::unmetered(Arc::new(FsStat));
    let hit = path_kinds(cwd, &[text.to_string()], &gate, home)
        .into_iter()
        .next()??;
    if hit.kind != PathKind::File || !may_show_as_app(&hit.abs) {
        return None;
    }
    let real = canonical(&hit.abs)?;
    may_show_as_app(&real).then_some((hit.abs, real))
}

/// A file for one of Flare's own viewers (`view.model`): resolved again in the backend like
/// [`resolve_openable_file`] (rule 13), with the same checks on the name as typed and as the file system names it (rule
/// 14), but the extension must be one of `exts` ([`may_show_as`]).
pub fn resolve_file_of(
    cwd: &str,
    text: &str,
    home: &str,
    exts: &[&str],
) -> Option<(String, String)> {
    let gate = StatGate::unmetered(Arc::new(FsStat));
    let hit = path_kinds(cwd, &[text.to_string()], &gate, home)
        .into_iter()
        .next()??;
    if hit.kind != PathKind::File || !may_show_as(&hit.abs, exts) {
        return None;
    }
    let real = canonical(&hit.abs)?;
    may_show_as(&real, exts).then_some((hit.abs, real))
}

pub trait PathOpeners {
    /// The app Windows gives it.
    fn open_path(&self, abs: &str);
    /// A folder, by absolute path; false = not opened (nothing else is tried).
    fn open_folder(&self, _abs: &str) -> bool {
        false
    }
    /// Explorer, with it selected.
    fn reveal_path(&self, abs: &str);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    Open,
    Reveal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opened {
    Opened,
    Revealed,
}

/// Open or reveal what `text` names, after resolving and checking it here. One unmetered stat.
/// Clicking text must never start a program: a file not on the allowlist is revealed instead.
pub fn open_term_path(
    cwd: &str,
    text: &str,
    mode: OpenMode,
    openers: &dyn PathOpeners,
    home: &str,
) -> Option<Opened> {
    let gate = StatGate::unmetered(Arc::new(FsStat));
    let hit = path_kinds(cwd, &[text.to_string()], &gate, home)
        .into_iter()
        .next()??;
    if mode == OpenMode::Reveal
        || (hit.kind == PathKind::File
            && !(may_open(&hit.abs) && canonical(&hit.abs).is_some_and(|c| may_open(&c))))
    {
        openers.reveal_path(&hit.abs);
        return Some(Opened::Revealed);
    }
    if hit.kind == PathKind::Dir {
        // A folder is never handed to the default verb: by the time it runs the name may be a
        // runnable file.
        return openers.open_folder(&hit.abs).then_some(Opened::Opened);
    }
    openers.open_path(&hit.abs);
    Some(Opened::Opened)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn tree() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs").join("sign-off")).unwrap();
        std::fs::write(dir.path().join("docs/sign-off/rapport.pdf"), b"x").unwrap();
        std::fs::write(dir.path().join("run.bat"), b"x").unwrap();
        let root = dir.path().to_str().unwrap().to_string();
        (dir, root)
    }

    #[derive(Default)]
    struct Rec {
        opened: Mutex<Vec<String>>,
        revealed: Mutex<Vec<String>>,
        folders: Mutex<Vec<String>>,
        handles_folders: bool,
    }
    impl PathOpeners for Rec {
        fn open_path(&self, abs: &str) {
            lock(&self.opened).push(abs.into());
        }
        fn open_folder(&self, abs: &str) -> bool {
            lock(&self.folders).push(abs.into());
            self.handles_folders
        }
        fn reveal_path(&self, abs: &str) {
            lock(&self.revealed).push(abs.into());
        }
    }

    #[test]
    fn an_openable_file_resolves_to_itself_and_its_canonical_form() {
        let (_d, root) = tree();
        let got = resolve_openable_file(&root, "docs/sign-off/rapport.pdf", "").unwrap();
        assert!(got.0.ends_with("rapport.pdf") && got.1.ends_with("rapport.pdf"));
        assert!(std::path::Path::new(&got.0).is_file());
    }

    #[test]
    fn a_folder_a_missing_file_a_script_and_a_network_path_resolve_to_nothing() {
        let (_d, root) = tree();
        assert!(resolve_openable_file(&root, "docs/sign-off", "").is_none());
        assert!(resolve_openable_file(&root, "docs/none.pdf", "").is_none());
        assert!(resolve_openable_file(&root, "run.bat", "").is_none());
        assert!(resolve_openable_file(&root, r"\\server\share\a.png", "").is_none());
        assert!(resolve_openable_file(&root, "a\0b.png", "").is_none());
        assert!(resolve_openable_file("", "a.png", "").is_none());
    }

    #[test]
    fn a_page_for_a_mini_app_is_html_or_htm_and_nothing_else() {
        for f in ["a.html", "C:\\x\\B.HTM", "C:\\x\\y\\page.Html"] {
            assert!(may_show_as_app(f), "{f}");
            // A click still never opens one.
            assert!(!may_open(f), "{f}");
        }
        for f in [
            "a.js",
            "a.exe",
            "a.svg",
            "a.xhtml",
            "a.html.exe",
            "a.html.",
            "a.html ",
            "a.html:stream",
            "C:\\x.\\a.html",
            "README",
            "html",
        ] {
            assert!(!may_show_as_app(f), "{f}");
        }
    }

    #[test]
    fn a_file_for_a_viewer_is_one_of_its_extensions_under_the_same_name_rules() {
        let exts = ["glb", "stl"];
        for f in ["a.glb", "C:\\x\\B.STL"] {
            assert!(may_show_as(f, &exts), "{f}");
        }
        for f in [
            "a.exe",
            "a.glb.exe",
            "a.glb.",
            "a.glb ",
            "a.glb:stream",
            "glb",
        ] {
            assert!(!may_show_as(f, &exts), "{f}");
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.glb"), b"x").unwrap();
        std::fs::write(dir.path().join("b.exe"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("d.glb")).unwrap();
        let root = dir.path().to_str().unwrap().to_string();
        assert!(resolve_file_of(&root, "a.glb", "", &exts).is_some());
        assert!(resolve_file_of(&root, "b.exe", "", &exts).is_none());
        assert!(
            resolve_file_of(&root, "d.glb", "", &exts).is_none(),
            "a folder"
        );
        assert!(resolve_file_of(&root, "c.glb", "", &exts).is_none());
        assert!(resolve_file_of(&root, r"\\server\share\a.glb", "", &exts).is_none());
    }

    #[test]
    fn a_page_resolves_in_the_backend_and_a_script_a_folder_or_a_share_does_not() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.html"), b"<p>x").unwrap();
        std::fs::write(dir.path().join("b.js"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("d.html")).unwrap();
        let root = dir.path().to_str().unwrap().to_string();
        let got = resolve_html_file(&root, "a.html", "").unwrap();
        assert!(got.0.ends_with("a.html") && got.1.ends_with("a.html"));
        assert!(resolve_html_file(&root, "b.js", "").is_none());
        assert!(resolve_html_file(&root, "d.html", "").is_none(), "a folder");
        assert!(resolve_html_file(&root, "missing.html", "").is_none());
        assert!(resolve_html_file(&root, r"\\server\share\a.html", "").is_none());
        assert!(resolve_html_file("", "a.html", "").is_none());
        // A page is not an openable file for a click.
        assert!(resolve_openable_file(&root, "a.html", "").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn resolve_against_the_shell_folder() {
        let r = |t: &str, h: &str| resolve_term_path("C:\\work", t, h);
        assert_eq!(
            r("docs/a.pdf", "").as_deref(),
            Some("C:\\work\\docs\\a.pdf")
        );
        assert_eq!(r("..\\up.txt", "").as_deref(), Some("C:\\up.txt"));
        assert_eq!(r("D:/x/y.md", "").as_deref(), Some("D:\\x\\y.md"));
        assert_eq!(r("\\rooted.txt", "").as_deref(), Some("C:\\rooted.txt"));
        assert_eq!(
            r("~/n.txt", "C:\\Users\\me").as_deref(),
            Some("C:\\Users\\me\\n.txt")
        );
    }

    #[test]
    fn resolve_refuses_unc_nul_and_no_folder() {
        let r = |c: &str, t: &str| resolve_term_path(c, t, "");
        assert_eq!(r("C:\\work", "\\\\server\\share\\a.txt"), None);
        assert_eq!(r("C:\\work", "//server/share/a.txt"), None);
        assert_eq!(r("C:\\work", "a\0.txt"), None);
        assert_eq!(r("", "docs/a.pdf"), None);
        assert_eq!(r("relative", "docs/a.pdf"), None);
        assert_eq!(r("C:\\work", ""), None);
    }

    #[cfg(windows)]
    #[test]
    fn normalise_keeps_trailing_separator_and_stops_at_the_root() {
        assert_eq!(
            resolve_term_path("C:\\work", "a//b\\.\\c/", "").as_deref(),
            Some("C:\\work\\a\\b\\c\\")
        );
        assert_eq!(
            resolve_term_path("C:\\w", "..\\..\\..\\x", "").as_deref(),
            Some("C:\\x")
        );
    }

    #[cfg(windows)]
    #[test]
    fn path_kinds_says_what_exists() {
        let (_d, root) = tree();
        let gate = StatGate::unmetered(Arc::new(FsStat));
        let texts: Vec<String> = [
            "docs/sign-off/rapport.pdf",
            "docs/sign-off/",
            "missing/x.txt",
            "and/or",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let got = path_kinds(&root, &texts, &gate, "");
        let kinds: Vec<Option<PathKind>> = got.iter().map(|h| h.as_ref().map(|h| h.kind)).collect();
        assert_eq!(
            kinds,
            vec![Some(PathKind::File), Some(PathKind::Dir), None, None]
        );
        assert_eq!(
            got[0].as_ref().unwrap().abs,
            format!("{root}\\docs\\sign-off\\rapport.pdf")
        );
    }

    #[test]
    fn open_a_file_and_a_folder() {
        let (_d, root) = tree();
        let r = Rec {
            handles_folders: true,
            ..Rec::default()
        };
        let o = |t: &str| open_term_path(&root, t, OpenMode::Open, &r, "");
        assert_eq!(o("docs/sign-off/rapport.pdf"), Some(Opened::Opened));
        assert_eq!(o("docs"), Some(Opened::Opened));
        assert_eq!(lock(&r.opened).len(), 1);
        assert_eq!(lock(&r.folders).len(), 1);
        assert_eq!(lock(&r.revealed).len(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn a_runnable_file_is_revealed_never_run() {
        let (_d, root) = tree();
        let r = Rec::default();
        assert_eq!(
            open_term_path(&root, "run.bat", OpenMode::Open, &r, ""),
            Some(Opened::Revealed)
        );
        assert!(lock(&r.opened).is_empty());
        assert_eq!(*lock(&r.revealed), vec![format!("{root}\\run.bat")]);
    }

    #[test]
    fn reveal_on_request_and_nothing_for_missing() {
        let (_d, root) = tree();
        let r = Rec::default();
        assert_eq!(
            open_term_path(&root, "docs/sign-off/rapport.pdf", OpenMode::Reveal, &r, ""),
            Some(Opened::Revealed)
        );
        assert_eq!(
            open_term_path(&root, "missing.txt", OpenMode::Open, &r, ""),
            None
        );
        assert!(lock(&r.opened).is_empty());
    }

    #[test]
    fn may_open_allows_documents_media_and_safe_code_only() {
        for f in [
            "a.pdf", "b.md", "c.png", "d.txt", "e.docx", "f.ts", "g.json", "h.MP4", "i.rs",
        ] {
            assert!(may_open(f), "{f}");
        }
        for f in [
            "a.exe",
            "b.BAT",
            "c.ps1",
            "d.lnk",
            "e.msi",
            "f.js",
            "g.vbs",
            "h.py",
            "i.url",
            "j.reg",
            "k.pif",
            "l.scr",
            "m.cpl",
            "n.msc",
            "o.hta",
            "p.chm",
            "q.jar",
            "r.scf",
            "s.xll",
            "t.wsf",
            "u.theme",
            "v.themepack",
            "w.iso",
            "x.cer",
            "y.docm",
            "z.vhd",
            "README",
            "noext.",
            "a.unknownext",
            "b.lua",
            "c.php",
        ] {
            assert!(!may_open(f), "{f}");
        }
    }

    #[test]
    fn trailing_dot_space_and_streams_are_refused() {
        for f in [
            r"C:\x\evil.exe.",
            r"C:\x\evil.exe ",
            r"C:\x\evil.txt.",
            r"C:\x\ok.txt ",
            r"C:\x\evil.exe:stream",
            r"C:\x\ok.txt:evil.exe",
            r"C:\x\dir.\ok.txt",
            r"C:\x\dir \ok.txt",
            r"C:\x\...\ok.txt",
        ] {
            assert!(!may_open(f), "{f}");
        }
        assert!(may_open(r"C:\x\ok.txt"));
        assert!(may_open(r"C:\x y\ok.txt"));
    }

    #[test]
    fn an_unknown_type_is_revealed_never_opened() {
        let (d, root) = tree();
        std::fs::write(d.path().join("disk.iso"), b"x").unwrap();
        std::fs::write(d.path().join("noext"), b"x").unwrap();
        let r = Rec::default();
        for t in ["disk.iso", "noext", "run.bat"] {
            assert_eq!(
                open_term_path(&root, t, OpenMode::Open, &r, ""),
                Some(Opened::Revealed),
                "{t}"
            );
        }
        assert!(lock(&r.opened).is_empty());
    }

    #[test]
    fn a_trailing_dot_exe_is_revealed_never_opened() {
        let (d, root) = tree();
        std::fs::write(d.path().join("evil.exe"), b"x").unwrap();
        let r = Rec::default();
        for t in ["evil.exe.", "evil.exe ", "evil.exe:stream"] {
            let got = open_term_path(&root, t, OpenMode::Open, &r, "");
            assert_ne!(got, Some(Opened::Opened), "{t}");
        }
        assert!(lock(&r.opened).is_empty());
    }

    #[test]
    fn the_extension_comes_from_the_canonical_path() {
        // A link hides the real extension: the real one decides.
        let (d, root) = tree();
        let target = d.path().join("evil.exe");
        std::fs::write(&target, b"x").unwrap();
        let link = d.path().join("innocent.txt");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link);
        #[cfg(not(windows))]
        let made = std::os::unix::fs::symlink(&target, &link);
        if made.is_err() {
            return; // no symlink privilege on this machine
        }
        let r = Rec::default();
        assert_eq!(
            open_term_path(&root, "innocent.txt", OpenMode::Open, &r, ""),
            Some(Opened::Revealed)
        );
        assert!(lock(&r.opened).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_uses_open_folder_a_file_never_does() {
        let (_d, root) = tree();
        let r = Rec {
            handles_folders: true,
            ..Rec::default()
        };
        let o = |t: &str, m| open_term_path(&root, t, m, &r, "");
        assert_eq!(o("docs", OpenMode::Open), Some(Opened::Opened));
        assert_eq!(*lock(&r.folders), vec![format!("{root}\\docs")]);
        assert_eq!(
            o("docs/sign-off/rapport.pdf", OpenMode::Open),
            Some(Opened::Opened)
        );
        assert_eq!(
            *lock(&r.opened),
            vec![format!("{root}\\docs\\sign-off\\rapport.pdf")]
        );
        assert_eq!(o("run.bat", OpenMode::Open), Some(Opened::Revealed));
        assert_eq!(o("docs", OpenMode::Reveal), Some(Opened::Revealed));
        assert_eq!(lock(&r.folders).len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_explorer_did_not_take_is_never_handed_to_the_default_verb() {
        // The stat said Dir; when the folder opener fails nothing opens (the name may have become
        // a runnable file since the stat).
        let (_d, root) = tree();
        let r = Rec::default();
        assert_eq!(open_term_path(&root, "docs", OpenMode::Open, &r, ""), None);
        assert_eq!(*lock(&r.folders), vec![format!("{root}\\docs")]);
        assert!(lock(&r.opened).is_empty());
    }

    #[test]
    fn an_adobe_script_is_revealed_never_opened() {
        // Photoshop and Illustrator RUN a double-clicked .jsx (ExtendScript).
        assert!(!may_open(r"C:\x\a.jsx"));
    }

    struct Blocking {
        active: AtomicUsize,
        most: AtomicUsize,
        tokens: Mutex<mpsc::Receiver<()>>,
    }
    impl StatFn for Blocking {
        fn kind(&self, _abs: &str) -> Option<PathKind> {
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            let _ = lock(&self.tokens).recv();
            self.active.fetch_sub(1, Ordering::SeqCst);
            Some(PathKind::File)
        }
    }

    #[cfg(windows)]
    #[test]
    fn never_more_than_three_stats_in_flight() {
        let (tx, rx) = mpsc::channel();
        let stat = Arc::new(Blocking {
            active: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
            tokens: Mutex::new(rx),
        });
        let gate = StatGate::new(
            stat.clone(),
            StatGateOptions {
                slots: 3,
                ..StatGateOptions::default()
            },
        );
        let texts: Vec<String> = (0..40).map(|i| format!("f{i}.txt")).collect();
        let h = std::thread::spawn(move || path_kinds("C:\\w", &texts, &gate, ""));
        for _ in 0..40 {
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        let got = h.join().unwrap();
        assert_eq!(stat.most.load(Ordering::SeqCst), 3);
        assert_eq!(got.len(), 40);
        assert!(
            got.iter()
                .all(|h| h.as_ref().map(|h| h.kind) == Some(PathKind::File))
        );
    }

    #[cfg(windows)]
    struct Drives {
        asked: Mutex<Vec<String>>,
        tokens: Mutex<mpsc::Receiver<()>>,
    }
    #[cfg(windows)]
    impl StatFn for Drives {
        fn kind(&self, abs: &str) -> Option<PathKind> {
            lock(&self.asked).push(abs.into());
            if abs.starts_with("Z:") {
                let _ = lock(&self.tokens).recv();
                return None;
            }
            Some(PathKind::File)
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_hanging_drive_answers_nothing_for_a_minute() {
        let (tx, rx) = mpsc::channel();
        let stat = Arc::new(Drives {
            asked: Mutex::new(vec![]),
            tokens: Mutex::new(rx),
        });
        let now = Arc::new(AtomicU64::new(0));
        let c = now.clone();
        let gate = StatGate::new(
            stat.clone(),
            StatGateOptions {
                slots: 3,
                slow_ms: 50,
                slow_for_ms: 60_000,
                clock: Arc::new(move || c.load(Ordering::SeqCst)),
            },
        );
        assert_eq!(gate.kind("Z:\\share\\a.txt"), None);
        let t = Instant::now();
        assert_eq!(gate.kind("Z:\\share\\b.txt"), None);
        assert!(t.elapsed() < Duration::from_millis(20));
        assert_eq!(gate.kind("C:\\local.txt"), Some(PathKind::File));
        assert_eq!(
            *lock(&stat.asked),
            vec!["Z:\\share\\a.txt".to_string(), "C:\\local.txt".to_string()]
        );
        now.store(60_000, Ordering::SeqCst);
        assert_eq!(gate.kind("Z:\\share\\c.txt"), None);
        assert!(lock(&stat.asked).contains(&"Z:\\share\\c.txt".to_string()));
        for _ in 0..2 {
            let _ = tx.send(());
        }
    }

    /// The POSIX twins of the tests above that spell a Windows path (`{root}\\docs`, `C:\\w`).
    #[cfg(not(windows))]
    mod posix {
        use super::*;

        #[test]
        fn path_kinds_says_what_exists() {
            let (_d, root) = tree();
            let gate = StatGate::unmetered(Arc::new(FsStat));
            let texts: Vec<String> = [
                "docs/sign-off/rapport.pdf",
                "docs/sign-off/",
                "missing/x.txt",
                "and/or",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            let got = path_kinds(&root, &texts, &gate, "");
            let kinds: Vec<Option<PathKind>> =
                got.iter().map(|h| h.as_ref().map(|h| h.kind)).collect();
            assert_eq!(
                kinds,
                vec![Some(PathKind::File), Some(PathKind::Dir), None, None]
            );
            assert_eq!(
                got[0].as_ref().unwrap().abs,
                format!("{root}/docs/sign-off/rapport.pdf")
            );
        }

        #[test]
        fn a_home_path_and_an_absolute_one_exist_from_any_folder() {
            let (_d, root) = tree();
            let gate = StatGate::unmetered(Arc::new(FsStat));
            let texts = vec![
                "~/docs/sign-off/rapport.pdf".to_string(),
                format!("{root}/run.bat"),
                "~".to_string(),
            ];
            let got = path_kinds("/", &texts, &gate, &root);
            let kinds: Vec<Option<PathKind>> =
                got.iter().map(|h| h.as_ref().map(|h| h.kind)).collect();
            assert_eq!(
                kinds,
                vec![
                    Some(PathKind::File),
                    Some(PathKind::File),
                    Some(PathKind::Dir)
                ]
            );
        }

        #[test]
        fn a_runnable_file_is_revealed_never_run() {
            let (d, root) = tree();
            std::fs::write(d.path().join("run.sh"), b"x").unwrap();
            std::fs::write(d.path().join("tool.command"), b"x").unwrap();
            let r = Rec::default();
            for t in ["run.bat", "run.sh", "tool.command"] {
                assert_eq!(
                    open_term_path(&root, t, OpenMode::Open, &r, ""),
                    Some(Opened::Revealed),
                    "{t}"
                );
            }
            assert!(lock(&r.opened).is_empty());
            assert_eq!(
                *lock(&r.revealed),
                vec![
                    format!("{root}/run.bat"),
                    format!("{root}/run.sh"),
                    format!("{root}/tool.command")
                ]
            );
        }

        #[test]
        fn a_folder_uses_open_folder_a_file_never_does() {
            let (_d, root) = tree();
            let r = Rec {
                handles_folders: true,
                ..Rec::default()
            };
            let o = |t: &str, m| open_term_path(&root, t, m, &r, "");
            assert_eq!(o("docs", OpenMode::Open), Some(Opened::Opened));
            assert_eq!(*lock(&r.folders), vec![format!("{root}/docs")]);
            assert_eq!(
                o("docs/sign-off/rapport.pdf", OpenMode::Open),
                Some(Opened::Opened)
            );
            assert_eq!(
                *lock(&r.opened),
                vec![format!("{root}/docs/sign-off/rapport.pdf")]
            );
            assert_eq!(o("run.bat", OpenMode::Open), Some(Opened::Revealed));
            assert_eq!(o("docs", OpenMode::Reveal), Some(Opened::Revealed));
            assert_eq!(lock(&r.folders).len(), 1);
        }

        #[test]
        fn a_folder_the_finder_did_not_take_is_never_handed_to_the_default_verb() {
            let (_d, root) = tree();
            let r = Rec::default();
            assert_eq!(open_term_path(&root, "docs", OpenMode::Open, &r, ""), None);
            assert_eq!(*lock(&r.folders), vec![format!("{root}/docs")]);
            assert!(lock(&r.opened).is_empty());
        }

        #[test]
        fn an_app_bundle_is_a_folder_and_never_opened_by_the_default_verb() {
            // `Evil.app` is a folder to the stat; opening it runs it, so only the folder opener
            // (which reveals a bundle) may take it.
            let (d, root) = tree();
            std::fs::create_dir_all(d.path().join("Evil.app/Contents/MacOS")).unwrap();
            let r = Rec::default();
            assert_eq!(
                open_term_path(&root, "Evil.app", OpenMode::Open, &r, ""),
                None
            );
            assert!(lock(&r.opened).is_empty());
            assert_eq!(*lock(&r.folders), vec![format!("{root}/Evil.app")]);
        }

        #[test]
        fn a_path_under_an_automount_point_is_never_statted() {
            struct Never;
            impl StatFn for Never {
                fn kind(&self, abs: &str) -> Option<PathKind> {
                    panic!("statted {abs}");
                }
            }
            let gate = StatGate::unmetered(Arc::new(Never));
            let texts = vec![
                "/net/evil-host/share/a.txt".to_string(),
                "/Network/Servers/evil/a.txt".to_string(),
                "../../net/evil/a.txt".to_string(),
            ];
            let got = path_kinds("/Users/me", &texts, &gate, "");
            assert!(got.iter().all(Option::is_none));
        }

        #[test]
        fn never_more_than_three_stats_in_flight() {
            let (tx, rx) = mpsc::channel();
            let stat = Arc::new(Blocking {
                active: AtomicUsize::new(0),
                most: AtomicUsize::new(0),
                tokens: Mutex::new(rx),
            });
            let gate = StatGate::new(
                stat.clone(),
                StatGateOptions {
                    slots: 3,
                    ..StatGateOptions::default()
                },
            );
            let texts: Vec<String> = (0..40).map(|i| format!("f{i}.txt")).collect();
            let h = std::thread::spawn(move || path_kinds("/w", &texts, &gate, ""));
            for _ in 0..40 {
                tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(2));
            }
            let got = h.join().unwrap();
            assert_eq!(stat.most.load(Ordering::SeqCst), 3);
            assert_eq!(got.len(), 40);
            assert!(
                got.iter()
                    .all(|h| h.as_ref().map(|h| h.kind) == Some(PathKind::File))
            );
        }

        struct Volumes {
            asked: Mutex<Vec<String>>,
            tokens: Mutex<mpsc::Receiver<()>>,
        }
        impl StatFn for Volumes {
            fn kind(&self, abs: &str) -> Option<PathKind> {
                lock(&self.asked).push(abs.into());
                if abs.starts_with("/Volumes/Share") {
                    let _ = lock(&self.tokens).recv();
                    return None;
                }
                Some(PathKind::File)
            }
        }

        #[test]
        fn a_hanging_volume_answers_nothing_for_a_minute_and_the_disk_still_answers() {
            let (tx, rx) = mpsc::channel();
            let stat = Arc::new(Volumes {
                asked: Mutex::new(vec![]),
                tokens: Mutex::new(rx),
            });
            let now = Arc::new(AtomicU64::new(0));
            let c = now.clone();
            let gate = StatGate::new(
                stat.clone(),
                StatGateOptions {
                    slots: 3,
                    slow_ms: 50,
                    slow_for_ms: 60_000,
                    clock: Arc::new(move || c.load(Ordering::SeqCst)),
                },
            );
            assert_eq!(gate.kind("/Volumes/Share/a.txt"), None);
            let t = Instant::now();
            assert_eq!(gate.kind("/Volumes/share/b.txt"), None);
            assert!(t.elapsed() < Duration::from_millis(20));
            assert_eq!(gate.kind("/Volumes/Other/x.txt"), Some(PathKind::File));
            assert_eq!(gate.kind("/Users/me/local.txt"), Some(PathKind::File));
            assert_eq!(
                *lock(&stat.asked),
                vec![
                    "/Volumes/Share/a.txt".to_string(),
                    "/Volumes/Other/x.txt".to_string(),
                    "/Users/me/local.txt".to_string()
                ]
            );
            now.store(60_000, Ordering::SeqCst);
            assert_eq!(gate.kind("/Volumes/Share/c.txt"), None);
            assert!(lock(&stat.asked).contains(&"/Volumes/Share/c.txt".to_string()));
            for _ in 0..2 {
                let _ = tx.send(());
            }
        }
    }
}
