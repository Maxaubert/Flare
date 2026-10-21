//! CloseFlow: the close question state, and the close question's body (Task 12).

use crate::modal::escape_markdown;
use flare_agent::clock::human_for;
use flare_agent::close::agent_name;
use flare_agent::kinds::DetectedAgent;

/// The ink of the names in the close question (`Theme.text` of pt-default; M4 passes the theme's own).
pub const NAME_INK: &str = "#dedacf";

/// Whether a close goes ahead now or the question is asked first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseVerdict {
    Ask,
    Close,
}

/// What a go-ahead finishes: an interrupted quit, or a window close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmed {
    Quit,
    Close,
}

/// The window's close question as state. Every route out of the window ends at `on_close`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloseFlow {
    quitting: bool,
    quit_wanted: bool,
    agent_busy: bool,
    close_agreed: bool,
}

impl CloseFlow {
    pub fn new() -> Self {
        Self::default()
    }

    /// The UI's word: an agent is working (or no longer is).
    pub fn set_busy(&mut self, busy: bool) {
        self.agent_busy = busy;
    }

    /// A quit is under way.
    pub fn quit_started(&mut self) {
        self.quitting = true;
    }

    /// The window is closing: a working agent with no go-ahead vetoes it and asks.
    pub fn on_close(&mut self) -> CloseVerdict {
        if self.agent_busy && !self.close_agreed {
            // The veto cancels a quit too: remember it, so the go-ahead finishes it.
            self.quit_wanted = self.quitting;
            self.quitting = false;
            return CloseVerdict::Ask;
        }
        self.close_agreed = false;
        self.agent_busy = false;
        CloseVerdict::Close
    }

    /// The go-ahead: finishes the quit the veto interrupted, or closes the window.
    pub fn confirm(&mut self) -> Confirmed {
        self.close_agreed = true;
        if self.quit_wanted {
            self.quit_wanted = false;
            return Confirmed::Quit;
        }
        Confirmed::Close
    }

    /// Answered in advance (an update install): the next close goes through.
    pub fn agree(&mut self) {
        self.close_agreed = true;
    }
}

/// Wraps `s` in the name ink, as styled-text markup.
fn named(ink: &str, s: &str) -> String {
    format!("<font color=\"{ink}\">{s}</font>")
}

/// The close question's body (styled-text markdown). `for_ms` None: the agent is only present.
pub fn close_question_body(
    agent: DetectedAgent,
    tab_label: &str,
    for_ms: Option<u64>,
    others: usize,
    install: bool,
    ink: &str,
) -> String {
    let who = named(ink, agent_name(agent));
    let label = named(ink, &escape_markdown(tab_label));
    match for_ms {
        None => format!(
            "{who} is running in {label}. Closing the tab kills the shell, and the session with it."
        ),
        Some(ms) => {
            let mut body = format!("{who} has been working for {} in {label}", human_for(ms));
            if others > 0 {
                let tabs = if others == 1 { "tab is" } else { "tabs are" };
                body.push_str(&format!(", and {others} other {tabs} working too"));
            }
            body.push_str(". ");
            body.push_str(if install {
                "Installing restarts the app, which kills the shell, and the answer with it."
            } else {
                "Closing kills the shell, and the answer with it."
            });
            body
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lets_the_window_go_when_no_agent_works() {
        let mut f = CloseFlow::new();
        assert_eq!(f.on_close(), CloseVerdict::Close);
    }

    #[test]
    fn holds_while_one_works_and_the_go_ahead_closes() {
        let mut f = CloseFlow::new();
        f.set_busy(true);
        assert_eq!(f.on_close(), CloseVerdict::Ask);
        assert_eq!(f.confirm(), Confirmed::Close);
        assert_eq!(f.on_close(), CloseVerdict::Close);
    }

    #[test]
    fn cancel_keeps_holding() {
        let mut f = CloseFlow::new();
        f.set_busy(true);
        assert_eq!(f.on_close(), CloseVerdict::Ask);
        assert_eq!(f.on_close(), CloseVerdict::Ask);
    }

    #[test]
    fn an_interrupted_quit_is_finished_by_the_go_ahead() {
        let mut f = CloseFlow::new();
        f.set_busy(true);
        f.quit_started();
        assert_eq!(f.on_close(), CloseVerdict::Ask);
        assert_eq!(f.confirm(), Confirmed::Quit);
        assert_eq!(f.on_close(), CloseVerdict::Close);
    }

    #[test]
    fn after_cancel_a_later_go_ahead_is_a_close() {
        let mut f = CloseFlow::new();
        f.set_busy(true);
        f.quit_started();
        assert_eq!(f.on_close(), CloseVerdict::Ask);
        f.set_busy(true);
        assert_eq!(f.on_close(), CloseVerdict::Ask);
        assert_eq!(f.confirm(), Confirmed::Close);
    }

    #[test]
    fn an_install_answered_in_advance_goes_through_once() {
        let mut f = CloseFlow::new();
        f.set_busy(true);
        f.agree();
        assert_eq!(f.on_close(), CloseVerdict::Close);
        f.set_busy(true);
        assert_eq!(f.on_close(), CloseVerdict::Ask);
    }

    #[test]
    fn bodies_name_the_work() {
        let n = |s: &str| format!("<font color=\"{NAME_INK}\">{s}</font>");
        let body = |for_ms, others, install| {
            close_question_body(
                DetectedAgent::Claude,
                "api",
                for_ms,
                others,
                install,
                NAME_INK,
            )
        };
        assert_eq!(
            body(None, 0, false),
            format!(
                "{} is running in {}. Closing the tab kills the shell, and the session with it.",
                n("Claude"),
                n("api")
            )
        );
        assert_eq!(
            body(Some(180_000), 0, false),
            format!(
                "{} has been working for 3 minutes in {}. Closing kills the shell, and the answer with it.",
                n("Claude"),
                n("api")
            )
        );
        let one = body(Some(180_000), 1, false);
        assert!(one.contains(&format!(
            "in {}, and 1 other tab is working too. Closing kills",
            n("api")
        )));
        let two = body(Some(180_000), 2, false);
        assert!(two.contains("2 other tabs are working too"));
        let install = body(Some(180_000), 0, true);
        assert!(install.ends_with(
            "Installing restarts the app, which kills the shell, and the answer with it."
        ));
        let other = close_question_body(DetectedAgent::Other, "api", None, 0, false, NAME_INK);
        assert!(other.starts_with(&n("The agent")));
        let escaped =
            close_question_body(DetectedAgent::Claude, "my_app", None, 0, false, NAME_INK);
        assert!(escaped.contains(&n("my\\_app")));
        let white = close_question_body(DetectedAgent::Claude, "api", None, 0, false, "#ffffff");
        assert!(white.contains("<font color=\"#ffffff\">"));
    }
}
