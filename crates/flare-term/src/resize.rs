//! `resize_conpty`: a resize that agrees with ConPTY about where the cursor row sits (decision O3).
//!
//! ConPTY keeps its viewport top-anchored and reflows only the old screen rows; alacritty keeps
//! the cursor row fixed and pushes extra rows into history. After a plain `Term::resize` the two
//! models disagree, and ConPTY's absolute cursor addressing then lands on the wrong rows. This
//! module computes the row ConPTY will have the cursor on and shifts alacritty's grid to match.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::Color;

use crate::emulator::Dims;

/// Rows the temporary grow in the pull-down may add at most (defensive; never reached in practice).
const MAX_PULL_ROWS: usize = 1000;

fn is_blank(c: &Cell) -> bool {
    (c.c == ' ' || c.c == '\0')
        && !c.flags.contains(Flags::WIDE_CHAR_SPACER)
        && c.zerowidth().is_none()
}

/// Every screen row below the cursor line is blank.
fn rows_below_cursor_blank<T>(term: &Term<T>) -> bool {
    let g = term.grid();
    let cur = g.cursor.point.line.0;
    let lines = i32::try_from(g.screen_lines()).unwrap_or(i32::MAX);
    for line in (cur + 1)..lines {
        for col in 0..g.columns() {
            if !is_blank(&g[Point::new(Line(line), Column(col))]) {
                return false;
            }
        }
    }
    true
}

/// Where ConPTY will have the cursor row after the resize. ConPTY reflows only the old screen
/// rows, keeps the viewport top fixed and leaves blank rows at the bottom. Logical lines above
/// the cursor's line take `max(1, ceil(trimmed_len / new_cols))` rows each; the cursor's own line
/// adds `offset / new_cols` rows. The sum is clamped to `new_rows - 1`.
fn conpty_cursor_row<T>(term: &Term<T>, new_cols: usize, new_rows: usize) -> usize {
    let g = term.grid();
    let w = g.columns();
    let lines = g.screen_lines();
    let cur_row = usize::try_from(g.cursor.point.line.0).unwrap_or(0);
    let mut above_rows = 0usize;
    let mut offset = 0usize;
    let mut r = 0usize;
    while r < lines {
        let s = r;
        let mut e = r;
        while e + 1 < lines
            && g[Point::new(Line(i32::try_from(e).unwrap_or(0)), Column(w - 1))]
                .flags
                .contains(Flags::WRAPLINE)
        {
            e += 1;
        }
        if cur_row >= s && cur_row <= e {
            offset = (cur_row - s) * w + g.cursor.point.column.0;
            break;
        }
        let mut len = 0usize;
        for row in s..=e {
            for c in 0..w {
                let line = Line(i32::try_from(row).unwrap_or(0));
                if !is_blank(&g[Point::new(line, Column(c))]) {
                    len = (row - s) * w + c + 1;
                }
            }
        }
        above_rows += len.div_ceil(new_cols.max(1)).max(1);
        r = e + 1;
    }
    (above_rows + offset / new_cols.max(1)).min(new_rows.saturating_sub(1))
}

/// Resizes `term` the way ConPTY resizes its own buffer, so the cursor row stays where ConPTY's
/// absolute cursor addressing will look for it.
pub fn resize_conpty<T: EventListener>(term: &mut Term<T>, cols: usize, rows: usize) {
    if (term.columns(), term.screen_lines()) == (cols, rows) {
        return;
    }
    let alt = term.mode().contains(TermMode::ALT_SCREEN);
    let target = if !alt && rows_below_cursor_blank(term) {
        Some(conpty_cursor_row(term, cols, rows))
    } else {
        None
    };
    term.resize(Dims { cols, rows });
    let Some(r_t) = target else { return };
    let r_a = usize::try_from(term.grid().cursor.point.line.0).unwrap_or(0);
    if r_t > r_a {
        // ConPTY kept more of the top: pull rows back out of history. Growing the line count
        // pulls from history and moves the cursor down; shrinking back drops the blank bottom.
        let pull = (r_t - r_a).min(term.grid().history_size());
        if pull > 0 && rows + pull <= MAX_PULL_ROWS {
            term.resize(Dims {
                cols,
                rows: rows + pull,
            });
            term.resize(Dims { cols, rows });
        }
    } else if r_a > r_t {
        // ConPTY has the cursor higher: push the extra top rows into history.
        let k = r_a - r_t;
        let end = Line(i32::try_from(rows).unwrap_or(i32::MAX));
        term.grid_mut().scroll_up::<Color>(&(Line(0)..end), k);
        term.grid_mut().cursor.point.line = Line(i32::try_from(r_t).unwrap_or(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::Emulator;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    const H1: &str = "Microsoft Windows [Version 10.0.26300.9711]";
    const H2: &str = "(c) Microsoft Corporation. All rights reserved.";
    const P: &str =
        "C:\\Users\\Admin\\Documents\\Claude\\Github\\Flare\\.claude\\worktrees\\m0\\crates>";

    struct T {
        term: Term<VoidListener>,
        proc: Processor,
    }

    fn make(cols: usize, rows: usize) -> T {
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        T {
            term: Term::new(config, &Dims { cols, rows }, VoidListener),
            proc: Processor::new(),
        }
    }

    fn feed(t: &mut T, s: &str) {
        t.proc.advance(&mut t.term, s.as_bytes());
    }

    fn row(t: &T, line: i32) -> String {
        let g = t.term.grid();
        let mut s = String::new();
        for c in 0..g.columns() {
            let cell = &g[Point::new(Line(line), Column(c))];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            s.push(cell.c);
        }
        s.trim_end().to_string()
    }

    fn cur(t: &T) -> (i32, usize) {
        let p = t.term.grid().cursor.point;
        (p.line.0, p.column.0)
    }

    fn hist(t: &T) -> usize {
        t.term.grid().history_size()
    }

    fn banner_state() -> T {
        let mut t = make(80, 24);
        feed(&mut t, &format!("{H1}\r\n{H2}\r\n\r\n{P}abcdef"));
        t
    }

    fn full_screen_state() -> T {
        let mut t = make(80, 24);
        for i in 1..=60 {
            feed(&mut t, &format!("line{i}\r\n"));
        }
        feed(&mut t, &format!("{P}abcdef"));
        t
    }

    #[test]
    fn narrowing_keeps_the_banner_on_screen() {
        let mut t = banner_state();
        assert_eq!(cur(&t), (3, 79));
        resize_conpty(&mut t.term, 60, 24);
        assert_eq!(cur(&t), (4, 19));
        assert_eq!(hist(&t), 0);
        assert_eq!(row(&t, 0), H1);
        assert_eq!(row(&t, 1), H2);
        assert_eq!(row(&t, 3), &P[..60]);
        assert_eq!(row(&t, 4), "es\\m0\\crates>abcdef");
    }

    #[test]
    fn narrowing_again_to_forty() {
        let mut t = banner_state();
        resize_conpty(&mut t.term, 60, 24);
        resize_conpty(&mut t.term, 40, 24);
        assert_eq!(cur(&t), (6, 39));
        assert_eq!(hist(&t), 0);
    }

    #[test]
    fn widening_back_restores_the_single_row() {
        let mut t = banner_state();
        resize_conpty(&mut t.term, 60, 24);
        resize_conpty(&mut t.term, 40, 24);
        resize_conpty(&mut t.term, 100, 24);
        assert_eq!(cur(&t), (3, 79));
        assert_eq!(row(&t, 3), format!("{P}abcdef"));
        assert_eq!(hist(&t), 0);
        resize_conpty(&mut t.term, 80, 24);
        assert_eq!(cur(&t), (3, 79));
        assert_eq!(row(&t, 3), format!("{P}abcdef"));
        assert_eq!(hist(&t), 0);
    }

    #[test]
    fn a_full_screen_narrowing_keeps_the_cursor_row() {
        let mut t = full_screen_state();
        assert_eq!(cur(&t), (23, 79));
        assert_eq!(hist(&t), 37);
        resize_conpty(&mut t.term, 60, 24);
        assert_eq!(cur(&t), (23, 19));
        assert_eq!(hist(&t), 38);
        assert_eq!(row(&t, 22), &P[..60]);
        assert_eq!(row(&t, 23), "es\\m0\\crates>abcdef");
    }

    #[test]
    fn a_full_screen_widening_moves_the_prompt_up_one_row() {
        let mut t = full_screen_state();
        resize_conpty(&mut t.term, 60, 24);
        resize_conpty(&mut t.term, 100, 24);
        assert_eq!(cur(&t), (22, 79));
        assert_eq!(hist(&t), 38);
        assert_eq!(row(&t, 21), "line60");
        assert_eq!(row(&t, 22), format!("{P}abcdef"));
        assert_eq!(row(&t, 23), "");
    }

    #[test]
    fn growing_rows_does_not_pull_history_down() {
        let mut t = full_screen_state();
        resize_conpty(&mut t.term, 80, 30);
        assert_eq!(cur(&t), (23, 79));
        assert_eq!(hist(&t), 37);
        assert_eq!(row(&t, 23), format!("{P}abcdef"));
        for l in 24..30 {
            assert_eq!(row(&t, l), "");
        }
    }

    #[test]
    fn shrinking_rows_scrolls_the_top_into_history() {
        let mut t = full_screen_state();
        resize_conpty(&mut t.term, 80, 30);
        resize_conpty(&mut t.term, 80, 18);
        assert_eq!(cur(&t), (17, 79));
        assert_eq!(hist(&t), 43);
        assert_eq!(row(&t, 17), format!("{P}abcdef"));
    }

    #[test]
    fn the_alternate_screen_is_left_to_a_plain_resize() {
        let mut t = make(80, 24);
        feed(&mut t, "\x1b[?1049hX");
        resize_conpty(&mut t.term, 60, 24);
        assert!(t.term.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(hist(&t), 0);
    }

    #[test]
    fn text_below_the_cursor_skips_the_correction() {
        let script = "a\r\nb\r\nc\x1b[2;1H";
        let mut a = make(80, 24);
        feed(&mut a, script);
        let mut b = make(80, 24);
        feed(&mut b, script);
        resize_conpty(&mut a.term, 40, 24);
        b.term.resize(Dims { cols: 40, rows: 24 });
        assert_eq!(cur(&a), cur(&b));
        assert_eq!(hist(&a), hist(&b));
    }

    #[test]
    fn an_unchanged_size_is_a_no_op() {
        let mut t = banner_state();
        let before = (cur(&t), hist(&t));
        resize_conpty(&mut t.term, 80, 24);
        assert_eq!((cur(&t), hist(&t)), before);
    }

    #[test]
    fn the_emulator_resize_uses_the_correction() {
        let mut e = Emulator::new(80, 24);
        e.feed(format!("{H1}\r\n{H2}\r\n\r\n{P}abcdef").as_bytes());
        e.resize(60, 24);
        assert_eq!(e.cursor(), (4, 19));
        assert_eq!(e.history_size(), 0);
    }
}
