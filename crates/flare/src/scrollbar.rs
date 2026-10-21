//! The terminal's scrollbar (M10 D11, parity M1 line 36): where the thumb sits on the 6 px strip
//! `ui/scrollbar.slint` draws. Pure geometry here; `place` hands the front shell's history to the
//! window after every rendered frame.

use crate::AppWindow;
use crate::window::FrontCell;

/// The shortest thumb, in css px, so a long history still leaves something to grab and see.
pub const MIN_THUMB: f32 = 24.0;

/// The thumb as fractions of the track: where it starts and how long it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thumb {
    pub top: f32,
    pub len: f32,
}

/// The thumb for a shell with `history` rows above the `rows` on screen, `offset` rows scrolled
/// back, on a track `track_css` tall. `None` when there is nothing to scroll (no history, the
/// alternate screen) or no track.
pub fn thumb(history: usize, rows: usize, offset: usize, track_css: f32) -> Option<Thumb> {
    if history == 0 || rows == 0 || track_css <= 0.0 {
        return None;
    }
    let total = (history + rows) as f32;
    let len = (rows as f32 / total).max(MIN_THUMB / track_css).min(1.0);
    let back = offset.min(history);
    let top = (history - back) as f32 / history as f32 * (1.0 - len);
    Some(Thumb { top, len })
}

/// Hands the front shell's scroll state to the window.
pub fn place(app: &AppWindow, front: &FrontCell) {
    let Some(handle) = front.borrow().clone() else {
        app.set_scroll_on(false);
        return;
    };
    let (history, rows, offset) =
        handle.with_emu_quiet(|e| (e.history_size(), e.rows(), e.display_offset()));
    let scale = app.get_scale().max(0.01);
    let track = app.get_grid_phys_height() as f32 / scale;
    match thumb(history, rows, offset, track) {
        Some(t) => {
            app.set_scroll_top(t.top);
            app.set_scroll_len(t.len);
            app.set_scroll_back(offset > 0);
            app.set_scroll_on(true);
        }
        None => app.set_scroll_on(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScrollbarPreview;
    use crate::Theme;
    use crate::ui_harness::{Frame, Harness, SCALES};
    use slint::ComponentHandle;

    #[test]
    fn no_history_or_no_track_draws_no_thumb() {
        assert_eq!(thumb(0, 30, 0, 400.0), None);
        assert_eq!(thumb(100, 0, 0, 400.0), None);
        assert_eq!(thumb(100, 30, 0, 0.0), None);
    }

    #[test]
    fn the_thumb_rests_at_the_bottom_live_and_the_top_fully_back() {
        let live = thumb(270, 30, 0, 600.0).unwrap();
        assert!((live.top + live.len - 1.0).abs() < 1e-5, "{live:?}");
        assert!((live.len - 0.1).abs() < 1e-5);
        let top = thumb(270, 30, 270, 600.0).unwrap();
        assert_eq!(top.top, 0.0);
        let over = thumb(270, 30, 9999, 600.0).unwrap();
        assert_eq!(over.top, 0.0);
    }

    #[test]
    fn a_long_history_keeps_a_thumb_you_can_see() {
        let t = thumb(1_000_000, 30, 0, 480.0).unwrap();
        assert!((t.len * 480.0 - MIN_THUMB).abs() < 1e-3);
    }

    fn bar(scale: f32) -> Harness<ScrollbarPreview> {
        Harness::new(scale, 400, 300, || ScrollbarPreview::new().unwrap())
    }

    fn lum(p: [u8; 4]) -> f32 {
        0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])
    }

    #[test]
    fn the_thumb_is_six_css_px_wide_on_whole_device_pixels_at_every_scale() {
        for s in SCALES {
            let h = bar(s);
            h.advance(0);
            let f = h.render();
            let mid = f.h / 2;
            let ground = f.px(0, mid);
            let (x0, _, x1, _) = f
                .bbox_where(0, 0, f.w, f.h, |p| !Frame::near(p, ground, 2))
                .unwrap_or_else(|| panic!("scale {s}: no thumb drawn"));
            let want = (6.0 * s).round() as u32;
            assert_eq!(x1 + 1 - x0, want, "scale {s}");
            assert_eq!(x1 + 1, f.w, "scale {s}: flush with the right edge");
        }
    }

    #[test]
    fn the_thumb_is_lighter_than_the_ground_on_dark_and_darker_on_light() {
        let dark = bar(2.25);
        dark.advance(0);
        let f = dark.render();
        let ground = lum(f.px(10, f.h / 2));
        let thumb_l = lum(f.px(f.w - 4, f.h / 2));
        assert!(thumb_l - ground > 25.0, "dark: {thumb_l} vs {ground}");

        let light = bar(2.25);
        let t = light.comp.global::<Theme>();
        t.set_ground(slint::Color::from_rgb_u8(0xe6, 0xd8, 0xc0));
        t.set_text(slint::Color::from_rgb_u8(0x2a, 0x24, 0x18));
        light.advance(0);
        let f = light.render();
        let ground = lum(f.px(10, f.h / 2));
        let thumb_l = lum(f.px(f.w - 4, f.h / 2));
        assert!(ground - thumb_l > 25.0, "light: {thumb_l} vs {ground}");
    }

    #[test]
    fn the_thumb_is_rounded_and_darker_when_the_view_is_back() {
        let h = bar(1.0);
        h.advance(0);
        let live = h.render();
        let corner = lum(live.px(live.w - 6, 75));
        let side = lum(live.px(live.w - 3, 150));
        assert!(side > corner + 10.0, "a 3 px radius rounds the corner");
        h.comp.set_back(true);
        h.advance(0);
        let back = h.render();
        assert!(
            lum(back.px(back.w - 3, 150)) > side,
            "65 percent is stronger than 40"
        );
    }
}
