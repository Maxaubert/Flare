//! The follower (plugins phase 5): one worker thread that follows a session's transcript from where it ends now and
//! hands over each new entry ([`crate::said`]) as it is written. The app starts one per tab only while a plugin that
//! holds `sessions` listens to that tab's session, and drops it when nobody does: nothing runs otherwise.
//!
//! What was said before the follower started is not news (a plugin reads it with `session.read`): the tail starts at
//! the file's end. A transcript that is not there yet is waited for (and looked for in the other project folders, as
//! the tracker does); once it appears, all of it is new. A file that is rewritten or cut is followed from its new end:
//! its old lines are never handed over a second time. A batch holds at most [`MAX_BATCH`] entries (the newest); the
//! rest are counted in `skipped`.
//!
//! `emit` runs on the worker; the app posts what it gets to the UI thread. Nothing here touches the UI thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::path::find_transcript_in;
use crate::said::{Entry, entries_of};
use crate::tail::Tail;
use crate::track::Source;

/// How often the file is read while the follower runs.
pub const POLL: Duration = Duration::from_millis(250);
/// The most entries one batch hands over; older ones of a bigger batch are skipped and counted.
pub const MAX_BATCH: usize = 200;
/// How often a transcript that is not there yet is looked for in the other project folders, at first.
const SEARCH_EVERY: Duration = Duration::from_secs(2);
/// The search backs off (doubling) up to this while the transcript stays missing.
const SEARCH_MAX: Duration = Duration::from_secs(30);

/// What the follower hands over: new entries, oldest first, and how many it skipped since the last batch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Said {
    pub entries: Vec<Entry>,
    pub skipped: u64,
}

/// The app's handle on the worker. Dropping it stops the thread within a moment.
pub struct Follower {
    stop: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
}

impl Follower {
    /// Starts following `src` from the end of its transcript. `emit` is called from the worker thread.
    pub fn start(src: Source, emit: Arc<dyn Fn(Said) + Send + Sync>) -> Follower {
        let stop = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let (flag, done) = (Arc::clone(&stop), Arc::clone(&exited));
        let spawned = std::thread::Builder::new()
            .name("flare-convo-follow".into())
            .spawn(move || {
                run(src, &emit, &flag);
                done.store(true, Ordering::SeqCst);
            });
        if let Err(e) = spawned {
            log::warn!("convo: follower did not start: {e}");
            exited.store(true, Ordering::SeqCst);
        }
        Follower { stop, exited }
    }

    /// True once the worker thread has returned (a test reads it to see that a dropped follower really stops).
    #[doc(hidden)]
    pub fn exit_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.exited)
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Sleeps for `total` in short slices so a stop is seen quickly. False once stopped.
fn nap(total: Duration, stop: &AtomicBool) -> bool {
    let slice = Duration::from_millis(25);
    let mut slept = Duration::ZERO;
    while slept < total {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(slice); // sync-ok: the follower's own thread
        slept += slice;
    }
    !stop.load(Ordering::SeqCst)
}

/// Waits for the transcript: the expected file, or the same session in another project folder. Gives the tail to
/// read it with: from its start when the file appeared after the follower started, from its end when it was there
/// already. None once stopped.
fn find(src: &Source, stop: &AtomicBool) -> Option<Tail> {
    if src.transcript.is_file() {
        // sync-ok: the follower's own thread
        let mut tail = Tail::new(src.transcript.clone());
        if let Err(e) = tail.skip_to_end() {
            log::debug!("convo: the transcript's end could not be read: {e}");
        }
        return Some(tail);
    }
    let mut last_search = Instant::now();
    let mut every = SEARCH_EVERY;
    loop {
        if !nap(POLL, stop) {
            return None;
        }
        if src.transcript.is_file() {
            // sync-ok: the follower's own thread
            // Written since the follower started: every line of it is new.
            return Some(Tail::new(src.transcript.clone()));
        }
        if last_search.elapsed() >= every {
            last_search = Instant::now();
            every = (every * 2).min(SEARCH_MAX);
            if let Some(other) = find_transcript_in(&src.claude_dir, &src.session) {
                let mut tail = Tail::new(PathBuf::from(&other));
                if let Err(e) = tail.skip_to_end() {
                    log::debug!("convo: the transcript's end could not be read: {e}");
                }
                return Some(tail);
            }
        }
    }
}

fn run(src: Source, emit: &Arc<dyn Fn(Said) + Send + Sync>, stop: &AtomicBool) {
    let Some(mut tail) = find(&src, stop) else {
        return;
    };
    let mut skipped = 0u64;
    while !stop.load(Ordering::SeqCst) {
        let batch = match tail.poll() {
            Ok(b) => b,
            Err(e) => {
                log::debug!("convo: transcript read failed, retrying: {e}");
                if !nap(POLL, stop) {
                    return;
                }
                continue;
            }
        };
        if batch.restarted {
            // Rewritten or cut: what it holds now is not news. Follow it from its new end.
            if let Err(e) = tail.skip_to_end() {
                log::debug!("convo: the transcript's end could not be read: {e}");
            }
            continue;
        }
        let mut entries: Vec<Entry> = batch.lines.iter().flat_map(|l| entries_of(l)).collect();
        if entries.len() > MAX_BATCH {
            let over = entries.len() - MAX_BATCH;
            skipped += over as u64;
            entries.drain(..over);
        }
        if !entries.is_empty() {
            emit(Said {
                entries,
                skipped: std::mem::take(&mut skipped),
            });
        }
        if !batch.more && !nap(POLL, stop) {
            return;
        }
    }
}
