//! Settings > Machines (remote sessions PR 7), headless, at 100 and 225 percent: the rail lists the page after
//! Plugins; with no machine it is the "+ Add a machine" row alone, with no subtext; linked machines are a table under a
//! heading row (logo, name, Address, Flare version, status, a menu with Test and Remove); the add row opens the Add a
//! machine dialog (machine pairing PR 1, rule 1237) and a failed check is one red line in it; a machine that is Not
//! linked shows why as one grey line under its row, in plain words when ssh said why (rule 1236), with no tooltip. Every link check runs against an in-memory
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
    /// The settings store (the Computer sharing switch saves there).
    prefs: PrefStore,
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
    fx_with(
        scale,
        io,
        Arc::new(crate::sharing::NoSharingOs),
        PrefStore::memory(),
    )
}

/// The page with the Computer sharing switch's OS calls (a fake) and a settings store the test holds.
fn fx_with(
    scale: f32,
    io: FakeIo,
    sharing: Arc<dyn crate::sharing::SharingOs>,
    prefs: PrefStore,
) -> Fx {
    fx_full(scale, io, sharing, prefs, None)
}

/// Pairing for a page (machine pairing PR 3): the fake network, and the folder for Flare's key and known hosts.
type PairWith = (Arc<crate::pairing::fake::FakeNet>, std::path::PathBuf);

fn fx_full(
    scale: f32,
    io: FakeIo,
    sharing: Arc<dyn crate::sharing::SharingOs>,
    prefs: PrefStore,
    pair: Option<PairWith>,
) -> Fx {
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
    let mut page = MachinesPage::new(io.clone(), post).with_sharing_os(sharing);
    if let Some((net, dir)) = pair {
        page = page.with_pairing(
            net,
            crate::pair_keys::PairFiles::new(&dir),
            crate::pairing::Me {
                name: "Max's PC".into(),
                os: Some(Os::Pc),
                version: "0.4.2".into(),
            },
        );
    }
    ctl.borrow_mut().set_machines(page);
    ctl.borrow_mut().show();
    Fx {
        h,
        ctl,
        io,
        q,
        sent: RefCell::default(),
        prefs,
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

    /// The Linked machines section (This computer comes before it since machine pairing PR 2).
    fn table(&self) -> SetSection {
        self.sections()
            .into_iter()
            .find(|s| s.id == "machines")
            .expect("the Linked machines section")
    }

    fn kinds(&self) -> Vec<BlockKind> {
        self.table().rows.iter().map(|r| r.block).collect()
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
        assert!(f.table().rows.iter().all(|r| r.sub.is_empty()));
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
        let sec = f.table();
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

/// Rule 1237: the add row opens the Add a machine dialog over the page (the page keeps its add row); its Machine
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

/// Rule 1237: the dialog draws only over the Machines page. Another page closes it, and a form that is still open
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

/// Rule 1236: a machine whose ssh said why it failed shows that in plain words as the grey line under its row.
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

/// Rule 1237: the dialog draws in the Dialog look: a 380 px box in the middle of the scrim, "Pick a machine" in the
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
                    vendor: Default::default(),
                })
                .chain(std::iter::once(SetOption {
                    id: " address".into(),
                    name: "Type an address".into(),
                    face: SharedString::new(),
                    vendor: Default::default(),
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
        let rows: Vec<SetRow> = f.table().rows.iter().collect();
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

/// Rule 1238: the Machines page starts with This computer, whose one row is the Computer sharing switch with its fixed
/// subtext and no tooltip; Linked machines comes after it. Flipping the switch on Windows runs the elevated setup (a
/// fake: nothing runs), the switch shows on and dimmed while it runs, then on and saved. At 100 and 225 percent.
#[test]
fn this_computer_is_first_and_its_switch_turns_sharing_on() {
    use crate::sharing::Platform;
    use crate::sharing::fake::FakeOs;
    for s in SCALES2 {
        let os = Arc::new(FakeOs::new(Platform::Windows));
        *os.elevated.lock().unwrap() = Ok("ok=1\ninstalled=1\nstarted=1\n".into());
        let f = fx_with(s, FakeIo::default(), os.clone(), PrefStore::memory());
        f.page();
        f.settle("the read", |f| f.row("machines-add").is_some());
        let ids: Vec<String> = f.sections().iter().map(|s| s.id.to_string()).collect();
        assert_eq!(ids, ["this-computer", "machines"], "{s}");
        assert_eq!(f.sections()[0].title.as_str(), "This computer");
        assert_eq!(f.sections()[1].title.as_str(), "Linked machines");
        let row = f.row("machines-sharing").unwrap();
        assert_eq!(row.label.as_str(), "Computer sharing");
        assert_eq!(
            row.sub.as_str(),
            "Allows you to control sessions from other devices"
        );
        assert_eq!(row.row_tip.as_str(), "", "a labelled row has no tooltip");
        assert!(!row.on && !row.off && row.tap, "{s}: off at first");
        f.b().invoke_row_switch("machines-sharing".into(), true);
        let busy = f.row("machines-sharing").unwrap();
        assert!(
            busy.on && busy.off,
            "{s}: on and dimmed while the setup runs"
        );
        assert_eq!(busy.sub, row.sub, "the subtext never reports state");
        f.settle("the setup", |f| {
            f.row("machines-sharing").is_some_and(|r| r.on && !r.off)
        });
        assert!(flare_settings::machine_prefs::sharing_on(&f.prefs));
        assert_eq!(
            os.scripts.lock().unwrap().len(),
            1,
            "{s}: one elevated script"
        );
        assert!(!f.b().get_sharing_ask_open());
        assert_eq!(f.row("machines-sharing").unwrap().sub, row.sub);
    }
}

/// Rule 1238: a switch left on is on after a restart (read from the settings), with no setup run again.
#[test]
fn the_sharing_switch_stays_on_across_restarts() {
    use crate::sharing::Platform;
    use crate::sharing::fake::FakeOs;
    let prefs = PrefStore::memory();
    flare_settings::machine_prefs::set_sharing_on(&prefs, true);
    let os = Arc::new(FakeOs::new(Platform::Windows));
    let f = fx_with(1.0, FakeIo::default(), os.clone(), prefs);
    f.page();
    f.settle("the read", |f| f.row("machines-sharing").is_some());
    let row = f.row("machines-sharing").unwrap();
    assert!(row.on && !row.off);
    assert!(os.scripts.lock().unwrap().is_empty());
    assert_eq!(*os.announced.lock().unwrap(), [true]);
}

/// Rule 1240: when macOS refuses, the Flare dialog "Turn on Remote Login" draws over the Machines page with Cancel and
/// Open System Settings; Open System Settings opens Sharing and the dialog waits; it closes by itself once Remote Login
/// is on, and the switch is on. It draws only over the Machines page while Settings is in front.
#[test]
fn a_refused_mac_shows_turn_on_remote_login_over_the_page() {
    use crate::sharing::fake::FakeOs;
    use crate::sharing::{Platform, SHARING_SETTINGS_URL};
    let os = Arc::new(FakeOs::new(Platform::Mac));
    *os.admin.lock().unwrap() =
        Ok("Turning Remote Login on or off requires Full Disk Access privileges.".into());
    let f = fx_with(1.0, FakeIo::default(), os.clone(), PrefStore::memory());
    f.page();
    f.settle("the read", |f| f.row("machines-sharing").is_some());
    f.b().invoke_row_switch("machines-sharing".into(), true);
    f.settle("the refusal", |f| f.b().get_sharing_ask_open());
    let look = f.b().get_sharing_ask();
    assert_eq!(look.title.as_str(), "Turn on Remote Login");
    assert_eq!(look.line.as_str(), "Flare continues when it is on");
    assert!(look.spin);
    assert_eq!(look.cancel.as_str(), "Cancel");
    assert_eq!(look.primary.as_str(), "Open System Settings");
    let row = f.row("machines-sharing").unwrap();
    assert!(row.on && row.off, "on and dimmed while Flare waits");
    // Another page, or Settings hidden: the dialog waits off screen and comes back with the page.
    f.b().invoke_page_chosen("terminal".into());
    assert!(!f.b().get_sharing_ask_open());
    f.page();
    assert!(f.b().get_sharing_ask_open());
    f.b()
        .invoke_row_button("machines-sharing".into(), "primary".into());
    f.settle("System Settings opens", |_| {
        !os.urls.lock().unwrap().is_empty()
    });
    assert_eq!(*os.urls.lock().unwrap(), [SHARING_SETTINGS_URL]);
    assert!(f.b().get_sharing_ask_open(), "the dialog waits");
    os.open_port();
    f.settle("Remote Login is on", |f| !f.b().get_sharing_ask_open());
    let row = f.row("machines-sharing").unwrap();
    assert!(row.on && !row.off);
    assert!(flare_settings::machine_prefs::sharing_on(&f.prefs));
}

/// Rule 1241: turning off when Flare turned the SSH server on asks "Stop the SSH server too?" with Keep it and Stop
/// it; the switch is off at once; Escape is Keep it.
#[test]
fn turning_sharing_off_asks_to_stop_flares_server() {
    use crate::sharing::Platform;
    use crate::sharing::fake::FakeOs;
    let prefs = PrefStore::memory();
    flare_settings::machine_prefs::set_sharing_on(&prefs, true);
    flare_settings::machine_prefs::set_server_by_flare(&prefs, true);
    let os = Arc::new(FakeOs::new(Platform::Windows));
    let f = fx_with(1.0, FakeIo::default(), os.clone(), prefs);
    f.page();
    f.settle("the read", |f| f.row("machines-sharing").is_some());
    f.b().invoke_row_switch("machines-sharing".into(), false);
    assert!(f.b().get_sharing_ask_open());
    let look = f.b().get_sharing_ask();
    assert_eq!(look.title.as_str(), "Stop the SSH server too?");
    assert_eq!(
        (look.cancel.as_str(), look.primary.as_str()),
        ("Keep it", "Stop it")
    );
    assert!(!look.spin);
    assert!(!f.row("machines-sharing").unwrap().on, "off at once");
    assert!(!flare_settings::machine_prefs::sharing_on(&f.prefs));
    assert!(f.ctl.borrow_mut().escape(), "Escape is Keep it");
    assert!(!f.b().get_sharing_ask_open());
    f.settle("the clean up", |f| {
        f.row("machines-sharing").is_some_and(|r| !r.off)
    });
    assert_eq!(*os.stripped.lock().unwrap(), 1, "Flare's keys removed");
    assert!(os.scripts.lock().unwrap().is_empty(), "the server stays");
    assert!(flare_settings::machine_prefs::server_by_flare(&f.prefs));
}

/// Rules 1240 and 1241: the sharing dialog draws in the Dialog look, a 380 px box in the middle of the scrim; a
/// notice has its one button only; the spinner turns before a waiting line; the buttons and Escape answer. At 100 and
/// 225 percent.
#[test]
fn the_sharing_dialog_draws_its_box_its_line_and_its_buttons() {
    use crate::SharingAskLook;
    for s in SCALES2 {
        let h = Harness::new(s, 900, 600, || crate::SharingAskPreview::new().unwrap());
        h.activate();
        h.comp.set_look(SharingAskLook {
            title: "Turn on Remote Login".into(),
            line: "Flare continues when it is on".into(),
            spin: true,
            cancel: "Cancel".into(),
            primary: "Open System Settings".into(),
        });
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
        assert!(h.comp.get_cancel_w() > 40.0, "{s}: Cancel shows");
        // The spinner's accent arc draws left of the line, and it turns.
        let k = |v: f32| (v * s).round() as u32;
        let accent = |f: &crate::ui_harness::Frame| {
            f.count_where(
                k(bx + 18.0),
                k(by + 40.0),
                k(bx + 36.0),
                k(by + 70.0),
                |p| p[0] > 180 && p[1] > 100 && p[1] < 190 && p[2] < 110,
            )
        };
        let first = h.render();
        assert!(
            accent(&first) > 2,
            "{s}: the spinner draws ({})",
            accent(&first)
        );
        let (px, py, pw) = (
            h.comp.get_primary_x(),
            h.comp.get_primary_y(),
            h.comp.get_primary_w(),
        );
        h.click(px + pw / 2.0, py + 15.0);
        assert_eq!(h.comp.get_primaries(), 1, "{s}");
        h.click(20.0, 20.0);
        assert_eq!(h.comp.get_cancels(), 1, "{s}: a press outside is Cancel");
        h.key_text("\u{1b}");
        assert_eq!(h.comp.get_cancels(), 2, "{s}: Escape is Cancel");
        // A notice: OK only.
        h.comp.set_look(SharingAskLook {
            title: "Computer sharing".into(),
            line: "Windows' SSH server would not start.".into(),
            spin: false,
            cancel: "".into(),
            primary: "OK".into(),
        });
        h.advance(16);
        h.render();
        assert!(h.comp.get_cancel_w() < 0.5, "{s}: no left button");
        assert_eq!(accent(&h.render()), 0, "{s}: no spinner");
    }
}

/// Rule 1243 (mockup "The code"): the host's code dialog draws in the Dialog look, a 380 px box in the middle, with
/// the code in large digits across its middle; Decline and Escape answer, a press outside does not. At 100 and 225
/// percent.
#[test]
fn the_code_dialog_draws_the_code_in_the_middle_and_decline_answers() {
    use crate::PairCodeLook;
    for s in SCALES2 {
        let h = Harness::new(s, 900, 600, || crate::PairCodePreview::new().unwrap());
        h.activate();
        h.comp.set_look(PairCodeLook {
            title: "Max's Mac mini wants to connect".into(),
            code: "482 913".into(),
            line: "Enter this code on Max's Mac mini".into(),
        });
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
        let (cy, ch) = (h.comp.get_code_y(), h.comp.get_code_h());
        assert!(ch >= 30.0, "{s}: large digits ({ch})");
        // The digits draw in light ink, centred: lit pixels both left and right of the middle, none at the box edges.
        let k = |v: f32| (v * s).round() as u32;
        let f = h.render();
        let lit = |x0: f32, x1: f32| {
            f.count_where(k(x0), k(cy), k(x1), k(cy + ch), |p| {
                p[0] > 170 && p[1] > 170 && p[2] > 160
            })
        };
        assert!(
            lit(bx + 100.0, bx + 190.0) > 30,
            "{s}: the code's left half"
        );
        assert!(
            lit(bx + 190.0, bx + 280.0) > 30,
            "{s}: the code's right half"
        );
        assert_eq!(lit(bx + 2.0, bx + 40.0), 0, "{s}: centred, not at the edge");
        // A press outside does nothing; Decline and Escape decline.
        h.click(20.0, 20.0);
        assert_eq!(
            h.comp.get_declines(),
            0,
            "{s}: a press outside keeps the code"
        );
        assert_eq!(h.comp.get_behind_clicks(), 0, "{s}: the scrim takes it");
        let (dx, dy, dw) = (
            h.comp.get_decline_x(),
            h.comp.get_decline_y(),
            h.comp.get_decline_w(),
        );
        assert!(dx + dw > bx + bw - 30.0, "{s}: Decline sits at the right");
        h.click(dx + dw / 2.0, dy + 15.0);
        assert_eq!(h.comp.get_declines(), 1, "{s}");
        h.key_text("\u{1b}");
        assert_eq!(h.comp.get_declines(), 2, "{s}: Escape is Decline");
    }
}

/// Rule 1243 (mockup "Add a machine"): a nearby computer picked shows "Code shown on <name>" under the drop-down,
/// with the keys in it, and its logo before its name in the drop-down.
#[test]
fn a_nearby_computer_shows_its_logo_and_the_code_field() {
    use crate::MachineAddLook;
    use crate::SetOption;
    use slint::{ModelRc, VecModel};
    for s in SCALES2 {
        let h = Harness::new(s, 900, 600, || crate::MachineAddPreview::new().unwrap());
        h.activate();
        let options = ModelRc::new(VecModel::from(vec![
            SetOption {
                id: " nearby:pc".into(),
                name: "Max's PC".into(),
                face: "".into(),
                vendor: "pc".into(),
            },
            SetOption {
                id: " address".into(),
                name: "Type an address".into(),
                face: "".into(),
                vendor: "".into(),
            },
        ]));
        let look = |code_for: &str| MachineAddLook {
            pick: " nearby:pc".into(),
            options: options.clone(),
            code_for: code_for.into(),
            ..MachineAddLook::default()
        };
        h.comp.set_look(look(""));
        h.comp.set_open(true);
        h.advance(16);
        h.render();
        let plain_h = h.comp.get_box_h();
        // The Windows logo draws before the name in the drop-down (light ink in its first 12 px).
        let k = |v: f32| (v * s).round() as u32;
        let (px, py) = (h.comp.get_pick_x(), h.comp.get_pick_y());
        let f = h.render();
        let logo = f.count_where(
            k(px + 10.0),
            k(py + 10.0),
            k(px + 22.0),
            k(py + 22.0),
            |p| p[0] > 150 && p[1] > 150 && p[2] > 140,
        );
        assert!(logo > 10, "{s}: the logo draws ({logo} lit)");
        // The code field shows under the drop-down and takes the keys.
        h.comp.set_look(look("Max's PC"));
        h.advance(16);
        h.render();
        let code_h = h.comp.get_box_h();
        assert!(
            code_h > plain_h + 40.0 && code_h < plain_h + 80.0,
            "{s}: one labelled field ({code_h} over {plain_h})"
        );
        h.key_text("482913");
        h.advance(16);
        assert_eq!(
            h.comp.get_code_typed().as_str(),
            "482913",
            "{s}: the code field has the keys"
        );
    }
}

/// Rules 1242 and 1243: with Computer sharing on, a computer that asks makes "<name> wants to connect" show with its
/// code, over any tab (here with the Settings tab hidden); Decline closes it and ends the request.
#[test]
fn the_hosts_code_dialog_shows_over_any_tab_and_decline_ends_the_request() {
    use crate::pair_wire as wire;
    use crate::pairing::fake::{FakeNet, Hub};
    use crate::pairing::{Nearby, PairNet as _, SERVICE};
    use crate::sharing::Platform;
    use crate::sharing::fake::FakeOs;
    let hub = Arc::new(Hub::default());
    let net = Arc::new(FakeNet::new(&hub, "192.168.0.139"));
    let dir = tempfile::tempdir().unwrap();
    let prefs = PrefStore::memory();
    flare_settings::machine_prefs::set_sharing_on(&prefs, true);
    let f = fx_full(
        1.0,
        FakeIo::default(),
        Arc::new(FakeOs::new(Platform::Windows)),
        prefs,
        Some((Arc::clone(&net), dir.path().to_path_buf())),
    );
    let port = net.port().expect("sharing on: announced with its port");
    f.ctl.borrow_mut().hide();
    assert!(!f.b().get_pair_code_open());
    // A viewer connects and says hello.
    let viewer = FakeNet::new(&hub, "192.168.0.120");
    let to = Nearby {
        id: format!("Max's PC.{SERVICE}"),
        name: "Max's PC".into(),
        os: Some(Os::Pc),
        version: "0.4.2".into(),
        addrs: vec!["192.168.0.139".parse().unwrap()],
        port,
    };
    let (mut s, _) = viewer.connect(&to).unwrap();
    wire::send(
        &mut *s,
        &serde_json::json!({"t": "hello", "proto": 1, "name": "Max's Mac mini", "os": "mac", "version": "0.4.2"}),
    )
    .unwrap();
    f.settle("the code shows", |f| f.b().get_pair_code_open());
    let look = f.b().get_pair_code();
    assert_eq!(look.title.as_str(), "Max's Mac mini wants to connect");
    assert_eq!(look.line.as_str(), "Enter this code on Max's Mac mini");
    let code = look.code.to_string();
    assert!(
        code.len() == 7 && code.as_bytes()[3] == b' ',
        "two groups of three: {code}"
    );
    assert_eq!(wire::kind(&wire::recv(&mut *s).unwrap()), "shown");
    // Decline: the dialog closes and the viewer's connection ends.
    f.b()
        .invoke_row_button("machines-pair".into(), "decline".into());
    assert!(!f.b().get_pair_code_open());
    assert!(wire::recv(&mut *s).is_err(), "the request ended");
}
