//! UI tests of Find a setting and the keyboard (M7 Task 47); headless only.
//!
//! The whole window (`AppWindow`) with the Settings controller on it, as the app builds it: a
//! label typed into the find field, Enter, and the page, scroll, flash and focus are read back
//! from the bridge. The keys of the frame itself (Escape, Tab, the rail's arrows) are read on the
//! `FramePreview` component, which reports every key it hands on.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::platform::Key;
use slint::{ComponentHandle, Model};

use flare_settings::find::{IndexEnv, settings_index};
use flare_settings::help_prefs::help_enabled;
use flare_settings::kv::PrefStore;
use flare_settings::look::LookStore;
use flare_settings::page::{GlassSupport, ThemeColours};
use flare_settings::pages::PAGES;
use flare_settings::rows::{ROW_ORDER, row};
use flare_settings::verb::{FakeDiag, FakeVerb, VerbState};

use crate::AppWindow;
use crate::FramePreview;
use crate::SettingsBridge;
use crate::appctl::{Post, UiMsg};
use crate::settings_ctl::{SettingsCtl, SettingsDeps};
use crate::ui_harness::Harness;

/// The window's size in css px (the view tests use the same).
const W: u32 = 1000;
const H: u32 = 640;

fn esc() -> String {
    String::from(char::from(Key::Escape))
}

fn key_str(k: Key) -> String {
    String::from(char::from(k))
}

/// The settings window with its controller, and the queue its workers answer through.
struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<SettingsCtl>>,
    prefs: PrefStore,
    q: Arc<Mutex<VecDeque<UiMsg>>>,
    clock: Rc<std::cell::Cell<u64>>,
}

fn fx() -> Fx {
    let prefs = PrefStore::memory();
    let look = LookStore::new(prefs.clone());
    let q: Arc<Mutex<VecDeque<UiMsg>>> = Arc::default();
    let post: Post = {
        let q = Arc::clone(&q);
        Arc::new(move |m| {
            q.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push_back(m);
        })
    };
    let clock = Rc::new(std::cell::Cell::new(1_000_u64));
    let h = Harness::new(1.0, W, H, || AppWindow::new().expect("the window builds"));
    h.comp.set_front_kind(1);
    h.advance(0);
    let deps = SettingsDeps {
        prefs: prefs.clone(),
        look,
        diag: Rc::new(FakeDiag::new("C:/logs", false)),
        verb: Arc::new(FakeVerb::new(VerbState::Off)),
        dictation: Rc::new(crate::dictation_page::DictationPageProvider),
        pick_folder: Rc::new(|_, done| done(None::<PathBuf>)),
        open_path: Rc::new(|_: &Path| {}),
        clipboard: Rc::new(|_: &str| {}),
        shell_list: Arc::new(|| vec![("pwsh".to_string(), "PowerShell".to_string())]),
        glass: Rc::new(|| GlassSupport::Ok),
        face_of: Rc::new(|_: &str| String::new()),
        theme_colours: Rc::new(|| ThemeColours {
            accent: "#fe8f34".into(),
            background: "#101010".into(),
            secondary: "#181818".into(),
        }),
        agent_colours: Rc::new(|| ["#111111".into(), "#222222".into(), "#333333".into()]),
        agent_key_labels: Rc::new(String::new),
        post,
        version: "1.2.3".to_string(),
        copyright: "Copyright 2026".to_string(),
        e2e: true,
        clock: {
            let c = Rc::clone(&clock);
            Rc::new(move || c.get())
        },
        still: Rc::new(|| false),
    };
    let ctl = SettingsCtl::new(&h.comp, deps);
    ctl.borrow_mut().show();
    let f = Fx {
        h,
        ctl,
        prefs,
        q,
        clock,
    };
    // The shell list is a worker's answer: it is handed over here, as the window's pump does.
    let start = Instant::now();
    while f.pump() == 0 && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(5));
    }
    f.h.advance(0);
    f
}

impl Fx {
    fn b(&self) -> SettingsBridge<'_> {
        self.h.comp.global::<SettingsBridge>()
    }

    /// Hands every queued message to the controller, as the window's pump does.
    fn pump(&self) -> usize {
        let msgs: Vec<UiMsg> = self
            .q
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect();
        let n = msgs.len();
        for m in msgs {
            if let UiMsg::Settings(m) = m {
                self.ctl.borrow_mut().on_msg(m);
            }
        }
        n
    }

    fn tick(&self, ms: u64) {
        self.clock.set(self.clock.get() + ms);
        self.h.advance(ms);
    }

    /// Pumps the workers' answers until `id` is drawn (the shell list arrives late), then ticks
    /// the poll so the chosen row's flash starts. Gives up after three seconds.
    fn wait_for_row(&self, id: &str) {
        let start = Instant::now();
        while self.row(id).is_none() && start.elapsed() < Duration::from_secs(3) {
            self.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        self.pump();
        self.tick(60);
    }

    fn page(&self, id: &str) {
        self.b().invoke_page_chosen(id.into());
    }

    fn sections(&self) -> Vec<crate::SetSection> {
        self.b().get_sections().iter().collect()
    }

    fn row(&self, id: &str) -> Option<crate::SetRow> {
        self.sections()
            .iter()
            .flat_map(|s| s.rows.iter())
            .find(|r| r.id == id)
    }

    /// Types `text` into the find field: the field holds the text and reports the edit, as the
    /// window does for each keystroke.
    fn type_query(&self, text: &str) {
        self.b().set_find_text(text.into());
        self.b().invoke_find_edited(text.into());
    }

    /// The hits' ids, in result order.
    fn hit_ids(&self) -> Vec<String> {
        self.b()
            .get_hits()
            .iter()
            .map(|h| h.id.to_string())
            .collect()
    }

    /// The text of the line under an empty result list, joined from its three parts.
    fn nothing_line(&self) -> String {
        let b = self.b();
        format!(
            "{}{}{}",
            b.get_nothing_pre(),
            b.get_nothing_query(),
            b.get_nothing_post()
        )
    }
}

/// The rows the bridge draws on one page (the Row blocks only), in drawn order.
fn drawn_rows(f: &Fx) -> Vec<String> {
    f.sections()
        .iter()
        .flat_map(|s| s.rows.iter())
        .filter(|r| r.block == crate::BlockKind::Row)
        .map(|r| r.id.to_string())
        .collect()
}

/// The rows the index says this PC draws (no NVIDIA; the Dictation page is wired, M8 Task 24).
fn index_ids() -> Vec<String> {
    settings_index(&IndexEnv {
        nvidia: false,
        dictation_wired: true,
    })
    .into_iter()
    .map(|e| e.id)
    .collect()
}

// ---- the index and the field ------------------------------------------------------------------

#[test]
fn the_index_has_30_rows_on_this_pc() {
    let n = index_ids().len();
    assert_eq!(
        n, 42,
        "the 33 rows of the other pages and the 9 dictation rows"
    );
    assert!(n >= 30, "PT's own gate says at least 30");
}

#[test]
fn no_status_line_when_the_field_is_empty() {
    let f = fx();
    f.type_query("");
    assert_eq!(
        f.b().get_status_text(),
        "",
        "no status line for an empty field"
    );
    assert!(!f.b().get_searching());
    f.type_query("   ");
    assert_eq!(f.b().get_status_text(), "", "spaces are empty too");
}

#[test]
fn an_empty_field_lets_escape_through_and_text_claims_it() {
    let h = Harness::new(1.0, W, 604, || {
        FramePreview::new().expect("the preview builds")
    });
    h.advance(0);
    h.comp.invoke_focus_field();
    h.advance(0);
    h.key_text(&esc());
    assert_eq!(
        h.comp.get_escape_unhandled_count(),
        1,
        "an empty field hands Escape to the host"
    );
    h.key_text("ab");
    h.key_text(&esc());
    assert_eq!(
        h.comp.get_escape_unhandled_count(),
        1,
        "a field with text claims Escape"
    );
    assert_eq!(h.comp.get_last_key(), "find:Escape");
}

#[test]
fn typing_each_label_opens_its_page_flashes_and_focuses_its_first_control() {
    let f = fx();
    for id in index_ids() {
        let r = row(&id).expect("an indexed row has a definition");
        f.type_query(r.label);
        assert_eq!(
            f.hit_ids().first().map(String::as_str),
            Some(id.as_str()),
            "the label {:?} finds its own row first",
            r.label
        );
        f.b().invoke_find_key("Enter".into(), false);
        assert_eq!(f.ctl.borrow().current_page(), r.page, "{id}: the page");
        // The models' wrapper is no Row (its model blocks are drawn), so there is no row to flash.
        if id == "dictation-model" {
            continue;
        }
        f.wait_for_row(&id);
        let shown = f.row(&id).unwrap_or_else(|| panic!("{id} is drawn"));
        assert!(shown.flash > 0.0, "{id}: the flash is positive");
        assert_eq!(
            f.b().get_focus_id(),
            id.as_str(),
            "{id}: the row has the focus"
        );
        // The scroll is the row's centre, clamped to the page (the row's own geometry).
        let scroll = f64::from(f.b().get_scroll_y());
        let pane_h = f64::from(f.b().get_pane_h());
        let content_h = f64::from(f.b().get_content_h());
        let row_top = f64::from(
            f.sections()
                .iter()
                .find(|s| s.rows.iter().any(|x| x.id == id))
                .unwrap()
                .panel_y
                + shown.y,
        );
        let want = (row_top + f64::from(shown.h) / 2.0 - pane_h / 2.0)
            .clamp(0.0, (content_h - pane_h).max(0.0));
        assert!(
            (scroll - want).abs() < 2.0,
            "{id}: scroll {scroll} is the row's centre {want}"
        );
        f.tick(1500);
    }
}

#[test]
fn the_status_line_says_n_results_and_no_results() {
    let f = fx();
    f.type_query("face");
    let n = f.b().get_hits().row_count();
    assert!(n >= 1, "face finds a row");
    let want = if n == 1 {
        "1 result".to_string()
    } else {
        format!("{n} results")
    };
    assert_eq!(f.b().get_status_text(), want.as_str());
    f.type_query("zzzzqq");
    assert_eq!(f.b().get_status_text(), "No results");
}

#[test]
fn down_marks_the_first_result_and_enter_chooses_it() {
    let f = fx();
    f.type_query("face");
    assert_eq!(f.b().get_hit_cursor(), -1, "no cursor before Down");
    f.b().invoke_find_key("Down".into(), false);
    assert_eq!(f.b().get_hit_cursor(), 0, "Down marks the first result");
    let first = f.hit_ids()[0].clone();
    let page = row(&first).unwrap().page;
    f.b().invoke_hit_key("Enter".into());
    assert_eq!(f.ctl.borrow().current_page(), page);
    assert!(!f.b().get_searching(), "choosing clears the query");
}

#[test]
fn no_rail_item_is_chosen_while_results_show() {
    let f = fx();
    f.page("terminal");
    f.type_query("face");
    assert!(f.b().get_searching());
    let chosen = f.b().get_rail().iter().filter(|r| r.chosen).count();
    assert_eq!(chosen, 0, "no rail page is current while results show");
    f.type_query("");
    let chosen: Vec<String> = f
        .b()
        .get_rail()
        .iter()
        .filter(|r| r.chosen)
        .map(|r| r.id.to_string())
        .collect();
    assert_eq!(
        chosen,
        vec!["terminal".to_string()],
        "the page is current again"
    );
}

#[test]
fn escape_clears_the_field_and_a_list_escape_returns_to_it() {
    let f = fx();
    f.type_query("face");
    f.b().invoke_find_key("Escape".into(), false);
    assert_eq!(f.b().get_find_text(), "", "Escape in the field clears it");
    assert!(!f.b().get_searching());
    // A list Escape clears the query too (the list hands the field back first).
    f.type_query("face");
    f.b().invoke_find_key("Down".into(), false);
    f.b().invoke_hit_key("Escape".into());
    assert_eq!(f.b().get_find_text(), "", "Escape in the list clears it");
    assert!(!f.b().get_searching());
}

#[test]
fn an_unmatched_word_says_nothing_matches_it() {
    let f = fx();
    f.type_query("zebra");
    assert_eq!(
        f.nothing_line(),
        "Nothing matches zebra. Try a shorter word."
    );
    assert_eq!(f.b().get_status_text(), "No results");
    f.type_query("");
    assert_eq!(f.nothing_line(), "");
}

// ---- the keys ---------------------------------------------------------------------------------

#[test]
fn tab_from_the_field_lands_on_the_first_page_down_walks_end_is_about_home_is_appearance_enter_opens()
 {
    let h = Harness::new(1.0, W, 604, || {
        FramePreview::new().expect("the preview builds")
    });
    h.advance(0);
    h.comp.invoke_focus_field();
    h.advance(0);
    h.key_text(&key_str(Key::Tab));
    h.key_text(&key_str(Key::DownArrow));
    assert_eq!(h.comp.get_last_key(), "rail:Down", "Tab lands on the rail");
    h.comp.set_focus_rail_index(0);
    h.advance(0);
    h.key_text(&key_str(Key::End));
    assert_eq!(h.comp.get_last_key(), "rail:End");
    h.key_text(&key_str(Key::Home));
    assert_eq!(h.comp.get_last_key(), "rail:Home");
    h.key_text(&key_str(Key::Return));
    assert_eq!(h.comp.get_page_picked(), "appearance");

    // The same keys on the whole window: End is About, Home is Appearance, Enter opens.
    let f = fx();
    f.b().invoke_find_edited("".into());
    f.h.comp.invoke_settings_focus_rail(0);
    f.h.advance(0);
    f.h.key_text(&key_str(Key::End));
    f.h.advance(0);
    f.h.key_text(&key_str(Key::Return));
    f.h.advance(0);
    assert_eq!(
        f.ctl.borrow().current_page(),
        "about",
        "End then Enter opens About"
    );
    f.h.comp.invoke_settings_focus_rail(0);
    f.h.advance(0);
    f.h.key_text(&key_str(Key::Home));
    f.h.advance(0);
    f.h.key_text(&key_str(Key::Return));
    f.h.advance(0);
    assert_eq!(
        f.ctl.borrow().current_page(),
        "appearance",
        "Home then Enter opens Appearance"
    );
}

#[test]
fn tab_after_the_rail_enters_the_page_content() {
    let h = Harness::new(1.0, W, 604, || {
        FramePreview::new().expect("the preview builds")
    });
    h.advance(0);
    h.comp.set_focus_rail_index(5);
    h.advance(0);
    assert!(!h.comp.get_content_focused());
    h.key_text(&key_str(Key::Tab));
    assert!(
        h.comp.get_content_focused(),
        "the page follows the last rail item"
    );
}

// ---- the page -----------------------------------------------------------------------------------

#[test]
fn settings_opens_on_appearance_keeps_dictation_across_tab_switches_and_resets_on_reopen() {
    let f = fx();
    assert_eq!(
        f.ctl.borrow().current_page(),
        "appearance",
        "opens on Appearance"
    );
    f.page("dictation");
    f.ctl.borrow_mut().hide();
    f.ctl.borrow_mut().show();
    assert_eq!(
        f.ctl.borrow().current_page(),
        "dictation",
        "a tab switch keeps the page"
    );
    f.ctl.borrow_mut().on_settings_tab(false);
    f.ctl.borrow_mut().show();
    assert_eq!(
        f.ctl.borrow().current_page(),
        "appearance",
        "reopening resets the page"
    );
}

#[test]
fn the_page_shows_the_tables_and_nothing_else() {
    let f = fx();
    let mut drawn: Vec<String> = Vec::new();
    for p in PAGES.iter() {
        f.page(p.id);
        drawn.extend(drawn_rows(&f));
    }
    drawn.sort();
    drawn.dedup();
    // The models are drawn as model blocks, so the index's `dictation-model` wrapper is no Row; the
    // GPU row is drawn only with NVIDIA, which this PC is not.
    let mut want: Vec<String> = ROW_ORDER
        .iter()
        .filter(|id| !matches!(**id, "dictation-model" | "dictation-gpu"))
        .map(|id| id.to_string())
        .collect();
    want.sort();
    assert_eq!(
        drawn, want,
        "the drawn rows are the tables less the models' wrapper"
    );
    assert!(
        !drawn
            .iter()
            .any(|id| id == "confirm-close" || id == "tab-width")
    );
}

#[test]
fn each_core_section_is_drawn_once_in_one_panel_in_the_tables_order() {
    let f = fx();
    let entries = settings_index(&IndexEnv {
        nvidia: false,
        dictation_wired: true,
    });
    let mut seen_sections: Vec<String> = Vec::new();
    for p in PAGES.iter() {
        f.page(p.id);
        let sections: Vec<String> = f.sections().iter().map(|s| s.id.to_string()).collect();
        // The sections of this page, in the index's (table's) order, each once.
        let mut want: Vec<String> = Vec::new();
        for e in entries.iter().filter(|e| e.page == p.id) {
            if let Some(s) = &e.section_id
                && !want.contains(s)
            {
                want.push(s.clone());
            }
        }
        for s in &sections {
            assert!(
                !seen_sections.contains(s),
                "section {s} is drawn once overall"
            );
        }
        seen_sections.extend(sections.iter().cloned());
        // Rows inside the page follow the tables' order. The index's `dictation-model` is PT's one
        // wrapper round the model rows; Flare draws the models as their own blocks, not as a Row.
        let rows = drawn_rows(&f);
        let table: Vec<String> = entries
            .iter()
            .filter(|e| e.page == p.id && e.id != "dictation-model")
            .map(|e| e.id.clone())
            .collect();
        assert_eq!(rows, table, "{}: the rows are in the tables' order", p.id);
        assert_eq!(
            sections.iter().filter(|s| want.contains(s)).count(),
            want.len(),
            "{}: every table section is drawn",
            p.id
        );
    }
}

#[test]
fn no_gpu_row_without_nvidia() {
    let f = fx();
    let gpu = row("dictation-gpu").expect("the GPU row is defined");
    assert!(gpu.only_nvidia, "the GPU row is for NVIDIA only");
    assert!(!index_ids().iter().any(|id| id == "dictation-gpu"));
    for p in PAGES.iter() {
        f.page(p.id);
        assert!(!drawn_rows(&f).iter().any(|id| id == "dictation-gpu"));
    }
}

#[test]
fn help_off_removes_the_row_from_find_only_when_the_row_is_not_drawn() {
    let f = fx();
    f.b().invoke_row_switch("help-enabled".into(), false);
    assert!(!help_enabled(&f.prefs), "the switch stored Off");
    // The help row is drawn on its page (its switch is the row), so Find still finds it.
    f.type_query("command help");
    assert!(
        f.hit_ids().iter().any(|id| id == "help-enabled"),
        "a drawn row stays in Find"
    );
    f.type_query("");
    f.page("terminal");
    let r = f.row("help-enabled").expect("the help row is drawn");
    assert!(!r.on, "the row shows Off");
}
