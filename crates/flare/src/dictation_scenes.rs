//! The dictation scenes that are not the main window (M8 Task 35): the widget card, which is a window of its own, and
//! the tab mark on the real strip and sidebar geometry. `--scene <name>` draws them like every other scene (the
//! DPI matrix, `tools/dpi-matrix`, reads the names from `--scene list`), through the same scene platform and clock, so
//! the matrix makes one image per scene, scaling and renderer. They are windows of their own size, not 1000 x 640:
//! `tools/dpi-matrix/check.py` keeps the same sizes in `SCENE_SIZE`, and a test here holds the two together.
//!
//! The grid renderer (software or GPU) draws only the terminal's cells, and these scenes have none, so the two
//! renderers make the same pixels here (the matrix still checks that).

use std::rc::Rc;

use flare_dictation::route::OverlayRow;
use flare_dictation::service::{OverlayState, Tone, WIDGET_WIDTH};
use slint::platform::WindowEvent;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::{ComponentHandle, ModelRc, PhysicalSize, VecModel};

use crate::dictation_widget::{face_of, set_face};
use crate::headless::{advance_scene_clock, offer_scene_window, straight_rgba};
use crate::{DictationTabMarkScene, DictationWidget, WidgetItem};

/// The scenes this file draws, in `SCENES` order.
pub const COMPONENT_SCENES: [&str; 7] = [
    "dictation-mark-strip",
    "dictation-mark-sidebar",
    "dictation-widget-listening",
    "dictation-widget-starting",
    "dictation-widget-pick",
    "dictation-widget-sent",
    "dictation-widget-warning",
];

/// The strip and the sidebar column of the mark scenes, in css px (the scene component's own size).
pub const STRIP_CSS: (u32, u32) = (400, 36);
pub const SIDEBAR_CSS: (u32, u32) = (260, 148);

/// The words the widget scenes dictate.
const SPOKEN: &str = "check the build";

/// Whether `name` is drawn by this file.
pub fn is_component_scene(name: &str) -> bool {
    COMPONENT_SCENES.contains(&name)
}

fn row(id: &str, n: u8, group: &str) -> OverlayRow {
    OverlayRow {
        id: id.into(),
        n,
        label: format!("tab {n}"),
        folder: "Flare".into(),
        state: None,
        disabled: None,
        lead: false,
        group: Some(group.to_string()),
    }
}

/// The widget's state in a widget scene.
fn widget_state(name: &str) -> Option<OverlayState> {
    Some(match name {
        "dictation-widget-listening" => OverlayState::Listening {
            text: SPOKEN.into(),
            level: 0.5,
            starting: false,
        },
        "dictation-widget-starting" => OverlayState::Listening {
            text: String::new(),
            level: 0.0,
            starting: true,
        },
        // Two groups: the Flare folder's two sessions and a second folder's one.
        "dictation-widget-pick" => OverlayState::Pick {
            text: SPOKEN.into(),
            rows: vec![
                row("a", 1, "Flare"),
                row("b", 2, "Flare"),
                row("c", 3, "Flare (2)"),
            ],
        },
        "dictation-widget-sent" => OverlayState::Sent { to: "Flare".into() },
        "dictation-widget-warning" => OverlayState::Note {
            message: "No Claude session, copied".into(),
            tone: Tone::Warn,
        },
        _ => return None,
    })
}

/// The scene's window size in css px: the widget is 480 wide and as tall as its face needs.
pub fn css_size(name: &str) -> Option<(u32, u32)> {
    match name {
        "dictation-mark-strip" => Some(STRIP_CSS),
        "dictation-mark-sidebar" => Some(SIDEBAR_CSS),
        _ => widget_state(name).map(|s| (WIDGET_WIDTH, face_of(&s).height)),
    }
}

/// Draws the scene at `scale`: its physical size and straight RGBA pixels.
pub fn render(name: &str, scale: f32) -> Result<(u32, u32, Vec<u8>), String> {
    render_with(name, scale, true)
}

/// `render`, with the mark shown or not (the tests compare the two).
pub fn render_with(name: &str, scale: f32, mark: bool) -> Result<(u32, u32, Vec<u8>), String> {
    let css = css_size(name).ok_or_else(|| format!("no dictation component scene '{name}'"))?;
    let phys = (
        (css.0 as f32 * scale).round() as u32,
        (css.1 as f32 * scale).round() as u32,
    );
    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    offer_scene_window(&win)?;
    let place = |win: &Rc<MinimalSoftwareWindow>| {
        win.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        win.set_size(PhysicalSize::new(phys.0, phys.1));
    };
    // The component stays alive until the pixels are read.
    let _keep: Box<dyn std::any::Any> = if let Some(state) = widget_state(name) {
        let comp = DictationWidget::new().map_err(|e| format!("cannot create the widget: {e}"))?;
        let items = Rc::new(VecModel::<WidgetItem>::default());
        comp.set_items(ModelRc::from(items.clone()));
        set_face(&comp, &items, &face_of(&state));
        place(&win);
        comp.show()
            .map_err(|e| format!("cannot show the widget: {e}"))?;
        Box::new((comp, items))
    } else {
        let comp = DictationTabMarkScene::new()
            .map_err(|e| format!("cannot create the mark scene: {e}"))?;
        comp.set_sidebar(name == "dictation-mark-sidebar");
        comp.set_active(mark);
        place(&win);
        comp.show()
            .map_err(|e| format!("cannot show the mark scene: {e}"))?;
        Box::new(comp)
    };
    advance_scene_clock(500);
    let mut pixels = vec![
        slint::platform::software_renderer::PremultipliedRgbaColor::default();
        (phys.0 * phys.1) as usize
    ];
    win.request_redraw();
    win.draw_if_needed(|r| {
        r.render(&mut pixels, phys.0 as usize);
    });
    Ok((
        phys.0,
        phys.1,
        pixels.iter().flat_map(straight_rgba).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_args;
    use crate::headless::{
        SCENES, Scene, is_dictation_page_scene, is_dictation_pill_scene, scene_args, setup,
    };
    use crate::ui_harness::SCALES;
    use slint::Model;

    /// Every dictation scene, in the order `SCENES` lists them.
    fn dictation_scenes() -> Vec<&'static str> {
        SCENES
            .iter()
            .copied()
            .filter(|s| s.starts_with("dictation-"))
            .collect()
    }

    /// A window scene (the pill or the Dictation page) staged and drawn at `scale`.
    fn staged(name: &str, scale: f32) -> (Scene, Vec<u8>) {
        let argv: Vec<String> = [
            "--scene",
            name,
            "--scale",
            &scale.to_string(),
            "--size",
            "1000x640",
            "--renderer",
            "software",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let args = scene_args(name, &parse_args(&argv, None).expect("the arguments parse"));
        if is_dictation_page_scene(name) {
            // The page scenes stage the download runs, which a page test clears: one at a time.
            crate::dictation_ctl::fixture::hold_page_state();
        }
        let (saved, prep) = setup(name);
        let scene = Scene::build_with(&args, saved, prep, false, false).expect("the scene builds");
        if is_dictation_page_scene(name) {
            // The GPU card needs NVIDIA: the scene says so, as `run_scene` does.
            // SAFETY: one fixed value, set before the answer is read; no test reads this variable another way.
            unsafe { std::env::set_var(flare_win::names::SEAM_NVIDIA, "1") };
            flare_win::seam::set_open(true);
            scene.stage_settings(&args).expect("the page stages");
        } else {
            scene.stage(name).expect("the scene stages");
        }
        let pixels = scene.render();
        (scene, pixels)
    }

    fn px(frame: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * w + x) * 4) as usize;
        [frame[i], frame[i + 1], frame[i + 2]]
    }

    fn far(a: [u8; 3], b: [u8; 3], by: u8) -> bool {
        a.iter().zip(b).any(|(x, y)| x.abs_diff(y) > by)
    }

    /// The rows of `y0..y1` that hold ink (a pixel far from the region's ground) in the columns `x0..x1`, as runs.
    fn ink_runs(frame: &[u8], w: u32, x: (u32, u32), y: (u32, u32)) -> usize {
        let ground = px(frame, w, x.0, y.0);
        let mut runs = 0;
        let mut inside = false;
        for row in y.0..y.1 {
            let inked = (x.0..x.1).any(|col| far(px(frame, w, col, row), ground, 60));
            if inked && !inside {
                runs += 1;
            }
            inside = inked;
        }
        runs
    }

    #[test]
    fn the_scene_list_holds_the_fifteen_dictation_scenes_once_each() {
        let names = dictation_scenes();
        assert_eq!(names.len(), 15, "{names:?}");
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "no name repeats");
        for name in COMPONENT_SCENES {
            assert!(names.contains(&name), "{name} is listed");
        }
        assert_eq!(
            names.iter().filter(|s| is_dictation_pill_scene(s)).count(),
            4
        );
        assert_eq!(
            names.iter().filter(|s| is_dictation_page_scene(s)).count(),
            4
        );
    }

    /// `tools/dpi-matrix/check.py` checks each image's size against its own table; it must say what the app draws.
    #[test]
    fn check_py_knows_each_component_scene_size() {
        let py = include_str!("../../../tools/dpi-matrix/check.py");
        for name in COMPONENT_SCENES {
            let (w, h) = css_size(name).expect("a size");
            let want = format!("\"{name}\": ({w}, {h})");
            assert!(py.contains(&want), "check.py SCENE_SIZE lacks {want}");
        }
    }

    #[test]
    fn every_component_scene_is_its_size_and_not_blank_at_every_scale() {
        for name in COMPONENT_SCENES {
            let css = css_size(name).unwrap();
            for scale in SCALES {
                let (w, h, rgba) = render(name, scale).expect("the scene draws");
                assert_eq!(
                    (w, h),
                    (
                        (css.0 as f32 * scale).round() as u32,
                        (css.1 as f32 * scale).round() as u32
                    ),
                    "{name} at {scale}"
                );
                let ground = px(&rgba, w, 0, 0);
                let ink = (0..w * h)
                    .filter(|i| far(px(&rgba, w, i % w, i / w), ground, 40))
                    .count();
                assert!(
                    ink > (w * h / 50) as usize,
                    "{name} at {scale}: {ink} inked"
                );
            }
        }
    }

    /// The mark: with it and without it the strip and the sidebar differ only in the target's end box, which is
    /// 18 css px square, and the difference is accent ink.
    #[test]
    fn the_mark_is_a_microphone_in_the_target_end_box_on_the_strip_and_the_sidebar() {
        // (scene, the end box's left and top in css px, the size of the box)
        for (name, (bx, by)) in [
            ("dictation-mark-strip", (222.0, 9.0)),
            ("dictation-mark-sidebar", (226.0, 65.0)),
        ] {
            for scale in SCALES {
                let (w, h, on) = render_with(name, scale, true).unwrap();
                let (_, _, off) = render_with(name, scale, false).unwrap();
                let (mut count, mut inside) = (0, true);
                let (x0, y0) = (((bx - 1.0) * scale) as u32, ((by - 1.0) * scale) as u32);
                let (x1, y1) = (
                    ((bx + 19.0) * scale).ceil() as u32,
                    ((by + 19.0) * scale).ceil() as u32,
                );
                for y in 0..h {
                    for x in 0..w {
                        if far(px(&on, w, x, y), px(&off, w, x, y), 8) {
                            count += 1;
                            inside &= x >= x0 && x < x1 && y >= y0 && y < y1;
                        }
                    }
                }
                assert!(
                    inside,
                    "{name} at {scale}: the mark drew outside its end box"
                );
                assert!(
                    count as f32 > 12.0 * scale * scale,
                    "{name} at {scale}: only {count} marked pixels"
                );
            }
        }
    }

    /// The pill, in the real window: it appears (the picture differs from the same window without it), only
    /// at the bottom centre of the terminal area, and nothing else in the picture moves.
    #[cfg_attr(
        not(windows),
        ignore = "Windows fonts and text metrics; macOS phase 2 of the port plan (fonts)"
    )]
    #[test]
    fn the_pill_scenes_draw_the_pill_at_the_bottom_centre_at_every_scale() {
        for name in [
            "dictation-pill-listening",
            "dictation-pill-starting",
            "dictation-pill-transcribing",
            "dictation-pill-message",
        ] {
            for scale in SCALES {
                let (scene, with) = staged(name, scale);
                scene.app.set_dict_phase(0);
                let without = scene.render();
                let w = scene.phys.0;
                let (mut x0, mut x1, mut y0, mut y1, mut n) = (u32::MAX, 0, u32::MAX, 0, 0);
                for y in 0..scene.phys.1 {
                    for x in 0..w {
                        if far(px(&with, w, x, y), px(&without, w, x, y), 6) {
                            (x0, x1, y0, y1, n) =
                                (x0.min(x), x1.max(x), y0.min(y), y1.max(y), n + 1);
                        }
                    }
                }
                assert!(n > 0, "{name} at {scale}: no pill");
                // Centred on the terminal area (the strip is horizontal: the area is the whole width).
                let centre = (x0 + x1) as f32 / 2.0;
                assert!(
                    (centre - w as f32 / 2.0).abs() <= 2.0 * scale,
                    "{name} at {scale}: the pill is centred at {centre} of {w}"
                );
                // In the lower part of the window, 16 css px above the foot at most plus its shadow.
                assert!(
                    y0 as f32 > scene.phys.1 as f32 * 0.8,
                    "{name} at {scale}: the pill starts at row {y0}"
                );
                assert!(
                    y1 < scene.phys.1,
                    "{name} at {scale}: the pill is in the window"
                );
            }
        }
    }

    /// The Dictation page through the real Settings frame: the page title, the cards in Prism Terminal's order, the
    /// label of every control, and ink in the label column (the old sheets drew bare controls with no labels).
    #[cfg_attr(
        not(windows),
        ignore = "Windows fonts and text metrics; macOS phase 2 of the port plan (fonts)"
    )]
    #[test]
    fn the_page_scenes_show_the_real_page_with_its_labels_at_every_scale() {
        for scale in SCALES {
            let (scene, pixels) = staged("dictation-page-top", scale);
            let bridge = scene.app.global::<crate::SettingsBridge>();
            assert_eq!(bridge.get_page_title().as_str(), "Dictation");
            let sections = bridge.get_sections();
            let ids: Vec<String> = (0..sections.row_count())
                .map(|i| sections.row_data(i).unwrap().id.to_string())
                .collect();
            let nvidia = flare_dictation::gpu::has_nvidia();
            let mut want = vec!["dictation", "listening", "while", "models"];
            if nvidia {
                want.push("gpu");
            }
            want.push("anywhere");
            assert_eq!(ids, want, "at {scale}");
            let labels: Vec<String> = (0..sections.row_count())
                .flat_map(|i| {
                    let rows = sections.row_data(i).unwrap().rows;
                    (0..rows.row_count())
                        .map(|r| rows.row_data(r).unwrap().label.to_string())
                        .collect::<Vec<_>>()
                })
                .collect();
            for label in [
                "Use dictation",
                "Dictation key",
                "Microphone",
                "Dictate from any app",
            ] {
                assert!(
                    labels.iter().any(|l| l == label),
                    "{label} at {scale}: {labels:?}"
                );
            }
            // The label column: the title, the master row, and four rows of the Listening card.
            let w = scene.phys.0;
            let s = |css: f32| (css * scale).round() as u32;
            let runs = ink_runs(&pixels, w, (s(348.0), s(520.0)), (s(60.0), s(600.0)));
            assert!(
                runs >= 12,
                "the label column holds {runs} lines of ink at {scale}"
            );
        }
    }

    /// The models card at a download in progress, the question, and the foot of the page.
    #[cfg_attr(
        not(windows),
        ignore = "Windows fonts and text metrics; macOS phase 2 of the port plan (fonts)"
    )]
    #[test]
    fn the_models_scenes_show_the_rows_their_state_and_the_question() {
        for scale in SCALES {
            let (scene, pixels) = staged("dictation-page-models", scale);
            let sections = scene.app.global::<crate::SettingsBridge>().get_sections();
            let models = (0..sections.row_count())
                .map(|i| sections.row_data(i).unwrap())
                .find(|s| s.id.as_str() == "models")
                .expect("the models card");
            let rows: Vec<_> = (0..models.rows.row_count())
                .map(|r| models.rows.row_data(r).unwrap())
                .collect();
            assert!(rows.len() >= 5, "{} model rows", rows.len());
            assert_eq!(rows[0].state.as_str(), "active");
            assert_eq!(rows[1].state.as_str(), "downloading");
            assert_eq!(rows[1].num, 50);
            assert_eq!(rows[2].state.as_str(), "absent");
            for r in &rows {
                assert!(!r.label.is_empty() && !r.size_text.is_empty(), "{:?}", r.id);
            }
            let w = scene.phys.0;
            let s = |css: f32| (css * scale).round() as u32;
            // Every model row has its name and sub line: two bands of ink each, in the name column.
            let runs = ink_runs(&pixels, w, (s(348.0), s(520.0)), (s(160.0), s(480.0)));
            assert!(
                runs >= 10,
                "the models card holds {runs} lines of ink at {scale}"
            );
            let (ask, _) = staged("dictation-page-ask", scale);
            let bridge = ask.app.global::<crate::SettingsBridge>();
            assert!(
                bridge.get_dict_ask_open(),
                "the question is open at {scale}"
            );
        }
    }
}
