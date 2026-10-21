//! The agent poll: the back-off and the change-only send (Task 9; Prism Terminal's agentPoll.ts).
//!
//! The rules live in `AgentPoll::look`, with time and the outside world passed in, so the measured
//! timelines replay without a thread. `PollWorker` is the thread around it. The process query itself
//! (a native snapshot) is `flare-win`'s; here it is only a blocking closure.
//!
//! Three ideas keep it cheap: ask only when a pty has printed something since the last answer, back off
//! (2.5 s doubling to 20 s) while the answer holds, and let a failing query back off for everyone
//! (doubling to a minute, forced looks included).

use crate::detect::{ProcRow, tree_agent_kind};
use crate::kinds::DetectedAgent;
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const AGENT_POLL_MIN: u64 = 2500;
pub const AGENT_POLL_MAX: u64 = 20_000;
/// A forced look waits this long after the last query started.
pub const AGENT_FORCE_GAP: u64 = 5000;
/// A failing query backs off doubling up to this.
pub const AGENT_FAIL_MAX: u64 = 60_000;

/// What the poll reads the world through. Not `Send`: the worker builds it on its own thread.
pub struct PollDeps {
    /// Live shells: (tab id, shell pid).
    pub pids: Box<dyn Fn() -> Vec<(String, u32)>>,
    /// Pty output ticks (`SessionManager::output_ticks`).
    pub ticks: Box<dyn Fn() -> u64>,
    /// BLOCKS. `None` = the query did not answer. The shell pids say whose descendants need command lines.
    pub query: ProcQuery,
    /// Milliseconds.
    pub now: Box<dyn Fn() -> u64>,
    /// Said once when the query has failed three times in a row.
    pub failing: Option<Box<dyn Fn()>>,
}

/// The blocking process query: shell pids in, process rows out (`None` = no answer).
pub type ProcQuery = Box<dyn Fn(&[u32]) -> Option<Vec<ProcRow>>>;

/// (tab id, has agent, kind): sent on changes only.
pub type AgentSend = Box<dyn Fn(&str, bool, Option<DetectedAgent>)>;

/// The asks other threads leave for the poll: shared, so one that arrives DURING a look is kept for
/// the look after it.
#[derive(Clone, Default)]
pub struct PollHandle {
    asked: Arc<Mutex<HashSet<String>>>,
}

impl PollHandle {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.asked.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// That session's next answer is sent, changed or not.
    pub fn again(&self, id: &str) {
        self.lock().insert(id.to_string());
    }

    /// A copy of the asks.
    pub fn snapshot(&self) -> HashSet<String> {
        self.lock().clone()
    }

    /// The asks, emptying the set.
    pub fn take(&self) -> HashSet<String> {
        std::mem::take(&mut *self.lock())
    }

    /// Give asks back (a query that did not answer does not spend them).
    pub fn put_back(&self, ids: HashSet<String>) {
        self.lock().extend(ids);
    }
}

pub struct AgentPoll {
    send: AgentSend,
    deps: PollDeps,
    agent_state: HashMap<String, bool>,
    seen_ticks: Option<u64>,
    every: u64,
    next: u64,
    last_start: Option<u64>,
    fails: u32,
    fail_next: u64,
    handle: PollHandle,
}

impl AgentPoll {
    pub fn new(send: AgentSend, deps: PollDeps) -> Self {
        Self::with_handle(send, deps, PollHandle::new())
    }

    pub fn with_handle(send: AgentSend, deps: PollDeps, handle: PollHandle) -> Self {
        Self {
            send,
            deps,
            agent_state: HashMap::new(),
            seen_ticks: None,
            every: AGENT_POLL_MIN,
            next: 0,
            last_start: None,
            fails: 0,
            fail_next: 0,
            handle,
        }
    }

    pub fn handle(&self) -> PollHandle {
        self.handle.clone()
    }

    /// One look, blocking on `deps.query`.
    pub fn look(&mut self, as_forced: bool) {
        let pids = (self.deps.pids)();
        if pids.is_empty() {
            return;
        }
        let start_at = (self.deps.now)();
        // A failing query backs off for everyone, forced looks too.
        if self.fails > 0 && start_at < self.fail_next {
            return;
        }
        // A forced look inside the gap is an ordinary one: its waits apply.
        let force = as_forced
            && self
                .last_start
                .is_none_or(|l| start_at.saturating_sub(l) >= AGENT_FORCE_GAP);
        let ticks = (self.deps.ticks)();
        let quiet = Some(ticks) == self.seen_ticks;
        let asked = self.handle.snapshot();
        let known = pids
            .iter()
            .all(|(id, _)| self.agent_state.contains_key(id) && !asked.contains(id));
        // A shell that said nothing since the last look, whose answer we have, cannot have changed.
        if quiet && known && !force {
            return;
        }
        // The back-off is for an answer that keeps coming back the same; a session nobody has
        // asked about yet is not made to wait for its first one.
        let now = (self.deps.now)();
        if known && now < self.next && !force {
            return;
        }
        self.last_start = Some(start_at);
        let asking = self.handle.take();
        let shell_pids: Vec<u32> = pids.iter().map(|(_, p)| *p).collect();
        let rows = (self.deps.query)(&shell_pids).unwrap_or_default();
        // Only a query that answered counts as a look: a failed one does not consume the tick
        // and does not spend an ask.
        if rows.is_empty() {
            self.handle.put_back(asking);
            self.fails += 1;
            let backoff = AGENT_POLL_MIN.saturating_mul(1u64 << self.fails.min(16));
            self.fail_next = (self.deps.now)().saturating_add(backoff.min(AGENT_FAIL_MAX));
            if self.fails == 3
                && let Some(f) = &self.deps.failing
            {
                f();
            }
            return;
        }
        self.fails = 0;
        self.seen_ticks = Some(ticks);
        let mut changed = false;
        let live = (self.deps.pids)();
        for (id, pid) in &live {
            let kind = tree_agent_kind(&rows, *pid);
            let has = kind.is_some();
            if self.agent_state.get(id) != Some(&has) {
                self.agent_state.insert(id.clone(), has);
                changed = true;
                (self.send)(id, has, kind);
            } else if asking.contains(id) {
                (self.send)(id, has, kind);
            }
        }
        // Forget sessions that ended.
        let alive: HashSet<&String> = live.iter().map(|(id, _)| id).collect();
        self.agent_state.retain(|id, _| alive.contains(id));
        self.every = if changed {
            AGENT_POLL_MIN
        } else {
            AGENT_POLL_MAX.min(self.every.saturating_mul(2))
        };
        self.next = (self.deps.now)().saturating_add(self.every);
    }
}

enum PollCmd {
    Look,
    Soon(u64),
    Stop,
}

/// The thread around the poll (PT's setInterval, pollAgentsSoon, pollAgentsNow, pollAgentAgain).
pub struct PollWorker {
    tx: Sender<PollCmd>,
    handle: PollHandle,
    join: Option<JoinHandle<()>>,
}

impl PollWorker {
    /// `make` runs ON the worker thread and returns the send closure and the deps (neither needs to
    /// be `Send`); `tick` is the interval (`AGENT_POLL_MIN` in the app, a few ms in tests).
    pub fn start(
        tick: Duration,
        make: impl FnOnce(PollHandle) -> (AgentSend, PollDeps) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<PollCmd>();
        let handle = PollHandle::new();
        let thread_handle = handle.clone();
        let join = std::thread::Builder::new()
            .name("flare-agent-poll".into())
            .spawn(move || {
                let (send, deps) = make(thread_handle.clone());
                let mut poll = AgentPoll::with_handle(send, deps, thread_handle);
                let mut next_tick = Instant::now() + tick;
                let mut soon: Option<Instant> = None;
                let mut warned = false;
                let mut look = |poll: &mut AgentPoll, forced: bool| {
                    let r = std::panic::catch_unwind(AssertUnwindSafe(|| poll.look(forced)));
                    if r.is_err() && !warned {
                        warned = true;
                        log::warn!("the agent poll panicked in a look; it goes on");
                    }
                };
                loop {
                    let due = soon.map_or(next_tick, |s| s.min(next_tick));
                    let wait = due.saturating_duration_since(Instant::now());
                    // sync-ok: the poll worker thread waits here, never the UI thread
                    match rx.recv_timeout(wait) {
                        Ok(PollCmd::Look) => look(&mut poll, true),
                        Ok(PollCmd::Soon(ms)) => {
                            soon = Some(Instant::now() + Duration::from_millis(ms));
                        }
                        Ok(PollCmd::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {
                            let now = Instant::now();
                            let mut did = false;
                            if soon.is_some_and(|s| now >= s) {
                                soon = None;
                                did = true;
                            }
                            if now >= next_tick {
                                next_tick = now + tick;
                                did = true;
                            }
                            if did {
                                look(&mut poll, false);
                            }
                        }
                    }
                }
            })
            .expect("spawn the agent poll thread");
        Self {
            tx,
            handle,
            join: Some(join),
        }
    }

    /// A forced look at once.
    pub fn look_now(&self) {
        let _ = self.tx.send(PollCmd::Look);
    }

    /// A normal look after `ms`.
    pub fn soon(&self, ms: u64) {
        let _ = self.tx.send(PollCmd::Soon(ms));
    }

    /// That session's next answer is sent, changed or not.
    pub fn again(&self, id: &str) {
        self.handle.again(id);
    }

    /// Joins; idempotent.
    pub fn stop(&mut self) {
        let _ = self.tx.send(PollCmd::Stop);
        if let Some(j) = self.join.take() {
            let _ = j.join(); // sync-ok: joining the poll's own thread on stop
        }
    }
}

impl Drop for PollWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    const CLAUDE: &str =
        r"C:\Users\me\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\cli.js";

    fn rows(s: &str) -> Vec<ProcRow> {
        s.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let mut it = l.trim().splitn(3, ' ');
                let pid = it.next().unwrap().parse().unwrap();
                let ppid = it.next().unwrap().parse().unwrap();
                ProcRow {
                    pid,
                    ppid,
                    cmd: it.next().unwrap_or("").to_string(),
                }
            })
            .collect()
    }

    type Sent = Vec<(String, bool, Option<DetectedAgent>)>;

    struct World {
        pids: Vec<(String, u32)>,
        ticks: u64,
        now: u64,
        out: Option<Vec<ProcRow>>,
        asked: usize,
        failing: usize,
        sent: Sent,
        on_query: Option<Box<dyn Fn()>>,
    }

    fn world(out: Option<Vec<ProcRow>>) -> Rc<RefCell<World>> {
        Rc::new(RefCell::new(World {
            pids: vec![("t1".into(), 100)],
            ticks: 0,
            now: 0,
            out,
            asked: 0,
            failing: 0,
            sent: vec![],
            on_query: None,
        }))
    }

    fn poll_in(w: &Rc<RefCell<World>>, handle: PollHandle) -> AgentPoll {
        let (a, b, c, d, e, f) = (
            w.clone(),
            w.clone(),
            w.clone(),
            w.clone(),
            w.clone(),
            w.clone(),
        );
        let deps = PollDeps {
            pids: Box::new(move || a.borrow().pids.clone()),
            ticks: Box::new(move || b.borrow().ticks),
            query: Box::new(move |_| {
                c.borrow_mut().asked += 1;
                let hook = c.borrow_mut().on_query.take();
                if let Some(h) = hook {
                    h();
                }
                c.borrow().out.clone()
            }),
            now: Box::new(move || d.borrow().now),
            failing: Some(Box::new(move || e.borrow_mut().failing += 1)),
        };
        let send: AgentSend = Box::new(move |id, has, kind| {
            f.borrow_mut().sent.push((id.to_string(), has, kind));
        });
        AgentPoll::with_handle(send, deps, handle)
    }

    fn sent(w: &Rc<RefCell<World>>) -> Sent {
        w.borrow().sent.clone()
    }

    fn t1(has: bool, k: Option<DetectedAgent>) -> (String, bool, Option<DetectedAgent>) {
        ("t1".into(), has, k)
    }

    #[test]
    fn a_forced_look_sees_an_agent_inside_the_backoff_and_says_only_what_changed() {
        let w = world(Some(rows("100 1")));
        let mut p = poll_in(&w, PollHandle::new());
        p.look(false);
        assert_eq!(sent(&w), vec![t1(false, None)]);
        w.borrow_mut().out = Some(rows("100 1\n200 100 claude"));
        w.borrow_mut().now = 1;
        p.look(false);
        assert_eq!(sent(&w), vec![t1(false, None)]);
        w.borrow_mut().now = AGENT_FORCE_GAP;
        p.look(true);
        assert_eq!(
            sent(&w),
            vec![t1(false, None), t1(true, Some(DetectedAgent::Claude))]
        );
        w.borrow_mut().out = Some(rows("100 1"));
        w.borrow_mut().now = 2 * AGENT_FORCE_GAP;
        p.look(true);
        assert_eq!(sent(&w).last().unwrap(), &t1(false, None));
        let n = sent(&w).len();
        w.borrow_mut().now = 3 * AGENT_FORCE_GAP;
        p.look(true);
        assert_eq!(sent(&w).len(), n);
    }

    #[test]
    fn an_unchanged_no_agent_is_sent_when_asked_again() {
        let w = world(Some(rows("100 1")));
        let h = PollHandle::new();
        let mut p = poll_in(&w, h.clone());
        p.look(false);
        w.borrow_mut().ticks = 1;
        w.borrow_mut().now = 1;
        p.look(false);
        assert_eq!(sent(&w), vec![t1(false, None)]);
        h.again("t1");
        w.borrow_mut().now = 2;
        p.look(false);
        assert_eq!(sent(&w), vec![t1(false, None), t1(false, None)]);
    }

    #[test]
    fn no_look_while_no_shell_is_alive() {
        let w = world(Some(rows("100 1")));
        w.borrow_mut().pids = vec![];
        let mut p = poll_in(&w, PollHandle::new());
        p.look(false);
        p.look(true);
        assert_eq!(w.borrow().asked, 0);
    }

    #[test]
    fn reports_an_agent_with_its_kind_once() {
        let w = world(Some(rows(&format!("100 1\n200 100 node {CLAUDE}"))));
        let mut p = poll_in(&w, PollHandle::new());
        p.look(false);
        assert_eq!(sent(&w), vec![t1(true, Some(DetectedAgent::Claude))]);
        w.borrow_mut().ticks = 1;
        w.borrow_mut().now = 60_000;
        p.look(false);
        assert_eq!(sent(&w).len(), 1);
    }

    #[test]
    fn skips_a_shell_that_printed_nothing() {
        let w = world(Some(rows("100 1")));
        let mut p = poll_in(&w, PollHandle::new());
        p.look(false);
        w.borrow_mut().now = 60_000;
        p.look(false);
        p.look(false);
        assert_eq!(w.borrow().asked, 1);
    }

    #[test]
    fn backs_off_while_the_answer_holds_and_comes_back_when_it_changes() {
        let w = world(Some(rows("100 1")));
        let mut p = poll_in(&w, PollHandle::new());
        p.look(false);
        assert_eq!(w.borrow().asked, 1);
        let m = AGENT_POLL_MIN;
        w.borrow_mut().ticks = 1;
        w.borrow_mut().now = m - 1;
        p.look(false);
        assert_eq!(w.borrow().asked, 1);
        w.borrow_mut().now = m;
        p.look(false);
        assert_eq!(w.borrow().asked, 2);
        w.borrow_mut().ticks = 2;
        w.borrow_mut().now = m + 2 * m - 1;
        p.look(false);
        assert_eq!(w.borrow().asked, 2);
        w.borrow_mut().now = m + 2 * m;
        p.look(false);
        assert_eq!(w.borrow().asked, 3);
    }

    #[test]
    fn never_waits_longer_than_the_ceiling() {
        let w = world(Some(rows("100 1")));
        let mut p = poll_in(&w, PollHandle::new());
        for i in 0..12u64 {
            w.borrow_mut().ticks = i;
            w.borrow_mut().now = i * AGENT_POLL_MAX;
            p.look(false);
        }
        assert_eq!(w.borrow().asked, 12);
    }

    #[test]
    fn asks_at_once_about_a_new_shell() {
        let w = world(Some(rows("100 1\n101 1")));
        let mut p = poll_in(&w, PollHandle::new());
        p.look(false);
        assert_eq!(w.borrow().asked, 1);
        w.borrow_mut().pids.push(("t2".into(), 101));
        w.borrow_mut().now = 1;
        p.look(false);
        assert_eq!(w.borrow().asked, 2);
        assert!(sent(&w).contains(&("t2".into(), false, None)));
    }

    #[test]
    fn a_failed_query_is_not_a_look() {
        let w = world(None);
        let mut p = poll_in(&w, PollHandle::new());
        p.look(false);
        assert!(sent(&w).is_empty());
        w.borrow_mut().out = Some(rows(&format!("100 1\n200 100 node {CLAUDE}")));
        p.look(false);
        assert_eq!(w.borrow().asked, 1);
        w.borrow_mut().now = 2 * AGENT_POLL_MIN;
        p.look(false);
        assert_eq!(w.borrow().asked, 2);
        assert_eq!(sent(&w), vec![t1(true, Some(DetectedAgent::Claude))]);
    }

    #[test]
    fn a_failing_query_backs_off_to_a_minute_and_says_so_once() {
        let w = world(None);
        let mut p = poll_in(&w, PollHandle::new());
        let mut t = 0u64;
        let mut at: Vec<u64> = vec![];
        let mut last = 0usize;
        for i in 0..400u64 {
            t += 500;
            {
                let mut b = w.borrow_mut();
                b.now = t;
                b.ticks = i;
            }
            p.look(i % 2 == 0);
            let n = w.borrow().asked;
            if n != last {
                at.push(t);
                last = n;
            }
        }
        let gaps: Vec<u64> = at.windows(2).map(|p| p[1] - p[0]).collect();
        assert_eq!(&gaps[..4], &[5000, 10_000, 20_000, 40_000]);
        assert_eq!(*gaps.iter().max().unwrap(), AGENT_FAIL_MAX);
        assert_eq!(w.borrow().failing, 1);
        w.borrow_mut().out = Some(rows("100 1"));
        w.borrow_mut().now = t + AGENT_FAIL_MAX;
        p.look(false);
        let after_good = w.borrow().asked;
        assert_eq!(after_good, last + 1);
        {
            let mut b = w.borrow_mut();
            b.ticks = 999;
            b.now = t + AGENT_FAIL_MAX + AGENT_POLL_MIN;
        }
        p.look(false);
        assert_eq!(w.borrow().asked, after_good + 1);
    }

    #[test]
    fn a_forced_look_waits_for_the_gap() {
        let w = world(Some(rows("100 1")));
        let mut p = poll_in(&w, PollHandle::new());
        p.look(false);
        for i in 1..10u64 {
            w.borrow_mut().now = i * 400;
            p.look(true);
        }
        assert_eq!(w.borrow().asked, 1);
        w.borrow_mut().now = AGENT_FORCE_GAP;
        p.look(true);
        assert_eq!(w.borrow().asked, 2);
    }

    #[test]
    fn forgets_a_session_that_ended_so_its_id_starts_clean() {
        let w = world(Some(rows(&format!("100 1\n200 100 node {CLAUDE}\n101 1"))));
        w.borrow_mut().pids = vec![("t1".into(), 100), ("t2".into(), 101)];
        let mut p = poll_in(&w, PollHandle::new());
        p.look(false);
        w.borrow_mut().pids = vec![("t1".into(), 100)];
        w.borrow_mut().out = Some(rows("100 1"));
        w.borrow_mut().ticks = 1;
        w.borrow_mut().now = AGENT_POLL_MIN;
        p.look(false);
        assert_eq!(
            sent(&w),
            vec![
                t1(true, Some(DetectedAgent::Claude)),
                ("t2".into(), false, None),
                t1(false, None)
            ]
        );
        // t2 comes back as a fresh session: reported again, not remembered.
        w.borrow_mut().pids = vec![("t1".into(), 100), ("t2".into(), 101)];
        w.borrow_mut().ticks = 2;
        w.borrow_mut().now = 10 * AGENT_POLL_MIN;
        p.look(false);
        assert_eq!(sent(&w).last().unwrap(), &("t2".to_string(), false, None));
    }

    #[test]
    fn sends_the_next_answer_of_a_session_asked_about_again() {
        let w = world(Some(rows("100 1\n200 100 codex resume --last")));
        let h = PollHandle::new();
        let mut p = poll_in(&w, h.clone());
        p.look(false);
        h.again("t1");
        p.look(false);
        assert_eq!(
            sent(&w),
            vec![
                t1(true, Some(DetectedAgent::Codex)),
                t1(true, Some(DetectedAgent::Codex))
            ]
        );
        p.look(false);
        assert_eq!(w.borrow().asked, 2);
    }

    #[test]
    fn keeps_an_ask_that_came_during_a_look() {
        let w = world(Some(rows("100 1")));
        let h = PollHandle::new();
        let mut p = poll_in(&w, h.clone());
        let h2 = h.clone();
        w.borrow_mut().on_query = Some(Box::new(move || h2.again("t1")));
        p.look(false);
        assert_eq!(sent(&w), vec![t1(false, None)]);
        p.look(false);
        assert_eq!(sent(&w), vec![t1(false, None), t1(false, None)]);
    }

    /// Counting deps for the worker tests (they cross threads, so atomics).
    fn counting(
        queries: Arc<AtomicUsize>,
        sent: Arc<AtomicUsize>,
        panic_first: bool,
    ) -> impl FnOnce(PollHandle) -> (AgentSend, PollDeps) + Send + 'static {
        move |_h| {
            let clock = Arc::new(AtomicU64::new(0));
            let first = Arc::new(AtomicUsize::new(0));
            let tick = Arc::new(AtomicU64::new(0));
            let (c1, c2) = (clock.clone(), tick.clone());
            let deps = PollDeps {
                pids: Box::new(move || {
                    if panic_first && first.fetch_add(1, Ordering::SeqCst) == 0 {
                        panic!("first pids() panics");
                    }
                    vec![("t1".into(), 100)]
                }),
                ticks: Box::new(move || c2.fetch_add(1, Ordering::SeqCst)),
                query: Box::new(move |_| {
                    queries.fetch_add(1, Ordering::SeqCst);
                    Some(vec![ProcRow {
                        pid: 100,
                        ppid: 1,
                        cmd: "pwsh".into(),
                    }])
                }),
                now: Box::new(move || c1.fetch_add(10_000, Ordering::SeqCst)),
                failing: None,
            };
            let send: AgentSend = Box::new(move |_, _, _| {
                sent.fetch_add(1, Ordering::SeqCst);
            });
            (send, deps)
        }
    }

    fn wait_for(f: impl Fn() -> bool, ms: u64) -> bool {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        f()
    }

    #[test]
    fn the_worker_looks_on_its_tick_and_stops() {
        let q = Arc::new(AtomicUsize::new(0));
        let s = Arc::new(AtomicUsize::new(0));
        let mut w = PollWorker::start(Duration::from_millis(10), counting(q.clone(), s, false));
        assert!(wait_for(|| q.load(Ordering::SeqCst) >= 3, 2000));
        let t = Instant::now();
        w.stop();
        assert!(t.elapsed() < Duration::from_secs(1));
        let n = q.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(q.load(Ordering::SeqCst), n);
        w.stop();
    }

    #[test]
    fn the_worker_answers_look_now_and_again() {
        let q = Arc::new(AtomicUsize::new(0));
        let s = Arc::new(AtomicUsize::new(0));
        let mut w = PollWorker::start(
            Duration::from_secs(10),
            counting(q.clone(), s.clone(), false),
        );
        w.look_now();
        assert!(wait_for(|| q.load(Ordering::SeqCst) == 1, 1000));
        assert!(wait_for(|| s.load(Ordering::SeqCst) == 1, 1000));
        w.again("t1");
        w.look_now();
        assert!(wait_for(|| s.load(Ordering::SeqCst) == 2, 1000));
        w.stop();
    }

    #[test]
    fn a_panic_in_a_look_does_not_end_the_worker() {
        let q = Arc::new(AtomicUsize::new(0));
        let s = Arc::new(AtomicUsize::new(0));
        let mut w = PollWorker::start(Duration::from_millis(10), counting(q.clone(), s, true));
        assert!(wait_for(|| q.load(Ordering::SeqCst) >= 1, 2000));
        w.stop();
    }
}
