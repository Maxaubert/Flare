//! plan_strip: named, icon or hidden tabs for the strip (Task 6).
//!
//! How the top bar fits many tabs (icons first, then the list), in three steps, each only when
//! the one before has run out of room: every tab named; resting tabs become icons (the front tab
//! and tabs that need you keep names, and then those become icons too, newest first); tabs go
//! into the +N list, resting ones first, newest first, then those that need you. The tab in front
//! is never an icon and never hidden. Order never changes.

use std::collections::HashSet;

/// A named tab's floor.
pub const TAB_FLOOR: f64 = 84.0;
/// An icon tab.
pub const ICON_TAB: f64 = 40.0;
/// The +N button, with its count, dots and chevron.
pub const MORE_BUTTON: f64 = 72.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripSlot {
    Full,
    Icon,
    Hidden,
}

fn used_width(slot: &[StripSlot]) -> f64 {
    let mut w = 0.0;
    let mut hidden = false;
    for s in slot {
        match s {
            StripSlot::Full => w += TAB_FLOOR,
            StripSlot::Icon => w += ICON_TAB,
            StripSlot::Hidden => hidden = true,
        }
    }
    if hidden { w + MORE_BUTTON } else { w }
}

/// `needs`: tabs that need you (Finished, Question, Failed); `room`: the strip's width for tabs
/// and the +N button (`f64::INFINITY` allowed).
pub fn plan_strip(
    ids: &[String],
    active_id: Option<&str>,
    needs: &HashSet<String>,
    room: f64,
) -> Vec<StripSlot> {
    let n = ids.len();
    if n as f64 * TAB_FLOOR <= room {
        return vec![StripSlot::Full; n];
    }
    let is_active = |i: usize| Some(ids[i].as_str()) == active_id;
    let mut slot: Vec<StripSlot> = (0..n)
        .map(|i| {
            if is_active(i) || needs.contains(&ids[i]) {
                StripSlot::Full
            } else {
                StripSlot::Icon
            }
        })
        .collect();
    // Newest first: the end of the bar gives way before its start.
    let order: Vec<usize> = (0..n).rev().collect();
    for &i in &order {
        if used_width(&slot) <= room {
            return slot;
        }
        if slot[i] == StripSlot::Full && !is_active(i) {
            slot[i] = StripSlot::Icon;
        }
    }
    for wanted in [false, true] {
        for &i in &order {
            if used_width(&slot) <= room {
                return slot;
            }
            if slot[i] == StripSlot::Icon && needs.contains(&ids[i]) == wanted {
                slot[i] = StripSlot::Hidden;
            }
        }
    }
    slot
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("t{i}")).collect()
    }

    fn set(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_every_tab_while_they_fit_at_the_floor() {
        let plan = plan_strip(&ids(10), Some("t0"), &set(&[]), 840.0);
        assert_eq!(plan.len(), 10);
        assert!(plan.iter().all(|s| *s == StripSlot::Full));
    }

    #[test]
    fn past_the_floor_resting_tabs_become_icons() {
        let plan = plan_strip(&ids(12), Some("t5"), &set(&["t2", "t9"]), 900.0);
        for k in [5, 2, 9] {
            assert_eq!(plan[k], StripSlot::Full);
        }
        assert_eq!(plan.iter().filter(|s| **s == StripSlot::Icon).count(), 9);
        assert!(!plan.contains(&StripSlot::Hidden));
        assert!(used_width(&plan) <= 900.0);
    }

    #[test]
    fn needy_tabs_become_icons_newest_first_before_any_hides() {
        let all = ids(12);
        let needs: HashSet<String> = all.iter().cloned().collect();
        let room = 84.0 * 3.0 + 40.0 * 9.0;
        let plan = plan_strip(&all, Some("t0"), &needs, room);
        for s in &plan[0..3] {
            assert_eq!(*s, StripSlot::Full);
        }
        for s in &plan[3..] {
            assert_eq!(*s, StripSlot::Icon);
        }
    }

    #[test]
    fn then_resting_tabs_go_into_the_list_newest_first() {
        let plan = plan_strip(&ids(40), Some("t0"), &set(&["t39", "t1"]), 1000.0);
        assert!(plan.contains(&StripSlot::Hidden));
        assert_ne!(plan[39], StripSlot::Hidden);
        assert_ne!(plan[1], StripSlot::Hidden);
        let first = plan.iter().position(|s| *s == StripSlot::Hidden).unwrap();
        for k in 0..plan.len() - first {
            assert!(plan[first + k] == StripSlot::Hidden || first + k == 39);
        }
        assert!(used_width(&plan) <= 1000.0);
    }

    #[test]
    fn the_front_tab_is_never_an_icon_or_hidden() {
        for front in [0usize, 20, 39] {
            let id = format!("t{front}");
            let plan = plan_strip(&ids(40), Some(&id), &set(&[]), 300.0);
            assert_eq!(plan[front], StripSlot::Full);
        }
    }

    #[test]
    fn a_bar_too_narrow_still_shows_the_front_tab() {
        let plan = plan_strip(&ids(5), Some("t3"), &set(&["t1"]), 10.0);
        assert_eq!(plan[3], StripSlot::Full);
        for k in [0, 1, 2, 4] {
            assert_eq!(plan[k], StripSlot::Hidden);
        }
    }

    #[test]
    fn no_tabs_no_plan() {
        assert!(plan_strip(&[], None, &set(&[]), 500.0).is_empty());
    }
}
