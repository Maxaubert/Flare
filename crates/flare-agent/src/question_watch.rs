//! The question watch on a tab screen (Task 6). Ported from Prism Terminal's `questionWatch.ts` (Sonara #323).
//!
//! Watches one tab for Claude Code's question widget and says when the question showing changes: after
//! output settles (`delay_ms`), the live screen is read and the widget's question extracted. A change is
//! reported once; the widget gone resets, so the same question shown again later is reported again.
//! The glue calls `poll` at or after `next_due`; this struct never sleeps or runs a timer of its own.

use crate::question_widget::{ShownQuestion, extract_question, same_question};

/// The live screen rows of a terminal: the buffer's bottom, not the viewport, so a user scrolled up still
/// has the widget followed. The rows read are from `max(0, base_y - rows)` to `base_y + rows` of the
/// buffer, trailing blanks trimmed, so a widget taller than the pane (its tab row in scrollback) is found.
pub trait RowSource {
    /// The rows to read, top to bottom.
    fn live_rows(&self) -> Vec<String>;
}

/// The watch state for one tab.
pub struct QuestionWatch {
    /// The question last reported or seen; None once the widget is gone.
    last: Option<ShownQuestion>,
    /// The time (ms) the next look is due, or None when no output is pending a look.
    due: Option<u64>,
    /// How long output must settle before a look (150 in the app).
    delay_ms: u64,
    /// Set by `dispose`: no more looks report.
    disposed: bool,
}

impl QuestionWatch {
    /// A watch that looks `delay_ms` after output settles.
    pub fn new(delay_ms: u64) -> Self {
        QuestionWatch {
            last: None,
            due: None,
            delay_ms,
            disposed: false,
        }
    }

    /// Output arrived at `now`: (re)arms the look at `now + delay_ms`, so a later output pushes it later.
    pub fn on_output(&mut self, now: u64) {
        if !self.disposed {
            self.due = Some(now + self.delay_ms);
        }
    }

    /// The time (ms) the next look is due, if one is pending.
    pub fn next_due(&self) -> Option<u64> {
        self.due
    }

    /// Reads `src` when a look is due at or before `now`. Returns the question ONLY when it changed.
    pub fn poll(&mut self, now: u64, src: &dyn RowSource) -> Option<ShownQuestion> {
        if self.disposed {
            return None;
        }
        match self.due {
            Some(due) if now >= due => {}
            _ => return None,
        }
        self.due = None;
        let current = extract_question(&src.live_rows());
        if same_question(current.as_ref(), self.last.as_ref()) {
            return None;
        }
        self.last = current.clone();
        current
    }

    /// Stops the watch: no more polls report, and no output arms one.
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.due = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::cell::RefCell;

    /// A named screen from the fixture the PT source measured (`question_widget_screens.json`).
    fn at(name: &str) -> Vec<String> {
        let all: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/question_widget_screens.json"
        ))
        .expect("fixture parses");
        all[name]
            .as_array()
            .expect("fixture key is an array")
            .iter()
            .map(|v| v.as_str().expect("row is a string").to_string())
            .collect()
    }

    /// A buffer of `lines`, `rows` high: the last `rows` lines are the live screen, the rest scrollback.
    struct FakeRows {
        lines: RefCell<Vec<String>>,
        rows: usize,
    }

    impl FakeRows {
        fn new(rows: usize) -> Self {
            FakeRows {
                lines: RefCell::new(Vec::new()),
                rows,
            }
        }
    }

    impl RowSource for FakeRows {
        fn live_rows(&self) -> Vec<String> {
            let lines = self.lines.borrow();
            let base_y = lines.len().saturating_sub(self.rows);
            let from = base_y.saturating_sub(self.rows);
            let to = (base_y + self.rows).min(lines.len());
            lines[from..to].to_vec()
        }
    }

    /// A watch, a fake buffer and a clock, driven the way the glue drives it.
    struct Harness {
        watch: QuestionWatch,
        rows: FakeRows,
        now: u64,
        got: Vec<ShownQuestion>,
    }

    impl Harness {
        fn new(rows: usize) -> Self {
            Harness {
                watch: QuestionWatch::new(150),
                rows: FakeRows::new(rows),
                now: 0,
                got: Vec::new(),
            }
        }

        /// Output arrives: the buffer becomes `lines` and the watch hears it.
        fn write(&mut self, lines: Vec<String>) {
            *self.rows.lines.borrow_mut() = lines;
            self.watch.on_output(self.now);
        }

        /// Time passes by `ms`; the glue polls once the look is due.
        fn advance(&mut self, ms: u64) {
            self.now += ms;
            if let Some(due) = self.watch.next_due()
                && self.now >= due
                && let Some(shown) = self.watch.poll(self.now, &self.rows)
            {
                self.got.push(shown);
            }
        }
    }

    fn text(s: &str) -> ShownQuestion {
        ShownQuestion::Text(s.to_string())
    }

    #[test]
    fn reports_each_change_of_question_once_after_output_settles() {
        let mut h = Harness::new(40);
        h.write(at("run2-01-question-1"));
        h.write(at("run2-01-question-1"));
        assert!(h.got.is_empty());
        h.advance(150);
        assert_eq!(h.got, vec![text("Pick a colour?")]);
        h.write(at("run2-01-question-1"));
        h.advance(150);
        assert_eq!(h.got.len(), 1);
        h.write(at("run2-02-right"));
        h.advance(150);
        h.write(at("run2-08-enter-answer"));
        h.advance(150);
        assert_eq!(
            h.got,
            vec![
                text("Pick a colour?"),
                text("Pick a size?"),
                ShownQuestion::Review
            ]
        );
    }

    #[test]
    fn reports_the_same_question_again_after_the_widget_went_away() {
        let mut h = Harness::new(40);
        h.write(at("run2-02-right"));
        h.advance(150);
        h.write(vec!["PS C:\\> ".to_string()]);
        h.advance(150);
        h.write(at("run2-02-right"));
        h.advance(150);
        assert_eq!(h.got, vec![text("Pick a size?"), text("Pick a size?")]);
    }

    #[test]
    fn finds_a_widget_taller_than_the_pane() {
        let mut h = Harness::new(8);
        h.write(at("run2-02-right"));
        h.advance(150);
        assert_eq!(h.got, vec![text("Pick a size?")]);
    }

    #[test]
    fn follows_the_newer_of_two_widget_frames() {
        let mut h = Harness::new(50);
        h.write(at("run2-01-question-1"));
        h.advance(150);
        let mut both = at("run2-01-question-1");
        both.extend(at("run2-02-right"));
        h.write(both);
        h.advance(150);
        assert_eq!(h.got, vec![text("Pick a colour?"), text("Pick a size?")]);
    }

    #[test]
    fn reads_the_live_screen_while_the_user_has_scrolled_up() {
        let mut h = Harness::new(30);
        let mut lines = at("run2-01-question-1");
        lines.extend(std::iter::repeat_n("output".to_string(), 40));
        lines.extend(at("run2-02-right"));
        h.write(lines);
        h.advance(150);
        assert_eq!(h.got, vec![text("Pick a size?")]);
    }

    #[test]
    fn stops_on_dispose() {
        let mut h = Harness::new(40);
        h.write(at("run2-02-right"));
        h.watch.dispose();
        h.advance(500);
        h.write(at("run2-01-question-1"));
        h.advance(500);
        assert!(h.got.is_empty());
        assert_eq!(h.watch.next_due(), None);
    }
}
