//! The agent kinds in the diagnostics log (port of Prism Terminal's `core/renderer/lib/agentDiag.ts`,
//! #152). Bugs like #151 (restored tabs showed Finished after a restart) could only be guessed at
//! before this log existed, so it says, at the quiet level and always on:
//!
//! - `agent-hook`: each hook signal the tab received (state, and a failure's kind, one of Claude's own
//!   or `other`). A run of the SAME state on a tab is folded: the first line is written at once, the
//!   rest are counted and written as ONE line with `repeats` when the state changes or the tab closes,
//!   and otherwise at most every `HOOK_FLUSH_MS`.
//! - `agent-title`: the agent title's MEANING (`idle`, `working`, `question`, `none`), only on a change.
//!   `none` is written only once it has held `TITLE_NONE_MS`.
//! - `agent-mark`: each change of the mark a tab's indicator holds, with `held` and the rule `why`.
//! - `agent-restore`: a tab restored over an agent conversation; a mark within `RESTORED_MS` carries
//!   `restored` (ms since).
//!
//! Every tab has at most `AGENT_LINES_PER_WINDOW` such lines per `AGENT_WINDOW_MS`; the next line
//! written says `dropped`. NEVER typed text, screen text, a title's text or anything of the
//! conversation (rule 110): states, rule names, tab ids and the folder.
//!
//! Time is passed in: `tick(now)` replaces the page's timers and `next_due()` says when the glue
//! must call it next.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// A hook run held this long is said (its repeats), while the state goes on folding.
pub const HOOK_FLUSH_MS: u64 = 30_000;
/// The same state after this much silence starts a new line.
pub const HOOK_GAP_MS: u64 = 60_000;
/// How often the record looks for held repeats to say.
pub const SWEEP_MS: u64 = 5_000;
/// `none` is written only once it has held this long (a child process's title is not an agent's).
pub const TITLE_NONE_MS: u64 = 2_000;
pub const AGENT_WINDOW_MS: u64 = 10_000;
pub const AGENT_LINES_PER_WINDOW: u32 = 40;
/// A rule noted longer ago than this belongs to no change.
pub const WHY_MS: u64 = 1_000;
/// A mark within this long of a restore carries `restored`.
pub const RESTORED_MS: u64 = 60_000;

/// The fields a diagnostics line may carry.
#[derive(Clone, Debug, PartialEq)]
pub enum DiagValue {
    Str(String),
    Num(u64),
    Bool(bool),
    List(Vec<String>),
}

/// Where lines go. M9's `diag.jsonl` writer implements it; until then `NullSink` (the app) and
/// `MemorySink` (the tests).
pub trait DiagSink: Send + Sync {
    fn record(&self, kind: &str, fields: &[(&str, DiagValue)], at_ms: u64);
}

/// Drops every line.
#[derive(Default)]
pub struct NullSink;

impl DiagSink for NullSink {
    fn record(&self, _kind: &str, _fields: &[(&str, DiagValue)], _at_ms: u64) {}
}

/// One recorded line.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub k: String,
    pub fields: Vec<(String, DiagValue)>,
    pub at: u64,
}

impl Line {
    pub fn get(&self, name: &str) -> Option<&DiagValue> {
        self.fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    pub fn text(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Some(DiagValue::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn num(&self, name: &str) -> Option<u64> {
        match self.get(name) {
            Some(DiagValue::Num(n)) => Some(*n),
            _ => None,
        }
    }

    pub fn list(&self, name: &str) -> Option<&[String]> {
        match self.get(name) {
            Some(DiagValue::List(v)) => Some(v),
            _ => None,
        }
    }
}

/// Keeps every line in memory (the tests).
#[derive(Default)]
pub struct MemorySink {
    lines: Mutex<Vec<Line>>,
}

impl MemorySink {
    pub fn lines(&self) -> Vec<Line> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl DiagSink for MemorySink {
    fn record(&self, kind: &str, fields: &[(&str, DiagValue)], at_ms: u64) {
        let line = Line {
            k: kind.to_string(),
            fields: fields
                .iter()
                .map(|(n, v)| (n.to_string(), v.clone()))
                .collect(),
            at: at_ms,
        };
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(line);
    }
}

/// The mark a tab holds, in the strip's order (working first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShownMark {
    None,
    Working,
    Question,
    Failed,
    Done,
}

impl ShownMark {
    pub fn as_str(self) -> &'static str {
        match self {
            ShownMark::None => "none",
            ShownMark::Working => "working",
            ShownMark::Question => "question",
            ShownMark::Failed => "failed",
            ShownMark::Done => "done",
        }
    }

    fn parse(s: &str) -> ShownMark {
        match s {
            "working" => ShownMark::Working,
            "question" => ShownMark::Question,
            "failed" => ShownMark::Failed,
            "done" => ShownMark::Done,
            _ => ShownMark::None,
        }
    }
}

/// The indicator's sets after a render.
pub struct MarkSets<'a> {
    pub working: &'a HashSet<String>,
    pub question: &'a HashSet<String>,
    pub failed: &'a HashSet<String>,
    pub done: &'a HashSet<String>,
}

/// The mark a tab holds: working > question > failed > done > none.
pub fn shown_mark(s: &MarkSets, id: &str) -> ShownMark {
    if s.working.contains(id) {
        ShownMark::Working
    } else if s.question.contains(id) {
        ShownMark::Question
    } else if s.failed.contains(id) {
        ShownMark::Failed
    } else if s.done.contains(id) {
        ShownMark::Done
    } else {
        ShownMark::None
    }
}

/// Every attention mark a tab holds, in the order question, failed, done.
pub fn held_marks(s: &MarkSets, id: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    if s.question.contains(id) {
        out.push("question");
    }
    if s.failed.contains(id) {
        out.push("failed");
    }
    if s.done.contains(id) {
        out.push("done");
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkChange {
    pub id: String,
    pub from: ShownMark,
    pub to: ShownMark,
    pub held: Vec<&'static str>,
}

/// The tabs whose mark (or the marks under it) changed since `prev`, which is brought up to date.
/// A tab back at `none` with nothing held is forgotten.
pub fn mark_changes(prev: &mut HashMap<String, String>, s: &MarkSets) -> Vec<MarkChange> {
    let mut ids: BTreeSet<String> = prev.keys().cloned().collect();
    for set in [s.working, s.question, s.failed, s.done] {
        ids.extend(set.iter().cloned());
    }
    let mut out = Vec::new();
    for id in ids {
        let to = shown_mark(s, &id);
        let held = held_marks(s, &id);
        let key = format!("{}|{}", to.as_str(), held.join(","));
        let was = prev
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "none|".to_string());
        if key == was {
            continue;
        }
        let from = ShownMark::parse(was.split('|').next().unwrap_or("none"));
        let nothing_held = held.is_empty();
        out.push(MarkChange {
            id: id.clone(),
            from,
            to,
            held,
        });
        if to == ShownMark::None && nothing_held {
            prev.remove(&id);
        } else {
            prev.insert(id, key);
        }
    }
    out
}

/// One `agent-hook` line before it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookLine {
    pub id: String,
    pub state: String,
    pub kind: Option<String>,
    pub repeats: Option<u32>,
    pub at: u64,
}

/// The folding of `agent-hook` lines. Pure: `offer` a signal, `sweep` for runs gone quiet,
/// `forget` a tab that closed.
pub struct HookFold {
    flush_ms: u64,
    gap_ms: u64,
    runs: BTreeMap<String, Run>,
}

struct Run {
    state: String,
    kind: Option<String>,
    count: u32,
    last: u64,
    wrote: u64,
}

fn repeats_line(id: &str, run: &Run) -> Option<HookLine> {
    (run.count > 0).then(|| HookLine {
        id: id.to_string(),
        state: run.state.clone(),
        kind: run.kind.clone(),
        repeats: Some(run.count),
        at: run.last,
    })
}

impl HookFold {
    pub fn new(flush_ms: u64, gap_ms: u64) -> Self {
        HookFold {
            flush_ms,
            gap_ms,
            runs: BTreeMap::new(),
        }
    }

    pub fn offer(&mut self, id: &str, state: &str, kind: Option<&str>, now: u64) -> Vec<HookLine> {
        if let Some(run) = self.runs.get_mut(id) {
            if run.state == state
                && run.kind.as_deref() == kind
                && now.saturating_sub(run.last) < self.gap_ms
            {
                run.count += 1;
                run.last = now;
                return Vec::new();
            }
        }
        let mut out = Vec::new();
        if let Some(run) = self.runs.get(id) {
            out.extend(repeats_line(id, run));
        }
        self.runs.insert(
            id.to_string(),
            Run {
                state: state.to_string(),
                kind: kind.map(str::to_string),
                count: 0,
                last: now,
                wrote: now,
            },
        );
        out.push(HookLine {
            id: id.to_string(),
            state: state.to_string(),
            kind: kind.map(str::to_string),
            repeats: None,
            at: now,
        });
        out
    }

    /// The held repeats of each run last written `flush_ms` ago or more; the run goes on. A run
    /// silent for `gap_ms` with nothing held is let go.
    pub fn sweep(&mut self, now: u64) -> Vec<HookLine> {
        let (flush, gap) = (self.flush_ms, self.gap_ms);
        let mut out = Vec::new();
        self.runs.retain(|id, run| {
            if run.count > 0 && now.saturating_sub(run.wrote) >= flush {
                out.extend(repeats_line(id, run));
                run.count = 0;
                run.wrote = now;
            }
            !(run.count == 0 && now.saturating_sub(run.last) >= gap)
        });
        out
    }

    pub fn forget(&mut self, id: &str) -> Vec<HookLine> {
        match self.runs.remove(id) {
            Some(run) => repeats_line(id, &run).into_iter().collect(),
            None => Vec::new(),
        }
    }

    /// Whether any run still has repeats to say (a sweep is then due).
    pub fn pending(&self) -> bool {
        self.runs.values().any(|r| r.count > 0)
    }
}

/// The new meaning of a tab's title when it changed, else None. A tab that never had an agent
/// title says nothing for a title that is `none`.
pub fn title_change(prev: Option<&str>, next: &str) -> Option<String> {
    if next == prev.unwrap_or("none") {
        None
    } else {
        Some(next.to_string())
    }
}

fn s(v: &str) -> DiagValue {
    DiagValue::Str(v.to_string())
}

fn hook_fields(l: &HookLine) -> Vec<(&'static str, DiagValue)> {
    let mut f = vec![("id", s(&l.id)), ("state", s(&l.state))];
    if let Some(k) = &l.kind {
        f.push(("kind", s(k)));
    }
    if let Some(r) = l.repeats {
        f.push(("repeats", DiagValue::Num(u64::from(r))));
    }
    f
}

struct Budget {
    start: u64,
    n: u32,
    dropped: u64,
}

/// The record of one window: the sink, the fold, the title and mark memory and the deadlines.
pub struct AgentDiag {
    sink: Arc<dyn DiagSink>,
    fold: HookFold,
    titles: HashMap<String, String>,
    none_due: BTreeMap<String, u64>,
    whys: HashMap<String, Vec<(String, u64)>>,
    marks: HashMap<String, String>,
    restored_at: HashMap<String, u64>,
    budgets: HashMap<String, Budget>,
    sweep_due: Option<u64>,
}

impl AgentDiag {
    pub fn new(sink: Arc<dyn DiagSink>) -> Self {
        AgentDiag {
            sink,
            fold: HookFold::new(HOOK_FLUSH_MS, HOOK_GAP_MS),
            titles: HashMap::new(),
            none_due: BTreeMap::new(),
            whys: HashMap::new(),
            marks: HashMap::new(),
            restored_at: HashMap::new(),
            budgets: HashMap::new(),
            sweep_due: None,
        }
    }

    /// A line for `id`, within the tab's budget (`AGENT_LINES_PER_WINDOW` per `AGENT_WINDOW_MS`).
    /// `at` is the line's time; `now` is when it is written (the budget's clock).
    fn emit(
        &mut self,
        id: &str,
        kind: &str,
        mut fields: Vec<(&'static str, DiagValue)>,
        at: u64,
        now: u64,
    ) {
        let budget = self.budgets.entry(id.to_string()).or_insert(Budget {
            start: now,
            n: 0,
            dropped: 0,
        });
        if now.saturating_sub(budget.start) >= AGENT_WINDOW_MS {
            budget.start = now;
            budget.n = 0;
        }
        if budget.n >= AGENT_LINES_PER_WINDOW {
            budget.dropped += 1;
            return;
        }
        budget.n += 1;
        let dropped = std::mem::take(&mut budget.dropped);
        if dropped > 0 {
            fields.push(("dropped", DiagValue::Num(dropped)));
        }
        self.sink.record(kind, &fields, at);
    }

    fn write_hooks(&mut self, lines: Vec<HookLine>, now: u64) {
        for l in lines {
            let fields = hook_fields(&l);
            self.emit(&l.id, "agent-hook", fields, l.at, now);
        }
    }

    fn arm_sweep(&mut self, now: u64) {
        if self.sweep_due.is_none() && self.fold.pending() {
            self.sweep_due = Some(now + SWEEP_MS);
        }
    }

    /// A hook signal reached the tab. A failure kind outside Claude's own list is written as `other`.
    pub fn log_hook(&mut self, id: &str, state: &str, kind: Option<&str>, now: u64) {
        let known = kind.map(|k| {
            if flare_agent::signal::STOP_FAILURE_KINDS.contains(&k) {
                k
            } else {
                "other"
            }
        });
        let lines = self.fold.offer(id, state, known, now);
        self.write_hooks(lines, now);
        self.arm_sweep(now);
    }

    /// The tab's title was read as `state` (`none`: no agent's title).
    pub fn log_title(&mut self, id: &str, state: &str, agent: Option<&str>, now: u64) {
        if state != "none" {
            self.none_due.remove(id);
        }
        let Some(change) = title_change(self.titles.get(id).map(String::as_str), state) else {
            return;
        };
        if change != "none" {
            self.titles.insert(id.to_string(), change.clone());
            let mut fields = vec![("id", s(id)), ("state", s(&change))];
            if let Some(a) = agent {
                fields.push(("agent", s(a)));
            }
            self.emit(id, "agent-title", fields, now, now);
            return;
        }
        if self.none_due.contains_key(id) {
            return;
        }
        self.none_due.insert(id.to_string(), now + TITLE_NONE_MS);
    }

    /// The rule about to change a tab's marks; written with the change it makes.
    pub fn note_why(&mut self, id: &str, why: &str, now: u64) {
        let entry = self.whys.entry(id.to_string()).or_default();
        entry.retain(|(w, at)| now.saturating_sub(*at) <= WHY_MS && w != why);
        entry.push((why.to_string(), now));
    }

    /// The indicator's sets after a render: a line for each tab whose mark moved.
    pub fn log_marks(
        &mut self,
        sets: &MarkSets,
        agent_of: &dyn Fn(&str) -> Option<String>,
        now: u64,
    ) {
        for c in mark_changes(&mut self.marks, sets) {
            let fresh: Vec<String> = self
                .whys
                .get(&c.id)
                .map(|w| {
                    w.iter()
                        .filter(|(_, at)| now.saturating_sub(*at) <= WHY_MS)
                        .map(|(why, _)| why.clone())
                        .collect()
                })
                .unwrap_or_default();
            self.whys.remove(&c.id);
            let why = if fresh.is_empty() {
                "unknown".to_string()
            } else {
                fresh.join(" + ")
            };
            let mut fields = vec![
                ("id", s(&c.id)),
                ("from", s(c.from.as_str())),
                ("to", s(c.to.as_str())),
                (
                    "held",
                    DiagValue::List(c.held.iter().map(|h| h.to_string()).collect()),
                ),
                ("why", s(&why)),
            ];
            if let Some(a) = agent_of(&c.id) {
                fields.push(("agent", s(&a)));
            }
            if let Some(&r) = self.restored_at.get(&c.id) {
                let since = now.saturating_sub(r);
                if since <= RESTORED_MS {
                    fields.push(("restored", DiagValue::Num(since)));
                }
            }
            self.emit(&c.id, "agent-mark", fields, now, now);
        }
        // A closed tab's rules and budget, once nothing is left to say with them.
        self.whys
            .retain(|_, w| w.iter().any(|(_, at)| now.saturating_sub(*at) <= WHY_MS));
        self.budgets
            .retain(|_, b| b.dropped != 0 || now.saturating_sub(b.start) < AGENT_WINDOW_MS);
    }

    /// A tab restored over an agent conversation (the resume rides its spawn).
    pub fn log_restore(&mut self, id: &str, cwd: Option<&str>, now: u64) {
        self.restored_at.insert(id.to_string(), now);
        let mut fields = vec![("id", s(id))];
        if let Some(c) = cwd {
            fields.push(("cwd", s(c)));
        }
        fields.push(("resume", DiagValue::Bool(true)));
        self.sink.record("agent-restore", &fields, now);
    }

    /// The tab closed: its folded repeats are said, and the rest forgotten.
    pub fn forget(&mut self, id: &str, now: u64) {
        let lines = self.fold.forget(id);
        self.write_hooks(lines, now);
        self.none_due.remove(id);
        self.titles.remove(id);
        self.restored_at.remove(id);
    }

    /// Runs the sweep and the `none` timers that are due at `now`.
    pub fn tick(&mut self, now: u64) {
        let due: Vec<String> = self
            .none_due
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            self.none_due.remove(&id);
            self.titles.remove(&id);
            self.emit(
                &id,
                "agent-title",
                vec![("id", s(&id)), ("state", s("none"))],
                now,
                now,
            );
        }
        if let Some(at) = self.sweep_due {
            if at <= now {
                self.sweep_due = None;
                let lines = self.fold.sweep(now);
                self.write_hooks(lines, now);
                self.arm_sweep(now);
            }
        }
    }

    /// The next deadline the glue must `tick` at, if any.
    pub fn next_due(&self) -> Option<u64> {
        self.none_due.values().copied().chain(self.sweep_due).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_800_000_000_000;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    struct Sets {
        working: HashSet<String>,
        question: HashSet<String>,
        failed: HashSet<String>,
        done: HashSet<String>,
    }

    impl Sets {
        fn new(working: &[&str], question: &[&str], failed: &[&str], done: &[&str]) -> Self {
            Sets {
                working: set(working),
                question: set(question),
                failed: set(failed),
                done: set(done),
            }
        }
        fn view(&self) -> MarkSets<'_> {
            MarkSets {
                working: &self.working,
                question: &self.question,
                failed: &self.failed,
                done: &self.done,
            }
        }
    }

    fn no_agent(_: &str) -> Option<String> {
        None
    }

    fn hooks(sink: &MemorySink) -> Vec<Line> {
        sink.lines()
            .into_iter()
            .filter(|l| l.k == "agent-hook")
            .collect()
    }

    fn sink_pair() -> (Arc<MemorySink>, AgentDiag) {
        let sink = Arc::new(MemorySink::default());
        let diag = AgentDiag::new(sink.clone());
        (sink, diag)
    }

    #[test]
    fn writes_the_first_signal_at_once_and_folds_the_same_state_after_it() {
        let mut f = HookFold::new(HOOK_FLUSH_MS, HOOK_GAP_MS);
        assert_eq!(
            f.offer("t1", "working", None, T0),
            vec![HookLine {
                id: "t1".into(),
                state: "working".into(),
                kind: None,
                repeats: None,
                at: T0
            }]
        );
        assert!(f.offer("t1", "working", None, T0 + 100).is_empty());
        assert!(f.offer("t1", "working", None, T0 + 200).is_empty());
        assert_eq!(
            f.offer("t1", "done", None, T0 + 300),
            vec![
                HookLine {
                    id: "t1".into(),
                    state: "working".into(),
                    kind: None,
                    repeats: Some(2),
                    at: T0 + 200
                },
                HookLine {
                    id: "t1".into(),
                    state: "done".into(),
                    kind: None,
                    repeats: None,
                    at: T0 + 300
                },
            ]
        );
    }

    #[test]
    fn keeps_each_tab_its_own_run() {
        let mut f = HookFold::new(HOOK_FLUSH_MS, HOOK_GAP_MS);
        f.offer("t1", "working", None, T0);
        assert_eq!(f.offer("t2", "working", None, T0 + 10).len(), 1);
        assert!(f.offer("t1", "working", None, T0 + 20).is_empty());
    }

    #[test]
    fn a_failure_kind_is_part_of_the_state() {
        let mut f = HookFold::new(HOOK_FLUSH_MS, HOOK_GAP_MS);
        f.offer("t1", "failed", Some("rate_limit"), T0);
        let out = f.offer("t1", "failed", None, T0 + 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].at, T0 + 5);
    }

    #[test]
    fn held_repeats_are_said_at_most_every_flush_ms_and_the_run_goes_on_folding() {
        let mut f = HookFold::new(HOOK_FLUSH_MS, HOOK_GAP_MS);
        f.offer("t1", "working", None, T0);
        f.offer("t1", "working", None, T0 + 1000);
        assert!(f.pending());
        assert!(f.sweep(T0 + HOOK_FLUSH_MS - 1).is_empty());
        assert_eq!(
            f.sweep(T0 + HOOK_FLUSH_MS),
            vec![HookLine {
                id: "t1".into(),
                state: "working".into(),
                kind: None,
                repeats: Some(1),
                at: T0 + 1000
            }]
        );
        assert!(!f.pending());
        assert!(f
            .offer("t1", "working", None, T0 + HOOK_FLUSH_MS + 8000)
            .is_empty());
    }

    #[test]
    fn the_same_state_after_hook_gap_ms_of_silence_starts_a_new_line() {
        let mut f = HookFold::new(HOOK_FLUSH_MS, HOOK_GAP_MS);
        f.offer("t1", "working", None, T0);
        let out = f.offer("t1", "working", None, T0 + HOOK_GAP_MS);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].at, T0 + HOOK_GAP_MS);
    }

    #[test]
    fn a_single_signal_has_no_repeats_line_and_forget_says_what_was_held() {
        let mut f = HookFold::new(HOOK_FLUSH_MS, HOOK_GAP_MS);
        f.offer("t1", "done", None, T0);
        assert!(f.sweep(T0 + HOOK_GAP_MS + 10_000).is_empty());
        f.offer("t1", "working", None, T0 + HOOK_GAP_MS + 11_000);
        f.offer("t1", "working", None, T0 + HOOK_GAP_MS + 11_100);
        assert_eq!(
            f.forget("t1"),
            vec![HookLine {
                id: "t1".into(),
                state: "working".into(),
                kind: None,
                repeats: Some(1),
                at: T0 + HOOK_GAP_MS + 11_100
            }]
        );
        assert!(f.forget("t1").is_empty());
    }

    #[test]
    fn tool_calls_seconds_apart_ten_minutes_stay_a_few_lines() {
        let mut f = HookFold::new(HOOK_FLUSH_MS, HOOK_GAP_MS);
        let mut out: Vec<HookLine> = Vec::new();
        for t in (0..600_000u64).step_by(1000) {
            if t % 8000 == 0 || t % 8000 == 2000 {
                out.extend(f.offer("t1", "working", None, T0 + t));
            }
            if t % 5000 == 0 {
                out.extend(f.sweep(T0 + t));
            }
        }
        out.extend(f.offer("t1", "done", None, T0 + 600_000));
        assert!(out.len() <= 23, "{} lines", out.len());
        let total: u32 = out.iter().map(|l| l.repeats.unwrap_or(1)).sum();
        assert_eq!(total, 151);
    }

    #[test]
    fn title_change_only_a_change_of_meaning() {
        assert_eq!(title_change(None, "working"), Some("working".to_string()));
        assert_eq!(title_change(Some("working"), "working"), None);
        assert_eq!(
            title_change(Some("working"), "idle"),
            Some("idle".to_string())
        );
        assert_eq!(title_change(Some("idle"), "none"), Some("none".to_string()));
    }

    #[test]
    fn a_tab_that_never_had_an_agent_title_says_nothing_for_none() {
        assert_eq!(title_change(None, "none"), None);
    }

    #[test]
    fn working_outranks_question_outranks_failed_then_done() {
        let s = Sets::new(&["a"], &[], &[], &["a"]);
        assert_eq!(shown_mark(&s.view(), "a"), ShownMark::Working);
        let s = Sets::new(&[], &["a"], &["a"], &["a"]);
        assert_eq!(shown_mark(&s.view(), "a"), ShownMark::Question);
        let s = Sets::new(&[], &[], &["a"], &["a"]);
        assert_eq!(shown_mark(&s.view(), "a"), ShownMark::Failed);
        let s = Sets::new(&[], &[], &[], &[]);
        assert_eq!(shown_mark(&s.view(), "a"), ShownMark::None);
    }

    #[test]
    fn says_each_move_once_with_the_marks_held_under_it() {
        let mut prev = HashMap::new();
        let s = Sets::new(&["a"], &[], &[], &[]);
        assert_eq!(
            mark_changes(&mut prev, &s.view()),
            vec![MarkChange {
                id: "a".into(),
                from: ShownMark::None,
                to: ShownMark::Working,
                held: vec![]
            }]
        );
        assert!(mark_changes(&mut prev, &s.view()).is_empty());
        let s = Sets::new(&[], &[], &["a"], &["a"]);
        assert_eq!(
            mark_changes(&mut prev, &s.view()),
            vec![MarkChange {
                id: "a".into(),
                from: ShownMark::Working,
                to: ShownMark::Failed,
                held: vec!["failed", "done"]
            }]
        );
        // Finished goes under the question: the shown mark moves, what is held moves with it.
        let s = Sets::new(&[], &["a"], &[], &[]);
        assert_eq!(
            mark_changes(&mut prev, &s.view()),
            vec![MarkChange {
                id: "a".into(),
                from: ShownMark::Failed,
                to: ShownMark::Question,
                held: vec!["question"]
            }]
        );
        let s = Sets::new(&[], &["a"], &[], &["a"]);
        assert_eq!(
            mark_changes(&mut prev, &s.view()),
            vec![MarkChange {
                id: "a".into(),
                from: ShownMark::Question,
                to: ShownMark::Question,
                held: vec!["question", "done"]
            }]
        );
        let s = Sets::new(&[], &[], &[], &[]);
        assert_eq!(
            mark_changes(&mut prev, &s.view()),
            vec![MarkChange {
                id: "a".into(),
                from: ShownMark::Question,
                to: ShownMark::None,
                held: vec![]
            }]
        );
        assert!(prev.is_empty());
    }

    #[test]
    fn folds_a_turn_of_tool_calls_into_two_hook_lines() {
        let (sink, mut diag) = sink_pair();
        for i in 0..40u64 {
            diag.log_hook("t1", "working", None, T0 + i * 10);
        }
        diag.log_hook("t1", "done", None, T0 + 400);
        diag.tick(T0 + 400);
        let got: Vec<(String, Option<u64>)> = hooks(&sink)
            .iter()
            .map(|l| {
                (
                    l.text("state").unwrap_or_default().to_string(),
                    l.num("repeats"),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("working".into(), None),
                ("working".into(), Some(39)),
                ("done".into(), None)
            ]
        );
    }

    #[test]
    fn a_quiet_tab_has_its_repeats_said_by_the_sweep() {
        let (sink, mut diag) = sink_pair();
        diag.log_hook("t1", "working", None, T0);
        diag.log_hook("t1", "working", None, T0);
        diag.tick(T0 + HOOK_FLUSH_MS + 5300);
        let got: Vec<u64> = hooks(&sink)
            .iter()
            .map(|l| l.num("repeats").unwrap_or(0))
            .collect();
        assert_eq!(got, vec![0, 1]);
    }

    #[test]
    fn writes_a_title_only_when_its_meaning_changes() {
        let (sink, mut diag) = sink_pair();
        diag.log_title("t1", "none", None, T0);
        diag.log_title("t1", "working", Some("claude"), T0 + 10);
        diag.log_title("t1", "working", Some("claude"), T0 + 20);
        diag.log_title("t1", "idle", Some("claude"), T0 + 30);
        diag.log_title("t1", "none", None, T0 + 40);
        diag.tick(T0 + 40 + TITLE_NONE_MS + 300);
        let states: Vec<String> = sink
            .lines()
            .iter()
            .filter(|l| l.k == "agent-title")
            .map(|l| l.text("state").unwrap_or_default().to_string())
            .collect();
        assert_eq!(states, vec!["working", "idle", "none"]);
    }

    #[test]
    fn a_childs_title_between_an_agents_is_not_a_none() {
        let (sink, mut diag) = sink_pair();
        let mut now = T0;
        diag.log_title("t1", "working", Some("codex"), now);
        for _ in 0..20 {
            now += 500;
            diag.log_title("t1", "none", None, now);
            now += 500;
            diag.log_title("t1", "working", Some("codex"), now);
            diag.tick(now);
        }
        diag.tick(now + TITLE_NONE_MS + 300);
        let states: Vec<String> = sink
            .lines()
            .iter()
            .filter(|l| l.k == "agent-title")
            .map(|l| l.text("state").unwrap_or_default().to_string())
            .collect();
        assert_eq!(states, vec!["working"]);
    }

    #[test]
    fn a_failure_kind_outside_claudes_own_list_is_written_as_other() {
        let (sink, mut diag) = sink_pair();
        diag.log_hook("t1", "failed", Some("rate_limit"), T0);
        diag.log_hook("t1", "failed", Some("hunter2"), T0 + 5);
        diag.tick(T0 + 300);
        let kinds: Vec<String> = hooks(&sink)
            .iter()
            .map(|l| l.text("kind").unwrap_or_default().to_string())
            .collect();
        assert_eq!(kinds, vec!["rate_limit", "other"]);
    }

    #[test]
    fn a_flood_of_signals_is_capped_per_tab_and_the_next_line_says_how_many_were_dropped() {
        let (sink, mut diag) = sink_pair();
        for i in 0..1000u64 {
            diag.log_hook("t1", if i % 2 == 1 { "working" } else { "done" }, None, T0);
        }
        diag.log_hook("t2", "working", None, T0);
        diag.tick(T0 + 300);
        let t1: Vec<Line> = hooks(&sink)
            .into_iter()
            .filter(|l| l.text("id") == Some("t1"))
            .collect();
        assert_eq!(t1.len(), AGENT_LINES_PER_WINDOW as usize);
        assert!(hooks(&sink).iter().any(|l| l.text("id") == Some("t2")));
        diag.log_hook("t1", "question", None, T0 + AGENT_WINDOW_MS);
        diag.tick(T0 + AGENT_WINDOW_MS + 300);
        let after: Vec<Line> = hooks(&sink)
            .into_iter()
            .filter(|l| l.text("id") == Some("t1"))
            .skip(AGENT_LINES_PER_WINDOW as usize)
            .collect();
        assert_eq!(
            after[0].num("dropped"),
            Some(1000 - AGENT_LINES_PER_WINDOW as u64)
        );
    }

    #[test]
    fn a_rule_noted_longer_ago_than_a_second_is_not_a_reason() {
        let (sink, mut diag) = sink_pair();
        let mut now = T0;
        diag.note_why("t1", "stale", now);
        for _ in 0..5 {
            now += 800;
            diag.note_why("t1", "hook working", now);
        }
        diag.note_why("t1", "hook done", now);
        let done = set(&["t1"]);
        let empty = HashSet::new();
        let sets = MarkSets {
            working: &empty,
            question: &empty,
            failed: &empty,
            done: &done,
        };
        diag.log_marks(&sets, &no_agent, now);
        let mark = sink
            .lines()
            .into_iter()
            .find(|l| l.k == "agent-mark")
            .expect("a mark line");
        assert_eq!(mark.text("why"), Some("hook working + hook done"));
    }

    #[test]
    fn a_mark_carries_the_rule_that_moved_it_and_only_a_fresh_one() {
        let (sink, mut diag) = sink_pair();
        let empty = HashSet::new();
        let working = set(&["t1"]);
        let done = set(&["t1"]);
        diag.note_why("t1", "hook working", T0);
        let agent = |_: &str| Some("claude".to_string());
        diag.log_marks(
            &MarkSets {
                working: &working,
                question: &empty,
                failed: &empty,
                done: &empty,
            },
            &agent,
            T0,
        );
        diag.note_why("t1", "hook done", T0);
        diag.note_why("t1", "hook done", T0);
        diag.log_marks(
            &MarkSets {
                working: &empty,
                question: &empty,
                failed: &empty,
                done: &done,
            },
            &no_agent,
            T0,
        );
        diag.note_why("t1", "stale", T0);
        diag.log_marks(
            &MarkSets {
                working: &empty,
                question: &empty,
                failed: &empty,
                done: &empty,
            },
            &no_agent,
            T0 + 1500,
        );
        let marks: Vec<Line> = sink
            .lines()
            .into_iter()
            .filter(|l| l.k == "agent-mark")
            .collect();
        let got: Vec<(String, String, String)> = marks
            .iter()
            .map(|l| {
                (
                    l.text("from").unwrap_or_default().to_string(),
                    l.text("to").unwrap_or_default().to_string(),
                    l.text("why").unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("none".into(), "working".into(), "hook working".into()),
                ("working".into(), "done".into(), "hook done".into()),
                ("done".into(), "none".into(), "unknown".into()),
            ]
        );
        assert_eq!(marks[0].text("agent"), Some("claude"));
    }

    #[test]
    fn a_restored_tab_says_so_and_its_marks_say_how_long_after() {
        let (sink, mut diag) = sink_pair();
        diag.log_restore("t1", Some("C:\\work"), T0);
        diag.note_why("t1", "hook done", T0 + 2000);
        let done = set(&["t1"]);
        let empty = HashSet::new();
        diag.log_marks(
            &MarkSets {
                working: &empty,
                question: &empty,
                failed: &empty,
                done: &done,
            },
            &no_agent,
            T0 + 2000,
        );
        let lines = sink.lines();
        let restore = lines
            .iter()
            .find(|l| l.k == "agent-restore")
            .expect("a restore line");
        assert_eq!(restore.text("id"), Some("t1"));
        assert_eq!(restore.text("cwd"), Some("C:\\work"));
        assert_eq!(restore.get("resume"), Some(&DiagValue::Bool(true)));
        let mark = lines
            .iter()
            .find(|l| l.k == "agent-mark")
            .expect("a mark line");
        assert_eq!(mark.text("to"), Some("done"));
        assert_eq!(mark.text("why"), Some("hook done"));
        assert_eq!(mark.num("restored"), Some(2000));
    }

    #[test]
    fn a_closed_tab_says_its_held_repeats() {
        let (sink, mut diag) = sink_pair();
        diag.log_hook("t1", "working", None, T0);
        diag.log_hook("t1", "working", None, T0);
        diag.forget("t1", T0 + 1);
        let got: Vec<u64> = hooks(&sink)
            .iter()
            .map(|l| l.num("repeats").unwrap_or(0))
            .collect();
        assert_eq!(got, vec![0, 1]);
    }

    #[test]
    fn no_free_text_reaches_the_sink() {
        let (sink, mut diag) = sink_pair();
        let marker = "hunter2 SECRET-TITLE";
        let working = set(&["t-1"]);
        let empty = HashSet::new();
        let agent = |_: &str| Some("claude".to_string());
        diag.log_hook("t-1", "failed", Some(marker), T0);
        diag.log_hook("t-1", "working", Some(marker), T0 + 1);
        diag.log_title("t-1", "working", Some("claude"), T0 + 2);
        diag.note_why("t-1", "hook working", T0 + 3);
        diag.log_marks(
            &MarkSets {
                working: &working,
                question: &empty,
                failed: &empty,
                done: &empty,
            },
            &agent,
            T0 + 3,
        );
        diag.log_restore("t-1", Some("C:\\work"), T0 + 4);
        diag.forget("t-1", T0 + 5);
        diag.tick(T0 + 60_000);
        let allowed = [
            "id", "state", "kind", "repeats", "agent", "from", "to", "held", "why", "restored",
            "dropped", "cwd", "resume",
        ];
        for line in sink.lines() {
            assert!(
                matches!(
                    line.k.as_str(),
                    "agent-hook" | "agent-title" | "agent-mark" | "agent-restore"
                ),
                "unexpected kind {}",
                line.k
            );
            assert!(!line.k.contains("hunter2") && !line.k.contains("SECRET"));
            for (name, value) in &line.fields {
                assert!(allowed.contains(&name.as_str()), "unexpected field {name}");
                let text = match value {
                    DiagValue::Str(s) => s.clone(),
                    DiagValue::List(v) => v.join(","),
                    DiagValue::Num(_) | DiagValue::Bool(_) => String::new(),
                };
                assert!(
                    !text.contains("hunter2") && !text.contains("SECRET"),
                    "marker in {name}"
                );
            }
        }
    }
}
