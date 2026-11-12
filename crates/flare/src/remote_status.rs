//! The viewer's side of a remote tab's status (remote sessions PR 4, rule 294).
//!
//! A remote tab's processes and screen are the host's: the viewer's poll is blind to them and its screen is not read
//! for a question (rule 289). So the host says what its own marks say, as words: `remote.status` answers every host
//! tab with one state word (`working`, `question`, `done`, `failed`, `idle`), the agent, its Claude session and the
//! words of its question. The tab's link polls it every second or two ([`StatusPoll`]) and hands a change to the
//! window, which gives it to the hub (`AgentHub::on_remote_status`).
//!
//! What the hub makes of it ([`mark_of`]): a question and work are shown as the host says them; a finished turn or
//! a failure is never copied. When the host stops working, the viewer raises its own Finished or Failed mark only if
//! it showed the tab working and nobody here was looking, and its own looking clears it (rule 61). So a status read
//! again after a reconnect never raises a mark a second time.

use std::time::{Duration, Instant};

use flare_agent::indicator::RemoteMark;
use flare_ctl::remote::TabState;
use flare_term::remote_pty::HostStatus;
use serde_json::Value;

/// Pure: the mark a host status gives the viewer's hub; None for a word this end does not know (a newer host's word is
/// no news, rule 40).
pub fn mark_of(status: &HostStatus) -> Option<RemoteMark> {
    Some(match TabState::parse(&status.state)? {
        TabState::Working => RemoteMark::Working,
        TabState::Question => RemoteMark::Question,
        TabState::Done => RemoteMark::Finished,
        TabState::Failed => RemoteMark::Failed {
            kind: status.failed.clone(),
        },
        TabState::Idle => RemoteMark::Idle,
    })
}

/// Pure: the status of host tab `tab` in a `remote.status` answer. None when the answer does not name it (the tab
/// closed on the host; the output poll hears `no_tab`).
pub fn status_of(answer: &Value, tab: &str) -> Option<HostStatus> {
    let t = answer["tabs"]
        .as_array()?
        .iter()
        .find(|t| t["tab"].as_str() == Some(tab))?;
    let word = |k: &str| t[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    Some(HostStatus {
        state: word("state")?,
        agent: word("kind"),
        session: word("session").filter(|s| flare_agent::signal::is_session_id(s)),
        question: word("question_text"),
        failed: word("failed"),
    })
}

/// One remote tab's `remote.status` polls on its link worker: when the next is due, and the last status handed on.
pub struct StatusPoll {
    every: Duration,
    next: Option<Instant>,
    last: Option<HostStatus>,
    /// The host has no `remote.status` (an older Flare): not asked again on this link.
    off: bool,
}

impl StatusPoll {
    pub fn new(every: Duration) -> Self {
        Self {
            every,
            next: None,
            last: None,
            off: false,
        }
    }

    /// The tab attached (again): ask at once, and the first answer is news even if it says what the last one did.
    pub fn attached(&mut self) {
        self.next = None;
        self.last = None;
        self.off = false;
    }

    /// Whether a poll is due at `now`; when it is, the next one is `every` later.
    pub fn due(&mut self, now: Instant) -> bool {
        if self.off || self.next.is_some_and(|n| now < n) {
            return false;
        }
        self.next = Some(now + self.every);
        true
    }

    /// The host has no `remote.status`.
    pub fn off(&mut self) {
        self.off = true;
    }

    /// The tab's status in `answer` when it differs from the last one handed on.
    pub fn news(&mut self, answer: &Value, tab: &str) -> Option<HostStatus> {
        let s = status_of(answer, tab)?;
        if self.last.as_ref() == Some(&s) {
            return None;
        }
        self.last = Some(s.clone());
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SID: &str = "11111111-1111-4111-8111-111111111111";

    fn st(state: &str) -> HostStatus {
        HostStatus {
            state: state.into(),
            ..HostStatus::default()
        }
    }

    /// Rule 294: question and work are shown as the host says; done and failed are an end, never a copied mark.
    #[test]
    fn each_host_state_word_maps_to_one_mark_and_an_unknown_word_to_none() {
        assert_eq!(mark_of(&st("working")), Some(RemoteMark::Working));
        assert_eq!(mark_of(&st("question")), Some(RemoteMark::Question));
        assert_eq!(mark_of(&st("done")), Some(RemoteMark::Finished));
        let mut f = st("failed");
        f.failed = Some("rate_limit".into());
        assert_eq!(
            mark_of(&f),
            Some(RemoteMark::Failed {
                kind: Some("rate_limit".into())
            })
        );
        assert_eq!(mark_of(&st("idle")), Some(RemoteMark::Idle));
        assert_eq!(mark_of(&st("blocked")), None);
    }

    #[test]
    fn a_status_answer_is_read_for_the_one_host_tab_and_only_as_words() {
        let answer = json!({"seq": 4, "tabs": [
            {"tab": "t1", "state": "idle"},
            {"tab": "t2", "state": "question", "kind": "claude", "session": SID,
             "question_text": "Overwrite it?", "folder": "/Users/max/p", "title": "p"},
            {"tab": "t3", "state": "working", "session": "../../etc/passwd"},
        ]});
        assert_eq!(
            status_of(&answer, "t2"),
            Some(HostStatus {
                state: "question".into(),
                agent: Some("claude".into()),
                session: Some(SID.into()),
                question: Some("Overwrite it?".into()),
                failed: None,
            })
        );
        assert_eq!(
            status_of(&answer, "t3").unwrap().session,
            None,
            "a session that is not a UUID is dropped"
        );
        assert_eq!(status_of(&answer, "t9"), None);
        assert_eq!(status_of(&json!({"tabs": [{"tab": "t1"}]}), "t1"), None);
        assert_eq!(status_of(&json!({}), "t1"), None);
    }

    #[test]
    fn the_poll_is_due_every_period_and_hands_on_only_a_change() {
        let t0 = Instant::now();
        let mut p = StatusPoll::new(Duration::from_millis(1500));
        assert!(p.due(t0), "the first poll is at once");
        assert!(!p.due(t0 + Duration::from_millis(1000)));
        assert!(p.due(t0 + Duration::from_millis(1500)));
        let a = json!({"tabs": [{"tab": "h1", "state": "working"}]});
        assert!(p.news(&a, "h1").is_some());
        assert!(
            p.news(&a, "h1").is_none(),
            "the same status twice is no news"
        );
        let b = json!({"tabs": [{"tab": "h1", "state": "done"}]});
        assert_eq!(p.news(&b, "h1").map(|s| s.state), Some("done".into()));
        // A reconnect asks at once, and its first answer is news again.
        p.attached();
        assert!(p.due(t0 + Duration::from_millis(1600)));
        assert!(p.news(&b, "h1").is_some());
        // An older host has no remote.status: not asked again until the next attach.
        p.off();
        assert!(!p.due(t0 + Duration::from_secs(60)));
        p.attached();
        assert!(p.due(t0 + Duration::from_secs(60)));
    }
}
