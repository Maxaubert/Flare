//! Every block's rectangle in device-pixel-snapped logical pixels (M7 Task 24).
//!
//! Source: the Tailwind classes of Prism Terminal's settings frame, row, section and wall (Appendix B of
//! the M7 plan), held to `pt_settings_metrics.json` (Task 3). This is arithmetic only: a row never wraps,
//! so no text is measured. Every edge is snapped to a whole device pixel at the scale in force
//! (`SnapGrid`), and two neighbours share their edge, so nothing gaps or overlaps at any scale.

use crate::find_state::centre_scroll;
use crate::page::{BlockModel, PageModel};
use crate::picker::Rect;
use crate::wall::WallLayout;

/// The eight display scalings (100 to 300 percent) every layout is checked at.
pub const SCALES: [f64; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

/// Prism Terminal's numbers, in CSS px at 100 percent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub rail_w: f64,
    pub rail_w_narrow: f64,
    pub rail_side: f64,
    pub rail_side_narrow: f64,
    pub rail_pad_top: f64,
    pub rail_pad_bottom: f64,
    pub rail_title_h: f64,
    pub rail_title_gap: f64,
    pub find_h: f64,
    pub find_gap: f64,
    pub item_h: f64,
    pub item_gap: f64,
    pub narrow_below: f64,
    pub column_max: f64,
    pub pad_x: f64,
    pub pad_x_narrow: f64,
    pub pad_top: f64,
    pub pad_top_narrow: f64,
    pub pad_bottom: f64,
    pub pad_bottom_narrow: f64,
    pub header_font: f64,
    pub header_line: f64,
    pub first_gap: f64,
    pub section_gap: f64,
    pub heading_line: f64,
    pub heading_min: f64,
    pub heading_gap: f64,
    pub border: f64,
    pub row_h: f64,
    pub row_pad_left: f64,
    pub row_pad_right: f64,
    pub row_gap: f64,
    pub tile: f64,
    pub note_h: f64,
    pub about_h: f64,
    pub wall_overhead: f64,
}

impl Metrics {
    /// Prism Terminal v0.58 (the measured file), at 100 percent.
    pub fn reference() -> Metrics {
        Metrics {
            rail_w: 244.0,
            rail_w_narrow: 56.0,
            rail_side: 12.0,
            rail_side_narrow: 8.0,
            rail_pad_top: 18.0,
            rail_pad_bottom: 12.0,
            rail_title_h: 22.5,
            rail_title_gap: 12.0,
            find_h: 32.0,
            find_gap: 14.0,
            item_h: 36.0,
            item_gap: 2.0,
            narrow_below: 760.0,
            column_max: 960.0,
            pad_x: 44.0,
            pad_x_narrow: 20.0,
            pad_top: 26.0,
            pad_top_narrow: 20.0,
            pad_bottom: 64.0,
            pad_bottom_narrow: 48.0,
            header_font: 24.0,
            header_line: 1.15,
            first_gap: 18.0,
            section_gap: 26.0,
            heading_line: 19.5,
            heading_min: 18.0,
            heading_gap: 8.0,
            border: 1.0,
            row_h: 58.0,
            row_pad_left: 14.0,
            row_pad_right: 16.0,
            row_gap: 14.0,
            tile: 32.0,
            note_h: 58.0,
            about_h: 96.0,
            wall_overhead: 66.0,
        }
    }
}

/// The page's (top, pad_x, bottom) paddings at this width.
fn pads(narrow: bool, m: &Metrics) -> (f64, f64, f64) {
    if narrow {
        (m.pad_top_narrow, m.pad_x_narrow, m.pad_bottom_narrow)
    } else {
        (m.pad_top, m.pad_x, m.pad_bottom)
    }
}

/// Edges snapped to whole device pixels at one scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapGrid {
    pub scale: f64,
}

impl SnapGrid {
    /// The device pixel an edge at `css` lands on, as logical pixels.
    pub fn edge(&self, css: f64) -> f64 {
        // Half a device pixel rounds up, with a tolerance, so an edge that lands on .5 moves the same
        // way as one 73 device pixels further on (a row of whole device pixels keeps its height).
        (css * self.scale + 0.5 + 1e-7).floor() / self.scale
    }

    /// `css` rounded UP to whole device pixels: a row never draws shorter than its CSS height (at
    /// 225 percent, 58 css is 130.5 device pixels, which a plain snap would draw as 130).
    pub fn at_least(&self, css: f64) -> f64 {
        (css * self.scale - 1e-9).ceil() / self.scale
    }

    /// `(top, height)` for a box at `y` and `h`: the top and the bottom are both snapped.
    pub fn span(&self, y: f64, h: f64) -> (f64, f64) {
        let top = self.edge(y);
        (top, self.edge(y + h) - top)
    }
}

/// The app icon picker (rules 1230 and 1263, layout A of 2026-11-14): fixed 36 px tiles with 10 px between them, left
/// aligned at the words' column (60), 2 under the row's 58 of words, wrapping onto more lines when the row is too
/// narrow, 16 under the last line. Rust and the view both place the tiles by `icon_cols`.
pub const ICON_TILE: f64 = 36.0;
pub const ICON_GAP: f64 = 10.0;
pub const ICON_TILES_X: f64 = 60.0;
pub const ICON_TILES_Y: f64 = 60.0;
/// The room kept right of the tiles and under them.
pub const ICON_PAD_RIGHT: f64 = 16.0;
pub const ICON_PAD_BOTTOM: f64 = 16.0;

/// How many of `n` tiles fit one line of a row `row_w` wide (CSS px, inside the panel's border): at least one.
pub fn icon_cols(row_w: f64, n: usize) -> usize {
    let room = (row_w - ICON_TILES_X - ICON_PAD_RIGHT).max(0.0);
    let fit = ((room + ICON_GAP) / (ICON_TILE + ICON_GAP) + 1e-9).floor();
    (fit.max(1.0) as usize).min(n.max(1))
}

/// How many lines `n` tiles take in a row `row_w` wide.
pub fn icon_lines(row_w: f64, n: usize) -> usize {
    n.max(1).div_ceil(icon_cols(row_w, n))
}

/// The CSS height of the app icon row: its words, its lines of tiles and the room under them.
pub fn icon_row_h(row_w: f64, n: usize) -> f64 {
    let lines = icon_lines(row_w, n) as f64;
    ICON_TILES_Y + lines * ICON_TILE + (lines - 1.0) * ICON_GAP + ICON_PAD_BOTTOM
}

/// A row's CSS width inside a panel `content_w` wide (the panel's border on each side).
pub fn row_w_of(content_w: f64) -> f64 {
    content_w - 2.0 * Metrics::reference().border
}

/// A speech model's or the GPU pack's row: PT's `min-h-16` (M8 Task 25).
pub const MODEL_ROW_H: f64 = 64.0;
/// The id the GPU pack's block carries (the store's id for the pack).
pub const GPU_ROW_ID: &str = "gpu-pack";
/// The Machines page's table (remote sessions PR 7): the heading row, a machine's row, and the add form, which grows
/// by one line for its red line. A machine that is Not linked grows by its grey reason line (2026-11-13).
pub const MACHINE_HEAD_H: f64 = 34.0;
pub const MACHINE_ROW_H: f64 = 50.0;
pub const MACHINE_REASON_H: f64 = 18.0;
pub const MACHINE_FORM_H: f64 = 58.0;
pub const MACHINE_FORM_LINE_H: f64 = 26.0;

/// The kind of a panel's block (Slint's `BlockKind` names the same ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Row,
    Wall,
    About,
    Slint,
    Note,
    /// A speech model or the GPU pack (M8 Task 25).
    Model,
    /// The Machines page's table heading, a machine, and the add form (remote sessions PR 7).
    MachineHead,
    Machine,
    MachineAdd,
}

/// One block of a panel. `id` is the row id for a row and "" for the other kinds.
#[derive(Clone, Debug, PartialEq)]
pub struct RowLayout {
    pub id: &'static str,
    pub block: BlockKind,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// The app icon row's tiles a line (rule 1263; the view places its tiles by it); 0 for every other block.
    pub cols: usize,
}

/// One section: the heading's box, the panel's box and every block of the panel, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionLayout {
    pub id: &'static str,
    pub title_y: f64,
    pub title_h: f64,
    pub panel_x: f64,
    pub panel_y: f64,
    pub panel_w: f64,
    pub panel_h: f64,
    pub rows: Vec<RowLayout>,
}

/// A whole page. Positions are in the page's own content coordinates (the pane's top is 0).
#[derive(Clone, Debug, PartialEq)]
pub struct PageLayout {
    pub header_y: f64,
    pub header_h: f64,
    pub sections: Vec<SectionLayout>,
    pub pad_x: f64,
    pub column_x: f64,
    pub content_w: f64,
    pub content_h: f64,
    pub narrow: bool,
    /// The wall block's height once the wall is open (`full_h + 66`), for the expand animation.
    pub wall_expanded_h: f64,
}

/// The Results list: one panel of hit rows under a "Results" heading.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultsLayout {
    pub header_y: f64,
    pub header_h: f64,
    pub section: SectionLayout,
    pub content_h: f64,
}

/// The rail: its width, the title, the Find field and the item boxes (`(y, h)`, shared x and w).
#[derive(Clone, Debug, PartialEq)]
pub struct RailLayout {
    pub width: f64,
    pub narrow: bool,
    pub scale: f64,
    pub title_y: f64,
    pub title_h: f64,
    pub find_y: f64,
    pub find_h: f64,
    pub inner_x: f64,
    pub inner_w: f64,
    pub items: Vec<(f64, f64)>,
}

/// Below this frame width the Settings tab uses its narrow paddings and rail.
pub fn is_narrow(frame_w: f64) -> bool {
    frame_w < Metrics::reference().narrow_below
}

/// The rail's width at this frame width: 244, or 56 when narrow.
pub fn rail_width(frame_w: f64) -> f64 {
    let m = Metrics::reference();
    if is_narrow(frame_w) {
        m.rail_w_narrow
    } else {
        m.rail_w
    }
}

/// One block of a panel before placement: its kind, its row id ("" for non-rows), its CSS height.
struct Child {
    kind: BlockKind,
    id: &'static str,
    css_h: f64,
    cols: usize,
}

/// The CSS height of a block. A wall is its clip plus 66 (the heading gap, the toggle and the paddings).
fn child_of(
    block: &BlockModel,
    wall: &WallLayout,
    slint_badge_h: f64,
    m: &Metrics,
    grid: SnapGrid,
    content_w: f64,
) -> Child {
    let (kind, id, css_h) = match block {
        // The app icon picker's row is its words, then its lines of tiles under them (rules 1230 and 1263).
        BlockModel::Row(r) => match &r.control {
            crate::page::Control::Icons { options, .. } => (
                BlockKind::Row,
                r.id,
                icon_row_h(row_w_of(content_w), options.len()),
            ),
            _ => (BlockKind::Row, r.id, m.row_h),
        },
        BlockModel::Wall => (BlockKind::Wall, "", wall.clip_h + m.wall_overhead),
        BlockModel::AboutCard { .. } => (BlockKind::About, "", m.about_h),
        BlockModel::SlintBadge => (BlockKind::Slint, "", slint_badge_h),
        BlockModel::Note(_) => (BlockKind::Note, "", m.note_h),
        // A speech model or the GPU pack: PT's `min-h-16` row (M8 Task 25), named by its catalogue id.
        BlockModel::Model(r) => (BlockKind::Model, r.id, MODEL_ROW_H),
        BlockModel::Gpu(_) => (BlockKind::Model, GPU_ROW_ID, MODEL_ROW_H),
        BlockModel::MachineHead => (BlockKind::MachineHead, "", MACHINE_HEAD_H),
        BlockModel::Machine(r) => (
            BlockKind::Machine,
            "",
            MACHINE_ROW_H
                + if r.sub_line().is_empty() {
                    0.0
                } else {
                    MACHINE_REASON_H
                },
        ),
        BlockModel::MachineAdd(a) => (
            BlockKind::MachineAdd,
            crate::page::machines::ADD_ROW,
            MACHINE_FORM_H
                + if a.form.error.is_some() {
                    MACHINE_FORM_LINE_H
                } else {
                    0.0
                },
        ),
    };
    let cols = match block {
        BlockModel::Row(r) => match &r.control {
            crate::page::Control::Icons { options, .. } => {
                icon_cols(row_w_of(content_w), options.len())
            }
            _ => 0,
        },
        _ => 0,
    };
    Child {
        kind,
        id,
        css_h: grid.at_least(css_h),
        cols,
    }
}

/// Places one section below `top` (CSS px): the heading, 8 px above the panel, the panel's border and
/// its blocks. Returns the section and the CSS bottom of its panel.
#[allow(clippy::too_many_arguments)]
fn place_section(
    grid: SnapGrid,
    m: &Metrics,
    id: &'static str,
    top: f64,
    gap: f64,
    pad_x: f64,
    content_w: f64,
    children: &[Child],
    has_heading: bool,
) -> (SectionLayout, f64) {
    let title_top = top + gap;
    let title_h = if has_heading {
        m.heading_line.max(m.heading_min)
    } else {
        0.0
    };
    let panel_top = title_top + title_h + if has_heading { m.heading_gap } else { 0.0 };
    let (rx, rw) = grid.span(pad_x + m.border, content_w - 2.0 * m.border);
    let mut y = panel_top + m.border;
    let mut rows = Vec::with_capacity(children.len());
    for c in children {
        let (ry, rh) = grid.span(y, c.css_h);
        rows.push(RowLayout {
            id: c.id,
            block: c.kind,
            x: rx,
            y: ry,
            w: rw,
            h: rh,
            cols: c.cols,
        });
        y += c.css_h;
    }
    let panel_bottom = y + m.border;
    let (title_y, title_snapped_h) = grid.span(title_top, title_h);
    let (panel_x, panel_w) = grid.span(pad_x, content_w);
    let (panel_y, panel_h) = grid.span(panel_top, panel_bottom - panel_top);
    let section = SectionLayout {
        id,
        title_y,
        title_h: title_snapped_h,
        panel_x,
        panel_y,
        panel_w,
        panel_h,
        rows,
    };
    (section, panel_bottom)
}

/// Lays out one page of the Settings tab in a frame `frame_w` wide (rail included).
pub fn layout_page(
    model: &PageModel,
    frame_w: f64,
    scale: f64,
    wall: &WallLayout,
    expanded_wall_h: f64,
    slint_badge_h: f64,
) -> PageLayout {
    let m = Metrics::reference();
    let grid = SnapGrid { scale };
    let narrow = is_narrow(frame_w);
    let pane_w = frame_w - rail_width(frame_w);
    let (pad_top, pad_x, pad_bottom) = pads(narrow, &m);
    let content_w = (m.column_max.min(pane_w) - 2.0 * pad_x).max(0.0);
    let header_css_h = m.header_font * m.header_line;
    let (header_y, header_h) = grid.span(pad_top, header_css_h);
    let mut top = pad_top + header_css_h;
    let mut sections = Vec::with_capacity(model.sections.len());
    for (i, s) in model.sections.iter().enumerate() {
        let gap = if i == 0 { m.first_gap } else { m.section_gap };
        let children: Vec<Child> = s
            .blocks
            .iter()
            .map(|b| child_of(b, wall, slint_badge_h, &m, grid, content_w))
            .collect();
        let (section, bottom) = place_section(
            grid,
            &m,
            s.id,
            top,
            gap,
            pad_x,
            content_w,
            &children,
            !s.title.is_empty(),
        );
        sections.push(section);
        top = bottom;
    }
    PageLayout {
        header_y,
        header_h,
        sections,
        pad_x,
        column_x: 0.0,
        content_w,
        content_h: grid.edge(top + pad_bottom),
        narrow,
        wall_expanded_h: expanded_wall_h + m.wall_overhead,
    }
}

/// The Results list of `hits` rows (each 58 high) in a frame `frame_w` wide.
pub fn layout_results(hits: usize, frame_w: f64, scale: f64) -> ResultsLayout {
    let m = Metrics::reference();
    let grid = SnapGrid { scale };
    let narrow = is_narrow(frame_w);
    let pane_w = frame_w - rail_width(frame_w);
    let (pad_top, pad_x, pad_bottom) = pads(narrow, &m);
    let content_w = (m.column_max.min(pane_w) - 2.0 * pad_x).max(0.0);
    let header_css_h = m.header_font * m.header_line;
    let (header_y, header_h) = grid.span(pad_top, header_css_h);
    let children: Vec<Child> = (0..hits)
        .map(|_| Child {
            kind: BlockKind::Row,
            id: "",
            css_h: m.row_h,
            cols: 0,
        })
        .collect();
    let (section, bottom) = place_section(
        grid,
        &m,
        "results",
        pad_top + header_css_h,
        m.first_gap,
        pad_x,
        content_w,
        &children,
        true,
    );
    ResultsLayout {
        header_y,
        header_h,
        section,
        content_h: grid.edge(bottom + pad_bottom),
    }
}

/// The rail at this frame width with `items` items (About last). Positions are from the top; see `about_y`.
pub fn rail_layout(frame_w: f64, scale: f64, items: usize) -> RailLayout {
    let m = Metrics::reference();
    let grid = SnapGrid { scale };
    let narrow = is_narrow(frame_w);
    let width = rail_width(frame_w);
    let side = if narrow {
        m.rail_side_narrow
    } else {
        m.rail_side
    };
    let (title_y, title_h) = grid.span(m.rail_pad_top, m.rail_title_h);
    let find_top = m.rail_pad_top + m.rail_title_h + m.rail_title_gap;
    let (find_y, find_h) = grid.span(find_top, m.find_h);
    let mut y = find_top + m.find_h + m.find_gap;
    let mut boxes = Vec::with_capacity(items);
    for _ in 0..items {
        boxes.push(grid.span(y, m.item_h));
        y += m.item_h + m.item_gap;
    }
    // The rail's own right border is one px; the items sit `side` in from the left.
    let (inner_x, inner_w) = grid.span(side, width - m.border - 2.0 * side);
    RailLayout {
        width,
        narrow,
        scale,
        title_y,
        title_h,
        find_y,
        find_h,
        inner_x,
        inner_w,
        items: boxes,
    }
}

/// The About item's top: pinned to the bottom of a rail `frame_h` high, 12 px above the edge.
pub fn about_y(rail: &RailLayout, frame_h: f64) -> f64 {
    let m = Metrics::reference();
    let css = frame_h - m.rail_pad_bottom - m.item_h;
    SnapGrid { scale: rail.scale }.edge(css)
}

/// The scroll offset that centres the row `row_id` in a pane `pane_h` high; `None` when no row has that id.
pub fn scroll_target(layout: &PageLayout, row_id: &str, pane_h: f64) -> Option<f64> {
    layout
        .sections
        .iter()
        .flat_map(|s| s.rows.iter())
        .find(|r| r.block == BlockKind::Row && r.id == row_id)
        .map(|r| centre_scroll(r.y, r.h, pane_h, layout.content_h))
}

/// The Select menu's `(x, y, h)`: right edge to the anchor's right, 4 below it, height
/// `min(content_h, max_h)` shrunk to fit the window less 8. When fewer than 120 px fit below and more
/// fit above, it opens upward. Never left of 8.
pub fn place_menu(
    anchor: Rect,
    menu_w: f64,
    content_h: f64,
    window: (f64, f64),
    max_h: f64,
) -> (f64, f64, f64) {
    place_menu_at(anchor, menu_w, content_h, window, max_h, 0.0)
}

/// `place_menu` with every edge rounded to the device grid at `scale` (0 leaves them as they are).
pub fn place_menu_at(
    anchor: Rect,
    menu_w: f64,
    content_h: f64,
    window: (f64, f64),
    max_h: f64,
    scale: f64,
) -> (f64, f64, f64) {
    let (x, y, h) = place_menu_raw(anchor, menu_w, content_h, window, max_h);
    if scale > 0.0 {
        let sn = |v: f64| (v * scale).round() / scale;
        (sn(x), sn(y), sn(y + h) - sn(y))
    } else {
        (x, y, h)
    }
}

fn place_menu_raw(
    anchor: Rect,
    menu_w: f64,
    content_h: f64,
    window: (f64, f64),
    max_h: f64,
) -> (f64, f64, f64) {
    const GAP: f64 = 4.0;
    const EDGE: f64 = 8.0;
    const CRAMPED: f64 = 120.0;
    let x = (anchor.x + anchor.w - menu_w).max(EDGE);
    let want = content_h.min(max_h);
    let below_top = anchor.y + anchor.h + GAP;
    let below_room = window.1 - EDGE - below_top;
    let above_room = anchor.y - GAP - EDGE;
    if below_room < CRAMPED && above_room > below_room {
        let h = want.min(above_room);
        (x, anchor.y - GAP - h, h)
    } else {
        (x, below_top, want.min(below_room.max(0.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::test_env::with_env;
    use crate::page::{NoDictation, build_page, section};
    use crate::wall::{layout as wall_layout, total_themes};

    const PAGES: [&str; 6] = [
        "appearance",
        "terminal",
        "agents",
        "dictation",
        "diagnostics",
        "about",
    ];

    fn metrics() -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("flare")
            .join("tests")
            .join("fixtures")
            .join("pt_settings_metrics.json");
        let text = std::fs::read_to_string(&path).expect("the metrics fixture");
        serde_json::from_str(&text).expect("valid JSON")
    }

    fn num(v: &serde_json::Value) -> f64 {
        v.as_f64().expect("a number")
    }

    /// The wall as the page builds it: the panel's inner width under the column.
    fn wall_for(frame_w: f64, expanded: bool) -> WallLayout {
        let m = Metrics::reference();
        let narrow = is_narrow(frame_w);
        let pad_x = if narrow { m.pad_x_narrow } else { m.pad_x };
        let pane = frame_w - rail_width(frame_w);
        let content = m.column_max.min(pane) - 2.0 * pad_x;
        wall_layout(content - 2.0 * m.border, total_themes(false), expanded, 1.0)
    }

    fn laid_out(id: &str, frame_w: f64, scale: f64) -> PageLayout {
        with_env(|env| {
            let page = build_page(id, env, &NoDictation);
            let wall = wall_for(frame_w, false);
            layout_page(&page, frame_w, scale, &wall, wall.full_h, 112.0)
        })
    }

    fn is_whole(v: f64, scale: f64) -> bool {
        let d = v * scale;
        (d - d.round()).abs() < 1e-6
    }

    #[test]
    fn at_all_eight_scales_rows_are_58_or_more_and_edges_are_whole_device_pixels() {
        for id in PAGES {
            for scale in SCALES {
                let l = laid_out(id, 1000.0, scale);
                assert!(is_whole(l.header_y, scale) && is_whole(l.header_h, scale));
                for s in &l.sections {
                    for v in [s.title_y, s.title_h, s.panel_y, s.panel_h] {
                        assert!(is_whole(v, scale), "{id} at {scale}: {v} is not whole");
                    }
                    for r in &s.rows {
                        for v in [r.y, r.h] {
                            assert!(is_whole(v, scale), "{id} at {scale}: {v} is not whole");
                        }
                        assert!(
                            r.h >= 58.0 - 1e-9,
                            "{id} at {scale}: block {:?} is {}",
                            r.block,
                            r.h
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn neighbouring_rows_share_their_edge() {
        for id in PAGES {
            for scale in SCALES {
                let l = laid_out(id, 1000.0, scale);
                for s in &l.sections {
                    for pair in s.rows.windows(2) {
                        let gap = (pair[0].y + pair[0].h - pair[1].y).abs();
                        assert!(gap < 1e-9, "{id} at {scale}: rows part by {gap}");
                    }
                }
            }
        }
    }

    #[test]
    fn panels_do_not_overlap_and_sections_are_26_apart() {
        for scale in SCALES {
            let l = laid_out("appearance", 1000.0, scale);
            let first_gap = l.sections[0].title_y - (l.header_y + l.header_h);
            assert!(
                (first_gap - 18.0).abs() <= 1.0 / scale + 1e-9,
                "first {first_gap}"
            );
            for pair in l.sections.windows(2) {
                let bottom = pair[0].panel_y + pair[0].panel_h;
                let gap = pair[1].title_y - bottom;
                assert!(
                    (gap - 26.0).abs() <= 1.0 / scale + 1e-9,
                    "gap {gap} at {scale}"
                );
                assert!(
                    pair[1].title_y >= bottom,
                    "a heading overlaps the panel above"
                );
            }
        }
    }

    #[test]
    fn the_column_fits_the_pane_at_900_and_1600_wide() {
        let small = laid_out("appearance", 900.0, 1.0);
        assert!(small.content_w + 2.0 * small.pad_x <= 900.0 - 244.0 + 1e-9);
        let wide = laid_out("appearance", 1600.0, 1.0);
        assert!(
            (wide.content_w - (960.0 - 2.0 * 44.0)).abs() < 1e-9,
            "{}",
            wide.content_w
        );
    }

    #[test]
    fn narrow_mode_below_760_uses_the_narrow_paddings() {
        assert!(is_narrow(759.0));
        assert!(!is_narrow(760.0));
        let narrow = laid_out("appearance", 759.0, 1.0);
        assert!(narrow.narrow);
        assert_eq!(narrow.pad_x, 20.0);
        let wide = laid_out("appearance", 760.0, 1.0);
        assert!(!wide.narrow);
        assert_eq!(wide.pad_x, 44.0);
        assert_eq!(rail_width(759.0), 56.0);
        assert_eq!(rail_width(760.0), 244.0);
    }

    #[test]
    fn the_rail_is_244_wide_or_56_when_narrow() {
        let wide = rail_layout(1000.0, 1.0, 6);
        assert_eq!(wide.width, 244.0);
        assert_eq!(wide.inner_x, 12.0);
        assert_eq!(wide.inner_w, 219.0);
        for pair in wide.items.windows(2) {
            assert_eq!(pair[0].1, 36.0);
            assert!((pair[1].0 - pair[0].0 - 38.0).abs() < 1e-9);
        }
        assert_eq!(about_y(&wide, 604.0), 556.0);
        let narrow = rail_layout(700.0, 1.0, 6);
        assert_eq!(narrow.width, 56.0);
        assert_eq!(narrow.inner_x, 8.0);
        assert_eq!(narrow.inner_w, 39.0);
    }

    #[test]
    fn the_wall_block_is_clip_plus_66() {
        let page = PageModel {
            id: "appearance",
            label: "Appearance",
            sections: vec![section("theme", vec![BlockModel::Wall])],
        };
        let collapsed = wall_for(1000.0, false);
        assert_eq!(collapsed.clip_h, 268.0);
        let l = layout_page(&page, 1000.0, 1.0, &collapsed, collapsed.full_h, 112.0);
        assert_eq!(l.sections[0].rows[0].block, BlockKind::Wall);
        assert_eq!(l.sections[0].rows[0].h, 268.0 + 66.0);
        let expanded = wall_for(1000.0, true);
        let open = layout_page(&page, 1000.0, 1.0, &expanded, expanded.full_h, 112.0);
        assert_eq!(open.sections[0].rows[0].h, expanded.full_h + 66.0);
        assert_eq!(open.wall_expanded_h, expanded.full_h + 66.0);
    }

    /// Rules 1230 and 1263: the app icon row is its words over one line of 36 px tiles, 112 high, at the usual width;
    /// the other rows stay 58.
    #[test]
    fn the_app_icon_row_is_its_words_over_a_line_of_tiles() {
        for scale in [1.0, 1.5, 2.25] {
            let l = laid_out("appearance", 1000.0, scale);
            let window = &l.sections[0];
            let ids: Vec<&str> = window.rows.iter().map(|r| r.id).collect();
            assert_eq!(
                ids,
                ["tab-layout", "tab-switch", "window-edges", "app-icon"]
            );
            let icons = &window.rows[3];
            assert_eq!(icons.block, BlockKind::Row);
            assert!((icons.h - 112.0).abs() < 1.0, "{scale}: {}", icons.h);
            assert!((window.rows[2].h - 58.0).abs() < 1.0, "{scale}");
            assert_eq!(icon_cols(row_w_of(l.content_w), 12), 12, "{scale}");
        }
        assert_eq!(ICON_TILES_Y + ICON_TILE + ICON_PAD_BOTTOM, 112.0);
    }

    /// Rule 1263: a row too narrow for every tile wraps them onto more lines and grows by a tile and a gap per line;
    /// a row with room for them all keeps one line.
    #[test]
    fn the_app_icon_tiles_wrap_and_the_row_grows() {
        // 60 of words column, 16 at the right: twelve tiles need 60 + 12 * 36 + 11 * 10 + 16 = 618.
        assert_eq!(icon_cols(618.0, 12), 12);
        assert_eq!(icon_lines(618.0, 12), 1);
        assert_eq!(icon_cols(617.0, 12), 11);
        assert_eq!(icon_lines(617.0, 12), 2);
        assert_eq!(icon_row_h(617.0, 12), 60.0 + 2.0 * 36.0 + 10.0 + 16.0);
        // Room for three: four lines.
        let three = 60.0 + 3.0 * 36.0 + 2.0 * 10.0 + 16.0;
        assert_eq!(icon_cols(three, 12), 3);
        assert_eq!(icon_lines(three, 12), 4);
        // Never fewer than one a line, and never more columns than tiles.
        assert_eq!(icon_cols(10.0, 12), 1);
        assert_eq!(icon_cols(5000.0, 12), 12);
        // A narrow window: the row is laid out taller, by whole lines.
        for scale in [1.0, 1.5, 2.25] {
            let l = laid_out("appearance", 600.0, scale);
            assert!(l.narrow);
            let rw = row_w_of(l.content_w);
            let lines = icon_lines(rw, 12);
            assert!(lines >= 2, "{scale}: {rw} css px holds {lines} lines");
            let icons = &l.sections[0].rows[3];
            assert_eq!(icons.id, "app-icon");
            assert_eq!(icons.cols, icon_cols(rw, 12), "{scale}");
            assert_eq!(l.sections[0].rows[2].cols, 0);
            assert!(
                (icons.h - icon_row_h(rw, 12)).abs() < 1.0,
                "{scale}: {} vs {}",
                icons.h,
                icon_row_h(rw, 12)
            );
        }
    }

    #[test]
    fn about_cards_are_96_and_the_slint_badge_is_its_measured_height() {
        let l = laid_out("about", 1000.0, 1.0);
        let kinds: Vec<(BlockKind, f64)> =
            l.sections[0].rows.iter().map(|r| (r.block, r.h)).collect();
        assert_eq!(kinds[0], (BlockKind::About, 96.0));
        assert_eq!(kinds[2], (BlockKind::Slint, 112.0));
    }

    #[test]
    fn a_model_row_is_64_high_at_every_scale_and_keeps_its_catalogue_id() {
        use crate::page::dictation::{ModelRow, ModelState, Vendor};
        let row = |id: &'static str| ModelRow {
            id,
            name: "Whisper Base",
            size_text: "148 MB".to_string(),
            vendor: Vendor::Openai,
            recommended: false,
            badge: None,
            sub: String::new(),
            warn: false,
            state: ModelState::Absent,
            failure: None,
            buttons: Vec::new(),
            uninstall_title: String::new(),
        };
        let page = PageModel {
            id: "dictation",
            label: "Dictation",
            sections: vec![section(
                "models",
                vec![
                    BlockModel::Model(row("base")),
                    BlockModel::Model(row("small")),
                ],
            )],
        };
        for scale in SCALES {
            let wall = wall_for(1000.0, false);
            let l = layout_page(&page, 1000.0, scale, &wall, wall.full_h, 112.0);
            let rows = &l.sections[0].rows;
            assert_eq!(rows[0].block, BlockKind::Model);
            assert_eq!(rows[0].id, "base");
            assert_eq!(rows[1].id, "small");
            for r in rows {
                assert!(r.h >= MODEL_ROW_H - 1e-9, "scale {scale}: {}", r.h);
                assert!(is_whole(r.y, scale) && is_whole(r.h, scale));
            }
            assert!((rows[0].y + rows[0].h - rows[1].y).abs() < 1e-9);
        }
    }

    #[test]
    fn rows_match_the_references_measured_boxes() {
        let m = metrics();
        let pt = &m["scales"]["1"]["appearance"];
        let l = laid_out("appearance", 1000.0, 1.0);
        let s0 = &l.sections[0];
        let row = &s0.rows[0];
        let checks = [
            ("header y", l.header_y, num(&pt["header"]["y"])),
            ("heading y", s0.title_y, num(&pt["sectionHeading"]["y"])),
            ("panel y", s0.panel_y, num(&pt["panel"]["y"])),
            ("panel width", s0.panel_w, num(&pt["panel"]["width"])),
            ("row y", row.y, num(&pt["row"]["y"])),
            // The layout is pane-relative; Prism Terminal's boxes are frame-relative (the rail is 244).
            (
                "panel x",
                s0.panel_x + rail_width(1000.0),
                num(&pt["panel"]["x"]),
            ),
            ("row x", row.x + rail_width(1000.0), num(&pt["row"]["x"])),
            ("row height", row.h, num(&pt["row"]["height"])),
        ];
        for (what, got, want) in checks {
            assert!(
                (got - want).abs() <= 1.0,
                "{what}: layout {got}, the reference {want}"
            );
        }
        let tile_dx = num(&pt["rowTile"]["x"]) - num(&pt["row"]["x"]);
        let tile_dy = num(&pt["rowTile"]["y"]) - num(&pt["row"]["y"]);
        assert_eq!(tile_dx, Metrics::reference().row_pad_left);
        assert_eq!(num(&pt["rowTile"]["width"]), Metrics::reference().tile);
        assert_eq!(tile_dy, (row.h - Metrics::reference().tile) / 2.0);
    }

    #[test]
    fn results_layout_is_one_panel_of_58_rows() {
        let l = layout_results(3, 1000.0, 1.0);
        assert_eq!(l.section.rows.len(), 3);
        for r in &l.section.rows {
            assert_eq!(r.h, 58.0);
            assert_eq!(r.block, BlockKind::Row);
        }
        assert_eq!(l.section.panel_h, 2.0 + 3.0 * 58.0);
        assert_eq!(layout_results(0, 1000.0, 1.0).section.rows.len(), 0);
    }

    #[test]
    fn scroll_target_centres_the_row_and_clamps() {
        let l = laid_out("appearance", 1000.0, 1.0);
        let row = l.sections[0].rows[0].clone();
        let t = scroll_target(&l, row.id, 400.0).expect("the first row has an id");
        assert_eq!(t, centre_scroll(row.y, row.h, 400.0, l.content_h));
        assert!(t >= 0.0 && t <= (l.content_h - 400.0).max(0.0));
        assert_eq!(scroll_target(&l, "no-such-row", 400.0), None);
    }

    #[test]
    fn the_select_menu_opens_below_right_aligned_flips_up_when_cramped_and_stays_on_screen() {
        let window = (1000.0, 600.0);
        let high = Rect {
            x: 700.0,
            y: 100.0,
            w: 168.0,
            h: 32.0,
        };
        let (x, y, h) = place_menu(high, 200.0, 500.0, window, 300.0);
        assert_eq!(x, 700.0 + 168.0 - 200.0);
        assert_eq!(y, 100.0 + 32.0 + 4.0);
        assert_eq!(h, 300.0);
        let low = Rect {
            x: 700.0,
            y: 560.0,
            w: 168.0,
            h: 32.0,
        };
        let (_, y, h) = place_menu(low, 200.0, 500.0, window, 300.0);
        assert!(
            y + h <= 560.0 - 4.0 + 1e-9,
            "the menu opens upward and stays above the trigger"
        );
        assert!(y >= 8.0, "the menu stays on screen");
        let edge = Rect {
            x: 2.0,
            y: 10.0,
            w: 20.0,
            h: 20.0,
        };
        let (x, _, _) = place_menu(edge, 200.0, 100.0, window, 300.0);
        assert_eq!(x, 8.0, "never left of 8");
    }
}
