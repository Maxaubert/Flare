//! Selecting text in the Text panel (2026-11-10) inside the window controller: the pointer through the window's own
//! callbacks, the keys through `on_key`, the copy into the in-memory clipboard and "Ask agent" into a fake shell.

use flare_surfaces::mdview::MdView;
use flare_web::fake::{Call, FakeWeb};

use super::*;

/// A text file in the front tab's panel, once its rows are on show.
fn open(f: &Fx, name: &str, body: &str) -> String {
    let abs = show_text(f, name, body);
    f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
    abs
}

/// The point of the text's content over column `col` of row `row` (the coordinates the window reports a press in).
fn at(f: &Fx, row: usize, col: f32) -> (f32, f32) {
    let a = &f.h.comp;
    (
        a.get_panel_gutter_w() + col * a.get_panel_text_char_w(),
        (row as f32 + 0.5) * a.get_panel_text_row_h(),
    )
}

fn down(f: &Fx, row: usize, col: f32, shift: bool) {
    let (x, y) = at(f, row, col);
    f.h.comp.invoke_panel_text_down(x, y, shift);
}

fn drag(f: &Fx, row: usize, col: f32) {
    let (x, y) = at(f, row, col);
    f.h.comp.invoke_panel_text_move(x, y);
}

fn up(f: &Fx) {
    f.h.comp.invoke_panel_text_up();
}

fn selected(f: &Fx) -> Option<String> {
    f.ctl.borrow().panels.selection_text()
}

/// The highlight the window was told to draw: (from row, column) to (row, column), or None.
fn painted(f: &Fx) -> Option<((i32, i32), (i32, i32))> {
    let a = &f.h.comp;
    a.get_panel_sel_on().then(|| {
        (
            (a.get_panel_sel_a_row(), a.get_panel_sel_a_col()),
            (a.get_panel_sel_b_row(), a.get_panel_sel_b_col()),
        )
    })
}

/// The panel takes the keys, as a click on it does.
fn focus(f: &Fx) {
    f.h.activate();
    f.h.comp.invoke_focus_panel();
    f.h.advance(0);
    assert!(f.h.comp.get_panel_has_focus());
}

fn escape(f: &Fx) {
    f.h.key_text(&String::from(char::from(slint::platform::Key::Escape)));
    f.h.advance(0);
}

/// Clicks spread far enough apart that none counts as a second click of another.
fn pause() {
    std::thread::sleep(Duration::from_millis(600)); // sync-ok: test code only
}

#[test]
fn a_drag_selects_the_shown_text_and_ctrl_c_copies_it_with_tabs_as_spaces() {
    let (f, _h, _) = live(Opts::default());
    open(&f, "a.txt", "one\ttwo\nthree four\nfive");
    down(&f, 0, 0.0, false);
    assert_eq!(selected(&f), None, "a press is a caret");
    drag(&f, 1, 5.0);
    up(&f);
    assert_eq!(selected(&f).as_deref(), Some("one    two\nthree"));
    assert_eq!(painted(&f), Some(((0, 0), (1, 5))));
    focus(&f);
    assert_eq!(
        f.key(KeyEvent::new("c", "KeyC").ctrl()),
        KeyOutcome::Consumed
    );
    f.pump_until("the copy", |f| f.clip.read_text().is_some());
    assert_eq!(f.clip.read_text().as_deref(), Some("one    two\nthree"));
    assert!(selected(&f).is_some(), "a copy keeps the selection");
}

#[test]
fn ctrl_c_in_the_panel_is_a_copy_even_where_the_agent_guard_would_take_it() {
    let (f, _h, shell) = live(Opts::default());
    let cwd = f.cwds()[0].clone();
    let id = f.ctl.borrow().front_id().unwrap();
    f.say(&cwd, TITLE_IDLE);
    f.pump_until("the agent", |f| f.agent_here(&id));
    open(&f, "a.txt", "alpha beta");
    down(&f, 0, 0.0, false);
    drag(&f, 0, 5.0);
    up(&f);
    focus(&f);
    assert_eq!(
        f.key(KeyEvent::new("c", "KeyC").ctrl()),
        KeyOutcome::Consumed
    );
    f.pump_until("the copy", |f| f.clip.read_text().is_some());
    assert_eq!(f.clip.read_text().as_deref(), Some("alpha"));
    assert!(shell.written().is_empty(), "the shell heard nothing");
}

#[test]
fn a_double_click_picks_a_word_a_triple_click_a_line_and_shift_click_extends() {
    let (f, _h, _) = live(Opts::default());
    open(&f, "a.txt", "let (alpha, beta) = 1;\nsecond line\nthird");
    down(&f, 0, 7.2, false);
    up(&f);
    down(&f, 0, 7.2, false);
    up(&f);
    assert_eq!(selected(&f).as_deref(), Some("alpha"));
    down(&f, 0, 7.2, false);
    up(&f);
    assert_eq!(selected(&f).as_deref(), Some("let (alpha, beta) = 1;\n"));
    pause();
    down(&f, 1, 3.0, false);
    up(&f);
    assert_eq!(selected(&f), None);
    down(&f, 2, 2.0, true);
    up(&f);
    assert_eq!(selected(&f).as_deref(), Some("ond line\nth"));
}

#[test]
fn an_empty_file_takes_clicks_and_shift_keys_without_a_crash_or_a_selection() {
    let (f, _h, _) = live(Opts::default());
    open(&f, "empty.txt", "");
    for _ in 0..3 {
        down(&f, 0, 2.0, false);
        drag(&f, 1, 4.0);
        up(&f);
    }
    assert_eq!(selected(&f), None);
    focus(&f);
    for k in [
        "End",
        "ArrowRight",
        "ArrowLeft",
        "ArrowDown",
        "ArrowUp",
        "Home",
    ] {
        assert_eq!(
            f.key(KeyEvent::new(k, k).shift()),
            KeyOutcome::Consumed,
            "{k}"
        );
    }
    assert_eq!(selected(&f), None);
    assert_eq!(painted(&f), None);
}

#[test]
fn esc_lets_the_selection_go_first_and_gives_the_keys_back_only_with_nothing_selected() {
    let (f, _h, _) = live(Opts::default());
    open(&f, "a.txt", "alpha beta\ngamma");
    focus(&f);
    down(&f, 0, 0.0, false);
    drag(&f, 1, 2.0);
    up(&f);
    assert!(selected(&f).is_some());
    escape(&f);
    assert_eq!(selected(&f), None);
    assert_eq!(painted(&f), None);
    assert!(f.h.comp.get_panel_has_focus(), "the panel keeps the keys");
    assert!(f.h.comp.get_panel_open(), "and stays open");
    escape(&f);
    assert!(!f.h.comp.get_panel_has_focus(), "now the terminal has them");
    assert!(f.h.comp.get_panel_open());
}

#[test]
fn shift_with_the_arrows_and_ctrl_a_select_from_the_keyboard() {
    let (f, _h, _) = live(Opts::default());
    open(&f, "a.txt", "abc\ndefgh\nij");
    focus(&f);
    down(&f, 0, 1.0, false);
    up(&f);
    let shift = |k: &str| KeyEvent::new(k, k).shift();
    assert_eq!(f.key(shift("ArrowRight")), KeyOutcome::Consumed);
    assert_eq!(selected(&f).as_deref(), Some("b"));
    assert_eq!(f.key(shift("ArrowDown")), KeyOutcome::Consumed);
    assert_eq!(selected(&f).as_deref(), Some("bc\nde"));
    assert_eq!(f.key(shift("End")), KeyOutcome::Consumed);
    assert_eq!(selected(&f).as_deref(), Some("bc\ndefgh"));
    assert_eq!(
        f.key(KeyEvent::new("End", "End").shift().ctrl()),
        KeyOutcome::Consumed
    );
    assert_eq!(selected(&f).as_deref(), Some("bc\ndefgh\nij"));
    // An arrow without Shift still scrolls: it is the panel's, not the selection's.
    assert_eq!(
        f.key(KeyEvent::new("ArrowDown", "ArrowDown")),
        KeyOutcome::Slint
    );
    assert_eq!(
        f.key(KeyEvent::new("a", "KeyA").ctrl()),
        KeyOutcome::Consumed
    );
    assert_eq!(selected(&f).as_deref(), Some("abc\ndefgh\nij"));
}

#[test]
fn the_selection_goes_with_a_reload_another_file_or_another_tab() {
    let (f, _h, _) = live(Opts::default());
    let select_all = |f: &Fx| {
        let app = f.ctl.borrow().app().unwrap();
        assert!(f.ctl.borrow_mut().panels.select_all(&app));
        assert!(painted(f).is_some());
    };
    open(&f, "a.txt", "one\ntwo");
    select_all(&f);
    // The same file clicked again is read afresh.
    show_text(&f, "a.txt", "one\ntwo");
    f.pump();
    assert_eq!(selected(&f), None, "a reload");
    assert_eq!(painted(&f), None);
    f.pump_until("the text again", |f| f.h.comp.get_panel_kind() == 2);
    select_all(&f);
    open(&f, "b.txt", "other");
    assert_eq!(selected(&f), None, "another file");
    select_all(&f);
    let _b = f.ctl.borrow_mut().open_folder("C:\\b");
    f.ctl.borrow_mut().refresh();
    f.pump();
    assert_eq!(selected(&f), None, "another tab");
    assert_eq!(painted(&f), None);
}

#[test]
fn turning_the_wrap_on_and_off_keeps_the_selection_on_the_same_text() {
    let (f, _h, _) = live(Opts::default());
    let long = "abcdefghij".repeat(20);
    open(&f, "a.txt", &format!("{long}\nxyz"));
    let cols = usize::try_from(f.h.comp.get_panel_wrapped_cols()).unwrap();
    assert!(cols > 8 && cols < 200, "wrapped at {cols}");
    let rows = 200usize.div_ceil(cols);
    // From the second row of the long line to the third character of the next line.
    down(&f, 1, 2.0, false);
    drag(&f, rows, 3.0);
    up(&f);
    let want = format!("{}\nxyz", &long[cols + 2..]);
    assert_eq!(selected(&f).as_deref(), Some(want.as_str()));
    let rows_i = i32::try_from(rows).unwrap();
    assert_eq!(painted(&f), Some(((1, 2), (rows_i, 3))));
    f.h.comp.invoke_panel_wrap_toggled();
    f.pump();
    assert!(!f.h.comp.get_panel_wrapped());
    assert_eq!(selected(&f).as_deref(), Some(want.as_str()));
    let start = i32::try_from(cols + 2).unwrap();
    assert_eq!(painted(&f), Some(((0, start), (1, 3))));
    f.h.comp.invoke_panel_wrap_toggled();
    f.pump();
    assert!(f.h.comp.get_panel_wrapped());
    assert_eq!(selected(&f).as_deref(), Some(want.as_str()));
    assert_eq!(painted(&f), Some(((1, 2), (rows_i, 3))));
}

#[test]
fn a_drag_held_below_the_view_scrolls_and_the_selection_follows() {
    let (f, _h, _) = live(Opts::default());
    let body: Vec<String> = (0..300).map(|i| format!("line {i}")).collect();
    open(&f, "long.txt", &body.join("\n"));
    let in_view = (f.h.comp.get_panel_text_view_h() / f.h.comp.get_panel_text_row_h()) as usize;
    down(&f, 0, 0.0, false);
    // Below the bottom edge by 30 px, the view not scrolled yet.
    let (x, _) = at(&f, 0, 2.0);
    let y = f.h.comp.get_panel_text_view_h() + 30.0;
    f.h.comp.invoke_panel_text_move(x, y);
    assert!(crate::timer_registry::running("panel.sel-scroll"));
    let before = selected(&f).unwrap().lines().count();
    assert!(before <= in_view + 4, "{before} lines, {in_view} in view");
    for _ in 0..5 {
        assert!(f.ctl.borrow_mut().panel_drag_tick());
    }
    assert!(
        f.h.comp.get_panel_text_scroll_y() < 0.0,
        "the view scrolled"
    );
    let after = selected(&f).unwrap().lines().count();
    assert!(
        after > before + 10,
        "the end followed: {before} then {after}"
    );
    up(&f);
    assert!(!crate::timer_registry::running("panel.sel-scroll"));
    assert!(
        !f.ctl.borrow_mut().panel_drag_tick(),
        "nothing scrolls after the release"
    );
    assert_eq!(
        selected(&f).unwrap().lines().count(),
        after,
        "the selection stays"
    );
}

#[test]
fn wide_characters_and_emoji_are_selected_by_the_columns_they_take() {
    let (f, _h, _) = live(Opts::default());
    // a | 日 cols 1-3 | 本 cols 3-5 | b | 😀 cols 6-8 | c
    open(&f, "w.txt", "a\u{65e5}\u{672c}b\u{1f600}c");
    down(&f, 0, 2.1, false);
    drag(&f, 0, 6.2);
    assert_eq!(selected(&f).as_deref(), Some("\u{672c}b"));
    drag(&f, 0, 7.5);
    up(&f);
    assert_eq!(selected(&f).as_deref(), Some("\u{672c}b\u{1f600}"));
    assert_eq!(painted(&f), Some(((0, 3), (0, 8))));
}

#[test]
fn ask_agent_sends_the_selection_framed_with_the_file_and_its_lines_and_no_enter() {
    let (f, h, shell) = live(Opts::default());
    let cwd = f.cwds()[0].clone();
    let id = f.ctl.borrow().front_id().unwrap();
    f.say(&cwd, TITLE_IDLE);
    f.pump_until("the agent", |f| f.agent_here(&id));
    feed(&h, "\x1b[?2004h");
    open(&f, "notes.txt", "first\nsecond line\nthird\nfourth");
    down(&f, 1, 0.0, false);
    drag(&f, 2, 5.0);
    up(&f);
    f.ctl.borrow_mut().panel_ask_agent();
    f.pump();
    // The file's text goes through the same frame as a page's selection, unchanged.
    let framed =
        crate::ask_agent::frame_selection("notes.txt lines 2-3", "second line\nthird").unwrap();
    assert_eq!(
        framed,
        "About this from notes.txt lines 2-3:\n> second line\n> third"
    );
    let want = format!("\x1b[200~{}\x1b[201~", framed.replace('\n', "\r"));
    until_written(&shell, |w| w == want.as_bytes());
    std::thread::sleep(Duration::from_millis(40)); // sync-ok: test code only
    f.pump();
    assert_eq!(shell.written(), want.as_bytes(), "no Enter follows");
}

#[test]
fn hostile_file_text_cannot_end_the_paste_or_press_enter() {
    let (f, h, shell) = live(Opts::default());
    let cwd = f.cwds()[0].clone();
    let id = f.ctl.borrow().front_id().unwrap();
    f.say(&cwd, TITLE_IDLE);
    f.pump_until("the agent", |f| f.agent_here(&id));
    feed(&h, "\x1b[?2004h");
    // A control in a file is drawn as U+FFFD by the shaping, so the panel's text never holds an escape; the frame's gate
    // stays the last word on what reaches the prompt.
    open(&f, "evil.txt", "a\x1b[201~\rrm -rf\nok");
    let app = f.ctl.borrow().app().unwrap();
    assert!(f.ctl.borrow_mut().panels.select_all(&app));
    let (label, text) = f.ctl.borrow().panels.selection_ask(&id).unwrap();
    assert!(label.starts_with("evil.txt line"), "{label}");
    assert!(!text.contains('\x1b') && !text.contains('\r'), "{text:?}");
    f.ctl.borrow_mut().panel_ask_agent();
    f.pump();
    let framed = crate::ask_agent::frame_selection(&label, &text).unwrap();
    let want = format!("\x1b[200~{}\x1b[201~", framed.replace('\n', "\r"));
    until_written(&shell, |w| w == want.as_bytes());
    let written = shell.written();
    assert_eq!(
        written.windows(6).filter(|w| *w == b"\x1b[201~").count(),
        1,
        "one end of the paste: {written:?}"
    );
}

/// Every browser the controller made (the markdown page's), so no real one is ever started.
fn with_fake_web(f: &Fx) -> Rc<RefCell<Vec<FakeWeb>>> {
    let slot: Rc<RefCell<Vec<FakeWeb>>> = Rc::default();
    let keep = slot.clone();
    f.ctl.borrow_mut().set_web_factory(Box::new(move |sink| {
        let fake = FakeWeb::new(sink);
        keep.borrow_mut().push(fake.clone());
        Box::new(fake)
    }));
    slot
}

#[test]
fn the_code_view_of_a_markdown_file_selects_and_asks_like_any_text_file() {
    let (f, h, shell) = live(Opts::default());
    let slot = with_fake_web(&f);
    f.ctl.borrow_mut().set_apps_root(f.dir.path().join("apps"));
    let cwd = f.cwds()[0].clone();
    let id = f.ctl.borrow().front_id().unwrap();
    f.say(&cwd, TITLE_IDLE);
    f.pump_until("the agent", |f| f.agent_here(&id));
    feed(&h, "\x1b[?2004h");
    show_text(&f, "r.md", "# Title\n\nSome *text*\n");
    f.pump_until("the compiled page", |_| {
        slot.borrow()
            .iter()
            .flat_map(FakeWeb::calls)
            .any(|c| matches!(c, Call::OpenApp { .. }))
    });
    f.h.comp.invoke_panel_md_view_picked(MdView::Code.index());
    f.pump_until("the code view", |f| f.h.comp.get_panel_kind() == 2);
    down(&f, 2, 5.0, false);
    drag(&f, 2, 11.0);
    up(&f);
    assert_eq!(selected(&f).as_deref(), Some("*text*"));
    f.ctl.borrow_mut().panel_ask_agent();
    f.pump();
    let want = "\x1b[200~About this from r.md line 3:\r> *text*\x1b[201~";
    until_written(&shell, |w| w == want.as_bytes());
    // Back to a page view: the native selection goes with the text.
    f.h.comp
        .invoke_panel_md_view_picked(MdView::Compiled.index());
    f.pump();
    assert_eq!(selected(&f), None);
}
