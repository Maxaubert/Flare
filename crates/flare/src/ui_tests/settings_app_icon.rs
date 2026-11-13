//! Settings > Appearance > App icon (rule 1230), headless at 100 and 225 percent: the row's words over one line of
//! twelve icon tiles, the original first, picked on a fresh profile and named as the default at the row's right; a
//! press on a tile stores its id in the settings and the row follows. The tiles are found by the original icon's own
//! red-orange flame, which no other design and no other part of the page wears.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use flare_settings::app_icon::{APP_ICONS, KEY};
use flare_settings::kv::PrefStore;
use flare_settings::look::LookStore;
use flare_settings::page::{GlassSupport, ThemeColours};
use flare_settings::verb::{FakeDiag, FakeVerb, VerbState};
use slint::{ComponentHandle, Model};

use crate::appctl::{Post, UiMsg};
use crate::settings_ctl::{SettingsCtl, SettingsDeps};
use crate::ui_harness::{Frame, Harness};
use crate::{AppWindow, SetKind, SetRow, SettingsBridge};

const SCALES2: [f32; 2] = [1.0, 2.25];

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<SettingsCtl>>,
    prefs: PrefStore,
    q: Queue,
}

fn fx_at(scale: f32, prefs: PrefStore) -> Fx {
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
    let h = Harness::new(scale, 1000, 640, || AppWindow::new().unwrap());
    h.comp.set_front_kind(1);
    h.advance(0);
    let deps = SettingsDeps {
        prefs: prefs.clone(),
        look,
        diag: Rc::new(FakeDiag::new("C:/logs", false)),
        verb: Arc::new(FakeVerb::new(VerbState::Off)),
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
    let f = Fx { h, ctl, prefs, q };
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

/// The centre of tile `i` in css px: tile 0 is found by the original's flame, the others follow 48 css px apart.
fn tile_centre(frame: &Frame, scale: f32, i: usize) -> (f32, f32) {
    let (x0, y0, x1, y1) = frame
        .bbox_where(0, 0, frame.w, frame.h, original_orange)
        .expect("the original's tile is drawn");
    let w = (x1 - x0) as f32 / scale;
    assert!(
        w > 8.0 && w < 40.0,
        "{scale}: one tile's flame, {w} css px wide"
    );
    let cx = (x0 + x1) as f32 / 2.0 / scale;
    let cy = (y0 + y1) as f32 / 2.0 / scale;
    (cx + 48.0 * i as f32, cy)
}

#[test]
fn the_picker_shows_every_icon_with_the_original_picked_and_named_the_default() {
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
        assert_eq!(r.badge.as_str(), "Original, the default");
        let ids: Vec<String> = r.options.iter().map(|o| o.id.to_string()).collect();
        let want: Vec<String> = APP_ICONS.iter().map(|i| i.id.to_string()).collect();
        assert_eq!(ids, want);
        assert_eq!(r.art.row_count(), APP_ICONS.len());
        for (i, art) in r.art.iter().enumerate() {
            let size = art.size();
            assert!(
                size.width > 0 && size.height > 0,
                "{s}: tile {i} has a picture"
            );
        }
        assert!((f64::from(r.h) - 114.0).abs() < 1.0, "{s}: {}", r.h);
        let frame = f.h.render();
        let (cx, cy) = tile_centre(&frame, s, 0);
        // Daybreak, the fourth tile, is drawn 3 tiles along with its own colours; its neighbours are dark.
        let pale = |i: usize| {
            let (x, _) = tile_centre(&frame, s, i);
            let half = 16.0 * s;
            frame.count_where(
                ((x * s) - half) as u32,
                ((cy * s) - half) as u32,
                ((x * s) + half) as u32,
                ((cy * s) + half) as u32,
                daybreak_ground,
            )
        };
        let area = (32.0 * s) * (32.0 * s);
        assert!(
            pale(3) as f32 > area * 0.3,
            "{s}: Daybreak's ground, {} px",
            pale(3)
        );
        assert!(
            (pale(2) as f32) < area * 0.05,
            "{s}: High Voltage is dark, {} px",
            pale(2)
        );
        // Twelve tiles fit the row: the last one's centre is inside the frame.
        let (lx, _) = tile_centre(&frame, s, APP_ICONS.len() - 1);
        assert!(lx + 20.0 < 1000.0, "{s}: {cx} .. {lx}");
    }
}

#[test]
fn a_press_on_a_tile_stores_it_and_the_row_follows() {
    for s in SCALES2 {
        let f = fx_at(s, PrefStore::memory());
        let frame = f.h.render();
        let (x, y) = tile_centre(&frame, s, 3);
        f.h.click(x, y);
        f.pump();
        assert_eq!(f.prefs.get(KEY).as_deref(), Some("daybreak"), "{s}");
        let r = f.row();
        assert_eq!(r.value.as_str(), "daybreak", "{s}");
        assert_eq!(r.badge.as_str(), "Daybreak");
        // Back to the original from its own tile.
        let frame = f.h.render();
        let (x, y) = tile_centre(&frame, s, 0);
        f.h.click(x, y);
        f.pump();
        assert_eq!(f.prefs.get(KEY).as_deref(), Some("original"), "{s}");
        assert_eq!(f.row().badge.as_str(), "Original, the default");
    }
}

#[test]
fn a_stored_pick_is_shown_and_an_unknown_one_shows_the_original() {
    let prefs = PrefStore::memory();
    prefs.set(KEY, "jade-glass");
    let f = fx_at(1.0, prefs);
    assert_eq!(f.row().value.as_str(), "jade-glass");
    assert_eq!(f.row().badge.as_str(), "Jade Glass");
    let prefs = PrefStore::memory();
    prefs.set(KEY, "neon-sign");
    let f = fx_at(1.0, prefs);
    assert_eq!(f.row().value.as_str(), "original");
}
