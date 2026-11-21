//! The pulse watch (rule 1324): evidence for a "pulse", something on screen that flips to another
//! look and straight back, too fast to see as a change but fast enough to look like a flicker.
//!
//! Max, 2026-11-20: the coloured bars of Claude Code's status line "pulsate"; shrinking the window
//! and maximising it again stops it. Headless runs found no cause: the grid painter, the atlas and
//! a femtovg composite give the same pixels frame after frame, and a recording of Claude Code
//! 2.1.329 fed through the emulator never shows the status row in another state. So the watch
//! records, while the app runs, the two places a flip can come from:
//!
//! - the cells of a painted frame ([`RowWatch`]): a row that reads A, then B, then A again within
//!   [`FLIP_WINDOW`] (a redraw shown half done, or a program alternating two versions);
//! - the geometry of the presented frame ([`PresentWatch`]): the window, the grid's box and texture,
//!   and on macOS the Metal layer's drawable against its bounds. A flip there (or a drawable that
//!   does not match the layer) means the window server scales or shifts the frame.
//!
//! Lines go to the diagnostics log (`row-flip`, `present-geom`, `present-flip`). They carry no
//! text: a row is described by its column span and its colours only (the log's privacy rule).

use flare_render::color::Color;
use flare_render::grid::{Cell, Grid};
use serde_json::{Map, Value, json};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// A look that comes back within this long after it was replaced is a flip.
pub const FLIP_WINDOW: Duration = Duration::from_millis(300);
/// A row or the geometry logs one flip in this long; the ones held back are counted in the next.
pub const LOG_GAP: Duration = Duration::from_secs(5);
/// Distinct colours a `row-flip` line names per side.
const COLOURS_MAX: usize = 6;

/// One logged flip: how long the other look showed, and the flips held back since the last line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flip {
    pub held: Duration,
    pub repeats: u32,
}

/// Follows one value (a hash) frame by frame and tells A -> B -> A within [`FLIP_WINDOW`].
#[derive(Clone, Debug, Default)]
pub struct FlipWatch {
    cur: Option<u64>,
    prev: Option<u64>,
    since: Option<Instant>,
    last_log: Option<Instant>,
    held_back: u32,
}

impl FlipWatch {
    /// Notes the frame's value. Some when it went back to the look before the current one within
    /// [`FLIP_WINDOW`] and the line is not held back by [`LOG_GAP`].
    pub fn observe(&mut self, v: u64, now: Instant) -> Option<Flip> {
        if self.cur == Some(v) {
            return None;
        }
        let held = self.since.map(|s| now.saturating_duration_since(s));
        let flip = self.prev == Some(v) && held.is_some_and(|h| h <= FLIP_WINDOW);
        self.prev = self.cur;
        self.cur = Some(v);
        self.since = Some(now);
        if !flip {
            return None;
        }
        if self
            .last_log
            .is_some_and(|t| now.saturating_duration_since(t) < LOG_GAP)
        {
            self.held_back += 1;
            return None;
        }
        self.last_log = Some(now);
        let repeats = std::mem::take(&mut self.held_back);
        Some(Flip {
            held: held.unwrap_or_default(),
            repeats,
        })
    }
}

/// A row of a painted frame that went A -> B -> A: where it differed and in what (no text).
#[derive(Clone, Debug, PartialEq)]
pub struct RowFlip {
    pub row: usize,
    pub first_col: usize,
    pub last_col: usize,
    /// Cells that differ between the two looks.
    pub cells: usize,
    pub ch_changed: bool,
    pub fg_changed: bool,
    pub bg_changed: bool,
    pub flip: Flip,
    /// The backgrounds of the differing cells in the look that came back, and in the one between.
    pub bg_back: Vec<String>,
    pub bg_between: Vec<String>,
}

impl RowFlip {
    pub fn fields(&self) -> Map<String, Value> {
        let v = json!({
            "row": self.row,
            "cols": [self.first_col, self.last_col],
            "cells": self.cells,
            "ch": self.ch_changed,
            "fg": self.fg_changed,
            "bg": self.bg_changed,
            "heldMs": self.flip.held.as_millis() as u64,
            "repeats": self.flip.repeats,
            "bgBack": self.bg_back,
            "bgBetween": self.bg_between,
        });
        match v {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RowSlot {
    watch: FlipWatch,
    cur: Vec<Cell>,
}

/// Watches every row of the painted frames ([`RowFlip`]).
#[derive(Clone, Debug, Default)]
pub struct RowWatch {
    dims: (usize, usize),
    rows: Vec<RowSlot>,
}

impl RowWatch {
    /// Forgets every row (another tab came to the front: its rows are not a flip of the last one's).
    pub fn reset(&mut self) {
        self.dims = (0, 0);
        self.rows.clear();
    }

    /// Notes one painted frame; the rows that flipped back.
    pub fn observe(&mut self, grid: &Grid, now: Instant) -> Vec<RowFlip> {
        let (cols, rows) = (grid.cols(), grid.rows());
        if self.dims != (cols, rows) {
            // A resize is a new screen, not a flip.
            self.dims = (cols, rows);
            self.rows = vec![RowSlot::default(); rows];
        }
        let mut out = Vec::new();
        for (r, slot) in self.rows.iter_mut().enumerate() {
            // The usual frame repaints most rows unchanged: a compare against the kept row is about
            // five times cheaper than hashing it (a maximised window holds 10k cells or more), and
            // the watch would do nothing with the same look anyway.
            if slot.cur.len() == cols && (0..cols).all(|c| grid.cell(c, r) == &slot.cur[c]) {
                continue;
            }
            let h = row_hash(grid, r);
            let changed = slot.watch.cur != Some(h);
            let flip = slot.watch.observe(h, now);
            if !changed {
                continue;
            }
            let row: Vec<Cell> = (0..cols).map(|c| grid.cell(c, r).clone()).collect();
            if let Some(flip) = flip {
                out.push(describe(r, &row, &slot.cur, flip));
            }
            slot.cur = row;
        }
        out
    }
}

fn row_hash(grid: &Grid, r: usize) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for c in 0..grid.cols() {
        let cell = grid.cell(c, r);
        cell.ch.hash(&mut h);
        cell.combining.hash(&mut h);
        cell.fg.hash(&mut h);
        cell.bg.hash(&mut h);
        cell.flags.hash(&mut h);
    }
    h.finish()
}

fn colour_name(c: Color) -> String {
    match c {
        Color::DefaultFg => "fg".to_string(),
        Color::DefaultBg => "bg".to_string(),
        Color::Indexed(i) => format!("i{i}"),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

fn push_colour(list: &mut Vec<String>, c: Color) {
    let n = colour_name(c);
    if list.len() < COLOURS_MAX && !list.contains(&n) {
        list.push(n);
    }
}

fn describe(row: usize, back: &[Cell], between: &[Cell], flip: Flip) -> RowFlip {
    let mut f = RowFlip {
        row,
        first_col: usize::MAX,
        last_col: 0,
        cells: 0,
        ch_changed: false,
        fg_changed: false,
        bg_changed: false,
        flip,
        bg_back: Vec::new(),
        bg_between: Vec::new(),
    };
    let blank = Cell::default();
    for (c, a) in back.iter().enumerate() {
        let b = between.get(c).unwrap_or(&blank);
        if a == b {
            continue;
        }
        f.first_col = f.first_col.min(c);
        f.last_col = c;
        f.cells += 1;
        f.ch_changed |= a.ch != b.ch || a.combining != b.combining;
        f.fg_changed |= a.fg != b.fg;
        f.bg_changed |= a.bg != b.bg;
        push_colour(&mut f.bg_back, a.bg);
        push_colour(&mut f.bg_between, b.bg);
    }
    if f.cells == 0 {
        f.first_col = 0;
    }
    f
}

/// The geometry of one presented frame, in device pixels (fractions kept: a half pixel that comes
/// and goes is exactly what this looks for).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PresentGeom {
    /// The window's inner size.
    pub win_w: u32,
    pub win_h: u32,
    /// The grid's box: where the grid image is drawn, and its size.
    pub box_x: f32,
    pub box_y: f32,
    pub box_w: f32,
    pub box_h: f32,
    /// The size of the texture the grid was painted into.
    pub tex_w: u32,
    pub tex_h: u32,
    /// The grid's width held during a sidebar slide (0 when none).
    pub frozen_w: f32,
    /// macOS: the Metal layer.
    pub layer: Option<LayerGeom>,
}

/// The window's `CAMetalLayer`: the drawable's size against the layer's bounds times its scale.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayerGeom {
    pub drawable_w: f64,
    pub drawable_h: f64,
    pub bounds_w: f64,
    pub bounds_h: f64,
    pub scale: f64,
    pub window_scale: f64,
    pub opaque: bool,
    /// The layer names a colour space (None leaves the contents untagged).
    pub tagged: bool,
}

impl LayerGeom {
    /// The drawable is not the layer's size in pixels: the window server scales every frame.
    pub fn scaled(&self) -> bool {
        (self.drawable_w - self.bounds_w * self.scale).abs() >= 0.5
            || (self.drawable_h - self.bounds_h * self.scale).abs() >= 0.5
    }
}

impl PresentGeom {
    fn hash(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.win_w, self.win_h, self.tex_w, self.tex_h).hash(&mut h);
        for f in [
            self.box_x,
            self.box_y,
            self.box_w,
            self.box_h,
            self.frozen_w,
        ] {
            f.to_bits().hash(&mut h);
        }
        if let Some(l) = self.layer {
            for f in [
                l.drawable_w,
                l.drawable_h,
                l.bounds_w,
                l.bounds_h,
                l.scale,
                l.window_scale,
            ] {
                f.to_bits().hash(&mut h);
            }
            (l.opaque, l.tagged).hash(&mut h);
        }
        h.finish()
    }

    /// The texture is not the box's size: the grid image is stretched (nearest) onto the screen.
    pub fn stretched(&self) -> bool {
        (self.box_w - self.tex_w as f32).abs() >= 0.5
            || (self.box_h - self.tex_h as f32).abs() >= 0.5
    }

    /// The box starts between two pixels.
    pub fn off_pixel(&self) -> bool {
        self.box_x.fract().abs() > 1e-3 || self.box_y.fract().abs() > 1e-3
    }

    pub fn fields(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("win".into(), json!([self.win_w, self.win_h]));
        m.insert(
            "box".into(),
            json!([self.box_x, self.box_y, self.box_w, self.box_h]),
        );
        m.insert("tex".into(), json!([self.tex_w, self.tex_h]));
        m.insert("frozenW".into(), json!(self.frozen_w));
        m.insert("stretched".into(), json!(self.stretched()));
        m.insert("offPixel".into(), json!(self.off_pixel()));
        if let Some(l) = self.layer {
            m.insert("drawable".into(), json!([l.drawable_w, l.drawable_h]));
            m.insert("bounds".into(), json!([l.bounds_w, l.bounds_h]));
            m.insert("layerScale".into(), json!(l.scale));
            m.insert("windowScale".into(), json!(l.window_scale));
            m.insert("opaque".into(), json!(l.opaque));
            m.insert("tagged".into(), json!(l.tagged));
            m.insert("layerScaled".into(), json!(l.scaled()));
        }
        m
    }
}

/// What [`PresentWatch::observe`] wants logged.
#[derive(Clone, Debug, PartialEq)]
pub enum PresentNote {
    /// The geometry changed (rate-limited to one line per [`LOG_GAP`], the last one always kept).
    Changed(PresentGeom),
    /// The geometry went back to the one before within [`FLIP_WINDOW`].
    Flipped(PresentGeom, Flip),
}

/// Watches the presented frame's geometry ([`PresentGeom`]).
#[derive(Clone, Debug, Default)]
pub struct PresentWatch {
    watch: FlipWatch,
    last: Option<PresentGeom>,
    last_log: Option<Instant>,
    /// A change not logged yet (held back by the gap); logged once the gap ends.
    pending: bool,
}

impl PresentWatch {
    pub fn observe(&mut self, g: PresentGeom, now: Instant) -> Vec<PresentNote> {
        let mut out = Vec::new();
        if self.last != Some(g) {
            self.last = Some(g);
            if let Some(f) = self.watch.observe(g.hash(), now) {
                out.push(PresentNote::Flipped(g, f));
            }
            self.pending = true;
        }
        let gap_over = self
            .last_log
            .is_none_or(|t| now.saturating_duration_since(t) >= LOG_GAP);
        if self.pending && gap_over {
            self.pending = false;
            self.last_log = Some(now);
            out.push(PresentNote::Changed(g));
        }
        out
    }
}

/// Writes a frame's notes to the diagnostics log.
pub fn log_rows(flips: &[RowFlip]) {
    if flips.is_empty() {
        return;
    }
    let log = flare_diag::writer::main();
    for f in flips {
        log.write("app", "row-flip", f.fields());
    }
}

/// Writes the geometry notes to the diagnostics log.
pub fn log_present(notes: &[PresentNote]) {
    if notes.is_empty() {
        return;
    }
    let log = flare_diag::writer::main();
    for n in notes {
        match n {
            PresentNote::Changed(g) => log.write("app", "present-geom", g.fields()),
            PresentNote::Flipped(g, f) => {
                let mut m = g.fields();
                m.insert("heldMs".into(), json!(f.held.as_millis() as u64));
                m.insert("repeats".into(), json!(f.repeats));
                log.write("app", "present-flip", m);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The status line of the owner's screenshot: a bar of coloured cells on row 1.
    fn status(cols: usize, bar: Color) -> Grid {
        let mut g = Grid::new(cols, 3);
        for c in 2..8 {
            let cell = g.cell_mut(c, 1);
            cell.ch = ' ';
            cell.bg = bar;
        }
        g
    }

    #[test]
    fn a_row_that_goes_back_within_the_window_is_a_flip() {
        let t = Instant::now();
        let mut w = FlipWatch::default();
        assert_eq!(w.observe(1, t), None);
        assert_eq!(w.observe(2, t + ms(100)), None);
        assert_eq!(
            w.observe(1, t + ms(116)),
            Some(Flip {
                held: ms(16),
                repeats: 0
            })
        );
    }

    #[test]
    fn a_slow_change_back_is_not_a_flip() {
        let t = Instant::now();
        let mut w = FlipWatch::default();
        w.observe(1, t);
        w.observe(2, t + ms(10));
        assert_eq!(w.observe(1, t + ms(10) + FLIP_WINDOW + ms(1)), None);
    }

    #[test]
    fn a_new_look_is_not_a_flip_and_the_same_look_is_nothing() {
        let t = Instant::now();
        let mut w = FlipWatch::default();
        w.observe(1, t);
        assert_eq!(w.observe(1, t + ms(5)), None);
        w.observe(2, t + ms(10));
        assert_eq!(w.observe(3, t + ms(20)), None);
    }

    #[test]
    fn flips_inside_the_gap_are_counted_in_the_next_line() {
        let t = Instant::now();
        let mut w = FlipWatch::default();
        w.observe(1, t);
        w.observe(2, t + ms(10));
        assert!(w.observe(1, t + ms(20)).is_some());
        w.observe(2, t + ms(30));
        assert_eq!(w.observe(1, t + ms(40)), None);
        w.observe(2, t + ms(50));
        assert_eq!(w.observe(1, t + ms(60)), None);
        // Each way back counts: 2, 1, 2, 1 inside the gap are four flips held back.
        let later = t + LOG_GAP + ms(100);
        w.observe(2, later);
        assert_eq!(
            w.observe(1, later + ms(10)),
            Some(Flip {
                held: ms(10),
                repeats: 4
            })
        );
    }

    /// Rule 1324: a coloured bar that shows blank for one frame is caught, with its colours and no text.
    #[test]
    fn a_status_bar_drawn_blank_for_one_frame_is_logged_with_its_colours() {
        let t = Instant::now();
        let green = Color::Rgb(0xb7, 0xe8, 0x76);
        let mut w = RowWatch::default();
        assert!(w.observe(&status(20, green), t).is_empty());
        assert!(
            w.observe(&status(20, Color::DefaultBg), t + ms(100))
                .is_empty()
        );
        let flips = w.observe(&status(20, green), t + ms(108));
        assert_eq!(flips.len(), 1);
        let f = &flips[0];
        assert_eq!((f.row, f.first_col, f.last_col, f.cells), (1, 2, 7, 6));
        assert!(f.bg_changed && !f.ch_changed && !f.fg_changed);
        assert_eq!(f.bg_back, vec!["#b7e876".to_string()]);
        assert_eq!(f.bg_between, vec!["bg".to_string()]);
        assert_eq!(f.flip.held, ms(8));
        let m = f.fields();
        assert_eq!(m["cols"], json!([2, 7]));
        assert!(!m.contains_key("text"));
    }

    #[test]
    fn identical_frames_and_a_resize_log_nothing() {
        let t = Instant::now();
        let green = Color::Rgb(0xb7, 0xe8, 0x76);
        let mut w = RowWatch::default();
        for i in 0..50 {
            assert!(w.observe(&status(20, green), t + ms(i * 16)).is_empty());
        }
        assert!(w.observe(&status(30, green), t + ms(900)).is_empty());
        assert!(w.observe(&status(20, green), t + ms(910)).is_empty());
    }

    fn geom(box_x: f32) -> PresentGeom {
        PresentGeom {
            win_w: 3840,
            win_h: 1472,
            box_x,
            box_y: 72.0,
            box_w: 3412.0,
            box_h: 1400.0,
            tex_w: 3412,
            tex_h: 1400,
            frozen_w: 0.0,
            layer: Some(LayerGeom {
                drawable_w: 3840.0,
                drawable_h: 1472.0,
                bounds_w: 1920.0,
                bounds_h: 736.0,
                scale: 2.0,
                window_scale: 2.0,
                opaque: false,
                tagged: false,
            }),
        }
    }

    #[test]
    fn a_grid_box_that_jumps_half_a_pixel_and_back_is_a_present_flip() {
        let t = Instant::now();
        let mut w = PresentWatch::default();
        assert_eq!(
            w.observe(geom(428.0), t),
            vec![PresentNote::Changed(geom(428.0))]
        );
        assert!(w.observe(geom(428.0), t + ms(16)).is_empty());
        w.observe(geom(428.5), t + ms(32));
        let notes = w.observe(geom(428.0), t + ms(48));
        assert!(matches!(notes[0], PresentNote::Flipped(g, _) if g == geom(428.0)));
        assert!(geom(428.5).off_pixel() && !geom(428.0).off_pixel());
    }

    #[test]
    fn geometry_changes_inside_the_gap_log_the_last_one_after_it() {
        let t = Instant::now();
        let mut w = PresentWatch::default();
        w.observe(geom(428.0), t);
        assert!(w.observe(geom(500.0), t + ms(500)).is_empty());
        assert!(w.observe(geom(600.0), t + ms(900)).is_empty());
        assert_eq!(
            w.observe(geom(600.0), t + LOG_GAP),
            vec![PresentNote::Changed(geom(600.0))]
        );
    }

    #[test]
    fn a_drawable_off_the_layer_size_is_scaled_and_a_texture_off_the_box_is_stretched() {
        let mut g = geom(428.0);
        assert!(!g.layer.unwrap().scaled() && !g.stretched());
        g.layer = Some(LayerGeom {
            drawable_w: 3838.0,
            ..g.layer.unwrap()
        });
        g.tex_w = 3410;
        assert!(g.layer.unwrap().scaled() && g.stretched());
        assert_eq!(g.fields()["layerScaled"], json!(true));
    }
}
