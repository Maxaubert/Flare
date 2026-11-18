//! The Windows logo (rule 1280), headless: inset 3 to 21 with a 3-unit cross between its panes, so the cross stays
//! open where Flare draws it (11 px in the sidebar, 12 px on a tab, 13 px in the + menu, 15 px on Settings > Machines)
//! at 1x and 2x, at every sub-pixel offset; the simple-icons squares it replaced are measured the same way and blur
//! shut. In the + menu a machine's logo fills in the label's ink (text-soft, text while lit) in its 13 px box, never
//! in the folder colour.

use slint::{ComponentHandle, ModelRc, VecModel};

use super::icons_tabs::{Ink, blend, ink_edges, svg_bbox};
use crate::menus_ui::{GLYPH_FOLDER_FILL, GLYPH_LOGO_PC, glyph_path};
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{LogoPreview, MenuRow, MenusPreview};

/// The logo before rule 1280 (simple-icons): the four squares fill the box, 1.25 units apart.
const OLD_LOGO_PC: &str =
    "M0 0h11.377v11.372H0zm12.623 0H24v11.372H12.623zM0 12.623h11.377V24H0zm12.623 0H24V24H12.623z";

/// The sizes Flare draws the logo at: the sidebar's path line, a tab, the + menu, Settings > Machines.
const SIZES: [f32; 4] = [11.0, 12.0, 13.0, 15.0];

/// Sub-pixel offsets of the logo's box, in device pixels.
const OFFSETS: [f32; 4] = [0.0, 0.25, 0.5, 0.75];

fn rgb(c: slint::Color) -> [f32; 3] {
    [
        f32::from(c.red()),
        f32::from(c.green()),
        f32::from(c.blue()),
    ]
}

/// The ink the most open device column of a gap of `gap` device pixels can still hold, at the worst offset: what is
/// left of the pixel the gap covers most (a gap of two pixels or more always clears one whole pixel).
fn worst_gap_ink(gap: f32) -> f32 {
    (0..100)
        .map(|k| {
            let start = k as f32 / 100.0;
            let end = start + gap;
            let most = (0..4)
                .map(|p| {
                    let (a, b) = (p as f32, p as f32 + 1.0);
                    (end.min(b) - start.max(a)).max(0.0)
                })
                .fold(0.0, f32::max);
            1.0 - most
        })
        .fold(0.0, f32::max)
}

/// The logo drawn alone at `size` css px, its box at (8 + off, 8 + off) device pixels; the frame and its ink.
fn draw(s: f32, commands: &str, size: f32, off: f32) -> (Frame, Ink, f32, f32) {
    let h = Harness::new(s, 40, 40, || LogoPreview::new().unwrap());
    let at = 8.0 + off / s;
    h.comp.set_commands(commands.into());
    h.comp.set_size(size);
    h.comp.set_at_x(at);
    h.comp.set_at_y(at);
    h.advance(0);
    let f = h.render();
    let theme = h.comp.global::<crate::Theme>();
    let bg = f.px(1, 1);
    let ink = Ink::new(bg, rgb(theme.get_text()));
    (f, ink, at * s, size * s / 24.0)
}

/// How open the cross is: the least ink of any device column across the vertical gap (rows inside the panes) and of
/// any device row across the horizontal gap (columns inside the panes), the larger of the two; and the least ink in
/// the middle of a pane, which must be full. `panes` gives the panes' near and far edges in units (3 to `inner` and
/// 24 - `inner` to 21).
fn openness(f: &Frame, ink: &Ink, at: f32, unit: f32, panes: (f32, f32)) -> (f32, f32) {
    let (outer, inner) = panes;
    let dev = |u: f32| at + u * unit;
    // Device pixels lying wholly inside [a, b].
    let whole = |a: f32, b: f32| (a.ceil() as u32)..(b.floor() as u32);
    // Device pixels touching (a, b).
    let touching = |a: f32, b: f32| (a.floor() as u32)..(b.ceil() as u32);
    let pane_lines: Vec<u32> = whole(dev(outer), dev(inner))
        .chain(whole(dev(24.0 - inner), dev(24.0 - outer)))
        .collect();
    assert!(!pane_lines.is_empty(), "a pane holds whole pixels");
    let gap = touching(dev(inner), dev(24.0 - inner));
    let mean = |v: Vec<f32>| v.iter().sum::<f32>() / v.len() as f32;
    let col_gap = gap
        .clone()
        .map(|x| mean(pane_lines.iter().map(|&y| ink.cov(f.px(x, y))).collect()))
        .fold(1.0, f32::min);
    let row_gap = gap
        .map(|y| mean(pane_lines.iter().map(|&x| ink.cov(f.px(x, y))).collect()))
        .fold(1.0, f32::min);
    let full = pane_lines
        .iter()
        .flat_map(|&y| pane_lines.iter().map(move |&x| (x, y)))
        .map(|(x, y)| ink.cov(f.px(x, y)))
        .fold(1.0, f32::min);
    (col_gap.max(row_gap), full)
}

/// Rule 1280: at 11, 12, 13 and 15 px, at 1x and 2x and every sub-pixel offset, the cross between the panes holds a
/// column and a row of ground no more inked than the gap's width allows (a whole pixel of ground at 2x), while the
/// panes are solid. The old logo, measured the same way, closes its cross to more than half ink at 1x.
#[test]
fn the_windows_logos_cross_stays_open_at_11_to_15_px_at_1x_and_2x() {
    let new = glyph_path(GLYPH_LOGO_PC);
    assert_eq!(svg_bbox(new), (3.0, 3.0, 21.0, 21.0), "inset 3 to 21");
    for s in [1.0, 2.0] {
        for size in SIZES {
            let gap = 3.0 * size * s / 24.0;
            let bound = worst_gap_ink(gap) + 0.15;
            let mut old_worst: f32 = 0.0;
            for off in OFFSETS {
                let what = format!("{size} px at {s}x, offset {off}");
                let (f, ink, at, unit) = draw(s, new, size, off);
                let (open, full) = openness(&f, &ink, at, unit, (3.0, 10.5));
                assert!(
                    open <= bound,
                    "{what}: the cross holds {open:.2} ink, at most {bound:.2} for a {gap:.2} px gap"
                );
                assert!(open < 0.5, "{what}: the cross is ground ({open:.2} ink)");
                if s >= 2.0 {
                    assert!(
                        open < 0.15,
                        "{what}: a whole pixel of ground ({open:.2} ink)"
                    );
                }
                assert!(full > 0.85, "{what}: the panes are solid ({full:.2})");
                let (f, ink, at, unit) = draw(s, OLD_LOGO_PC, size, off);
                let (open, _) = openness(&f, &ink, at, unit, (0.0, 11.377));
                old_worst = old_worst.max(open);
            }
            if s < 2.0 {
                assert!(
                    old_worst > 0.5,
                    "{size} px at 1x: the old logo's cross closes ({old_worst:.2} ink), so the measure tells them apart"
                );
            }
        }
    }
}

fn logo_row(label: &str) -> MenuRow {
    MenuRow {
        label: label.into(),
        icon: glyph_path(GLYPH_LOGO_PC).into(),
        logo_icon: true,
        parent: -1,
        ..Default::default()
    }
}

fn folder_row(label: &str) -> MenuRow {
    MenuRow {
        label: label.into(),
        icon: glyph_path(GLYPH_FOLDER_FILL).into(),
        filled_icon: true,
        parent: -1,
        ..Default::default()
    }
}

/// A look's chrome tokens written into the menus preview's Theme (the ones a menu row draws with).
fn paint(h: &Harness<MenusPreview>, id: &str) {
    use crate::chrome_apply::colour;
    use flare_theme::edges::DEFAULT_WINDOW_EDGES;
    use flare_theme::look::{LookInput, apply_look};
    let l = apply_look(&LookInput {
        theme_id: id,
        custom: None,
        draft: None,
        background: None,
        accent: None,
        secondary: None,
        edges: DEFAULT_WINDOW_EDGES,
        see_through: 0,
        glass_possible: false,
    });
    let tok = |n: &str| colour(&l.chrome.vars[n]);
    let t = h.comp.global::<crate::Theme>();
    t.set_light(l.light);
    t.set_ground(tok("--p-bg"));
    t.set_side_flat(tok("--p-side-flat"));
    t.set_text(tok("--p-text"));
    t.set_text_soft(tok("--p-text-soft"));
    t.set_dim(tok("--p-dim"));
    t.set_dim2(tok("--p-dim2"));
    t.set_hover(tok("--p-hover"));
    t.set_divider(tok("--p-divider"));
    t.set_tree_folder(tok("--p-tree-folder"));
    h.advance(0);
}

/// The colour a pane's middle pixel holds in the row whose top is `top` css px (13 px box 11 px into the panel).
fn pane_px(f: &Frame, s: f32, x0: f32, top: f32) -> [u8; 4] {
    // The top-left pane's middle: units 6.75, 6.75 of the 13 px box, which sits in the middle of the 28 px row.
    let bx = x0 + 11.0;
    let by = top + (28.0 - 13.0) / 2.0;
    let u = 13.0 / 24.0;
    f.px(((bx + 6.75 * u) * s) as u32, ((by + 6.75 * u) * s) as u32)
}

/// Rule 1280: in the + menu a machine's Windows logo fills in text-soft (text while the row is lit) at the stroked
/// icons' 0.8, its 13 px box not grown (the ink spans the path's own 18 units), and never in the folder colour the
/// folder rows beside it wear. At every scale, in a dark and a light look.
#[test]
fn a_machines_logo_in_the_menu_wears_the_label_ink_not_the_folder_colour() {
    for look in ["pt-default", "paper"] {
        for s in SCALES {
            let h = Harness::new(s, 400, 300, || MenusPreview::new().unwrap());
            paint(&h, look);
            h.comp.set_menu_rows(ModelRc::new(VecModel::from(vec![
                folder_row("Flare"),
                logo_row("home-pc"),
            ])));
            h.comp.set_menu_x(40.0);
            h.comp.set_menu_y(40.0);
            h.comp.set_menu_open(true);
            h.advance(0);
            h.render();
            h.advance(0);
            let f = h.render();
            let theme = h.comp.global::<crate::Theme>();
            let (soft, text, folder) = (
                rgb(theme.get_text_soft()),
                rgb(theme.get_text()),
                rgb(theme.get_tree_folder()),
            );
            let (px, py) = (h.comp.get_panel_x(), h.comp.get_panel_y());
            let hair = s.floor().max(1.0) / s;
            // Row 1's top: the panel's border, row 0, the hairline between them.
            let top = py + hair + 28.0 + hair;
            let bg = f.px(((px + 4.0) * s) as u32, ((top + 14.0) * s) as u32);
            let what = format!("{look} at {s}");
            let got = pane_px(&f, s, px + hair, top);
            let want = blend(bg, soft, 0.8);
            let near = |p: [u8; 4], w: [f32; 3], tol: f32| {
                (0..3).all(|c| (f32::from(p[c]) - w[c]).abs() <= tol)
            };
            assert!(
                near(got, want, 10.0),
                "{what}: the logo fills in text-soft at 0.8 ({got:?}, want {want:?})"
            );
            let as_folder = blend(bg, folder, 0.8);
            let d: f32 = (0..3).map(|c| (want[c] - as_folder[c]).abs()).sum();
            if d > 30.0 {
                assert!(
                    !near(got, as_folder, 10.0),
                    "{what}: not the folder colour ({got:?})"
                );
            }
            // The 13 px box, not grown: the ink spans 18 of its 24 units.
            let k = |v: f32| (v * s).round() as u32;
            let region = (
                k(px + hair + 9.0),
                k(top + 4.0),
                k(px + hair + 26.0),
                k(top + 24.0),
            );
            let edges = ink_edges(&f, region, &Ink::new(bg, want)).expect("the logo draws");
            let (w, hh) = (edges.2 - edges.0, edges.3 - edges.1);
            let span = 18.0 * 13.0 / 24.0 * s;
            assert!(
                (w - span).abs() <= 1.0 && (hh - span).abs() <= 1.0,
                "{what}: drawn {w} by {hh} px, want {span}"
            );
            // Lit, the logo takes the text ink like its label.
            h.move_to(px + 60.0, top + 14.0);
            h.advance(16);
            let f = h.render();
            let bg = f.px(((px + 4.0) * s) as u32, ((top + 14.0) * s) as u32);
            let got = pane_px(&f, s, px + hair, top);
            let want = blend(bg, text, 0.8);
            assert!(
                near(got, want, 10.0),
                "{what}: lit, the logo fills in text at 0.8 ({got:?}, want {want:?})"
            );
        }
    }
}
