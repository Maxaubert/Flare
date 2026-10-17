//! edge_for and the window border applier (Task 18).
//!
//! Port of Prism Terminal's `windowEdge.ts`. Time is passed in: the caller owns a timer that
//! calls `tick` at `next_deadline`.

use crate::dwm::Border;
use crate::material::{is_light_bg, valid_bg};
use flare_theme::edges::{WindowEdges, edge_border_step};
use flare_theme::ground::DEFAULT_GROUND;
use flare_theme::num::js_round;
use std::time::{Duration, Instant};

/// The window border colour for a ground: `None` means no border, else `#rrggbb`.
pub fn edge_for(bg: &str, edges: WindowEdges) -> Option<String> {
    let steps = edge_border_step(edges)?;
    let hex = if valid_bg(bg) { bg } else { DEFAULT_GROUND };
    let light = is_light_bg(hex);
    let to = if light { 0.0 } else { 255.0 };
    let step = if light { steps.light } else { steps.dark };
    let n = u32::from_str_radix(&hex[1..], 16).unwrap_or(0);
    let ch = |shift: u32| -> u32 {
        let c = f64::from((n >> shift) & 255);
        js_round(c + (to - c) * step).clamp(0.0, 255.0) as u32
    };
    let packed = (ch(16) << 16) | (ch(8) << 8) | ch(0);
    Some(format!("#{packed:06x}"))
}

pub trait EdgeTarget {
    fn set_border(&mut self, b: &Border);
    /// Maximised or full screen.
    fn is_flat(&self) -> bool;
}

pub struct EdgeApplier<T: EdgeTarget> {
    target: T,
    bg: Box<dyn Fn() -> String>,
    edges: Box<dyn Fn() -> WindowEdges>,
    enabled: bool,
    due: Option<Instant>,
}

impl<T: EdgeTarget> EdgeApplier<T> {
    pub const DELAY: Duration = Duration::from_millis(250);

    pub fn new(
        target: T,
        bg: impl Fn() -> String + 'static,
        edges: impl Fn() -> WindowEdges + 'static,
        enabled: bool,
    ) -> Self {
        Self {
            target,
            bg: Box::new(bg),
            edges: Box::new(edges),
            enabled,
            due: None,
        }
    }

    /// Restarts the wait; a no-op when disabled.
    pub fn apply(&mut self, now: Instant) {
        if self.enabled {
            self.due = Some(now + Self::DELAY);
        }
    }

    pub fn tick(&mut self, now: Instant) {
        let Some(due) = self.due else { return };
        if due > now {
            return;
        }
        self.due = None;
        let border = if self.target.is_flat() {
            Border::None
        } else {
            match edge_for(&(self.bg)(), (self.edges)()) {
                Some(hex) => Border::Colour(hex),
                None => Border::None,
            }
        };
        self.target.set_border(&border);
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.due
    }

    pub fn target(&mut self) -> &mut T {
        &mut self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_theme::edges::WINDOW_EDGES;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    fn luma(hex: &str) -> f64 {
        let n = u32::from_str_radix(&hex[1..], 16).unwrap();
        0.2126 * f64::from((n >> 16) & 255)
            + 0.7152 * f64::from((n >> 8) & 255)
            + 0.0722 * f64::from(n & 255)
    }

    fn off(bg: &str, e: WindowEdges) -> f64 {
        (luma(&edge_for(bg, e).unwrap()) - luma(bg)).abs()
    }

    #[test]
    fn a_dark_ground_gets_a_lighter_edge() {
        let out = edge_for("#0b0b0f", WindowEdges::Hairline).unwrap();
        assert!(luma(&out) > luma("#0b0b0f"));
        assert!(edge_border_step(WindowEdges::Hairline).unwrap().dark < 0.2);
    }

    #[test]
    fn a_light_ground_gets_a_darker_edge() {
        let out = edge_for("#f4f4f4", WindowEdges::Hairline).unwrap();
        assert!(luma(&out) < luma("#f4f4f4"));
        assert!(edge_border_step(WindowEdges::Hairline).unwrap().light < 0.25);
    }

    #[test]
    fn edge_for_answers_a_hex_for_anything() {
        let out = edge_for("javascript:1", WindowEdges::Hairline).unwrap();
        assert_eq!(out.len(), 7);
        assert!(out.starts_with('#'));
        assert!(
            out[1..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }

    #[test]
    fn a_hairline_is_the_border_as_it_always_was() {
        assert_eq!(
            edge_for("#0b0b0f", WindowEdges::Hairline).unwrap(),
            "#2b2b2e"
        );
        assert_eq!(
            edge_for("#f4f4f4", WindowEdges::Hairline).unwrap(),
            "#bebebe"
        );
    }

    #[test]
    fn faint_stands_off_less_than_a_hairline_and_solid_more() {
        for bg in ["#0b0b0f", "#f4f4f4"] {
            let f = off(bg, WindowEdges::Faint);
            let h = off(bg, WindowEdges::Hairline);
            let s = off(bg, WindowEdges::Solid);
            assert!(f > 0.0 && f < h && h < s, "{bg}: {f} {h} {s}");
        }
    }

    #[test]
    fn no_edges_is_no_border_whatever_the_ground() {
        for bg in ["#0b0b0f", "#f4f4f4", "javascript:1"] {
            assert_eq!(edge_for(bg, WindowEdges::None), None);
        }
    }

    #[test]
    fn oracle_edge_vectors() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../flare-theme/tests/fixtures/edge_vectors.json");
        let text = std::fs::read_to_string(path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let rows = v.as_array().unwrap();
        assert!(rows.len() >= 28);
        for r in rows {
            let bg = r["bg"].as_str().unwrap();
            let word = r["edges"].as_str().unwrap();
            let want = r["out"].as_str().unwrap();
            let e = WINDOW_EDGES
                .iter()
                .copied()
                .find(|e| e.word() == word)
                .unwrap();
            let got = edge_for(bg, e);
            let want = if want == "none" {
                None
            } else {
                Some(want.to_string())
            };
            assert_eq!(got, want, "{bg} {word}");
        }
    }

    struct Fake {
        log: Rc<RefCell<Vec<Border>>>,
        flat: Rc<Cell<bool>>,
    }
    impl EdgeTarget for Fake {
        fn set_border(&mut self, b: &Border) {
            self.log.borrow_mut().push(b.clone());
        }
        fn is_flat(&self) -> bool {
            self.flat.get()
        }
    }

    type Log = Rc<RefCell<Vec<Border>>>;

    fn applier(enabled: bool) -> (EdgeApplier<Fake>, Log, Rc<Cell<bool>>) {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let flat = Rc::new(Cell::new(false));
        let a = EdgeApplier::new(
            Fake {
                log: log.clone(),
                flat: flat.clone(),
            },
            || "#0b0b0f".to_string(),
            || WindowEdges::Hairline,
            enabled,
        );
        (a, log, flat)
    }

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn apply_is_debounced_250_ms() {
        let (mut a, log, _) = applier(true);
        let t0 = Instant::now();
        a.apply(t0);
        a.tick(ms(t0, 249));
        assert!(log.borrow().is_empty());
        a.tick(ms(t0, 250));
        assert_eq!(*log.borrow(), vec![Border::Colour("#2b2b2e".into())]);
        assert_eq!(a.next_deadline(), None);
    }

    #[test]
    fn a_new_apply_restarts_the_wait() {
        let (mut a, log, _) = applier(true);
        let t0 = Instant::now();
        a.apply(t0);
        a.apply(ms(t0, 200));
        a.tick(ms(t0, 300));
        assert!(log.borrow().is_empty());
        a.tick(ms(t0, 450));
        assert_eq!(log.borrow().len(), 1);
    }

    #[test]
    fn maximised_or_full_screen_has_no_border() {
        let (mut a, log, flat) = applier(true);
        flat.set(true);
        let t0 = Instant::now();
        a.apply(t0);
        a.tick(ms(t0, 250));
        assert_eq!(*log.borrow(), vec![Border::None]);
    }

    #[test]
    fn disabled_makes_no_call() {
        let (mut a, log, _) = applier(false);
        let t0 = Instant::now();
        a.apply(t0);
        assert_eq!(a.next_deadline(), None);
        a.tick(ms(t0, 1000));
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn tick_before_the_deadline_does_nothing() {
        let (mut a, log, _) = applier(true);
        let t0 = Instant::now();
        a.tick(t0);
        a.apply(t0);
        a.tick(ms(t0, 100));
        assert!(log.borrow().is_empty());
        assert_eq!(a.next_deadline(), Some(ms(t0, 250)));
    }
}
