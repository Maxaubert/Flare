//! The send rules: where heard text goes (Task 9). Ported from Prism Terminal's `core/main/orchestratorSend.ts`,
//! `core/main/dictationClient.ts` and the pure parts of `src/main/dictateAnywhere.ts` at e5848cb.
//!
//! Flare has no orchestrator (D9): `dictation_route` is called with `orchestrator_on = false` and `SendTab::orchestrator`
//! is always false, so a later port of the orchestrator is one switch.
//!
//! Typing into a tab is the rule 2 exception: a running Claude that is not asking, never a shell. It is checked three
//! times: by the tab's mark and session (`typeable`), by Windows' process tree (`SendHost::agent_under`), and by the same
//! tab and session right before the paste and again before the Enter (`type_checked`).

use flare_shells::paste::sanitize_paste;

/// What the send rules need of one tab: a Claude tab, or a shell that the rules refuse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendTab {
    /// The session id (`<pid>:<id>` for a tab of a linked copy).
    pub id: String,
    /// The shell's tab id.
    pub tab: String,
    pub label: String,
    pub cwd: String,
    /// The agent the tab runs (`claude`), when its own output or the hook says so.
    pub agent: Option<String>,
    /// `question` when the agent waits on an answer.
    pub state: Option<String>,
    /// Always false in Flare (D9).
    pub orchestrator: bool,
    /// The Claude session the tab runs.
    pub session: Option<String>,
    /// The copy of the app the tab is in, named only when more than one copy is open (N27).
    pub copy: Option<String>,
}

/// Whether a tab may take dictated text, and the tab when it may. After a send: `Ok` the words were sent,
/// `Typed` they are in the tab's prompt but the Enter did not send them, `No` they were not typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendDecision {
    Ok(SendTab),
    Typed(SendTab),
    No(String),
}

/// A linked copy of the app: its process id and its name in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteCopy {
    pub pid: u32,
    pub name: String,
}

/// Where dictated text goes when it was heard with no window of this copy in front.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Lead(SendTab),
    Pick(Vec<SendTab>),
}

/// One row of the picker. `disabled` names why a row cannot be picked; `group` is the copy's name when the list has
/// more than one copy (the header), else `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayRow {
    pub id: String,
    pub n: u8,
    pub label: String,
    pub folder: String,
    pub state: Option<String>,
    pub disabled: Option<String>,
    pub lead: bool,
    pub group: Option<String>,
}

/// Long enough for any dictation; a paste this size is not a message (the same cap as the orchestrator's).
pub const SEND_MAX: usize = 8000;
/// The lone Enter follows the paste after at least this long, only if the same tab is still sendable.
pub const SEND_ENTER_MS: u64 = 300;
/// How long the words may take to show in Claude's prompt: this, plus `WORDS_PER_CHAR_MS` a character, at
/// most `WORDS_MAX_MS`. Claude takes in a long paste slowly, and an Enter before it has is lost.
pub const WORDS_BASE_MS: u64 = 300;
pub const WORDS_PER_CHAR_MS: u64 = 1;
pub const WORDS_MAX_MS: u64 = 3000;
/// How long the Enter may take to empty the prompt.
pub const ENTER_TAKE_MS: u64 = 2000;
/// How often the prompt is read while waiting.
pub const PROMPT_POLL_MS: u64 = 50;
/// The picker shows nine rows at most (the digits 1 to 9).
const PICK_MAX: usize = 9;

/// What the send needs from the app: the shell's child check, the write to a tab's shell, and a delay.
pub trait SendHost {
    /// What Windows says runs under a tab's shell (`claude`, or something else). Called off the UI thread.
    fn agent_under(&self, id: &str) -> Option<String>;
    /// Write to a tab's shell. False when nothing was written (the window could not take it).
    fn write(&self, id: &str, bytes: &str) -> bool;
    /// Whether Claude's prompt box in this tab holds unsent text (read off its screen): `None` when no box
    /// can be read (then the send cannot watch its words land, and falls back to a timed Enter).
    fn prompt_draft(&self, _id: &str) -> Option<bool> {
        None
    }
    /// Run `f` after `ms` milliseconds. The real host sleeps on the send's worker, then calls `f`.
    fn later(&self, ms: u64, f: Box<dyn FnOnce() + '_>);
    /// Wait `ms` milliseconds (on the send's worker).
    fn pause(&self, ms: u64) {
        self.later(ms, Box::new(|| {}));
    }
    /// A step of the send, for the diagnostics log: its name, the text's length or a wait in ms. Never the words.
    fn stage(&self, _what: &str, _n: u64) {}
}

/// Whether the app may type into this tab at all: a Claude session that is not asking.
pub fn typeable(t: &SendTab) -> SendDecision {
    // Only into a Claude Code prompt: never a shell, where the text would run as a command.
    if t.agent.as_deref() != Some("claude") || t.session.is_none() {
        return SendDecision::No(format!("{} has no Claude session running", t.label));
    }
    // A question or permission box would take the text as its answer.
    if t.state.as_deref() == Some("question") {
        return SendDecision::No(format!(
            "{} is waiting on a question: answer it there first",
            t.label
        ));
    }
    SendDecision::Ok(t.clone())
}

/// The tab the picked id names, if it may take text now.
pub fn decide_into(id: &str, tabs: &[SendTab]) -> SendDecision {
    match tabs.iter().find(|t| t.id == id) {
        Some(t) => typeable(t),
        None => SendDecision::No("that tab has closed".into()),
    }
}

/// Where dictated text goes: the lead when the orchestrator is on and its Claude can take it, else the tabs with a
/// Claude and a session, the lead first, nine at most. Flare passes `orchestrator_on = false` (D9).
pub fn dictation_route(tabs: &[SendTab], orchestrator_on: bool) -> Route {
    if orchestrator_on {
        if let Some(lead) = tabs.iter().find(|t| t.orchestrator) {
            if let SendDecision::Ok(t) = typeable(lead) {
                return Route::Lead(t);
            }
        }
    }
    let (mut lead, rest): (Vec<SendTab>, Vec<SendTab>) = tabs
        .iter()
        .filter(|t| t.agent.as_deref() == Some("claude") && t.session.is_some())
        .cloned()
        .partition(|t| t.orchestrator);
    lead.extend(rest);
    lead.truncate(PICK_MAX);
    Route::Pick(lead)
}

/// What is typed: the text as one bracketed paste, CRLF and CR taken to LF, every control but tab, LF and CR taken out.
pub fn paste_of(text: &str) -> String {
    let lf = text.replace("\r\n", "\n").replace('\r', "\n");
    format!("\u{1b}[200~{}\u{1b}[201~", sanitize_paste(&lf))
}

/// The picker's rows: grouped by copy in the order the copies first appear, numbered 1 to 9 across the groups, a row
/// that is asking disabled.
pub fn pick_rows(tabs: &[SendTab]) -> Vec<OverlayRow> {
    let mut order: Vec<Option<String>> = Vec::new();
    for t in tabs {
        if !order.contains(&t.copy) {
            order.push(t.copy.clone());
        }
    }
    let many = order.len() > 1;
    order
        .iter()
        .flat_map(|g| tabs.iter().filter(move |t| t.copy == *g))
        .take(PICK_MAX)
        .enumerate()
        .map(|(i, t)| OverlayRow {
            id: t.id.clone(),
            n: (i + 1) as u8,
            label: t.label.clone(),
            folder: folder_tail(&t.cwd),
            state: t.state.clone(),
            disabled: (t.state.as_deref() == Some("question"))
                .then(|| "Waiting on a question: answer it there first".to_string()),
            lead: t.orchestrator,
            group: if many { t.copy.clone() } else { None },
        })
        .collect()
}

/// Each copy's name; a second of the same name is numbered (`Flare (2)`).
pub fn copy_names(names: &[String]) -> Vec<String> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let k = names[..=i].iter().filter(|m| *m == n).count();
            if k == 1 {
                n.clone()
            } else {
                format!("{n} ({k})")
            }
        })
        .collect()
}

/// This copy's tabs and every linked copy's, as one list. A linked copy's tab id is `<pid>:<id>`.
pub fn merge_tabs(
    local: &[SendTab],
    remote: &[(RemoteCopy, Vec<SendTab>)],
    me: &str,
) -> Vec<SendTab> {
    let mut all = vec![me.to_string()];
    all.extend(remote.iter().map(|(c, _)| c.name.clone()));
    let names = copy_names(&all);
    let many = !remote.is_empty();
    let mut out: Vec<SendTab> = local
        .iter()
        .map(|t| SendTab {
            copy: many.then(|| names[0].clone()),
            ..t.clone()
        })
        .collect();
    for (i, (c, tabs)) in remote.iter().enumerate() {
        out.extend(tabs.iter().map(|t| SendTab {
            id: format!("{}:{}", c.pid, t.id),
            copy: Some(names[i + 1].clone()),
            ..t.clone()
        }));
    }
    out
}

/// The last two parts of a folder path, joined with a backslash.
pub fn folder_tail(p: &str) -> String {
    let parts: Vec<&str> = p.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    parts[parts.len().saturating_sub(2)..].join("\\")
}

/// Types into a tab, checked. `decide` names the tab and its limits; then Windows is asked whether Claude runs there,
/// `decide` is asked again (the same tab and session must win), the paste is written, and the Enter is written only
/// if `decide` still names the same tab and session and Windows still says Claude runs there.
///
/// The answer is honest: with a prompt box to read, the send waits until the words show in it (a bound that grows
/// with their length), then writes the Enter at least `SEND_ENTER_MS` after them, then waits until the prompt is
/// empty. `Ok` only when it emptied; `Typed` when the words stayed there; `No` when they never showed. With no box
/// to read, the Enter follows after `SEND_ENTER_MS` and a written Enter is `Ok`.
pub fn type_checked(
    decide: &dyn Fn() -> SendDecision,
    text: &str,
    host: &dyn SendHost,
) -> SendDecision {
    let said = match decide() {
        SendDecision::Ok(t) if host.agent_under(&t.id).as_deref() != Some("claude") => {
            SendDecision::No(format!(
                "no Claude is running in {} (checked with Windows)",
                t.label
            ))
        }
        other => other,
    };
    let said = match said {
        SendDecision::Ok(was) => match decide() {
            SendDecision::Ok(now) if now.id == was.id && now.session == was.session => {
                SendDecision::Ok(now)
            }
            _ => SendDecision::No(format!(
                "{} changed while it was checked: send again",
                was.label
            )),
        },
        other => other,
    };
    // Claude reads a leading "!" as a shell command and "#" as a memory note: dictated words are neither.
    let mut readable = false;
    let said = match said {
        SendDecision::Ok(t) if text.trim_start().starts_with(['!', '#']) => SendDecision::No(
            format!("{}: words that start with ! or # are not typed", t.label),
        ),
        SendDecision::Ok(t) => match host.prompt_draft(&t.id) {
            Some(true) => SendDecision::No(format!(
                "{} has unsent text in its prompt: send or clear it first",
                t.label
            )),
            seen => {
                readable = seen.is_some();
                SendDecision::Ok(t)
            }
        },
        other => other,
    };
    let t = match said {
        SendDecision::Ok(t) => t,
        other => {
            host.stage("refused", 0);
            return other;
        }
    };
    let len = text.chars().count() as u64;
    host.stage(
        if readable {
            "checked"
        } else {
            "checked, no prompt to read"
        },
        len,
    );
    // Words that never reached the shell get no Enter: an Enter alone would run what was already typed.
    if !host.write(&t.id, &paste_of(text)) {
        host.stage("words refused", len);
        return SendDecision::No(format!("{} could not take the words: send again", t.label));
    }
    // The same tab, the same session, and Windows still says Claude runs there: if Claude quit while the words
    // were landing, the shell's line editor holds them, and an Enter now would run them as a command.
    let still_there = || {
        let same = match decide() {
            SendDecision::Ok(still) => still.id == t.id && still.session == t.session,
            _ => false,
        };
        same && host.agent_under(&t.id).as_deref() == Some("claude")
    };
    let held_back = || {
        host.stage("enter held back", 0);
        SendDecision::No(format!("{} changed before the Enter", t.label))
    };
    if !readable {
        host.pause(SEND_ENTER_MS);
        if !still_there() {
            return held_back();
        }
        if !host.write(&t.id, "\r") {
            host.stage("enter refused", 0);
            return SendDecision::Typed(t);
        }
        host.stage("enter written, unseen", SEND_ENTER_MS);
        return SendDecision::Ok(t);
    }
    // The words show in the prompt first: an Enter before Claude has taken in the paste is lost.
    let bound = (WORDS_BASE_MS + WORDS_PER_CHAR_MS * len).min(WORDS_MAX_MS);
    let mut waited = 0;
    let shown = loop {
        host.pause(PROMPT_POLL_MS);
        waited += PROMPT_POLL_MS;
        if host.prompt_draft(&t.id) == Some(true) {
            break true;
        }
        if waited >= bound {
            break false;
        }
    };
    if !shown {
        host.stage("words not shown", waited);
        return SendDecision::No(format!("the words did not show in {}", t.label));
    }
    host.stage("words shown", waited);
    if waited < SEND_ENTER_MS {
        host.pause(SEND_ENTER_MS - waited);
    }
    if !still_there() {
        return held_back();
    }
    if !host.write(&t.id, "\r") {
        host.stage("enter refused", 0);
        return SendDecision::Typed(t);
    }
    // The Enter took when the prompt is empty again (or its box has gone, the words with it).
    let mut waited = 0;
    let taken = loop {
        host.pause(PROMPT_POLL_MS);
        waited += PROMPT_POLL_MS;
        let now = host.prompt_draft(&t.id);
        if now == Some(false) {
            break true;
        }
        if waited >= ENTER_TAKE_MS {
            break now.is_none();
        }
    };
    host.stage(
        if taken {
            "enter taken"
        } else {
            "enter not taken"
        },
        waited,
    );
    if taken {
        SendDecision::Ok(t)
    } else {
        SendDecision::Typed(t)
    }
}

/// The tabs as the list gets them: the orchestrator marked only when it is switched on and its Claude runs per Windows.
pub fn publish_tabs(
    tabs: Vec<SendTab>,
    orchestrator_on: bool,
    agent_under: &dyn Fn(&str) -> Option<String>,
) -> Vec<SendTab> {
    let lead = if orchestrator_on {
        tabs.iter().find(|t| t.orchestrator).map(|t| t.id.clone())
    } else {
        None
    };
    let active = lead
        .as_deref()
        .is_some_and(|id| agent_under(id).as_deref() == Some("claude"));
    tabs.into_iter()
        .map(|t| SendTab {
            orchestrator: active && lead.as_deref() == Some(t.id.as_str()),
            ..t
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    // DROPPED: the five other decideSend tests (orchestratorSend.test.ts): decideSend is the orchestrator's (DIFF-16).

    const A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    fn tab() -> SendTab {
        SendTab {
            id: "p1".into(),
            tab: "t1".into(),
            label: "Wind".into(),
            cwd: "C:\\src\\wind".into(),
            agent: Some("claude".into()),
            state: None,
            orchestrator: false,
            session: Some(A.into()),
            copy: None,
        }
    }

    fn lead() -> SendTab {
        SendTab {
            id: "p0".into(),
            label: "Lead".into(),
            orchestrator: true,
            ..tab()
        }
    }

    /// A stand-in host: Windows says `agent`, writes are recorded, and `later` runs at once (recording its delay).
    #[derive(Default)]
    struct Fake {
        /// What each read of the prompt box says, in turn; the last one stays. Empty: no box to read.
        reads: RefCell<Vec<Option<bool>>>,
        /// The prompt reads true once the words are written, and false once an Enter is (a Claude that takes them).
        claude_screen: bool,
        agent: Option<String>,
        /// What Windows says from the second question on (Claude quit in between).
        agent_then: Option<Option<String>>,
        asked: Cell<usize>,
        /// The window could not take a write (busy): nothing is written.
        refuse: bool,
        written: RefCell<Vec<String>>,
        delays: RefCell<Vec<u64>>,
    }

    impl SendHost for Fake {
        fn agent_under(&self, _id: &str) -> Option<String> {
            let n = self.asked.get();
            self.asked.set(n + 1);
            match (&self.agent_then, n) {
                (Some(then), 1..) => then.clone(),
                _ => self.agent.clone(),
            }
        }
        fn prompt_draft(&self, _id: &str) -> Option<bool> {
            if self.claude_screen {
                let w = self.written.borrow();
                return Some(w.len() == 1);
            }
            let mut r = self.reads.borrow_mut();
            match r.len() {
                0 => None,
                1 => r[0],
                _ => r.remove(0),
            }
        }
        fn write(&self, _id: &str, bytes: &str) -> bool {
            if self.refuse {
                return false;
            }
            self.written.borrow_mut().push(bytes.to_string());
            true
        }
        fn later(&self, ms: u64, f: Box<dyn FnOnce() + '_>) {
            self.delays.borrow_mut().push(ms);
            f();
        }
    }

    #[test]
    fn decide_into_sends_to_the_picked_tab_only_while_its_claude_can_take_it() {
        assert!(matches!(decide_into("p1", &[tab()]), SendDecision::Ok(_)));
        assert_eq!(
            decide_into("gone", &[tab()]),
            SendDecision::No("that tab has closed".into())
        );
        let asking = SendTab {
            state: Some("question".into()),
            ..tab()
        };
        assert!(matches!(decide_into("p1", &[asking]), SendDecision::No(_)));
        let shell = SendTab {
            agent: None,
            session: None,
            ..tab()
        };
        assert!(matches!(decide_into("p1", &[shell]), SendDecision::No(_)));
    }

    #[test]
    fn pick_rows_numbers_the_rows_and_disables_a_tab_that_is_asking() {
        let web = SendTab {
            id: "p2".into(),
            label: "Web".into(),
            cwd: "C:\\a\\b\\web".into(),
            state: Some("question".into()),
            ..tab()
        };
        let rows = pick_rows(&[
            SendTab {
                orchestrator: true,
                ..tab()
            },
            web,
        ]);
        let got: Vec<(u8, &str, &str, bool, bool)> = rows
            .iter()
            .map(|r| {
                (
                    r.n,
                    r.label.as_str(),
                    r.folder.as_str(),
                    r.disabled.is_some(),
                    r.lead,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (1, "Wind", "src\\wind", false, true),
                (2, "Web", "b\\web", true, false)
            ]
        );
    }

    #[test]
    fn folder_tail_shows_the_last_two_parts() {
        assert_eq!(folder_tail("C:\\Users\\me\\code\\api"), "code\\api");
        assert_eq!(folder_tail("/home/me/api/"), "me\\api");
    }

    #[test]
    fn pick_rows_keeps_each_copy_together_numbered_across_them() {
        let rows = pick_rows(&[
            SendTab {
                id: "a".into(),
                label: "Lead".into(),
                orchestrator: true,
                copy: Some("Flare".into()),
                ..tab()
            },
            SendTab {
                id: "b".into(),
                label: "Wind".into(),
                copy: Some("Flare (Dev)".into()),
                ..tab()
            },
            SendTab {
                id: "c".into(),
                label: "Sonara".into(),
                copy: Some("Flare".into()),
                ..tab()
            },
        ]);
        let got: Vec<(u8, &str, Option<&str>)> = rows
            .iter()
            .map(|r| (r.n, r.label.as_str(), r.group.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                (1, "Lead", Some("Flare")),
                (2, "Sonara", Some("Flare")),
                (3, "Wind", Some("Flare (Dev)"))
            ]
        );
    }

    #[test]
    fn pick_rows_draws_no_headers_when_every_session_is_in_one_copy() {
        let rows = pick_rows(&[
            SendTab {
                id: "b".into(),
                copy: Some("Flare".into()),
                ..tab()
            },
            SendTab {
                id: "c".into(),
                copy: Some("Flare".into()),
                ..tab()
            },
        ]);
        assert!(rows.iter().all(|r| r.group.is_none()));
    }

    #[test]
    fn merge_tabs_lists_every_copy_naming_the_app_only_when_there_is_more_than_one() {
        let solo = merge_tabs(&[tab()], &[], "Flare");
        assert_eq!(
            solo.iter()
                .map(|t| (t.id.as_str(), t.copy.clone()))
                .collect::<Vec<_>>(),
            vec![("p1", None)]
        );
        let remote = RemoteCopy {
            pid: 9,
            name: "Flare (Dev)".into(),
        };
        let two = merge_tabs(
            &[tab()],
            &[(
                remote,
                vec![SendTab {
                    id: "q1".into(),
                    ..tab()
                }],
            )],
            "Flare",
        );
        assert_eq!(
            two.iter()
                .map(|t| (t.id.as_str(), t.copy.as_deref()))
                .collect::<Vec<_>>(),
            vec![("p1", Some("Flare")), ("9:q1", Some("Flare (Dev)"))]
        );
    }

    #[test]
    fn merge_tabs_numbers_a_second_copy_of_the_same_name() {
        let remote = RemoteCopy {
            pid: 9,
            name: "Flare".into(),
        };
        let t = merge_tabs(
            &[tab()],
            &[(
                remote,
                vec![SendTab {
                    id: "q1".into(),
                    ..tab()
                }],
            )],
            "Flare",
        );
        assert_eq!(
            t.iter()
                .map(|x| x.copy.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["Flare", "Flare (2)"]
        );
        assert_eq!(
            copy_names(&["Flare".to_string(), "Flare".to_string()]),
            vec!["Flare", "Flare (2)"]
        );
    }

    #[test]
    fn dictation_route_goes_to_the_orchestrator_when_on_promoted_and_its_claude_can_take_it() {
        let tabs = vec![lead(), tab()];
        assert_eq!(dictation_route(&tabs, true), Route::Lead(lead()));
        // Flare passes false (D9): the same tabs are listed instead.
        assert!(matches!(dictation_route(&tabs, false), Route::Pick(_)));
    }

    #[test]
    fn dictation_route_lists_the_sessions_when_the_orchestrator_is_not_active() {
        let cases: Vec<(Vec<SendTab>, bool)> = vec![
            (vec![lead(), tab()], false),
            (
                vec![
                    SendTab {
                        state: Some("question".into()),
                        ..lead()
                    },
                    tab(),
                ],
                true,
            ),
            (
                vec![
                    SendTab {
                        agent: None,
                        session: None,
                        ..lead()
                    },
                    tab(),
                ],
                true,
            ),
        ];
        for (tabs, on) in cases {
            assert!(
                matches!(dictation_route(&tabs, on), Route::Pick(_)),
                "on = {on}"
            );
        }
    }

    #[test]
    fn dictation_route_lists_only_tabs_with_a_claude_the_orchestrator_first_nine_at_most() {
        let mut tabs = vec![SendTab {
            id: "sh".into(),
            agent: None,
            session: None,
            ..tab()
        }];
        tabs.extend((0..12).map(|i| SendTab {
            id: format!("p{i}"),
            label: format!("t{i}"),
            ..tab()
        }));
        tabs.push(SendTab {
            id: "L".into(),
            orchestrator: true,
            state: Some("question".into()),
            ..tab()
        });
        for on in [true, false] {
            let Route::Pick(p) = dictation_route(&tabs, on) else {
                panic!("expected a list")
            };
            let ids: Vec<&str> = p.iter().map(|t| t.id.as_str()).collect();
            assert_eq!(
                ids,
                vec!["L", "p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7"]
            );
        }
    }

    #[test]
    fn type_checked_pastes_then_presses_enter_only_if_the_tab_can_still_take_it() {
        let host = Fake {
            agent: Some("claude".into()),
            ..Default::default()
        };
        let decide = || SendDecision::Ok(tab());
        assert_eq!(
            type_checked(&decide, "hello", &host),
            SendDecision::Ok(tab())
        );
        assert_eq!(
            *host.written.borrow(),
            vec!["\x1b[200~hello\x1b[201~".to_string(), "\r".to_string()]
        );
        assert_eq!(*host.delays.borrow(), vec![SEND_ENTER_MS]);
    }

    #[test]
    fn type_checked_never_presses_enter_after_words_that_were_not_written() {
        let host = Fake {
            agent: Some("claude".into()),
            refuse: true,
            ..Default::default()
        };
        let decide = || SendDecision::Ok(tab());
        assert_eq!(
            type_checked(&decide, "hello", &host),
            SendDecision::No("Wind could not take the words: send again".into())
        );
        assert!(host.written.borrow().is_empty());
        assert!(
            host.delays.borrow().is_empty(),
            "no Enter is even scheduled"
        );
    }

    #[test]
    fn type_checked_types_nothing_into_a_prompt_that_holds_an_unsent_draft() {
        let host = Fake {
            agent: Some("claude".into()),
            reads: RefCell::new(vec![Some(true)]),
            ..Default::default()
        };
        let decide = || SendDecision::Ok(tab());
        assert_eq!(
            type_checked(&decide, "hello", &host),
            SendDecision::No("Wind has unsent text in its prompt: send or clear it first".into())
        );
        assert!(
            host.written.borrow().is_empty(),
            "the draft is never sent with it"
        );
        assert!(host.delays.borrow().is_empty());
    }

    #[test]
    fn type_checked_types_nothing_that_claude_would_read_as_a_command_or_a_note() {
        for text in ["!del *", "  # remember", "#x"] {
            let host = Fake {
                agent: Some("claude".into()),
                ..Default::default()
            };
            let decide = || SendDecision::Ok(tab());
            assert!(
                matches!(type_checked(&decide, text, &host), SendDecision::No(_)),
                "{text}"
            );
            assert!(host.written.borrow().is_empty(), "{text}");
        }
        let host = Fake {
            agent: Some("claude".into()),
            ..Default::default()
        };
        let decide = || SendDecision::Ok(tab());
        assert!(matches!(
            type_checked(&decide, "hello! world #1", &host),
            SendDecision::Ok(_)
        ));
    }

    #[test]
    fn type_checked_types_nothing_where_windows_says_no_claude_runs() {
        let host = Fake {
            agent: Some("pwsh".into()),
            ..Default::default()
        };
        let decide = || SendDecision::Ok(tab());
        assert_eq!(
            type_checked(&decide, "hello", &host),
            SendDecision::No("no Claude is running in Wind (checked with Windows)".into())
        );
        assert!(host.written.borrow().is_empty());
    }

    #[test]
    fn type_checked_types_nothing_when_the_tab_changed_while_windows_was_asked() {
        let host = Fake {
            agent: Some("claude".into()),
            ..Default::default()
        };
        let n = Cell::new(0);
        let decide = || {
            n.set(n.get() + 1);
            let s = if n.get() == 1 { "one" } else { "two" };
            SendDecision::Ok(SendTab {
                session: Some(s.to_string()),
                ..tab()
            })
        };
        assert_eq!(
            type_checked(&decide, "hello", &host),
            SendDecision::No("Wind changed while it was checked: send again".into())
        );
        assert!(host.written.borrow().is_empty());
    }

    #[test]
    fn type_checked_holds_the_enter_back_when_claude_quit_while_the_words_landed() {
        let host = Fake {
            agent: Some("claude".into()),
            // The first question says Claude; by the Enter, Windows says the shell is alone.
            agent_then: Some(Some("pwsh".into())),
            ..Default::default()
        };
        let decide = || SendDecision::Ok(tab());
        assert_eq!(
            type_checked(&decide, "hello", &host),
            SendDecision::No("Wind changed before the Enter".into()),
            "never \"Sent\" for words that were not sent"
        );
        assert_eq!(
            *host.written.borrow(),
            vec!["\x1b[200~hello\x1b[201~".to_string()],
            "the words went in, but no Enter follows them into a shell"
        );
    }

    #[test]
    fn type_checked_holds_the_enter_back_when_the_session_changed_before_it() {
        let host = Fake {
            agent: Some("claude".into()),
            ..Default::default()
        };
        let calls = Cell::new(0);
        let decide = || {
            calls.set(calls.get() + 1);
            // Two for the check; the third, at Enter time, is another conversation.
            let session = if calls.get() <= 2 { A } else { "other" };
            SendDecision::Ok(SendTab {
                session: Some(session.into()),
                ..tab()
            })
        };
        let _ = type_checked(&decide, "hello", &host);
        assert_eq!(host.written.borrow().len(), 1, "no Enter");
    }

    #[test]
    fn type_checked_holds_the_enter_back_when_a_question_opened_before_it() {
        let host = Fake {
            agent: Some("claude".into()),
            ..Default::default()
        };
        let calls = Cell::new(0);
        let decide = || {
            calls.set(calls.get() + 1);
            if calls.get() <= 2 {
                SendDecision::Ok(tab())
            } else {
                SendDecision::No("asking".into())
            }
        };
        assert_eq!(
            type_checked(&decide, "hello", &host),
            SendDecision::No("Wind changed before the Enter".into())
        );
        assert_eq!(
            *host.written.borrow(),
            vec!["\x1b[200~hello\x1b[201~".to_string()]
        );
    }

    #[test]
    fn type_checked_says_sent_only_once_the_words_showed_and_the_enter_emptied_the_prompt() {
        let host = Fake {
            agent: Some("claude".into()),
            claude_screen: true,
            ..Default::default()
        };
        let decide = || SendDecision::Ok(tab());
        assert_eq!(
            type_checked(&decide, "hello", &host),
            SendDecision::Ok(tab())
        );
        assert_eq!(
            *host.written.borrow(),
            vec!["\x1b[200~hello\x1b[201~".to_string(), "\r".to_string()]
        );
        // One read for the words, the rest of the 300 ms before the Enter, one read for the empty prompt.
        assert_eq!(
            *host.delays.borrow(),
            vec![
                PROMPT_POLL_MS,
                SEND_ENTER_MS - PROMPT_POLL_MS,
                PROMPT_POLL_MS
            ]
        );
    }

    #[test]
    fn type_checked_presses_no_enter_when_the_words_never_show_in_the_prompt() {
        let host = Fake {
            agent: Some("claude".into()),
            // The box is there and stays empty: the paste never reached Claude's prompt.
            reads: RefCell::new(vec![Some(false)]),
            ..Default::default()
        };
        let decide = || SendDecision::Ok(tab());
        assert_eq!(
            type_checked(&decide, "hello", &host),
            SendDecision::No("the words did not show in Wind".into())
        );
        assert_eq!(
            host.written.borrow().len(),
            1,
            "the words, and never an Enter"
        );
        // A short text waits the base bound: 300 ms + 1 ms a character, in 50 ms reads.
        let waited: u64 = host.delays.borrow().iter().sum();
        assert_eq!(waited, 350);
    }

    #[test]
    fn type_checked_waits_longer_for_a_long_paste_but_never_past_three_seconds() {
        let wait_for = |len: usize| {
            let host = Fake {
                agent: Some("claude".into()),
                reads: RefCell::new(vec![Some(false)]),
                ..Default::default()
            };
            let decide = || SendDecision::Ok(tab());
            let _ = type_checked(&decide, &"a".repeat(len), &host);
            let waited: u64 = host.delays.borrow().iter().sum();
            waited
        };
        assert_eq!(wait_for(1000), 1300);
        assert_eq!(wait_for(8000), WORDS_MAX_MS);
    }

    #[test]
    fn type_checked_says_typed_not_sent_when_the_enter_leaves_the_words_in_the_prompt() {
        let host = Fake {
            agent: Some("claude".into()),
            // Empty before, the words after, and they stay there.
            reads: RefCell::new(vec![Some(false), Some(true)]),
            ..Default::default()
        };
        let decide = || SendDecision::Ok(tab());
        assert_eq!(
            type_checked(&decide, "hello", &host),
            SendDecision::Typed(tab())
        );
        assert_eq!(host.written.borrow().len(), 2, "the words and one Enter");
        let after_enter: u64 = host.delays.borrow().iter().skip(2).sum();
        assert_eq!(after_enter, ENTER_TAKE_MS);
    }

    #[test]
    fn publish_tabs_marks_the_orchestrator_only_when_switched_on_and_its_claude_runs() {
        let tabs = vec![
            SendTab {
                orchestrator: true,
                ..tab()
            },
            SendTab {
                id: "p2".into(),
                ..tab()
            },
        ];
        let claude = |_: &str| Some("claude".to_string());
        let none = |_: &str| None::<String>;
        let flags = |t: Vec<SendTab>| t.iter().map(|x| x.orchestrator).collect::<Vec<_>>();
        assert_eq!(
            flags(publish_tabs(tabs.clone(), true, &claude)),
            vec![true, false]
        );
        assert_eq!(
            flags(publish_tabs(tabs.clone(), false, &claude)),
            vec![false, false]
        );
        assert_eq!(
            flags(publish_tabs(tabs.clone(), true, &none)),
            vec![false, false]
        );
    }

    #[test]
    fn paste_of_frames_the_text_as_one_paste_with_every_escape_and_control_taken_out() {
        assert_eq!(
            paste_of("fix it\r\nthen test\u{1b}[201~\u{3} done"),
            "\u{1b}[200~fix it\nthen test[201~ done\u{1b}[201~"
        );
    }

    #[test]
    fn never_types_into_a_shell_or_into_a_question() {
        let shell = SendTab {
            agent: None,
            session: None,
            ..tab()
        };
        assert_eq!(
            typeable(&shell),
            SendDecision::No("Wind has no Claude session running".into())
        );
        let asking = SendTab {
            state: Some("question".into()),
            ..tab()
        };
        assert_eq!(
            typeable(&asking),
            SendDecision::No("Wind is waiting on a question: answer it there first".into())
        );
        let working = SendTab {
            state: Some("working".into()),
            ..tab()
        };
        assert!(matches!(typeable(&working), SendDecision::Ok(_)));
    }
}
