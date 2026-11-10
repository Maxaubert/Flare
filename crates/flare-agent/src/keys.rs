//! The agent's image, newline and word-delete keys (Task 9).
//! AgentKind, DetectedAgent, is_resumable, CODEX_RESUME (Task 12).

use crate::kinds::AgentKind;

/// ESC v (Alt+V) for Claude, ^V (0x16) for Codex, agy, any other agent and a plain shell. EXACTLY
/// ONE key. agy takes Codex's keys until its own are measured.
pub fn image_paste_key(agent: Option<AgentKind>) -> &'static str {
    match agent {
        Some(AgentKind::Claude) => "\x1bv",
        Some(AgentKind::Agy) => "\x16",
        Some(AgentKind::Codex) | None => "\x16",
    }
}

/// LF (Ctrl+J) for Claude; backslash then CR for everything else (Codex drops a bare LF; agy as Codex).
pub fn newline_key(agent: Option<AgentKind>) -> &'static str {
    match agent {
        Some(AgentKind::Claude) => "\n",
        Some(AgentKind::Agy) => "\\\r",
        Some(AgentKind::Codex) | None => "\\\r",
    }
}

/// What Ctrl+Backspace sends in place of the encoder's ^H (0x08), or None to leave it to the encoder.
/// agy's prompt (Bubble Tea) deletes a word on ^W or ESC DEL and ignores ^H and every encoded
/// Ctrl+Backspace (kitty, modifyOtherKeys, win32-input-mode), measured on agy 1.4.8 under ConPTY,
/// so it gets ^W (0x17). Claude, Codex and a plain shell keep ^H.
pub fn word_rubout_key(agent: Option<AgentKind>) -> Option<&'static str> {
    match agent {
        Some(AgentKind::Agy) => Some("\x17"),
        Some(AgentKind::Claude) => None,
        Some(AgentKind::Codex) | None => None,
    }
}

/// What the keyboard path asks about the tab in front. M6 implements it; until then `NoAgent`.
pub trait AgentView {
    /// The agent whose keys are armed in this tab (Claude's title glyphs or Codex's Action Required
    /// before the poll). None: no agent here, so Shift+Enter is Enter.
    fn armed(&self, tab: &str) -> Option<AgentKind>;
}

/// The view used until M6: no tab has an agent armed.
pub struct NoAgent;

impl AgentView for NoAgent {
    fn armed(&self, _tab: &str) -> Option<AgentKind> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_alt_v_for_claude() {
        assert_eq!(image_paste_key(Some(AgentKind::Claude)), "\x1bv");
    }

    #[test]
    fn is_ctrl_v_for_codex_and_a_plain_shell() {
        assert_eq!(image_paste_key(Some(AgentKind::Codex)), "\x16");
        assert_eq!(image_paste_key(None), "\x16");
    }

    #[test]
    fn is_one_key_never_both() {
        let claude = image_paste_key(Some(AgentKind::Claude));
        assert!(!claude.contains('\x16'), "Claude's key must not hold ^V");
        let codex = image_paste_key(Some(AgentKind::Codex));
        assert!(!codex.contains('\x1b'), "Codex's key must not hold ESC");
    }

    #[test]
    fn newline_is_ctrl_j_for_claude() {
        assert_eq!(newline_key(Some(AgentKind::Claude)), "\n");
    }

    #[test]
    fn newline_is_backslash_enter_for_codex_and_anything_else() {
        assert_eq!(newline_key(Some(AgentKind::Codex)), "\\\r");
        assert_eq!(newline_key(None), "\\\r");
    }

    #[test]
    fn agy_takes_codexs_keys_until_measured() {
        assert_eq!(image_paste_key(Some(AgentKind::Agy)), "\x16");
        assert_eq!(newline_key(Some(AgentKind::Agy)), "\\\r");
    }

    #[test]
    fn word_rubout_is_ctrl_w_for_agy_only() {
        assert_eq!(word_rubout_key(Some(AgentKind::Agy)), Some("\x17"));
        assert_eq!(word_rubout_key(Some(AgentKind::Claude)), None);
        assert_eq!(word_rubout_key(Some(AgentKind::Codex)), None);
        assert_eq!(word_rubout_key(None), None);
    }

    #[test]
    fn no_agent_arms_nothing() {
        assert_eq!(NoAgent.armed("t1"), None);
    }
}
