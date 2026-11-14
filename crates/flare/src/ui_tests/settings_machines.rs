//! Settings > Machines (remote sessions PR 7), headless, at 100 and 225 percent: the rail lists the page after
//! Plugins; with no machine it is the "+ Add a machine" row alone, with no subtext; linked machines are a table under a
//! heading row (logo, name, Address, Flare version, status, a menu with Test and Remove); the add row opens the Add a
//! machine dialog (machine pairing PR 1, rule 1251) and a failed check is one red line in it; a machine that is Not
//! linked shows why as one grey line under its row, in plain words when ssh said why (rule 1250), with no tooltip. Every link check runs against an in-memory
//! stand-in: no ssh.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use flare_settings::kv::PrefStore;
use flare_settings::look::LookStore;
use flare_settings::page::{GlassSupport, ThemeColours};
use flare_settings::verb::{FakeDiag, FakeVerb, VerbState};
use flare_tabs::tabs::Os;
use slint::{ComponentHandle, Model};

use crate::appctl::{Post, UiMsg};
use crate::machines::{FLARE_CANDIDATES, Machine};
use crate::machines_page::fake::FakeIo;
use crate::machines_page::{HelloFacts, MachinesIo, MachinesPage};
use crate::settings_ctl::{SettingsCtl, SettingsDeps};
use crate::ui_harness::Harness;
use crate::{AppWindow, BlockKind, SetRow, SetSection, SettingsBridge};

const SCALES2: [f32; 2] = [1.0, 2.25];

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<SettingsCtl>>,
    io: Arc<FakeIo>,
    q: Queue,
    /// The machine lists the window was sent.
    sent: RefCell<Vec<Vec<Machine>>>,
}

fn mac() -> Machine {
    Machine {
        id: "mac".into(),
        name: "Mac Mini".into(),
        kind: Some(Os::Mac),
        ssh: "mini".into(),
        user: None,
        flare: "flare".into(),
        ..Default::default()
    }
}

fn fx_at(scale: f32, io: FakeIo) -> Fx {
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
    let deps = SettingsDeps {
        prefs: prefs.clone(),
        look: look.clone(),
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
        post: Arc::clone(&post),
        version: "1.2.3".to_string(),
        copyright: "Copyright 2026".to_string(),
        e2e: true,
        clock: Rc::new(|| 1_000),
        still: Rc::new(|| false),
    };
    let ctl = SettingsCtl::new(&h.comp, deps);
    let io = Arc::new(io);
    ctl.borrow_mut()
        .set_machines(MachinesPage::new(io.clone(), post));
    ctl.borrow_mut().show();
    Fx {
        h,
        ctl,
        io,
        q,
        sent: RefCell::default(),
    }
}

impl Fx {
    fn b(&self) -> SettingsBridge<'_> {
        self.h.comp.global::<SettingsBridge>()
    }

    /// Takes in what the workers answered until `cond` holds (10 s at most).
    fn settle(&self, what: &str, cond: impl Fn(&Fx) -> bool) {
        let t = Instant::now();
        loop {
            let msgs: Vec<UiMsg> = self
                .q
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain(..)
                .collect();
            for m in msgs {
                match m {
                    UiMsg::Settings(m) => self.ctl.borrow_mut().on_msg(m),
                    UiMsg::Machines(l) => self.sent.borrow_mut().push(l),
                    _ => {}
                }
            }
            if cond(self) {
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(3)); // sync-ok: a test waits for its workers
        }
    }

    fn page(&self) {
        self.b().invoke_page_chosen("machines".into());
    }

    fn sections(&self) -> Vec<SetSection> {
        self.b().get_sections().iter().collect()
    }

    fn rows(&self) -> Vec<SetRow> {
        self.sections()
            .iter()
            .flat_map(|s| s.rows.iter().collect::<Vec<_>>())
            .collect()
    }

    fn row(&self, id: &str) -> Option<SetRow> {
        self.rows().into_iter().find(|r| r.id == id)
    }

    fn kinds(&self) -> Vec<BlockKind> {
        self.rows().iter().map(|r| r.block).collect()
    }
}

#[test]
fn the_rail_lists_machines_after_plugins() {
    let f = fx_at(1.0, FakeIo::default());
    let ids: Vec<String> = f.b().get_rail().iter().map(|r| r.id.to_string()).collect();
    assert_eq!(
        ids,
        [
            "appearance",
            "terminal",
            "agents",
            "dictation",
            "mods",
            "machines",
            "diagnostics",
            "about"
        ]
    );
    let item = f
        .b()
        .get_rail()
        .iter()
        .find(|r| r.id == "machines")
        .unwrap();
    assert_eq!(item.label.as_str(), "Machines");
    assert!(!item.icon.is_empty());
}

#[test]
fn with_no_machine_the_page_is_the_add_row_alone_with_no_subtext() {
    for s in SCALES2 {
        let f = fx_at(s, FakeIo::default());
        f.page();
        f.settle("the read", |f| f.row("machines-add").is_some());
        assert_eq!(f.kinds(), [BlockKind::Row], "{s}");
        let add = f.row("machines-add").unwrap();
        assert_eq!(add.label.as_str(), "Add a machine");
        assert_eq!(add.sub.as_str(), "", "no subtext (owner pick)");
        assert!(add.tap);
        assert!(f.rows().iter().all(|r| r.sub.is_empty()));
    }
}

#[test]
fn linked_machines_are_a_table_with_logo_host_version_and_status() {
    for s in SCALES2 {
        let mut io = FakeIo::default();
        *io.list.get_mut().unwrap() = vec![mac()];
        io.answers.insert(
            ("mini".into(), "flare".into()),
            Ok(HelloFacts {
                version: "0.3.1".into(),
                os: Some(Os::Mac),
            }),
        );
        let f = fx_at(s, io);
        f.page();
        f.settle("the check", |f| {
            f.row("machine:mac")
                .is_some_and(|r| r.state.as_str() == "linked")
        });
        assert_eq!(
            f.kinds(),
            [BlockKind::MachineHead, BlockKind::Machine, BlockKind::Row],
            "{s}"
        );
        let m = f.row("machine:mac").unwrap();
        assert_eq!(m.label.as_str(), "Mac Mini");
        assert_eq!(m.value.as_str(), "mini");
        assert_eq!(m.note.as_str(), "0.3.1");
        assert_eq!(m.badge.as_str(), "Linked");
        assert_eq!(m.sub.as_str(), "", "a linked machine has no line under it");
        assert!((f64::from(m.h) - 50.0).abs() < 0.5, "{s}: one line high");
        assert_eq!(m.vendor.as_str(), "mac", "the Apple logo");
        // Every block sits in its panel on whole device pixels, one after the other.
        let sec = &f.sections()[0];
        let rows: Vec<SetRow> = sec.rows.iter().collect();
        for w in rows.windows(2) {
            assert!((w[0].y + w[0].h - w[1].y).abs() < 0.01, "{s}: blocks touch");
        }
        for r in &rows {
            let dev = f64::from(r.y) * f64::from(s);
            assert!((dev - dev.round()).abs() < 0.01, "{s}: whole device pixels");
        }
    }
}

#[test]
fn the_menu_offers_test_and_remove_and_remove_tells_the_window() {
    let mut io = FakeIo::default();
    *io.list.get_mut().unwrap() = vec![mac()];
    let f = fx_at(1.0, io);
    f.page();
    f.settle("the check", |f| {
        f.row("machine:mac")
            .is_some_and(|r| r.state.as_str() == "failed")
    });
    let m = f.row("machine:mac").unwrap();
    assert_eq!(m.badge.as_str(), "Not linked");
    assert_eq!(m.sub.as_str(), "No Flare answered over SSH on mini.");
    assert_eq!(
        m.row_tip.as_str(),
        "",
        "the reason is a line, not a tooltip"
    );
    f.b()
        .invoke_select_open_request("machine:mac".into(), 600.0, 200.0, 120.0, 30.0);
    assert!(f.b().get_select_open());
    let names: Vec<String> = f
        .b()
        .get_select_options()
        .iter()
        .map(|o| o.name.to_string())
        .collect();
    assert_eq!(names, ["Test", "Remove"]);
    let probes = f.io.probes.lock().unwrap().len();
    f.b().invoke_select_chosen("test".into());
    f.settle("the second check", |f| {
        f.io.probes.lock().unwrap().len() > probes
            && f.row("machine:mac")
                .is_some_and(|r| r.state.as_str() == "failed")
    });
    f.b()
        .invoke_select_open_request("machine:mac".into(), 600.0, 200.0, 120.0, 30.0);
    f.b().invoke_select_chosen("remove".into());
    f.settle("the remove", |f| f.row("machine:mac").is_none());
    assert!(f.io.list().is_empty());
    assert_eq!(*f.sent.borrow(), vec![Vec::<Machine>::new()]);
    assert_eq!(f.kinds(), [BlockKind::Row]);
}

impl Fx {
    fn look(&self) -> crate::MachineAddLook {
        self.b().get_machine_add()
    }

    fn option_names(&self) -> Vec<String> {
        self.b()
            .get_select_options()
            .iter()
            .map(|o| o.name.to_string())
            .collect()
    }
}

/// Rule 1251: the add row opens the Add a machine dialog over the page (the page keeps its add row); its Machine
/// drop-down lists the SSH hosts in the code font, then "Type an address"; Add waits for a pick; a failed check is one
/// red line in the dialog, which stays open; a machine that answers is saved and the dialog closes; "Type an address"
/// shows the Address field, whose keystrokes reach the page with no rebuild; Cancel and Escape close it. At 100 and
/// 225 percent.
#[test]
fn the_add_row_opens_the_dialog_and_a_failed_check_is_one_red_line_in_it() {
    use flare_settings::page::machines::TYPE_ADDRESS;
    for s in SCALES2 {
        let mut io = FakeIo {
            hosts: vec!["studio".into(), "nas".into()],
            ..FakeIo::default()
        };
        io.answers.insert(
            ("studio".into(), "flare".into()),
            Err((true, "remote protocol 1 here, 0 there".into())),
        );
        io.answers.insert(
            ("nas".into(), FLARE_CANDIDATES[5].into()),
            Ok(HelloFacts {
                version: "0.3.1".into(),
                os: Some(Os::Mac),
            }),
        );
        let f = fx_at(s, io);
        f.page();
        f.settle("the read", |f| f.row("machines-add").is_some());
        assert!(!f.b().get_machine_add_open(), "{s}: closed at first");
        f.b().invoke_row_button("machines-add".into(), "tap".into());
        assert!(f.b().get_machine_add_open(), "{s}: the dialog opens");
        assert_eq!(
            f.kinds(),
            [BlockKind::Row],
            "{s}: the page keeps its add row"
        );
        // The hosts are read on a worker: the drop-down offers them once they are in.
        f.settle("the hosts are read", |f| {
            slint::Model::row_count(&f.look().options) == 3
        });
        let look = f.look();
        assert_eq!(look.pick.as_str(), "", "nothing picked: Pick a machine");
        assert!(!look.can_add && !look.typing && look.error.is_empty());
        // The drop-down is the Select menu: the hosts in the code font, then "Type an address" in the UI font.
        f.b()
            .invoke_select_open_request("machines-add".into(), 280.0, 250.0, 340.0, 32.0);
        assert!(f.b().get_select_open());
        assert_eq!(f.option_names(), ["studio", "nas", "Type an address"]);
        let faces: Vec<String> = f
            .b()
            .get_select_options()
            .iter()
            .map(|o| o.face.to_string())
            .collect();
        let mono = f.h.comp.global::<crate::SettingsFixed>().get_font_mono();
        assert_eq!(faces, [mono.to_string(), mono.to_string(), String::new()]);
        f.b().invoke_select_chosen("studio".into());
        assert!(!f.b().get_select_open());
        assert_eq!(f.look().pick.as_str(), "studio");
        assert!(f.look().can_add);
        f.b().invoke_row_button("machines-add".into(), "add".into());
        assert!(f.look().busy && !f.look().can_add, "{s}: Add waits");
        f.settle("the failed check", |f| {
            !f.look().error.is_empty() && !f.look().busy
        });
        assert_eq!(
            f.look().error.as_str(),
            "Flare there speaks remote protocol 0, here 1. Update it there."
        );
        assert!(f.b().get_machine_add_open(), "{s}: the dialog stays open");
        assert!(f.io.list().is_empty(), "nothing saved");
        // Another host answers: saved, the dialog closes, the window hears the list.
        f.b()
            .invoke_select_open_request("machines-add".into(), 280.0, 250.0, 340.0, 32.0);
        f.b().invoke_select_chosen("nas".into());
        assert!(f.look().error.is_empty(), "a new pick clears the line");
        f.b().invoke_row_button("machines-add".into(), "add".into());
        f.settle("the save", |f| f.row("machine:nas").is_some());
        assert!(!f.b().get_machine_add_open(), "{s}: the dialog closes");
        let saved = f.io.list();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].kind, Some(Os::Mac));
        assert_eq!(f.sent.borrow().len(), 1);
        let row = f.row("machine:nas").unwrap();
        assert_eq!(row.state.as_str(), "linked");
        assert_eq!(row.value.as_str(), "nas", "the Address column");
        // "Type an address": the fields show, a keystroke reaches the page with no rebuild, Cancel closes.
        f.b().invoke_row_button("machines-add".into(), "tap".into());
        f.b()
            .invoke_select_open_request("machines-add".into(), 280.0, 250.0, 340.0, 32.0);
        f.b().invoke_select_chosen(TYPE_ADDRESS.into());
        assert!(f.look().typing && !f.look().can_add);
        let sections = f.b().get_sections();
        f.b()
            .invoke_row_text("machines-add:address".into(), "192.168.0.9".into());
        f.b()
            .invoke_row_text("machines-add:user".into(), "max".into());
        assert_eq!(f.look().address.as_str(), "192.168.0.9");
        assert_eq!(f.look().user.as_str(), "max");
        assert!(f.look().can_add);
        assert!(
            sections == f.b().get_sections(),
            "{s}: typing rebuilds no page"
        );
        f.b()
            .invoke_row_button("machines-add".into(), "cancel".into());
        assert!(!f.b().get_machine_add_open());
        // Escape that no control took closes it too.
        f.b().invoke_row_button("machines-add".into(), "tap".into());
        assert!(f.ctl.borrow_mut().escape());
        assert!(!f.b().get_machine_add_open());
    }
}

/// Rule 1251: the dialog draws only over the Machines page. Another page closes it, and a form that is still open
/// (a check that runs) never draws over another page or over a hidden Settings tab.
#[test]
fn the_dialog_never_draws_over_another_page() {
    let f = fx_at(1.0, FakeIo::default());
    f.page();
    f.settle("the read", |f| f.row("machines-add").is_some());
    f.b().invoke_row_button("machines-add".into(), "tap".into());
    assert!(f.b().get_machine_add_open());
    f.b().invoke_page_chosen("terminal".into());
    assert!(!f.b().get_machine_add_open(), "another page closes it");
    f.page();
    assert!(!f.b().get_machine_add_open(), "and it stays closed");
    // A busy form outlives a hide, but draws only once Settings and its page are back.
    f.b().invoke_row_button("machines-add".into(), "tap".into());
    f.ctl.borrow_mut().hide();
    assert!(!f.b().get_machine_add_open(), "hidden with Settings");
}

/// Rule 1250: a machine whose ssh said why it failed shows that in plain words as the grey line under its row.
#[test]
fn a_machine_whose_ssh_said_why_shows_it_in_plain_words_under_its_row() {
    struct Said(FakeIo);
    impl MachinesIo for Said {
        fn list(&self) -> Vec<Machine> {
            self.0.list()
        }
        fn save(&self, list: &[Machine]) -> Result<(), String> {
            self.0.save(list)
        }
        fn ssh_hosts(&self) -> Vec<String> {
            self.0.ssh_hosts()
        }
        fn probe(&self, _m: &Machine) -> Result<HelloFacts, crate::remote_link::LinkError> {
            Err(crate::remote_link::LinkError::Down(
                "the link ended; ssh said: max@mini: Permission denied (publickey).".into(),
            ))
        }
    }
    let io = FakeIo::default();
    *io.list.lock().unwrap() = vec![mac()];
    let f = fx_at(1.0, FakeIo::default());
    let said = Arc::new(Said(io));
    f.ctl.borrow_mut().set_machines(MachinesPage::new(said, {
        let q = Arc::clone(&f.q);
        Arc::new(move |m: UiMsg| {
            q.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(m);
        })
    }));
    f.page();
    f.settle("the check", |f| {
        f.row("machine:mac")
            .is_some_and(|r| r.state.as_str() == "failed")
    });
    let m = f.row("machine:mac").unwrap();
    assert_eq!(m.sub.as_str(), "Not paired with this computer");
    assert_eq!(m.badge.as_str(), "Not linked");
}

/// Rule 1251: the dialog draws in the Dialog look: a 380 px box in the middle of the scrim, "Pick a machine" in the
/// drop-down, Add dimmed and deaf until it can be pressed, Cancel and a press outside cancel, the drop-down's menu opens
/// above the dialog and a pick reports its id, "Type an address" grows the box by the Address and User fields, and the
/// red line grows it by one line. At 100 and 225 percent.
#[test]
fn the_dialog_draws_its_box_its_drop_down_and_its_red_line() {
    use crate::MachineAddLook;
    use crate::SetOption;
    use slint::{ModelRc, SharedString, VecModel};
    for s in SCALES2 {
        let h = Harness::new(s, 900, 600, || crate::MachineAddPreview::new().unwrap());
        h.activate();
        let options = ModelRc::new(VecModel::from(
            ["studio", "nas"]
                .iter()
                .map(|n| SetOption {
                    id: (*n).into(),
                    name: (*n).into(),
                    face: SharedString::new(),
                })
                .chain(std::iter::once(SetOption {
                    id: " address".into(),
                    name: "Type an address".into(),
                    face: SharedString::new(),
                }))
                .collect::<Vec<_>>(),
        ));
        let look = |pick: &str, typing: bool, error: &str, can_add: bool| MachineAddLook {
            pick: pick.into(),
            options: options.clone(),
            typing,
            error: error.into(),
            can_add,
            ..MachineAddLook::default()
        };
        h.comp.set_look(look("", false, "", false));
        h.comp.set_open(true);
        h.advance(16);
        let (bx, by, bw, bh) = (
            h.comp.get_box_x(),
            h.comp.get_box_y(),
            h.comp.get_box_w(),
            h.comp.get_box_h(),
        );
        assert!((bw - 380.0).abs() < 0.5, "{s}: 380 px wide ({bw})");
        assert!((bx + bw / 2.0 - 450.0).abs() < 1.0, "{s}: centred ({bx})");
        assert!((by + bh / 2.0 - 300.0).abs() < 1.0, "{s}: centred ({by})");
        // "Pick a machine" draws in the drop-down: dim ink, left of its chevron.
        let k = |v: f32| (v * s).round() as u32;
        let (px, py, pw) = (
            h.comp.get_pick_x(),
            h.comp.get_pick_y(),
            h.comp.get_pick_w(),
        );
        assert!(
            (pw - 340.0).abs() < 0.5,
            "{s}: the drop-down spans the box ({pw})"
        );
        let frame = h.render();
        let ground = frame.px(k(px + pw - 40.0), k(py + 4.0));
        let ink = frame.count_where(k(px + 8.0), k(py + 6.0), k(px + 120.0), k(py + 26.0), |p| {
            (0..3).any(|c| p[c].abs_diff(ground[c]) > 40)
        });
        assert!(ink > 20, "{s}: the placeholder draws ({ink} lit)");
        // Add is dimmed and deaf; Cancel answers.
        let (ax, ay, aw) = (h.comp.get_add_x(), h.comp.get_add_y(), h.comp.get_add_w());
        h.click(ax + aw / 2.0, ay + 15.0);
        assert_eq!(h.comp.get_adds(), 0, "{s}: Add waits for a pick");
        let (cx, cy, cw) = (
            h.comp.get_cancel_x(),
            h.comp.get_cancel_y(),
            h.comp.get_cancel_w(),
        );
        h.click(cx + cw / 2.0, cy + 15.0);
        assert_eq!(h.comp.get_cancels(), 1, "{s}");
        // A press outside the box cancels; nothing behind it is reached.
        h.click(20.0, 20.0);
        assert_eq!(h.comp.get_cancels(), 2, "{s}");
        assert_eq!(
            h.comp.get_behind_clicks(),
            0,
            "{s}: the scrim takes the press"
        );
        // The drop-down opens its menu above the dialog; a pick of the third choice reports its id.
        h.click(px + pw / 2.0, py + 16.0);
        h.advance(16);
        assert!(h.comp.get_menu_open(), "{s}: the menu opens");
        let opt_h = 28.0_f32;
        let menu_top = py + 32.0 + 4.0 + 2.0;
        h.click(px + 40.0, menu_top + opt_h * 2.5);
        h.advance(16);
        assert!(!h.comp.get_menu_open(), "{s}: a pick closes it");
        assert_eq!(h.comp.get_picked().as_str(), " address", "{s}");
        // "Type an address": the box grows by the two fields; typing reaches the dialog's callback.
        h.comp.set_look(look(" address", true, "", false));
        h.advance(16);
        // The fields are made when the dialog is next drawn.
        h.render();
        let typing_h = h.comp.get_box_h();
        assert!(
            typing_h > bh + 100.0,
            "{s}: the fields grow the box ({typing_h} over {bh})"
        );
        h.key_text("10.0.0.5");
        h.advance(16);
        assert_eq!(
            h.comp.get_address_typed().as_str(),
            "10.0.0.5",
            "{s}: the Address field has the keys"
        );
        // A red line grows the box by one line, and Add answers once it can be pressed.
        h.comp
            .set_look(look(" address", true, "Not answering", true));
        h.advance(16);
        h.render();
        let with_line = h.comp.get_box_h();
        assert!(
            with_line > typing_h + 12.0 && with_line < typing_h + 40.0,
            "{s}: one line ({with_line} over {typing_h})"
        );
        let (ax, ay, aw) = (h.comp.get_add_x(), h.comp.get_add_y(), h.comp.get_add_w());
        h.click(ax + aw / 2.0, ay + 15.0);
        assert_eq!(h.comp.get_adds(), 1, "{s}: Add answers");
        // The red line's dot is the danger red.
        let f = h.render();
        let reds = f.count_where(
            k(h.comp.get_box_x()),
            k(h.comp.get_box_y()),
            k(h.comp.get_box_x() + 380.0),
            k(h.comp.get_box_y() + with_line),
            |p| p[0] > 140 && p[1] < 90 && p[2] < 100,
        );
        assert!(reds > 4, "{s}: the red dot draws ({reds})");
    }
}
/// The table row draws its system's logo before the name (the Apple logo for a Mac, nothing for a machine of a system
/// not known), and its menu button takes a click; at 100 and 225 percent.
#[test]
fn a_machine_row_draws_its_logo_and_its_menu_button_takes_a_click() {
    for s in SCALES2 {
        let h = Harness::new(s, 760, 220, || crate::MachinesPreview::new().unwrap());
        let row = |vendor: &str| SetRow {
            id: "machine:mac".into(),
            label: "Mac Mini".into(),
            value: "mini".into(),
            note: "0.1.0".into(),
            state: "linked".into(),
            badge: "Linked".into(),
            vendor: vendor.into(),
            ..SetRow::default()
        };
        // The logo's box: 14 to 29 css px across, the row's middle 15 px (the row is at 34, 50 high).
        let lit = |h: &Harness<crate::MachinesPreview>| {
            let f = h.render();
            let k = |v: f32| (v * s).round() as u32;
            f.count_where(k(14.0), k(51.5), k(29.0), k(66.5), |p| {
                p[0] > 150 && p[1] > 150 && p[2] > 150
            })
        };
        h.comp.set_machine(row("mac"));
        h.advance(0);
        let mac = lit(&h);
        h.comp.set_machine(row(""));
        h.advance(16);
        let none = lit(&h);
        assert!(mac > 20, "{s}: the Apple logo draws ({mac} lit)");
        assert_eq!(none, 0, "{s}: no system, no logo");
        // The menu button sits at the row's right end.
        h.click(760.0 - 21.0, 34.0 + 25.0);
        assert_eq!(h.comp.get_menus(), 1, "{s}");
    }
}

/// A machine that is Not linked carries the reason as its row's line (68 high, its blocks still touching on whole device
/// pixels), and hovering anywhere on the page, its status included, asks for no tooltip with the reason (owner rule
/// 2026-11-13: a control with words has no tooltip); at 100 and 225 percent.
#[test]
fn a_not_linked_machine_shows_its_reason_under_the_row_and_hovering_its_status_shows_no_tooltip() {
    for s in SCALES2 {
        let mut io = FakeIo::default();
        *io.list.get_mut().unwrap() = vec![mac()];
        let f = fx_at(s, io);
        f.page();
        f.settle("the check", |f| {
            f.row("machine:mac")
                .is_some_and(|r| r.state.as_str() == "failed")
        });
        let m = f.row("machine:mac").unwrap();
        assert_eq!(m.badge.as_str(), "Not linked");
        assert_eq!(m.sub.as_str(), "No Flare answered over SSH on mini.");
        assert_eq!(m.row_tip.as_str(), "");
        assert!(
            (f64::from(m.h) - 68.0).abs() < 0.5,
            "{s}: the row grows by its line ({})",
            m.h
        );
        let rows: Vec<SetRow> = f.sections()[0].rows.iter().collect();
        for w in rows.windows(2) {
            assert!((w[0].y + w[0].h - w[1].y).abs() < 0.01, "{s}: blocks touch");
        }
        for r in &rows {
            let dev = f64::from(r.y + r.h) * f64::from(s);
            assert!((dev - dev.round()).abs() < 0.01, "{s}: whole device pixels");
        }
        let tips: Rc<RefCell<Vec<String>>> = Rc::default();
        {
            let tips = Rc::clone(&tips);
            f.b()
                .on_tip(move |t, _, _, _, _| tips.borrow_mut().push(t.to_string()));
        }
        f.h.render();
        // The pointer crosses the whole page in small steps, over the status column and the reason line too.
        // Each step lets the hover handlers run, so a tooltip on any spot would be asked for.
        let mut y = 0.0;
        while y < 640.0 {
            let mut x = 0.0;
            while x < 1000.0 {
                f.h.move_to(x, y);
                f.h.advance(0);
                x += 8.0;
            }
            y += 4.0;
        }
        f.h.leave();
        assert!(
            tips.borrow()
                .iter()
                .all(|t| !t.contains("No Flare answered")),
            "{s}: no tooltip says the reason ({:?})",
            tips.borrow()
        );
    }
}

/// The reason is drawn as one grey line under the machine's row, from the name's left edge, while the row's columns
/// stay in its top 50 px; a long reason is elided on that one line and never reaches the menu. At 100 and 225 percent.
#[test]
fn a_machine_row_draws_its_reason_as_one_grey_line_under_it() {
    for s in SCALES2 {
        let h = Harness::new(s, 760, 240, || crate::MachinesPreview::new().unwrap());
        let row = |sub: &str, rh: f32| SetRow {
            id: "machine:mac".into(),
            label: "Mac Mini".into(),
            value: "mini".into(),
            state: "failed".into(),
            badge: "Not linked".into(),
            sub: sub.into(),
            vendor: "mac".into(),
            h: rh,
            ..SetRow::default()
        };
        let k = |v: f32| (v * s).round() as u32;
        // Ink that is not the ground (sampled left of the name, inside the row), in a box of css px.
        let ink = |f: &crate::ui_harness::Frame, x0: f32, y0: f32, x1: f32, y1: f32| {
            let g = f.px(k(4.0), k(34.0 + 30.0));
            f.count_where(k(x0), k(y0), k(x1), k(y1), |p| {
                (0..3).any(|c| p[c].abs_diff(g[c]) > 30)
            })
        };
        // The top edge of the name's bright pixels, in device px.
        let name_top = |f: &crate::ui_harness::Frame| {
            f.bbox_where(k(37.0), k(34.0), k(200.0), k(34.0 + 68.0), |p| {
                p[0] > 150 && p[1] > 150 && p[2] > 150
            })
            .map(|b| b.1)
        };
        let long = "No Flare answered over SSH on mini. ".repeat(8);
        // The line's band: inside 44 to 60 px down the 68 px row.
        let (bx0, by0, bx1, by1) = (37.0, 34.0 + 46.0, 700.0, 34.0 + 58.0);
        h.comp.set_machine(row("", 50.0));
        h.advance(0);
        let top_linked = name_top(&h.render());
        assert!(top_linked.is_some(), "{s}: the name draws");
        h.comp.set_machine(row("", 68.0));
        h.advance(16);
        let none = ink(&h.render(), bx0, by0, bx1, by1);
        h.comp.set_machine(row(&long, 68.0));
        h.advance(16);
        let f = h.render();
        let lit = ink(&f, bx0, by0, bx1, by1);
        assert!(lit > 40, "{s}: the reason draws under the row ({lit} lit)");
        assert_eq!(none, 0, "{s}: no reason, no line");
        assert_eq!(
            name_top(&f),
            top_linked,
            "{s}: the columns keep the top 50 px"
        );
        // One line: nothing under the band, nothing between the band's end and the menu's column.
        assert_eq!(
            ink(&f, 0.0, 34.0 + 61.0, 760.0, 34.0 + 67.0),
            0,
            "{s}: one line"
        );
        assert_eq!(
            ink(&f, 721.0, by0, 760.0, by1),
            0,
            "{s}: elided before the menu"
        );
        // The line is grey ink (the sub line's), not the status's red.
        let g = f.px(k(4.0), k(34.0 + 30.0));
        let grey = f.count_where(k(bx0), k(by0), k(bx1), k(by1), |p| {
            let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
            (0..3).any(|c| p[c].abs_diff(g[c]) > 30) && hi - lo < 24
        });
        assert!(grey > lit / 2, "{s}: grey ink ({grey} of {lit})");
    }
}
