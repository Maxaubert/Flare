//! The conversation panels (Surfaces stage 6), headless: the Changes list and diff and the Tasks list, drawn from rows
//! set directly on the window as the controller sets them. What is held: only the rows in view are built (a diff of
//! 100 000 rows, a list of 5 000 files), the keys scroll, the added and removed rows wear the theme's inks, a task wears
//! its state and the one in progress is highlighted, a folded group hides its tasks, and the clicks call back. Every
//! check runs at 100% and 225%.

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::Key;
use slint::{Global, ModelRc, SharedString, VecModel};

use crate::appctl::AppCtl;
use crate::ui_harness::{Frame, Harness};
use crate::{AppWindow, ConvoDiffRow, ConvoFileRow, ConvoInk, ConvoTaskRow, Theme};

const SCALES2: [f32; 2] = [1.0, 2.25];

fn window(scale: f32) -> Harness<AppWindow> {
    let h = Harness::new(scale, 1000, 640, || {
        AppWindow::new().expect("the window builds")
    });
    AppCtl::push_panel_limits(&h.comp);
    h.advance(0);
    h
}

fn key(k: Key) -> String {
    String::from(char::from(k))
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn files(n: usize) -> ModelRc<ConvoFileRow> {
    model(
        (0..n)
            .map(|i| ConvoFileRow {
                path: format!("C:\\p\\file{i}.rs").into(),
                name: format!("file{i}.rs").into(),
                dir: "C:\\p".into(),
                meta: "2 edits".into(),
                failed: false,
            })
            .collect(),
    )
}

fn diff(n: usize) -> ModelRc<ConvoDiffRow> {
    model(
        (0..n)
            .map(|i| ConvoDiffRow {
                kind: match i % 4 {
                    0 => 1,
                    1 => 2,
                    2 => 3,
                    _ => 1,
                },
                old_no: if i % 4 == 2 {
                    "".into()
                } else {
                    format!("{i}").into()
                },
                new_no: if i % 4 == 1 {
                    "".into()
                } else {
                    format!("{i}").into()
                },
                text: format!("let line_{i} = {i};").into(),
            })
            .collect(),
    )
}

fn task(kind: i32, state: i32, text: &str, active: bool) -> ConvoTaskRow {
    ConvoTaskRow {
        kind,
        state,
        text: text.into(),
        sub: if active {
            "Doing it".into()
        } else {
            SharedString::new()
        },
        active,
        key: if kind == 0 {
            text.into()
        } else {
            SharedString::new()
        },
        collapsed: false,
        count: if kind == 0 {
            "1 of 2 done".into()
        } else {
            SharedString::new()
        },
    }
}

fn open_changes(h: &Harness<AppWindow>, n_files: usize, selected: &str, n_rows: usize) {
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(4);
    h.comp.set_panel_title("Changes".into());
    h.comp.set_panel_convo_files(files(n_files));
    h.comp.set_panel_convo_selected(selected.into());
    h.comp.set_panel_convo_rows(diff(n_rows));
    h.comp.set_panel_convo_max_cols(30);
    h.advance(0);
}

fn open_tasks(h: &Harness<AppWindow>, rows: Vec<ConvoTaskRow>) {
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(5);
    h.comp.set_panel_title("Tasks".into());
    h.comp.set_panel_convo_tasks(model(rows));
    h.advance(0);
}

/// The physical pixel of css point (x, y).
fn at(h: &Harness<AppWindow>, f: &Frame, x: f32, y: f32) -> [u8; 4] {
    let s = h.scale();
    f.px((x * s) as u32, (y * s) as u32)
}

fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    Frame::near(a, b, tol)
}

/// `ink` at `alpha` over `under`.
fn over(under: [u8; 4], ink: slint::Color, alpha: f32) -> [u8; 4] {
    let m = |u: u8, i: u8| (f32::from(u) * (1.0 - alpha) + f32::from(i) * alpha).round() as u8;
    [
        m(under[0], ink.red()),
        m(under[1], ink.green()),
        m(under[2], ink.blue()),
        255,
    ]
}

// ---- the file list ----

#[test]
fn a_big_file_list_builds_only_the_rows_in_view() {
    for s in SCALES2 {
        let h = window(s);
        open_changes(&h, 5_000, "", 0);
        let built = h.comp.get_panel_convo_list_built();
        let row_h = h.comp.get_panel_convo_list_row_h();
        let want = (h.comp.get_panel_body_h() / row_h).ceil() as i32 + 2;
        assert_eq!(built, want, "scale {s}: the view and two spare rows");
        assert!(built < 40, "scale {s}: never the whole list ({built})");
        // A short list builds all it has.
        open_changes(&h, 3, "", 0);
        assert_eq!(h.comp.get_panel_convo_list_built(), 3, "scale {s}");
    }
}

#[test]
fn a_click_on_a_file_picks_it_and_the_bar_goes_back_or_opens_the_path() {
    for s in SCALES2 {
        let h = window(s);
        let picks = Rc::new(RefCell::new(Vec::<String>::new()));
        let opens = Rc::new(RefCell::new(Vec::<String>::new()));
        {
            let picks = picks.clone();
            h.comp
                .on_panel_convo_select(move |p| picks.borrow_mut().push(p.to_string()));
            let opens = opens.clone();
            h.comp
                .on_panel_convo_open(move |p| opens.borrow_mut().push(p.to_string()));
        }
        open_changes(&h, 4, "", 0);
        let (bx, by, bw) = (
            h.comp.get_panel_body_x(),
            h.comp.get_panel_body_y(),
            h.comp.get_panel_body_w(),
        );
        let row_h = h.comp.get_panel_convo_list_row_h();
        h.click(bx + bw / 2.0, by + row_h * 1.5);
        assert_eq!(
            picks.borrow().as_slice(),
            ["C:\\p\\file1.rs"],
            "scale {s}: the second row"
        );
        // In the diff the bar has the way back (left) and the path (the rest).
        open_changes(&h, 4, "C:\\p\\file1.rs", 12);
        h.click(bx + 30.0, by + 15.0);
        assert_eq!(
            picks.borrow().last().map(String::as_str),
            Some(""),
            "scale {s}: back"
        );
        h.click(bx + bw - 40.0, by + 15.0);
        assert_eq!(
            opens.borrow().as_slice(),
            ["C:\\p\\file1.rs"],
            "scale {s}: the path opens the file"
        );
    }
}

// ---- the diff ----

/// The view's own windowing, with a model far longer than the controller ever hands it (it caps a file's diff at
/// `view::MAX_ROWS`; see `convo_ctl`'s `a_capped_diff_...` test for that side).
#[test]
fn the_diff_view_windows_a_model_of_a_hundred_thousand_rows_and_scrolls() {
    for s in SCALES2 {
        let h = window(s);
        h.activate();
        open_changes(&h, 2, "C:\\p\\file0.rs", 100_000);
        let row_h = h.comp.get_panel_convo_diff_row_h();
        let view_h = h.comp.get_panel_body_h() - 30.0;
        let want = (view_h / row_h).ceil() as i32 + 2;
        assert_eq!(h.comp.get_panel_convo_diff_built(), want, "scale {s}");
        assert!(want < 80, "scale {s}: {want} rows built of 100 000");
        h.comp.invoke_focus_panel();
        h.advance(0);
        assert_eq!(h.comp.get_panel_convo_diff_first(), 0);
        for _ in 0..3 {
            h.key_text(&key(Key::DownArrow));
        }
        h.advance(0);
        assert_eq!(h.comp.get_panel_convo_diff_first(), 3, "scale {s}");
        h.key_text(&key(Key::PageDown));
        h.advance(0);
        assert!(h.comp.get_panel_convo_diff_first() > 20, "scale {s}");
        h.key_text(&key(Key::End));
        h.advance(0);
        assert!(h.comp.get_panel_convo_diff_first() > 99_900, "scale {s}");
        h.key_text(&key(Key::Home));
        h.advance(0);
        assert_eq!(h.comp.get_panel_convo_diff_first(), 0, "scale {s}");
        // Still the same few rows: the view moved, the tree did not grow.
        assert_eq!(h.comp.get_panel_convo_diff_built(), want, "scale {s}");
    }
}

#[test]
fn added_and_removed_rows_wear_the_themes_inks_and_context_rows_wear_none() {
    for s in SCALES2 {
        let h = window(s);
        open_changes(&h, 2, "C:\\p\\file0.rs", 40);
        let ink = ConvoInk::get(&h.comp);
        let ground = {
            let c = Theme::get(&h.comp).get_ground();
            [c.red(), c.green(), c.blue(), 255]
        };
        let f = h.render();
        let (bx, by, bw) = (
            h.comp.get_panel_body_x(),
            h.comp.get_panel_body_y(),
            h.comp.get_panel_body_w(),
        );
        let row_h = h.comp.get_panel_convo_diff_row_h();
        // Rows start under the 30 px bar. Row 0 is context, 1 removed, 2 added; far right of the text.
        let x = bx + bw - 24.0;
        let y = |n: f32| by + 30.0 + n * row_h + row_h / 2.0;
        let ctx = at(&h, &f, x, y(0.0));
        let removed = at(&h, &f, x, y(1.0));
        let added = at(&h, &f, x, y(2.0));
        assert!(
            near(ctx, ground, 3),
            "scale {s}: context {ctx:?} on {ground:?}"
        );
        assert!(
            near(removed, over(ground, ink.get_removed(), 0.16), 4),
            "scale {s}: removed {removed:?}"
        );
        assert!(
            near(added, over(ground, ink.get_added(), 0.16), 4),
            "scale {s}: added {added:?}"
        );
        assert!(!near(added, removed, 8), "scale {s}: the two inks differ");
    }
}

#[test]
fn backspace_goes_back_from_a_diff_and_the_arrows_do_not_scroll_an_empty_one() {
    let h = window(1.0);
    h.activate();
    let picks = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let picks = picks.clone();
        h.comp
            .on_panel_convo_select(move |p| picks.borrow_mut().push(p.to_string()));
    }
    open_changes(&h, 2, "C:\\p\\file0.rs", 0);
    h.comp.invoke_focus_panel();
    h.advance(0);
    h.key_text(&key(Key::DownArrow));
    h.advance(0);
    assert_eq!(h.comp.get_panel_convo_diff_first(), 0, "nothing to scroll");
    h.key_text(&key(Key::Backspace));
    assert_eq!(picks.borrow().as_slice(), [""], "back to the list");
    // In the list, Backspace is nobody's.
    open_changes(&h, 2, "", 0);
    h.key_text(&key(Key::Backspace));
    assert_eq!(picks.borrow().len(), 1);
}

// ---- the tasks ----

fn accent_pixels(h: &Harness<AppWindow>) -> usize {
    let f = h.render();
    let s = h.scale();
    let acc = Theme::get(&h.comp).get_accent();
    let want = [acc.red(), acc.green(), acc.blue(), 255];
    let x0 = (h.comp.get_panel_body_x() * s) as u32;
    let y0 = (h.comp.get_panel_body_y() * s) as u32;
    f.count_where(
        x0,
        y0,
        x0 + (h.comp.get_panel_body_w() * s) as u32,
        y0 + (h.comp.get_panel_body_h() * s) as u32,
        |p| Frame::near(p, want, 4),
    )
}

#[test]
fn a_task_wears_its_state_and_only_the_one_in_progress_has_the_accent() {
    for s in SCALES2 {
        let h = window(s);
        open_tasks(
            &h,
            vec![
                task(1, 2, "Read the code", false),
                task(1, 0, "Write the test", false),
                task(1, 3, "Something odd", false),
            ],
        );
        assert_eq!(h.comp.get_panel_convo_task_rows(), 3);
        let none = accent_pixels(&h);
        open_tasks(
            &h,
            vec![
                task(1, 2, "Read the code", false),
                task(1, 1, "Edit the file", true),
                task(1, 0, "Write the test", false),
            ],
        );
        let some = accent_pixels(&h);
        let min = (14.0 * 14.0 * 0.15 * s * s) as usize;
        assert!(
            some >= none + min,
            "scale {s}: the dot of the task in progress is the accent ({none} then {some})"
        );
    }
}

#[test]
fn wrapped_task_rows_sit_on_whole_device_pixels_at_fractional_scales() {
    for s in [1.0f32, 1.25, 1.75, 2.25] {
        let h = window(s);
        let long = "A task whose words are long enough to wrap onto a second and a third line in a narrow panel body";
        let mut rows = vec![task(1, 1, long, true)];
        for i in 0..5 {
            rows.push(task(1, 0, &format!("{long} {i}"), false));
        }
        rows.push(task(0, 0, "Subagent abc", false));
        open_tasks(&h, rows);
        let total = h.comp.get_panel_convo_task_content_h() * s;
        assert!(
            (total - total.round()).abs() < 0.01,
            "scale {s}: the list is {total} device pixels tall, not a whole number"
        );
    }
}

#[test]
fn the_active_task_has_a_tinted_row_and_a_plain_one_does_not() {
    for s in SCALES2 {
        let h = window(s);
        open_tasks(
            &h,
            vec![
                task(1, 1, "Edit the file", true),
                task(1, 0, "Write the test", false),
            ],
        );
        let f = h.render();
        let ground = {
            let c = Theme::get(&h.comp).get_ground();
            [c.red(), c.green(), c.blue(), 255]
        };
        let acc = Theme::get(&h.comp).get_accent();
        let (bx, by, bw) = (
            h.comp.get_panel_body_x(),
            h.comp.get_panel_body_y(),
            h.comp.get_panel_body_w(),
        );
        // The right end of the first row (past the text) and of the second.
        let x = bx + bw - 14.0;
        let first = at(&h, &f, x, by + 6.0);
        assert!(
            near(first, over(ground, acc, 0.14), 4),
            "scale {s}: the active row is tinted ({first:?})"
        );
        let second_y = by + h.comp.get_panel_convo_task_content_h() - 6.0;
        let second = at(&h, &f, x, second_y);
        assert!(
            near(second, ground, 3),
            "scale {s}: a plain row is bare ({second:?})"
        );
    }
}

#[test]
fn a_group_header_calls_back_with_its_key_and_a_folded_group_has_fewer_rows() {
    for s in SCALES2 {
        let h = window(s);
        let toggled = Rc::new(RefCell::new(Vec::<String>::new()));
        {
            let toggled = toggled.clone();
            h.comp
                .on_panel_convo_toggle(move |k| toggled.borrow_mut().push(k.to_string()));
        }
        let mut rows = vec![
            task(0, 0, "agent-one", false),
            task(1, 1, "Look", true),
            task(1, 0, "Think", false),
        ];
        open_tasks(&h, rows.clone());
        assert_eq!(h.comp.get_panel_convo_task_rows(), 3);
        h.render();
        let tall = h.comp.get_panel_convo_task_content_h();
        h.click(
            h.comp.get_panel_body_x() + 40.0,
            h.comp.get_panel_body_y() + 10.0,
        );
        assert_eq!(toggled.borrow().as_slice(), ["agent-one"], "scale {s}");
        // The controller then sends the list without the folded group's tasks.
        rows[0].collapsed = true;
        rows.truncate(1);
        open_tasks(&h, rows);
        assert_eq!(h.comp.get_panel_convo_task_rows(), 1);
        h.render();
        let short = h.comp.get_panel_convo_task_content_h();
        assert!(short < tall, "scale {s}: {short} against {tall}");
    }
}

#[test]
fn the_keys_scroll_a_long_task_list() {
    let h = window(1.0);
    h.activate();
    let rows: Vec<ConvoTaskRow> = (0..200)
        .map(|i| task(1, 0, &format!("Task number {i}"), false))
        .collect();
    open_tasks(&h, rows);
    h.comp.invoke_focus_panel();
    h.advance(0);
    let before = h.render();
    h.key_text(&key(Key::PageDown));
    h.advance(0);
    let after = h.render();
    assert_ne!(before.rgba, after.rgba, "a page down moved the list");
    h.key_text(&key(Key::Home));
    h.advance(0);
    assert_eq!(h.render().rgba, before.rgba, "home is back at the top");
}

// ---- nothing runs ----

#[test]
fn the_conversation_view_has_no_animation_and_no_timer() {
    let src = include_str!("../../ui/convo.slint");
    assert!(!src.contains("animate"), "an idle panel runs nothing");
    assert!(!src.contains("Timer"));
}

#[test]
fn a_closed_panel_holds_no_conversation_rows_after_the_controller_lets_go() {
    let h = window(1.0);
    open_changes(&h, 50, "C:\\p\\file0.rs", 500);
    open_tasks(&h, vec![task(1, 0, "x", false)]);
    h.comp.set_panel_convo_files(ModelRc::default());
    h.comp.set_panel_convo_rows(ModelRc::default());
    h.comp.set_panel_convo_tasks(ModelRc::default());
    h.advance(0);
    assert_eq!(h.comp.get_panel_convo_task_rows(), 0);
    assert_eq!(h.comp.get_panel_convo_list_built(), 0);
    assert_eq!(h.comp.get_panel_convo_diff_built(), 0);
}
