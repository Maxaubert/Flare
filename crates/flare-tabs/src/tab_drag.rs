//! TabDrag: the carry state machine (Task 7). Source: `useTabReorder.ts`.

use crate::tab_drop::{Lane, drop_slot};

/// The pointer must travel this far before a press becomes a carry.
pub const DRAG_SLOP: f64 = 4.0;
/// How long the other tabs take to slide into the gap, in milliseconds.
pub const CARRY_SLIDE_MS: u32 = 170;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Carry {
    pub id: String,
    pub from: usize,
    pub size: f64,
    pub d: f64,
    pub live: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Release {
    Nothing,
    Pick,
    Drop { id: String, slot: usize },
}

pub struct TabDrag {
    axis: Axis,
    lanes: Vec<Lane>,
    start: f64,
    container: (f64, f64),
    carry: Option<Carry>,
    drop_at: Option<usize>,
}

impl TabDrag {
    pub fn new(axis: Axis) -> Self {
        Self {
            axis,
            lanes: Vec::new(),
            start: 0.0,
            container: (0.0, 0.0),
            carry: None,
            drop_at: None,
        }
    }

    pub fn axis(&self) -> Axis {
        self.axis
    }

    /// Starts a carry. `lanes` are frozen now; `container` is the row's (low, high) along the
    /// axis. `refuse` (the sidebar while Find a tab filters) starts nothing and returns false.
    pub fn press(
        &mut self,
        id: &str,
        index: usize,
        along: f64,
        lanes: Vec<Lane>,
        container: (f64, f64),
        refuse: bool,
    ) -> bool {
        if refuse {
            return false;
        }
        let Some(lane) = lanes.get(index) else {
            return false;
        };
        self.carry = Some(Carry {
            id: id.to_string(),
            from: index,
            size: lane.width,
            d: 0.0,
            live: false,
        });
        self.start = along;
        self.container = container;
        self.lanes = lanes;
        self.drop_at = Some(index);
        true
    }

    pub fn moved(&mut self, along: f64) {
        let Some(carry) = self.carry.as_mut() else {
            return;
        };
        let raw = along - self.start;
        if !carry.live && raw.abs() < DRAG_SLOP {
            return;
        }
        let lane = self.lanes[carry.from];
        let low = self.container.0 - lane.left;
        let high = self.container.1 - (lane.left + lane.width);
        // A container narrower than the tab would invert the range; the low side wins.
        carry.d = raw.min(high).max(low);
        carry.live = true;
        self.drop_at = Some(drop_slot(&self.lanes, carry.from, carry.d));
    }

    pub fn release(&mut self) -> Release {
        let Some(carry) = self.carry.take() else {
            return Release::Nothing;
        };
        let at = self.drop_at.take();
        if !carry.live {
            return Release::Pick;
        }
        Release::Drop {
            id: carry.id,
            slot: at.unwrap_or(carry.from),
        }
    }

    pub fn cancel(&mut self) {
        self.carry = None;
        self.drop_at = None;
    }

    pub fn carry(&self) -> Option<&Carry> {
        self.carry.as_ref()
    }

    pub fn drop_at(&self) -> Option<usize> {
        self.drop_at
    }

    /// How far tab `i` slides to open the gap.
    pub fn slide(&self, i: usize) -> f64 {
        let (Some(carry), Some(at)) = (self.carry.as_ref(), self.drop_at) else {
            return 0.0;
        };
        if !carry.live || i == carry.from {
            return 0.0;
        }
        if at > carry.from && i > carry.from && i < at {
            -carry.size
        } else if at <= carry.from && i >= at && i < carry.from {
            carry.size
        } else {
            0.0
        }
    }

    /// The carried tab: `d`; every other: `slide(i)`.
    pub fn offset(&self, id: &str, i: usize) -> f64 {
        match self.carry.as_ref() {
            Some(c) if c.id == id => c.d,
            _ => self.slide(i),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab_drop::lanes;

    fn l3() -> Vec<Lane> {
        lanes(&[100.0, 100.0, 100.0])
    }

    #[test]
    fn a_press_that_does_not_travel_is_a_pick() {
        let mut d = TabDrag::new(Axis::X);
        d.press("b", 1, 150.0, l3(), (0.0, 300.0), false);
        d.moved(152.0);
        assert!(!d.carry().unwrap().live);
        assert_eq!(d.release(), Release::Pick);
    }

    #[test]
    fn the_slop_is_four_pixels_exactly() {
        let mut d = TabDrag::new(Axis::X);
        d.press("b", 1, 150.0, l3(), (0.0, 300.0), false);
        d.moved(153.9);
        assert!(!d.carry().unwrap().live);
        d.moved(154.0);
        let c = d.carry().unwrap();
        assert!(c.live);
        assert_eq!(c.d, 4.0);
    }

    #[test]
    fn the_carried_tab_is_clamped_to_its_row() {
        let mut d = TabDrag::new(Axis::X);
        d.press("a", 0, 50.0, l3(), (0.0, 300.0), false);
        d.moved(-500.0);
        assert_eq!(d.carry().unwrap().d, 0.0);
        d.moved(1000.0);
        assert_eq!(d.carry().unwrap().d, 200.0);
    }

    #[test]
    fn the_drop_slot_is_the_leading_edge() {
        let mut d = TabDrag::new(Axis::X);
        d.press(
            "b",
            1,
            150.0,
            lanes(&[90.0, 120.0, 100.0]),
            (0.0, 310.0),
            false,
        );
        d.moved(60.0);
        assert_eq!(d.carry().unwrap().d, -90.0);
        assert_eq!(
            d.release(),
            Release::Drop {
                id: "b".into(),
                slot: 0
            }
        );
    }

    #[test]
    fn neighbours_slide_to_open_the_gap() {
        let mut d = TabDrag::new(Axis::X);
        d.press("a", 0, 50.0, l3(), (0.0, 300.0), false);
        d.moved(110.0);
        assert_eq!(d.drop_at(), Some(2));
        assert_eq!(d.slide(1), -100.0);
        assert_eq!(d.slide(2), 0.0);
        assert_eq!(d.offset("a", 0), 60.0);
        d.cancel();
        d.press("c", 2, 250.0, l3(), (0.0, 300.0), false);
        d.moved(190.0);
        assert_eq!(d.drop_at(), Some(1));
        assert_eq!(d.slide(1), 100.0);
        assert_eq!(d.slide(0), 0.0);
    }

    #[test]
    fn a_refused_press_starts_nothing() {
        let mut d = TabDrag::new(Axis::Y);
        assert!(!d.press("a", 0, 10.0, l3(), (0.0, 300.0), true));
        assert!(d.carry().is_none());
        assert_eq!(d.release(), Release::Nothing);
    }

    #[test]
    fn release_without_a_carry_is_nothing() {
        let mut d = TabDrag::new(Axis::X);
        assert_eq!(d.release(), Release::Nothing);
    }

    #[test]
    fn cancel_drops_everything() {
        let mut d = TabDrag::new(Axis::X);
        d.press("a", 0, 50.0, l3(), (0.0, 300.0), false);
        d.moved(300.0);
        d.cancel();
        assert!(d.carry().is_none());
        assert_eq!(d.release(), Release::Nothing);
    }

    #[test]
    fn a_second_drag_starts_clean() {
        let mut d = TabDrag::new(Axis::X);
        d.press("a", 0, 50.0, l3(), (0.0, 300.0), false);
        d.moved(200.0);
        d.release();
        d.press("b", 1, 150.0, l3(), (0.0, 300.0), false);
        let c = d.carry().unwrap();
        assert_eq!(c.d, 0.0);
        assert!(!c.live);
        assert_eq!(d.drop_at(), Some(1));
    }
}
