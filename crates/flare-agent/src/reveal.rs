//! When a resuming tab shows its terminal: the reveal rules.
//!
//! A tab coming back to Claude or Codex wears a skeleton of the agent's layout until the agent has
//! drawn itself, and the shell's own words (its prompt, the resume command) are cleared before the
//! agent draws a cell, so they are never seen. MEASURED 2026-09-30 (Prism Terminal): pwsh starting
//! `claude` as its startup command through the bundled ConPTY: the first console title is exactly
//! `claude` (3.6 s), then Claude's own `✳ Claude Code` (5.4 s), and the switch to the alternate
//! screen 35 ms after it.
//!
//! So: CLEAR just before the title that names the agent; REVEAL once the agent has painted (the
//! next title that differs from its bare name, or a switch to the alternate screen), each given a
//! moment to land; or at once on a key, an exit or a failed spawn; or at a ceiling. Pure: the glue
//! feeds it and keeps the clock.

use crate::kinds::AgentKind;
use regex::bytes::Regex;
use std::sync::OnceLock;

/// A paint is given this long to land before the skeleton lifts.
pub const SETTLE_MS: u64 = 150;
/// Past this, the terminal is shown whatever it holds.
pub const CEILING_MS: u64 = 12_000;
/// Written to the GRID only (never the shell): screen, scrollback, home.
pub const CLEAR: &[u8] = b"\x1b[2J\x1b[3J\x1b[H";

/// Where a resuming tab stands in its reveal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealState {
    pub agent: AgentKind,
    pub started_at: u64,
    /// The shell's words are gone: the clear has been written.
    pub cleared: bool,
    /// When the agent's paint counts as landed, once it has begun.
    pub settle_at: Option<u64>,
    pub revealed: bool,
}

/// The agent a resume id is for: `codex...` is Codex, anything else Claude.
pub fn agent_of_resume(resume: &str) -> AgentKind {
    if resume.starts_with("codex") {
        AgentKind::Codex
    } else {
        AgentKind::Claude
    }
}

/// A fresh reveal for `agent`, started at `now`.
pub fn start_reveal(agent: AgentKind, now: u64) -> RevealState {
    RevealState {
        agent,
        started_at: now,
        cleared: false,
        settle_at: None,
        revealed: false,
    }
}

/// A title names the agent's PROGRAM: a bare name (`claude`, `claude.exe`, case-insensitive) or a
/// path ending in `<agent>.exe`. A folder called Claude, a command line that mentions it, or
/// `✳ Claude Code` is not.
pub fn names_agent(title: &str, agent: AgentKind) -> bool {
    let t = title.trim().to_lowercase();
    let exe = format!("{}.exe", agent.as_str());
    if !t.contains(['\\', '/']) {
        return t == agent.as_str() || t == exe;
    }
    t.rsplit(['\\', '/']).next() == Some(exe.as_str())
}

/// The title the agent itself sets once it runs (Claude Code's `<mark> Claude Code`), which is not
/// its program name. Only Claude Code has one that is certain.
fn is_agent_title(title: &str, agent: AgentKind) -> bool {
    agent == AgentKind::Claude && title.to_lowercase().contains("claude code")
}

/// The OSC 0 or 2 title scanner: ended by BEL or ST, matched on raw bytes so a split UTF-8
/// sequence before it cannot move the offset.
fn title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?-u)\x1b\](?:0|2);([^\x07\x1b]*)(?:\x07|\x1b\\)")
            .expect("the title pattern is a fixed, valid regex")
    })
}

/// A chunk of pty output arrives. Returns the new state and where to write the clear, as a BYTE
/// offset into `data` (the ESC of the title that names the agent), once.
pub fn on_chunk(s: &RevealState, data: &[u8], now: u64) -> (RevealState, Option<usize>) {
    if s.revealed {
        return (s.clone(), None);
    }
    let mut state = s.clone();
    let mut clear_at = None;
    for caps in title_re().captures_iter(data) {
        let Some(whole) = caps.get(0) else { continue };
        let title = caps.get(1).map_or_else(String::new, |m| {
            String::from_utf8_lossy(m.as_bytes()).into_owned()
        });
        if !state.cleared {
            if names_agent(&title, state.agent) {
                clear_at = Some(whole.start());
                state.cleared = true;
            } else if is_agent_title(&title, state.agent) {
                // The bare name was never seen whole (a title cut across two reads of the pty): the
                // agent's own title is the next best mark. It comes before the agent's first
                // paint, so the shell's words are cleared in time, and the agent is drawing now.
                clear_at = Some(whole.start());
                state.cleared = true;
                state.settle_at = Some(now + SETTLE_MS);
            }
            continue;
        }
        // After the bare name, the agent's own title: it is drawing.
        if state.settle_at.is_none() && !names_agent(&title, state.agent) {
            state.settle_at = Some(now + SETTLE_MS);
        }
    }
    (state, clear_at)
}

/// A full-screen agent draws on the alternate screen: the settle starts now, unless revealed or
/// already set.
pub fn on_alternate(s: &RevealState, now: u64) -> RevealState {
    if s.revealed || s.settle_at.is_some() {
        return s.clone();
    }
    RevealState {
        settle_at: Some(now + SETTLE_MS),
        ..s.clone()
    }
}

/// Show the terminal now: a key, an exit, a failed spawn.
pub fn reveal_now(s: &RevealState) -> RevealState {
    RevealState {
        revealed: true,
        ..s.clone()
    }
}

/// The clock: revealed when the settle has passed, or at the ceiling.
pub fn on_tick(s: &RevealState, now: u64) -> RevealState {
    if s.revealed {
        return s.clone();
    }
    let settled = s.settle_at.is_some_and(|at| now >= at);
    let ceiling = now.saturating_sub(s.started_at) >= CEILING_MS;
    if settled || ceiling {
        reveal_now(s)
    } else {
        s.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An OSC 0 or 2 title, ended by BEL (`st` false) or ST (`st` true).
    fn title(t: &str, st: bool) -> Vec<u8> {
        let mut v = format!("\x1b]0;{t}").into_bytes();
        v.extend_from_slice(if st { b"\x1b\\" } else { b"\x07" });
        v
    }

    fn cleared_at_3600() -> RevealState {
        let start = start_reveal(AgentKind::Claude, 0);
        let mut chunk = b"PS C:\\work> \x1b[1t\x1b[c\x1b[?1004h".to_vec();
        chunk.extend(title("claude", true));
        chunk.extend_from_slice(b"\x1b[?25l");
        let (s, _) = on_chunk(&start, &chunk, 3600);
        assert!(s.cleared);
        s
    }

    #[test]
    fn a_bare_name_cut_across_two_reads_still_clears_and_settles_on_the_agents_title() {
        let start = start_reveal(AgentKind::Claude, 0);
        // The bare name is split by the batching: no read holds it whole.
        let (s, at) = on_chunk(&start, b"PS C:\\work> \x1b]0;clau", 3600);
        assert_eq!((s.cleared, at), (false, None));
        let mut second = b"de\x1b\\".to_vec();
        let agent_title_at = second.len();
        second.extend(title("\u{2733} Claude Code", false));
        let (s, at) = on_chunk(&s, &second, 5400);
        assert!(s.cleared, "the agent's own title starts the clear");
        assert_eq!(at, Some(agent_title_at));
        assert_eq!(
            s.settle_at,
            Some(5400 + SETTLE_MS),
            "and the settle, not the ceiling"
        );
    }

    #[test]
    fn knows_which_agent_a_resume_is_for() {
        assert_eq!(
            agent_of_resume("3f2a9c1e-0000-4000-8000-123456789abc"),
            AgentKind::Claude
        );
        assert_eq!(agent_of_resume("codex:last"), AgentKind::Codex);
    }

    #[test]
    fn a_title_names_the_agent_only_when_it_is_the_program() {
        assert!(names_agent("claude", AgentKind::Claude));
        assert!(names_agent(
            r"C:\Users\me\.local\bin\claude.exe",
            AgentKind::Claude
        ));
        assert!(!names_agent(
            "pwsh -Command claude --resume abc",
            AgentKind::Claude
        ));
        assert!(!names_agent(
            r"C:\Users\me\Documents\Claude\Github",
            AgentKind::Claude
        ));
        assert!(!names_agent("✳ Claude Code", AgentKind::Claude));
        assert!(names_agent("codex", AgentKind::Codex));
    }

    #[test]
    fn clears_just_before_the_agents_own_title_with_either_terminator() {
        let before = "PS C:\\work> \x1b[1t\x1b[c\x1b[?1004h";
        let start = start_reveal(AgentKind::Claude, 0);
        let mut chunk = before.as_bytes().to_vec();
        chunk.extend(title("claude", true));
        chunk.extend_from_slice(b"\x1b[?25l");
        let (s, clear_at) = on_chunk(&start, &chunk, 3600);
        assert_eq!(clear_at, Some(before.len()));
        assert!(s.cleared);

        let (s2, clear2) = on_chunk(&s, &title("claude", false), 3700);
        assert_eq!(clear2, None);
        assert!(s2.cleared);
    }

    #[test]
    fn never_clears_on_a_shell_title_nor_on_a_folder_called_claude() {
        let start = start_reveal(AgentKind::Claude, 0);
        let (s, clear_at) = on_chunk(&start, &title(r"C:\pwsh\pwsh.exe", false), 100);
        assert_eq!(clear_at, None);
        assert!(!s.cleared);
        let (s, clear_at) = on_chunk(&start, &title(r"C:\Users\me\Documents\Claude", false), 100);
        assert_eq!(clear_at, None);
        assert!(!s.cleared);
    }

    #[test]
    fn reveals_a_moment_after_the_agents_own_title_not_at_once() {
        let s = cleared_at_3600();
        let mut chunk = b"\x1b[?2004h".to_vec();
        chunk.extend(title("✳ Claude Code", false));
        let (s, clear_at) = on_chunk(&s, &chunk, 5410);
        assert_eq!(clear_at, None);
        assert_eq!(s.settle_at, Some(5410 + SETTLE_MS));
        assert!(!on_tick(&s, 5410 + 149).revealed);
        assert!(on_tick(&s, 5410 + 150).revealed);
    }

    #[test]
    fn a_title_before_the_agent_took_the_console_is_not_its_paint() {
        let start = start_reveal(AgentKind::Claude, 0);
        let (s, _) = on_chunk(&start, &title("pwsh", false), 100);
        assert_eq!(s.settle_at, None);
    }

    #[test]
    fn reveals_after_the_switch_to_the_alternate_screen_too() {
        let start = start_reveal(AgentKind::Claude, 0);
        let s = on_alternate(&start, 5445);
        assert!(!on_tick(&s, 5445 + 149).revealed);
        assert!(on_tick(&s, 5445 + 150).revealed);
    }

    #[test]
    fn reveals_at_once_on_a_key_an_exit_or_a_failed_spawn_and_at_the_ceiling() {
        let start = start_reveal(AgentKind::Claude, 0);
        assert!(reveal_now(&start).revealed);
        assert!(!on_tick(&start, CEILING_MS - 1).revealed);
        assert!(on_tick(&start, CEILING_MS).revealed);
    }

    #[test]
    fn does_nothing_more_once_revealed() {
        let revealed = reveal_now(&start_reveal(AgentKind::Claude, 0));
        let (s, clear_at) = on_chunk(&revealed, &title("claude", false), 10);
        assert_eq!(s, revealed);
        assert_eq!(clear_at, None);
    }

    #[test]
    fn a_partial_utf8_chunk_does_not_shift_the_offset() {
        let start = start_reveal(AgentKind::Claude, 0);
        let mut chunk = vec![0xE2, 0x82];
        chunk.extend(title("claude", false));
        let (_, clear_at) = on_chunk(&start, &chunk, 0);
        assert_eq!(clear_at, Some(2));
    }
}
