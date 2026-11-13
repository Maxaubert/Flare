//! The conversation of a remote session (remote sessions PR 4, rule 297): what is said in a host tab's Claude session
//! crosses the link as numbered `Entry` values, never as a host path or a file.
//!
//! **The host** (`ConvoFeeds`, kept in the `RemoteHost`) keeps one feed a followed tab: the transcript's last
//! [`CONVO_MAX`] entries, read once from its end (`said::read_last_until`), then what is written after them, read by a
//! `Tail` at each `remote.convo` call on the control channel's worker (rule 34). Every entry gets the next number of
//! that tab; the numbers only grow while the host runs. The transcript is found on the host from the tab's session and
//! folder with the host's own Claude folder (`CLAUDE_CONFIG_DIR` or `~/.claude`), so a Mac's slug and a PC's slug are
//! each read where they belong.
//!
//! An answer is the entries after the viewer's `since`, and `reset` when what the viewer holds no longer fits: the
//! transcript was cut or rewritten, the tab's session changed, the viewer fell behind the kept window, or its number is
//! one this host never gave (the host started again). Then the answer is the whole window and the viewer starts over.
//!
//! **The viewer** (`ConvoMirror`) keeps what it was given and takes each entry once by its number, so a poll sent again
//! after a reconnect adds nothing twice.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use flare_convo::said::{Entry, Role, entries_of, read_last_until};
use flare_convo::tail::Tail;
use flare_ctl::remote::CONVO_MAX;
use serde_json::{Value, json};

fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The polls one call reads at most, so a transcript that grows fast never holds the worker long.
const POLLS_PER_CALL: usize = 4;

/// What one `remote.convo` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConvoAnswer {
    /// Entries after the viewer's number (all of the window on a reset), oldest first, with their numbers.
    pub entries: Vec<(u64, Entry)>,
    /// The viewer drops what it holds and starts over from these entries.
    pub reset: bool,
    /// The last number given so far (0: none yet).
    pub to: u64,
}

impl ConvoAnswer {
    /// The wire form: `{entries: [{seq, role, text, tool?, at?}], reset, to}`.
    pub fn to_json(&self, tab: &str) -> Value {
        let entries: Vec<Value> = self
            .entries
            .iter()
            .map(|(seq, e)| {
                let mut v = crate::plugin_sessions::entry_json(e);
                v["seq"] = json!(seq);
                v
            })
            .collect();
        json!({"tab": tab, "entries": entries, "reset": self.reset, "to": self.to})
    }
}

/// One tab's numbered conversation on the host.
struct Feed {
    session: String,
    path: PathBuf,
    tail: Tail,
    /// The kept window: the last [`CONVO_MAX`] entries with their numbers.
    kept: VecDeque<(u64, Entry)>,
    /// The number the next entry gets.
    next: u64,
    /// The first number of what the transcript holds now: a viewer that holds an older one holds what was cut or
    /// rewritten, or another session's words.
    floor: u64,
    /// The transcript was read once.
    started: bool,
}

impl Feed {
    fn new(session: &str, path: &Path) -> Feed {
        Feed {
            session: session.to_string(),
            path: path.to_path_buf(),
            tail: Tail::new(path),
            kept: VecDeque::new(),
            next: 1,
            floor: 1,
            started: false,
        }
    }

    /// Reads the transcript again from its last entries (a new feed, a rewrite, another session). The numbers go on
    /// from where they were: what a viewer holds is from before, and `floor` says so. BLOCKING (a file read).
    fn rebuild(&mut self, session: &str, path: &Path) -> std::io::Result<()> {
        self.session = session.to_string();
        self.path = path.to_path_buf();
        self.tail = Tail::new(path);
        self.started = true;
        self.tail.skip_to_end()?;
        let back = read_last_until(path, CONVO_MAX, self.tail.offset())?;
        self.kept.clear();
        self.floor = self.next;
        for e in back {
            self.push(e);
        }
        Ok(())
    }

    fn push(&mut self, e: Entry) {
        self.kept.push_back((self.next, e));
        self.next += 1;
        while self.kept.len() > CONVO_MAX {
            self.kept.pop_front();
        }
    }

    /// What was written since the last read. A transcript cut or replaced is read again from its end. BLOCKING.
    fn pull(&mut self) -> std::io::Result<()> {
        for _ in 0..POLLS_PER_CALL {
            let batch = self.tail.poll()?;
            if batch.restarted {
                let (session, path) = (self.session.clone(), self.path.clone());
                return self.rebuild(&session, &path);
            }
            for line in &batch.lines {
                for e in entries_of(line) {
                    self.push(e);
                }
            }
            if !batch.more {
                break;
            }
        }
        Ok(())
    }

    /// The answer for a viewer that holds everything up to `since` (0: nothing).
    fn after(&self, since: u64) -> ConvoAnswer {
        let to = self.next - 1;
        let first = self.kept.front().map_or(self.next, |(s, _)| *s);
        let reset = since > to || (since > 0 && since < self.floor) || since + 1 < first;
        let entries = self
            .kept
            .iter()
            .filter(|(s, _)| reset || *s > since)
            .cloned()
            .collect();
        ConvoAnswer { entries, reset, to }
    }
}

/// The host's numbered conversations, one a followed tab. Any worker thread: the map's lock is short, and a feed's own
/// lock is held only by the call that reads its file.
#[derive(Default)]
pub struct ConvoFeeds {
    feeds: Mutex<HashMap<String, Arc<Mutex<Feed>>>>,
}

impl ConvoFeeds {
    pub fn new() -> Self {
        Self::default()
    }

    /// `remote.convo {tab, since}` for a tab whose Claude session is `session`, its transcript at `path` on this host.
    /// BLOCKING (reads the transcript): the control channel's worker only.
    pub fn read(
        &self,
        tab: &str,
        session: &str,
        path: &Path,
        since: u64,
    ) -> std::io::Result<ConvoAnswer> {
        let feed = {
            let mut map = lk(&self.feeds);
            Arc::clone(
                map.entry(tab.to_string())
                    .or_insert_with(|| Arc::new(Mutex::new(Feed::new(session, path)))),
            )
        };
        let mut f = lk(&feed);
        if !f.started || f.session != session || f.path != path {
            // A new feed, another session in the tab, or its transcript moved: read from its last entries.
            f.rebuild(session, path)?;
        } else {
            f.pull()?;
        }
        Ok(f.after(since))
    }

    /// The tab has no Claude session (any more): its feed goes, and a viewer holding words starts over.
    pub fn none(&self, tab: &str, since: u64) -> ConvoAnswer {
        let gone = lk(&self.feeds).remove(tab);
        let to = gone.map_or(0, |f| lk(&f).next - 1);
        ConvoAnswer {
            entries: Vec::new(),
            reset: since > 0,
            to,
        }
    }

    /// Nobody follows the tab any more, or it closed: its feed goes.
    pub fn forget(&self, tab: &str) {
        lk(&self.feeds).remove(tab);
    }
}

/// The viewer's copy of a remote tab's conversation: each entry once, by its host number (rule 297).
#[allow(dead_code)] // PR 7's fitted conversation view polls `remote.convo` into it; its tests hold it now
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConvoMirror {
    entries: Vec<(u64, Entry)>,
    /// The last number taken (0: none): the next `remote.convo` asks from it.
    last: u64,
}

#[allow(dead_code)] // PR 7's fitted conversation view
impl ConvoMirror {
    pub fn new() -> Self {
        Self::default()
    }

    /// The number to ask from.
    pub fn since(&self) -> u64 {
        self.last
    }

    pub fn entries(&self) -> &[(u64, Entry)] {
        &self.entries
    }

    /// Takes one answer: a reset starts over; otherwise an entry whose number was taken already is left out. The
    /// mirror keeps at most [`CONVO_MAX`] entries, the last ones. True when what it shows changed.
    pub fn apply(&mut self, answer: &Value) -> bool {
        let reset = answer["reset"].as_bool() == Some(true);
        let mut changed = false;
        if reset {
            changed = !self.entries.is_empty();
            self.entries.clear();
            self.last = 0;
        }
        for v in answer["entries"].as_array().into_iter().flatten() {
            let Some(seq) = v["seq"].as_u64() else {
                continue;
            };
            if seq <= self.last {
                continue;
            }
            let Some(e) = entry_of(v) else { continue };
            self.entries.push((seq, e));
            self.last = seq;
            changed = true;
        }
        if self.entries.len() > CONVO_MAX {
            let over = self.entries.len() - CONVO_MAX;
            self.entries.drain(..over);
        }
        changed
    }
}

#[allow(dead_code)] // PR 7's fitted conversation view
/// One entry as the wire carries it, or None for a role this end does not know (rule 40: read as nothing).
fn entry_of(v: &Value) -> Option<Entry> {
    let role = match v["role"].as_str()? {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => return None,
    };
    Some(Entry {
        role,
        text: v["text"].as_str().unwrap_or_default().to_string(),
        tool: v["tool"].as_str().map(str::to_string),
        at: v["at"].as_str().map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn user(text: &str) -> String {
        json!({"type": "user", "message": {"role": "user", "content": text}}).to_string()
    }

    fn append(path: &Path, lines: &[String]) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    fn texts(a: &ConvoAnswer) -> Vec<(u64, String)> {
        a.entries
            .iter()
            .map(|(s, e)| (*s, e.text.clone()))
            .collect()
    }

    const S1: &str = "11111111-1111-4111-8111-111111111111";
    const S2: &str = "22222222-2222-4222-8222-222222222222";

    #[test]
    fn the_host_numbers_the_backfill_then_each_new_entry_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        append(&path, &[user("one"), user("two")]);
        let feeds = ConvoFeeds::new();
        let a = feeds.read("t1", S1, &path, 0).unwrap();
        assert_eq!(texts(&a), [(1, "one".into()), (2, "two".into())]);
        assert!(!a.reset);
        assert_eq!(a.to, 2);
        append(&path, &[user("three")]);
        let b = feeds.read("t1", S1, &path, 2).unwrap();
        assert_eq!(texts(&b), [(3, "three".into())]);
        assert!(!b.reset);
        // Asked again from the same number (a poll sent again after a reconnect): the same entries, the same numbers.
        let again = feeds.read("t1", S1, &path, 2).unwrap();
        assert_eq!(texts(&again), [(3, "three".into())]);
        assert!(feeds.read("t1", S1, &path, 3).unwrap().entries.is_empty());
    }

    #[test]
    fn a_reconnect_with_since_adds_no_entry_twice_on_the_viewer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        append(&path, &[user("one"), user("two")]);
        let feeds = ConvoFeeds::new();
        let mut mirror = ConvoMirror::new();
        assert!(mirror.apply(&feeds.read("t", S1, &path, 0).unwrap().to_json("t")));
        // The answer to the next poll is lost with the link; the viewer asks again from what it holds.
        append(&path, &[user("three")]);
        let _lost = feeds.read("t", S1, &path, mirror.since()).unwrap();
        let answer = feeds
            .read("t", S1, &path, mirror.since())
            .unwrap()
            .to_json("t");
        assert!(mirror.apply(&answer));
        assert!(!mirror.apply(&answer), "the same answer twice adds nothing");
        let said: Vec<&str> = mirror
            .entries()
            .iter()
            .map(|(_, e)| e.text.as_str())
            .collect();
        assert_eq!(said, ["one", "two", "three"]);
        assert_eq!(mirror.since(), 3);
    }

    #[test]
    fn a_cut_transcript_resets_the_viewer_and_the_numbers_go_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        append(&path, &[user("one"), user("two"), user("three")]);
        let feeds = ConvoFeeds::new();
        let mut mirror = ConvoMirror::new();
        mirror.apply(&feeds.read("t", S1, &path, 0).unwrap().to_json("t"));
        // Rewritten shorter (a compaction or another writer): what the viewer holds is not what the file says now.
        std::fs::write(&path, format!("{}\n", user("fresh"))).unwrap();
        let a = feeds.read("t", S1, &path, mirror.since()).unwrap();
        assert!(a.reset, "a cut transcript is a reset");
        assert_eq!(texts(&a), [(4, "fresh".into())]);
        assert!(mirror.apply(&a.to_json("t")));
        let said: Vec<&str> = mirror
            .entries()
            .iter()
            .map(|(_, e)| e.text.as_str())
            .collect();
        assert_eq!(said, ["fresh"]);
        // A viewer that already holds the new words is not reset again.
        assert!(!feeds.read("t", S1, &path, 4).unwrap().reset);
    }

    #[test]
    fn another_session_or_a_number_this_host_never_gave_is_a_reset() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.jsonl");
        let p2 = dir.path().join("b.jsonl");
        append(&p1, &[user("old one")]);
        append(&p2, &[user("new one")]);
        let feeds = ConvoFeeds::new();
        assert_eq!(feeds.read("t", S1, &p1, 0).unwrap().to, 1);
        let other = feeds.read("t", S2, &p2, 1).unwrap();
        assert!(other.reset);
        assert_eq!(texts(&other), [(2, "new one".into())]);
        // The host started again: its numbers begin at 1, the viewer's 9 is not one of them.
        let fresh = ConvoFeeds::new();
        let a = fresh.read("t", S2, &p2, 9).unwrap();
        assert!(a.reset);
        assert_eq!(texts(&a), [(1, "new one".into())]);
        // No session any more: the viewer's words go.
        let none = fresh.none("t", 1);
        assert!(none.reset && none.entries.is_empty());
        assert!(!fresh.none("t", 0).reset);
    }

    #[test]
    fn a_viewer_behind_the_kept_window_starts_over_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        append(&path, &[user("first")]);
        let feeds = ConvoFeeds::new();
        assert_eq!(feeds.read("t", S1, &path, 0).unwrap().to, 1);
        let many: Vec<String> = (0..CONVO_MAX + 10)
            .map(|i| user(&format!("m{i}")))
            .collect();
        append(&path, &many);
        let a = feeds.read("t", S1, &path, 1).unwrap();
        assert!(a.reset, "entry 2 fell out of the window");
        assert_eq!(a.entries.len(), CONVO_MAX);
        assert_eq!(a.entries.last().map(|(s, _)| *s), Some(a.to));
    }

    #[test]
    fn the_wire_form_carries_numbers_and_no_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        append(&path, &[user("hello")]);
        let v = ConvoFeeds::new()
            .read("t", S1, &path, 0)
            .unwrap()
            .to_json("t");
        assert_eq!(v["entries"][0]["seq"], 1);
        assert_eq!(v["entries"][0]["role"], "user");
        assert_eq!(v["entries"][0]["text"], "hello");
        let wire = v.to_string();
        assert!(!wire.contains(&dir.path().to_string_lossy().replace('\\', "\\\\")));
        assert!(!wire.contains("s.jsonl"));
        // A role this end does not know is read as nothing.
        let mut m = ConvoMirror::new();
        assert!(!m.apply(
            &json!({"entries": [{"seq": 1, "role": "system", "text": "x"}], "reset": false})
        ));
        assert_eq!(m.since(), 0);
    }
}
