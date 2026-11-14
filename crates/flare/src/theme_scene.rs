//! The theme-sample scene and SAMPLE_STREAM (Task 28).
//!
//! `--scene theme-sample` is M3's one-tab `Flare` window whose grid shows [`SAMPLE_STREAM`] run through
//! M2's `Emulator`: a prompt, the sixteen colours as text and as blocks, bold hues, the styles and a
//! visible cursor. The same bytes are in `docs/theme-sample.txt`, which the Prism Terminal capture tool
//! types into the installed app, so the two pictures can be set side by side.

use flare_render::grid::Grid;
use flare_term::emulator::Emulator;
use flare_theme::edges::DEFAULT_WINDOW_EDGES;
use flare_theme::fonts::{base_font_px, font_families, font_id_or_default, font_pct_from};
use flare_theme::look::{AppliedLook, LookInput, apply_look};
use flare_theme::preset::{HOST_DEFAULT_THEME, find_preset, presets};
use flare_theme::retired::live_theme_id;
use flare_theme::see_through::SEE_THROUGH_TOP;

use crate::config::LookOverrides;
use crate::host::GridSource;

/// The bytes the scene prints (identical to `docs/theme-sample.txt`; CRLF line ends, ESC = `\x1b`).
/// Line 6 holds the sixteen colour blocks, two cells each from column 9; the last line is the prompt
/// with the cursor left visible after it.
pub const SAMPLE_STREAM: &str = concat!(
    "PS C:\\Users\\Admin\\Documents> Get-ChildItem -Recurse\r\n",
    "\r\n",
    "\x1b[1mFlare theme sample\x1b[0m  https://example.com/themes\r\n",
    "normal : \x1b[30m 0 \x1b[0m\x1b[31m 1 \x1b[0m\x1b[32m 2 \x1b[0m\x1b[33m 3 \x1b[0m",
    "\x1b[34m 4 \x1b[0m\x1b[35m 5 \x1b[0m\x1b[36m 6 \x1b[0m\x1b[37m 7 \x1b[0m\r\n",
    "bright : \x1b[90m 8 \x1b[0m\x1b[91m 9 \x1b[0m\x1b[92m10 \x1b[0m\x1b[93m11 \x1b[0m",
    "\x1b[94m12 \x1b[0m\x1b[95m13 \x1b[0m\x1b[96m14 \x1b[0m\x1b[97m15 \x1b[0m\r\n",
    "blocks : \x1b[40m  \x1b[0m\x1b[41m  \x1b[0m\x1b[42m  \x1b[0m\x1b[43m  \x1b[0m",
    "\x1b[44m  \x1b[0m\x1b[45m  \x1b[0m\x1b[46m  \x1b[0m\x1b[47m  \x1b[0m",
    "\x1b[100m  \x1b[0m\x1b[101m  \x1b[0m\x1b[102m  \x1b[0m\x1b[103m  \x1b[0m",
    "\x1b[104m  \x1b[0m\x1b[105m  \x1b[0m\x1b[106m  \x1b[0m\x1b[107m  \x1b[0m\r\n",
    "bold   : \x1b[1;31mred\x1b[0m \x1b[1;32mgreen\x1b[0m \x1b[1;33myellow\x1b[0m ",
    "\x1b[1;34mblue\x1b[0m \x1b[1;35mmagenta\x1b[0m \x1b[1;36mcyan\x1b[0m\r\n",
    "style  : \x1b[4munderline\x1b[0m \x1b[7minverse\x1b[0m \x1b[90mprediction\x1b[0m\r\n",
    "\r\n",
    "PS C:\\Users\\Admin\\Documents> ",
);

/// The grid of the scene: the stream run through the emulator at whatever size the window gives.
pub struct StreamSource {
    stream: &'static str,
    emulator: Emulator,
}

impl StreamSource {
    pub fn new(stream: &'static str) -> Self {
        Self {
            stream,
            emulator: Emulator::new(80, 24),
        }
    }
}

impl GridSource for StreamSource {
    fn resize(&mut self, cols: usize, rows: usize) {
        let mut emulator = Emulator::new(cols, rows);
        emulator.feed(self.stream.as_bytes());
        self.emulator = emulator;
    }

    fn grid(&mut self) -> Grid {
        self.emulator.snapshot()
    }
}

/// The ids a `--theme` takes, in the order of the presets.
pub fn valid_theme_ids() -> Vec<&'static str> {
    presets().iter().map(|p| p.id).collect()
}

/// What the scene wears: pt-default and the defaults with the one-run overrides over them.
/// `glass_possible` lets `--see-through` show (the picture then carries the ground's alpha).
/// An id that names no theme is an error that lists the valid ones.
pub fn scene_look(o: &LookOverrides, glass_possible: bool) -> Result<AppliedLook, String> {
    let id = o.theme.as_deref().unwrap_or(HOST_DEFAULT_THEME);
    if find_preset(live_theme_id(id)).is_none() {
        return Err(format!(
            "unknown theme '{id}': use one of {}",
            valid_theme_ids().join(", ")
        ));
    }
    Ok(apply_look(&LookInput {
        theme_id: id,
        custom: None,
        draft: None,
        background: o.background.as_deref(),
        accent: o.accent.as_deref(),
        secondary: o.secondary.as_deref(),
        edges: o.edges.unwrap_or(DEFAULT_WINDOW_EDGES),
        see_through: o.see_through.unwrap_or(0).min(SEE_THROUGH_TOP),
        glass_possible,
    }))
}

/// The face stack and base size a scene wears: the overrides over the defaults.
pub fn scene_font(o: &LookOverrides) -> (Vec<String>, i32) {
    let id = font_id_or_default(o.font.as_deref());
    let pct = o.font_pct.unwrap_or_else(|| font_pct_from(None));
    (font_families(id), base_font_px(pct))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome_apply::colour;
    use crate::config::{Args, parse_args};
    use crate::headless::{Scene, scene_gpu, setup};
    use crate::ui_harness::SCALES;
    use flare_render::color::Rgba;
    use flare_render::metrics::CellMetrics;

    /// The whole bytes of `docs/theme-sample.txt`.
    const SAMPLE_FILE: &str = include_str!("../../../docs/theme-sample.txt");
    /// A backdrop colour behind a see-through window.
    const BACKDROP: [u8; 3] = [0x33, 0x66, 0x99];
    /// The line (zero based) and first column of the sixteen blocks in the sample.
    const BLOCK_ROW: usize = 5;
    const BLOCK_COL: usize = 9;
    /// The prompt on the last line is `PS C:\Users\Admin\Documents> ` (29 cells), the cursor after it.
    const CURSOR_COL: usize = 29;
    const CURSOR_ROW: usize = 9;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Painter {
        Cpu,
        Gpu,
    }

    impl Painter {
        fn flag(self) -> &'static str {
            match self {
                Painter::Cpu => "software",
                Painter::Gpu => "gpu",
            }
        }
        /// The GPU path reads its texture back through a float pipeline: one level of slack.
        fn tolerance(self) -> u8 {
            match self {
                Painter::Cpu => 0,
                Painter::Gpu => 1,
            }
        }
    }

    fn scene(theme: &str, scale: f32, painter: Painter, extra: &[&str]) -> Option<(Scene, Args)> {
        let scale_text = scale.to_string();
        let mut argv = vec![
            "--scene",
            "theme-sample",
            "--theme",
            theme,
            "--scale",
            &scale_text,
            "--size",
            "1000x640",
            "--renderer",
            painter.flag(),
        ];
        argv.extend_from_slice(extra);
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let args = parse_args(&argv, None).expect("the arguments parse");
        if painter == Painter::Gpu && scene_gpu().is_err() {
            eprintln!("skipping: no GPU adapter");
            return None;
        }
        let (saved, prep) = setup("theme-sample");
        let built = Scene::build(&args, saved, prep, true).expect("the scene builds");
        Some((built, args))
    }

    /// Where the sixteen blocks, the cursor and a blank cell are, in window pixels.
    struct Geometry {
        blocks: [(u32, u32); 16],
        cursor: (u32, u32),
        blank: (u32, u32),
        /// The top-left of the grid image in the window.
        origin: (u32, u32),
        cell: (u32, u32),
    }

    /// The pixel box of each of the 16 blocks (line 6, column 9 + 2 i) and the cursor cell, from the cell
    /// metrics and the grid image top-left in the window.
    fn sample_geometry(m: &CellMetrics, origin: (u32, u32)) -> Geometry {
        let pad = m.padding;
        let at = |col: u32, row: u32, cells: u32| -> (u32, u32) {
            (
                origin.0 + pad + col * m.cell_w + cells * m.cell_w / 2,
                origin.1 + pad + row * m.cell_h + m.cell_h / 2,
            )
        };
        Geometry {
            // The middle of the two cells of block i.
            blocks: std::array::from_fn(|i| at((BLOCK_COL + 2 * i) as u32, BLOCK_ROW as u32, 2)),
            cursor: at(CURSOR_COL as u32, CURSOR_ROW as u32, 1),
            blank: at(70, 0, 1),
            origin,
            cell: (m.cell_w, m.cell_h),
        }
    }

    fn geometry_of(s: &Scene) -> Geometry {
        let m = s.host.borrow().metrics();
        let gw = s.app.get_grid_phys_width().max(0) as u32;
        let gh = s.app.get_grid_phys_height().max(0) as u32;
        sample_geometry(&m, (s.phys.0 - gw, s.phys.1 - gh))
    }

    fn rgba_of(frame: &[u8], w: u32, (x, y): (u32, u32)) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
    }

    fn near(got: [u8; 4], want: [u8; 4], tol: u8) -> bool {
        got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= tol)
    }

    fn hex(s: &str) -> [u8; 4] {
        let c = Rgba::from_hex(s).unwrap_or_else(|| panic!("not a colour: {s}"));
        [c.r, c.g, c.b, c.a]
    }

    fn opaque(s: &str) -> [u8; 4] {
        let c = hex(s);
        [c[0], c[1], c[2], 255]
    }

    /// `over` (straight alpha) composited on an opaque `under`, rounded.
    fn composite(over: [u8; 4], under: [u8; 3]) -> [u8; 4] {
        let a = f64::from(over[3]) / 255.0;
        let ch = |o: u8, u: u8| (f64::from(o) * a + f64::from(u) * (1.0 - a)).round() as u8;
        [
            ch(over[0], under[0]),
            ch(over[1], under[1]),
            ch(over[2], under[2]),
            255,
        ]
    }

    /// Every colour the sample shows, against what the theme resolved to.
    fn assert_scene(s: &Scene, look: &AppliedLook, tol: u8, label: &str) -> Geometry {
        let frame = s.render();
        let w = s.phys.0;
        let g = geometry_of(s);
        for (i, at) in g.blocks.iter().enumerate() {
            let ansi = look
                .term
                .ansi_at(i)
                .unwrap_or_else(|| panic!("{label}: no ansi {i}"));
            let want = opaque(ansi);
            let got = rgba_of(&frame, w, *at);
            assert!(
                near(got, want, tol),
                "{label}: block {i} at {at:?} is {got:?}, want {want:?}"
            );
        }
        let blank = rgba_of(&frame, w, g.blank);
        assert!(
            near(blank, opaque(&look.term.background), tol),
            "{label}: blank cell {blank:?}, want {}",
            look.term.background
        );
        // The caret wears the accent, not the theme's cursor colour (rule 1233).
        let cursor = rgba_of(&frame, w, g.cursor);
        assert!(
            near(cursor, opaque(&look.caret.fill), tol),
            "{label}: cursor cell {cursor:?}, want {}",
            look.caret.fill
        );
        let tabs = colour(&look.chrome.vars["--p-tabs"]);
        let title = rgba_of(&frame, w, (2, 2));
        assert!(
            near(
                title,
                [tabs.red(), tabs.green(), tabs.blue(), 255],
                tol.max(1)
            ),
            "{label}: title row {title:?}, want {tabs:?}"
        );
        // The active tab rule: a run of accent along the top edge of the strip.
        let rule = colour(&look.chrome.vars["--p-accent-hi"]);
        let want = [rule.red(), rule.green(), rule.blue(), 255];
        let run = (0..w)
            .filter(|&x| near(rgba_of(&frame, w, (x, 0)), want, tol.max(1)))
            .count();
        assert!(
            run as f32 >= 30.0 * s.scale,
            "{label}: only {run} pixels of the active tab rule {want:?}"
        );
        g
    }

    fn check_theme(theme: &str, scale: f32, painter: Painter) {
        let Some((s, args)) = scene(theme, scale, painter, &[]) else {
            return;
        };
        let look = scene_look(&args.look, true).expect("the theme is valid");
        let label = format!("{theme} at {scale} on {}", painter.flag());
        assert_scene(&s, &look, painter.tolerance(), &label);
    }

    #[test]
    fn every_theme_draws_its_own_sixteen_at_100_150_and_225_on_both_painters() {
        for painter in [Painter::Cpu, Painter::Gpu] {
            for id in valid_theme_ids() {
                for scale in [1.0_f32, 1.5, 2.25] {
                    check_theme(id, scale, painter);
                }
            }
        }
    }

    #[test]
    fn three_themes_hold_at_all_eight_scales() {
        for painter in [Painter::Cpu, Painter::Gpu] {
            for id in ["pt-default", "paper", "volt"] {
                for scale in SCALES {
                    check_theme(id, scale, painter);
                }
            }
        }
    }

    #[test]
    fn a_theme_switch_in_place_repaints_without_moving_a_cell() {
        let Some((s, args)) = scene("pt-default", 1.5, Painter::Cpu, &[]) else {
            return;
        };
        let before_look = scene_look(&args.look, true).expect("pt-default");
        let before = assert_scene(&s, &before_look, 0, "pt-default");
        let before_frame = s.render();
        let nord = scene_look(
            &LookOverrides {
                theme: Some("nord".into()),
                ..LookOverrides::default()
            },
            true,
        )
        .expect("nord");
        s.wear(&nord);
        let after = assert_scene(&s, &nord, 0, "nord after the switch");
        assert_eq!(before.blocks, after.blocks);
        assert_eq!(before.cursor, after.cursor);
        assert_eq!(before.origin, after.origin);
        assert_eq!(before.cell, after.cell);
        let after_frame = s.render();
        let red = before.blocks[1];
        assert_ne!(
            rgba_of(&before_frame, s.phys.0, red),
            rgba_of(&after_frame, s.phys.0, red),
            "the red block changed colour"
        );
    }

    #[test]
    fn glass_paints_the_ground_once() {
        for painter in [Painter::Cpu, Painter::Gpu] {
            let Some((s, args)) = scene("pt-default", 1.0, painter, &["--see-through", "40"])
            else {
                continue;
            };
            let look = scene_look(&args.look, true).expect("pt-default");
            assert!(look.glass, "level 40 is glass");
            let frame = s.render();
            let w = s.phys.0;
            let g = geometry_of(&s);
            // The blank cell is the ground at its alpha: one coat, not two.
            let ground = hex(&look.chrome.vars["--p-bg"]);
            let want = composite(ground, BACKDROP);
            let pixel = rgba_of(&frame, w, g.blank);
            // The frame is straight alpha, rounded down from premultiplied: two levels of slack on
            // the colour. The alpha is the proof of one coat (two coats would be 0.84, not 0.6).
            assert!(
                pixel[3].abs_diff(ground[3]) <= 1,
                "{}: blank cell alpha {} is not the ground alpha {}",
                painter.flag(),
                pixel[3],
                ground[3]
            );
            let got = composite(pixel, BACKDROP);
            assert!(
                near(got, want, 2),
                "{}: blank cell over the backdrop {got:?}, want {want:?}",
                painter.flag()
            );
            // A block stays opaque.
            let block = rgba_of(&frame, w, g.blocks[1]);
            assert_eq!(block[3], 255, "{}: the red block is opaque", painter.flag());
            assert!(
                near(
                    block,
                    opaque(look.term.ansi_at(1).expect("red")),
                    painter.tolerance()
                ),
                "{}: red block {block:?}",
                painter.flag()
            );
        }
    }

    #[test]
    fn the_sample_stream_is_the_files_bytes() {
        assert_eq!(SAMPLE_STREAM, SAMPLE_FILE);
        assert!(!SAMPLE_STREAM.contains('\u{2014}'));
        assert_eq!(
            SAMPLE_STREAM.matches("\r\n").count(),
            9,
            "ten lines, the last without an end"
        );
        assert!(SAMPLE_STREAM.ends_with("Documents> "));
    }

    #[test]
    fn the_sample_stream_puts_the_blocks_and_the_cursor_where_the_geometry_says() {
        let mut source = StreamSource::new(SAMPLE_STREAM);
        source.resize(100, 30);
        let grid = source.grid();
        let blank_bg = grid.cell(70, 0).bg;
        for i in 0..16 {
            for col in [BLOCK_COL + 2 * i, BLOCK_COL + 2 * i + 1] {
                let cell = grid.cell(col, BLOCK_ROW);
                assert_eq!(cell.ch, ' ');
                assert_ne!(cell.bg, blank_bg, "block {i} column {col} has a colour");
            }
        }
        assert_eq!(
            (grid.cursor.col, grid.cursor.row, grid.cursor.visible),
            (CURSOR_COL, CURSOR_ROW, true)
        );
        assert_eq!(grid.cell(70, 0).ch, ' ');
    }

    #[test]
    fn an_unknown_theme_names_the_valid_ids() {
        let err = scene_look(
            &LookOverrides {
                theme: Some("nope".into()),
                ..LookOverrides::default()
            },
            false,
        )
        .expect_err("nope is not a theme");
        assert!(err.contains("nope"));
        for id in valid_theme_ids() {
            assert!(err.contains(id), "{err} lists {id}");
        }
        assert_eq!(valid_theme_ids().len(), 39);
        assert!(!err.contains('\u{2014}'));
    }

    #[test]
    fn a_retired_id_is_worn_as_its_replacement() {
        let (old, new) = flare_theme::presets_data::RETIRED[0];
        let look = scene_look(
            &LookOverrides {
                theme: Some(old.into()),
                ..LookOverrides::default()
            },
            false,
        )
        .expect("a retired id still wears");
        assert_eq!(look.theme_id, new);
    }
}
