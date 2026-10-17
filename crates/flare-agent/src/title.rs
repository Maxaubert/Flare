//! What the terminal title says about the agent: the title reader (Task 4).

use crate::kinds::AgentKind;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

const HALF: [char; 4] = ['◐', '◑', '◒', '◓'];
const IDLE_GLYPH: char = '✳';

fn is_braille(c: char) -> bool {
    ('\u{2800}'..='\u{28ff}').contains(&c)
}

/// Codex asking (#131): `[ ! ] Action Required` or its `[ . ]` frame.
fn codex_asks(t: &str) -> bool {
    static CODEX_ASKS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\[\s*[!.]\s*\]\s*Action Required\b").expect("static pattern")
    });
    CODEX_ASKS.is_match(t)
}

/// The state a title says an agent is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTitleState {
    Working,
    Idle,
    Starting,
    Question,
}

impl AgentTitleState {
    /// The word used in tests and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentTitleState::Working => "working",
            AgentTitleState::Idle => "idle",
            AgentTitleState::Starting => "starting",
            AgentTitleState::Question => "question",
        }
    }
}

/// One title change read as an agent state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentTitle {
    pub kind: AgentKind,
    pub state: AgentTitleState,
}

/// The agent a title names on its own, with no memory: Claude's half-circle spinner or
/// idle glyph, a braille spinner, or Codex asking. A bare folder name needs the memory.
pub fn title_agent(title: &str) -> Option<AgentKind> {
    let t = title.trim();
    let first = t.chars().next()?;
    if codex_asks(t) {
        return Some(AgentKind::Codex);
    }
    if HALF.contains(&first) || first == IDLE_GLYPH {
        return Some(AgentKind::Claude);
    }
    if is_braille(first) {
        return Some(AgentKind::Codex);
    }
    None
}

/// `title_agent` less a bare braille spinner (any CLI draws one): Codex only through
/// "Action Required". A braille title alone must not arm a shell's keys.
pub fn title_arms_agent(title: &str) -> Option<AgentKind> {
    let named = title_agent(title);
    if named == Some(AgentKind::Codex) && !codex_asks(title.trim()) {
        None
    } else {
        named
    }
}

/// Per-session memory of the title reader.
#[derive(Default)]
pub struct TitleReader {
    seen: HashMap<String, Seen>,
}

#[derive(Clone, Debug)]
struct Seen {
    kind: AgentKind,
    name: String,
    ready: bool,
}

impl TitleReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// One title change for a session. None when it says nothing about an agent.
    pub fn read(&mut self, id: &str, title: &str) -> Option<AgentTitle> {
        let t = title.trim();
        let first = t.chars().next()?;
        let rest = t[first.len_utf8()..].trim().to_string();
        let named = title_agent(t);
        let prev = self.seen.get(id).cloned();

        // Asking is past startup; the name it returns to at rest is kept.
        if named == Some(AgentKind::Codex) && codex_asks(t) {
            let name = prev.map(|s| s.name).unwrap_or_default();
            self.seen.insert(
                id.to_string(),
                Seen {
                    kind: AgentKind::Codex,
                    name,
                    ready: true,
                },
            );
            return Some(AgentTitle {
                kind: AgentKind::Codex,
                state: AgentTitleState::Question,
            });
        }
        // A spinner: working once an idle was seen for this agent, else its startup.
        if let Some(kind) = named.filter(|_| first != IDLE_GLYPH) {
            let ready = matches!(&prev, Some(p) if p.kind == kind && p.ready);
            self.seen.insert(
                id.to_string(),
                Seen {
                    kind,
                    name: rest,
                    ready,
                },
            );
            let state = if ready {
                AgentTitleState::Working
            } else {
                AgentTitleState::Starting
            };
            return Some(AgentTitle { kind, state });
        }
        // Claude's idle glyph.
        if named == Some(AgentKind::Claude) {
            let name = match &prev {
                Some(p) if p.kind == AgentKind::Claude => p.name.clone(),
                _ => rest,
            };
            self.seen.insert(
                id.to_string(),
                Seen {
                    kind: AgentKind::Claude,
                    name,
                    ready: true,
                },
            );
            return Some(AgentTitle {
                kind: AgentKind::Claude,
                state: AgentTitleState::Idle,
            });
        }
        // The bare folder name Codex shows at rest, once its name is known.
        if let Some(p) = self
            .seen
            .get_mut(id)
            .filter(|p| !p.name.is_empty() && t == p.name)
        {
            p.ready = true;
            return Some(AgentTitle {
                kind: p.kind,
                state: AgentTitleState::Idle,
            });
        }
        None
    }

    /// The session ended, or its agent left.
    pub fn forget(&mut self, id: &str) {
        self.seen.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(r: &mut TitleReader, t: &str) -> Option<String> {
        r.read("s", t)
            .map(|a| format!("{}:{}", a.kind.as_str(), a.state.as_str()))
    }

    fn check(r: &mut TitleReader, rows: &[(&str, Option<&str>)]) {
        for (t, want) in rows {
            assert_eq!(label(r, t).as_deref(), *want, "title {t:?}");
        }
    }

    #[test]
    fn claude_idle_at_birth_working_from_the_spinner_idle_again() {
        let mut r = TitleReader::new();
        check(
            &mut r,
            &[
                ("claude", None),
                ("✳ Claude Code", Some("claude:idle")),
                ("◐ Claude Code", Some("claude:working")),
                ("◑ Create and read note.txt", Some("claude:working")),
                ("✳ Create and read note.txt", Some("claude:idle")),
            ],
        );
    }

    #[test]
    fn a_spinner_before_any_idle_is_starting_not_working() {
        let mut r = TitleReader::new();
        check(
            &mut r,
            &[
                ("◐ Claude Code", Some("claude:starting")),
                ("✳ Claude Code", Some("claude:idle")),
                ("◐ Claude Code", Some("claude:working")),
            ],
        );
    }

    #[test]
    fn codex_spins_through_startup_is_ready_at_the_bare_name_then_works_and_rests() {
        let mut r = TitleReader::new();
        check(
            &mut r,
            &[
                ("yeah", None),
                ("⠙ yeah", Some("codex:starting")),
                ("npm exec @upstash/context7-mcp", None),
                ("⠼ yeah", Some("codex:starting")),
                ("yeah", Some("codex:idle")),
                ("⠴ yeah", Some("codex:working")),
                ("C:\\WINDOWS\\system32\\cmd.exe ", None),
                ("⠦ yeah", Some("codex:working")),
                ("yeah", Some("codex:idle")),
            ],
        );
    }

    #[test]
    fn action_required_is_waiting_on_you_in_both_frames() {
        let mut r = TitleReader::new();
        check(
            &mut r,
            &[
                ("⠙ proj", Some("codex:starting")),
                ("proj", Some("codex:idle")),
                ("⠴ proj", Some("codex:working")),
                ("[ ! ] Action Required | proj", Some("codex:question")),
                ("[ . ] Action Required | proj", Some("codex:question")),
                ("⠦ proj", Some("codex:working")),
                ("proj", Some("codex:idle")),
            ],
        );
    }

    #[test]
    fn codex_asking_needs_no_history() {
        let mut r = TitleReader::new();
        check(
            &mut r,
            &[("[ ! ] Action Required | proj", Some("codex:question"))],
        );
    }

    #[test]
    fn a_title_that_only_mentions_action_required_is_nothing() {
        let mut r = TitleReader::new();
        check(
            &mut r,
            &[
                ("Action Required", None),
                ("notes [ ! ] Action Required", None),
            ],
        );
    }

    #[test]
    fn shell_command_path_and_empty_titles_are_none() {
        let mut r = TitleReader::new();
        check(
            &mut r,
            &[
                ("C:\\Program Files\\PowerShell\\7\\pwsh.exe", None),
                ("", None),
                ("* not the glyph", None),
            ],
        );
    }

    #[test]
    fn forgetting_a_session_starts_it_over() {
        let mut r = TitleReader::new();
        check(&mut r, &[("✳ Claude Code", Some("claude:idle"))]);
        r.forget("s");
        check(&mut r, &[("◐ Claude Code", Some("claude:starting"))]);
    }

    #[test]
    fn title_agent_names_claude_codex_and_nothing_else() {
        assert_eq!(title_agent("◐ Claude Code"), Some(AgentKind::Claude));
        assert_eq!(
            title_agent("◓ Create and read note.txt"),
            Some(AgentKind::Claude)
        );
        assert_eq!(title_agent("✳ Claude Code"), Some(AgentKind::Claude));
        assert_eq!(title_agent("⠙ yeah"), Some(AgentKind::Codex));
        assert_eq!(
            title_agent("[ ! ] Action Required | proj"),
            Some(AgentKind::Codex)
        );
        assert_eq!(title_agent("PS C:\\x"), None);
        assert_eq!(
            title_agent("C:\\Program Files\\PowerShell\\7\\pwsh.exe"),
            None
        );
        assert_eq!(title_agent("yeah"), None);
        assert_eq!(title_agent(""), None);
        // title_agent holds no memory: a reader is unchanged by it.
        let mut r = TitleReader::new();
        let _ = title_agent("◐ Claude Code");
        assert_eq!(
            label(&mut r, "◐ Claude Code").as_deref(),
            Some("claude:starting")
        );
    }

    #[test]
    fn title_arms_on_claude_glyphs_and_action_required_never_a_bare_spinner() {
        assert_eq!(title_arms_agent("✳ Claude Code"), Some(AgentKind::Claude));
        assert_eq!(title_arms_agent("◐ Claude Code"), Some(AgentKind::Claude));
        assert_eq!(
            title_arms_agent("[ ! ] Action Required | proj"),
            Some(AgentKind::Codex)
        );
        assert_eq!(title_arms_agent("⠙ npm install"), None);
        assert_eq!(title_arms_agent("PS C:\\x"), None);
    }
}
