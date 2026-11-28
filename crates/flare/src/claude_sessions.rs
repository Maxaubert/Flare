//! The tab menu's "Claude sessions" rows (rules 1380 to 1383; Max, 2026-11-28: "some way for me to close all sessions
//! without closing the tabs ... in one place"): which tabs each row touches, the rows and their counts, the question
//! for busy agents, and the runner that ends, starts and restarts Claude tab by tab.
//!
//! Pure: the window hands in each tab's facts and the time, and carries out the acts the runner returns (keys typed
//! through `TermHandle::session_key`). Nothing here closes a tab or touches a shell's process.

use std::collections::BTreeMap;

use crate::menus_ui::MenuRowData;

/// What the window knows of one tab, read fresh at every step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabFacts {
    pub id: String,
    /// A local shell tab of this window (not hidden, not remote) whose shell is live. Every other field is ignored
    /// when false.
    pub local: bool,
    /// The process poll has answered for the tab at least once: only then does "no agent" mean none.
    pub polled: bool,
    /// Claude Code is the agent here.
    pub claude: bool,
    /// Another agent (Codex, agy, one not known), or an agent armed by its title before the poll said which.
    pub other_agent: bool,
    /// Claude works, or a question is up (an Enter there would answer it).
    pub busy: bool,
    /// The text in Claude's prompt box (`flare_agent::question::prompt_text`): None when no box is on the screen (a
    /// picker, a menu, the transcript view, an editor: "/exit" is typed only into an empty box). Read for Claude only.
    pub prompt_box: Option<String>,
    /// The shell reported its prompt after the last key typed in the tab.
    pub at_prompt: bool,
    /// The tab's shell (`pwsh`, `cmd`, `zsh`...), when known.
    pub shell: Option<String>,
    /// The tab's last Claude session id, when known.
    pub session: Option<String>,
    /// How many prompt reports the shell has made.
    pub prompts: u64,
}

impl TabFacts {
    /// Claude's prompt box holds unsent text (the tests read it; the runner matches the box itself).
    #[cfg(test)]
    pub fn draft(&self) -> bool {
        self.prompt_box.as_deref().is_some_and(|t| !t.is_empty())
    }

    /// The start line is made for this tab's shell.
    fn can_start(&self) -> bool {
        self.shell
            .as_deref()
            .is_some_and(|s| flare_shells::resume::typed_start_line_prior(s, None, None).is_some())
    }

    /// "End Claude session" acts here.
    pub fn can_end(&self) -> bool {
        self.local && self.claude
    }

    /// "Start Claude" acts here: a polled local shell at its prompt, no agent of any kind, a shell the line is made for.
    pub fn can_start_here(&self) -> bool {
        self.local
            && self.polled
            && !self.claude
            && !self.other_agent
            && self.at_prompt
            && self.can_start()
    }

    /// "Restart" acts here: Claude can be ended, and started again in this shell.
    pub fn can_restart(&self) -> bool {
        self.can_end() && self.can_start()
    }
}

/// The tabs "End all Claude sessions" touches, in the order given (strip order).
pub fn end_targets(tabs: &[TabFacts]) -> Vec<String> {
    ids(tabs, TabFacts::can_end)
}

/// The tabs "Start Claude in all tabs" touches.
pub fn start_targets(tabs: &[TabFacts]) -> Vec<String> {
    ids(tabs, TabFacts::can_start_here)
}

/// The tabs "Restart all Claude sessions" touches.
pub fn restart_targets(tabs: &[TabFacts]) -> Vec<String> {
    ids(tabs, TabFacts::can_restart)
}

fn ids(tabs: &[TabFacts], keep: fn(&TabFacts) -> bool) -> Vec<String> {
    tabs.iter()
        .filter(|t| keep(t))
        .map(|t| t.id.clone())
        .collect()
}

/// The busy ones among `ids`.
pub fn busy_among(ids: &[String], tabs: &[TabFacts]) -> Vec<String> {
    ids.iter()
        .filter(|id| tabs.iter().any(|t| &t.id == *id && t.busy))
        .cloned()
        .collect()
}

/// What a row of the tab menu does; its path carries it ([`ROW`] then the word).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    EndTab,
    StartTab,
    EndAll,
    StartAll,
    RestartAll,
}

/// The mark a "Claude sessions" row's path starts with: a NUL first, so no folder or plugin action reads as one.
pub const ROW: &str = "\u{0}claude:";

impl Pick {
    fn word(self) -> &'static str {
        match self {
            Pick::EndTab => "end-tab",
            Pick::StartTab => "start-tab",
            Pick::EndAll => "end-all",
            Pick::StartAll => "start-all",
            Pick::RestartAll => "restart-all",
        }
    }

    pub fn path(self) -> String {
        format!("{ROW}{}", self.word())
    }

    /// The pick a row's path names, for a "Claude sessions" row.
    pub fn of_path(path: &str) -> Option<Pick> {
        let w = path.strip_prefix(ROW)?;
        [
            Pick::EndTab,
            Pick::StartTab,
            Pick::EndAll,
            Pick::StartAll,
            Pick::RestartAll,
        ]
        .into_iter()
        .find(|p| p.word() == w)
    }
}

pub const END_TAB: &str = "End Claude session";
pub const START_TAB: &str = "Start Claude";
pub const GROUP: &str = "Claude sessions";

/// The tab menu's rows (rule 1380), to go after its own: this tab's row ("End Claude session" while Claude runs here,
/// "Start Claude" while the shell waits at its prompt with no agent), then the "Claude sessions" flyout with the rows
/// that have something to act on, each with its count. A row with nothing to act on is left out, and the flyout too
/// when all three are. `this` is None for a tab that is no shell tab (Settings). `first_index` is the index the first
/// returned row will have in the whole menu (the flyout's children name their parent by index).
pub fn menu_rows(
    this: Option<&TabFacts>,
    all: &[TabFacts],
    first_index: usize,
) -> Vec<(MenuRowData, String)> {
    let mut rows = Vec::new();
    if let Some(t) = this {
        let pick = if t.can_end() {
            Some((END_TAB, Pick::EndTab))
        } else if t.can_start_here() {
            Some((START_TAB, Pick::StartTab))
        } else {
            None
        };
        if let Some((label, pick)) = pick {
            let mut r = MenuRowData::labelled(label);
            r.group = true;
            rows.push((r, pick.path()));
        }
    }
    let counted = [
        (
            "End all Claude sessions",
            end_targets(all).len(),
            Pick::EndAll,
        ),
        (
            "Start Claude in all tabs",
            start_targets(all).len(),
            Pick::StartAll,
        ),
        (
            "Restart all Claude sessions",
            restart_targets(all).len(),
            Pick::RestartAll,
        ),
    ];
    let children: Vec<(MenuRowData, String)> = counted
        .into_iter()
        .filter(|(_, n, _)| *n > 0)
        .map(|(label, n, pick)| {
            (
                MenuRowData::labelled(&format!("{label} ({n})")),
                pick.path(),
            )
        })
        .collect();
    if !children.is_empty() {
        let parent = i32::try_from(first_index + rows.len()).unwrap_or(i32::MAX);
        let mut head = MenuRowData::labelled(GROUP);
        head.flyout = true;
        // Its own group unless this tab's row just opened one.
        head.group = rows.is_empty();
        rows.push((head, String::new()));
        for (mut r, path) in children {
            r.parent = parent;
            rows.push((r, path));
        }
    }
    rows
}

/// What the busy question is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// End Claude; the tab stays at its shell prompt.
    End,
    /// End Claude, then start it again (resumed) at the next prompt.
    Restart,
}

/// The busy question's words (rule 1382): title, body, the primary (interrupt first) and the alternative (wait).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskWords {
    pub title: String,
    pub body: &'static str,
    pub primary: &'static str,
    pub alt: &'static str,
}

pub fn ask_words(mode: Mode, busy: usize) -> AskWords {
    let title = if busy == 1 {
        "1 session is working".to_string()
    } else {
        format!("{busy} sessions are working")
    };
    match mode {
        Mode::End => AskWords {
            title,
            body: "End now stops their work first. Wait ends each one as soon as it is idle.",
            primary: "End now",
            alt: "Wait",
        },
        Mode::Restart => AskWords {
            title,
            body: "Restart now stops their work first. Wait restarts each one as soon as it is idle.",
            primary: "Restart now",
            alt: "Wait",
        },
    }
}

/// A key the runner presses, as `flare_term::handle::SessionKey` types it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Exit,
    Enter,
    Interrupt,
}

/// What the window does for the runner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Press {
        tab: String,
        key: Press,
    },
    /// Types the start line (`claude` or `claude --resume <id>`) and its Enter.
    Start {
        tab: String,
        shell: String,
        resume: Option<String>,
    },
    /// Asks the process poll to look soon.
    LookSoon,
}

/// "/exit" and its Enter this far apart, as every other typed send (`SEND_ENTER_MS`).
pub const ENTER_AFTER_MS: u64 = flare_dictation::route::SEND_ENTER_MS;
/// After an interrupt, the tab is looked at again only after this long (the title and hooks say the stop).
pub const INTERRUPT_SETTLE_MS: u64 = 800;
/// An interrupted turn has this long to come to rest; then the tab is given up (left running).
pub const INTERRUPT_WAIT_MS: u64 = 60_000;
/// After the Enter, the shell has this long to show its prompt again.
pub const LEAVE_WAIT_MS: u64 = 15_000;
/// "/exit" has this long to show in Claude's prompt box; the Enter is pressed only once it does.
pub const ECHO_WAIT_MS: u64 = 2_000;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// Waits until Claude is idle; `until` None waits as long as it takes (Wait).
    Idle { not_before: u64, until: Option<u64> },
    /// "/exit" is typed; the Enter is due at `at`, once the box shows exactly "/exit", and given up at `until`.
    Enter { at: u64, until: u64 },
    /// The Enter was pressed; waits for the shell's next prompt report.
    Leaving { after: u64, until: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Job {
    mode: Mode,
    phase: Phase,
    shell: Option<String>,
    session: Option<String>,
}

/// What became of a tab the runner let go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Ended (and started again, for a restart).
    Done,
    /// Left alone: Claude's prompt box held unsent text.
    KeptDraft,
    /// Left alone: Claude's prompt box was not on the screen (a picker, a menu, an editor).
    NoPrompt,
    /// Given up: it did not come to rest or the shell did not come back in time.
    GaveUp,
    /// The tab closed or stopped being a local shell.
    Gone,
}

/// The ends and restarts under way, one job a tab.
#[derive(Debug, Default)]
pub struct Runner {
    jobs: BTreeMap<String, Job>,
    /// Every tab let go since the last take, and why.
    done: Vec<(String, Outcome)>,
}

impl Runner {
    /// Starts ending (or restarting) `tabs`. `interrupt`: a busy tab gets Escape first and has
    /// [`INTERRUPT_WAIT_MS`] to come to rest; without it a busy tab waits as long as it takes. A tab still waiting for
    /// idle starts over; one whose "/exit" is typed already keeps its job (a second "/exit" would join the first). The
    /// tabs that are idle are acted on at once (the returned acts); other tabs' jobs are not touched.
    pub fn end(&mut self, tabs: &[TabFacts], mode: Mode, interrupt: bool, now: u64) -> Vec<Act> {
        let mut acts = Vec::new();
        let mut fresh = Vec::new();
        for t in tabs.iter().filter(|t| t.can_end()) {
            if self
                .jobs
                .get(&t.id)
                .is_some_and(|j| !matches!(j.phase, Phase::Idle { .. }))
            {
                continue;
            }
            fresh.push(t.id.clone());
            let phase = if interrupt && t.busy {
                acts.push(Act::Press {
                    tab: t.id.clone(),
                    key: Press::Interrupt,
                });
                Phase::Idle {
                    not_before: now + INTERRUPT_SETTLE_MS,
                    until: Some(now + INTERRUPT_WAIT_MS),
                }
            } else {
                Phase::Idle {
                    not_before: now,
                    until: None,
                }
            };
            self.jobs.insert(
                t.id.clone(),
                Job {
                    mode,
                    phase,
                    shell: t.shell.clone(),
                    session: t.session.clone(),
                },
            );
        }
        let facts = |id: &str| tabs.iter().find(|t| t.id == id).cloned();
        acts.extend(self.step_ids(now, fresh, &facts));
        acts
    }

    /// Starts Claude in `tabs` that can take it now (no job: nothing waits after the line).
    pub fn start(tabs: &[TabFacts]) -> Vec<Act> {
        let mut acts: Vec<Act> = tabs
            .iter()
            .filter(|t| t.can_start_here())
            .filter_map(|t| {
                Some(Act::Start {
                    tab: t.id.clone(),
                    shell: t.shell.clone()?,
                    resume: t.session.clone(),
                })
            })
            .collect();
        if !acts.is_empty() {
            acts.push(Act::LookSoon);
        }
        acts
    }

    /// Moves every job on, with each tab's facts as they are now.
    pub fn step(&mut self, now: u64, facts: &dyn Fn(&str) -> Option<TabFacts>) -> Vec<Act> {
        let ids: Vec<String> = self.jobs.keys().cloned().collect();
        self.step_ids(now, ids, facts)
    }

    /// Moves the jobs of `ids` on.
    fn step_ids(
        &mut self,
        now: u64,
        ids: Vec<String>,
        facts: &dyn Fn(&str) -> Option<TabFacts>,
    ) -> Vec<Act> {
        let mut acts = Vec::new();
        let mut look = false;
        for id in ids {
            let Some(job) = self.jobs.get(&id).cloned() else {
                continue;
            };
            let f = facts(&id).filter(|f| f.local);
            let Some(f) = f else {
                self.finish(&id, Outcome::Gone);
                continue;
            };
            match job.phase {
                Phase::Idle { not_before, until } => {
                    if now < not_before {
                        continue;
                    }
                    // The shell reported its prompt after the last key: it is in front, not Claude (a stale
                    // agent mark), so nothing is typed. Claude is gone (the person ended it): a restart starts it.
                    if !f.claude || f.at_prompt {
                        self.leave_or_start(&id, &job, &f, now, &mut acts, true);
                        continue;
                    }
                    if f.busy {
                        if until.is_some_and(|u| now >= u) {
                            self.finish(&id, Outcome::GaveUp);
                        }
                        continue;
                    }
                    match f.prompt_box.as_deref() {
                        // No box: Claude shows something else, where "/exit" would be keys of its own.
                        None => {
                            self.finish(&id, Outcome::NoPrompt);
                            continue;
                        }
                        // "/exit" would join the draft and the Enter would send it.
                        Some(t) if !t.is_empty() => {
                            self.finish(&id, Outcome::KeptDraft);
                            continue;
                        }
                        Some(_) => {}
                    }
                    acts.push(Act::Press {
                        tab: id.clone(),
                        key: Press::Exit,
                    });
                    if let Some(j) = self.jobs.get_mut(&id) {
                        // The session Claude is in now (a /clear or /resume under Wait named another one).
                        if f.session.is_some() {
                            j.session = f.session.clone();
                        }
                        j.phase = Phase::Enter {
                            at: now + ENTER_AFTER_MS,
                            until: now + ENTER_AFTER_MS + ECHO_WAIT_MS,
                        };
                    }
                }
                Phase::Enter { at, until } => {
                    if now < at {
                        continue;
                    }
                    // Checked again: a question that came up since would take the Enter as its answer.
                    if !f.claude || f.busy || f.at_prompt {
                        self.finish(&id, Outcome::GaveUp);
                        continue;
                    }
                    // Only once the box shows "/exit" and nothing else: the Enter must not send words the person
                    // typed meanwhile, nor reach anything but Claude's prompt.
                    if f.prompt_box.as_deref() != Some("/exit") {
                        if now >= until {
                            self.finish(&id, Outcome::GaveUp);
                        }
                        continue;
                    }
                    acts.push(Act::Press {
                        tab: id.clone(),
                        key: Press::Enter,
                    });
                    look = true;
                    self.set_phase(
                        &id,
                        Phase::Leaving {
                            after: f.prompts,
                            until: now + LEAVE_WAIT_MS,
                        },
                    );
                }
                Phase::Leaving { after, until } => {
                    if f.prompts > after && f.at_prompt {
                        self.leave_or_start(&id, &job, &f, now, &mut acts, false);
                        look = true;
                    } else if now >= until {
                        self.finish(&id, Outcome::GaveUp);
                    }
                }
            }
        }
        if look {
            acts.push(Act::LookSoon);
        }
        acts
    }

    /// Claude has left `id`: an end is done; a restart starts Claude at the prompt now, or (`wait_prompt`, the prompt
    /// is not up yet) at the next prompt report.
    fn leave_or_start(
        &mut self,
        id: &str,
        job: &Job,
        f: &TabFacts,
        now: u64,
        acts: &mut Vec<Act>,
        wait_prompt: bool,
    ) {
        if job.mode == Mode::End {
            self.finish(id, Outcome::Done);
            return;
        }
        if !f.at_prompt {
            if wait_prompt {
                self.set_phase(
                    id,
                    Phase::Leaving {
                        after: f.prompts,
                        until: now + LEAVE_WAIT_MS,
                    },
                );
            } else {
                self.finish(id, Outcome::GaveUp);
            }
            return;
        }
        let shell = job.shell.clone().or_else(|| f.shell.clone());
        if let Some(shell) = shell {
            acts.push(Act::Start {
                tab: id.to_string(),
                shell,
                resume: job.session.clone(),
            });
        }
        self.finish(id, Outcome::Done);
    }

    fn set_phase(&mut self, id: &str, phase: Phase) {
        if let Some(j) = self.jobs.get_mut(id) {
            j.phase = phase;
        }
    }

    fn finish(&mut self, id: &str, why: Outcome) {
        if self.jobs.remove(id).is_some() {
            self.done.push((id.to_string(), why));
        }
    }

    /// Jobs under way.
    pub fn active(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// The tabs under way.
    #[cfg(test)]
    pub fn tabs(&self) -> Vec<String> {
        self.jobs.keys().cloned().collect()
    }

    /// The next time a job is due on its own (an Enter, a settle, a deadline), never one already past; a job that only
    /// waits for the tab's state (a Wait for idle, the echo of "/exit") adds none before its deadline: the window steps
    /// it on every drained batch of terminal events.
    pub fn next_due(&self, now: u64) -> Option<u64> {
        let next = |a: u64, b: u64| if now < a { a } else { b };
        self.jobs
            .values()
            .filter_map(|j| match j.phase {
                Phase::Idle { not_before, until } => until.map(|u| next(not_before, u)),
                Phase::Enter { at, until } => Some(next(at, until)),
                Phase::Leaving { until, .. } => Some(until),
            })
            .min()
    }

    /// The tabs let go since the last take, and why.
    pub fn take_done(&mut self) -> Vec<(String, Outcome)> {
        std::mem::take(&mut self.done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "5a1d0c2e-1111-4222-8333-94445555a666";

    fn claude(id: &str) -> TabFacts {
        TabFacts {
            id: id.into(),
            local: true,
            polled: true,
            claude: true,
            prompt_box: Some(String::new()),
            shell: Some("pwsh".into()),
            session: Some(SID.into()),
            ..TabFacts::default()
        }
    }

    fn shell(id: &str) -> TabFacts {
        TabFacts {
            id: id.into(),
            local: true,
            polled: true,
            at_prompt: true,
            shell: Some("pwsh".into()),
            ..TabFacts::default()
        }
    }

    fn labels(rows: &[(MenuRowData, String)]) -> Vec<String> {
        rows.iter().map(|(r, _)| r.label.clone()).collect()
    }

    /// Rule 1380: End touches the local Claude tabs only; Start the polled local shells at their prompt with no agent;
    /// Restart the Claude tabs whose shell can take the start line.
    #[test]
    fn each_row_touches_only_its_own_tabs() {
        let mut remote = claude("remote");
        remote.local = false;
        let mut codex = shell("codex");
        codex.other_agent = true;
        codex.at_prompt = false;
        let mut armed = shell("armed");
        armed.other_agent = true;
        let mut unpolled = shell("unpolled");
        unpolled.polled = false;
        let mut running = shell("running");
        running.at_prompt = false;
        let mut wsl = shell("wsl");
        wsl.shell = Some("wsl-Arch".into());
        let mut wsl_claude = claude("wslc");
        wsl_claude.shell = Some("wsl-Arch".into());
        let mut busy = claude("busy");
        busy.busy = true;
        let tabs = vec![
            claude("a"),
            busy,
            remote,
            codex,
            armed,
            unpolled,
            running,
            wsl,
            wsl_claude,
            shell("b"),
        ];
        assert_eq!(end_targets(&tabs), ["a", "busy", "wslc"]);
        assert_eq!(start_targets(&tabs), ["b"]);
        assert_eq!(restart_targets(&tabs), ["a", "busy"]);
        assert_eq!(
            busy_among(&end_targets(&tabs), &tabs),
            ["busy"],
            "only the busy one asks"
        );
    }

    /// Rule 1380: the rows show their counts, a row with nothing to act on is left out, and the flyout with them.
    #[test]
    fn the_rows_show_counts_and_nothing_acts_on_zero_tabs() {
        let tabs = vec![claude("a"), claude("b"), shell("c")];
        let rows = menu_rows(Some(&tabs[0]), &tabs, 2);
        assert_eq!(
            labels(&rows),
            [
                END_TAB,
                GROUP,
                "End all Claude sessions (2)",
                "Start Claude in all tabs (1)",
                "Restart all Claude sessions (2)"
            ]
        );
        assert!(rows[0].0.group, "this tab's row opens a group");
        assert!(rows[1].0.flyout && !rows[1].0.group);
        assert!(
            rows[2..].iter().all(|(r, _)| r.parent == 3),
            "under the flyout at index 3"
        );
        assert_eq!(Pick::of_path(&rows[0].1), Some(Pick::EndTab));
        assert_eq!(Pick::of_path(&rows[3].1), Some(Pick::StartAll));
        // A plain shell's own menu: Start Claude.
        let rows = menu_rows(Some(&tabs[2]), &tabs, 2);
        assert_eq!(labels(&rows)[0], START_TAB);
        // Only plain shells: the end and restart rows are left out.
        let shells = vec![shell("c"), shell("d")];
        assert_eq!(
            labels(&menu_rows(None, &shells, 1)),
            [GROUP, "Start Claude in all tabs (2)"]
        );
        assert!(menu_rows(None, &shells, 1)[0].0.group, "its own group");
        // Nothing to act on anywhere: no row at all.
        let mut busy_shell = shell("e");
        busy_shell.at_prompt = false;
        assert!(menu_rows(Some(&busy_shell), &[busy_shell.clone()], 2).is_empty());
        assert_eq!(Pick::of_path("C:\\claude:end-all"), None);
        assert_eq!(Pick::of_path(&format!("{ROW}nope")), None);
    }

    #[test]
    fn the_question_counts_the_busy_sessions() {
        assert_eq!(ask_words(Mode::End, 1).title, "1 session is working");
        let w = ask_words(Mode::End, 3);
        assert_eq!(w.title, "3 sessions are working");
        assert_eq!((w.primary, w.alt), ("End now", "Wait"));
        assert_eq!(ask_words(Mode::Restart, 2).primary, "Restart now");
        for w in [ask_words(Mode::End, 2), ask_words(Mode::Restart, 2)] {
            assert!(!w.body.contains('\u{2014}') && !w.title.contains('\u{2014}'));
        }
    }

    fn press(tab: &str, key: Press) -> Act {
        Act::Press {
            tab: tab.into(),
            key,
        }
    }

    /// A world of facts the test changes between steps.
    struct World(std::cell::RefCell<Vec<TabFacts>>);

    impl World {
        fn new(tabs: Vec<TabFacts>) -> Self {
            World(std::cell::RefCell::new(tabs))
        }
        fn set(&self, id: &str, f: impl FnOnce(&mut TabFacts)) {
            let mut v = self.0.borrow_mut();
            f(v.iter_mut().find(|t| t.id == id).expect("a tab"));
        }
        /// Claude's box shows what the keys typed: "/exit" after the exit, empty after the Enter.
        fn echo(&self, acts: &[Act]) {
            for a in acts {
                if let Act::Press { tab, key } = a {
                    match key {
                        Press::Exit => self.set(tab, |t| t.prompt_box = Some("/exit".into())),
                        Press::Enter => self.set(tab, |t| t.prompt_box = Some(String::new())),
                        Press::Interrupt => {}
                    }
                }
            }
        }
        fn step(&self, r: &mut Runner, now: u64) -> Vec<Act> {
            let tabs = self.0.borrow().clone();
            let acts = r.step(now, &|id| tabs.iter().find(|t| t.id == id).cloned());
            self.echo(&acts);
            acts
        }
        fn end(&self, r: &mut Runner, mode: Mode, interrupt: bool, now: u64) -> Vec<Act> {
            let acts = r.end(&self.all(), mode, interrupt, now);
            self.echo(&acts);
            acts
        }
        fn all(&self) -> Vec<TabFacts> {
            self.0.borrow().clone()
        }
        /// Claude left the tab and the shell printed its prompt.
        fn left(&self, id: &str) {
            self.set(id, |t| {
                t.claude = false;
                t.prompts += 1;
                t.at_prompt = true;
            });
        }
    }

    /// Rule 1381: an idle Claude gets "/exit", then the Enter 300 ms later, and is done at the shell's next prompt.
    #[test]
    fn an_idle_claude_gets_exit_then_enter_and_the_tab_stays() {
        let w = World::new(vec![claude("a")]);
        let mut r = Runner::default();
        assert_eq!(
            w.end(&mut r, Mode::End, false, 1000),
            [press("a", Press::Exit)]
        );
        assert_eq!(r.next_due(1000), Some(1000 + ENTER_AFTER_MS));
        assert!(w.step(&mut r, 1000 + ENTER_AFTER_MS - 1).is_empty());
        assert_eq!(
            w.step(&mut r, 1000 + ENTER_AFTER_MS),
            [press("a", Press::Enter), Act::LookSoon]
        );
        // The prompt that was there before the Enter is not the shell coming back.
        assert!(w.step(&mut r, 1400).is_empty());
        w.left("a");
        assert_eq!(w.step(&mut r, 1500), [Act::LookSoon]);
        assert!(!r.active());
        assert_eq!(r.take_done(), [("a".to_string(), Outcome::Done)]);
    }

    /// Rule 1382: Wait ends a busy Claude once it is idle, however long that takes; the idle ones end at once.
    #[test]
    fn wait_ends_each_busy_claude_when_it_next_goes_idle() {
        let mut b = claude("b");
        b.busy = true;
        let w = World::new(vec![claude("a"), b]);
        let mut r = Runner::default();
        assert_eq!(
            w.end(&mut r, Mode::End, false, 0),
            [press("a", Press::Exit)],
            "no key for the busy one"
        );
        // Hours later it still works: nothing is pressed, and nothing is given up.
        assert!(
            !w.step(&mut r, 3_600_000)
                .iter()
                .any(|a| matches!(a, Act::Press { tab, .. } if tab == "b"))
        );
        assert!(r.tabs().contains(&"b".to_string()));
        w.set("b", |t| t.busy = false);
        let acts = w.step(&mut r, 3_600_100);
        assert!(acts.contains(&press("b", Press::Exit)), "{acts:?}");
    }

    /// Rule 1382: End now sends Escape to each busy Claude, waits for it to rest, then ends it; a turn that never rests
    /// is given up and left running.
    #[test]
    fn end_now_interrupts_first_and_gives_up_on_a_turn_that_never_rests() {
        let mut b = claude("b");
        b.busy = true;
        let mut c = claude("c");
        c.busy = true;
        let w = World::new(vec![claude("a"), b, c]);
        let mut r = Runner::default();
        let acts = w.end(&mut r, Mode::End, true, 0);
        assert_eq!(
            acts,
            [
                press("b", Press::Interrupt),
                press("c", Press::Interrupt),
                press("a", Press::Exit)
            ]
        );
        // Escape stops b at once, but it is looked at only after the settle.
        w.set("b", |t| t.busy = false);
        assert!(
            !w.step(&mut r, INTERRUPT_SETTLE_MS - 1)
                .contains(&press("b", Press::Exit))
        );
        assert!(
            w.step(&mut r, INTERRUPT_SETTLE_MS)
                .contains(&press("b", Press::Exit))
        );
        // c never rests: given up at the limit, never typed into.
        let acts = w.step(&mut r, INTERRUPT_WAIT_MS);
        assert!(
            !acts
                .iter()
                .any(|a| matches!(a, Act::Press { tab, .. } if tab == "c"))
        );
        assert!(r.take_done().contains(&("c".to_string(), Outcome::GaveUp)));
    }

    /// Rule 1381: a draft in Claude's prompt box keeps the tab (the Enter would send it), and an interrupted turn that
    /// put its words back is kept too.
    #[test]
    fn a_draft_keeps_the_tab() {
        let mut a = claude("a");
        a.prompt_box = Some("half a thought".into());
        let mut b = claude("b");
        b.busy = true;
        let w = World::new(vec![a, b]);
        let mut r = Runner::default();
        assert_eq!(
            w.end(&mut r, Mode::End, true, 0),
            [press("b", Press::Interrupt)]
        );
        w.set("b", |t| {
            t.busy = false;
            t.prompt_box = Some("the words it put back".into());
        });
        assert!(w.step(&mut r, INTERRUPT_SETTLE_MS).is_empty());
        let done = r.take_done();
        assert!(done.contains(&("a".to_string(), Outcome::KeptDraft)));
        assert!(done.contains(&("b".to_string(), Outcome::KeptDraft)));
    }

    /// Rule 1381: the Enter is checked again: a question that came up after "/exit" is never answered by it.
    #[test]
    fn a_question_after_exit_holds_the_enter_back() {
        let w = World::new(vec![claude("a")]);
        let mut r = Runner::default();
        w.end(&mut r, Mode::End, false, 0);
        w.set("a", |t| t.busy = true);
        assert!(w.step(&mut r, ENTER_AFTER_MS).is_empty());
        assert_eq!(r.take_done(), [("a".to_string(), Outcome::GaveUp)]);
    }

    /// Rule 1383: a restart ends Claude and types the start line with the session it had, at the next prompt only.
    #[test]
    fn a_restart_starts_the_same_session_at_the_next_prompt() {
        let w = World::new(vec![claude("a")]);
        let mut r = Runner::default();
        w.end(&mut r, Mode::Restart, false, 0);
        w.step(&mut r, ENTER_AFTER_MS);
        // The hub forgets the session as the agent leaves; the runner kept it.
        w.set("a", |t| t.session = None);
        assert!(w.step(&mut r, 1000).is_empty(), "no prompt yet");
        w.left("a");
        assert_eq!(
            w.step(&mut r, 1100),
            [
                Act::Start {
                    tab: "a".into(),
                    shell: "pwsh".into(),
                    resume: Some(SID.into())
                },
                Act::LookSoon
            ]
        );
        // A shell that never comes back is given up, nothing typed.
        let w = World::new(vec![claude("b")]);
        let mut r = Runner::default();
        w.end(&mut r, Mode::Restart, false, 0);
        w.step(&mut r, ENTER_AFTER_MS);
        assert!(w.step(&mut r, ENTER_AFTER_MS + LEAVE_WAIT_MS).is_empty());
        assert_eq!(r.take_done(), [("b".to_string(), Outcome::GaveUp)]);
    }

    /// Rule 1380: a tab that closes, or turns remote, drops its job; nothing is typed into it.
    #[test]
    fn a_closed_tab_drops_its_job() {
        let mut b = claude("b");
        b.busy = true;
        let w = World::new(vec![b]);
        let mut r = Runner::default();
        w.end(&mut r, Mode::End, false, 0);
        w.set("b", |t| t.local = false);
        assert!(w.step(&mut r, 10).is_empty());
        assert_eq!(r.take_done(), [("b".to_string(), Outcome::Gone)]);
        assert!(r.step(20, &|_| None).is_empty());
    }

    /// Rule 1381: Start types the line in each tab that can take it, with its last session; none on zero tabs.
    #[test]
    fn start_types_the_line_in_each_free_shell() {
        let mut b = shell("b");
        b.session = Some(SID.into());
        let tabs = vec![shell("a"), b, claude("c")];
        assert_eq!(
            Runner::start(&tabs),
            [
                Act::Start {
                    tab: "a".into(),
                    shell: "pwsh".into(),
                    resume: None
                },
                Act::Start {
                    tab: "b".into(),
                    shell: "pwsh".into(),
                    resume: Some(SID.into())
                },
                Act::LookSoon
            ]
        );
        assert!(Runner::start(&[claude("c")]).is_empty());
    }

    /// A Wait for idle adds no deadline of its own (the window steps it as the tab's state moves); every timed phase
    /// does.
    #[test]
    fn only_timed_phases_are_due() {
        let mut b = claude("b");
        b.busy = true;
        let w = World::new(vec![b]);
        let mut r = Runner::default();
        w.end(&mut r, Mode::End, false, 0);
        assert_eq!(r.next_due(0), None);
        let mut r = Runner::default();
        w.end(&mut r, Mode::End, true, 0);
        assert_eq!(r.next_due(0), Some(INTERRUPT_SETTLE_MS));
        // Past the settle, the deadline is next: a time already gone would wake the timer on every tick.
        assert_eq!(r.next_due(INTERRUPT_SETTLE_MS + 1), Some(INTERRUPT_WAIT_MS));
        // "/exit" typed: the Enter, then the echo's deadline.
        let w = World::new(vec![claude("a")]);
        let mut r = Runner::default();
        r.end(&w.all(), Mode::End, false, 0);
        assert_eq!(r.next_due(0), Some(ENTER_AFTER_MS));
        assert_eq!(
            r.next_due(ENTER_AFTER_MS + 1),
            Some(ENTER_AFTER_MS + ECHO_WAIT_MS)
        );
    }

    /// Review: a second pick acts on its own tabs only; the jobs already under way go on (a restart still starts).
    #[test]
    fn a_second_pick_leaves_the_other_jobs_alone() {
        let w = World::new(vec![claude("a"), claude("b")]);
        let mut r = Runner::default();
        let only_a: Vec<TabFacts> = w.all().into_iter().filter(|t| t.id == "a").collect();
        let acts = r.end(&only_a, Mode::Restart, false, 0);
        w.echo(&acts);
        w.step(&mut r, ENTER_AFTER_MS);
        let only_b: Vec<TabFacts> = w.all().into_iter().filter(|t| t.id == "b").collect();
        let acts = r.end(&only_b, Mode::End, false, 400);
        assert_eq!(acts, [press("b", Press::Exit)]);
        assert!(r.take_done().is_empty(), "a is not dropped");
        w.left("a");
        assert!(w.step(&mut r, 500).contains(&Act::Start {
            tab: "a".into(),
            shell: "pwsh".into(),
            resume: Some(SID.into())
        }));
    }

    /// Review: picking again while "/exit" waits for its Enter types no second "/exit".
    #[test]
    fn a_pick_again_never_types_exit_twice() {
        let w = World::new(vec![claude("a")]);
        let mut r = Runner::default();
        assert_eq!(
            w.end(&mut r, Mode::End, false, 0),
            [press("a", Press::Exit)]
        );
        assert!(w.end(&mut r, Mode::End, false, 100).is_empty());
        assert_eq!(
            w.step(&mut r, ENTER_AFTER_MS),
            [press("a", Press::Enter), Act::LookSoon]
        );
        assert!(w.end(&mut r, Mode::End, true, 400).is_empty());
    }

    /// Review: the shell's prompt came after the last key, so the shell is in front, not Claude (a stale agent
    /// mark): nothing is typed; a restart types the start line there.
    #[test]
    fn a_shell_at_its_prompt_gets_no_exit() {
        let mut a = claude("a");
        a.at_prompt = true;
        a.prompts = 1;
        let w = World::new(vec![a]);
        let mut r = Runner::default();
        assert!(w.end(&mut r, Mode::End, false, 0).is_empty());
        assert_eq!(r.take_done(), [("a".to_string(), Outcome::Done)]);
        let acts = w.end(&mut r, Mode::Restart, false, 0);
        assert!(
            !acts.iter().any(|a| matches!(a, Act::Press { .. })),
            "{acts:?}"
        );
        assert!(acts.iter().any(|a| matches!(a, Act::Start { .. })));
    }

    /// Review: no prompt box on the screen (a picker, a menu, an editor): "/exit" is never typed.
    #[test]
    fn no_prompt_box_gets_no_exit() {
        let mut a = claude("a");
        a.prompt_box = None;
        let w = World::new(vec![a]);
        let mut r = Runner::default();
        assert!(w.end(&mut r, Mode::End, false, 0).is_empty());
        assert_eq!(r.take_done(), [("a".to_string(), Outcome::NoPrompt)]);
    }

    /// Review: the Enter waits for "/exit" to show in the box, and is never pressed when the box shows anything
    /// else (the person typed meanwhile) or is gone.
    #[test]
    fn the_enter_waits_for_the_echo_and_never_sends_other_words() {
        let w = World::new(vec![claude("a"), claude("b"), claude("c")]);
        let mut r = Runner::default();
        r.end(&w.all(), Mode::End, false, 0);
        // a: the echo is late; b: the person typed too; c: the box went.
        w.set("b", |t| t.prompt_box = Some("/exit and more".into()));
        w.set("c", |t| t.prompt_box = None);
        assert!(w.step(&mut r, ENTER_AFTER_MS).is_empty());
        w.set("a", |t| t.prompt_box = Some("/exit".into()));
        assert_eq!(
            w.step(&mut r, ENTER_AFTER_MS + 100),
            [press("a", Press::Enter), Act::LookSoon]
        );
        assert!(w.step(&mut r, ENTER_AFTER_MS + ECHO_WAIT_MS).is_empty());
        let done = r.take_done();
        assert!(
            done.contains(&("b".to_string(), Outcome::GaveUp)),
            "{done:?}"
        );
        assert!(
            done.contains(&("c".to_string(), Outcome::GaveUp)),
            "{done:?}"
        );
    }

    /// Review: a restart resumes the session Claude was in when "/exit" was typed, not the one at the pick.
    #[test]
    fn a_restart_resumes_the_session_at_the_exit() {
        const OTHER: &str = "0f0e0d0c-2222-4333-8444-955566667777";
        let mut a = claude("a");
        a.busy = true;
        let w = World::new(vec![a]);
        let mut r = Runner::default();
        w.end(&mut r, Mode::Restart, false, 0);
        // Under Wait the person ran /clear: the hooks named a new session.
        w.set("a", |t| {
            t.session = Some(OTHER.into());
            t.busy = false;
        });
        w.step(&mut r, 1000);
        w.step(&mut r, 1000 + ENTER_AFTER_MS);
        w.left("a");
        assert!(w.step(&mut r, 2000).contains(&Act::Start {
            tab: "a".into(),
            shell: "pwsh".into(),
            resume: Some(OTHER.into())
        }));
    }
}
