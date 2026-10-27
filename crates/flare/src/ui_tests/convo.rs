//! The conversation panel (Surfaces stage 6), headless: the Changes list and diff, drawn from rows set directly on the
//! window as the controller sets them. What is held: only the rows in view are built (a diff of 100 000 rows, a list of
//! 5 000 files), the keys scroll, the added and removed rows wear the theme's inks, and the clicks call back. Every
//! check runs at 100% and 225%.

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::Key;
use slint::{Global, ModelRc, VecModel};

use crate::appctl::AppCtl;
use crate::ui_harness::{Frame, Harness};
use crate::{AppWindow, ConvoDiffRow, ConvoFileRow, ConvoInk, Theme};

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
fn another_file_opens_at_the_top_not_at_the_offset_of_the_last_one() {
    for s in SCALES2 {
        let h = window(s);
        h.activate();
        open_changes(&h, 3, "C:\\p\\file0.rs", 5_000);
        h.comp.invoke_focus_panel();
        h.advance(0);
        h.key_text(&key(Key::PageDown));
        h.key_text(&key(Key::PageDown));
        h.advance(0);
        assert!(h.comp.get_panel_convo_diff_first() > 20, "scale {s}");
        // Back to the list and another file, with a diff just as long.
        h.comp.set_panel_convo_selected("C:\\p\\file1.rs".into());
        h.comp.set_panel_convo_rows(diff(5_000));
        h.advance(0);
        assert_eq!(
            h.comp.get_panel_convo_diff_first(),
            0,
            "scale {s}: the new diff opened part way down"
        );
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
    h.comp.set_panel_convo_files(ModelRc::default());
    h.comp.set_panel_convo_rows(ModelRc::default());
    h.advance(0);
    assert_eq!(h.comp.get_panel_convo_list_built(), 0);
    assert_eq!(h.comp.get_panel_convo_diff_built(), 0);
}
