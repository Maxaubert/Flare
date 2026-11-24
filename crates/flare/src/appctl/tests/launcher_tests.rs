//! The panel launcher and "Open in" through the real controller and window (2026-11-24, rules 1372 to 1376): where the
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

/// Rule 1372: the button shows on a shell tab whose panel is closed, and goes while the panel is open.
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

/// Rule 1372: at 100, 150 and 225 percent the button is 26 px round, 8 px from the top and 10 px from the right of the
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

/// Rule 1373, with the pointer: where the menu would be does nothing; the button opens it; the gap to the menu keeps it;
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

/// Rule 1373, the keys: a click opens the menu with the keys and its first row lit; the arrows move, Enter picks, and
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

/// Rule 1374: Browser opens the tab's web panel on an empty page with the address field holding the keys, and the
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

/// Rule 1374: Open a file... starts the Open dialog in the tab's folder; a picture picked there shows in the panel the
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

/// Rule 1375: Last panel appears once the tab's panel showed a file (or a page, or a device) and is closed, and brings
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

/// Rule 1376: a file's "Open in" hands it to its default app through the path openers (on a worker), with the words
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

/// Rule 1376: a web page's "Open in browser" sends the page's address to the default browser through `open_url`; an
/// empty page has no button.
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
    assert_eq!(f.h.comp.get_panel_open_in().as_str(), "Open in browser");
    f.h.comp.invoke_panel_open_in_clicked();
    assert_eq!(
        f.log.borrow().urls.last().map(String::as_str),
        Some("https://flare-test.local/index.html")
    );
}
