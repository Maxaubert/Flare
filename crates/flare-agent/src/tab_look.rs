//! Every tab's state and mark, and its tooltip (Task 13). Ported from Prism Terminal's
//! `useTabLooks` (`tabLooks.ts`), `tabTitle` (`tabList.ts`) and the agent switches.

use std::collections::HashSet;

use crate::tab_mark::{Indicator, MarkState, TabMark, resolve_tab_mark};

/// The user's switches for the attention marks, and the indicator style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switches {
    pub indicator: Indicator,
    pub done_on: bool,
    pub question_on: bool,
    pub failed_on: bool,
    pub rainbow: bool,
}

/// The id sets the indicator keeps: which tabs hold an agent, and each state's tabs.
pub struct TabSets<'a> {
    pub agents: &'a HashSet<String>,
    pub working: &'a HashSet<String>,
    pub question: &'a HashSet<String>,
    pub failed: &'a HashSet<String>,
    pub done: &'a HashSet<String>,
}

/// One tab's agent state and the mark it wears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabLook {
    pub failed: bool,
    pub state: Option<MarkState>,
    pub mark: TabMark,
}

/// A tab's state and mark. Working outranks question, question outranks failed, failed outranks
/// done. Working needs an agent in the tab; the other three each sit behind their own switch.
pub fn tab_look(id: &str, sets: &TabSets<'_>, sw: &Switches) -> TabLook {
    let working = sets.agents.contains(id) && sets.working.contains(id);
    let question = sw.question_on && !working && sets.question.contains(id);
    let failed = sw.failed_on && !working && !question && sets.failed.contains(id);
    let done = sw.done_on && !working && !question && !failed && sets.done.contains(id);
    let state = if working {
        Some(MarkState::Working)
    } else if question {
        Some(MarkState::Question)
    } else if failed {
        Some(MarkState::Failed)
    } else if done {
        Some(MarkState::Done)
    } else {
        None
    };
    TabLook {
        failed,
        state,
        mark: resolve_tab_mark(sw.indicator, state, sw.rainbow),
    }
}

/// The plain words for a stop-failure kind, as Prism Terminal's `failedLabel` has them. An
/// unknown kind shows its own words with the underscores as spaces.
fn failed_words(kind: &str) -> String {
    let known = match kind {
        "rate_limit" => "rate limit",
        "overloaded" => "servers overloaded",
        "authentication_failed" => "sign-in failed",
        "oauth_org_not_allowed" => "organisation not allowed",
        "account_on_hold" => "account on hold",
        "billing_error" => "billing",
        "invalid_request" => "invalid request",
        "model_not_found" => "model not found",
        "server_error" => "server error",
        "max_output_tokens" => "reply too long",
        "cloud_credential_error" => "cloud credentials",
        "unknown" => "unknown error",
        other => return other.replace('_', " "),
    };
    known.to_string()
}

/// A tab's tooltip: its whole path, and while the Failed line shows, what failed under it.
/// Settings has none.
pub fn tab_title(cwd: &str, settings: bool, failed: bool, kind: Option<&str>) -> Option<String> {
    if settings {
        return None;
    }
    if !failed {
        return Some(cwd.to_string());
    }
    let line = match kind {
        Some(k) if !k.is_empty() => format!("Failed: {}", failed_words(k)),
        _ => "Failed".to_string(),
    };
    Some(format!("{cwd}\n{line}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn switches() -> Switches {
        Switches {
            indicator: Indicator::Bar,
            done_on: true,
            question_on: true,
            failed_on: true,
            rainbow: true,
        }
    }

    #[test]
    fn working_outranks_question_outranks_failed_outranks_done() {
        let agents = set(&["t"]);
        let working = set(&["t"]);
        let question = set(&["t"]);
        let failed = set(&["t"]);
        let done = set(&["t"]);
        let sw = switches();
        let state =
            |working: &HashSet<String>, question: &HashSet<String>, failed: &HashSet<String>| {
                let sets = TabSets {
                    agents: &agents,
                    working,
                    question,
                    failed,
                    done: &done,
                };
                tab_look("t", &sets, &sw).state
            };
        let empty = HashSet::new();
        assert_eq!(
            state(&working, &question, &failed),
            Some(MarkState::Working)
        );
        assert_eq!(state(&empty, &question, &failed), Some(MarkState::Question));
        assert_eq!(state(&empty, &empty, &failed), Some(MarkState::Failed));
        assert_eq!(state(&empty, &empty, &empty), Some(MarkState::Done));
    }

    #[test]
    fn an_id_not_in_agents_is_never_working() {
        let agents = HashSet::new();
        let working = set(&["t"]);
        let empty = HashSet::new();
        let sets = TabSets {
            agents: &agents,
            working: &working,
            question: &empty,
            failed: &empty,
            done: &empty,
        };
        assert_eq!(tab_look("t", &sets, &switches()).state, None);
    }

    #[test]
    fn each_attention_mark_is_behind_its_own_switch() {
        let agents = HashSet::new();
        let empty = HashSet::new();
        let question = set(&["q"]);
        let failed = set(&["f"]);
        let done = set(&["f"]);
        let sets = TabSets {
            agents: &agents,
            working: &empty,
            question: &question,
            failed: &failed,
            done: &done,
        };
        let mut sw = switches();
        // A question-only tab has no state when the question switch is off.
        sw.question_on = false;
        assert_eq!(tab_look("q", &sets, &sw).state, None);
        // Failed hidden: the tab falls to Done when it is also in the done set.
        sw.failed_on = false;
        assert_eq!(tab_look("f", &sets, &sw).state, Some(MarkState::Done));
        assert!(!tab_look("f", &sets, &sw).failed);
        // Done off: no Done at all.
        sw.done_on = false;
        assert_eq!(tab_look("f", &sets, &sw).state, None);
        // With the question switch on again the question shows.
        sw.question_on = true;
        assert_eq!(tab_look("q", &sets, &sw).state, Some(MarkState::Question));
    }

    #[test]
    fn the_failed_flag_is_set_only_for_a_failed_tab() {
        let agents = HashSet::new();
        let empty = HashSet::new();
        let failed = set(&["f"]);
        let sets = TabSets {
            agents: &agents,
            working: &empty,
            question: &empty,
            failed: &failed,
            done: &empty,
        };
        let look = tab_look("f", &sets, &switches());
        assert!(look.failed);
        assert_eq!(look.state, Some(MarkState::Failed));
        assert_eq!(
            look.mark,
            resolve_tab_mark(Indicator::Bar, Some(MarkState::Failed), true)
        );
    }

    #[test]
    fn the_tooltip_is_the_path_and_the_failure_under_it() {
        assert_eq!(
            tab_title("C:\\a", false, false, None),
            Some("C:\\a".to_string())
        );
        assert_eq!(
            tab_title("C:\\a", false, true, Some("rate_limit")),
            Some("C:\\a\nFailed: rate limit".to_string())
        );
        assert_eq!(
            tab_title("C:\\a", false, true, None),
            Some("C:\\a\nFailed".to_string())
        );
        assert_eq!(
            tab_title("C:\\a", false, true, Some("brand_new_error")),
            Some("C:\\a\nFailed: brand new error".to_string())
        );
        assert_eq!(tab_title("C:\\a", true, true, Some("rate_limit")), None);
        assert_eq!(tab_title("C:\\a", true, false, None), None);
    }
}
