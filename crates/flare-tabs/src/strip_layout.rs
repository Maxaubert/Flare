//! layout_strip: device-pixel slots for the plan (Task 6).

use crate::strip_plan::{ICON_TAB, MORE_BUTTON, StripSlot, TAB_FLOOR};

/// A named tab's flex basis (its ceiling).
pub const TAB_BASIS: f64 = 150.0;
pub const STRIP_PAD_RIGHT: f64 = 4.0;
pub const PLUS_W: f64 = 28.0;

/// Device pixels from the strip's left edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub x: i32,
    pub w: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StripLayout {
    pub slots: Vec<Option<Slot>>,
    pub more: Option<Slot>,
    pub plus: Slot,
}

/// The room the plan gets: the strip, less 4, the + and 4.
pub fn room_for_plan(strip_css: f64) -> f64 {
    strip_css - 4.0 - PLUS_W - 4.0
}

/// Flex layout of the row in CSS pixels, snapped to device pixels. Every edge is
/// `round(edge_css * scale)`, and a slot is `[edge_i, edge_i+1)`: edges are whole device
/// pixels, slots touch, and widths differ by at most one pixel.
pub fn layout_strip(plan: &[StripSlot], strip_phys: i32, scale: f64) -> StripLayout {
    layout_strip_with(plan, strip_phys, scale, MORE_BUTTON)
}

/// `layout_strip` with the +N button `more_css` wide (`strip_plan::more_button_css`).
pub fn layout_strip_with(
    plan: &[StripSlot],
    strip_phys: i32,
    scale: f64,
    more_css: f64,
) -> StripLayout {
    let scale = if scale.is_nan() || scale <= 0.0 {
        1.0
    } else {
        scale
    };
    let strip_css = f64::from(strip_phys) / scale;
    let inner = strip_css - STRIP_PAD_RIGHT;
    let n_full = plan.iter().filter(|s| **s == StripSlot::Full).count();
    let n_icon = plan.iter().filter(|s| **s == StripSlot::Icon).count();
    let any_hidden = plan.contains(&StripSlot::Hidden);
    let fixed = ICON_TAB * n_icon as f64 + if any_hidden { more_css } else { 0.0 } + PLUS_W;
    let full_w = if n_full == 0 {
        TAB_FLOOR
    } else {
        ((inner - fixed) / n_full as f64).clamp(TAB_FLOOR, TAB_BASIS)
    };
    let edge = |css: f64| (css * scale).round() as i32;
    let (mut fulls, mut icons) = (0usize, 0usize);
    let pos = |fulls: usize, icons: usize| full_w * fulls as f64 + ICON_TAB * icons as f64;
    let mut slots = Vec::with_capacity(plan.len());
    for s in plan {
        let start = pos(fulls, icons);
        match s {
            StripSlot::Full => fulls += 1,
            StripSlot::Icon => icons += 1,
            StripSlot::Hidden => {
                slots.push(None);
                continue;
            }
        }
        let (a, b) = (edge(start), edge(pos(fulls, icons)));
        slots.push(Some(Slot { x: a, w: b - a }));
    }
    let mut cursor = pos(fulls, icons);
    let more = if any_hidden {
        let (a, b) = (edge(cursor), edge(cursor + more_css));
        cursor += more_css;
        Some(Slot { x: a, w: b - a })
    } else {
        None
    };
    let (a, b) = (edge(cursor), edge(cursor + PLUS_W));
    StripLayout {
        slots,
        more,
        plus: Slot { x: a, w: b - a },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use StripSlot::{Full, Hidden, Icon};

    fn sl(x: i32, w: i32) -> Option<Slot> {
        Some(Slot { x, w })
    }

    fn widths(l: &StripLayout) -> Vec<i32> {
        l.slots.iter().flatten().map(|s| s.w).collect()
    }

    #[test]
    fn room_for_plan_subtracts_four_the_plus_and_four() {
        assert_eq!(room_for_plan(1000.0), 964.0);
    }

    #[test]
    fn wide_strip_gives_every_tab_150() {
        let l = layout_strip(&[Full, Full, Full], 1000, 1.0);
        assert_eq!(l.slots, vec![sl(0, 150), sl(150, 150), sl(300, 150)]);
        assert_eq!(l.more, None);
        assert_eq!(l.plus, Slot { x: 450, w: 28 });
    }

    #[test]
    fn at_225_edges_round_to_device_pixels() {
        let l = layout_strip(&[Full, Full, Full], 2250, 2.25);
        assert_eq!(l.slots, vec![sl(0, 338), sl(338, 337), sl(675, 338)]);
        assert_eq!(l.plus, Slot { x: 1013, w: 63 });
    }

    #[test]
    fn tabs_shrink_equally_when_the_room_is_short() {
        let l = layout_strip(&[Full; 10], 1000, 1.0);
        let w = widths(&l);
        assert!(w.iter().all(|w| *w == 96 || *w == 97));
        assert_eq!(w.iter().sum::<i32>(), 968);
        assert_eq!(l.plus, Slot { x: 968, w: 28 });
    }

    #[test]
    fn the_floor_is_84() {
        let l = layout_strip(&[Full; 12], 500, 1.0);
        assert!(widths(&l).iter().all(|w| *w == 84));
    }

    #[test]
    fn icons_are_40_and_names_take_the_rest_up_to_150() {
        let l = layout_strip(&[Full, Icon, Icon], 600, 1.0);
        assert_eq!(l.slots, vec![sl(0, 150), sl(150, 40), sl(190, 40)]);
        assert_eq!(l.plus, Slot { x: 230, w: 28 });
        let l = layout_strip(&[Full, Icon, Hidden, Hidden], 600, 1.0);
        assert_eq!(l.slots, vec![sl(0, 150), sl(150, 40), None, None]);
        assert_eq!(l.more, sl(190, 72));
        assert_eq!(l.plus, Slot { x: 262, w: 28 });
    }

    #[test]
    fn an_empty_plan_has_only_the_plus() {
        let l = layout_strip(&[], 800, 1.0);
        assert!(l.slots.is_empty());
        assert_eq!(l.more, None);
        assert_eq!(l.plus, Slot { x: 0, w: 28 });
    }

    #[test]
    fn whole_pixels_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0] {
            let strip = (1000.0_f64 * scale).round() as i32;
            let l = layout_strip(&[Full; 7], strip, scale);
            let s: Vec<Slot> = l.slots.iter().map(|s| s.unwrap()).collect();
            assert_eq!(s[0].x, 0);
            for i in 0..6 {
                assert_eq!(s[i + 1].x, s[i].x + s[i].w, "scale {scale}");
            }
            let ideal = 138.2857 * scale;
            for t in &s {
                let w = f64::from(t.w);
                assert!(
                    w == ideal.floor() || w == ideal.ceil(),
                    "scale {scale} w {w}"
                );
            }
            let mn = s.iter().map(|t| t.w).min().unwrap();
            let mx = s.iter().map(|t| t.w).max().unwrap();
            assert!(mx - mn <= 1);
            assert_eq!(l.plus.x, s[6].x + s[6].w);
            assert!(l.plus.x + l.plus.w <= strip);
        }
    }
}
