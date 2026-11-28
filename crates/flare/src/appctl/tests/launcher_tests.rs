//! The panel launcher and "Open in" through the real controller and window (2026-11-24, rules 1384 to 1388): where the
//! button shows, the hover rules with a real pointer (the menu opens only from the button, survives the gap, closes
//! 250 ms after the pointer left both), the keys, what each row opens, and where "Open in" sends a file or a page.

use flare_web::fake::FakeWeb;

use super::*;
use crate::openers::OpenedRecord;

/// The button's middle, in window coordinates.
fn button_mid(f: &Fx) -> (f32, f32) {
    let c = &f.h.comp;
    let w = c.get_launcher_button_w();
    (
        c.get_launcher_button_x() + w / 2.0,
        c.get_launcher_button_y() + w / 2.0,
    )
}

/// A point inside the menu's row `i` (as the controller lays the rows out), in window coordinates.
fn row_mid(f: &Fx, i: usize) -> (f32, f32) {
    let c = &f.h.comp;
    let rows = c.get_launcher_rows();
    let row = rows.row_data(i).expect("the row");
    let top = row.top + if row.rule { 7.0 } else { 0.0 };
    (
        c.get_launcher_menu_x() + c.get_launcher_menu_w() / 2.0,
        c.get_launcher_menu_y() + top + 14.0,
    )
}

fn labels(f: &Fx) -> Vec<String> {
    let rows = f.h.comp.get_launcher_rows();
    (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .map(|r| r.label.to_string())
        .collect()
}

fn index_of(f: &Fx, label: &str) -> usize {
    labels(f)
        .iter()
        .position(|l| l == label)
        .unwrap_or_else(|| panic!("no {label} row in {:?}", labels(f)))
}

/// Moves the pointer and lets the window's change handlers run.
fn point(f: &Fx, at: (f32, f32)) {
    f.h.move_to(at.0, at.1);
    f.h.advance(0);
}

/// Both clocks move on: the controller's and the window's timers.
fn wait(f: &Fx, ms: u64) {
    f.clock.set(f.clock.get() + ms);
    f.h.advance(ms);
}

fn open(f: &Fx) -> bool {
    f.h.comp.get_launcher_open()
}

/// Rule 1384: the button shows on a shell tab whose panel is closed, and goes while the panel is open.
#[test]
fn the_button_shows_while_the_tabs_panel_is_closed_and_goes_when_it_opens() {
    let (f, _h, _) = live(Opts::default());
    f.ctl.borrow_mut().refresh();
    assert!(f.h.comp.get_launcher_shown(), "a shell tab with no panel");
    assert!(!open(&f));
    show_text(&f, "a.txt", "hello");
    f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
    assert!(!f.h.comp.get_launcher_shown(), "the panel is open");
    f.h.comp.invoke_panel_close();
    f.h.advance(0);
    assert!(f.h.comp.get_launcher_shown(), "the panel closed again");
    // Settings in front: no button.
    f.ctl.borrow_mut().toggle_settings();
    assert!(!f.h.comp.get_launcher_shown());
}

/// Rule 1384 (review fix): the find bar sits in the same corner, so the button and its menu go while it is open and
/// come back when it closes.
#[test]
fn the_button_goes_while_the_find_bar_is_open() {
    let (f, _h, _) = live(Opts::default());
    f.ctl.borrow_mut().refresh();
    point(&f, button_mid(&f));
    assert!(open(&f));
    f.ctl.borrow_mut().toggle_find();
    f.h.advance(0);
    assert!(f.h.comp.get_find_open());
    assert!(
        !f.h.comp.get_launcher_shown(),
        "the find bar covers the corner"
    );
    assert!(!open(&f), "the menu goes with the button");
    f.ctl.borrow_mut().toggle_find();
    f.h.advance(0);
    assert!(!f.h.comp.get_find_open());
    assert!(
        f.h.comp.get_launcher_shown(),
        "back when the find bar closes"
    );
}

/// Rule 1384: at 100, 150 and 225 percent the button is 26 px round, 8 px from the top and 10 px from the right of the
/// terminal, it draws, and its menu hangs 6 px under it with their right edges met, inside the window.
#[test]
fn the_button_and_its_menu_sit_in_the_corner_at_every_scale() {
    for scale in [1.0_f32, 1.5, 2.25] {
        let (f, _h, _) = live(Opts {
            scale,
            ..Opts::default()
        });
        f.ctl.borrow_mut().refresh();
        f.h.advance(0);
        let c = &f.h.comp;
        let px = 1.01 / scale;
        let (bx, by, bw) = (
            c.get_launcher_button_x(),
            c.get_launcher_button_y(),
            c.get_launcher_button_w(),
        );
        assert!((bw - 26.0).abs() <= px, "{scale}: {bw}");
        assert!((by - c.get_grid_top() - 8.0).abs() <= px, "{scale}: {by}");
        let grid_right = c.get_grid_left() + c.get_grid_phys_width() as f32 / scale;
        assert!(
            (grid_right - (bx + bw) - 10.0).abs() <= 2.0 * px,
            "{scale}: right edge {} of {grid_right}",
            bx + bw
        );
        // Whole device pixels.
        assert!(
            ((bx * scale) - (bx * scale).round()).abs() < 0.01,
            "{scale}: x {bx}"
        );
        // It draws: the icon's ink differs from the ground beside the button.
        let frame = f.h.render();
        let (x0, y0) = ((bx * scale) as u32, (by * scale) as u32);
        let side = (bw * scale) as u32;
        let ground = frame.px(x0.saturating_sub(side * 2), y0 + side / 2);
        let ink = frame.count_where(x0, y0, x0 + side, y0 + side, |p| !Frame::near(p, ground, 6));
        assert!(ink > 0, "{scale}: the button draws");
        // The menu: under it, right edges met, inside the window.
        c.invoke_launcher_clicked();
        f.h.advance(0);
        let (mx, my, mw, mh) = (
            c.get_launcher_menu_x(),
            c.get_launcher_menu_y(),
            c.get_launcher_menu_w(),
            c.get_launcher_menu_hh(),
        );
        assert!((my - (by + bw) - 6.0).abs() <= px, "{scale}: menu top {my}");
        assert!(((mx + mw) - (bx + bw)).abs() <= px, "{scale}: right edges");
        assert!(mx > c.get_grid_left() && mw > 80.0, "{scale}: {mx} {mw}");
        assert!(my + mh < c.get_grid_top() + 640.0, "{scale}: {my} {mh}");
    }
}

use crate::ui_harness::Frame;

/// Rule 1385, with the pointer: where the menu would be does nothing; the button opens it; the gap to the menu keeps it;
/// off both for 250 ms closes it, and back within that time does not.
#[test]
fn the_menu_opens_only_from_the_button_and_survives_the_gap() {
    let (f, _h, _) = live(Opts::default());
    f.ctl.borrow_mut().refresh();
    let (bx, by) = button_mid(&f);
    // Where the menu would hang (below and left of the button): nothing opens.
    point(&f, (bx - 40.0, by + 60.0));
    assert!(!open(&f), "a closed menu takes no pointer");
    // Onto the button.
    point(&f, (bx, by));
    assert!(open(&f), "the button opens it");
    assert!(f.h.comp.get_launcher_lit());
    // Down across the 6 px gap (the bridge) into the first row.
    let gap_y = f.h.comp.get_launcher_menu_y() - 3.0;
    point(&f, (bx, gap_y));
    wait(&f, 400);
    assert!(open(&f), "the gap belongs to the menu");
    point(&f, row_mid(&f, 0));
    wait(&f, 400);
    assert!(open(&f));
    assert_eq!(
        f.h.comp.get_launcher_hot(),
        0,
        "the row under the pointer is lit"
    );
    // Off both: still open within the grace, back in time keeps it.
    point(&f, (bx - 300.0, by + 300.0));
    wait(&f, 200);
    assert!(open(&f), "200 ms is inside the grace");
    point(&f, row_mid(&f, 0));
    wait(&f, 400);
    assert!(open(&f), "back in time");
    // Off both for the whole grace: it closes.
    point(&f, (bx - 300.0, by + 300.0));
    wait(&f, 260);
    assert!(!open(&f), "250 ms off both closes it");
    // And the place the menu was takes no pointer again.
    point(&f, row_mid(&f, 0));
    wait(&f, 10);
    assert!(!open(&f));
}

/// Rule 1385, the keys: a click opens the menu with the keys and its first row lit; the arrows move, Enter picks, and
/// Escape is the menu's. A menu the pointer opened takes no key: its Escape still reaches the shell.
#[test]
fn a_clicked_menu_holds_its_keys_and_a_hovered_one_does_not() {
    let (f, _h, _) = live(Opts::default());
    f.ctl.borrow_mut().refresh();
    f.h.comp.invoke_launcher_clicked();
    assert!(open(&f));
    assert_eq!(f.h.comp.get_launcher_hot(), 0);
    let n = labels(&f).len() as i32;
    assert_eq!(
        f.key(KeyEvent::new("ArrowDown", "ArrowDown")),
        KeyOutcome::Consumed
    );
    assert_eq!(f.h.comp.get_launcher_hot(), 1.min(n - 1));
    assert_eq!(
        f.key(KeyEvent::new("ArrowUp", "ArrowUp")),
        KeyOutcome::Consumed
    );
    assert_eq!(
        f.key(KeyEvent::new("ArrowUp", "ArrowUp")),
        KeyOutcome::Consumed
    );
    assert_eq!(f.h.comp.get_launcher_hot(), n - 1, "the arrows wrap");
    assert_eq!(
        f.key(KeyEvent::new("Escape", "Escape")),
        KeyOutcome::Consumed
    );
    assert!(!open(&f));
    // Opened by the pointer: Escape closes it and goes on to the shell; an arrow is the shell's.
    point(&f, button_mid(&f));
    assert!(open(&f));
    assert_ne!(
        f.key(KeyEvent::new("ArrowDown", "ArrowDown")),
        KeyOutcome::Consumed
    );
    assert!(open(&f));
    assert_ne!(
        f.key(KeyEvent::new("Escape", "Escape")),
        KeyOutcome::Consumed
    );
    assert!(!open(&f));
}

/// Rule 1386: Browser opens the tab's web panel on an empty page with the address field holding the keys, and the
/// button goes because the panel is open.
#[test]
fn browser_opens_the_web_panel_in_its_new_tab_state() {
    let (f, _h, _) = live(Opts::default());
    let slot: Rc<RefCell<Option<FakeWeb>>> = Rc::default();
    let keep = slot.clone();
    f.ctl.borrow_mut().set_web_factory(Box::new(move |sink| {
        let fake = FakeWeb::new(sink);
        *keep.borrow_mut() = Some(fake.clone());
        Box::new(fake)
    }));
    f.ctl.borrow_mut().refresh();
    let id = f.ctl.borrow().front_id().unwrap();
    point(&f, button_mid(&f));
    let i = index_of(&f, "Browser");
    point(&f, row_mid(&f, i));
    let (x, y) = row_mid(&f, i);
    f.h.click(x, y);
    f.h.advance(0);
    f.pump();
    let snap = f.ctl.borrow().panels.snap(&id).expect("a panel");
    assert!(snap.open);
    assert_eq!(snap.url.as_deref(), Some("about:blank"));
    assert!(!open(&f), "a pick closes the menu");
    assert!(!f.h.comp.get_launcher_shown(), "the panel is open");
    assert!(
        f.h.comp.get_panel_web_input_focused(),
        "the address field holds the keys"
    );
}

/// Rule 1386: a page the tab's panel hid is not shown again by Browser: the view goes to the empty page.
#[test]
fn browser_after_a_hidden_page_goes_to_an_empty_page() {
    let (f, _h, _) = live(Opts::default());
    let slot: Rc<RefCell<Option<FakeWeb>>> = Rc::default();
    let keep = slot.clone();
    f.ctl.borrow_mut().set_web_factory(Box::new(move |sink| {
        let fake = FakeWeb::new(sink);
        *keep.borrow_mut() = Some(fake.clone());
        Box::new(fake)
    }));
    let id = f.ctl.borrow().front_id().unwrap();
    let page = "https://flare-test.local/index.html";
    assert!(f.ctl.borrow_mut().browser_open(&id, page).is_ok());
    f.pump();
    f.h.comp.invoke_panel_close();
    f.h.advance(0);
    f.pump();
    assert!(f.h.comp.get_launcher_shown());
    f.h.comp.invoke_launcher_clicked();
    let i = index_of(&f, "Browser");
    f.h.comp.invoke_launcher_picked(i as i32);
    f.pump();
    let snap = f.ctl.borrow().panels.snap(&id).unwrap();
    assert!(snap.open);
    assert_eq!(snap.url.as_deref(), Some("about:blank"));
    let fake = slot.borrow().clone().expect("the browser was made");
    let calls = fake.calls();
    let last_nav = calls.iter().rev().find_map(|c| match c {
        flare_web::fake::Call::Open { url, .. } | flare_web::fake::Call::Navigate(_, url) => {
            Some(url.clone())
        }
        _ => None,
    });
    assert_eq!(last_nav.as_deref(), Some("about:blank"), "{calls:?}");
}

/// Rule 1386: Open a file... starts the Open dialog in the tab's folder; a picture picked there shows in the panel the
/// way a file link's "Show in side panel" shows it; a file the panel cannot show is said in the panel; a cancel does
/// nothing.
#[test]
fn open_a_file_shows_the_picked_file_and_says_so_when_it_cannot() {
    let (f, _h, _) = live(Opts::default());
    let pic = f.dir.path().join("p.png");
    write_png(&pic, 4, 3);
    let zip = f.dir.path().join("x.zip");
    std::fs::write(&zip, b"PK").unwrap(); // sync-ok: test code only
    let answer: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(Some(pic.clone())));
    let started: Rc<RefCell<Vec<Option<PathBuf>>>> = Rc::default();
    {
        let (answer, started) = (answer.clone(), started.clone());
        f.ctl.borrow_mut().set_pick_file(Rc::new(move |dir, done| {
            started.borrow_mut().push(dir);
            done(answer.borrow().clone());
        }));
    }
    f.ctl.borrow_mut().refresh();
    let id = f.ctl.borrow().front_id().unwrap();
    f.h.comp.invoke_launcher_clicked();
    let i = index_of(&f, "Open a file\u{2026}");
    f.h.comp.invoke_launcher_picked(i as i32);
    assert_eq!(
        started.borrow().as_slice(),
        &[Some(PathBuf::from("C:\\a"))],
        "the dialog starts in the tab's folder"
    );
    f.pump_until("the picture", |f| f.h.comp.get_panel_kind() == 1);
    let snap = f.ctl.borrow().panels.snap(&id).unwrap();
    assert!(snap.open);
    assert_eq!(snap.kind, "image");
    // A zip: the panel says it cannot show it, and nothing opens outside Flare.
    f.h.comp.invoke_panel_close();
    f.h.advance(0);
    *answer.borrow_mut() = Some(zip);
    f.h.comp.invoke_launcher_clicked();
    let i = index_of(&f, "Open a file\u{2026}");
    f.h.comp.invoke_launcher_picked(i as i32);
    f.pump_until("the note", |f| {
        f.h.comp.get_panel_open() && f.h.comp.get_panel_title().as_str() == "Cannot show this file"
    });
    assert!(
        f.openers.records().is_empty(),
        "nothing opened outside Flare"
    );
    // A cancel: nothing changes.
    f.h.comp.invoke_panel_close();
    f.h.advance(0);
    *answer.borrow_mut() = None;
    f.h.comp.invoke_launcher_clicked();
    let i = index_of(&f, "Open a file\u{2026}");
    f.h.comp.invoke_launcher_picked(i as i32);
    for _ in 0..5 {
        f.pump();
    }
    assert!(!f.h.comp.get_panel_open());
}

/// Rule 1387: Last panel appears once the tab's panel showed a file (or a page, or a device) and is closed, and brings
/// that back. A note handed over is not something to bring back.
#[test]
fn last_panel_brings_back_what_the_tab_showed_last() {
    let (f, _h, _) = live(Opts::default());
    f.ctl.borrow_mut().refresh();
    assert!(
        !labels(&f).contains(&"Last panel".to_string()),
        "nothing shown yet: {:?}",
        labels(&f)
    );
    let abs = show_text(&f, "notes.txt", "one\ntwo\n");
    f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
    f.h.comp.invoke_panel_close();
    f.h.advance(0);
    let i = index_of(&f, "Last panel");
    assert_eq!(i, labels(&f).len() - 1, "Last panel is the last row");
    let rows = f.h.comp.get_launcher_rows();
    assert!(rows.row_data(i).unwrap().rule, "a rule sits above it");
    f.h.comp.invoke_launcher_clicked();
    f.h.comp.invoke_launcher_picked(i as i32);
    f.pump_until("the file again", |f| f.h.comp.get_panel_kind() == 2);
    let id = f.ctl.borrow().front_id().unwrap();
    let snap = f.ctl.borrow().panels.snap(&id).unwrap();
    assert!(snap.open);
    assert_eq!(snap.path.as_deref(), Some(abs.as_str()));
}

/// Rule 1388: a file's "Open in" hands it to its default app through the path openers (on a worker), with the words
/// for a default app no one named; the device panel has no button.
#[test]
fn open_in_hands_a_file_to_its_default_app() {
    let (f, _h, _) = live(Opts::default());
    let abs = show_text(&f, "readme.txt", "hi");
    f.pump_until("the panel", |f| f.h.comp.get_panel_kind() == 2);
    assert_eq!(f.h.comp.get_panel_open_in().as_str(), "Open in default app");
    assert_eq!(f.h.comp.get_panel_open_in_short().as_str(), "Open");
    assert!(f.h.comp.get_panel_open_in_visible());
    f.h.comp.invoke_panel_open_in_clicked();
    f.pump_until("the open", |f| !f.openers.records().is_empty());
    let want = std::fs::canonicalize(&abs) // sync-ok: test code only
        .map(|p| p.to_string_lossy().trim_start_matches(r"\\?\").to_string())
        .unwrap_or(abs.clone());
    match f.openers.records().as_slice() {
        [OpenedRecord::Open(p)] => assert!(
            p.eq_ignore_ascii_case(&abs) || p.eq_ignore_ascii_case(&want),
            "{p}"
        ),
        other => panic!("{other:?}"),
    }
}

/// Rule 1388: the header gives "Open in" only the room the title can spare: full words when they fit, the short word
/// when only that fits, none at all at the narrowest slot. A markdown file (its view control in the header) with a long
/// app name, at the default slot and at the smallest.
#[test]
fn open_in_takes_only_the_room_the_title_can_spare() {
    let (f, _h, _) = live(Opts::default());
    let long = format!("{}\u{2026}", "A".repeat(31));
    f.ctl
        .borrow_mut()
        .set_app_name_for_test("md", Some(long.as_str()));
    show_text(&f, "notes.md", "# Notes\n");
    f.pump_until("the panel", |f| f.h.comp.get_panel_open());
    let id = f.ctl.borrow().front_id().unwrap();
    assert_eq!(
        f.h.comp.get_panel_open_in().as_str(),
        format!("Open in {long}")
    );
    for share in [0.4_f32, 0.3, 0.24] {
        f.ctl.borrow_mut().panels.set_share(&id, share);
        f.ctl.borrow_mut().refresh();
        f.h.advance(0);
        let c = &f.h.comp;
        let slot_left = c.get_page_left() + c.get_panel_slot_x();
        let slot_w = c.get_panel_visible_w();
        if c.get_panel_open_in_visible() {
            let (x, w) = (c.get_panel_open_in_x(), c.get_panel_open_in_w());
            assert!(
                x >= slot_left + 12.0 + 80.0 - 1.0,
                "share {share}: the title keeps 80 px ({x} in a slot at {slot_left}, {slot_w} wide)"
            );
            assert!(
                x + w <= slot_left + slot_w,
                "share {share}: inside the slot"
            );
        }
    }
    // The smallest slot (240 px) with a view control: no room for the button.
    assert!(!f.h.comp.get_panel_open_in_visible());
}

// ---- the web bar's More menu (rule 1389) ----

fn menu_labels(f: &Fx) -> Vec<String> {
    let rows = f.h.comp.get_menu_rows();
    (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .map(|r| r.label.to_string())
        .collect()
}

/// The More button's middle, in window coordinates.
fn more_mid(f: &Fx) -> (f32, f32) {
    let c = &f.h.comp;
    let w = c.get_panel_web_more_w();
    (
        c.get_panel_web_more_x() + w / 2.0,
        c.get_panel_web_more_y() + w / 2.0,
    )
}

/// A real click on the web bar's More button; its menu's rows.
pub(super) fn open_more(f: &Fx) -> Vec<String> {
    assert!(
        f.h.comp.get_panel_web_more_visible(),
        "the More button shows"
    );
    let (x, y) = more_mid(f);
    f.h.click(x, y);
    f.h.advance(0);
    assert!(f.h.comp.get_menu_open(), "its menu opened");
    menu_labels(f)
}

/// A real click on the open menu's row with these words (rows are 28 px with a hairline between).
pub(super) fn pick_more(f: &Fx, label: &str) {
    let labels = menu_labels(f);
    let i = labels
        .iter()
        .position(|l| l == label)
        .unwrap_or_else(|| panic!("no {label} row in {labels:?}"));
    let c = &f.h.comp;
    let y = c.get_menu_panel_y() + 1.0 + i as f32 * 29.0 + 14.0;
    f.h.click(c.get_menu_panel_x() + c.get_menu_w() / 2.0, y);
    f.h.advance(0);
    f.pump();
}

/// A web page up in the front tab's panel through a stand-in browser.
fn web_page(f: &Fx, url: &str) -> String {
    f.ctl
        .borrow_mut()
        .set_web_factory(Box::new(move |sink| Box::new(FakeWeb::new(sink))));
    let id = f.ctl.borrow().front_id().unwrap();
    assert!(f.ctl.borrow_mut().browser_open(&id, url).is_ok());
    f.pump();
    f.h.advance(0);
    id
}

/// Rule 1389: the web bar's More button drops the window's menu under itself, right edges together, pressed while it
/// is open and never lighting the sidebar's New tab arrow; "Open in browser" there sends the page to the default
/// browser (rule 1388) and closes the menu. An empty page with no agent has no row, so no button.
#[test]
fn the_more_menu_drops_under_its_button_and_opens_the_page_in_the_browser() {
    let (f, _h, _) = live(Opts::default());
    let _ = web_page(&f, "about:blank");
    assert!(!f.h.comp.get_panel_web_more_visible(), "nothing to offer");
    let url = "https://flare-test.local/index.html";
    let id = f.ctl.borrow().front_id().unwrap();
    assert!(f.ctl.borrow_mut().browser_open(&id, url).is_ok());
    f.pump();
    f.h.advance(0);
    assert_eq!(
        open_more(&f),
        ["Open in browser"],
        "no agent: no Draw arrow"
    );
    let c = &f.h.comp;
    let (bx, by, bw) = (
        c.get_panel_web_more_x(),
        c.get_panel_web_more_y(),
        c.get_panel_web_more_w(),
    );
    assert!(c.get_menu_drop() && c.get_menu_below());
    assert!(
        c.get_menu_panel_y() >= by + bw,
        "under the button: {} {by} {bw}",
        c.get_menu_panel_y()
    );
    assert!(
        (c.get_menu_panel_x() + c.get_menu_w() - (bx + bw)).abs() <= 1.0,
        "right edges together"
    );
    assert!(c.get_panel_web_more_open(), "pressed while open");
    assert!(c.get_overlay_open(), "the page hides under the menu");
    pick_more(&f, "Open in browser");
    assert_eq!(f.log.borrow().urls.last().map(String::as_str), Some(url));
    assert!(!f.h.comp.get_menu_open());
    assert!(!f.h.comp.get_panel_web_more_open());
}

/// Rule 1389: the menu takes the keys the other Flare menus take: Escape closes it and does nothing else.
#[test]
fn escape_closes_the_more_menu_and_opens_nothing() {
    let (f, _h, _) = live(Opts::default());
    let _ = web_page(&f, "https://flare-test.local/index.html");
    open_more(&f);
    f.h.key_text(slint::SharedString::from(slint::platform::Key::Escape).as_str());
    f.h.advance(0);
    f.pump();
    assert!(!f.h.comp.get_menu_open());
    assert!(!f.h.comp.get_panel_web_more_open());
    assert!(f.log.borrow().urls.is_empty(), "nothing was opened");
}

/// Rule 1389: the More button is icon-only, so it has a tooltip, "More", on its left over the bar (below it is the
/// page's own window, which would hide it).
#[test]
fn the_more_button_has_a_tooltip_left_of_it() {
    let (f, _h, _) = live(Opts::default());
    let _ = web_page(&f, "https://flare-test.local/index.html");
    let (x, y) = more_mid(&f);
    f.h.move_to(x, y);
    f.h.advance(0);
    f.clock.set(f.clock.get() + 1001);
    f.ctl.borrow_mut().tick();
    f.h.advance(0);
    let c = &f.h.comp;
    assert!(c.get_tip_visible());
    assert_eq!(c.get_tip_text().as_str(), "More");
    assert_eq!(c.get_tip_side(), 3, "on the left");
    assert!(c.get_tip_x() + c.get_tip_w() <= c.get_panel_web_more_x());
    // A click opens the menu and takes the tip away.
    f.h.click(x, y);
    f.h.advance(0);
    assert!(f.h.comp.get_menu_open());
    assert!(!f.h.comp.get_tip_visible());
}

/// Rule 1388: the label follows the page the person went to, not only one an agent opened.
#[test]
fn open_in_follows_a_page_the_browser_went_to() {
    let (f, _h, _) = live(Opts::default());
    let slot: Rc<RefCell<Option<FakeWeb>>> = Rc::default();
    let keep = slot.clone();
    f.ctl.borrow_mut().set_web_factory(Box::new(move |sink| {
        let fake = FakeWeb::new(sink);
        *keep.borrow_mut() = Some(fake.clone());
        Box::new(fake)
    }));
    let id = f.ctl.borrow().front_id().unwrap();
    assert!(f.ctl.borrow_mut().browser_open(&id, "about:blank").is_ok());
    f.pump();
    assert_eq!(f.h.comp.get_panel_open_in().as_str(), "");
    let fake = slot.borrow().clone().expect("the browser was made");
    let url = "https://flare-test.local/two.html";
    fake.say(flare_web::WebEvent::Loading {
        tab: id.clone(),
        url: url.into(),
        nav: 1,
    });
    fake.say(flare_web::WebEvent::Loaded {
        tab: id.clone(),
        url: url.into(),
        ok: true,
        status: "0".into(),
        nav: 1,
    });
    f.pump_until("the label", |f| {
        f.h.comp.get_panel_open_in().as_str() == "Open in browser"
    });
    f.h.advance(0);
    assert_eq!(open_more(&f), ["Open in browser"]);
    pick_more(&f, "Open in browser");
    assert_eq!(f.log.borrow().urls.last().map(String::as_str), Some(url));
}

/// Rule 1388: a web page's "Open in browser" (a row of the bar's More menu, rule 1389) sends the page's address to the
/// default browser through `open_url`; an empty page has no row.
#[test]
fn open_in_browser_sends_the_page_address() {
    let (f, _h, _) = live(Opts::default());
    f.ctl
        .borrow_mut()
        .set_web_factory(Box::new(move |sink| Box::new(FakeWeb::new(sink))));
    let id = f.ctl.borrow().front_id().unwrap();
    assert!(f.ctl.borrow_mut().browser_open(&id, "about:blank").is_ok());
    f.pump();
    assert_eq!(f.h.comp.get_panel_open_in().as_str(), "", "an empty page");
    assert!(
        f.ctl
            .borrow_mut()
            .browser_open(&id, "https://flare-test.local/index.html")
            .is_ok()
    );
    f.pump();
    f.h.advance(0);
    assert_eq!(f.h.comp.get_panel_open_in().as_str(), "Open in browser");
    assert_eq!(open_more(&f), ["Open in browser"]);
    pick_more(&f, "Open in browser");
    assert_eq!(
        f.log.borrow().urls.last().map(String::as_str),
        Some("https://flare-test.local/index.html")
    );
}
