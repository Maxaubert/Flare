//! The tracker: one worker thread that follows a tab's transcript while a panel wants it and hands the app a snapshot
//! each time something changed. Nothing runs when no tracker exists (the panels cost nothing while closed).
//!
//! The thread owns the tail subscription and the folded [`Convo`]; it builds a [`Snapshot`] (the file list, the
//! summary and, for the selected file, its diff rows) and calls `emit` with it. `emit` runs on the worker,
//! so the app posts it to the UI thread and nothing heavy happens there. A snapshot is built at most about every
//! `WAKE` while news arrives, and again when the selection changes.
//!
//! A replaced or truncated file (`Batch::restarted`) starts the read again from nothing, for every file of the
//! session, so the model never holds lines of a file that no longer says them.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::fold::Convo;
use crate::path::{find_transcript_in, subagent_dir_of};
use crate::tail::{self, Subscription};
use crate::view::{self, FileDiff, FileSummary, Summary};

/// How long the worker waits for a batch before it looks at the selection and the stop flag again.
pub const WAKE: Duration = Duration::from_millis(100);
/// How often a transcript that is not there yet is looked for in the other project folders.
const SEARCH_EVERY: Duration = Duration::from_secs(2);
/// The search backs off (doubling) up to this while the transcript stays missing, so a fresh session that has not
/// written its file yet does not make the worker read every project folder every 2 s for as long as the panel is open.
const SEARCH_MAX: Duration = Duration::from_secs(30);

/// Where to read: the expected transcript, and where to look for it if it is not there.
#[derive(Debug, Clone)]
pub struct Source {
    /// `<claude folder>\projects\<slug>\<session>.jsonl`, as `path::transcript_path_in` made it.
    pub transcript: PathBuf,
    /// Claude's folder, for the search when the transcript is not at `transcript` (the tab's folder moved).
    pub claude_dir: PathBuf,
    pub session: String,
}

/// Everything the panels show, as of one moment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// Counts up with each snapshot of one tracker.
    pub seq: u64,
    /// The transcript file exists (it may still be empty).
    pub found: bool,
    pub files: Vec<FileSummary>,
    pub summary: Summary,
    /// The diff of the file the app selected, with its path as the tracker knows it. None: nothing selected.
    pub diff: Option<(String, FileDiff)>,
}

/// The app's handle on the worker. Dropping it stops the thread within a moment.
pub struct Tracker {
    stop: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    select: Arc<Mutex<Option<String>>>,
    select_gen: Arc<AtomicU64>,
}

impl Tracker {
    /// Starts following `src`. `emit` is called from the worker thread with each snapshot.
    pub fn start(src: Source, emit: Arc<dyn Fn(Snapshot) + Send + Sync>) -> Tracker {
        let stop = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let select = Arc::new(Mutex::new(None));
        let select_gen = Arc::new(AtomicU64::new(0));
        let w = Worker {
            stop: Arc::clone(&stop),
            select: Arc::clone(&select),
            select_gen: Arc::clone(&select_gen),
        };
        let done = Arc::clone(&exited);
        let spawned = std::thread::Builder::new()
            .name("flare-convo-track".into())
            .spawn(move || {
                w.run(src, emit);
                done.store(true, Ordering::SeqCst);
            });
        if let Err(e) = spawned {
            log::warn!("convo: tracker did not start: {e}");
            exited.store(true, Ordering::SeqCst);
        }
        Tracker {
            stop,
            exited,
            select,
            select_gen,
        }
    }

    /// Asks for the diff of `path` in the snapshots from now on (None: no diff). The next snapshot carries it.
    pub fn select(&self, path: Option<String>) {
        if let Ok(mut s) = self.select.lock() {
            if *s == path {
                return;
            }
            *s = path;
        }
        self.select_gen.fetch_add(1, Ordering::SeqCst);
    }

    /// True once the worker thread has returned (a test reads it to see that a dropped tracker really stops).
    #[doc(hidden)]
    pub fn exit_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.exited)
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct Worker {
    stop: Arc<AtomicBool>,
    select: Arc<Mutex<Option<String>>>,
    select_gen: Arc<AtomicU64>,
}

/// A diff already built, with what it was built from, so a snapshot caused by something else does not build it again.
struct DiffCache {
    path: String,
    edits_sig: u64,
    diff: FileDiff,
}

impl Worker {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn selection(&self) -> Option<String> {
        self.select.lock().ok().and_then(|s| s.clone())
    }

    fn subscribe(path: &std::path::Path) -> Subscription {
        match subagent_dir_of(path) {
            Some(dir) => tail::subscribe_session(path.to_path_buf(), dir),
            None => tail::subscribe(path.to_path_buf()),
        }
    }

    fn run(self, src: Source, emit: Arc<dyn Fn(Snapshot) + Send + Sync>) {
        let mut path = src.transcript.clone();
        let mut sub = Self::subscribe(&path);
        let mut convo = Convo::new();
        let mut found = false;
        let mut last_search = Instant::now();
        let mut search_every = SEARCH_EVERY;
        let mut checked_found = Instant::now() - SEARCH_EVERY;
        let mut cache: Option<DiffCache> = None;
        let mut seq = 0u64;
        let mut seen_select = self.select_gen.load(Ordering::SeqCst);
        let mut dirty = true; // the first snapshot goes out even when the transcript is empty
        while !self.stopped() {
            // A transcript that is not there yet: look at it, and now and then look in the other folders.
            if !found && checked_found.elapsed() >= Duration::from_millis(400) {
                checked_found = Instant::now();
                found = path.is_file(); // sync-ok: runs on the tracker worker
                if found {
                    dirty = true;
                } else if last_search.elapsed() >= search_every {
                    last_search = Instant::now();
                    search_every = (search_every * 2).min(SEARCH_MAX);
                    if let Some(other) = find_transcript_in(&src.claude_dir, &src.session)
                        && other != path
                    {
                        path = other;
                        sub = Self::subscribe(&path);
                        found = true;
                        dirty = true;
                    }
                }
            }
            // One wait, then everything that is already queued.
            let mut next = sub.next_timeout(WAKE);
            let mut restarted = false;
            let burst = Instant::now();
            while let Some(batch) = next {
                if batch.restarted {
                    restarted = true;
                } else {
                    for line in &batch.lines {
                        convo.fold_from(&batch.source, line);
                    }
                    if !batch.lines.is_empty() {
                        dirty = true;
                        found = true;
                    }
                }
                if self.stopped() || burst.elapsed() >= WAKE {
                    break;
                }
                next = sub.try_next();
            }
            if restarted {
                // The file was replaced or cut: read every file again from nothing.
                sub = Self::subscribe(&path);
                convo = Convo::new();
                cache = None;
                dirty = true;
            }
            let sel_gen = self.select_gen.load(Ordering::SeqCst);
            if sel_gen != seen_select {
                seen_select = sel_gen;
                dirty = true;
            }
            if !dirty || self.stopped() {
                continue;
            }
            dirty = false;
            seq += 1;
            let snap = self.snapshot(seq, found, &convo, &mut cache);
            emit(snap);
        }
    }

    fn snapshot(
        &self,
        seq: u64,
        found: bool,
        convo: &Convo,
        cache: &mut Option<DiffCache>,
    ) -> Snapshot {
        let files = view::files(convo);
        let diff = self.selection().map(|path| {
            let sig = edits_signature(convo, &path);
            if let Some(c) = cache.as_ref()
                && c.path == path
                && c.edits_sig == sig
            {
                return (path, c.diff.clone());
            }
            let diff = view::file_diff(convo, &path);
            *cache = Some(DiffCache {
                path: path.clone(),
                edits_sig: sig,
                diff: diff.clone(),
            });
            (path, diff)
        });
        Snapshot {
            seq,
            found,
            summary: view::summary(convo),
            files,
            diff,
        }
    }
}

/// A number that changes whenever an edit of `path` is added, answered or dropped.
fn edits_signature(convo: &Convo, path: &str) -> u64 {
    let want = path.replace('/', "\\").to_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for e in &convo.edits {
        if e.path.replace('/', "\\").to_lowercase() == want {
            e.id.bytes().for_each(&mut mix);
            mix(e.outcome as u8);
            mix(0xff);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;

    const SID: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";

    fn edit_line(id: &str, path: &str, old: &str, new: &str) -> String {
        serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use",
            "id": id, "name": "Edit",
            "input": {"file_path": path, "old_string": old, "new_string": new}}]}})
        .to_string()
    }

    fn append(path: &std::path::Path, lines: &[String]) {
        let mut f = std::fs::OpenOptions::new() // sync-ok: test code only
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    struct Rig {
        _dir: tempfile::TempDir,
        file: PathBuf,
        claude: PathBuf,
        rx: mpsc::Receiver<Snapshot>,
        tracker: Tracker,
    }

    fn rig(create: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        let file = claude
            .join("projects")
            .join("C--p")
            .join(format!("{SID}.jsonl"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap(); // sync-ok: test code only
        if create {
            std::fs::write(&file, "").unwrap(); // sync-ok: test code only
        }
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let tracker = Tracker::start(
            Source {
                transcript: file.clone(),
                claude_dir: claude.clone(),
                session: SID.to_string(),
            },
            Arc::new(move |s| {
                let _ = tx.lock().unwrap().send(s);
            }),
        );
        Rig {
            _dir: dir,
            file,
            claude,
            rx,
            tracker,
        }
    }

    /// The first snapshot that satisfies `ok`, within 10 s.
    fn until(rx: &mpsc::Receiver<Snapshot>, ok: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            // sync-ok: test code only
            let s = rx.recv_timeout(left).expect("a snapshot in time");
            if ok(&s) {
                return s;
            }
        }
    }

    #[test]
    fn an_empty_transcript_gives_a_first_snapshot_and_news_gives_more() {
        let r = rig(true);
        let first = until(&r.rx, |_| true);
        assert!(first.files.is_empty());
        append(&r.file, &[edit_line("e1", r"C:\p\a.rs", "x", "y")]);
        let s = until(&r.rx, |s| !s.files.is_empty());
        assert!(s.found);
        assert_eq!(s.files[0].name, "a.rs");
        assert_eq!(s.summary.edits, 1);
        assert!(s.diff.is_none(), "nothing is selected");
        assert!(s.seq > first.seq);
    }

    #[test]
    fn selecting_a_file_adds_its_diff_and_a_new_edit_updates_it() {
        let r = rig(true);
        append(&r.file, &[edit_line("e1", r"C:\p\a.rs", "x", "y")]);
        until(&r.rx, |s| s.files.len() == 1);
        r.tracker.select(Some(r"C:\p\a.rs".to_string()));
        let s = until(&r.rx, |s| s.diff.is_some());
        let (path, diff) = s.diff.unwrap();
        assert_eq!(path, r"C:\p\a.rs");
        assert!(!diff.rows.is_empty());
        let before = diff.rows.len();
        append(&r.file, &[edit_line("e2", r"C:\p\a.rs", "y", "z")]);
        let s = until(&r.rx, |s| {
            s.diff.as_ref().is_some_and(|(_, d)| d.rows.len() > before)
        });
        assert_eq!(s.files[0].edits, 2);
        r.tracker.select(None);
        until(&r.rx, |s| s.diff.is_none());
    }

    #[test]
    fn a_transcript_that_appears_later_is_picked_up() {
        let r = rig(false);
        let first = until(&r.rx, |_| true);
        assert!(!first.found, "no file yet");
        append(&r.file, &[edit_line("e1", r"C:\p\a.rs", "x", "y")]);
        let s = until(&r.rx, |s| s.found && s.files.len() == 1);
        assert_eq!(s.files[0].edits, 1);
    }

    #[test]
    fn a_transcript_in_another_project_folder_is_found() {
        let r = rig(false);
        until(&r.rx, |_| true);
        let other = r.claude.join("projects").join("D--elsewhere");
        std::fs::create_dir_all(&other).unwrap(); // sync-ok: test code only
        append(
            &other.join(format!("{SID}.jsonl")),
            &[edit_line("e1", r"C:\p\moved.rs", "x", "y")],
        );
        let s = until(&r.rx, |s| s.files.len() == 1);
        assert_eq!(s.files[0].name, "moved.rs");
    }

    #[test]
    fn a_replaced_transcript_starts_the_model_again() {
        let r = rig(true);
        append(
            &r.file,
            &[
                edit_line("e1", r"C:\p\a.rs", "x", "y"),
                edit_line("e2", r"C:\p\b.rs", "x", "y"),
            ],
        );
        until(&r.rx, |s| s.files.len() == 2);
        // A shorter file in its place.
        std::fs::write(
            &r.file,
            format!("{}\n", edit_line("n1", r"C:\p\c.rs", "x", "y")),
        )
        .unwrap(); // sync-ok: test code only
        let s = until(&r.rx, |s| s.files.len() == 1);
        assert_eq!(s.files[0].name, "c.rs");
    }

    #[test]
    fn subagent_edits_arrive_from_their_own_file() {
        let r = rig(true);
        let sub = r.file.with_extension("").join("subagents");
        std::fs::create_dir_all(&sub).unwrap(); // sync-ok: test code only
        append(
            &sub.join("agent-abc1234.jsonl"),
            &[edit_line("s1", r"C:\p\side.rs", "x", "y")],
        );
        let s = until(&r.rx, |s| s.files.len() == 1);
        assert_eq!(s.files[0].name, "side.rs");
        assert!(s.files[0].by_subagent);
    }

    #[test]
    fn dropping_the_tracker_stops_the_worker() {
        let r = rig(true);
        until(&r.rx, |_| true);
        let exited = r.tracker.exit_flag();
        drop(r.tracker);
        let end = Instant::now() + Duration::from_secs(5);
        while !exited.load(Ordering::SeqCst) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: test code only
        }
        assert!(exited.load(Ordering::SeqCst), "the worker returned");
    }
}
