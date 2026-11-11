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
    // A question box takes the place of the prompt box. With the empty input box up, between its two
    // rules, nothing is being asked: words that quote the footer or "Do you want to ...?" are
    // conversation (review #49).
    if input_box_up(rows) {
        return false;
    }
    let text = rows.join("\n");
    // The question tool's footer.
    if FOOTER_SELECT.is_match(&text) && FOOTER_CANCEL.is_match(&text) {
        return true;
    }
    // A permission prompt: the question and its first, numbered answer.
    PERMISSION_ASK.is_match(&text) && PERMISSION_YES.is_match(&text)
}

/// A rule of the prompt's box: a long run of box-drawing dashes.
static RULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*─{20,}\s*$").expect("RULE"));

/// The prompt box is on the screen: a rule, the `❯` input row (and any more rows of a draft), a rule.
fn input_box_up(rows: &[String]) -> bool {
    rows.iter().enumerate().any(|(i, r)| {
        RULE.is_match(r)
            && rows.get(i + 1).is_some_and(|p| p.starts_with('❯'))
            && rows[i + 2..rows.len().min(i + 8)]
                .iter()
                .any(|x| RULE.is_match(x))
    })
}

/// Text already typed in Claude's prompt box, unsent: a row of the box (between its two rules) holds
/// something after the `❯` mark. `false` when no box is on the screen to be read: the caller then cannot
/// tell, and the send's other checks stand (review #27).
pub fn prompt_has_draft(rows: &[String]) -> bool {
    prompt_draft(rows).unwrap_or(false)
}

/// [`prompt_has_draft`], telling "no box on the screen" (`None`) from an empty box (`Some(false)`): the
/// dictation send watches its words land in the box and its Enter empty it, and can only when it is there.
pub fn prompt_draft(rows: &[String]) -> Option<bool> {
    let low = rows.iter().rposition(|r| RULE.is_match(r))?;
    let top = rows[..low].iter().rposition(|r| RULE.is_match(r))?;
    Some(rows[top + 1..low].iter().any(|r| {
        let body = r.trim_start_matches(['❯', '>']);
        !body
            .trim_matches(|c: char| c.is_whitespace() || c == '\u{a0}')
            .is_empty()
    }))
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
    fn a_draft_in_the_prompt_box_is_seen_and_an_empty_box_is_not() {
        let dashes = "─".repeat(40);
        let with =
            |input: &str| rows(&["● done", "", &dashes, input, &dashes, "  ⏵⏵ auto mode on"]);
        assert!(!prompt_has_draft(&with("❯ ")));
        assert!(!prompt_has_draft(&with("❯\u{a0}")));
        assert!(prompt_has_draft(&with("❯ fix the thing")));
        assert!(prompt_has_draft(&with("❯\u{a0}half a thought")));
        // No box on the screen: unknown, so no draft is claimed.
        assert!(!prompt_has_draft(&rows(&["PS C:\\w> npm test"])));
    }

    #[test]
    fn prompt_draft_tells_no_box_from_an_empty_box_and_sees_a_long_paste() {
        let dashes = "─".repeat(40);
        let with = |input: &str| rows(&["● done", &dashes, input, &dashes, "  ⏵⏵ auto mode on"]);
        assert_eq!(prompt_draft(&with("❯ ")), Some(false));
        assert_eq!(prompt_draft(&with("❯ fix it")), Some(true));
        // Claude shows a long paste as one line that names it.
        assert_eq!(
            prompt_draft(&with("❯ [Pasted text #3 +40 lines]")),
            Some(true)
        );
        assert_eq!(prompt_draft(&rows(&["PS C:\\w> npm test"])), None);
    }

    #[test]
    fn conversation_that_quotes_a_question_above_the_prompt_box_is_not_one() {
        let dashes = "─".repeat(40);
        let quoting = rows(&[
            "● The footer says \"Enter to select · Esc to cancel\" and the prompt",
            "  asks: Do you want to proceed?",
            "   1. Yes",
            "",
            &dashes,
            "❯ ",
            &dashes,
            "  ⏵⏵ auto mode on (shift+tab to cycle)",
        ]);
        assert!(!looks_like_question(&quoting));
        // The same words with the real box up (no input box) still read as a question.
        assert!(looks_like_question(&permission()));
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
