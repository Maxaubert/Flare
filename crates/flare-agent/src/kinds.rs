//! AgentKind, DetectedAgent, is_resumable, CODEX_RESUME (Task 12).

/// An agent the restore can resume (the list M6 uses; widening it is a decision with a test).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Claude,
    Codex,
}

/// The agents a tab can resume after a restart.
pub const RESUMABLE_AGENTS: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];

/// The startup command that resumes the most recent Codex session.
pub const CODEX_RESUME: &str = "codex:last";

impl AgentKind {
    /// The name written to `tabs.json`.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
        }
    }

    /// The kind named by `s`, if it is one of the resumable agents.
    pub fn parse(s: &str) -> Option<AgentKind> {
        match s {
            "claude" => Some(AgentKind::Claude),
            "codex" => Some(AgentKind::Codex),
            _ => None,
        }
    }
}

/// Whether `s` names an agent the restore can resume.
pub fn is_resumable(s: &str) -> bool {
    AgentKind::parse(s).is_some()
}

/// An agent found in a tab's process tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DetectedAgent {
    Claude,
    Codex,
    Other,
}

impl From<AgentKind> for DetectedAgent {
    fn from(k: AgentKind) -> Self {
        match k {
            AgentKind::Claude => DetectedAgent::Claude,
            AgentKind::Codex => DetectedAgent::Codex,
        }
    }
}

impl DetectedAgent {
    /// Claude and Codex are resumable; Other is not.
    pub fn resumable(self) -> Option<AgentKind> {
        match self {
            DetectedAgent::Claude => Some(AgentKind::Claude),
            DetectedAgent::Codex => Some(AgentKind::Codex),
            DetectedAgent::Other => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_from_kind_and_back() {
        assert_eq!(
            DetectedAgent::from(AgentKind::Claude),
            DetectedAgent::Claude
        );
        assert_eq!(DetectedAgent::from(AgentKind::Codex), DetectedAgent::Codex);
        assert_eq!(DetectedAgent::Claude.resumable(), Some(AgentKind::Claude));
        assert_eq!(DetectedAgent::Codex.resumable(), Some(AgentKind::Codex));
        assert_eq!(DetectedAgent::Other.resumable(), None);
        for k in RESUMABLE_AGENTS {
            assert_eq!(DetectedAgent::from(k).resumable(), Some(k));
        }
    }

    #[test]
    fn is_resumable_says_yes_to_the_list_and_no_to_the_rest() {
        for a in RESUMABLE_AGENTS {
            assert!(is_resumable(a.as_str()));
        }
        for s in ["other", "aider", ""] {
            assert!(!is_resumable(s), "{s:?} must not be resumable");
        }
        assert_eq!(AgentKind::parse("claude"), Some(AgentKind::Claude));
        assert_eq!(AgentKind::parse("codex"), Some(AgentKind::Codex));
        assert_eq!(AgentKind::parse("aider"), None);
        assert_eq!(CODEX_RESUME, "codex:last");
    }
}
