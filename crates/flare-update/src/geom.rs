//! M9 Task 12: chip, notice and window geometry in logical pixels, snapped to device pixels.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub const CHIP_H: f32 = 24.0;
pub const CHIP_PAD_X: f32 = 10.0;
pub const CHIP_GAP: f32 = 6.0;
pub const CHIP_RADIUS: f32 = 6.0;
pub const CHIP_FONT: f32 = 11.5;
pub const CHIP_ICON: f32 = 11.0;
/// Measured by Task 3 (`tools/pt-metrics/update.json`, `toNextControl`): 10 css px at 100, 150 and 225 percent.
pub const CHIP_TO_BUTTONS: f32 = 10.0;
pub const NOTICE_GAP: f32 = 8.0;
pub const NOTICE_MS: u64 = 8000;
pub const NOTICE_PAD_X: f32 = 10.0;
pub const NOTICE_PAD_Y: f32 = 6.0;
pub const NOTICE_FONT: f32 = 11.5;
pub const NOTICE_RADIUS: f32 = 6.0;
pub const DIALOG_MAX_W: f32 = 460.0;
pub const DIALOG_MARGIN: f32 = 24.0;
pub const DIALOG_PAD: f32 = 20.0;
pub const NOTES_MAX_H: f32 = 300.0;
pub const NOTES_VH: f32 = 0.44;
pub const TRACK_H: f32 = 6.0;
pub const FILL_EASE_MS: u32 = 300;
pub const TRACK_FADE_MS: u32 = 200;

/// Rounds a logical length to the nearest device pixel (halves go up).
pub fn snap(l: f32, scale: f32) -> f32 {
    (l * scale + 0.5).floor() / scale
}

/// The hairline width: one device pixel below 200 percent, a whole number of device pixels above
/// (`max(1, floor(scale))` device pixels, in logical units).
pub fn hair(scale: f32) -> f32 {
    let device_px = if scale < 2.0 { 1.0 } else { scale.floor() };
    device_px / scale
}

/// The chip's rectangle: right edge `CHIP_TO_BUTTONS` left of the window buttons, vertically centred in the bar.
pub fn chip_rect(bar_h: f32, buttons_left: f32, chip_w: f32, scale: f32) -> Rect {
    let right = snap(buttons_left - CHIP_TO_BUTTONS, scale);
    Rect {
        x: right - snap(chip_w, scale),
        y: snap((bar_h - CHIP_H) / 2.0, scale),
        w: snap(chip_w, scale),
        h: snap(CHIP_H, scale),
    }
}

/// The icon's and the label's left edges, from the chip's left edge.
pub fn chip_inner(_chip_w: f32, scale: f32) -> (f32, f32) {
    (
        snap(CHIP_PAD_X, scale),
        snap(CHIP_PAD_X + CHIP_ICON + CHIP_GAP, scale),
    )
}

/// The notice: right edge on the chip's right edge, `NOTICE_GAP` under the chip.
pub fn notice_rect(chip: Rect, w: f32, h: f32, scale: f32) -> Rect {
    let right = chip.x + chip.w;
    let w = snap(w, scale);
    Rect {
        x: right - w,
        y: snap(chip.y + chip.h + NOTICE_GAP, scale),
        w,
        h: snap(h, scale),
    }
}

/// The window box: at most `DIALOG_MAX_W` wide and `DIALOG_MARGIN` inside the window, centred to the pixel.
pub fn dialog_rect(win_w: f32, win_h: f32, content_h: f32, scale: f32) -> Rect {
    let w = DIALOG_MAX_W.min(win_w - DIALOG_MARGIN * 2.0);
    let h = content_h.min(win_h - DIALOG_MARGIN * 2.0);
    let x = ((win_w - w) / 2.0).max(0.0);
    let y = ((win_h - h) / 2.0).max(0.0);
    Rect {
        x: snap(x, scale),
        y: snap(y, scale),
        w: snap(w.max(0.0), scale),
        h: snap(h.max(0.0), scale),
    }
}

/// The release notes' scroll height: the smaller of 44 percent of the window and 300 css px.
pub fn notes_max_h(win_h: f32) -> f32 {
    (NOTES_VH * win_h).min(NOTES_MAX_H)
}

/// The progress fill's width for `pct` percent of the track, on a device pixel, never above the track.
pub fn fill_w(track_w: f32, pct: u8, scale: f32) -> f32 {
    if pct >= 100 {
        return track_w;
    }
    let raw = snap(track_w * f32::from(pct) / 100.0, scale);
    raw.clamp(0.0, track_w)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f32; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn on_device_grid(v: f32, scale: f32) -> bool {
        let d = v * scale;
        (d - d.round()).abs() < 1e-3
    }

    #[test]
    fn snap_rounds_half_up_to_a_device_pixel() {
        assert!(close(snap(10.3, 1.25), 10.4));
        assert!(close(snap(0.5, 1.0), 1.0));
    }

    #[test]
    fn hair_is_one_device_pixel_below_200_percent_and_whole_above() {
        for s in [1.0_f32, 1.25, 1.5, 1.75] {
            assert!(close(hair(s), 1.0 / s), "scale {s}");
        }
        // Above 200 percent the hairline is a whole number of device pixels, never a fraction.
        for s in [2.0_f32, 2.25, 2.5, 3.0] {
            let device = hair(s) * s;
            assert!(
                close(device, device.round()),
                "scale {s} gave {device} device px"
            );
            assert!(device >= 1.0, "scale {s}");
        }
        assert!(close(hair(2.0) * 2.0, 2.0));
        assert!(close(hair(3.0) * 3.0, 3.0));
    }

    #[test]
    fn every_rect_edge_is_a_whole_device_pixel_at_all_eight_scales() {
        for &s in &SCALES {
            for i in 0..40 {
                let win_w = 400.0 + i as f32 * 15.3;
                let win_h = 300.0 + i as f32 * 9.7;
                let chip = chip_rect(36.0, win_w - 40.0 - 3.0, 109.41 + i as f32 * 0.37, s);
                let notice = notice_rect(chip, 198.8, 31.25, s);
                let dialog = dialog_rect(win_w, win_h, 470.56, s);
                for r in [chip, notice, dialog] {
                    for v in [r.x, r.y, r.x + r.w, r.y + r.h] {
                        assert!(
                            on_device_grid(v, s),
                            "scale {s} window {win_w}x{win_h} edge {v}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_chip_height_is_24_css_px_snapped_the_width_is_the_callers_and_constant() {
        for &s in &SCALES {
            let a = chip_rect(36.0, 800.0, 109.41, s);
            let b = chip_rect(36.0, 800.0, 109.41, s);
            assert!(close(a.h, CHIP_H), "scale {s}");
            assert!(close(a.w, b.w), "scale {s}");
            assert!(close(a.w, snap(109.41, s)), "scale {s}");
        }
    }

    #[test]
    fn the_chip_sits_the_measured_10_px_left_of_the_buttons() {
        for &s in &SCALES {
            let chip = chip_rect(36.0, 800.0, 109.41, s);
            assert!(close(chip.x + chip.w, snap(800.0 - 10.0, s)), "scale {s}");
        }
    }

    #[test]
    fn the_icon_and_label_do_not_overlap() {
        for &s in &SCALES {
            let (icon_x, label_x) = chip_inner(109.41, s);
            assert!(label_x - icon_x >= CHIP_ICON, "scale {s}");
        }
    }

    #[test]
    fn the_notice_is_right_aligned_under_the_chip_with_an_8_px_gap() {
        for &s in &SCALES {
            let chip = chip_rect(36.0, 800.0, 109.41, s);
            let n = notice_rect(chip, 198.8, 31.25, s);
            assert!(close(n.x + n.w, chip.x + chip.w), "scale {s}");
            assert!(close(n.y, snap(chip.y + chip.h + 8.0, s)), "scale {s}");
        }
    }

    #[test]
    fn the_dialog_is_centred_to_the_pixel_and_capped() {
        for &s in &SCALES {
            let wide = dialog_rect(1000.0, 640.0, 470.0, s);
            assert!(close(wide.w, 460.0), "scale {s}");
            assert!(
                (wide.x - (1000.0 - wide.w) / 2.0).abs() <= 1.0 / s + 1e-3,
                "scale {s}"
            );
            let narrow = dialog_rect(400.0, 640.0, 470.0, s);
            assert!(close(narrow.w, 352.0), "scale {s}");
            assert!(
                (narrow.x - (400.0 - narrow.w) / 2.0).abs() <= 1.0 / s + 1e-3,
                "scale {s}"
            );
        }
    }

    #[test]
    fn the_dialog_never_leaves_the_window() {
        for &s in &SCALES {
            let d = dialog_rect(1000.0, 420.0, 900.0, s);
            assert!(d.y >= 0.0, "scale {s}");
            assert!(d.y + d.h <= 420.0 + 1e-3, "scale {s}");
        }
    }

    #[test]
    fn notes_max_h_is_the_smaller_of_44_percent_and_300() {
        assert!(close(notes_max_h(420.0), 184.8));
        assert!(close(notes_max_h(900.0), 300.0));
    }

    #[test]
    fn the_fill_is_monotonic_and_ends_on_the_track() {
        for &s in &SCALES {
            for w in [100.0_f32, 418.0, 333.33] {
                let mut last = 0.0_f32;
                for pct in 0..=100u8 {
                    let f = fill_w(w, pct, s);
                    assert!(f >= last - 1e-6, "scale {s} pct {pct}");
                    assert!(f <= w, "scale {s} pct {pct}");
                    last = f;
                }
                assert!(close(fill_w(w, 100, s), w), "scale {s}");
                assert!(close(fill_w(w, 0, s), 0.0), "scale {s}");
            }
        }
    }
}
