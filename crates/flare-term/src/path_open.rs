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
const TEXT_MAX: usize = 1024;
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

const RUNNABLE: &[&str] = &[
    "exe",
    "com",
    "scr",
    "pif",
    "cpl",
    "msc",
    "msi",
    "msp",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "appref-ms",
    "application",
    "gadget",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "psd1",
    "ps1xml",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "ws",
    "wsc",
    "sct",
    "hta",
    "jar",
    "py",
    "pyw",
    "pyc",
    "pyz",
    "rb",
    "pl",
    "sh",
    "bash",
    "ahk",
    "au3",
    "lnk",
    "url",
    "website",
    "reg",
    "inf",
    "ins",
    "isp",
    "scf",
    "shb",
    "shs",
    "library-ms",
    "settingcontent-ms",
    "search-ms",
    "diagcab",
    "xbap",
    "vsto",
];

/// True when Windows would run this file (a program, a script, a shortcut, an installer).
pub fn is_runnable(file: &str) -> bool {
    let name = file.rsplit(['\\', '/']).next().unwrap_or(file);
    match name.rfind('.') {
        Some(i) => {
            let ext = name[i + 1..].to_lowercase();
            RUNNABLE.contains(&ext.as_str())
        }
        None => false,
    }
}

fn has_drive_root(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// '/' -> '\', collapse repeated separators, resolve "." and ".." (never above the drive root),
/// keep a trailing separator, keep the drive letter's case. `p` starts with `X:`.
fn normalise_win(p: &str) -> String {
    let drive: String = p.chars().take(2).collect();
    let rest: String = p.chars().skip(2).collect();
    let mut parts: Vec<&str> = Vec::new();
    for seg in rest.split(is_sep) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut out = format!("{drive}\\{}", parts.join("\\"));
    if !parts.is_empty() && rest.ends_with(is_sep) {
        out.push('\\');
    }
    out
}

/// What `text` names from inside `cwd`, or None when it names nothing this checks: a network path,
/// a NUL, a relative path with no folder to be relative to, an over-long text.
pub fn resolve_term_path(cwd: &str, text: &str, home: &str) -> Option<String> {
    if text.is_empty() || text.chars().count() > TEXT_MAX || text.contains('\0') {
        return None;
    }
    let mut p = text.to_string();
    if p == "~" || p.starts_with("~/") || p.starts_with("~\\") {
        p = format!("{home}{}", &p[1..]);
    }
    let mut it = p.chars();
    if it.next().is_some_and(is_sep) && it.next().is_some_and(is_sep) {
        return None;
    }
    if has_drive_root(&p) {
        return Some(normalise_win(&p));
    }
    if p.starts_with(is_sep) {
        if !has_drive_root(cwd) {
            return None;
        }
        let drive: String = cwd.chars().take(2).collect();
        return Some(normalise_win(&format!("{drive}{p}")));
    }
    if !has_drive_root(cwd) || cwd.contains('\0') {
        return None;
    }
    Some(normalise_win(&format!("{cwd}\\{p}")))
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
        let drive: String = abs.chars().take(2).collect::<String>().to_lowercase();
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

pub trait PathOpeners {
    /// The app Windows gives it.
    fn open_path(&self, abs: &str);
    /// A folder through Windows' default verb for folders; false = not handled.
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
/// Clicking text must never start a program: a runnable file is revealed instead.
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
    if mode == OpenMode::Reveal || (hit.kind == PathKind::File && is_runnable(&hit.abs)) {
        openers.reveal_path(&hit.abs);
        return Some(Opened::Revealed);
    }
    if hit.kind == PathKind::Dir && openers.open_folder(&hit.abs) {
        return Some(Opened::Opened);
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
        let r = Rec::default();
        let o = |t: &str| open_term_path(&root, t, OpenMode::Open, &r, "");
        assert_eq!(o("docs/sign-off/rapport.pdf"), Some(Opened::Opened));
        assert_eq!(o("docs"), Some(Opened::Opened));
        assert_eq!(lock(&r.opened).len(), 2);
        assert_eq!(lock(&r.revealed).len(), 0);
    }

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
    fn is_runnable_knows_programs_not_documents() {
        for f in [
            "a.exe", "b.BAT", "c.ps1", "d.lnk", "e.msi", "f.js", "g.vbs", "h.py", "i.url", "j.reg",
        ] {
            assert!(is_runnable(f), "{f}");
        }
        for f in [
            "a.pdf", "b.md", "c.png", "d.txt", "e.docx", "f.ts", "g.json", "README",
        ] {
            assert!(!is_runnable(f), "{f}");
        }
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

    struct Drives {
        asked: Mutex<Vec<String>>,
        tokens: Mutex<mpsc::Receiver<()>>,
    }
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
}
