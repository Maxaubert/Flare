//! Settings > Plugins (the page key `mods`; Surfaces stage 8, plugins phase 2), headless, at 100 and 225 percent:
//! the rail lists the page, each mod is a switch row and a permissions row in plain words (a plugin adds who made
//! it and its warnings), a folder that is not a mod is a warning with its reason, the
//! buttons and the switch reach the mods port, and every row sits in the panel on whole device pixels whatever
//! the length of its words.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use flare_settings::kv::PrefStore;
use flare_settings::look::LookStore;
use flare_settings::page::mods::{ModInfo, ModsInfo, Problem};
use flare_settings::page::{GlassSupport, ThemeColours};
use flare_settings::verb::{FakeDiag, FakeVerb, VerbState};
use slint::{ComponentHandle, Model};

use crate::appctl::{Post, UiMsg};
use crate::settings_ctl::{ModsPort, SettingsCtl, SettingsDeps, SettingsMsg};
use crate::ui_harness::Harness;
use crate::{AppWindow, SetRow, SetSection, SettingsBridge};

const SCALES2: [f32; 2] = [1.0, 2.25];

/// A mods port with a page of its own to show, recording what the person did.
#[derive(Default)]
struct FakePort {
    info: RefCell<ModsInfo>,
    asked: Cell<u32>,
    switched: RefCell<Vec<(String, bool)>>,
    /// What each switch carried (what the card listed).
    switched_with: RefCell<Vec<flare_mods::hub::Shown>>,
    shown: RefCell<Vec<String>>,
}

impl ModsPort for FakePort {
    fn info(&self) -> ModsInfo {
        self.info.borrow().clone()
    }
    fn page_shown(&self) {
        self.asked.set(self.asked.get() + 1);
    }
    fn set_enabled(&self, id: &str, on: bool, shown: flare_mods::hub::Shown) {
        self.switched_with.borrow_mut().push(shown);
        self.switched.borrow_mut().push((id.to_string(), on));
    }
    fn show_panel(&self, id: &str) {
        self.shown.borrow_mut().push(id.to_string());
    }
    fn dir(&self) -> PathBuf {
        PathBuf::from(r"C:\Users\x\AppData\Roaming\Flare\mods")
    }
    fn plugins_dir(&self) -> Option<PathBuf> {
        Some(PathBuf::from(r"C:\Users\x\AppData\Roaming\Flare\plugins"))
    }
}

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<SettingsCtl>>,
    port: Rc<FakePort>,
    opened: Rc<RefCell<Vec<PathBuf>>>,
    q: Queue,
}

fn notes(enabled: bool) -> ModInfo {
    ModInfo {
        id: "notes".into(),
        name: "Notes".into(),
        version: "1.0.0".into(),
        description: "A notes panel the agent can add a line to, with the append_note tool.".into(),
        permissions: vec![
            "Show its panel in a tab".into(),
            "Answer the agent's tool calls".into(),
        ],
        enabled,
        needs_approval: false,
        has_panel: true,
        ..ModInfo::default()
    }
}

fn info(mods: Vec<ModInfo>) -> ModsInfo {
    ModsInfo {
        scanned: true,
        dir: r"C:\Users\x\AppData\Roaming\Flare\mods".into(),
        mods,
        ..ModsInfo::default()
    }
}

fn fx_at(scale: f32, page: ModsInfo) -> Fx {
    let prefs = PrefStore::memory();
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
    let opened: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    let deps = SettingsDeps {
        prefs: prefs.clone(),
        look: look.clone(),
        diag: Rc::new(FakeDiag::new("C:/logs", false)),
        verb: Arc::new(FakeVerb::new(VerbState::Off)),
        dictation: Rc::new(crate::dictation_page::DictationPageProvider),
        pick_folder: Rc::new(|_, done| done(None)),
        open_path: {
            let opened = Rc::clone(&opened);
            Rc::new(move |p: &Path| opened.borrow_mut().push(p.to_path_buf()))
        },
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
    let port = Rc::new(FakePort::default());
    *port.info.borrow_mut() = page;
    ctl.borrow_mut().set_mods(port.clone());
    ctl.borrow_mut().show();
    let f = Fx {
        h,
        ctl,
        port,
        opened,
        q,
    };
    f.pump();
    f
}

impl Fx {
    fn b(&self) -> SettingsBridge<'_> {
        self.h.comp.global::<SettingsBridge>()
    }

    fn pump(&self) {
        // The shell list and the Explorer verb answer from workers; whatever has arrived is taken in.
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
    }

    fn page(&self, id: &str) {
        self.b().invoke_page_chosen(id.into());
        self.pump();
    }

    fn sections(&self) -> Vec<SetSection> {
        self.b().get_sections().iter().collect()
    }

    fn row(&self, id: &str) -> Option<SetRow> {
        self.sections()
            .iter()
            .flat_map(|s| s.rows.iter())
            .find(|r| r.id == id)
    }
}

#[test]
fn the_rail_lists_mods_between_dictation_and_diagnostics() {
    for s in SCALES2 {
        let f = fx_at(s, info(vec![]));
        let ids: Vec<String> = f.b().get_rail().iter().map(|r| r.id.to_string()).collect();
        assert_eq!(
            ids,
            [
                "appearance",
                "terminal",
                "agents",
                "dictation",
                "mods",
                "diagnostics",
                "about"
            ],
            "{s}"
        );
        let item = f.b().get_rail().iter().find(|r| r.id == "mods").unwrap();
        assert_eq!(item.label.as_str(), "Plugins");
        assert!(!item.icon.is_empty());
    }
}

#[test]
fn opening_the_page_asks_for_a_scan_each_time_it_comes_up() {
    let f = fx_at(1.0, info(vec![]));
    assert_eq!(f.port.asked.get(), 0, "another page is showing");
    f.page("mods");
    assert_eq!(f.port.asked.get(), 1);
    f.page("terminal");
    f.page("mods");
    assert_eq!(f.port.asked.get(), 2);
    // The scan's answer draws the page again.
    f.ctl.borrow_mut().on_msg(SettingsMsg::Mods);
}

#[test]
fn each_mod_shows_its_name_version_description_permissions_and_a_switch_that_is_off_until_turned_on()
 {
    for s in SCALES2 {
        let f = fx_at(s, info(vec![notes(false)]));
        f.page("mods");
        let sw = f.row("mod:notes").expect("the mod's switch row");
        assert_eq!(sw.label.as_str(), "Notes 1.0.0");
        assert!(sw.sub.as_str().contains("append_note"));
        assert!(sw.tap, "a press anywhere on the row flips it");
        assert!(!sw.on, "off until the user turns it on");
        let perms = f.row("modperm:notes").expect("the permissions row");
        assert_eq!(
            perms.sub.as_str(),
            "Show its panel in a tab, Answer the agent's tool calls",
            "the words are shown while the mod is still off"
        );
        let heading = f
            .sections()
            .into_iter()
            .find(|sec| sec.id == "modcard:notes")
            .expect("the mod's card");
        assert_eq!(heading.title.as_str(), "Notes");
    }
}

#[test]
fn the_switch_reaches_the_mods_port_and_the_page_draws_what_the_port_then_says() {
    for s in SCALES2 {
        let f = fx_at(s, info(vec![notes(false)]));
        f.page("mods");
        f.b().invoke_row_switch("mod:notes".into(), true);
        f.pump();
        assert_eq!(
            f.port.switched.borrow().as_slice(),
            [("notes".to_string(), true)]
        );
        // The click carries what the card listed (permission words, maker, address), so only that is approved.
        let m = f.port.info.borrow().mods[0].clone();
        let listed = flare_mods::hub::Shown {
            permissions: m.permissions,
            author: m.author,
            homepage: m.homepage,
        };
        assert_eq!(f.port.switched_with.borrow().as_slice(), [listed]);
        // The port answers in the window; the page then draws the new state.
        *f.port.info.borrow_mut() = info(vec![notes(true)]);
        f.ctl.borrow_mut().on_msg(SettingsMsg::Mods);
        assert!(f.row("mod:notes").unwrap().on);
        let show = f.row("modperm:notes").unwrap();
        assert!(
            !show.buttons.iter().next().unwrap().off,
            "Show panel is offered once it is on"
        );
    }
}

#[test]
fn the_panel_button_waits_for_the_mod_and_the_folder_button_opens_the_mods_folder() {
    let f = fx_at(1.0, info(vec![notes(false)]));
    f.page("mods");
    let perms = f.row("modperm:notes").unwrap();
    let btn = perms.buttons.iter().next().unwrap();
    assert_eq!(btn.id.as_str(), "show");
    assert!(btn.off, "no panel while the mod is off");
    *f.port.info.borrow_mut() = info(vec![notes(true)]);
    f.ctl.borrow_mut().on_msg(SettingsMsg::Mods);
    f.b()
        .invoke_row_button("modperm:notes".into(), "show".into());
    assert_eq!(f.port.shown.borrow().as_slice(), ["notes".to_string()]);
    let folder = f.row("mods-folder").unwrap();
    assert_eq!(
        folder.buttons.iter().next().unwrap().label.as_str(),
        "Open mods folder"
    );
    f.b().invoke_row_button("mods-folder".into(), "open".into());
    assert_eq!(
        f.opened.borrow().as_slice(),
        [PathBuf::from(r"C:\Users\x\AppData\Roaming\Flare\mods")]
    );
}

#[test]
fn a_folder_that_is_not_a_mod_is_a_warning_with_its_reason_and_the_others_still_list() {
    let mut page = info(vec![notes(true)]);
    page.problems = vec![
        Problem {
            folder: "broken-mod".into(),
            reason: "mod.json is not valid JSON: expected value at line 1 column 1".into(),
            plugins: false,
        },
        Problem {
            folder: "wrong".into(),
            reason: "the folder is named \"wrong\" but mod.json says id \"other\"; the folder name must be the id".into(),
            plugins: false,
        },
    ];
    page.notes = vec![
        "The tool name mod_a_b is claimed by a-b (c) and a (b_c); none of them is offered.".into(),
    ];
    for s in SCALES2 {
        let f = fx_at(s, page.clone());
        f.page("mods");
        assert!(f.row("mod:notes").is_some(), "the good mod is listed");
        let bad = f.row("modbad:0").expect("a warning row");
        assert_eq!(bad.label.as_str(), "broken-mod");
        assert!(bad.warn && bad.sub.as_str().contains("not valid JSON"));
        assert!(
            f.row("modbad:1")
                .unwrap()
                .sub
                .as_str()
                .contains("folder name must be the id")
        );
        assert!(f.row("modnote:0").unwrap().warn);
        let sec = f
            .sections()
            .into_iter()
            .find(|sec| sec.id == "mods-problems")
            .expect("the problems card");
        assert_eq!(sec.title.as_str(), "Not loaded");
    }
}

#[test]
fn before_the_first_scan_the_page_says_it_is_looking_and_with_no_mods_it_says_so() {
    let f = fx_at(1.0, ModsInfo::default());
    f.page("mods");
    assert!(f.row("mods-looking").is_some());
    *f.port.info.borrow_mut() = info(vec![]);
    f.ctl.borrow_mut().on_msg(SettingsMsg::Mods);
    assert!(f.row("mods-looking").is_none());
    assert!(f.row("mods-none").is_some());
}

#[test]
fn every_row_sits_whole_in_its_card_on_device_pixels_at_every_scale_and_long_words_do_not_grow_a_row()
 {
    let mut long = notes(true);
    long.name = "A mod with a very long name indeed that goes on and on".into();
    long.description = "Words ".repeat(200);
    long.permissions = (0..5)
        .map(|i| format!("A permission with several words {i}"))
        .collect();
    let mut page = info(vec![notes(false), long]);
    page.problems = vec![Problem {
        folder: "x".repeat(40),
        reason: "y ".repeat(300),
        plugins: false,
    }];
    for s in SCALES2 {
        let f = fx_at(s, page.clone());
        f.page("mods");
        let scale = f64::from(s);
        let rows_h: Vec<f64> = f
            .sections()
            .iter()
            .flat_map(|sec| sec.rows.iter().map(|r| f64::from(r.h)))
            .collect();
        assert!(rows_h.len() >= 6, "{rows_h:?}");
        for h in &rows_h {
            // A row is 58 css px whatever its words say: the line elides, it does not wrap.
            assert!((h - 58.0).abs() < 1.0 / scale + 1e-3, "{h} at {s}");
        }
        for sec in f.sections() {
            let mut y = 0.0f64;
            for r in sec.rows.iter() {
                let (ry, rh) = (f64::from(r.y), f64::from(r.h));
                assert!(
                    (ry - y).abs() < 1e-3,
                    "rows touch: {ry} after {y} in {} at {s}",
                    sec.id
                );
                // Whole device pixels.
                let dev = ry * scale;
                assert!((dev - dev.round()).abs() < 1e-3, "{} y {ry} at {s}", r.id);
                y = ry + rh;
            }
            assert!(
                y <= f64::from(sec.panel_h) + 1e-3,
                "{} rows end at {y} inside a panel {} high at {s}",
                sec.id,
                sec.panel_h
            );
        }
        // The page is as long as its cards: the content height covers the last card.
        let last = f.sections().into_iter().last().unwrap();
        assert!(
            f64::from(f.b().get_content_h()) >= f64::from(last.panel_y) + f64::from(last.panel_h),
            "content height covers the last card at {s}"
        );
    }
}

/// A protocol 2 plugin as the app fills it: who made it, its permissions in plain words and the warnings.
fn todo_plugin() -> ModInfo {
    ModInfo {
        id: "todo".into(),
        name: "Session to-dos".into(),
        version: "1.0.0".into(),
        description: "Things to check per session.".into(),
        permissions: vec![
            "Read what you and Claude say in sessions".into(),
            "Type and run commands in your terminals (full control)".into(),
        ],
        enabled: false,
        has_panel: false,
        plugin: true,
        author: "someone".into(),
        homepage: "https://example.com/todo".into(),
        warnings: vec![
            flare_mods::hub::WARN_INPUT.into(),
            flare_mods::hub::WARN_INPUT_SESSIONS.into(),
        ],
        full_control: true,
        ..ModInfo::default()
    }
}

#[test]
fn a_plugin_card_draws_its_maker_its_permissions_and_a_full_control_warning_and_a_mod_card_is_unchanged()
 {
    for s in SCALES2 {
        let f = fx_at(s, info(vec![notes(false), todo_plugin()]));
        f.page("mods");
        let card = |id: &str| -> Vec<String> {
            f.sections()
                .into_iter()
                .find(|sec| sec.id == id)
                .map(|sec| sec.rows.iter().map(|r| r.id.to_string()).collect())
                .unwrap_or_default()
        };
        assert_eq!(card("modcard:notes"), ["mod:notes", "modperm:notes"]);
        assert_eq!(
            card("modcard:todo"),
            ["mod:todo", "modby:todo", "modperm:todo", "modwarn:todo"]
        );
        assert_eq!(
            f.row("mod:todo").unwrap().label.as_str(),
            "Session to-dos 1.0.0"
        );
        assert!(
            !f.row("mod:todo").unwrap().on,
            "off until the user turns it on"
        );
        let by = f.row("modby:todo").unwrap();
        assert_eq!(by.sub.as_str(), "someone, https://example.com/todo");
        let perms = f.row("modperm:todo").unwrap();
        assert!(perms.sub.as_str().contains("(full control)"));
        let warn = f.row("modwarn:todo").unwrap();
        assert!(warn.warn);
        assert_eq!(warn.label.as_str(), "Full control");
        assert!(warn.sub.as_str().contains("steer Claude"));
    }
}

#[test]
fn the_plugins_folder_button_opens_the_plugins_folder() {
    let mut page = info(vec![]);
    page.plugins_dir = r"C:\Users\x\AppData\Roaming\Flare\plugins".into();
    let f = fx_at(1.0, page);
    f.page("mods");
    let row = f.row("plugins-folder").expect("the plugins folder row");
    assert_eq!(
        row.buttons.iter().next().unwrap().label.as_str(),
        "Open plugins folder"
    );
    f.b()
        .invoke_row_button("plugins-folder".into(), "open".into());
    assert_eq!(
        f.opened.borrow().as_slice(),
        [PathBuf::from(r"C:\Users\x\AppData\Roaming\Flare\plugins")]
    );
}
