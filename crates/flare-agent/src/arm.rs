//! AgentArm: whether an agent's keys are armed in a tab (Task 4).

use crate::kinds::{AgentKind, DetectedAgent};
use crate::title::{title_agent, title_arms_agent};
use regex::Regex;
use std::sync::LazyLock;

/// Codex's title at rest follows "Action Required |" (#131).
static ASK_REST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Action Required\s*\|\s*(.*)$").expect("static pattern"));

/// Whether an agent runs in a tab, and what the tab's keys may assume about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentArm {
    pub here: bool,
    pub kind: Option<DetectedAgent>,
    /// The poll's last word was "present": an agent it saw is its to take back.
    pub polled: bool,
    /// The title vouches for it; for Codex, `rest` is its title at rest.
    pub titled: bool,
    pub rest: String,
    /// A resume's agent the poll has said "no agent" about once.
    pub doubted: bool,
}

/// One move. `gone`: an agent known to be here has left. `recheck`: ask the poll
/// for its next answer for this session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmStep {
    pub arm: AgentArm,
    pub gone: bool,
    pub recheck: bool,
}

fn bare() -> AgentArm {
    AgentArm {
        here: false,
        kind: None,
        polled: false,
        titled: false,
        rest: String::new(),
        doubted: false,
    }
}

fn step(arm: AgentArm, gone: bool, recheck: bool) -> ArmStep {
    ArmStep { arm, gone, recheck }
}

/// The arm a tab starts with: armed for a resumed agent, bare otherwise.
pub fn arm_at_start(resumed: Option<DetectedAgent>) -> AgentArm {
    match resumed {
        Some(kind) => AgentArm {
            here: true,
            kind: Some(kind),
            ..bare()
        },
        None => bare(),
    }
}

/// Whether `title` is still the title-armed agent's own.
fn still_names(s: &AgentArm, title: &str) -> bool {
    let t = title.trim();
    let named = title_agent(t);
    match s.kind {
        Some(DetectedAgent::Claude) => named == Some(AgentKind::Claude),
        // Codex: asking, busy (its braille spinner before the folder) or at rest.
        Some(DetectedAgent::Codex) => {
            named == Some(AgentKind::Codex) || (!s.rest.is_empty() && t == s.rest)
        }
        _ => false,
    }
}

/// A title change: a title that arms an agent arms it; a title that stops naming the
/// agent it armed disarms it, unless the poll has seen the agent (then the poll decides).
pub fn arm_on_title(s: &AgentArm, title: &str) -> ArmStep {
    if let Some(kind) = title_arms_agent(title) {
        let rest = if kind == AgentKind::Codex {
            ASK_REST
                .captures(title.trim())
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_else(|| s.rest.clone())
        } else {
            String::new()
        };
        return step(
            AgentArm {
                here: true,
                kind: Some(DetectedAgent::from(kind)),
                titled: true,
                rest,
                doubted: false,
                ..s.clone()
            },
            false,
            false,
        );
    }
    if !s.titled || still_names(s, title) {
        return step(s.clone(), false, false);
    }
    // The poll saw it: the poll says when it leaves, whatever the title does.
    if s.polled {
        return step(
            AgentArm {
                titled: false,
                ..s.clone()
            },
            false,
            false,
        );
    }
    step(bare(), true, true)
}

/// A poll answer. "Present" arms the agent; "absent" takes back only what the poll
/// saw itself, and a resume's agent gets one doubt before it is taken back.
pub fn arm_on_poll(s: &AgentArm, present: bool, kind: Option<DetectedAgent>) -> ArmStep {
    if present {
        // A wrapper ('other') does not undo the title's agent.
        let k = if kind == Some(DetectedAgent::Other) && s.kind.is_some() {
            s.kind
        } else {
            kind
        };
        return step(
            AgentArm {
                here: true,
                kind: k,
                polled: true,
                doubted: false,
                ..s.clone()
            },
            false,
            false,
        );
    }
    if s.polled {
        return step(bare(), true, false);
    }
    // The poll never saw the title's agent (WSL): its "no" is not about it.
    if s.titled {
        return step(s.clone(), false, false);
    }
    if s.here && !s.doubted {
        return step(
            AgentArm {
                doubted: true,
                ..s.clone()
            },
            false,
            true,
        );
    }
    step(bare(), false, false)
}

/// A shell prompt: nothing runs in front of it, so the arm is taken back at once.
pub fn arm_on_prompt(s: &AgentArm) -> ArmStep {
    // Gone even with nothing armed: a program that died without its ?2031l left
    // reports on, and a push would reach the shell.
    step(bare(), true, s.polled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::DetectedAgent::{Claude, Codex, Other};

    fn bare() -> AgentArm {
        arm_at_start(None)
    }

    fn title(s: &AgentArm, t: &str) -> ArmStep {
        arm_on_title(s, t)
    }

    #[test]
    fn starts_armed_for_a_resumed_agent_bare_otherwise() {
        let a = arm_at_start(Some(Codex));
        assert!(a.here);
        assert_eq!(a.kind, Some(Codex));
        let b = bare();
        assert!(!b.here && b.kind.is_none() && !b.polled && !b.titled && b.rest.is_empty());
    }

    #[test]
    fn is_armed_by_the_title_before_the_poll() {
        let s = title(&bare(), "✳ Claude Code").arm;
        assert!(s.here);
        assert_eq!(s.kind, Some(Claude));
    }

    #[test]
    fn is_not_armed_by_a_bare_braille_spinner() {
        assert!(!title(&bare(), "⠙ npm").arm.here);
    }

    #[test]
    fn is_disarmed_by_the_poll_left_and_that_ends_what_it_asked_for() {
        let found = arm_on_poll(&bare(), true, Some(Claude));
        assert!(found.arm.here);
        let left = arm_on_poll(&found.arm, false, None);
        assert!(!left.arm.here);
        assert!(left.gone);
    }

    #[test]
    fn keeps_the_titles_agent_when_the_poll_only_sees_a_wrapper() {
        let s = title(&bare(), "✳ Claude Code").arm;
        let p = arm_on_poll(&s, true, Some(Other)).arm;
        assert_eq!(p.kind, Some(Claude));
        assert!(p.here);
    }

    #[test]
    fn does_not_take_a_resumes_agent_back_on_the_first_no_agent() {
        let s = arm_at_start(Some(Claude));
        let first = arm_on_poll(&s, false, None);
        assert!(first.arm.here);
        assert!(!first.gone);
        assert!(first.recheck);
    }

    #[test]
    fn takes_a_resumes_agent_back_when_the_poll_says_no_agent_twice() {
        let s = arm_at_start(Some(Claude));
        let first = arm_on_poll(&s, false, None);
        let second = arm_on_poll(&first.arm, false, None);
        assert!(!second.arm.here);
        assert!(!second.gone);
        assert!(!second.recheck);
    }

    #[test]
    fn is_no_departure_on_a_first_no_agent_with_nothing_armed() {
        assert!(!arm_on_poll(&bare(), false, None).gone);
    }

    #[test]
    fn keeps_a_titles_agent_on_the_polls_no_agent() {
        let s = title(&bare(), "✳ Claude Code").arm;
        let p = arm_on_poll(&s, false, None);
        assert!(p.arm.here);
        assert!(!p.gone);
    }

    #[test]
    fn is_disarmed_when_the_title_stops_naming_a_title_armed_agent() {
        let s = title(&bare(), "◐ Claude Code").arm;
        let step = title(&s, "me@box: ~/src");
        assert!(!step.arm.here);
        assert!(step.gone);
    }

    #[test]
    fn asks_the_poll_again_when_the_title_stops_naming_an_agent_only_the_title_armed() {
        let s = title(&bare(), "◐ Claude Code").arm;
        assert!(title(&s, "").recheck);
        assert!(
            title(
                &s,
                "C:\\WINDOWS\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
            )
            .recheck
        );
        assert!(!title(&s, "◐ Claude Code").recheck);
    }

    #[test]
    fn keeps_a_title_armed_agent_through_its_own_titles() {
        let mut s = bare();
        for t in ["◐ Claude Code", "◓ Claude Code", "✳ Fix the bug"] {
            let step = title(&s, t);
            assert!(step.arm.here, "{t:?}");
            assert!(!step.gone, "{t:?}");
            s = step.arm;
        }
    }

    #[test]
    fn takes_an_empty_title_as_the_agent_gone() {
        let s = title(&bare(), "◐ Claude Code").arm;
        let step = title(&s, "");
        assert!(step.gone);
        assert!(!step.arm.here);
    }

    #[test]
    fn keeps_a_title_armed_codex_at_rest_and_busy() {
        let mut s = title(&bare(), "[ ! ] Action Required | yeah").arm;
        assert!(s.here);
        assert_eq!(s.rest, "yeah");
        for t in ["yeah", "⠙ yeah", "[ . ] Action Required | yeah", "yeah"] {
            let step = title(&s, t);
            assert!(step.arm.here, "{t:?}");
            assert!(!step.gone, "{t:?}");
            s = step.arm;
        }
        let gone = title(&s, "me@box: ~");
        assert!(!gone.arm.here);
        assert!(gone.gone);
    }

    #[test]
    fn leaves_a_polled_agent_to_the_poll_when_the_title_changes() {
        let s = title(&bare(), "✳ Claude Code").arm;
        let polled = arm_on_poll(&s, true, Some(Claude)).arm;
        let step = title(&polled, "npm");
        assert!(step.arm.here);
        assert!(!step.gone);
        assert!(!step.arm.titled);
    }

    #[test]
    fn is_disarmed_by_the_prompt_when_only_the_title_armed_it() {
        let s = title(&bare(), "✳ Claude Code").arm;
        let p = arm_on_prompt(&s);
        assert!(!p.arm.here);
        assert!(p.gone);
        assert!(!p.recheck);
    }

    #[test]
    fn is_disarmed_by_the_prompt_after_a_resumed_agent_ends() {
        let p = arm_on_prompt(&arm_at_start(Some(Codex)));
        assert!(!p.arm.here);
        assert!(p.gone);
    }

    #[test]
    fn is_disarmed_by_the_prompt_even_when_the_poll_saw_the_agent_and_asks_the_poll_again() {
        let s = arm_on_poll(&bare(), true, Some(Claude)).arm;
        let p = arm_on_prompt(&s);
        assert!(!p.arm.here);
        assert!(p.recheck);
        assert!(p.gone);
    }

    #[test]
    fn does_not_ask_the_poll_again_on_a_prompt_it_has_nothing_to_correct() {
        assert!(!arm_on_prompt(&arm_at_start(None)).recheck);
        let s = title(&bare(), "✳ Claude Code").arm;
        assert!(!arm_on_prompt(&s).recheck);
    }

    #[test]
    fn is_armed_again_when_the_poll_answers_after_the_prompt() {
        let p = arm_on_prompt(&arm_on_poll(&bare(), true, Some(Claude)).arm).arm;
        let again = arm_on_poll(&p, true, Some(Claude));
        assert!(again.arm.here);
    }
}
