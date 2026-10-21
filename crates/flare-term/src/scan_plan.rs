//! The sliced scan plan for find and links: how a walk over a buffer stays
//! inside a time budget (rule 16; Prism Terminal's `linkScanPlan`, #167).
//!
//! A full pass paints the live screen at once (at most `rows` lines), then the
//! scrollback bottom up, in slices that stop once `budget_ms` is spent. The
//! budget covers the visiting too, so the work the caller does per line is
//! inside the slice.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The time budget of one slice, in milliseconds.
pub const SLICE_MS: f64 = 8.0;

/// The live screen's lines as `(first, last)`: `base_y` to `base_y + rows - 1`,
/// stopping at the last line of a buffer shorter than the screen. A zero-length
/// buffer gives `(0, 0)`; callers never ask for one.
pub fn live_range(base_y: usize, rows: usize, length: usize) -> (usize, usize) {
    if length == 0 {
        return (0, 0);
    }
    let last = (base_y + rows.saturating_sub(1)).min(length - 1);
    (base_y, last)
}

/// A walk from `to` down to `from`, handed out one slice at a time.
pub struct RowPlan {
    at: i64,
    from: i64,
    cancelled: Arc<AtomicBool>,
    budget_ms: f64,
    now: Box<dyn Fn() -> f64>,
}

/// Lines `from` to `to`, walked from `to` up, `budget_ms` at a time by `now`.
pub fn slice_rows(from: usize, to: usize, budget_ms: f64, now: Box<dyn Fn() -> f64>) -> RowPlan {
    RowPlan {
        at: to as i64,
        from: from as i64,
        cancelled: Arc::new(AtomicBool::new(false)),
        budget_ms,
        now,
    }
}

impl RowPlan {
    /// The next slice, walked from the bottom up: `visit` is called for each row
    /// and the clock is read AFTER each visit; a slice ends on the first row that
    /// finds the budget spent and always holds at least one row. `resume` moves
    /// the walk when the buffer trimmed rows. `None` when done or cancelled.
    pub fn next(
        &mut self,
        mut visit: Option<&mut dyn FnMut(usize)>,
        resume: Option<usize>,
    ) -> Option<Vec<usize>> {
        if let Some(r) = resume {
            self.at = r as i64;
        }
        if self.is_cancelled() || self.at < self.from {
            return None;
        }
        let mut out = Vec::new();
        let start = (self.now)();
        while self.at >= self.from {
            let row = self.at as usize;
            self.at -= 1;
            out.push(row);
            if let Some(v) = visit.as_mut() {
                v(row);
            }
            if self.is_cancelled() || (self.now)() - start >= self.budget_ms {
                break;
            }
        }
        Some(out)
    }

    /// Stops the walk: no slice is handed out after this, even mid-slice.
    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// A handle that cancels this walk from inside a visit (the walk is borrowed
    /// mutably while a slice runs, so a visit cannot call `cancel` itself).
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::rc::Rc;

    /// A clock that moves only when a row is painted: 1 ms of work per row.
    fn work_clock() -> (Rc<Cell<f64>>, Box<dyn Fn() -> f64>) {
        let t = Rc::new(Cell::new(0.0));
        let read = Rc::clone(&t);
        (t, Box::new(move || read.get()))
    }

    #[test]
    fn live_range_is_the_screen() {
        assert_eq!(live_range(9970, 30, 10000), (9970, 9999));
    }

    #[test]
    fn live_range_stops_at_a_short_buffer() {
        assert_eq!(live_range(0, 30, 12), (0, 11));
    }

    #[test]
    fn walks_a_10000_line_backlog_bottom_up_in_slices_of_at_most_8_ms() {
        let (t, now) = work_clock();
        let mut plan = slice_rows(0, 9999, SLICE_MS, now);
        let mut painted: Vec<usize> = Vec::new();
        let mut slices = 0;
        loop {
            let mut paint = |row: usize| {
                painted.push(row);
                t.set(t.get() + 1.0);
            };
            let Some(slice) = plan.next(Some(&mut paint), None) else {
                break;
            };
            slices += 1;
            assert!(!slice.is_empty());
            assert!(slice.len() <= 8);
        }
        assert_eq!(painted.len(), 10000);
        let distinct: HashSet<usize> = painted.iter().copied().collect();
        assert_eq!(distinct.len(), 10000);
        assert_eq!(painted[0], 9999);
        assert_eq!(painted[painted.len() - 1], 0);
        assert!(painted.windows(2).all(|w| w[1] + 1 == w[0]));
        assert_eq!(slices, 1250);
    }

    #[test]
    fn the_budget_covers_the_visiting_not_only_the_walk() {
        let (t, now) = work_clock();
        let mut plan = slice_rows(0, 99, SLICE_MS, now);
        let mut paint = |_: usize| t.set(t.get() + 1.0);
        assert_eq!(plan.next(Some(&mut paint), None).map(|s| s.len()), Some(8));
    }

    #[test]
    fn cheap_rows_all_go_in_one_slice() {
        let mut plan = slice_rows(10, 19, SLICE_MS, Box::new(|| 0.0));
        assert_eq!(
            plan.next(None, None),
            Some(vec![19, 18, 17, 16, 15, 14, 13, 12, 11, 10])
        );
        assert_eq!(plan.next(None, None), None);
    }

    #[test]
    fn an_empty_range_yields_nothing() {
        let mut plan = slice_rows(5, 4, SLICE_MS, Box::new(|| 0.0));
        assert_eq!(plan.next(None, None), None);
    }

    #[test]
    fn resumes_where_it_is_told_when_the_buffer_trimmed_rows() {
        let (t, now) = work_clock();
        let mut plan = slice_rows(0, 99, SLICE_MS, now);
        let mut paint = |_: usize| t.set(t.get() + 1.0);
        assert_eq!(plan.next(Some(&mut paint), None).map(|s| s[0]), Some(99));
        // 50 rows trimmed off the top: the walk stood on 91, it now stands on 41.
        let resumed = plan.next(Some(&mut paint), Some(41)).unwrap();
        assert_eq!(&resumed[..2], &[41, 40]);
    }

    #[test]
    fn a_cancelled_plan_yields_nothing_more_even_mid_slice() {
        let (t, now) = work_clock();
        let mut plan = slice_rows(0, 99, SLICE_MS, now);
        let mut paint = |_: usize| t.set(t.get() + 1.0);
        assert!(plan.next(Some(&mut paint), None).is_some());
        plan.cancel();
        assert_eq!(plan.next(Some(&mut paint), None), None);

        // Cancelled from inside a visit: the slice stops on that row.
        let mut other = slice_rows(0, 99, SLICE_MS, Box::new(|| 0.0));
        let flag = other.cancel_flag();
        let mut stop_now = |_: usize| flag.store(true, Ordering::SeqCst);
        assert_eq!(other.next(Some(&mut stop_now), None), Some(vec![99]));
        assert_eq!(other.next(None, None), None);
    }
}
