//! Background work on the Claude screen (Task 6). Ported from Prism Terminal's `agentBackground.ts`.
//!
//! IS THE AGENT STILL WORKING IN THE BACKGROUND? Claude Code ends its turn (the Stop hook) while a shell
//! it started with run_in_background, or a workflow, keeps running, and nothing it sends the terminal says
//! so. Its SCREEN does (MEASURED, Claude Code 2.1.301 fullscreen):
//! - the turn's summary line ends "· 1 shell still running", and is rewritten without it when the shell ends;
//! - the footer under the prompt's box says "· 1 shell" after the mode line, gone with it;
//! - a running workflow draws its progress row in the footer ("○ name ▬▬▬ 7/8 · 11m03s").
//!
//! Only these LIVE marks count. The "✻ Waiting for 1 dynamic workflow to finish" line stays in the
//! conversation after the wait is over, so it is not read.

use regex::Regex;
use std::sync::LazyLock;

/// A rule of the prompt's box: a long run of box-drawing dashes.
static RULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*─{20,}\s*$").expect("RULE"));
/// The turn's summary line, kept current by Claude: "· 1 shell still running".
static STILL_RUNNING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)·\s*[0-9]+\s+(?:background\s+)?(?:shells?|tasks?|commands?)\s+still running\b")
        .expect("STILL_RUNNING")
});
/// The mode line's count of background shells: "auto mode on · 1 shell".
static FOOTER_COUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)·\s*[0-9]+\s+(?:shells?|tasks?)\b").expect("FOOTER_COUNT"));
/// A running workflow's progress row: "7/8 · 11m03s".
static FOOTER_WORKFLOW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[0-9]+/[0-9]+\s+·\s+(?:[0-9]+h)?(?:[0-9]+m)?[0-9]+s\b").expect("FOOTER_WORKFLOW")
});

/// True when a shell or workflow is still running under this Claude screen.
pub fn looks_like_background_work(rows: &[String]) -> bool {
    if rows.iter().any(|r| STILL_RUNNING.is_match(r)) {
        return true;
    }
    // The footer: the rows under the prompt box's lower rule.
    let Some(last) = rows.iter().rposition(|r| RULE.is_match(r)) else {
        return false;
    };
    rows[last + 1..]
        .iter()
        .any(|r| FOOTER_COUNT.is_match(r) || FOOTER_WORKFLOW.is_match(r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// A named screen from the fixture the PT source measured (`background_screens.json`).
    fn screen(name: &str) -> Vec<String> {
        let all: Value =
            serde_json::from_str(include_str!("../tests/fixtures/background_screens.json"))
                .expect("fixture parses");
        all[name]
            .as_array()
            .expect("fixture key is an array")
            .iter()
            .map(|v| v.as_str().expect("row is a string").to_string())
            .collect()
    }

    #[test]
    fn a_shell_started_in_the_background_while_it_runs() {
        assert!(looks_like_background_work(&screen("shellRunning")));
    }

    #[test]
    fn the_same_session_once_the_shell_has_ended() {
        assert!(!looks_like_background_work(&screen("shellDone")));
    }

    #[test]
    fn a_workflow_running_though_the_old_waiting_for_lines_stay_above() {
        assert!(looks_like_background_work(&screen("workflowRunning")));
    }

    #[test]
    fn the_workflow_finished_its_stale_waiting_for_line_alone_is_not_work() {
        assert!(!looks_like_background_work(&screen("workflowDone")));
    }

    #[test]
    fn a_plain_prompt_or_an_empty_screen_is_not() {
        assert!(!looks_like_background_work(&[]));
        let plain = vec![
            "PS C:\\work> npm test".to_string(),
            "  12/40 passed · 3s".to_string(),
        ];
        assert!(!looks_like_background_work(&plain));
    }
}
