//! Settings > Appearance > App icon (rules 1230 and 1263), headless at 100 and 225 percent: the row's words over
//! twelve 36 px icon tiles 46 css px apart (layout A), the original first and picked on a fresh profile; a press on a
//! tile stores its id in the settings and the row follows. A narrow window wraps the tiles onto more lines and the row
//! grows by whole lines. The row's right holds the Restart Explorer (Restart Dock) button, never the icon's name; the
//! button runs the restart through the `ShellRestart` seam, here a fake, and a failure is one plain line under the
//! label. The tiles are found by the original icon's own red-orange flame, which no other design and no other part of
//! the page wears.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use flare_settings::app_icon::{
    APP_ICONS, FakeShellRestart, KEY, RESTART_BUTTON, RESTART_FAILED, RESTART_LABEL,
};
use flare_settings::kv::PrefStore;
use flare_settings::layout::{ICON_GAP, ICON_TILE};
use flare_settings::look::LookStore;
use flare_settings::page::{GlassSupport, ThemeColours};
use flare_settings::verb::{FakeDiag, FakeVerb, VerbState};
use slint::{ComponentHandle, Model};

use crate::appctl::{Post, UiMsg};
use crate::settings_ctl::{SettingsCtl, SettingsDeps};
use crate::ui_harness::{Frame, Harness};
use crate::{AppWindow, SetKind, SetRow, SettingsBridge};

const SCALES2: [f32; 2] = [1.0, 2.25];

/// One tile and its gap, in css px.
const STEP: f32 = (ICON_TILE + ICON_GAP) as f32;

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<SettingsCtl>>,
    prefs: PrefStore,
    q: Queue,
    restart: Arc<FakeShellRestart>,
}

fn fx_at(scale: f32, prefs: PrefStore) -> Fx {
    fx_sized(scale, prefs, 1000, Arc::new(FakeShellRestart::new()))
}

fn fx_sized(scale: f32, prefs: PrefStore, w: u32, restart: Arc<FakeShellRestart>) -> Fx {
    let look = LookStore::new(prefs.clone());
    let q: Queue = Arc::default();
    let post: Post = {
        let q = Arc::clone(&q);
        Arc::new(move |m| {
            q.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(m);
        })
    };
    let h = Harness::new(scale, w, 640, || AppWindow::new().unwrap());
    h.comp.set_front_kind(1);
    h.advance(0);
    let deps = SettingsDeps {
        prefs: prefs.clone(),
        look,
        diag: Rc::new(FakeDiag::new("C:/logs", false)),
        verb: Arc::new(FakeVerb::new(VerbState::Off)),
        shell_restart: restart.clone(),
        dictation: Rc::new(crate::dictation_page::DictationPageProvider),
        pick_folder: Rc::new(|_, done| done(None)),
        open_path: Rc::new(|_: &Path| {}),
        clipboard: Rc::new(|_: &str| {}),
        shell_list: Arc::new(|| vec![("pwsh".to_string(), "PowerShell".to_string())]),
        glass: Rc::new(|| GlassSupport::Ok),
        face_of: Rc::new(|_: &str| String::new()),
        theme_colours: Rc::new(|| ThemeColours {
            accent: String::new(),
            background: String::new(),
            secondary: String::new(),
        }),
        agent_colours: Rc::new(|| [String::new(), String::new(), String::new()]),
        agent_key_labels: Rc::new(String::new),
        post,
        version: "1.2.3".to_string(),
        copyright: "Copyright 2026".to_string(),
        e2e: true,
        clock: Rc::new(|| 1_000),
        still: Rc::new(|| false),
    };
    let ctl = SettingsCtl::new(&h.comp, deps);
    ctl.borrow_mut().show();
    let f = Fx {
        h,
        ctl,
        prefs,
        q,
        restart,
    };
    f.b().invoke_page_chosen("appearance".into());
    f.pump();
    f
}

impl Fx {
    fn b(&self) -> SettingsBridge<'_> {
        self.h.comp.global::<SettingsBridge>()
    }

    /// Hands every queued message to the controller, as the window's pump does, a few rounds.
    fn pump(&self) {
        for _ in 0..5 {
            let msgs: Vec<UiMsg> = self
                .q
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain(..)
                .collect();
            for m in msgs {
                if let UiMsg::Settings(m) = m {
                    self.ctl.borrow_mut().on_msg(m);
                }
            }
            self.h.advance(16);
        }
    }

    /// Pumps until `done` holds (the restart answers from a worker), at most 5 s.
    fn pump_until(&self, what: &str, done: impl Fn(&Fx) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "{what} never happened"
            );
            self.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn row(&self) -> SetRow {
        self.b()
            .get_sections()
            .iter()
            .flat_map(|s| s.rows.iter().collect::<Vec<_>>())
            .find(|r| r.id == "app-icon")
            .expect("the App icon row is drawn")
    }
}

/// The original's red-orange flame (full red, a little green, no blue): no other icon and nothing else on the page wears it.
fn original_orange(p: [u8; 4]) -> bool {
    p[0] > 240 && p[1] < 80 && p[2] < 12 && p[3] > 200
}

/// Daybreak's pale pink-to-blue ground, the only light tile among its neighbours.
fn daybreak_ground(p: [u8; 4]) -> bool {
    p[0] > 210 && p[1] > 215 && p[2] > 230
}

/// The centre of tile 0 in css px, found by the original's flame.
fn first_tile(frame: &Frame, scale: f32) -> (f32, f32) {
    let (x0, y0, x1, y1) = frame
        .bbox_where(0, 0, frame.w, frame.h, original_orange)
        .expect("the original's tile is drawn");
    let w = (x1 - x0) as f32 / scale;
    assert!(
        w > 8.0 && w < 36.0,
        "{scale}: one tile's flame, {w} css px wide"
    );
    (
        (x0 + x1) as f32 / 2.0 / scale,
        (y0 + y1) as f32 / 2.0 / scale,
    )
}

/// The centre of tile `i` in css px with `cols` tiles a line: 46 css px along, 46 down a line.
fn tile_centre(frame: &Frame, scale: f32, cols: usize, i: usize) -> (f32, f32) {
    let (cx, cy) = first_tile(frame, scale);
    (cx + STEP * (i % cols) as f32, cy + STEP * (i / cols) as f32)
}

/// How many of Daybreak's pale pixels lie in the 32 css px square round `(x, y)`.
fn pale_at(frame: &Frame, s: f32, (x, y): (f32, f32)) -> usize {
    let half = 16.0 * s;
    frame.count_where(
        ((x * s) - half) as u32,
        ((y * s) - half) as u32,
        ((x * s) + half) as u32,
        ((y * s) + half) as u32,
        daybreak_ground,
    )
}

#[test]
fn the_picker_shows_every_icon_on_one_line_with_the_original_picked() {
    for s in SCALES2 {
        let f = fx_at(s, PrefStore::memory());
        let r = f.row();
        assert_eq!(r.kind, SetKind::Icons, "{s}");
        assert_eq!(r.label.as_str(), "App icon");
        assert_eq!(
            r.sub.as_str(),
            flare_settings::page::appearance::APP_ICON_SUB
        );
        assert_eq!(r.value.as_str(), "original");
        let ids: Vec<String> = r.options.iter().map(|o| o.id.to_string()).collect();
        let want: Vec<String> = APP_ICONS.iter().map(|i| i.id.to_string()).collect();
        assert_eq!(ids, want);
        assert_eq!(r.options.row_data(0).unwrap().name, "Original, the default");
        assert_eq!(r.art.row_count(), APP_ICONS.len());
        for (i, art) in r.art.iter().enumerate() {
            let size = art.size();
            assert!(
                size.width > 0 && size.height > 0,
                "{s}: tile {i} has a picture"
            );
        }
        // One line at this width: the row is its 58 of words, 2, a 36 px line and 16 under it.
        assert_eq!(r.cols, 12, "{s}");
        assert!((f64::from(r.h) - 112.0).abs() < 1.0, "{s}: {}", r.h);
        let frame = f.h.render();
        let area = (32.0 * s) * (32.0 * s);
        // Daybreak, the fourth tile, is drawn 3 tiles along with its own colours; High Voltage beside it is dark.
        let day = pale_at(&frame, s, tile_centre(&frame, s, 12, 3));
        assert!(day as f32 > area * 0.3, "{s}: Daybreak's ground, {day} px");
        let hv = pale_at(&frame, s, tile_centre(&frame, s, 12, 2));
        assert!(
            (hv as f32) < area * 0.05,
            "{s}: High Voltage is dark, {hv} px"
        );
        // Nothing pale a line below Daybreak: the twelve tiles are one line.
        let below = {
            let (x, y) = tile_centre(&frame, s, 12, 3);
            pale_at(&frame, s, (x, y + STEP))
        };
        assert!(
            (below as f32) < area * 0.05,
            "{s}: {below} px under Daybreak"
        );
        let (lx, _) = tile_centre(&frame, s, 12, APP_ICONS.len() - 1);
        assert!(lx + 18.0 < 1000.0, "{s}: the last tile ends at {lx}");
    }
}

#[test]
fn a_narrow_window_wraps_the_tiles_and_the_row_grows_by_whole_lines() {
    for s in SCALES2 {
        let f = fx_sized(
            s,
            PrefStore::memory(),
            700,
            Arc::new(FakeShellRestart::new()),
        );
        let r = f.row();
        let cols = usize::try_from(r.cols).unwrap();
        assert!(cols > 1 && cols < APP_ICONS.len(), "{s}: {cols} a line");
        let lines = APP_ICONS.len().div_ceil(cols);
        let want = 60.0 + lines as f64 * 36.0 + (lines - 1) as f64 * 10.0 + 16.0;
        assert!(
            (f64::from(r.h) - want).abs() < 1.0,
            "{s}: {} vs {want}",
            r.h
        );
        let frame = f.h.render();
        // Daybreak where the wrapped grid puts it.
        let area = (32.0 * s) * (32.0 * s);
        let day = pale_at(&frame, s, tile_centre(&frame, s, cols, 3));
        assert!(day as f32 > area * 0.3, "{s}: Daybreak's ground, {day} px");
        // The last tile sits on the last line, under the first column it fills: a press there picks it.
        let (x, y) = tile_centre(&frame, s, cols, APP_ICONS.len() - 1);
        f.h.click(x, y);
        f.pump();
        assert_eq!(
            f.prefs.get(KEY).as_deref(),
            Some(APP_ICONS[APP_ICONS.len() - 1].id),
            "{s}: {cols} a line, {lines} lines"
        );
    }
}

#[test]
fn a_press_on_a_tile_stores_it_and_the_row_follows() {
    for s in SCALES2 {
        let f = fx_at(s, PrefStore::memory());
        let frame = f.h.render();
        let (x, y) = tile_centre(&frame, s, 12, 3);
        f.h.click(x, y);
        f.pump();
        assert_eq!(f.prefs.get(KEY).as_deref(), Some("daybreak"), "{s}");
        assert_eq!(f.row().value.as_str(), "daybreak", "{s}");
        // Back to the original from its own tile.
        let frame = f.h.render();
        let (x, y) = tile_centre(&frame, s, 12, 0);
        f.h.click(x, y);
        f.pump();
        assert_eq!(f.prefs.get(KEY).as_deref(), Some("original"), "{s}");
    }
}

#[test]
fn a_stored_pick_is_shown_and_an_unknown_one_shows_the_original() {
    let prefs = PrefStore::memory();
    prefs.set(KEY, "jade-glass");
    let f = fx_at(1.0, prefs);
    assert_eq!(f.row().value.as_str(), "jade-glass");
    let prefs = PrefStore::memory();
    prefs.set(KEY, "neon-sign");
    let f = fx_at(1.0, prefs);
    assert_eq!(f.row().value.as_str(), "original");
}

/// Rule 1263: the row's right is one labelled button and no icon name; the view draws no name there.
#[test]
fn the_right_side_is_the_restart_button_not_the_icon_name() {
    let prefs = PrefStore::memory();
    prefs.set(KEY, "safety-orange");
    let f = fx_at(1.0, prefs);
    let r = f.row();
    assert_eq!(r.badge.as_str(), "", "no icon name at the row's right");
    assert_eq!(r.buttons.row_count(), 1);
    let b = r.buttons.row_data(0).unwrap();
    assert_eq!(b.id.as_str(), RESTART_BUTTON);
    assert_eq!(b.label.as_str(), RESTART_LABEL);
    assert_eq!(b.tip.as_str(), "", "a labelled button has no tooltip");
    // The mockup's circular arrow left of the words (the icon's path data reaches the view).
    assert_eq!(
        b.icon.as_str(),
        flare_settings::icons::button_icon_path("redo").unwrap()
    );
    assert!(!b.off);
    // The view's row controls never print the row's badge (the icon's name lived there).
    let view = include_str!("../../ui/settings/view.slint");
    assert!(!view.contains("root.data.badge"));
}

/// Rule 1263: the button runs the restart once through the seam (here the fake), off while it runs, and a press
/// while it runs does nothing more.
#[test]
fn the_restart_button_runs_the_fake_restart() {
    let f = fx_at(1.0, PrefStore::memory());
    f.b()
        .invoke_row_button("app-icon".into(), RESTART_BUTTON.into());
    // Off at once, before the worker answers.
    f.h.advance(16);
    let off_now = f.row().buttons.row_data(0).unwrap().off;
    f.b()
        .invoke_row_button("app-icon".into(), RESTART_BUTTON.into());
    f.pump_until("the restart", |f| f.restart.calls() >= 1);
    f.pump_until("the button back on", |f| {
        !f.row().buttons.row_data(0).unwrap().off
    });
    assert!(off_now, "the button is off while it runs");

    assert_eq!(f.restart.calls(), 1, "one press, one restart");
    assert_eq!(
        f.row().sub.as_str(),
        flare_settings::page::appearance::APP_ICON_SUB
    );
    // A second press after it finished runs it again.
    f.b()
        .invoke_row_button("app-icon".into(), RESTART_BUTTON.into());
    f.pump_until("the second restart", |f| f.restart.calls() == 2);
}

/// Rule 1263: a failed restart is one plain line under the label until the next press.
#[test]
fn a_failed_restart_says_so_under_the_label() {
    let restart = Arc::new(FakeShellRestart::failing());
    let f = fx_sized(1.0, PrefStore::memory(), 1000, restart.clone());
    f.b()
        .invoke_row_button("app-icon".into(), RESTART_BUTTON.into());
    f.pump_until("the failure line", |f| {
        f.row().sub.as_str() == RESTART_FAILED
    });
    assert_eq!(restart.calls(), 1);
    assert!(!f.row().buttons.row_data(0).unwrap().off);
    assert!(!RESTART_FAILED.contains('\n'));
    // The next press clears it, and a restart that works leaves the usual words.
    restart.set_failing(false);
    f.b()
        .invoke_row_button("app-icon".into(), RESTART_BUTTON.into());
    f.pump_until("the usual words", |f| {
        f.row().sub.as_str() == flare_settings::page::appearance::APP_ICON_SUB
            && restart.calls() == 2
    });
}
