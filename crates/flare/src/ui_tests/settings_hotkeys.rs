//! Settings > Terminal > Keys > Flare hotkeys (rule 1262), headless at 100 and 225 percent: the row's info button
//! opens the hotkeys sheet with every key of the chord table and the prompt keys, its search filters the rows by
//! action or key and leaves out an empty group, and Escape or a press outside the box closes it.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use flare_settings::kv::PrefStore;
use flare_settings::look::LookStore;
use flare_settings::page::terminal::{HOTKEYS_BUTTON, HOTKEYS_TIP};
use flare_settings::page::{GlassSupport, ThemeColours};
use flare_settings::verb::{FakeDiag, FakeVerb, VerbState};
use flare_tabs::hotkeys::{APP, TABS, TYPING, hotkey_rows};
use flare_tabs::keymap::{ChordId, KeyOs, MAC_ONLY, ORDER, chord, key_label_on};
use slint::{ComponentHandle, Model, SharedString};

use crate::appctl::{Post, UiMsg};
use crate::settings_ctl::{SettingsCtl, SettingsDeps};
use crate::ui_harness::Harness;
use crate::{AppWindow, KeySheetRow, SetRow, SettingsBridge};

const SCALES2: [f32; 2] = [1.0, 2.25];

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

struct Fx {
    h: Harness<AppWindow>,
    q: Queue,
    ctl: Rc<RefCell<SettingsCtl>>,
}

fn fx_at(scale: f32) -> Fx {
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
    h.activate();
    h.advance(0);
    let deps = SettingsDeps {
        prefs,
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
        post,
        version: "1.2.3".to_string(),
        copyright: "Copyright 2026".to_string(),
        e2e: true,
        clock: Rc::new(|| 1_000),
        still: Rc::new(|| false),
    };
    let ctl = SettingsCtl::new(&h.comp, deps);
    ctl.borrow_mut().show();
    let f = Fx { h, q, ctl };
    f.b().invoke_page_chosen("terminal".into());
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
            .find(|r| r.id == "agent-keys")
            .expect("the Flare hotkeys row is drawn")
    }

    fn lines(&self) -> Vec<KeySheetRow> {
        self.b().get_keys_rows().iter().collect()
    }

    /// The sheet's hotkey rows (not the group names), as (action, keys).
    fn hotkeys(&self) -> Vec<(String, String)> {
        self.lines()
            .iter()
            .filter(|r| !r.heading)
            .map(|r| (r.text.to_string(), r.keys.to_string()))
            .collect()
    }

    fn groups(&self) -> Vec<String> {
        self.lines()
            .iter()
            .filter(|r| r.heading)
            .map(|r| r.text.to_string())
            .collect()
    }

    /// Presses the row's info button, as its click does.
    fn open(&self) {
        self.b()
            .invoke_row_button("agent-keys".into(), HOTKEYS_BUTTON.into());
        self.h.advance(16);
    }
}

#[test]
fn the_row_is_flare_hotkeys_with_an_info_button_before_its_switch() {
    for s in SCALES2 {
        let f = fx_at(s);
        let r = f.row();
        assert_eq!(r.label.as_str(), "Flare hotkeys", "{s}");
        assert_eq!(
            r.sub.as_str(),
            "Text box keys in agent prompts, and spare keys for tabs."
        );
        assert!(r.on, "on by default");
        assert_eq!(r.row_tip.as_str(), "", "a labelled control has no tooltip");
        assert_eq!(r.buttons.row_count(), 0);
        let b = r.lead_buttons.row_data(0).expect("the info button");
        assert_eq!(b.id.as_str(), HOTKEYS_BUTTON);
        assert_eq!(b.label.as_str(), "", "icon only");
        assert_eq!(b.tip.as_str(), HOTKEYS_TIP);
        assert_eq!(b.tip.as_str(), "Show all hotkeys");
        assert!(!b.icon.is_empty());
        // No note lines under the switch any more: the next row of the section is the guard.
        let sec = f
            .b()
            .get_sections()
            .iter()
            .find(|s| s.id == "keys")
            .expect("the keys section");
        let ids: Vec<String> = sec.rows.iter().map(|r| r.id.to_string()).collect();
        assert_eq!(ids, ["agent-keys", "agent-guard", "right-click-reports"]);
    }
}

#[test]
fn the_button_opens_the_sheet_the_search_filters_and_escape_closes_it() {
    for s in SCALES2 {
        let f = fx_at(s);
        assert!(!f.b().get_keys_open());
        f.open();
        assert!(f.b().get_keys_open(), "{s}: the sheet opens");
        assert!(
            f.h.comp.get_keys_field_focused(),
            "{s}: the search holds the keys"
        );
        assert_eq!(f.groups(), [TYPING, TABS, APP]);
        let all = hotkey_rows(KeyOs::HOST);
        assert_eq!(f.hotkeys().len(), all.len());
        // Every group's first row wears the stripe, the second does not.
        let lines = f.lines();
        assert!(lines[0].heading && lines[1].stripe && !lines[2].stripe);
        // The box is drawn, inside the window.
        let (bw, bh) = (f.h.comp.get_keys_box_w(), f.h.comp.get_keys_box_h());
        assert!(bw > 300.0 && bw <= 400.0, "{s}: {bw}");
        assert!(bh > 100.0 && bh <= 640.0 - 48.0, "{s}: {bh}");

        // Typing filters by action: "tab" keeps the Tabs and App rows that say it, and no typing row.
        f.h.key_text("tab");
        f.h.advance(16);
        assert_eq!(f.b().get_keys_query().as_str(), "tab");
        let kept = f.hotkeys();
        assert!(kept.iter().any(|(a, _)| a == "Next tab"), "{kept:?}");
        assert!(kept.iter().any(|(a, _)| a == "Tab on a machine"));
        assert!(kept.iter().all(|(a, k)| {
            a.to_lowercase().contains("tab") || k.to_lowercase().contains("tab")
        }));
        assert!(!f.groups().iter().any(|g| g == TYPING), "{:?}", f.groups());

        // By key: Ctrl+Shift+E is the folder key alone, in the App group alone.
        f.b().set_keys_query("".into());
        f.b().invoke_keys_query_edited("".into());
        f.h.key_text(key_label_on(KeyOs::HOST, ChordId::OpenFolder, false));
        f.h.advance(16);
        assert_eq!(
            f.hotkeys(),
            [(
                "Open the folder".to_string(),
                key_label_on(KeyOs::HOST, ChordId::OpenFolder, false).to_string()
            )]
        );
        assert_eq!(f.groups(), [APP]);

        // Nothing matches: no rows and no group names.
        f.b().set_keys_query("".into());
        f.b().invoke_keys_query_edited("".into());
        f.h.key_text("zzqq");
        f.h.advance(16);
        assert!(f.lines().is_empty());

        // Escape closes it.
        let esc: SharedString = slint::platform::Key::Escape.into();
        f.h.key_text(&esc);
        f.h.advance(16);
        assert!(!f.b().get_keys_open(), "{s}: Escape closes the sheet");

        // It opens again with an empty search and every row.
        f.open();
        assert_eq!(f.b().get_keys_query().as_str(), "");
        assert_eq!(f.hotkeys().len(), all.len());
    }
}

#[test]
fn a_press_outside_the_box_closes_the_sheet_and_one_inside_does_not() {
    for s in SCALES2 {
        let f = fx_at(s);
        f.open();
        let (x, y) = (f.h.comp.get_keys_box_x(), f.h.comp.get_keys_box_y());
        let (w, h) = (f.h.comp.get_keys_box_w(), f.h.comp.get_keys_box_h());
        // On the title, inside the box.
        f.h.click(x + 30.0, y + 20.0);
        f.h.advance(16);
        assert!(f.b().get_keys_open(), "{s}: a press inside keeps it");
        // Under the box.
        f.h.click(x + w / 2.0, (y + h + 12.0).min(630.0));
        f.h.advance(16);
        assert!(!f.b().get_keys_open(), "{s}: a press outside closes it");
    }
}

#[test]
fn every_chord_in_the_table_is_on_the_sheet() {
    let f = fx_at(1.0);
    f.open();
    let keys: Vec<String> = f.hotkeys().into_iter().map(|(_, k)| k).collect();
    let os = KeyOs::HOST;
    for id in ORDER.into_iter().chain(MAC_ONLY) {
        // The agent guard is no action (its own switch row): it keeps Ctrl+C and Ctrl+D from an agent.
        if id == ChordId::AgentGuard {
            continue;
        }
        let mut labels = vec![key_label_on(os, id, false)];
        if chord(id).agent.is_some() {
            labels.push(key_label_on(os, id, true));
        }
        for l in labels.into_iter().filter(|l| !l.is_empty()) {
            assert!(
                keys.iter().any(|k| k.split(", ").any(|p| p == l)),
                "{id:?} {l} is on the sheet: {keys:?}"
            );
        }
    }
}

#[test]
fn leaving_settings_closes_the_sheet() {
    let f = fx_at(1.0);
    f.open();
    assert!(f.b().get_keys_open());
    f.ctl.borrow_mut().hide();
    assert!(!f.b().get_keys_open());
}
