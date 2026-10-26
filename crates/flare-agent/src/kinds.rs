//! AgentKind, DetectedAgent, is_resumable, CODEX_RESUME, AGY_RESUME (Task 12).

/// An agent the restore can resume (the list M6 uses; widening it is a decision with a test).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Claude,
    Codex,
    /// The Antigravity CLI, `agy` (owner decision 2026-10-26).
    Agy,
}

/// The agents a tab can resume after a restart.
pub const RESUMABLE_AGENTS: [AgentKind; 3] = [AgentKind::Claude, AgentKind::Codex, AgentKind::Agy];

/// The startup command that resumes the most recent Codex session.
pub const CODEX_RESUME: &str = "codex:last";

/// The startup command that resumes the most recent agy conversation in the tab's folder (`agy -c`).
pub const AGY_RESUME: &str = "agy:last";

impl AgentKind {
    /// The name written to `tabs.json`.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Agy => "agy",
        }
    }

    /// The kind named by `s`, if it is one of the resumable agents.
    pub fn parse(s: &str) -> Option<AgentKind> {
        match s {
            "claude" => Some(AgentKind::Claude),
            "codex" => Some(AgentKind::Codex),
            "agy" => Some(AgentKind::Agy),
            _ => None,
        }
    }

    /// The name people read: "Claude", "Codex", "Antigravity".
    pub fn display_name(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude",
            AgentKind::Codex => "Codex",
            AgentKind::Agy => "Antigravity",
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
    Agy,
    Other,
}

impl From<AgentKind> for DetectedAgent {
    fn from(k: AgentKind) -> Self {
        match k {
            AgentKind::Claude => DetectedAgent::Claude,
            AgentKind::Codex => DetectedAgent::Codex,
            AgentKind::Agy => DetectedAgent::Agy,
        }
    }
}

impl DetectedAgent {
    /// Claude, Codex and agy are resumable; Other is not.
    pub fn resumable(self) -> Option<AgentKind> {
        match self {
            DetectedAgent::Claude => Some(AgentKind::Claude),
            DetectedAgent::Codex => Some(AgentKind::Codex),
            DetectedAgent::Agy => Some(AgentKind::Agy),
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
        assert_eq!(DetectedAgent::from(AgentKind::Agy), DetectedAgent::Agy);
        assert_eq!(DetectedAgent::Claude.resumable(), Some(AgentKind::Claude));
        assert_eq!(DetectedAgent::Codex.resumable(), Some(AgentKind::Codex));
        assert_eq!(DetectedAgent::Agy.resumable(), Some(AgentKind::Agy));
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
        for s in ["other", "aider", "", "antigravity", "Agy"] {
            assert!(!is_resumable(s), "{s:?} must not be resumable");
        }
        assert_eq!(AgentKind::parse("claude"), Some(AgentKind::Claude));
        assert_eq!(AgentKind::parse("codex"), Some(AgentKind::Codex));
        assert_eq!(AgentKind::parse("agy"), Some(AgentKind::Agy));
        assert_eq!(AgentKind::parse("aider"), None);
        assert_eq!(CODEX_RESUME, "codex:last");
        assert_eq!(AGY_RESUME, "agy:last");
    }

    #[test]
    fn each_agent_has_the_name_people_read() {
        assert_eq!(AgentKind::Claude.display_name(), "Claude");
        assert_eq!(AgentKind::Codex.display_name(), "Codex");
        assert_eq!(AgentKind::Agy.display_name(), "Antigravity");
    }
}
