//! Settings > Plugins (the page key `mods`; Surfaces stage 8, plugins phase 2), headless, at 100 and 225 percent:
//! the rail lists the page, each mod is a card of one row (its logo, name, description, switch, Update, the info
//! button and Remove, 2026-11-08), the info button opens the plugin's details sheet whose tiles reach the mods port,
//! a folder that is not a mod is a warning with its reason, the buttons and the switch reach the mods port, and every
//! row sits in the panel on whole device pixels whatever the length of its words.

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
    /// What each switch carried (what the details sheet listed).
    switched_with: RefCell<Vec<flare_mods::hub::Shown>>,
    /// The panels asked for.
    panels: RefCell<Vec<String>>,
    /// The install, update and remove asks, as `what:argument`.
    installs: RefCell<Vec<String>>,
    /// The details sheet the port shows (None: the plugin is gone), and the choices made on it.
    sheet: RefCell<Option<crate::plugin_look::Sheet>>,
    choices: RefCell<Vec<(String, String, String)>>,
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
        self.panels.borrow_mut().push(id.to_string());
    }
    fn dir(&self) -> PathBuf {
        PathBuf::from(r"C:\Users\x\AppData\Roaming\Flare\plugins")
    }
    fn install_file(&self) {
        self.installs.borrow_mut().push("file:".into());
    }
    fn install_url(&self, url: &str) {
        self.installs.borrow_mut().push(format!("url:{url}"));
    }
    fn update(&self, id: &str) {
        self.installs.borrow_mut().push(format!("update:{id}"));
    }
    fn remove(&self, id: &str) {
        self.installs.borrow_mut().push(format!("remove:{id}"));
    }
    fn sheet(&self, _id: &str) -> Option<crate::plugin_look::Sheet> {
        self.sheet.borrow().clone()
    }
    fn set_choice(&self, id: &str, key: &str, value: &str) {
        self.choices
            .borrow_mut()
            .push((id.to_string(), key.to_string(), value.to_string()));
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
        dir: r"C:\Users\x\AppData\Roaming\Flare\plugins".into(),
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
        let card = f
            .sections()
            .into_iter()
            .find(|sec| sec.id == "modcard:notes")
            .expect("the mod's card");
        assert_eq!(card.title.as_str(), "", "its row names it");
        assert_eq!(card.rows.iter().count(), 1, "one row");
    }
}

#[test]
fn turning_a_plugin_on_from_its_card_opens_its_sheet_and_turn_on_there_approves_what_it_lists() {
    for s in SCALES2 {
        let f = fx_at(s, info(vec![notes(false)]));
        *f.port.sheet.borrow_mut() = Some(vault_sheet_on(&Default::default(), false));
        f.page("mods");
        // The card lists no permission: its switch opens the sheet, and nothing is approved yet.
        f.b().invoke_row_switch("mod:notes".into(), true);
        f.pump();
        assert!(f.port.switched.borrow().is_empty(), "{s}");
        assert!(f.b().get_sheet_open());
        assert!(
            f.b().get_sheet_can_turn_on(),
            "off: the sheet offers Turn on"
        );
        assert!(!f.row("mod:notes").unwrap().on, "the switch stays off");
        // Turn on carries what the sheet lists (a tile per permission, the maker, the homepage), so only that is
        // approved; Full control was on the sheet in amber.
        assert!(
            f.b()
                .get_sheet_warnings()
                .as_str()
                .starts_with("Full control:")
        );
        f.b().invoke_sheet_turn_on();
        let listed = vault_sheet_on(&Default::default(), false).shown;
        assert_eq!(listed.author, "someone");
        assert_eq!(listed.permissions.len(), 4);
        assert_eq!(
            f.port.switched.borrow().as_slice(),
            [("notes".to_string(), true)]
        );
        assert_eq!(f.port.switched_with.borrow().as_slice(), [listed]);
        // The port answers in the window; the page and the sheet then draw the new state.
        *f.port.info.borrow_mut() = info(vec![notes(true)]);
        *f.port.sheet.borrow_mut() = Some(vault_sheet_on(&Default::default(), true));
        f.ctl.borrow_mut().on_msg(SettingsMsg::Mods);
        assert!(f.row("mod:notes").unwrap().on);
        assert!(!f.b().get_sheet_can_turn_on(), "on: no Turn on");
        // Off from the card goes at once and approves nothing.
        f.b().invoke_sheet_close();
        f.b().invoke_row_switch("mod:notes".into(), false);
        assert_eq!(
            f.port.switched.borrow().last().cloned(),
            Some(("notes".to_string(), false))
        );
        assert_eq!(
            f.port.switched_with.borrow().last().cloned(),
            Some(flare_mods::hub::Shown::default())
        );
        assert!(!f.b().get_sheet_open());
    }
}

#[test]
fn a_mod_with_nothing_to_approve_goes_on_from_its_card() {
    let f = fx_at(1.0, info(vec![notes(false)]));
    let tiny = flare_mods::manifest::parse_plugin(
        r#"{"id":"notes","name":"Notes","version":"1.0.0","protocol":2,"description":"d","permissions":[]}"#,
    )
    .unwrap();
    *f.port.sheet.borrow_mut() = Some(crate::plugin_look::sheet_of(
        &tiny,
        &Default::default(),
        false,
    ));
    f.page("mods");
    f.b().invoke_row_switch("mod:notes".into(), true);
    assert!(!f.b().get_sheet_open(), "nothing to show first");
    assert_eq!(
        f.port.switched.borrow().as_slice(),
        [("notes".to_string(), true)]
    );
    assert_eq!(
        f.port.switched_with.borrow().as_slice(),
        [flare_mods::hub::Shown::default()]
    );
}

#[test]
fn the_sheet_opens_the_panel_of_a_plugin_that_is_on_and_holds_it() {
    for s in SCALES2 {
        let f = fx_at(s, info(vec![notes(true)]));
        *f.port.sheet.borrow_mut() = Some(vault_sheet_on(&Default::default(), true));
        f.page("mods");
        f.b().invoke_row_button("mod:notes".into(), "info".into());
        assert_eq!(f.b().get_sheet_panel().as_str(), "Notes", "{s}");
        f.h.advance(0);
        assert!(
            f.h.comp.get_sheet_panel_w() > 0.0,
            "the Open button is drawn"
        );
        assert!(f.h.comp.get_sheet_turn_on_w() == 0.0, "on: no Turn on");
        f.b().invoke_sheet_open_panel();
        assert_eq!(f.port.panels.borrow().as_slice(), ["notes".to_string()]);
        assert!(!f.b().get_sheet_open(), "the panel opens in its tab");
        // The panel held back on the sheet: no Open button.
        let mut c = flare_mods::grants::Choices::default();
        flare_mods::grants::choose(&mut c, flare_ctl::Permission::Panel, "off").unwrap();
        *f.port.sheet.borrow_mut() = Some(vault_sheet_on(&c, true));
        f.b().invoke_row_button("mod:notes".into(), "info".into());
        f.h.advance(0);
        assert_eq!(f.b().get_sheet_panel().as_str(), "");
        assert!(f.h.comp.get_sheet_panel_w() == 0.0);
    }
}

#[test]
fn a_long_sheet_never_leaves_the_window_and_scrolls_to_its_last_tile() {
    // Thirteen permissions, every tile tall: taller than the window at 225 percent.
    let m = flare_mods::manifest::parse_plugin(
        r##"{"id":"notes","name":"Notes","version":"1.0.0","protocol":2,"description":"A plugin that asks for everything.","author":"someone",
        "homepage":"https://example.com/notes",
        "permissions":["tools","actions","panel","process","fill","tabs","decorate","sessions","network","input","view","convo","agent-looks"],
        "panel":{"entry":"p.html","title":"Notes"},"helper":{"command":"bin/notes.exe"},
        "contributes":{"actions":[{"key":"a","title":"Lock","where":["tab-menu"]}]},
        "tools":[{"name":"append_note","description":"d","input_schema":{"type":"object"}}]}"##,
    )
    .unwrap();
    for s in SCALES2 {
        let f = fx_at(s, info(vec![notes(false)]));
        *f.port.sheet.borrow_mut() =
            Some(crate::plugin_look::sheet_of(&m, &Default::default(), false));
        f.page("mods");
        f.b().invoke_row_button("mod:notes".into(), "info".into());
        f.h.advance(0);
        let a = &f.h.comp;
        let win_h = 640.0;
        let (by, bh) = (a.get_sheet_box_y(), a.get_sheet_box_h());
        assert!(by >= 24.0 - 1e-3, "{by} at {s}");
        assert!(
            by + bh <= win_h - 24.0 + 1.0,
            "the box ends at {} at {s}",
            by + bh
        );
        assert!(a.get_sheet_scroll_max() > 0.0, "the body scrolls at {s}");
        // The Turn on button stays in the window, under the scrolled part.
        let (ty, th) = (a.get_sheet_turn_on_y(), a.get_sheet_turn_on_h());
        assert!(
            a.get_sheet_turn_on_w() > 0.0 && ty + th <= by + bh,
            "at {s}"
        );
        // The keys scroll it to the end: the last tile's row is then inside the box.
        let page_down: slint::SharedString = slint::platform::Key::PageDown.into();
        for _ in 0..40 {
            f.h.key_text(&page_down);
        }
        f.h.advance(0);
        assert!(
            (a.get_sheet_scroll_y() - a.get_sheet_scroll_max()).abs() < 1.0,
            "{} of {} at {s}",
            a.get_sheet_scroll_y(),
            a.get_sheet_scroll_max()
        );
        let rows = (15.0f32 / 2.0).ceil();
        let last_end = a.get_sheet_grid_y() + 1.0 + rows * (a.get_sheet_cell_h() + 1.0);
        assert!(
            last_end <= ty,
            "the last tile ends at {last_end}, above the foot at {ty}, at {s}"
        );
    }
}

/// A plugin's sheet as the app builds it from a manifest: levels, toggles and a fact.
fn vault_sheet(c: &flare_mods::grants::Choices) -> crate::plugin_look::Sheet {
    vault_sheet_on(c, true)
}

fn vault_sheet_on(c: &flare_mods::grants::Choices, on: bool) -> crate::plugin_look::Sheet {
    let m = flare_mods::manifest::parse_plugin(
        r##"{"id":"notes","name":"Notes","version":"1.0.0","protocol":2,"description":"d","author":"someone",
        "permissions":["tools","panel","process","fill"],"panel":{"entry":"p.html","title":"Notes"},
        "helper":{"command":"bin/notes.exe"},"brand":{"color":"#175ddc","icon":"M3 3h18v18H3z"},
        "tools":[{"name":"append_note","description":"d","input_schema":{"type":"object"}}]}"##,
    )
    .unwrap();
    crate::plugin_look::sheet_of(&m, c, on)
}

#[test]
fn the_info_button_opens_the_details_sheet_whose_tiles_reach_the_port_and_escape_closes_it() {
    for s in SCALES2 {
        let f = fx_at(s, info(vec![notes(true)]));
        *f.port.sheet.borrow_mut() = Some(vault_sheet(&Default::default()));
        f.page("mods");
        assert!(!f.b().get_sheet_open());
        f.b().invoke_row_button("mod:notes".into(), "info".into());
        assert!(f.b().get_sheet_open(), "{s}");
        assert_eq!(f.ctl.borrow().sheet_of(), Some("notes"));
        assert_eq!(f.b().get_sheet_name().as_str(), "Notes");
        assert_eq!(f.b().get_sheet_sub().as_str(), "1.0.0 \u{b7} someone");
        assert!(f.b().get_sheet_look().branded);
        assert!(f.b().get_sheet_tall(), "the tools tile names its tool");
        let tiles: Vec<crate::SheetTile> = f.b().get_sheet_tiles().iter().collect();
        let keys: Vec<&str> = tiles.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys, ["process", "fill", "tools", "panel", "fact:program"]);
        assert_eq!(tiles[0].value_name.as_str(), "Full control");
        assert_eq!(tiles[0].tone, 2, "full control is amber");
        assert_eq!(tiles[1].value_name.as_str(), "Ask each time");
        assert_eq!(tiles[1].tone, 1, "the usual level is the brand's");
        assert!(
            f.b()
                .get_sheet_warnings()
                .as_str()
                .starts_with("Full control:")
        );
        // A toggle and a level reach the port; the sheet is drawn again from what the port says.
        f.b().invoke_sheet_toggle("tools".into(), false);
        f.b().invoke_sheet_choose("fill".into(), "always".into());
        assert_eq!(
            f.port.choices.borrow().as_slice(),
            [
                ("notes".into(), "tools".into(), "off".into()),
                ("notes".into(), "fill".into(), "always".into())
            ]
        );
        let mut c = flare_mods::grants::Choices::default();
        flare_mods::grants::choose(&mut c, flare_ctl::Permission::Tools, "off").unwrap();
        flare_mods::grants::choose(&mut c, flare_ctl::Permission::Fill, "always").unwrap();
        *f.port.sheet.borrow_mut() = Some(vault_sheet(&c));
        f.ctl.borrow_mut().on_msg(SettingsMsg::Mods);
        let tiles: Vec<crate::SheetTile> = f.b().get_sheet_tiles().iter().collect();
        assert!(!tiles[2].on);
        assert_eq!(
            (tiles[1].value_name.as_str(), tiles[1].tone),
            ("Always allow", 2)
        );
        // Escape closes it; a plugin that is gone closes it too.
        assert!(f.ctl.borrow_mut().escape());
        assert!(!f.b().get_sheet_open());
        f.b().invoke_row_button("mod:notes".into(), "info".into());
        *f.port.sheet.borrow_mut() = None;
        f.ctl.borrow_mut().on_msg(SettingsMsg::Mods);
        assert!(!f.b().get_sheet_open());
        assert_eq!(f.ctl.borrow().sheet_of(), None);
        // The X closes it.
        *f.port.sheet.borrow_mut() = Some(vault_sheet(&Default::default()));
        f.b().invoke_row_button("mod:notes".into(), "info".into());
        f.b().invoke_sheet_close();
        assert!(!f.b().get_sheet_open());
        assert!(
            f.row("mods-folder").is_none(),
            "one folder row, the plugins one"
        );
    }
}
#[test]
fn the_install_buttons_the_address_field_and_each_cards_update_and_remove_reach_the_port() {
    let f = fx_at(1.0, info(vec![notes(true)]));
    f.page("mods");
    let field = f.row("plugins-url").expect("the address row");
    assert_eq!(field.kind, crate::SetKind::Text);
    assert_eq!(field.placeholder.as_str(), "https://");
    f.b()
        .invoke_row_button("plugins-install".into(), "file".into());
    // What is typed is kept with no rebuild, and Install (or Enter) sends it.
    let before = f.ctl.borrow().rebuilds;
    f.b()
        .invoke_row_text("plugins-url".into(), " https://example.com/p.zip ".into());
    assert_eq!(
        f.ctl.borrow().rebuilds,
        before,
        "typing draws nothing again"
    );
    f.b().invoke_row_button("plugins-url".into(), "url".into());
    f.b()
        .invoke_row_button("plugins-url".into(), "submit".into());
    f.b().invoke_row_button("mod:notes".into(), "update".into());
    f.b().invoke_row_button("mod:notes".into(), "remove".into());
    assert_eq!(
        f.port.installs.borrow().as_slice(),
        [
            "file:",
            "url:https://example.com/p.zip",
            "url:https://example.com/p.zip",
            "update:notes",
            "remove:notes"
        ]
    );
    // The page drawn again keeps the address in the field.
    f.ctl.borrow_mut().on_msg(SettingsMsg::Mods);
    assert_eq!(
        f.row("plugins-url").unwrap().value.as_str(),
        " https://example.com/p.zip "
    );
    // What the install said is the install row's subtext, a warning when it failed.
    let mut page = info(vec![notes(true)]);
    page.install_note = Some((true, "The file is larger than 50 MB.".into()));
    *f.port.info.borrow_mut() = page;
    f.ctl.borrow_mut().on_msg(SettingsMsg::Mods);
    let row = f.row("plugins-install").unwrap();
    assert!(row.warn && row.sub.as_str() == "The file is larger than 50 MB.");
}

#[test]
fn a_folder_that_is_not_a_mod_is_a_warning_with_its_reason_and_the_others_still_list() {
    let mut page = info(vec![notes(true)]);
    page.problems = vec![
        Problem {
            folder: "broken-mod".into(),
            reason: "mod.json is not valid JSON: expected value at line 1 column 1".into(),
        },
        Problem {
            folder: "wrong".into(),
            reason: "the folder is named \"wrong\" but mod.json says id \"other\"; the folder name must be the id".into(),
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
fn every_card_is_one_row_with_update_info_and_remove_and_a_branded_one_draws_its_logo() {
    let mut branded = todo_plugin();
    branded.brand_color = "#175ddc".into();
    branded.logo = "M3 3h18v18H3z".into();
    for s in SCALES2 {
        let f = fx_at(s, info(vec![notes(false), branded.clone()]));
        f.page("mods");
        let card = |id: &str| -> Vec<String> {
            f.sections()
                .into_iter()
                .find(|sec| sec.id == id)
                .map(|sec| sec.rows.iter().map(|r| r.id.to_string()).collect())
                .unwrap_or_default()
        };
        assert_eq!(card("modcard:notes"), ["mod:notes"]);
        assert_eq!(card("modcard:todo"), ["mod:todo"]);
        let row = f.row("mod:todo").unwrap();
        assert_eq!(row.label.as_str(), "Session to-dos 1.0.0");
        assert!(!row.on, "off until the user turns it on");
        let b: Vec<(String, String, bool, bool)> = row
            .buttons
            .iter()
            .map(|b| {
                (
                    b.id.to_string(),
                    b.tip.to_string(),
                    b.icon.is_empty(),
                    b.warn,
                )
            })
            .collect();
        assert_eq!(
            b,
            [
                ("update".into(), String::new(), true, false),
                (
                    "info".into(),
                    "Details and permissions".into(),
                    false,
                    false
                ),
                (
                    "remove".into(),
                    "Remove (to the Recycle Bin)".into(),
                    false,
                    false
                ),
            ]
        );
        assert_eq!(row.logo.as_str(), "M3 3h18v18H3z");
        assert_eq!(row.logo_fill, slint::Color::from_rgb_u8(0x17, 0x5d, 0xdc));
        assert!(
            f.row("mod:notes").unwrap().logo.is_empty(),
            "no brand: the chip"
        );
    }
}
#[test]
fn the_plugins_folder_button_opens_the_plugins_folder() {
    let f = fx_at(1.0, info(vec![]));
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

#[test]
fn the_pointer_opens_a_dropdown_picks_a_level_and_flips_a_toggle_on_the_sheet() {
    for s in SCALES2 {
        let f = fx_at(s, info(vec![notes(true)]));
        *f.port.sheet.borrow_mut() = Some(vault_sheet(&Default::default()));
        f.page("mods");
        f.b().invoke_row_button("mod:notes".into(), "info".into());
        f.h.advance(0);
        let a = &f.h.comp;
        let (bx, gy, cw, ch) = (
            a.get_sheet_box_x(),
            a.get_sheet_grid_y(),
            a.get_sheet_cell_w(),
            a.get_sheet_cell_h(),
        );
        // Tile 1 (Fills logins): its dropdown opens, and its second level is picked.
        f.h.click(bx + cw + 1.0 + cw / 2.0, gy + 1.0 + ch / 2.0);
        f.h.advance(0);
        assert_eq!(a.get_sheet_menu_tile(), 1, "{s}");
        // The dropdown hangs from its tile's foot, under the detail line (the tiles are tall here).
        let drop = a.get_sheet_menu_drop();
        assert!(
            (drop - (ch - 4.0)).abs() < 1e-3,
            "{drop} under a tile {ch} high"
        );
        f.h.click(bx + cw + 1.0 + 100.0, gy + 1.0 + drop + 4.0 + 30.0 + 15.0);
        f.h.advance(0);
        assert_eq!(a.get_sheet_menu_tile(), -1);
        // Tile 2 (Agent tool): its switch.
        f.h.click(bx + cw - 32.0, gy + 1.0 + ch + 1.0 + ch / 2.0);
        f.h.advance(0);
        assert_eq!(
            f.port.choices.borrow().as_slice(),
            [
                ("notes".into(), "fill".into(), "always".into()),
                ("notes".into(), "tools".into(), "off".into())
            ],
            "{s}"
        );
    }
}
