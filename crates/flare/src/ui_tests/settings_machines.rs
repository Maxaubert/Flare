//! Settings > Machines (remote sessions PR 7), headless, at 100 and 225 percent: the rail lists the page after
//! Plugins; with no machine it is the "+ Add a machine" row alone, with no subtext; linked machines are a table under a
//! heading row (logo, name, SSH host, Flare version, status, a menu with Test and Remove); the add row opens the form
//! in its place (host drop-down, name, Cancel, Add) and a failed check is one red line under it. Every link check runs
//! against an in-memory stand-in: no ssh.

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
        assert_eq!(m.sub.as_str(), "Linked");
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
    assert_eq!(m.sub.as_str(), "Not linked");
    assert_eq!(m.row_tip.as_str(), "No Flare answered over SSH on mini.");
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

#[test]
fn the_add_row_opens_the_form_and_a_failed_check_is_one_red_line_under_it() {
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
            ("nas".into(), FLARE_CANDIDATES[1].into()),
            Ok(HelloFacts {
                version: "0.3.1".into(),
                os: Some(Os::Mac),
            }),
        );
        let f = fx_at(s, io);
        f.page();
        f.settle("the read", |f| f.row("machines-add").is_some());
        let closed_h = f.row("machines-add").unwrap().h;
        f.b().invoke_row_button("machines-add".into(), "tap".into());
        // The hosts are read on a worker: the form picks the first once they are in.
        f.settle("the first host is picked", |f| {
            f.row("machines-add")
                .is_some_and(|r| r.value.as_str() == "studio")
        });
        let form = f.row("machines-add").unwrap();
        assert_eq!(form.block, BlockKind::MachineAdd, "{s}");
        assert_eq!(form.value.as_str(), "studio", "the first host is picked");
        let hosts: Vec<String> = form.options.iter().map(|o| o.id.to_string()).collect();
        assert_eq!(hosts, ["studio", "nas"]);
        assert_eq!(form.sub.as_str(), "");
        // The host drop-down is the Select menu; the name is typed.
        f.b()
            .invoke_select_open_request("machines-add".into(), 60.0, 200.0, 168.0, 32.0);
        let names: Vec<String> = f
            .b()
            .get_select_options()
            .iter()
            .map(|o| o.name.to_string())
            .collect();
        assert_eq!(names, ["studio", "nas"]);
        f.b().invoke_select_chosen("studio".into());
        f.b()
            .invoke_row_text("machines-add".into(), "Studio".into());
        f.b().invoke_row_button("machines-add".into(), "add".into());
        assert_eq!(f.row("machines-add").unwrap().state.as_str(), "busy");
        f.settle("the failed check", |f| {
            f.row("machines-add")
                .is_some_and(|r| !r.sub.is_empty() && r.state.is_empty())
        });
        let form = f.row("machines-add").unwrap();
        assert_eq!(
            form.sub.as_str(),
            "Flare there speaks remote protocol 0, here 1. Update it there."
        );
        assert_eq!(form.note.as_str(), "Studio", "the name stays");
        assert!(
            form.h > closed_h,
            "{s}: the form grows by its red line ({} over {closed_h})",
            form.h
        );
        assert!(f.io.list().is_empty(), "nothing saved");
        // Another host answers: saved, the form closes, the window hears the list.
        f.b()
            .invoke_select_open_request("machines-add".into(), 60.0, 200.0, 168.0, 32.0);
        f.b().invoke_select_chosen("nas".into());
        f.b().invoke_row_text("machines-add".into(), "Nas".into());
        f.b().invoke_row_button("machines-add".into(), "add".into());
        f.settle("the save", |f| f.row("machine:nas").is_some());
        let saved = f.io.list();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].kind, Some(Os::Mac));
        assert_eq!(f.sent.borrow().len(), 1);
        let row = f.row("machine:nas").unwrap();
        assert_eq!(row.state.as_str(), "linked");
        assert_eq!(f.row("machines-add").unwrap().block, BlockKind::Row);
        // Cancel closes an open form with nothing saved.
        f.b().invoke_row_button("machines-add".into(), "tap".into());
        f.b()
            .invoke_row_button("machines-add".into(), "cancel".into());
        assert_eq!(f.row("machines-add").unwrap().block, BlockKind::Row);
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
            sub: "Linked".into(),
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
