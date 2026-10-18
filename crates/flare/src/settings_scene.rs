//! The Settings scene for headless captures and tests (M7 Task 51).
//!
//! `--scene settings` draws the Settings tab in one fixed state: a fixed fake environment (the
//! shells, the Explorer verb, the version, the glass, the log folder, no NVIDIA) over a fixed
//! in-memory store, so a Flare image and a Prism Terminal image show the same thing. Nothing here
//! reads or writes a profile folder: the store is `PrefStore::memory()`, the diagnostics control
//! is a fake, and no path under the user's data folder is named.
//!
//! This file holds the fixed environment and the geometry that finds a control; the window driving
//! (`stage_settings`) is in `headless.rs`, next to the other scenes, because it needs the scene's
//! private window and clock.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use flare_settings::kv::PrefStore;
use flare_settings::layout::{Metrics, is_narrow, rail_width};
use flare_settings::look::LookStore;
use flare_settings::new_tab::{NewTabMode, set_new_tab_mode};
use flare_settings::page::{GlassSupport, PlaceholderDictation};
use flare_settings::tab_layout::{TabLayout, set_tab_layout};
use flare_settings::verb::{FakeDiag, FakeVerb, VerbState};
use flare_settings::window_colours::set_window_edges;
use flare_theme::edges::WindowEdges;
use slint::{ComponentHandle, Model};

use crate::appctl::Post;
use crate::settings_ctl::SettingsDeps;
use crate::{AppWindow, SetKind, SettingsBridge};

/// The shells the scene lists, as `(id, name)`.
pub const SHELLS: [(&str, &str); 4] = [
    ("pwsh", "PowerShell 7"),
    ("powershell", "Windows PowerShell"),
    ("cmd", "Command Prompt"),
    ("wsl-ubuntu", "Ubuntu"),
];

/// The version the About page and the scene's rows show.
pub const VERSION: &str = "0.0.0-scene";

/// The diagnostics folder the Diagnostics page names (never created, never read).
pub const LOG_DIR: &str = r"C:\Users\You\AppData\Roaming\Flare\logs";

/// The copyright line of the About card (a fixed text: the scene reads no licence file).
pub const COPYRIGHT: &str = "Copyright 2026 The Flare authors.";

/// The accent that makes the theme dirty for the switch question.
pub const DIRTY_ACCENT: &str = "#e07a2f";

/// How far the pane is scrolled (css px) when the wall is shown open.
pub const WALL_SCROLL: f32 = 330.0;

/// A control row's right padding and a control's shape, in css px (Prism Terminal's classes).
const ROW_PAD_RIGHT: f64 = 16.0;
const SELECT_W: f64 = 168.0;
const CONTROL_H: f64 = 32.0;
const SWATCH_W: f64 = 36.0;
const SWATCH_H: f64 = 24.0;

/// The fixed store: the values the Prism Terminal capture script sets, everything else default.
pub fn prep(prefs: &PrefStore) {
    let look = LookStore::new(prefs.clone());
    look.set_theme_id("pt-default");
    look.set_font_id("cascadia");
    look.set_font_pct(100);
    set_window_edges(prefs, WindowEdges::Hairline);
    set_tab_layout(prefs, TabLayout::Horizontal);
    set_new_tab_mode(prefs, NewTabMode::Folder, None);
}

/// The Settings controller's ports in the scene: fixed answers, no profile, no clipboard, no
/// folder picker, no opened path. `post` is the scene's queue.
pub fn deps(prefs: &PrefStore, post: Post, clock: Rc<dyn Fn() -> u64>) -> SettingsDeps {
    let look = LookStore::new(prefs.clone());
    SettingsDeps {
        prefs: prefs.clone(),
        look: LookStore::new(prefs.clone()),
        diag: Rc::new(FakeDiag::new(LOG_DIR, false)),
        verb: Arc::new(FakeVerb::new(VerbState::Off)),
        dictation: Rc::new(PlaceholderDictation),
        pick_folder: Rc::new(|_, done| done(None::<PathBuf>)),
        open_path: Rc::new(|_| {}),
        clipboard: Rc::new(|_: &str| {}),
        shell_list: Arc::new(|| {
            SHELLS
                .iter()
                .map(|(id, name)| ((*id).to_string(), (*name).to_string()))
                .collect()
        }),
        glass: Rc::new(|| GlassSupport::Ok),
        face_of: Rc::new(|id: &str| {
            flare_theme::fonts::font_families(id)
                .into_iter()
                .next()
                .unwrap_or_default()
        }),
        theme_colours: {
            let prefs = prefs.clone();
            Rc::new(move || flare_settings::page::appearance::theme_colours(&look, &prefs, false))
        },
        agent_colours: {
            let prefs = prefs.clone();
            Rc::new(move || crate::settings_ctl::agent_colours_in_force(&prefs))
        },
        agent_key_labels: Rc::new(|| {
            flare_tabs::keymap::agent_chords()
                .into_iter()
                .map(|id| flare_tabs::keymap::key_label(id, true))
                .collect::<Vec<_>>()
                .join(", ")
        }),
        post,
        version: VERSION.to_string(),
        copyright: COPYRIGHT.to_string(),
        e2e: true,
        clock,
        // No animation: the wall opens at once and nothing is left running for the capture.
        still: Rc::new(|| true),
    }
}

/// Where a row's control sits in the window, in css px: `[x, y, w, h]` of its trigger or swatch.
/// Slint reports the real rectangle when a control is clicked; the scene opens a menu or a picker
/// without a click, so it computes the same box from the layout the controller gave the bridge.
pub fn control_anchor(
    app: &AppWindow,
    window_css: (f64, f64),
    scale: f64,
    row_id: &str,
    kind: SetKind,
) -> Option<[f64; 4]> {
    let bridge = app.global::<SettingsBridge>();
    let m = Metrics::reference();
    let (pane_w, pane_h) = (
        f64::from(bridge.get_pane_w()),
        f64::from(bridge.get_pane_h()),
    );
    if pane_w <= 0.0 || pane_h <= 0.0 {
        return None;
    }
    let wide = pane_w + rail_width(1000.0);
    let frame_w = if is_narrow(wide) {
        pane_w + rail_width(0.0)
    } else {
        wide
    };
    let rail = rail_width(frame_w);
    let pad_x = if is_narrow(frame_w) {
        m.pad_x_narrow
    } else {
        m.pad_x
    };
    let content_w = (m.column_max.min(pane_w) - 2.0 * pad_x).max(0.0);
    // The view fills the window right of the sidebar and below the strip.
    let origin = (window_css.0 - frame_w, window_css.1 - pane_h);
    let hair = 1.0_f64.max(scale.floor()) / scale;
    let right = origin.0 + rail + pad_x + content_w - hair - ROW_PAD_RIGHT;
    for section in bridge.get_sections().iter() {
        for row in section.rows.iter() {
            if row.id != row_id {
                continue;
            }
            let mid = origin.1 + f64::from(section.panel_y) + hair + f64::from(row.y)
                - f64::from(bridge.get_scroll_y())
                + f64::from(row.h) / 2.0;
            return Some(match kind {
                SetKind::Select => [right - SELECT_W, mid - CONTROL_H / 2.0, SELECT_W, CONTROL_H],
                _ => [right - SWATCH_W, mid - SWATCH_H / 2.0, SWATCH_W, SWATCH_H],
            });
        }
    }
    None
}

/// The scroll (css px) that brings `anchor` into the pane, centred, or `None` when the whole box is
/// in view already. This is what a click on a control below the fold does (Playwright scrolls
/// the element into the middle), so the scene and the Prism Terminal capture agree.
pub fn reveal_scroll(app: &AppWindow, window_css: (f64, f64), anchor: [f64; 4]) -> Option<f32> {
    let bridge = app.global::<SettingsBridge>();
    let pane_h = f64::from(bridge.get_pane_h());
    let top = window_css.1 - pane_h;
    if anchor[1] >= top && anchor[1] + anchor[3] <= top + pane_h {
        return None;
    }
    let scroll = f64::from(bridge.get_scroll_y());
    let mid = anchor[1] + anchor[3] / 2.0;
    let want = scroll + mid - (top + pane_h / 2.0);
    let most = (f64::from(bridge.get_content_h()) - pane_h).max(0.0);
    Some(want.clamp(0.0, most) as f32)
}

/// The id of the first row of `kind` on the page now in the bridge.
pub fn first_row_of(app: &AppWindow, kind: SetKind) -> Option<String> {
    let bridge = app.global::<SettingsBridge>();
    for section in bridge.get_sections().iter() {
        for row in section.rows.iter() {
            if row.kind == kind && !row.id.is_empty() {
                return Some(row.id.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Args, RendererChoice, SettingsOpen, parse_args};
    use crate::headless::run_scene;
    use flare_settings::pages::PAGES;

    fn scene_args(extra: &[&str], out: &std::path::Path) -> Args {
        let mut v: Vec<String> = ["--scene", "settings", "--renderer", "software"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        v.extend(extra.iter().map(|s| (*s).to_string()));
        v.push("--screenshot".to_string());
        v.push(out.to_string_lossy().into_owned());
        parse_args(&v, None).unwrap()
    }

    fn decode(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
        let file = std::fs::File::open(path).unwrap();
        let mut reader = png::Decoder::new(std::io::BufReader::new(file))
            .read_info()
            .unwrap();
        let mut buf = vec![0; reader.output_buffer_size().expect("png size")];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    fn colours(rgba: &[u8], limit: usize) -> usize {
        let mut seen = std::collections::HashSet::new();
        for px in rgba.as_chunks::<4>().0 {
            seen.insert([px[0], px[1], px[2]]);
            if seen.len() > limit {
                break;
            }
        }
        seen.len()
    }

    #[test]
    fn the_scene_renders_every_page_without_a_window_and_is_not_blank() {
        let dir = tempfile::tempdir().unwrap();
        for page in PAGES {
            let png = dir.path().join(format!("{}.png", page.id));
            let args = scene_args(&["--settings-page", page.id], &png);
            assert_eq!(args.settings_open, SettingsOpen::None);
            assert_eq!(args.renderer, RendererChoice::Software);
            run_scene(&args).unwrap_or_else(|e| panic!("{}: {e}", page.id));
            let (w, h, rgba) = decode(&png);
            assert_eq!((w, h), (1000, 640), "{}", page.id);
            assert!(colours(&rgba, 3) > 3, "{} is blank", page.id);
        }
    }

    #[test]
    fn the_scene_state_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.png");
        run_scene(&scene_args(&["--settings-page", "terminal"], &a)).unwrap();
        run_scene(&scene_args(&["--settings-page", "terminal"], &b)).unwrap();
        assert_eq!(decode(&a).2, decode(&b).2, "two runs differ");
        // A different page really is a different image.
        let c = dir.path().join("c.png");
        run_scene(&scene_args(&["--settings-page", "about"], &c)).unwrap();
        assert_ne!(decode(&a).2, decode(&c).2);
    }

    #[test]
    fn every_overlay_and_a_find_result_renders() {
        let dir = tempfile::tempdir().unwrap();
        let none = dir.path().join("none.png");
        run_scene(&scene_args(&[], &none)).unwrap();
        let base = decode(&none).2;
        for open in ["picker", "editor", "ask", "help", "wall-expanded"] {
            let png = dir.path().join(format!("{open}.png"));
            run_scene(&scene_args(&["--settings-open", open], &png))
                .unwrap_or_else(|e| panic!("{open}: {e}"));
            assert_ne!(decode(&png).2, base, "{open} changed nothing");
        }
        let select = dir.path().join("select.png");
        run_scene(&scene_args(
            &["--settings-page", "terminal", "--settings-open", "select"],
            &select,
        ))
        .unwrap();
        let terminal = dir.path().join("terminal.png");
        run_scene(&scene_args(&["--settings-page", "terminal"], &terminal)).unwrap();
        assert_ne!(
            decode(&select).2,
            decode(&terminal).2,
            "the menu is not open"
        );
        let find = dir.path().join("find.png");
        run_scene(&scene_args(&["--settings-find", "colour"], &find)).unwrap();
        assert_ne!(decode(&find).2, base, "the find result is not shown");
        // A page with no select cannot show one: an error, never a silent plain page.
        let none = dir.path().join("noselect.png");
        let err = run_scene(&scene_args(&["--settings-open", "select"], &none)).unwrap_err();
        assert!(err.contains("select"), "{err}");
    }

    #[test]
    fn the_scene_never_touches_the_real_profile() {
        // The scene's source names no data folder and no real store: the store is in memory and
        // the diagnostics control is a fake. The needles are split so this test does not match itself.
        let scene = include_str!("settings_scene.rs");
        let stage = include_str!("headless.rs");
        let fn_at = stage.find("fn stage_settings").expect("the stage exists");
        let stage = &stage[fn_at..];
        let end = stage.find("\n    }\n").unwrap_or(stage.len());
        let stage = &stage[..end];
        for text in [scene, stage] {
            for needle in [
                ["APP", "DATA"].concat(),
                ["Pref", "Store::new("].concat(),
                ["Pref", "Store::open("].concat(),
                ["Diag", "Control::new("].concat(),
                ["Profile", "Paths"].concat(),
                ["data", "_dir"].concat(),
            ] {
                let scan: String = text
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .filter(|l| !l.contains(".concat()"))
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(!scan.contains(&needle), "the scene names {needle}");
            }
        }
        assert!(scene.contains("FakeDiag::new(LOG_DIR"));
        let builder = include_str!("headless.rs");
        assert!(builder.contains("let prefs = PrefStore::memory();"));
        // The fixed store is the scene's own: a run leaves the prepared values readable.
        let prefs = PrefStore::memory();
        prep(&prefs);
        assert_eq!(LookStore::new(prefs).theme_id(), "pt-default");
    }
}
