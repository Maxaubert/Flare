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

/// Whether a tab may take dictated text, and the tab when it may.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendDecision {
    Ok(SendTab),
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
/// The lone Enter follows the paste after this long, only if the same tab is still sendable.
pub const SEND_ENTER_MS: u64 = 300;
/// The picker shows nine rows at most (the digits 1 to 9).
const PICK_MAX: usize = 9;

/// What the send needs from the app: the shell's child check, the write to a tab's shell, and a delay.
pub trait SendHost {
    /// What Windows says runs under a tab's shell (`claude`, or something else). Called off the UI thread.
    fn agent_under(&self, id: &str) -> Option<String>;
    /// Write to a tab's shell. False when nothing was written (the window could not take it).
    fn write(&self, id: &str, bytes: &str) -> bool;
    /// Run `f` after `ms` milliseconds. The real host sleeps on the send's worker, then calls `f`.
    fn later(&self, ms: u64, f: Box<dyn FnOnce() + '_>);
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
/// `decide` is asked again (the same tab and session must win), the paste is written, and after `SEND_ENTER_MS` the Enter
/// is written only if `decide` still names the same tab.
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
    if let SendDecision::Ok(t) = &said {
        let id = t.id.clone();
        // Words that never reached the shell get no Enter: an Enter alone would run what was already typed.
        if !host.write(&id, &paste_of(text)) {
            return SendDecision::No(format!("{} could not take the words: send again", t.label));
        }
        host.later(
            SEND_ENTER_MS,
            Box::new(move || {
                if let SendDecision::Ok(still) = decide() {
                    if still.id == id {
                        let _ = host.write(&id, "\r");
                    }
                }
            }),
        );
    }
    said
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
        agent: Option<String>,
        /// The window could not take a write (busy): nothing is written.
        refuse: bool,
        written: RefCell<Vec<String>>,
        delays: RefCell<Vec<u64>>,
    }

    impl SendHost for Fake {
        fn agent_under(&self, _id: &str) -> Option<String> {
            self.agent.clone()
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
            SendDecision::Ok(tab())
        );
        assert_eq!(
            *host.written.borrow(),
            vec!["\x1b[200~hello\x1b[201~".to_string()]
        );
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
