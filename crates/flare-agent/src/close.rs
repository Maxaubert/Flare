//! asks_before_closing_tab, holds_window_close and the texts (Task 12).

use crate::kinds::DetectedAgent;

/// Closing this tab: ask first? A plain shell closes unasked; a tab hosting an agent asks, working or idle.
pub fn asks_before_closing_tab(agent_present: bool) -> bool {
    agent_present
}

/// Closing the window: hold it and ask? Only while an agent is mid-answer.
pub fn holds_window_close(agents_working: usize) -> bool {
    agents_working > 0
}

/// The agent's name in the close question.
pub fn agent_name(a: DetectedAgent) -> &'static str {
    match a {
        DetectedAgent::Claude => "Claude",
        DetectedAgent::Codex => "Codex",
        DetectedAgent::Agy => "Antigravity",
        DetectedAgent::Other => "The agent",
    }
}

/// What the close question is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseTarget {
    Tab,
    Window,
    Install,
}

/// The question's title. `for_ms` is how long the agent has been working, None when it is only present.
pub fn close_question_title(t: CloseTarget, for_ms: Option<u64>) -> &'static str {
    match t {
        CloseTarget::Install => "Stop the agent and install the update?",
        CloseTarget::Window => "Stop the agent and close the window?",
        CloseTarget::Tab => match for_ms {
            None => "Close the tab and end the agent?",
            Some(_) => "Stop the agent and close?",
        },
    }
}

/// The label of the button that goes ahead (the other button is `Cancel`).
pub fn close_primary_label(t: CloseTarget) -> &'static str {
    match t {
        CloseTarget::Install => "Install and restart",
        CloseTarget::Window => "Close window",
        CloseTarget::Tab => "Close tab",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_shell_closes_unasked_an_agent_asks() {
        assert!(!asks_before_closing_tab(false));
        assert!(asks_before_closing_tab(true));
    }

    #[test]
    fn the_window_is_held_only_while_an_agent_works() {
        assert!(!holds_window_close(0));
        assert!(holds_window_close(2));
    }

    #[test]
    fn titles_say_what_they_interrupt() {
        assert_eq!(
            close_question_title(CloseTarget::Tab, None),
            "Close the tab and end the agent?"
        );
        assert_eq!(
            close_question_title(CloseTarget::Tab, Some(90_000)),
            "Stop the agent and close?"
        );
        assert!(close_question_title(CloseTarget::Window, Some(90_000)).contains("window"));
        let install = close_question_title(CloseTarget::Install, Some(90_000));
        assert_eq!(install, "Stop the agent and install the update?");
        assert!(!install.contains("window"));
    }

    #[test]
    fn primary_labels() {
        assert_eq!(
            close_primary_label(CloseTarget::Install),
            "Install and restart"
        );
        assert_eq!(close_primary_label(CloseTarget::Window), "Close window");
        assert_eq!(close_primary_label(CloseTarget::Tab), "Close tab");
    }

    #[test]
    fn agent_names() {
        assert_eq!(agent_name(DetectedAgent::Claude), "Claude");
        assert_eq!(agent_name(DetectedAgent::Codex), "Codex");
        assert_eq!(agent_name(DetectedAgent::Agy), "Antigravity");
        assert_eq!(agent_name(DetectedAgent::Other), "The agent");
    }
}
