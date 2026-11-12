//! A remote tab's fitted conversation view (remote sessions PR 7, rule 1193), headless: the conversation is wrapped to
//! the terminal's width at three window widths and two scales, newest at the foot, and a click anywhere on it is the
//! takeover. Nothing here starts a link.

use std::cell::Cell;
use std::rc::Rc;

use slint::{ModelRc, VecModel};

use crate::appctl::AppCtl;
use crate::ui_harness::Harness;
use crate::{AppWindow, RemoteConvoLine};

const SCALES2: [f32; 2] = [1.0, 2.25];
/// Three window widths: narrow, the usual, wide.
const WIDTHS: [u32; 3] = [520, 1000, 1600];

fn lines(n: usize) -> Vec<RemoteConvoLine> {
    (0..n)
        .map(|i| RemoteConvoLine {
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            text: format!(
                "Line {i}: a sentence long enough to wrap at a narrow width, so the fitted view has to wrap it to the window."
            )
            .into(),
            tool: "".into(),
        })
        .collect()
}

fn window(scale: f32, w: u32) -> Harness<AppWindow> {
    let h = Harness::new(scale, w, 640, || {
        AppWindow::new().expect("the window builds")
    });
    AppCtl::push_panel_limits(&h.comp);
    h.comp.set_front_kind(0);
    h.comp.set_has_tabs(true);
    h.comp.set_rconvo_machine("Mac Mini".into());
    h.comp
        .set_rconvo_lines(ModelRc::new(VecModel::from(lines(30))));
    h.comp.set_rconvo_open(true);
    h.advance(0);
    h.advance(16);
    h
}

#[test]
fn the_conversation_is_fitted_to_the_terminals_width_with_the_newest_at_the_foot() {
    for s in SCALES2 {
        let mut widths = Vec::new();
        for w in WIDTHS {
            let h = window(s, w);
            let col = h.comp.get_rconvo_column_w();
            // The text column is the window's width less the view's two paddings (18 each), whatever the width.
            assert!(
                col > 0.0 && col <= w as f32 - 36.0 + 0.5,
                "{s} {w}: column {col}"
            );
            assert!(
                h.comp.get_rconvo_at_foot(),
                "{s} {w}: the newest line shows"
            );
            widths.push(col);
        }
        assert!(
            widths[0] < widths[1] && widths[1] < widths[2],
            "{s}: the column follows the window: {widths:?}"
        );
    }
}

#[test]
fn a_click_on_the_view_takes_the_terminal_back_and_a_hidden_view_takes_none() {
    for s in SCALES2 {
        let h = window(s, 1000);
        let taken = Rc::new(Cell::new(0));
        {
            let t = Rc::clone(&taken);
            h.comp.on_rconvo_take_over(move || t.set(t.get() + 1));
        }
        h.click(300.0, 300.0);
        assert_eq!(taken.get(), 1, "{s}");
        h.comp.set_rconvo_open(false);
        h.advance(16);
        h.click(300.0, 300.0);
        assert_eq!(taken.get(), 1, "{s}: the terminal has the click now");
    }
}
