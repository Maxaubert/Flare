//! The resume skeleton tests (Task 26), headless: pixel measuring at the eight display scalings,
//! the platform clock for the pulse and the 180 ms fade.

use slint::ComponentHandle;

use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{MarkMotion, SkeletonPreview};

/// `Theme.ground` (the terminal ground).
const GROUND: [u8; 4] = [23, 23, 23, 255];
/// The theme's text ink (#dedacf) at 7 percent over the ground, pulse 1.0.
const LINE: [u8; 4] = [37, 37, 36, 255];
/// The same line with the pulse at 0.5: the line's alpha halves.
const LINE_HALF: [u8; 4] = [30, 30, 29, 255];

fn harness(scale: f32) -> Harness<SkeletonPreview> {
    let h = Harness::new(scale, 400, 300, || {
        SkeletonPreview::new().expect("the preview builds")
    });
    h.advance(0);
    h
}

fn is_line(p: [u8; 4]) -> bool {
    Frame::near(p, LINE, 2)
}

#[test]
fn the_skeleton_covers_its_box_and_cuts_off_at_the_bottom() {
    let h = harness(1.0);
    let f = h.render();
    assert_eq!((f.w, f.h), (400, 300));
    // The first line starts at (16, 16) and is 11 px high.
    assert!(Frame::near(f.px(17, 20), LINE, 2), "{:?}", f.px(17, 20));
    assert_eq!(f.run_len_v(20, 16, is_line), 11);
    // Outside the padding the box is the ground.
    assert!(Frame::near(f.px(0, 0), GROUND, 1));
    assert!(Frame::near(f.px(399, 0), GROUND, 1));
    assert!(Frame::near(f.px(15, 16), GROUND, 1));
    // The page runs past the bottom edge. With 11 px lines, 10 px gaps and 26 px between
    // blocks, the third line of paragraph 3 (rows 274 to 284) is the last one above row 300;
    // paragraph 4 starts at row 311, so the bottom row of the box is ground.
    let last = (0..300).rev().find(|&y| is_line(f.px(20, y)));
    assert_eq!(last, Some(284));
    assert!(Frame::near(f.px(20, 299), GROUND, 1));
}

#[test]
fn the_header_and_paragraph_widths() {
    let h = harness(1.0);
    let f = h.render();
    // Paragraph 2 starts at row 132 and its lines are 11 high with 10 between them.
    let want = [(132, 88.0), (153, 94.0), (174, 71.0), (195, 38.0)];
    for (y, pct) in want {
        let n = f.run_len_h(y + 5, 16, is_line) as f32;
        let exp = 368.0 * pct / 100.0;
        assert!((n - exp).abs() <= 1.0, "row {y}: {n} vs {exp}");
    }
}

#[test]
fn lines_are_eleven_high_with_ten_between_and_twenty_six_between_blocks() {
    let h = harness(1.0);
    let f = h.render();
    // The header's logo column: three lines at 16, 37 and 58.
    assert_eq!(f.run_len_v(20, 16, is_line), 11);
    assert_eq!(f.run_len_v(20, 27, is_line), 0);
    assert_eq!(f.run_len_v(20, 37, is_line), 11);
    assert_eq!(f.run_len_v(20, 58, is_line), 11);
    // The header is 53 px tall; 26 px later paragraph 1 starts at 95.
    assert_eq!(f.run_len_v(20, 70, is_line), 0);
    assert_eq!(f.run_len_v(20, 95, is_line), 11);
    // Paragraph 1 is one line, so paragraph 2 starts 26 px after it, at 132.
    assert_eq!(f.run_len_v(20, 106, is_line), 0);
    assert_eq!(f.run_len_v(20, 132, is_line), 11);
    assert_eq!(f.run_len_v(20, 143, is_line), 0);
}

#[test]
fn it_pulses_under_the_clock_and_stills_when_asked() {
    let h = harness(1.0);
    let mm = h.comp.global::<MarkMotion>();
    mm.set_pulse(1.0);
    h.advance(0);
    let full = h.render().px(20, 20);
    assert!(Frame::near(full, LINE, 2), "{full:?}");
    mm.set_pulse(0.5);
    let half = h.render().px(20, 20);
    assert!(Frame::near(half, LINE_HALF, 2), "{half:?}");
    // Still wins over whatever the pulse says.
    mm.set_still(true);
    let still = h.render().px(20, 20);
    assert!(Frame::near(still, LINE, 2), "{still:?}");
}

#[test]
fn it_fades_out_in_180_ms() {
    let h = harness(1.0);
    // The fade animates from a frame that was drawn, so draw one before it starts.
    assert!(Frame::near(h.render().px(20, 20), LINE, 2));
    h.comp.set_leaving(true);
    // Sample every 30 ms: the first sample is part way down, the sixth (180 ms) is the ground.
    let mut samples = Vec::new();
    for _ in 0..6 {
        h.advance(30);
        samples.push(h.render().px(20, 20)[0]);
    }
    assert!(
        samples[0] > GROUND[0] && samples[0] < LINE[0],
        "30 ms {samples:?}"
    );
    // Falling, within the rounding of the un-premultiplied pixels (a level or two).
    assert!(
        samples.windows(2).all(|w| w[1] <= w[0] + 2),
        "falling {samples:?}"
    );
    assert!(samples[5].abs_diff(GROUND[0]) <= 2, "180 ms {samples:?}");
}

#[test]
fn the_skeleton_works_at_the_eight_scales() {
    for s in SCALES {
        let h = harness(s);
        let f = h.render();
        let top = h.phys(16.0);
        let x = h.phys(20.0);
        let line_h = f.run_len_v(x, top, is_line) as f32;
        let want_h = (11.0 * s).round();
        assert!(
            (line_h - want_h).abs() <= 1.0,
            "scale {s}: line {line_h} vs {want_h}"
        );
        // The middle of the line: its corners are rounded, so the edge is read off the middle.
        let row = top + (want_h / 2.0) as u32;
        let left = (0..f.w).find(|&x| is_line(f.px(x, row))).unwrap_or(0) as f32;
        let want_l = (16.0 * s).round();
        assert!(
            (left - want_l).abs() <= 1.0,
            "scale {s}: pad {left} vs {want_l}"
        );
    }
}
