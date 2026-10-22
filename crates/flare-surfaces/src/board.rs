//! The agent board's rows (Surfaces stage 7): every tab with an agent, its state and how long it has been
//! in it, the folder and the question it waits on. Pure: the window hands in what the agent hub already
//! holds (no disk, no poll) and draws the rows this makes.
//!
//! The rows keep the tab strip's order, so a row does not jump under the pointer when a state changes;
//! the state's mark (the same ring the strip shows) says which ones want the person.

/// What an agent is doing, as the board words it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardState {
    Working,
    Question,
    Finished,
    Failed,
    /// The agent is in the tab and has nothing to say.
    Idle,
}

impl BoardState {
    /// The word on the row and on the wire (`working`, `question`, `finished`, `failed`, `idle`).
    pub fn word(self) -> &'static str {
        match self {
            BoardState::Working => "working",
            BoardState::Question => "question",
            BoardState::Finished => "finished",
            BoardState::Failed => "failed",
            BoardState::Idle => "idle",
        }
    }

    /// The tab mark's state number (0 none, 1 working, 2 question, 3 finished, 4 failed).
    pub fn mark(self) -> i32 {
        match self {
            BoardState::Working => 1,
            BoardState::Question => 2,
            BoardState::Finished => 3,
            BoardState::Failed => 4,
            BoardState::Idle => 0,
        }
    }
}

/// Which agent runs in the tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardAgent {
    Claude,
    Codex,
    Other,
}

impl BoardAgent {
    pub fn name(self) -> &'static str {
        match self {
            BoardAgent::Claude => "Claude",
            BoardAgent::Codex => "Codex",
            BoardAgent::Other => "Agent",
        }
    }
}

/// What the agent hub holds of one agent tab: the half of a [`BoardInput`] the hub knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardEntry {
    pub agent: BoardAgent,
    pub state: BoardState,
    /// How long the tab has been in this state, in milliseconds, when that is known.
    pub in_state_ms: Option<u64>,
    pub question: Option<String>,
    pub failed_kind: Option<String>,
}

/// What the window knows of one agent tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardInput {
    pub tab: String,
    /// The tab's name as the strip shows it.
    pub name: String,
    pub agent: BoardAgent,
    pub state: BoardState,
    /// The tab's folder, as the strip's second line says it.
    pub folder: String,
    /// How long the tab has been in this state, in milliseconds; None when it is not known.
    pub in_state_ms: Option<u64>,
    /// The question the agent waits on, when the screen showed it.
    pub question: Option<String>,
    /// Why the turn failed (`rate_limit`), when it did.
    pub failed_kind: Option<String>,
}

/// One row of the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardRow {
    pub tab: String,
    pub name: String,
    pub agent: &'static str,
    pub state: BoardState,
    pub folder: String,
    /// "Working for 4m 12s", "Waiting 2m", "Finished 5m ago", "Failed 1m ago", "Idle".
    pub status: String,
    /// The question, or why it failed; empty for the others.
    pub detail: String,
}

/// The longest detail line kept, in characters.
pub const DETAIL_MAX: usize = 400;

/// The rows for these tabs, in the order given.
pub fn rows(inputs: &[BoardInput]) -> Vec<BoardRow> {
    inputs.iter().map(row).collect()
}

fn row(i: &BoardInput) -> BoardRow {
    let detail = match i.state {
        BoardState::Question => i
            .question
            .as_deref()
            .map(|q| one_line(q, DETAIL_MAX))
            .unwrap_or_default(),
        BoardState::Failed => i
            .failed_kind
            .as_deref()
            .map(|k| one_line(&k.replace(['_', '-'], " "), 80))
            .unwrap_or_default(),
        _ => String::new(),
    };
    BoardRow {
        tab: i.tab.clone(),
        name: one_line(&i.name, 80),
        agent: i.agent.name(),
        state: i.state,
        folder: one_line(&i.folder, 160),
        status: status_words(i.state, i.in_state_ms),
        detail,
    }
}

/// "Working for 4m 12s", "Waiting 2m", "Finished 5m ago", "Failed 1m ago", "Idle".
pub fn status_words(state: BoardState, in_state_ms: Option<u64>) -> String {
    let span = in_state_ms.map(span_words);
    match (state, span) {
        (BoardState::Idle, _) => "Idle".to_string(),
        (BoardState::Working, Some(s)) => format!("Working for {s}"),
        (BoardState::Working, None) => "Working".to_string(),
        (BoardState::Question, Some(s)) => format!("Waiting {s}"),
        (BoardState::Question, None) => "Waiting".to_string(),
        (BoardState::Finished, Some(s)) => format!("Finished {s} ago"),
        (BoardState::Finished, None) => "Finished".to_string(),
        (BoardState::Failed, Some(s)) => format!("Failed {s} ago"),
        (BoardState::Failed, None) => "Failed".to_string(),
    }
}

/// A length of time in short words: "12s", "4m 12s", "23m", "2h 05m", "3d 4h". Under a second reads "0s".
pub fn span_words(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        return format!("{s}s");
    }
    let m = s / 60;
    if m < 10 {
        return format!("{m}m {:02}s", s % 60);
    }
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    if h < 24 {
        return format!("{h}h {:02}m", m % 60);
    }
    format!("{}d {}h", h / 24, h % 24)
}

/// Text from a screen or a hook, as one safe line: controls, line breaks and the characters that reorder or hide
/// text become spaces, runs of spaces become one, and it is cut at `max` characters with an ellipsis.
pub fn one_line(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut gap = true;
    let mut n = 0;
    for c in text.chars() {
        let c = if c.is_control() || hidden(c) { ' ' } else { c };
        if c.is_whitespace() {
            if !gap {
                out.push(' ');
                gap = true;
                n += 1;
            }
        } else {
            out.push(c);
            gap = false;
            n += 1;
        }
        if n > max {
            break;
        }
    }
    let trimmed = out.trim_end();
    if trimmed.chars().count() > max {
        let cut: String = trimmed.chars().take(max.saturating_sub(1)).collect();
        return format!("{}\u{2026}", cut.trim_end());
    }
    trimmed.to_string()
}

/// A character that changes how text reads without showing: a zero-width one, a bidirectional control, a tag character,
/// a variation selector and the byte order mark.
fn hidden(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164
        | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF9..=0xFFFB | 0xE0000..=0xE0FFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(tab: &str, state: BoardState) -> BoardInput {
        BoardInput {
            tab: tab.into(),
            name: format!("name {tab}"),
            agent: BoardAgent::Claude,
            state,
            folder: "~\\work".into(),
            in_state_ms: Some(0),
            question: None,
            failed_kind: None,
        }
    }

    #[test]
    fn the_rows_keep_the_order_they_were_given() {
        let r = rows(&[
            input("b", BoardState::Question),
            input("a", BoardState::Working),
            input("c", BoardState::Idle),
        ]);
        let tabs: Vec<&str> = r.iter().map(|x| x.tab.as_str()).collect();
        assert_eq!(tabs, ["b", "a", "c"]);
    }

    #[test]
    fn every_state_has_its_word_and_mark() {
        use BoardState::*;
        let all = [Working, Question, Finished, Failed, Idle];
        let words: Vec<&str> = all.iter().map(|s| s.word()).collect();
        assert_eq!(words, ["working", "question", "finished", "failed", "idle"]);
        let marks: Vec<i32> = all.iter().map(|s| s.mark()).collect();
        assert_eq!(marks, [1, 2, 3, 4, 0]);
    }

    #[test]
    fn time_reads_in_short_words() {
        for (ms, want) in [
            (0, "0s"),
            (999, "0s"),
            (1000, "1s"),
            (59_999, "59s"),
            (60_000, "1m 00s"),
            (252_000, "4m 12s"),
            (599_000, "9m 59s"),
            (600_000, "10m"),
            (3_599_000, "59m"),
            (3_600_000, "1h 00m"),
            (7_500_000, "2h 05m"),
            (86_399_000, "23h 59m"),
            (86_400_000, "1d 0h"),
            (97_200_000, "1d 3h"),
        ] {
            assert_eq!(span_words(ms), want, "{ms}");
        }
    }

    #[test]
    fn the_status_says_what_the_time_is_of() {
        use BoardState::*;
        assert_eq!(status_words(Working, Some(252_000)), "Working for 4m 12s");
        assert_eq!(status_words(Question, Some(120_000)), "Waiting 2m 00s");
        assert_eq!(status_words(Finished, Some(300_000)), "Finished 5m 00s ago");
        assert_eq!(status_words(Failed, Some(61_000)), "Failed 1m 01s ago");
        assert_eq!(status_words(Idle, Some(5_000)), "Idle");
        assert_eq!(status_words(Working, None), "Working");
        assert_eq!(status_words(Question, None), "Waiting");
        assert_eq!(status_words(Finished, None), "Finished");
        assert_eq!(status_words(Failed, None), "Failed");
    }

    #[test]
    fn only_a_question_row_carries_the_question_and_only_a_failed_row_the_reason() {
        let mut q = input("a", BoardState::Question);
        q.question = Some("Which colour?".into());
        q.failed_kind = Some("rate_limit".into());
        assert_eq!(row(&q).detail, "Which colour?");
        let mut f = input("b", BoardState::Failed);
        f.question = Some("Which colour?".into());
        f.failed_kind = Some("rate_limit".into());
        assert_eq!(row(&f).detail, "rate limit");
        let mut w = input("c", BoardState::Working);
        w.question = Some("stale".into());
        assert_eq!(row(&w).detail, "");
        // A question whose text is not known is a row with no detail, not an invented one.
        assert_eq!(row(&input("d", BoardState::Question)).detail, "");
    }

    #[test]
    fn text_from_the_screen_is_one_clean_line() {
        assert_eq!(one_line("a\r\nb\tc   d", 50), "a b c d");
        assert_eq!(one_line("  padded  ", 50), "padded");
        assert_eq!(one_line("x\u{202e}evil\u{200b}y", 50), "x evil y");
        assert_eq!(one_line("a\u{e0041}b", 50), "a b");
        assert_eq!(one_line("a\u{1b}[31mb", 50), "a [31mb");
        assert_eq!(one_line("", 5), "");
        assert_eq!(one_line("   ", 5), "");
    }

    #[test]
    fn a_long_line_is_cut_with_an_ellipsis() {
        let long = "word ".repeat(200);
        let cut = one_line(&long, 40);
        assert!(cut.chars().count() <= 40, "{}", cut.chars().count());
        assert!(cut.ends_with('\u{2026}'));
        let exact = "a".repeat(40);
        assert_eq!(one_line(&exact, 40), exact);
        let over = "a".repeat(41);
        assert_eq!(one_line(&over, 40).chars().count(), 40);
    }

    #[test]
    fn a_name_and_a_folder_are_cleaned_too() {
        let mut i = input("a", BoardState::Idle);
        i.name = "evil\u{202e}name\n".into();
        i.folder = "C:\\a\tb".into();
        let r = row(&i);
        assert_eq!(r.name, "evil name");
        assert_eq!(r.folder, "C:\\a b");
    }
}
