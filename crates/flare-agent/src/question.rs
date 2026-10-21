//! Reading a question on the Claude screen (Task 6). Ported from Prism Terminal's `agentQuestion.ts`.
//!
//! IS THE AGENT WAITING ON YOU? Claude Code says nothing to the terminal about a question. MEASURED:
//! the title goes idle exactly as it does when the agent has finished. What differs is the SCREEN: the
//! question box draws its footer, "Enter to select · ↑/↓ to navigate · Esc to cancel", and a permission
//! prompt asks "Do you want to ...?" over numbered choices ("1. Yes"). The rows here are the bottom rows
//! of the screen (trailing blanks trimmed), read when the title goes idle and again as output settles.

use regex::Regex;
use std::sync::LazyLock;

static FOOTER_SELECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Enter to select").expect("FOOTER_SELECT"));
static FOOTER_CANCEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Esc to cancel").expect("FOOTER_CANCEL"));
static PERMISSION_ASK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Do you want to [^\n]*\?").expect("PERMISSION_ASK"));
static PERMISSION_YES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)(^|\s)1\.\s+Yes\b").expect("PERMISSION_YES"));

/// True when the rows show a question box: the question tool's footer, or a permission prompt.
pub fn looks_like_question(rows: &[String]) -> bool {
    let text = rows.join("\n");
    // The question tool's footer.
    if FOOTER_SELECT.is_match(&text) && FOOTER_CANCEL.is_match(&text) {
        return true;
    }
    // A permission prompt: the question and its first, numbered answer.
    PERMISSION_ASK.is_match(&text) && PERMISSION_YES.is_match(&text)
}

/// The keys that settle a question: Enter, Esc, Ctrl+C, or exactly one digit 1 to 9.
///
/// A question lasts until it is answered. Turning a permission prompt down or cancelling with Esc ends
/// the turn with no hook, so these keys are heard too. Walking the choices is not one.
pub fn answers_question(key: &str) -> bool {
    if key == "\r" || key == "\x1b" || key == "\x03" {
        return true;
    }
    let mut chars = key.chars();
    matches!((chars.next(), chars.next()), (Some('1'..='9'), None))
}

/// A key that settles a box, with the box gone from the screen after it. A box still up is the next of
/// several questions, still waiting.
pub fn question_answered(key: &str, rows: &[String]) -> bool {
    answers_question(key) && !looks_like_question(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a row list from string slices.
    fn rows(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| (*s).to_string()).collect()
    }

    fn question_box() -> Vec<String> {
        rows(&[
            "● Which colour do you prefer?",
            "",
            "  ❯ 1. Red",
            "    2. Blue",
            "    3. Type something.",
            "",
            "Enter to select · ↑/↓ to navigate · Esc to cancel",
        ])
    }

    fn permission() -> Vec<String> {
        rows(&[
            " Bash command",
            "",
            "   echo hello-signals",
            "   Print a test string",
            "",
            " Do you want to proceed?",
            " ❯ 1. Yes",
            "   2. Yes, and don't ask again for echo commands in this folder",
            "   3. No, and tell Claude what to do differently (esc)",
        ])
    }

    fn finished() -> Vec<String> {
        rows(&[
            "● I ran the command and it printed: hello-signals",
            "",
            "✻ Worked for 7s · done 18:49",
            "",
            "────────────────────────────────────────",
            "❯ ",
            "────────────────────────────────────────",
            "  ⏵⏵ auto mode on (shift+tab to cycle)",
        ])
    }

    fn rule() -> String {
        "╌".repeat(40)
    }

    fn a_box() -> Vec<String> {
        let dashes = "─".repeat(40);
        let rule = rule();
        rows(&[
            "  ⎿  Waiting…",
            "",
            &dashes,
            " Bash command",
            " Run a 25-second sleep via PowerShell",
            &rule,
            " powershell -NoProfile -Command Start-Sleep 25",
            &rule,
            " Permission rule Bash(powershell *) requires confirmation for this command.",
            " /permissions to update rules",
            "",
            " Do you want to proceed?",
            " ❯ 1. Yes",
            "   2. No",
            "",
            " Esc to cancel · Tab to amend",
        ])
    }

    fn a_after_yes() -> Vec<String> {
        let dashes = "─".repeat(40);
        rows(&[
            "❯ Use the Bash tool to run exactly this command once, nothing else, then reply OK: powershell -NoProfile -Command Start-Sleep 25",
            "",
            "● Bash(powershell -NoProfile -Command Start-Sleep 25)",
            "  ⎿  Running…",
            "",
            "✻ Cultivating… (4s · ↓ 220 tokens)",
            "",
            &dashes,
            "❯ ",
            &dashes,
            "  ⏸ manual mode on · esc to interrupt · ← 3 agents                                       ◐ medium · /effort",
        ])
    }

    fn g2_box() -> Vec<String> {
        let dashes = "─".repeat(40);
        let rule = rule();
        rows(&[
            "✻ Waiting for 2 background agents to finish",
            "",
            &dashes,
            " Bash command · from the general-purpose agent",
            " Run a 25-second sleep via PowerShell",
            &rule,
            " powershell -NoProfile -Command Start-Sleep 25",
            &rule,
            " Permission rule Bash(powershell *) requires confirmation for this command.",
            " /permissions to update rules",
            "",
            " Do you want to proceed?",
            " ❯ 1. Yes",
            "   2. No",
            "",
            " Esc to cancel · Tab to amend · ctrl+x ctrl+k twice to stop background agents",
        ])
    }

    fn g2_after_yes() -> Vec<String> {
        let dashes = "─".repeat(40);
        rows(&[
            "✻ Waiting for 2 background agents to finish",
            "",
            &dashes,
            "❯ ",
            &dashes,
            "  ⏸ manual mode on · ? for shortcuts · ← 3 agents · ↓ to manage                        ◐ medium · /effort",
            "",
            "  ● main",
            "  ◯ general-purpose  Run 25s sleep command                                     19s · ↓ 34.6k tokens",
            "  ◯ general-purpose  Run five sequential echo calls                            19s · ↓ 37.3k tokens",
        ])
    }

    #[test]
    fn knows_the_question_tools_box() {
        assert!(looks_like_question(&question_box()));
    }

    #[test]
    fn knows_a_permission_prompt() {
        assert!(looks_like_question(&permission()));
    }

    #[test]
    fn does_not_take_a_finished_answer_or_an_idle_prompt_for_a_question() {
        assert!(!looks_like_question(&finished()));
        assert!(!looks_like_question(&rows(&["PS C:\\Users\\me> "])));
    }

    #[test]
    fn does_not_take_words_that_merely_mention_it() {
        assert!(!looks_like_question(&rows(&[
            "Press Esc to cancel a build.",
            "Done."
        ])));
        assert!(!looks_like_question(&rows(&[
            "Do you want to know more? Read the docs."
        ])));
    }

    #[test]
    fn enter_esc_ctrl_c_and_a_numbered_choice_settle_a_box() {
        for k in ["\r", "\x1b", "\x03", "1", "3", "9"] {
            assert!(answers_question(k), "key {k:?}");
        }
    }

    #[test]
    fn walking_the_choices_or_typing_a_letter_does_not() {
        for k in ["\x1b[A", "\x1b[B", "\t", "a", " ", "0", "12", ""] {
            assert!(!answers_question(k), "key {k:?}");
        }
    }

    #[test]
    fn an_answering_key_with_the_box_gone_is_an_answer() {
        assert!(question_answered("\r", &finished()));
        assert!(question_answered("\x1b", &rows(&["PS C:\\Users\\me> "])));
    }

    #[test]
    fn the_box_still_up_is_not() {
        assert!(!question_answered("\r", &question_box()));
        assert!(!question_answered("1", &permission()));
    }

    #[test]
    fn a_key_that_answers_nothing_is_not() {
        assert!(!question_answered("\x1b[B", &finished()));
    }

    #[test]
    fn the_permission_box_is_seen_main_agents_and_a_subagents() {
        assert!(looks_like_question(&a_box()));
        assert!(looks_like_question(&g2_box()));
    }

    #[test]
    fn the_repaint_after_yes_is_not_a_question() {
        assert!(!looks_like_question(&a_after_yes()));
        assert!(!looks_like_question(&g2_after_yes()));
    }

    #[test]
    fn the_box_fits_the_16_rows() {
        assert!(a_box().len() <= 16);
        assert!(g2_box().len() <= 16);
    }
}
