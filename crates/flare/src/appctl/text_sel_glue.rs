//! Selecting text in the Text panel (2026-11-10), in the window: the pointer over the text, the drag that scrolls past an
//! edge, the keys while the panel has them (Shift with the arrows, Home and End, Ctrl+A, Ctrl+C) and Esc, which lets a
//! selection go before it gives the keys back. The selection itself lives in `PanelCtl` (`crate::panel_sel`).

use std::sync::Arc;

use flare_term::keys::KeyEvent;
use flare_term::sel_machine::DRAG_SCROLL_MS;
use flare_win::clipboard_port::COPY_MAX;

use super::{AppCtl, TermMsgBox, UiMsg};
use crate::clip_flow::copy_async;
use crate::panel_sel::Step;
use crate::term_ui::TermMsg;

/// The timer that scrolls the text while a drag is held past an edge.
const DRAG_TIMER: &str = "panel.sel-scroll";

impl AppCtl {
    pub(super) fn panel_text_down(&mut self, x: f32, y: f32, shift: bool) {
        let Some(app) = self.app() else { return };
        self.panels.text_down(&app, x, y, shift);
    }

    /// A drag past an edge of the view starts the timer that scrolls toward it; it stops by itself when the pointer
    /// comes back, the button goes up or the text goes.
    pub(super) fn panel_text_move(&mut self, x: f32, y: f32) {
        let Some(app) = self.app() else { return };
        if !self.panels.text_move(&app, x, y) || crate::timer_registry::running(DRAG_TIMER) {
            return;
        }
        let weak = self.me.clone();
        crate::timer_registry::set(DRAG_TIMER, DRAG_SCROLL_MS, true, move || {
            let go_on = weak.upgrade().is_some_and(|rc| {
                rc.try_borrow_mut().is_ok_and(|mut c| match c.app() {
                    Some(app) => c.panels.drag_tick(&app),
                    None => false,
                })
            });
            if !go_on {
                crate::timer_registry::clear(DRAG_TIMER);
            }
        });
    }

    pub(super) fn panel_text_up(&mut self) {
        self.panels.text_up();
        crate::timer_registry::clear(DRAG_TIMER);
    }

    /// One step of the drag's scroll (the timer's, and the tests').
    #[cfg(test)]
    pub(crate) fn panel_drag_tick(&mut self) -> bool {
        self.app().is_some_and(|app| self.panels.drag_tick(&app))
    }

    /// Esc in the panel: a selection goes first and the panel keeps the keys; with nothing selected the keys go back to
    /// the terminal.
    pub(super) fn panel_escape(&mut self) {
        if let Some(app) = self.app()
            && self.panels.clear_selection(&app)
        {
            return;
        }
        self.panel_focus_seen = false;
        self.focus_terminal();
    }

    /// A key while the panel has the keys and shows a text file: the selection's keys are taken here, every other key
    /// goes on to the panel (it scrolls). True when taken.
    pub(super) fn panel_text_key(&mut self, ev: &KeyEvent) -> bool {
        let Some(app) = self.app() else { return false };
        // The app's modifier is Ctrl on Windows and Cmd on a Mac (regression rule 1006): the other one, and Alt, are
        // never the selection's.
        let os = self.key_os();
        let other = if os.is_mac() { ev.ctrl } else { ev.meta };
        if !ev.pressed || ev.alt || other || app.get_panel_kind() != 2 {
            return false;
        }
        let primary = os.primary(ev);
        let letter = |l: &str| {
            ev.key.eq_ignore_ascii_case(l) || ev.code == format!("Key{}", l.to_ascii_uppercase())
        };
        if primary && !ev.shift && letter("c") {
            self.copy_panel_selection();
            return true;
        }
        if primary && !ev.shift && letter("a") {
            return self.panels.select_all(&app);
        }
        if !ev.shift {
            return false;
        }
        let step = match (ev.key.as_str(), primary) {
            ("ArrowLeft", false) => Step::Left,
            ("ArrowRight", false) => Step::Right,
            ("ArrowUp", false) => Step::Up,
            ("ArrowDown", false) => Step::Down,
            ("Home", false) => Step::Home,
            ("End", false) => Step::End,
            ("Home", true) => Step::DocStart,
            ("End", true) => Step::DocEnd,
            // A Mac keyboard has no Home or End: Cmd with an arrow is the line's or the file's end.
            ("ArrowLeft", true) if os.is_mac() => Step::Home,
            ("ArrowRight", true) if os.is_mac() => Step::End,
            ("ArrowUp", true) if os.is_mac() => Step::DocStart,
            ("ArrowDown", true) if os.is_mac() => Step::DocEnd,
            _ => return false,
        };
        self.panels.extend_selection(&app, step)
    }

    /// Ctrl+C (Cmd+C on a Mac) in the panel: the selection goes to the clipboard on a worker, as the terminal's copy does, and the
    /// "Copied" badge says so. The selection stays. Nothing selected copies nothing.
    fn copy_panel_selection(&self) {
        let Some(text) = self.panels.selection_text().filter(|t| !t.is_empty()) else {
            return;
        };
        let post = Arc::clone(&self.deps.post);
        copy_async(
            Arc::clone(&self.deps.term.clip_port),
            text,
            COPY_MAX,
            move |ok| post(UiMsg::Term(TermMsgBox(TermMsg::Copied(ok)))),
        );
    }
}
