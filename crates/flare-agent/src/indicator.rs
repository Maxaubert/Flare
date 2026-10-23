//! The Indicator: the whole agent indicator as one struct (PT `useAgentIndicator.ts`).
//!
//! Each React `useEffect` subscription is an `Event`, each `setTimeout` is a deadline in `Timers`,
//! each `setXIds` is a `HashSet` mutation, and the render-time effect of "finished while away" is
//! `reconcile`. Time is passed in; the screen is behind the `Screen` trait. Task 17 writes the
//! state, the deadlines, `Poll`, `Title`, `Signal`, `reconcile`, looking, `forget` and the read
//! side; Task 18 adds `Output`, the question reads, the answer keys and the fallback scorer.

use std::collections::{HashMap, HashSet};

use crate::activity::Activity;
use crate::activity::ECHO_MS;
use crate::background::looks_like_background_work;
use crate::clock::AgentClock;
use crate::hook_state::{
    AttentionMark, BACKGROUND_CHECK_MS, HookEvent, HookPhase, HookSeen, HookSession,
    STOP_SETTLE_MS, hook_step, raised_while_seen, screen_decides,
};
use crate::kinds::DetectedAgent;
use crate::question::{answers_question, looks_like_question, question_answered};
use crate::signal::{AgentSignal, HookState};
use crate::title::{AgentTitleState, TitleReader};

pub const SCREEN_TAIL_ROWS: usize = 16;
/// Ask the poll to look at most once in 15 s per tab.
pub const LOOK_GAP_MS: u64 = 15_000;
/// A pointer move over an unfocused window is the owner back for this long.
pub const MOVE_LOOK_MS: u64 = 1000;
/// And takes the front tab's finished/failed mark down at most this often.
pub const MOVE_LOOK_GAP_MS: u64 = 500;
pub const QUESTION_READ_MS: u64 = 200;
pub const ANSWER_READS_MS: [u64; 4] = [250, 1000, 2500, 5000];
pub const TITLE_RECHECK_MS: u64 = 250;
pub const RUN_GAP_MS: u64 = 1500;
pub const RUN_SUSTAIN_MS: u64 = 1200;
pub const FALLBACK_QUIET_MS: u64 = 2000;

/// The bottom of a tab's text: the last `rows` rows ending at the LAST NON-BLANK line of the
/// buffer (history included), blanks above it kept (PT `setScreenTail`: under Claude's inline
/// renderer what was drawn last sits above empty rows). Unknown tab: empty.
pub trait Screen {
    fn tail(&self, tab: &str, rows: usize) -> Vec<String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Title {
        tab: String,
        text: String,
    },
    Signal {
        tab: String,
        signal: AgentSignal,
    },
    /// The pty printed (one per drained batch is enough).
    Output {
        tab: String,
    },
    /// PT's key string for Enter ("\r"), Esc ("\x1b"), Ctrl+C ("\x03"), digits.
    Key {
        tab: String,
        key: String,
    },
    Poll {
        tab: String,
        present: bool,
        kind: Option<DetectedAgent>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The glue calls `PollWorker::look_now()`.
    AskLook,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogItem {
    Hook {
        tab: String,
        state: String,
        kind: Option<String>,
    },
    Title {
        tab: String,
        state: String,
        agent: Option<String>,
    },
    Why {
        tab: String,
        why: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Slot {
    QuestionRead,
    AnswerRead(u8),
    TitleRecheck,
    Settle,
    Fallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HookEventKind {
    Settle,
    BgCheck,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    ReadQuestion,
    /// The key that asked for the read.
    AnswerCheck(String),
    TitleRecheck,
    Hook(HookEventKind),
    FallbackClear,
}

/// Deadlines, one per (tab, slot); `arm` replaces, `cancel` removes, `due(now)` pops the earliest.
#[derive(Default)]
struct Timers {
    map: HashMap<(String, Slot), (u64, u64, Action)>,
    seq: u64,
}

impl Timers {
    fn arm(&mut self, tab: &str, slot: Slot, at: u64, action: Action) {
        self.seq += 1;
        self.map
            .insert((tab.to_string(), slot), (at, self.seq, action));
    }

    fn cancel(&mut self, tab: &str, slot: Slot) {
        self.map.remove(&(tab.to_string(), slot));
    }

    fn armed(&self, tab: &str, slot: Slot) -> bool {
        self.map.contains_key(&(tab.to_string(), slot))
    }

    fn cancel_tab(&mut self, tab: &str) {
        self.map.retain(|(t, _), _| t != tab);
    }

    fn next_due(&self) -> Option<u64> {
        self.map.values().map(|(at, _, _)| *at).min()
    }

    fn due(&mut self, now: u64) -> Option<(String, Action)> {
        let key = self
            .map
            .iter()
            .filter(|(_, (at, _, _))| *at <= now)
            .min_by_key(|(_, (at, seq, _))| (*at, *seq))
            .map(|(k, _)| k.clone())?;
        let (_, _, action) = self.map.remove(&key)?;
        Some((key.0, action))
    }
}

/// A snapshot of what `take_changed` watches.
#[derive(Clone, PartialEq, Eq, Default)]
struct Snapshot {
    agent_ids: HashSet<String>,
    working: HashSet<String>,
    done: HashSet<String>,
    question: HashSet<String>,
    failed: HashSet<String>,
    failed_kinds: HashMap<String, String>,
}

pub struct Indicator {
    pub activity: Activity,
    agent_ids: HashSet<String>,
    working: HashSet<String>,
    done: HashSet<String>,
    question: HashSet<String>,
    failed: HashSet<String>,
    failed_kinds: HashMap<String, String>,
    agent_kinds: HashMap<String, DetectedAgent>,
    hooked: HashMap<String, HookSession>,
    titled: HashSet<String>,
    polled: HashSet<String>,
    title_state: HashMap<String, AgentTitleState>,
    look_asked: HashMap<String, u64>,
    output_runs: HashMap<String, (u64, u64)>,
    reader: TitleReader,
    clock: AgentClock,
    active: Option<String>,
    focused: bool,
    moved_at: Option<u64>,
    last_move_look: Option<u64>,
    prev_working: HashSet<String>,
    timers: Timers,
    log: Vec<LogItem>,
    snap: Snapshot,
    dirty: bool,
}

impl Default for Indicator {
    fn default() -> Self {
        Self::new()
    }
}

impl Indicator {
    /// Focused, nothing in front.
    pub fn new() -> Self {
        Self {
            activity: Activity::new(),
            agent_ids: HashSet::new(),
            working: HashSet::new(),
            done: HashSet::new(),
            question: HashSet::new(),
            failed: HashSet::new(),
            failed_kinds: HashMap::new(),
            agent_kinds: HashMap::new(),
            hooked: HashMap::new(),
            titled: HashSet::new(),
            polled: HashSet::new(),
            title_state: HashMap::new(),
            look_asked: HashMap::new(),
            output_runs: HashMap::new(),
            reader: TitleReader::new(),
            clock: AgentClock::new(),
            active: None,
            focused: true,
            moved_at: None,
            last_move_look: None,
            prev_working: HashSet::new(),
            timers: Timers::default(),
            log: Vec::new(),
            snap: Snapshot::default(),
            dirty: false,
        }
    }

    pub fn handle(&mut self, now: u64, ev: Event, screen: &dyn Screen) -> Vec<Effect> {
        let mut effects = Vec::new();
        match ev {
            Event::Poll { tab, present, kind } => self.on_poll(now, &tab, present, kind),
            Event::Title { tab, text } => self.on_title(now, &tab, &text, screen, &mut effects),
            Event::Signal { tab, signal } => {
                self.on_signal(now, &tab, signal, screen, &mut effects);
            }
            Event::Output { tab } => self.on_output(now, &tab),
            Event::Key { tab, key } => self.on_key(now, &tab, &key),
        }
        self.reconcile(now);
        effects
    }

    /// Fires every deadline `<= now`, earliest first.
    pub fn tick(&mut self, now: u64, screen: &dyn Screen) -> Vec<Effect> {
        while let Some((tab, action)) = self.timers.due(now) {
            self.run_action(now, &tab, action, screen);
        }
        self.reconcile(now);
        Vec::new()
    }

    pub fn next_due(&self) -> Option<u64> {
        self.timers.next_due()
    }

    /// The tab in front and whether the window has focus (PT: `looking`).
    pub fn set_looking(&mut self, now: u64, active: Option<&str>, focused: bool) {
        self.active = active.map(str::to_string);
        self.focused = focused;
        self.reconcile(now);
    }

    /// The pointer moved over the window (PT: the `pointermove` listener).
    pub fn pointer_moved(&mut self, now: u64) {
        if self.focused {
            return;
        }
        self.moved_at = Some(now);
        if !self.front_marked() {
            return;
        }
        if self
            .last_move_look
            .is_some_and(|l| now.saturating_sub(l) < MOVE_LOOK_GAP_MS)
        {
            return;
        }
        self.last_move_look = Some(now);
        self.reconcile(now);
    }

    /// PT `forget`: every mark, record, timer and the activity of the tab.
    pub fn forget(&mut self, tab: &str) {
        self.why(tab, "closed");
        self.agent_ids.remove(tab);
        self.working.remove(tab);
        self.done.remove(tab);
        self.question.remove(tab);
        self.failed.remove(tab);
        self.failed_kinds.remove(tab);
        self.agent_kinds.remove(tab);
        self.hooked.remove(tab);
        self.titled.remove(tab);
        self.polled.remove(tab);
        self.title_state.remove(tab);
        self.look_asked.remove(tab);
        self.output_runs.remove(tab);
        self.prev_working.remove(tab);
        self.reader.forget(tab);
        self.activity.forget(tab);
        self.timers.cancel_tab(tab);
        self.note_changes();
        self.dirty = true;
    }

    pub fn drain_log(&mut self) -> Vec<LogItem> {
        std::mem::take(&mut self.log)
    }

    /// True once after any set (or a failed kind) changed.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn agent_ids(&self) -> &HashSet<String> {
        &self.agent_ids
    }

    pub fn working_ids(&self) -> &HashSet<String> {
        &self.working
    }

    pub fn done_ids(&self) -> &HashSet<String> {
        &self.done
    }

    pub fn question_ids(&self) -> &HashSet<String> {
        &self.question
    }

    pub fn failed_ids(&self) -> &HashSet<String> {
        &self.failed
    }

    pub fn failed_kind(&self, tab: &str) -> Option<&str> {
        self.failed_kinds.get(tab).map(String::as_str)
    }

    pub fn agent_kind(&self, tab: &str) -> Option<DetectedAgent> {
        self.agent_kinds.get(tab).copied()
    }

    pub fn working_for(&self, tab: &str, now: u64) -> Option<u64> {
        self.clock.working_for(tab, now)
    }

    pub fn is_hooked(&self, tab: &str) -> bool {
        self.hooked.contains_key(tab)
    }

    pub fn is_polled(&self, tab: &str) -> bool {
        self.polled.contains(tab)
    }

    // ---- internals ----

    fn why(&mut self, tab: &str, why: &str) {
        self.log.push(LogItem::Why {
            tab: tab.to_string(),
            why: why.to_string(),
        });
    }

    fn looked_at(&self, tab: &str) -> bool {
        self.focused && self.active.as_deref() == Some(tab)
    }

    fn front_marked(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| self.done.contains(a) || self.failed.contains(a))
    }

    fn seeing(&self, now: u64, tab: &str) -> bool {
        self.active.as_deref() == Some(tab)
            && (self.focused
                || self
                    .moved_at
                    .is_some_and(|m| now.saturating_sub(m) < MOVE_LOOK_MS))
    }

    fn note_changes(&mut self) {
        let snap = Snapshot {
            agent_ids: self.agent_ids.clone(),
            working: self.working.clone(),
            done: self.done.clone(),
            question: self.question.clone(),
            failed: self.failed.clone(),
            failed_kinds: self.failed_kinds.clone(),
        };
        if snap != self.snap {
            self.snap = snap;
            self.dirty = true;
        }
    }

    fn cancel_answer_reads(&mut self, tab: &str) {
        for i in 0..ANSWER_READS_MS.len() {
            self.timers.cancel(tab, Slot::AnswerRead(i as u8));
        }
    }

    /// Ask the poll to look unless it already reported the tab or one was asked within the gap.
    fn ask_look(&mut self, now: u64, tab: &str, effects: &mut Vec<Effect>) {
        if self.polled.contains(tab) {
            return;
        }
        self.ask_look_gap(now, tab, effects);
    }

    /// A title or a hook names `kind` for a tab the poll counts as another agent: the poll is asked
    /// again at once, even though it already reported the tab (its kind may have changed under it).
    fn ask_look_for_kind(
        &mut self,
        now: u64,
        tab: &str,
        kind: DetectedAgent,
        effects: &mut Vec<Effect>,
    ) {
        if self.agent_kinds.get(tab).is_some_and(|k| *k != kind) {
            self.ask_look_gap(now, tab, effects);
        } else {
            self.ask_look(now, tab, effects);
        }
    }

    fn ask_look_gap(&mut self, now: u64, tab: &str, effects: &mut Vec<Effect>) {
        if self
            .look_asked
            .get(tab)
            .is_some_and(|t| now.saturating_sub(*t) < LOOK_GAP_MS)
        {
            return;
        }
        self.look_asked.insert(tab.to_string(), now);
        effects.push(Effect::AskLook);
    }

    fn on_poll(&mut self, now: u64, tab: &str, present: bool, kind: Option<DetectedAgent>) {
        let was = self.polled.contains(tab);
        if present && !was {
            self.why(tab, "poll found agent");
        } else if !present && (was || self.hooked.contains_key(tab) || self.titled.contains(tab)) {
            self.why(tab, "poll lost agent");
        }
        if present {
            self.polled.insert(tab.to_string());
        } else {
            self.polled.remove(tab);
        }
        // Another agent in this shell now: what Claude's hooks said was about the one that left.
        if !present || kind.is_some_and(|k| k != DetectedAgent::Claude) {
            self.hooked.remove(tab);
        }
        if present && let Some(k) = kind {
            self.agent_kinds.insert(tab.to_string(), k);
        } else if !present {
            self.agent_kinds.remove(tab);
            self.titled.remove(tab);
            self.reader.forget(tab);
            self.timers.cancel(tab, Slot::Fallback);
            self.timers.cancel(tab, Slot::TitleRecheck);
            self.timers.cancel(tab, Slot::Settle);
            self.working.remove(tab);
            self.question.remove(tab);
            self.failed_kinds.remove(tab);
            self.failed.remove(tab);
        }
        if present {
            // An agent's birth state is idle: its startup paint is not the agent answering. Only the FIRST
            // present answer of a tab scored from its output says so: a hook or a title that already set
            // Working said more, and the poll lands seconds into a turn.
            if !was && !self.titled.contains(tab) && !self.hooked.contains_key(tab) {
                self.output_runs.remove(tab);
                self.activity.mark_born(tab, now);
                self.working.remove(tab);
            }
            self.agent_ids.insert(tab.to_string());
        } else {
            self.agent_ids.remove(tab);
        }
    }

    fn on_title(
        &mut self,
        now: u64,
        tab: &str,
        text: &str,
        screen: &dyn Screen,
        effects: &mut Vec<Effect>,
    ) {
        let Some(r) = self.reader.read(tab, text) else {
            self.log.push(LogItem::Title {
                tab: tab.to_string(),
                state: "none".into(),
                agent: None,
            });
            return;
        };
        let detected = DetectedAgent::from(r.kind);
        // A braille spinner is common currency until the poll found an agent.
        if detected == DetectedAgent::Codex && !self.agent_kinds.contains_key(tab) {
            return;
        }
        self.log.push(LogItem::Title {
            tab: tab.to_string(),
            state: r.state.as_str().into(),
            agent: Some(r.kind.as_str().into()),
        });
        self.ask_look_for_kind(now, tab, detected, effects);
        if self.titled.insert(tab.to_string()) {
            self.output_runs.remove(tab);
            self.timers.cancel(tab, Slot::Fallback);
        }
        self.agent_kinds.entry(tab.to_string()).or_insert(detected);
        self.agent_ids.insert(tab.to_string());
        self.title_state.insert(tab.to_string(), r.state);
        if self.hooked.contains_key(tab) {
            match r.state {
                AgentTitleState::Idle => self.apply_hook(now, tab, HookEvent::IdleTitle, screen),
                AgentTitleState::Working => {
                    self.apply_hook(now, tab, HookEvent::WorkingTitle, screen);
                }
                _ => {}
            }
            return;
        }
        self.why(tab, &format!("title {}", r.state.as_str()));
        match r.state {
            AgentTitleState::Idle => self.check_question(now, tab),
            AgentTitleState::Question => {
                if !self.question.contains(tab) {
                    self.cancel_answer_reads(tab);
                }
                self.question.insert(tab.to_string());
            }
            _ => {
                self.question.remove(tab);
            }
        }
        if r.state == AgentTitleState::Working {
            self.working.insert(tab.to_string());
        } else {
            self.working.remove(tab);
        }
    }

    fn on_signal(
        &mut self,
        now: u64,
        tab: &str,
        signal: AgentSignal,
        screen: &dyn Screen,
        effects: &mut Vec<Effect>,
    ) {
        self.log.push(LogItem::Hook {
            tab: tab.to_string(),
            state: signal.state.as_str().into(),
            kind: signal.kind.clone(),
        });
        self.ask_look_for_kind(now, tab, DetectedAgent::Claude, effects);
        if !self.hooked.contains_key(tab) {
            self.output_runs.remove(tab);
            self.timers.cancel(tab, Slot::Fallback);
            self.timers.cancel(tab, Slot::QuestionRead);
        }
        self.agent_kinds
            .entry(tab.to_string())
            .or_insert(DetectedAgent::Claude);
        self.agent_ids.insert(tab.to_string());
        self.apply_hook(now, tab, HookEvent::Signal(signal), screen);
    }

    /// PT `onTermData`: a hooked session says everything itself; a titled one only has its idle
    /// question box read; the rest is scored by a sustained run of output.
    fn on_output(&mut self, now: u64, tab: &str) {
        if self.hooked.contains_key(tab) {
            return;
        }
        if self.titled.contains(tab) {
            if self.title_state.get(tab) == Some(&AgentTitleState::Idle) {
                self.check_question(now, tab);
            }
            return;
        }
        let startup = self.activity.startup_output(tab, now);
        if startup
            || self.activity.suppressed(tab, now)
            || self.activity.input_echo(tab, now, ECHO_MS)
        {
            return;
        }
        if !self.agent_kinds.contains_key(tab) {
            return;
        }
        let run = match self.output_runs.get(tab) {
            Some((start, last)) if now.saturating_sub(*last) <= RUN_GAP_MS => (*start, now),
            _ => (now, now),
        };
        self.output_runs.insert(tab.to_string(), run);
        if run.1 - run.0 > RUN_SUSTAIN_MS {
            self.why(tab, "output scored working");
            self.working.insert(tab.to_string());
            self.timers.arm(
                tab,
                Slot::Fallback,
                now + FALLBACK_QUIET_MS,
                Action::FallbackClear,
            );
        }
    }

    /// PT `onTermKey`: a key that settles a box reads the screen again, four times, until it is gone.
    fn on_key(&mut self, now: u64, tab: &str, key: &str) {
        if !answers_question(key) || !self.question.contains(tab) {
            return;
        }
        self.cancel_answer_reads(tab);
        for (i, ms) in ANSWER_READS_MS.iter().enumerate() {
            self.timers.arm(
                tab,
                Slot::AnswerRead(i as u8),
                now + ms,
                Action::AnswerCheck(key.to_string()),
            );
        }
    }

    /// Arms the screen read of an idle title's question box.
    fn check_question(&mut self, now: u64, tab: &str) {
        if self.timers.armed(tab, Slot::QuestionRead) {
            return;
        }
        self.timers.arm(
            tab,
            Slot::QuestionRead,
            now + QUESTION_READ_MS,
            Action::ReadQuestion,
        );
    }

    /// Arms the re-read of a session in `Background`, unless a settle is already armed.
    fn arm_bg_check(&mut self, now: u64, tab: &str) {
        if self.timers.armed(tab, Slot::Settle) {
            return;
        }
        self.timers.arm(
            tab,
            Slot::Settle,
            now + BACKGROUND_CHECK_MS,
            Action::Hook(HookEventKind::BgCheck),
        );
    }

    fn apply_hook(&mut self, now: u64, tab: &str, ev: HookEvent, screen: &dyn Screen) {
        let prev = self.hooked.get(tab).cloned();
        let box_up = screen_decides(prev.as_ref(), &ev)
            && looks_like_question(&screen.tail(tab, SCREEN_TAIL_ROWS));
        let turn_ends = matches!(
            &ev,
            HookEvent::Settle | HookEvent::BgCheck | HookEvent::IdleTitle
        ) || is_done(&ev);
        let prev_phase = prev.as_ref().map(|p| p.phase);
        let bg_up = turn_ends
            && (ev != HookEvent::IdleTitle
                || matches!(prev_phase, Some(HookPhase::Ending | HookPhase::Background)))
            && looks_like_background_work(&screen.tail(tab, SCREEN_TAIL_ROWS));
        let title_working = self.title_state.get(tab) == Some(&AgentTitleState::Working);
        let seen = HookSeen {
            question_on_screen: box_up,
            title_working: (is_done(&ev) || ev == HookEvent::BgCheck) && title_working,
            background_on_screen: bg_up,
        };
        let Some(o) = hook_step(prev.as_ref(), &ev, &seen) else {
            if prev_phase == Some(HookPhase::Background) {
                self.arm_bg_check(now, tab);
            }
            if box_up
                && ev == HookEvent::WorkingTitle
                && !self.timers.armed(tab, Slot::TitleRecheck)
            {
                self.timers.arm(
                    tab,
                    Slot::TitleRecheck,
                    now + TITLE_RECHECK_MS,
                    Action::TitleRecheck,
                );
            }
            return;
        };
        self.hooked.insert(
            tab.to_string(),
            HookSession {
                phase: o.phase,
                kind: o.kind.clone(),
            },
        );
        self.timers.cancel(tab, Slot::Settle);
        if o.phase == HookPhase::Ending {
            self.timers.arm(
                tab,
                Slot::Settle,
                now + STOP_SETTLE_MS,
                Action::Hook(HookEventKind::Settle),
            );
        }
        if o.phase == HookPhase::Background {
            self.arm_bg_check(now, tab);
        }
        // The rule that fired, for the log.
        let mut why = match &ev {
            HookEvent::BgCheck => "background work over".to_string(),
            _ if bg_up && o.phase == HookPhase::Background => {
                "work still running on screen".to_string()
            }
            HookEvent::Settle => "no title after a Stop".to_string(),
            HookEvent::IdleTitle => {
                if matches!(prev_phase, Some(HookPhase::Ending | HookPhase::Background)) {
                    let mut w = "idle title after a Stop".to_string();
                    if prev_phase == Some(HookPhase::Background) {
                        w.push_str(" and background work");
                    }
                    w
                } else {
                    "idle title after work (an Esc)".to_string()
                }
            }
            HookEvent::WorkingTitle => format!(
                "spinner after {}",
                prev_phase.map_or("nothing", HookPhase::as_str)
            ),
            HookEvent::Signal(s) if s.state == HookState::Done && o.phase == HookPhase::Ending => {
                "hook done, spinner still up".to_string()
            }
            HookEvent::Signal(s) => format!("hook {}", s.state.as_str()),
        };
        if box_up {
            why.push_str(", box on screen");
        }
        let looked = self.looked_at(tab);
        if looked {
            why.push_str(", tab in front");
        }
        self.why(tab, &why);

        if o.phase == HookPhase::Failed
            && let Some(k) = &o.kind
        {
            self.failed_kinds.insert(tab.to_string(), k.clone());
        } else if o.phase == HookPhase::Failed || o.clear.contains(&AttentionMark::Failed) {
            self.failed_kinds.remove(tab);
        }
        let away = !looked;
        if o.raise.contains(&AttentionMark::Question) {
            self.cancel_answer_reads(tab);
            self.timers.cancel(tab, Slot::TitleRecheck);
        }
        for mark in [
            AttentionMark::Finished,
            AttentionMark::Question,
            AttentionMark::Failed,
        ] {
            let set = match mark {
                AttentionMark::Finished => &mut self.done,
                AttentionMark::Question => &mut self.question,
                AttentionMark::Failed => &mut self.failed,
            };
            if (away || raised_while_seen(mark)) && o.raise.contains(&mark) {
                set.insert(tab.to_string());
            } else if o.clear.contains(&mark) {
                set.remove(tab);
            }
        }
        if o.working {
            self.working.insert(tab.to_string());
        } else {
            self.working.remove(tab);
        }
    }

    fn run_action(&mut self, now: u64, tab: &str, action: Action, screen: &dyn Screen) {
        match action {
            Action::ReadQuestion => {
                let asking = self.title_state.get(tab) == Some(&AgentTitleState::Idle)
                    && looks_like_question(&screen.tail(tab, SCREEN_TAIL_ROWS));
                self.why(
                    tab,
                    if asking {
                        "screen read: question box"
                    } else {
                        "screen read: no question box"
                    },
                );
                if asking {
                    self.question.insert(tab.to_string());
                } else {
                    self.question.remove(tab);
                }
            }
            Action::AnswerCheck(key) => {
                if !question_answered(&key, &screen.tail(tab, SCREEN_TAIL_ROWS)) {
                    return;
                }
                self.cancel_answer_reads(tab);
                self.why(tab, "answer key, box gone");
                self.question.remove(tab);
            }
            Action::TitleRecheck => {
                if self.title_state.get(tab) == Some(&AgentTitleState::Working) {
                    self.apply_hook(now, tab, HookEvent::WorkingTitle, screen);
                }
            }
            Action::Hook(HookEventKind::Settle) => {
                self.apply_hook(now, tab, HookEvent::Settle, screen);
            }
            Action::Hook(HookEventKind::BgCheck) => {
                self.apply_hook(now, tab, HookEvent::BgCheck, screen);
            }
            Action::FallbackClear => {
                self.why(tab, "output went quiet");
                self.working.remove(tab);
            }
        }
    }

    /// PT's render effect, called at the end of every entry point: the clock, finished-while-away,
    /// and the marks a look takes down.
    fn reconcile(&mut self, now: u64) {
        self.clock.note_working(&self.working, now);
        let was = std::mem::replace(&mut self.prev_working, self.working.clone());
        for id in &was {
            if !self.working.contains(id)
                && self.agent_ids.contains(id)
                && !self.seeing(now, id)
                && !self.hooked.contains_key(id)
            {
                self.why(id, "stopped working while away");
                self.done.insert(id.clone());
            }
        }
        let mut drop_done = Vec::new();
        for id in &self.done {
            let why = if self.working.contains(id) {
                "working again"
            } else if self.seeing(now, id) {
                "tab looked at"
            } else if !self.agent_ids.contains(id) {
                "agent gone"
            } else {
                continue;
            };
            drop_done.push((id.clone(), why));
        }
        for (id, why) in drop_done {
            self.why(&id, why);
            self.done.remove(&id);
        }
        // A look never clears a question.
        let gone: Vec<String> = self
            .question
            .iter()
            .filter(|id| !self.agent_ids.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            self.why(&id, "agent gone");
            self.question.remove(&id);
        }
        let mut drop_failed = Vec::new();
        for id in &self.failed {
            let why = if self.seeing(now, id) {
                "tab looked at"
            } else if self.working.contains(id) {
                "working again"
            } else if !self.agent_ids.contains(id) {
                "agent gone"
            } else {
                continue;
            };
            drop_failed.push((id.clone(), why));
        }
        for (id, why) in drop_failed {
            self.why(&id, why);
            self.failed.remove(&id);
        }
        self.note_changes();
    }
}

fn is_done(ev: &HookEvent) -> bool {
    matches!(
        ev,
        HookEvent::Signal(AgentSignal {
            state: HookState::Done,
            ..
        })
    )
}

/// Screens shared by this file's tests (Task 18 uses them too).
#[cfg(test)]
mod testdata {
    fn rows(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| (*s).to_string()).collect()
    }

    /// The question tool's box.
    pub fn question_box() -> Vec<String> {
        rows(&[
            "● Which colour do you prefer?",
            "",
            "  ❯ 1. Red",
            "    2. Blue",
            "    3. Type something.",
            "",
            "Enter to select · ↑/↓ to navigate · Esc to cancel",
        ])
    }

    /// Claude's screen after a finished answer.
    pub fn finished() -> Vec<String> {
        rows(&[
            "● I ran the command and it printed: hello-signals",
            "",
            "✻ Worked for 7s · done 18:49",
            "",
            "────────────────────────────────────────",
            "❯ ",
            "────────────────────────────────────────",
            "  ⏵⏵ auto mode on (shift+tab to cycle)",
        ])
    }

    /// A background shell still running under the turn's summary.
    pub fn background() -> Vec<String> {
        rows(&["✻ Churned for 8s · done 7:49 · 1 shell still running"])
    }
}

#[cfg(test)]
mod tests {
    use super::testdata::{background, finished, question_box};
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    struct FakeScreen {
        rows: HashMap<String, Vec<String>>,
        reads: Cell<usize>,
    }

    impl FakeScreen {
        fn set(&mut self, tab: &str, rows: Vec<String>) {
            self.rows.insert(tab.to_string(), rows);
        }
    }

    impl Screen for FakeScreen {
        fn tail(&self, tab: &str, _rows: usize) -> Vec<String> {
            self.reads.set(self.reads.get() + 1);
            self.rows.get(tab).cloned().unwrap_or_default()
        }
    }

    struct H {
        ind: Indicator,
        scr: FakeScreen,
    }

    impl H {
        fn new() -> Self {
            Self {
                ind: Indicator::new(),
                scr: FakeScreen::default(),
            }
        }

        /// A harness whose front tab is "front", focused.
        fn fronted() -> Self {
            let mut h = Self::new();
            h.ind.set_looking(0, Some("front"), true);
            h
        }

        fn ev(&mut self, now: u64, ev: Event) -> Vec<Effect> {
            self.ind.handle(now, ev, &self.scr)
        }

        fn title(&mut self, now: u64, tab: &str, text: &str) -> Vec<Effect> {
            self.ev(
                now,
                Event::Title {
                    tab: tab.into(),
                    text: text.into(),
                },
            )
        }

        fn hook(&mut self, now: u64, tab: &str, state: HookState) {
            self.hook_k(now, tab, state, None);
        }

        fn hook_k(&mut self, now: u64, tab: &str, state: HookState, kind: Option<&str>) {
            self.ev(
                now,
                Event::Signal {
                    tab: tab.into(),
                    signal: AgentSignal {
                        state,
                        kind: kind.map(str::to_string),
                        session: None,
                    },
                },
            );
        }

        fn poll(&mut self, now: u64, tab: &str, present: bool, kind: Option<DetectedAgent>) {
            self.ev(
                now,
                Event::Poll {
                    tab: tab.into(),
                    present,
                    kind,
                },
            );
        }

        fn tick(&mut self, now: u64) {
            self.ind.tick(now, &self.scr);
        }
    }

    fn has(set: &HashSet<String>, tab: &str) -> bool {
        set.contains(tab)
    }

    #[test]
    fn a_poll_report_makes_the_agent_present_and_births_it() {
        let mut h = H::new();
        h.poll(100, "t1", true, Some(DetectedAgent::Claude));
        assert!(has(h.ind.agent_ids(), "t1"));
        assert_eq!(h.ind.agent_kind("t1"), Some(DetectedAgent::Claude));
        assert!(h.ind.is_polled("t1"));
        assert!(h.ind.activity.startup_output("t1", 100));
    }

    #[test]
    fn the_poll_losing_an_agent_drops_what_it_asked_for() {
        let mut h = H::fronted();
        h.poll(0, "t1", true, Some(DetectedAgent::Claude));
        h.hook(10, "t1", HookState::Working);
        h.hook_k(20, "t1", HookState::Failed, Some("rate_limit"));
        assert!(has(h.ind.failed_ids(), "t1"));
        h.hook(30, "t1", HookState::Question);
        assert!(has(h.ind.question_ids(), "t1"));
        h.hook(40, "t1", HookState::Working);
        h.hook_k(50, "t1", HookState::Failed, Some("rate_limit"));
        h.poll(60, "t1", false, None);
        assert!(!has(h.ind.agent_ids(), "t1"));
        assert!(!has(h.ind.working_ids(), "t1"));
        assert!(!has(h.ind.question_ids(), "t1"));
        assert!(!has(h.ind.failed_ids(), "t1"));
        assert_eq!(h.ind.failed_kind("t1"), None);
        assert!(!h.ind.is_hooked("t1"));
    }

    #[test]
    fn a_poll_landing_mid_turn_keeps_the_working_a_hook_set_and_raises_no_finished() {
        let mut h = H::new();
        h.hook(0, "t1", HookState::Working);
        assert!(has(h.ind.working_ids(), "t1"));
        // The first present answer comes seconds into the turn.
        h.poll(3000, "t1", true, Some(DetectedAgent::Claude));
        assert!(has(h.ind.working_ids(), "t1"), "the hook's Working stays");
        assert!(h.ind.done_ids().is_empty(), "and nothing finished");
    }

    #[test]
    fn a_poll_landing_mid_turn_keeps_the_working_a_title_set() {
        let mut h = H::new();
        h.title(0, "t1", "◐ Claude Code");
        let was_working = has(h.ind.working_ids(), "t1");
        h.poll(3000, "t1", true, Some(DetectedAgent::Claude));
        assert_eq!(has(h.ind.working_ids(), "t1"), was_working);
        assert!(h.ind.done_ids().is_empty());
    }

    #[test]
    fn another_agent_in_the_shell_forgets_what_claudes_hooks_said() {
        let mut h = H::new();
        h.hook(0, "t1", HookState::Working);
        assert!(h.ind.is_hooked("t1"));
        h.poll(10, "t1", true, Some(DetectedAgent::Codex));
        assert!(!h.ind.is_hooked("t1"));
    }

    #[test]
    fn a_claude_title_is_the_agent_present_ahead_of_the_poll_and_asks_a_look() {
        let mut h = H::new();
        assert_eq!(h.title(0, "t1", "◐ Claude Code"), vec![Effect::AskLook]);
        assert!(has(h.ind.agent_ids(), "t1"));
        assert_eq!(h.ind.agent_kind("t1"), Some(DetectedAgent::Claude));
        assert!(h.title(1000, "t1", "◐ Claude Code").is_empty());
        assert_eq!(
            h.title(15_000, "t1", "◐ Claude Code"),
            vec![Effect::AskLook]
        );
        h.poll(16_000, "t1", true, Some(DetectedAgent::Claude));
        assert!(h.title(40_000, "t1", "◐ Claude Code").is_empty());
    }

    #[test]
    fn a_claude_title_over_a_tab_the_poll_counts_as_codex_asks_for_a_new_look() {
        let mut h = H::new();
        h.poll(0, "t1", true, Some(DetectedAgent::Codex));
        // The poll already reported the tab, yet Claude's title says the agent changed.
        assert_eq!(
            h.title(1000, "t1", "\u{25d0} Claude Code"),
            vec![Effect::AskLook]
        );
        // The poll's next answer names the new kind.
        h.poll(1500, "t1", true, Some(DetectedAgent::Claude));
        assert_eq!(h.ind.agent_kind("t1"), Some(DetectedAgent::Claude));
    }

    #[test]
    fn a_codex_braille_title_is_ignored_until_the_poll_found_an_agent() {
        let mut h = H::new();
        assert!(h.title(0, "t1", "⠙ proj").is_empty());
        assert!(h.ind.agent_ids().is_empty());
        h.poll(10, "t1", true, Some(DetectedAgent::Codex));
        h.title(20, "t1", "⠙ proj");
        assert!(has(h.ind.agent_ids(), "t1"));
        assert!(!has(h.ind.working_ids(), "t1"));
    }

    #[test]
    fn an_unhooked_claude_title_drives_working() {
        let mut h = H::new();
        h.title(0, "t1", "✳ Claude Code");
        assert!(!has(h.ind.working_ids(), "t1"));
        h.title(10, "t1", "◐ Claude Code");
        assert!(has(h.ind.working_ids(), "t1"));
        h.title(20, "t1", "✳ Fix");
        assert!(!has(h.ind.working_ids(), "t1"));
    }

    #[test]
    fn starting_is_present_but_not_working() {
        let mut h = H::new();
        h.title(0, "t1", "◐ Claude Code");
        assert!(has(h.ind.agent_ids(), "t1"));
        assert!(!has(h.ind.working_ids(), "t1"));
    }

    #[test]
    fn codex_asking_is_a_question_until_it_works_again() {
        let mut h = H::new();
        h.poll(0, "t1", true, Some(DetectedAgent::Codex));
        h.title(10, "t1", "⠙ proj");
        h.title(20, "t1", "proj");
        h.title(30, "t1", "[ ! ] Action Required | proj");
        assert!(has(h.ind.question_ids(), "t1"));
        h.title(40, "t1", "⠦ proj");
        assert!(!has(h.ind.question_ids(), "t1"));
        assert!(has(h.ind.working_ids(), "t1"));
    }

    #[test]
    fn a_hook_makes_the_session_hooked_and_present() {
        let mut h = H::new();
        h.hook(0, "t1", HookState::Working);
        assert!(h.ind.is_hooked("t1"));
        assert!(has(h.ind.agent_ids(), "t1"));
        assert_eq!(h.ind.agent_kind("t1"), Some(DetectedAgent::Claude));
        assert!(has(h.ind.working_ids(), "t1"));
        let log = h.ind.drain_log();
        assert!(log.contains(&LogItem::Hook {
            tab: "t1".into(),
            state: "working".into(),
            kind: None
        }));
        assert!(log.contains(&LogItem::Why {
            tab: "t1".into(),
            why: "hook working".into()
        }));
    }

    #[test]
    fn finished_is_raised_on_a_tab_nobody_is_looking_at() {
        let mut h = H::fronted();
        h.hook(0, "t1", HookState::Working);
        h.hook(10, "t1", HookState::Done);
        assert!(has(h.ind.done_ids(), "t1"));
        assert!(!has(h.ind.working_ids(), "t1"));
    }

    #[test]
    fn finished_is_not_raised_on_the_tab_in_front_but_a_question_is() {
        let mut h = H::new();
        h.ind.set_looking(0, Some("t1"), true);
        h.hook(1, "t1", HookState::Working);
        h.hook(2, "t1", HookState::Done);
        assert!(h.ind.done_ids().is_empty());
        h.hook(3, "t1", HookState::Working);
        h.hook(4, "t1", HookState::Question);
        assert!(has(h.ind.question_ids(), "t1"));
    }

    #[test]
    fn a_failure_is_failed_and_finished_with_its_kind() {
        let mut h = H::fronted();
        h.hook(0, "t1", HookState::Working);
        h.hook_k(10, "t1", HookState::Failed, Some("rate_limit"));
        assert!(has(h.ind.failed_ids(), "t1"));
        assert_eq!(h.ind.failed_kind("t1"), Some("rate_limit"));
        assert!(has(h.ind.done_ids(), "t1"));
    }

    #[test]
    fn a_new_prompt_clears_failed_finished_and_the_kind() {
        let mut h = H::fronted();
        h.hook(0, "t1", HookState::Working);
        h.hook_k(10, "t1", HookState::Failed, Some("rate_limit"));
        h.hook(20, "t1", HookState::Working);
        assert!(!has(h.ind.failed_ids(), "t1"));
        assert!(!has(h.ind.done_ids(), "t1"));
        assert_eq!(h.ind.failed_kind("t1"), None);
    }

    #[test]
    fn an_idle_title_after_work_is_an_esc_no_finished() {
        let mut h = H::fronted();
        h.hook(0, "t1", HookState::Working);
        h.title(10, "t1", "✳ Claude Code");
        assert!(!has(h.ind.working_ids(), "t1"));
        assert!(!has(h.ind.done_ids(), "t1"));
    }

    #[test]
    fn a_hooked_stop_after_the_idle_title_still_finishes() {
        let mut h = H::fronted();
        h.hook(0, "t1", HookState::Working);
        h.title(10, "t1", "✳ Claude Code");
        h.hook(20, "t1", HookState::Done);
        assert!(has(h.ind.done_ids(), "t1"));
    }

    #[test]
    fn looking_takes_down_finished_and_failed_not_question() {
        let mut h = H::fronted();
        h.hook(0, "t1", HookState::Working);
        h.hook(10, "t1", HookState::Done);
        h.hook(20, "t1", HookState::Question);
        assert!(has(h.ind.done_ids(), "t1"));
        assert!(has(h.ind.question_ids(), "t1"));
        h.hook(30, "t2", HookState::Working);
        h.hook_k(40, "t2", HookState::Failed, Some("overloaded"));
        assert!(has(h.ind.failed_ids(), "t2"));
        h.ind.set_looking(50, Some("t1"), true);
        assert!(!has(h.ind.done_ids(), "t1"));
        assert!(has(h.ind.question_ids(), "t1"));
        h.ind.set_looking(60, Some("t2"), true);
        assert!(!has(h.ind.failed_ids(), "t2"));
        assert!(!has(h.ind.done_ids(), "t2"));
    }

    #[test]
    fn a_pointer_move_over_an_unfocused_window_is_a_look() {
        let mut h = H::new();
        h.ind.set_looking(0, Some("t1"), false);
        h.hook(100, "t1", HookState::Working);
        h.hook(200, "t1", HookState::Done);
        assert!(has(h.ind.done_ids(), "t1"));
        h.ind.pointer_moved(300);
        assert!(!has(h.ind.done_ids(), "t1"));
        // A new mark after the look window has passed stays until the next move.
        h.hook(5000, "t1", HookState::Working);
        h.hook(5100, "t1", HookState::Done);
        assert!(has(h.ind.done_ids(), "t1"));
        h.ind.pointer_moved(5200);
        assert!(!has(h.ind.done_ids(), "t1"));
    }

    #[test]
    fn a_pointer_move_while_focused_does_nothing() {
        let mut h = H::new();
        h.ind.set_looking(0, Some("other"), true);
        h.hook(1, "t1", HookState::Working);
        h.hook(2, "t1", HookState::Done);
        h.ind.pointer_moved(10);
        assert!(has(h.ind.done_ids(), "t1"));
        assert!(h.ind.moved_at.is_none());
    }

    #[test]
    fn coming_back_is_a_look_but_a_resting_pointer_is_not() {
        let mut h = H::new();
        h.ind.set_looking(0, Some("t1"), false);
        h.hook(100, "t1", HookState::Working);
        h.hook(200, "t1", HookState::Done);
        h.tick(10_000);
        assert!(has(h.ind.done_ids(), "t1"));
        h.ind.set_looking(11_000, Some("t1"), true);
        assert!(!has(h.ind.done_ids(), "t1"));
    }

    #[test]
    fn working_again_clears_done_and_failed() {
        let mut h = H::fronted();
        h.hook(0, "t1", HookState::Working);
        h.hook(10, "t1", HookState::Done);
        assert!(has(h.ind.done_ids(), "t1"));
        h.hook(20, "t1", HookState::Working);
        assert!(!has(h.ind.done_ids(), "t1"));
        assert!(!has(h.ind.failed_ids(), "t1"));
    }

    #[test]
    fn an_unhooked_session_that_stops_while_away_is_finished() {
        let mut h = H::fronted();
        h.title(0, "t1", "✳ Claude Code");
        h.title(10, "t1", "◐ Claude Code");
        assert!(has(h.ind.working_ids(), "t1"));
        h.title(20, "t1", "✳ Claude Code");
        assert!(has(h.ind.done_ids(), "t1"));
        assert!(h.ind.drain_log().contains(&LogItem::Why {
            tab: "t1".into(),
            why: "stopped working while away".into()
        }));
    }

    #[test]
    fn a_hooked_session_gets_done_only_from_its_hooks() {
        let mut h = H::fronted();
        h.hook(0, "t1", HookState::Working);
        h.title(10, "t1", "✳ Claude Code");
        assert!(!has(h.ind.done_ids(), "t1"));
    }

    #[test]
    fn forget_drops_every_mark_record_and_timer() {
        let mut h = H::fronted();
        h.poll(0, "t2", true, Some(DetectedAgent::Claude));
        h.poll(0, "t1", true, Some(DetectedAgent::Claude));
        assert_eq!(h.ind.activity.output_clocks(), 2);
        h.title(1, "t1", "✳ Claude Code");
        h.title(2, "t1", "◐ Claude Code");
        h.hook(3, "t1", HookState::Working);
        h.hook(4, "t1", HookState::Done);
        assert!(h.ind.next_due().is_some());
        h.hook(5, "t1", HookState::Question);
        h.ind.forget("t1");
        assert!(h.ind.agent_ids().iter().all(|t| t != "t1"));
        assert!(!has(h.ind.working_ids(), "t1"));
        assert!(!has(h.ind.done_ids(), "t1"));
        assert!(!has(h.ind.question_ids(), "t1"));
        assert!(!has(h.ind.failed_ids(), "t1"));
        assert!(!h.ind.is_hooked("t1"));
        assert!(!h.ind.is_polled("t1"));
        assert_eq!(h.ind.next_due(), None);
        // Only the forgotten tab's activity goes.
        assert_eq!(h.ind.activity.output_clocks(), 1);
        assert!(h.ind.take_changed());
    }

    #[test]
    fn the_clock_runs_while_working_and_is_forgotten_when_it_stops() {
        let mut h = H::fronted();
        h.hook(1000, "t1", HookState::Working);
        assert_eq!(h.ind.working_for("t1", 4000), Some(3000));
        h.hook(5000, "t1", HookState::Done);
        assert_eq!(h.ind.working_for("t1", 6000), None);
    }

    #[test]
    fn a_stop_with_the_spinner_up_waits_for_the_next_title() {
        let mut h = H::fronted();
        h.title(0, "t1", "✳ Claude Code");
        h.title(10, "t1", "◐ Claude Code");
        h.hook(20, "t1", HookState::Working);
        h.hook(1000, "t1", HookState::Done);
        assert!(has(h.ind.working_ids(), "t1"));
        assert!(!has(h.ind.done_ids(), "t1"));
        h.tick(1000 + STOP_SETTLE_MS);
        assert!(has(h.ind.done_ids(), "t1"));
        assert!(!has(h.ind.working_ids(), "t1"));
        assert!(h.ind.drain_log().contains(&LogItem::Why {
            tab: "t1".into(),
            why: "no title after a Stop".into()
        }));
    }

    #[test]
    fn the_star_after_a_stop_finishes_at_once_and_cancels_the_settle() {
        let mut h = H::fronted();
        h.title(0, "t1", "✳ Claude Code");
        h.title(10, "t1", "◐ Claude Code");
        h.hook(20, "t1", HookState::Working);
        h.hook(1000, "t1", HookState::Done);
        assert_eq!(h.ind.next_due(), Some(1000 + STOP_SETTLE_MS));
        h.title(1040, "t1", "✳ Claude Code");
        assert!(has(h.ind.done_ids(), "t1"));
        assert_eq!(h.ind.next_due(), None);
    }

    #[test]
    fn background_work_on_screen_holds_working_and_is_rechecked_every_3_seconds() {
        let mut h = H::fronted();
        h.scr.set("t1", background());
        h.hook(0, "t1", HookState::Working);
        h.hook(1000, "t1", HookState::Done);
        assert!(has(h.ind.working_ids(), "t1"));
        assert!(!has(h.ind.done_ids(), "t1"));
        assert_eq!(h.ind.next_due(), Some(1000 + BACKGROUND_CHECK_MS));
        h.tick(1000 + BACKGROUND_CHECK_MS);
        assert!(has(h.ind.working_ids(), "t1"));
        assert_eq!(h.ind.next_due(), Some(1000 + 2 * BACKGROUND_CHECK_MS));
        h.scr.set("t1", finished());
        h.tick(1000 + 2 * BACKGROUND_CHECK_MS);
        assert!(has(h.ind.done_ids(), "t1"));
        assert!(!has(h.ind.working_ids(), "t1"));
        assert_eq!(h.ind.next_due(), None);
    }

    #[test]
    fn a_question_stands_while_its_box_is_on_screen() {
        let mut h = H::fronted();
        h.scr.set("t1", question_box());
        h.hook(0, "t1", HookState::Question);
        assert!(has(h.ind.question_ids(), "t1"));
        h.hook(10, "t1", HookState::Working);
        assert!(has(h.ind.question_ids(), "t1"));
        assert!(!has(h.ind.working_ids(), "t1"));
        h.hook(20, "t1", HookState::Done);
        assert!(has(h.ind.question_ids(), "t1"));
        assert!(has(h.ind.done_ids(), "t1"));
        h.scr.set("t1", finished());
        h.hook(30, "t1", HookState::Working);
        assert!(!has(h.ind.question_ids(), "t1"));
    }

    #[test]
    fn a_spinner_held_by_the_box_is_read_again_after_250_ms() {
        let mut h = H::fronted();
        h.title(0, "t1", "✳ Claude Code");
        h.scr.set("t1", question_box());
        h.hook(10, "t1", HookState::Question);
        h.title(100, "t1", "◐ Claude Code");
        assert!(has(h.ind.question_ids(), "t1"));
        assert_eq!(h.ind.next_due(), Some(100 + TITLE_RECHECK_MS));
        h.scr.set("t1", finished());
        h.tick(100 + TITLE_RECHECK_MS);
        assert!(!has(h.ind.question_ids(), "t1"));
        assert!(has(h.ind.working_ids(), "t1"));
    }

    #[test]
    fn take_changed_reports_a_set_change_once() {
        let mut h = H::new();
        assert!(!h.ind.take_changed());
        h.hook(0, "t1", HookState::Working);
        assert!(h.ind.take_changed());
        assert!(!h.ind.take_changed());
    }

    // ---- Task 18: scoring, question reads, answer keys, the replay ----

    impl H {
        fn output(&mut self, now: u64, tab: &str) {
            self.ev(now, Event::Output { tab: tab.into() });
        }

        fn key(&mut self, now: u64, tab: &str, key: &str) {
            self.ev(
                now,
                Event::Key {
                    tab: tab.into(),
                    key: key.into(),
                },
            );
        }

        /// A Claude found by the poll whose startup paint is over.
        fn known_agent() -> Self {
            let mut h = Self::fronted();
            h.poll(0, "t1", true, Some(DetectedAgent::Claude));
            h.output(3000, "t1");
            h
        }
    }

    #[test]
    fn output_scores_working_after_a_sustained_run_and_clears_after_two_seconds_of_quiet() {
        let mut h = H::known_agent();
        for t in (5000..=6500).step_by(300) {
            h.output(t, "t1");
            if t <= 6000 {
                assert!(!has(h.ind.working_ids(), "t1"), "at {t}");
            }
        }
        h.output(6500, "t1");
        assert!(has(h.ind.working_ids(), "t1"));
        h.tick(8499);
        assert!(has(h.ind.working_ids(), "t1"));
        h.tick(8500);
        assert!(!has(h.ind.working_ids(), "t1"));
        assert!(h.ind.drain_log().contains(&LogItem::Why {
            tab: "t1".into(),
            why: "output went quiet".into()
        }));
    }

    #[test]
    fn a_gap_over_1500_restarts_the_run() {
        let mut h = H::known_agent();
        for t in [5000, 5300, 7000, 7300] {
            h.output(t, "t1");
        }
        assert!(!has(h.ind.working_ids(), "t1"));
    }

    #[test]
    fn the_startup_paint_after_the_agent_appears_is_not_scored() {
        let mut h = H::fronted();
        h.poll(0, "t1", true, Some(DetectedAgent::Claude));
        for t in (300..=9000).step_by(300) {
            h.output(t, "t1");
            assert!(!has(h.ind.working_ids(), "t1"), "at {t}");
        }
        // After a 2000 ms silence the next run scores normally.
        for t in (11_000..=12_600).step_by(300) {
            h.output(t, "t1");
        }
        assert!(has(h.ind.working_ids(), "t1"));
    }

    #[test]
    fn output_right_behind_a_keystroke_is_an_echo() {
        let mut h = H::known_agent();
        for t in (5000..8000).step_by(100) {
            h.ind.activity.mark_touched("t1", t);
            h.output(t + 50, "t1");
            assert!(!has(h.ind.working_ids(), "t1"), "at {t}");
        }
    }

    #[test]
    fn output_after_a_refit_is_suppressed_for_800_ms() {
        let mut h = H::known_agent();
        h.ind.activity.suppress("t1", 5000, 800);
        for t in (5000..5800).step_by(100) {
            h.output(t, "t1");
        }
        h.output(5800, "t1");
        assert!(!has(h.ind.working_ids(), "t1"));
        // The window is the only thing that blocked it: the run starts after it.
        for t in (5900..=7300).step_by(200) {
            h.output(t, "t1");
        }
        assert!(has(h.ind.working_ids(), "t1"));
    }

    #[test]
    fn only_a_known_agent_is_scored() {
        let mut h = H::fronted();
        for t in (0..6000).step_by(300) {
            h.output(t, "t1");
        }
        assert!(h.ind.working_ids().is_empty());
        assert_eq!(h.ind.next_due(), None);
    }

    #[test]
    fn a_titled_session_is_never_scored_from_its_output() {
        let mut h = H::fronted();
        h.title(0, "t1", "✳ Claude Code");
        for t in (5000..9000).step_by(300) {
            h.output(t, "t1");
        }
        assert!(!has(h.ind.working_ids(), "t1"));
        // Idle title: an output chunk arms the 200 ms question read.
        assert!(h.ind.next_due().is_some());
    }

    #[test]
    fn a_hooked_session_is_never_scored_and_its_screen_is_not_read_for_output() {
        let mut h = H::fronted();
        h.scr.set("t1", question_box());
        h.hook(0, "t1", HookState::Working);
        let before = h.scr.reads.get();
        for i in 0..100 {
            h.output(1000 + i * 50, "t1");
        }
        h.tick(10_000);
        assert_eq!(h.scr.reads.get(), before);
        assert!(has(h.ind.working_ids(), "t1"));
        assert_eq!(h.ind.next_due(), None);
    }

    #[test]
    fn an_idle_titled_session_reads_the_screen_for_a_question_200_ms_after_output() {
        let mut h = H::fronted();
        h.title(0, "t1", "✳ Claude Code");
        h.tick(500);
        h.scr.set("t1", question_box());
        h.output(1000, "t1");
        h.tick(1199);
        assert!(!has(h.ind.question_ids(), "t1"));
        h.tick(1200);
        assert!(has(h.ind.question_ids(), "t1"));
        assert!(h.ind.drain_log().contains(&LogItem::Why {
            tab: "t1".into(),
            why: "screen read: question box".into()
        }));
        h.scr.set("t1", finished());
        h.output(2000, "t1");
        h.tick(2200);
        assert!(!has(h.ind.question_ids(), "t1"));
    }

    #[test]
    fn a_second_output_inside_the_read_does_not_arm_another() {
        let mut h = H::fronted();
        h.title(0, "t1", "✳ Claude Code");
        // The title itself armed a read; settle it first.
        h.tick(500);
        h.output(1000, "t1");
        h.output(1010, "t1");
        assert_eq!(h.ind.next_due(), Some(1200));
        h.tick(1200);
        assert_eq!(h.ind.next_due(), None);
    }

    /// A harness with a pending hooked question on t1 (the box up).
    fn asking() -> H {
        let mut h = H::fronted();
        h.scr.set("t1", question_box());
        h.hook(0, "t1", HookState::Working);
        h.hook(10, "t1", HookState::Question);
        assert!(has(h.ind.question_ids(), "t1"));
        h
    }

    #[test]
    fn walking_the_choices_is_not_an_answer() {
        let mut h = asking();
        for k in ["\x1b[B", "a", "0"] {
            h.key(100, "t1", k);
        }
        assert_eq!(h.ind.next_due(), None);
    }

    #[test]
    fn an_answer_key_reads_the_box_at_250_1000_2500_and_5000_and_clears_only_when_it_is_gone() {
        let mut h = asking();
        h.key(1000, "t1", "\r");
        assert_eq!(h.ind.next_due(), Some(1250));
        h.tick(1250);
        h.tick(2000);
        assert!(has(h.ind.question_ids(), "t1"));
        h.scr.set("t1", finished());
        h.tick(3499);
        assert!(has(h.ind.question_ids(), "t1"));
        h.tick(3500);
        assert!(!has(h.ind.question_ids(), "t1"));
        assert_eq!(h.ind.next_due(), None);
    }

    #[test]
    fn an_answer_key_with_the_next_question_still_up_keeps_the_question() {
        let mut h = asking();
        h.key(1000, "t1", "1");
        h.tick(6000);
        assert!(has(h.ind.question_ids(), "t1"));
        assert_eq!(h.ind.next_due(), None);
    }

    #[test]
    fn a_key_is_ignored_without_a_pending_question() {
        let mut h = H::fronted();
        h.key(0, "t1", "\r");
        assert_eq!(h.ind.next_due(), None);
    }

    #[test]
    fn a_new_question_cancels_pending_answer_reads() {
        let mut h = asking();
        h.key(1000, "t1", "\r");
        assert!(h.ind.next_due().is_some());
        h.hook(1100, "t1", HookState::Working);
        h.scr.set("t1", question_box());
        h.hook(1200, "t1", HookState::Question);
        assert_eq!(h.ind.next_due(), None);

        // A Codex Action Required title does the same.
        let mut h = H::fronted();
        h.poll(0, "t2", true, Some(DetectedAgent::Codex));
        h.title(10, "t2", "[ ! ] Action Required | proj");
        h.key(100, "t2", "\r");
        assert!(h.ind.next_due().is_some());
        h.title(200, "t2", "⠙ proj");
        h.title(300, "t2", "[ ! ] Action Required | proj");
        assert_eq!(h.ind.next_due(), None);
    }

    #[test]
    fn next_due_is_the_earliest_deadline_and_none_when_idle() {
        let mut h = H::fronted();
        assert_eq!(h.ind.next_due(), None);
        h.title(0, "t2", "✳ Claude Code");
        h.title(10, "t2", "◐ Claude Code");
        h.tick(300);
        assert_eq!(h.ind.next_due(), None);
        h.hook(400, "t2", HookState::Working);
        h.title(1450, "t1", "✳ Claude Code"); // a question read is armed
        h.hook(1500, "t2", HookState::Done); // spinner up: a settle is armed
        let read = 1450 + QUESTION_READ_MS;
        let settle = 1500 + STOP_SETTLE_MS;
        assert_eq!(h.ind.next_due(), Some(read.min(settle)));
        h.tick(read.min(settle));
        assert_eq!(h.ind.next_due(), Some(read.max(settle)));
        h.tick(read.max(settle));
        assert_eq!(h.ind.next_due(), None);
    }

    // ---- the measured timelines through the whole indicator ----

    struct Step {
        t: u64,
        event: String,
        box_on: bool,
        later: bool,
    }

    fn timeline(name: &str) -> Vec<Step> {
        let all: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/hook_timelines.json"))
                .expect("fixture parses");
        all[name]
            .as_array()
            .expect("a timeline")
            .iter()
            .map(|row| {
                let row = row.as_array().expect("a step");
                Step {
                    t: row[0].as_u64().expect("ms"),
                    event: row[1].as_str().expect("event").to_owned(),
                    box_on: row[2].as_u64() == Some(1),
                    later: row[3].as_u64() == Some(1),
                }
            })
            .collect()
    }

    /// The box is a function of time: the step's `box` for 250 ms after it, its `later` after that.
    struct TimelineScreen {
        now: Cell<u64>,
        stamps: Vec<(u64, bool, bool)>,
        reads: Cell<usize>,
    }

    impl Screen for TimelineScreen {
        fn tail(&self, _tab: &str, _rows: usize) -> Vec<String> {
            self.reads.set(self.reads.get() + 1);
            let t = self.now.get();
            let up = self
                .stamps
                .iter()
                .rev()
                .find(|(at, _, _)| *at <= t)
                .is_some_and(|(at, b, later)| if t - at < 250 { *b } else { *later });
            if up { question_box() } else { finished() }
        }
    }

    type Shown = Vec<(u64, Option<&'static str>)>;

    fn shown_now(ind: &Indicator) -> Option<&'static str> {
        if ind.working_ids().contains("t1") {
            Some("working")
        } else if ind.question_ids().contains("t1") {
            Some("question")
        } else if ind.failed_ids().contains("t1") {
            Some("failed")
        } else if ind.done_ids().contains("t1") {
            Some("finished")
        } else {
            None
        }
    }

    /// Drives a timeline; `outputs` feeds an Output every 50 ms to a hooked session. Returns what
    /// the tab shows after each step and each deadline, and the screen reads made.
    fn replay(name: &str, outputs: bool) -> (Shown, usize) {
        let steps = timeline(name);
        assert_eq!(steps[0].event, "idle-title");
        let scr = TimelineScreen {
            now: Cell::new(0),
            stamps: steps.iter().map(|s| (s.t, s.box_on, s.later)).collect(),
            reads: Cell::new(0),
        };
        let mut ind = Indicator::new();
        ind.set_looking(0, Some("other"), true);
        let mut shown: Shown = Vec::new();
        let mut cur = 0u64;
        let advance = |ind: &mut Indicator, shown: &mut Shown, cur: &mut u64, to: u64| {
            // Deadlines and (when asked) the 50 ms output grid, in time order.
            loop {
                let due = ind.next_due().filter(|d| *d <= to);
                let grid = if outputs {
                    let g = (*cur / 50 + 1) * 50;
                    (g <= to && ind.is_hooked("t1")).then_some(g)
                } else {
                    None
                };
                let next = match (due, grid) {
                    (Some(d), Some(g)) => d.min(g),
                    (Some(d), None) => d,
                    (None, Some(g)) => g,
                    (None, None) => break,
                };
                *cur = next.max(*cur);
                scr.now.set(*cur);
                if due == Some(next) {
                    ind.tick(next, &scr);
                    shown.push((next, shown_now(ind)));
                }
                if grid == Some(next) {
                    ind.handle(next, Event::Output { tab: "t1".into() }, &scr);
                }
            }
            *cur = to;
        };
        for step in &steps {
            advance(&mut ind, &mut shown, &mut cur, step.t);
            scr.now.set(step.t);
            let ev = match step.event.as_str() {
                "idle-title" => Some(Event::Title {
                    tab: "t1".into(),
                    text: "✳ Claude Code".into(),
                }),
                "working-title" => Some(Event::Title {
                    tab: "t1".into(),
                    text: "◐ Claude Code".into(),
                }),
                "working" | "question" | "done" | "failed" => Some(Event::Signal {
                    tab: "t1".into(),
                    signal: AgentSignal {
                        state: match step.event.as_str() {
                            "working" => HookState::Working,
                            "question" => HookState::Question,
                            "done" => HookState::Done,
                            _ => HookState::Failed,
                        },
                        kind: None,
                        session: None,
                    },
                }),
                "key:submit" => None,
                e if e.starts_with("key:enter") => Some(Event::Key {
                    tab: "t1".into(),
                    key: "\r".into(),
                }),
                e if e.starts_with("key:esc") => Some(Event::Key {
                    tab: "t1".into(),
                    key: "\x1b".into(),
                }),
                e if e.starts_with("key:key-1") => Some(Event::Key {
                    tab: "t1".into(),
                    key: "1".into(),
                }),
                other => panic!("unknown timeline event {other}"),
            };
            if let Some(ev) = ev {
                ind.handle(step.t, ev, &scr);
            }
            shown.push((step.t, shown_now(&ind)));
        }
        let end = cur + 8000;
        advance(&mut ind, &mut shown, &mut cur, end);
        (shown, scr.reads.get())
    }

    fn at(shown: &Shown, t: u64) -> Option<&'static str> {
        let mut v = None;
        for (when, s) in shown {
            if *when <= t {
                v = *s;
            }
        }
        v
    }

    fn holds(shown: &Shown, from: u64, to: u64, want: Option<&'static str>) {
        for t in (from..to).step_by(100) {
            assert_eq!(at(shown, t), want, "at {t} ms");
        }
    }

    #[test]
    fn timeline_a_replayed_through_the_indicator() {
        let (shown, _) = replay("A", false);
        holds(&shown, 13805, 22861, Some("question"));
        holds(&shown, 23111, 49815, Some("working"));
        holds(&shown, 49815, 49857, Some("working"));
        holds(&shown, 49857, 52000, Some("finished"));
    }

    #[test]
    fn timeline_b_replayed_through_the_indicator() {
        let (shown, _) = replay("B", false);
        holds(&shown, 10848, 19939, Some("question"));
        holds(&shown, 20189, 43000, None);
    }

    #[test]
    fn timeline_c_replayed_through_the_indicator() {
        let (shown, _) = replay("C", false);
        holds(&shown, 13120, 22174, Some("question"));
        holds(&shown, 22424, 49244, Some("working"));
        holds(&shown, 49244, 52000, Some("finished"));
    }

    #[test]
    fn timeline_d_replayed_through_the_indicator() {
        let (shown, _) = replay("D", false);
        holds(&shown, 12212, 15344, Some("question"));
        holds(&shown, 15594, 30212, Some("working"));
        holds(&shown, 30227, 33348, Some("question"));
        holds(&shown, 33598, 45465, Some("working"));
        holds(&shown, 45465, 48000, Some("finished"));
    }

    #[test]
    fn timeline_e_replayed_through_the_indicator() {
        let (shown, _) = replay("E", false);
        holds(&shown, 10913, 22664, Some("question"));
        holds(&shown, 22914, 49715, Some("working"));
        holds(&shown, 49715, 52000, Some("finished"));
    }

    #[test]
    fn timeline_g2_replayed_through_the_indicator() {
        let (shown, _) = replay("G2", false);
        holds(&shown, 22267, 36409, Some("question"));
        holds(&shown, 36659, 63980, Some("working"));
        holds(&shown, 63980, 66000, Some("finished"));
    }

    #[test]
    fn timeline_c3_replayed_through_the_indicator() {
        let (shown, _) = replay("C3", false);
        holds(&shown, 10640, 19133, Some("working"));
        holds(&shown, 19133, 20000, Some("finished"));
        for t in (0..54000).step_by(100) {
            assert_ne!(at(&shown, t), Some("question"), "question at {t} ms");
        }
    }

    #[test]
    fn a_hooked_session_in_every_timeline_never_reads_the_screen_for_output() {
        for name in ["A", "B", "C", "D", "E", "G2", "C3"] {
            let (plain, plain_reads) = replay(name, false);
            let (with_output, output_reads) = replay(name, true);
            assert_eq!(plain_reads, output_reads, "timeline {name}");
            // Output changes nothing a hooked session shows.
            for (t, s) in &plain {
                assert_eq!(at(&with_output, *t), *s, "timeline {name} at {t}");
            }
        }
    }
}
