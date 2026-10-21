//! The "Copied" notice for a program's own selection.
//!
//! A full-screen program that asked for the mouse draws its own selection and copies it itself,
//! through the system clipboard and not through the terminal (no OSC 52). Flare never hears that
//! copy, so after a selecting press (a drag, or a double or triple click) and after a Ctrl+C on a
//! standing selection, Flare looks at the clipboard a few times and announces once when its text
//! changed. Only the text is compared; it is never kept or logged.
//!
//! The clock is explicit: nothing happens by itself. The caller passes `now_ms`, and `advance`
//! runs the looks that are due. Source: PT/core/renderer/lib/appCopyNotice.ts.

/// When the clipboard is looked at after the release or the Ctrl+C, counted from that moment:
/// the program copies through a helper process, which takes a moment.
pub const COPY_LOOKS_MS: [u64; 3] = [150, 500, 1000];
/// A press that moved this far (in pixels) is a drag.
pub const DRAG_PX: f64 = 4.0;

pub trait AppCopyDeps {
    /// The clipboard's text now; None when it cannot be read. (Called on the UI thread in M5;
    /// keep calls rare.)
    fn read(&mut self) -> Option<String>;
    /// The program in front has the mouse (it draws its own selection).
    fn program_has_mouse(&self) -> bool;
    fn announce(&mut self);
}

struct Press {
    x: f64,
    y: f64,
    clicks: u32,
    /// None until the clipboard is read: a plain press reads nothing.
    before: Option<Option<String>>,
}

struct Selection {
    before: Option<String>,
}

/// One look at the clipboard, checked at `start + COPY_LOOKS_MS[step]`.
struct Look {
    epoch: u64,
    was: Option<String>,
    step: usize,
    start: u64,
}

pub struct AppCopyWatch<D: AppCopyDeps> {
    deps: D,
    press: Option<Press>,
    selection: Option<Selection>,
    epoch: u64,
    looks: Vec<Look>,
}

impl<D: AppCopyDeps> AppCopyWatch<D> {
    pub fn new(deps: D) -> Self {
        Self {
            deps,
            press: None,
            selection: None,
            epoch: 0,
            looks: Vec::new(),
        }
    }

    /// A press: epoch += 1, the selection is dropped, and a press is kept only when the program has
    /// the mouse. The clipboard is read here only for a double or triple click (known to select).
    pub fn down(&mut self, x: f64, y: f64, clicks: u32, _now_ms: u64) {
        self.epoch += 1;
        self.selection = None;
        self.press = if self.deps.program_has_mouse() {
            let before = if clicks >= 2 {
                Some(self.deps.read())
            } else {
                None
            };
            Some(Press {
                x,
                y,
                clicks,
                before,
            })
        } else {
            None
        };
    }

    /// The pointer moved with the button down. Once it is a drag, the clipboard is read (once)
    /// so a copy made at the instant of release is still heard.
    pub fn move_to(&mut self, x: f64, y: f64) {
        let needs_read = match &self.press {
            Some(p) => p.before.is_none() && (x - p.x).hypot(y - p.y) >= DRAG_PX,
            None => false,
        };
        if needs_read {
            let text = self.deps.read();
            if let Some(p) = self.press.as_mut() {
                p.before = Some(text);
            }
        }
    }

    /// The release. Nothing when there was no kept press; a plain click (not a drag, fewer than
    /// two clicks) selects nothing. Otherwise the selection stands and a look is scheduled.
    pub fn up(&mut self, x: f64, y: f64, now_ms: u64) {
        let Some(p) = self.press.take() else {
            return;
        };
        let dragged = (x - p.x).hypot(y - p.y) >= DRAG_PX;
        if !dragged && p.clicks < 2 {
            return;
        }
        let before = match p.before {
            Some(b) => b,
            None => self.deps.read(),
        };
        self.look_from(before.clone(), now_ms);
        self.selection = Some(Selection { before });
    }

    /// Ctrl+C: when a selection stands, it was the program's copy; the selection is taken and a
    /// look is scheduled from the clipboard it had at the press.
    pub fn copy_key(&mut self, now_ms: u64) {
        if let Some(s) = self.selection.take() {
            self.look_from(s.before, now_ms);
        }
    }

    /// Any other key: the program's selection is over.
    pub fn other_key(&mut self) {
        self.selection = None;
    }

    /// The program's own selection stands (a selecting release since the last press, key or copy).
    pub fn selecting(&self) -> bool {
        self.selection.is_some()
    }

    /// Runs every look that is due at `now_ms`, in the order they were scheduled. A look from an
    /// older press (its epoch differs) is dropped. When due: a readable, non-empty clipboard that
    /// differs from `was` announces once and the look ends; otherwise the look steps on, and
    /// after the last step it is dropped.
    pub fn advance(&mut self, now_ms: u64) {
        let looks = std::mem::take(&mut self.looks);
        let mut kept = Vec::with_capacity(looks.len());
        for mut look in looks {
            if look.epoch != self.epoch {
                continue;
            }
            let mut keep = true;
            while keep && now_ms >= look.start + COPY_LOOKS_MS[look.step] {
                let now = self.deps.read();
                let changed = match &now {
                    Some(text) => !text.is_empty() && look.was.as_deref() != Some(text.as_str()),
                    None => false,
                };
                if changed {
                    self.deps.announce();
                    keep = false;
                } else {
                    look.step += 1;
                    if look.step >= COPY_LOOKS_MS.len() {
                        keep = false;
                    }
                }
            }
            if keep {
                kept.push(look);
            }
        }
        self.looks = kept;
    }

    fn look_from(&mut self, was: Option<String>, now_ms: u64) {
        self.looks.push(Look {
            epoch: self.epoch,
            was,
            step: 0,
            start: now_ms,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct St {
        clip: String,
        reads: u32,
        announced: u32,
        mouse: bool,
    }

    struct Deps(Rc<RefCell<St>>);

    impl AppCopyDeps for Deps {
        fn read(&mut self) -> Option<String> {
            let mut s = self.0.borrow_mut();
            s.reads += 1;
            Some(s.clip.clone())
        }
        fn program_has_mouse(&self) -> bool {
            self.0.borrow().mouse
        }
        fn announce(&mut self) {
            self.0.borrow_mut().announced += 1;
        }
    }

    fn rig(mouse: bool) -> (AppCopyWatch<Deps>, Rc<RefCell<St>>) {
        let st = Rc::new(RefCell::new(St {
            clip: "old".to_string(),
            reads: 0,
            announced: 0,
            mouse,
        }));
        (AppCopyWatch::new(Deps(st.clone())), st)
    }

    fn set(st: &Rc<RefCell<St>>, text: &str) {
        st.borrow_mut().clip = text.to_string();
    }

    fn announced(st: &Rc<RefCell<St>>) -> u32 {
        st.borrow().announced
    }

    fn reads(st: &Rc<RefCell<St>>) -> u32 {
        st.borrow().reads
    }

    #[test]
    fn a_drag_that_changes_the_clipboard_shows_the_badge_once() {
        let (mut w, st) = rig(true);
        w.down(10.0, 10.0, 1, 0);
        w.up(80.0, 10.0, 0);
        set(&st, "copied text");
        w.advance(150);
        assert_eq!(announced(&st), 1);
        w.advance(500);
        w.advance(1000);
        assert_eq!(announced(&st), 1);
    }

    #[test]
    fn a_late_copy_is_still_heard() {
        let (mut w, st) = rig(true);
        w.down(10.0, 10.0, 1, 0);
        w.up(80.0, 10.0, 0);
        w.advance(150);
        w.advance(500);
        set(&st, "late");
        w.advance(1000);
        assert_eq!(announced(&st), 1);
    }

    #[test]
    fn a_plain_click_or_unchanged_clipboard_says_nothing() {
        // (a) a plain click (moved 1 px, one click) selects nothing
        let (mut w, st) = rig(true);
        w.down(10.0, 10.0, 1, 0);
        w.up(11.0, 10.0, 0);
        set(&st, "changed");
        w.advance(150);
        w.advance(500);
        w.advance(1000);
        assert_eq!(announced(&st), 0);

        // (b) a drag whose clipboard did not change
        let (mut w, st) = rig(true);
        w.down(10.0, 10.0, 1, 0);
        w.up(90.0, 10.0, 0);
        w.advance(150);
        w.advance(500);
        w.advance(1000);
        assert_eq!(announced(&st), 0);
    }

    #[test]
    fn a_double_click_counts_and_a_mouse_terminal_is_not_watched() {
        // (a) a double click selects and is watched
        let (mut w, st) = rig(true);
        w.down(10.0, 10.0, 2, 0);
        w.up(10.0, 10.0, 0);
        set(&st, "word");
        w.advance(150);
        w.advance(500);
        w.advance(1000);
        assert_eq!(announced(&st), 1);

        // (b) a program without the mouse is not watched
        let (mut w, st) = rig(false);
        w.down(10.0, 10.0, 1, 0);
        w.up(90.0, 10.0, 0);
        set(&st, "x");
        w.advance(150);
        w.advance(500);
        w.advance(1000);
        assert_eq!(announced(&st), 0);
    }

    #[test]
    fn selecting_stands_until_a_press_a_key_or_the_copy() {
        let (mut w, _st) = rig(true);
        assert!(!w.selecting());
        w.down(10.0, 10.0, 1, 0);
        w.up(80.0, 10.0, 0);
        assert!(w.selecting());
        w.other_key();
        assert!(!w.selecting());
        w.down(10.0, 10.0, 2, 0);
        w.up(10.0, 10.0, 0);
        assert!(w.selecting());
        w.down(50.0, 50.0, 1, 0);
        assert!(!w.selecting());
        w.up(51.0, 50.0, 0);
        assert!(!w.selecting());
    }

    #[test]
    fn ctrl_c_on_a_selection_shows_the_badge_after_the_copy() {
        let (mut w, st) = rig(true);
        w.down(10.0, 10.0, 2, 0);
        w.up(10.0, 10.0, 0);
        w.advance(150);
        w.advance(500);
        w.advance(1000);
        assert_eq!(announced(&st), 0);
        w.copy_key(1000);
        assert!(!w.selecting());
        set(&st, "the word");
        w.advance(1150);
        assert_eq!(announced(&st), 1);
    }

    #[test]
    fn a_plain_click_reads_no_clipboard_a_drag_reads_on_release() {
        let (mut w, st) = rig(true);
        for _ in 0..5 {
            w.down(10.0, 10.0, 1, 0);
            w.up(11.0, 10.0, 0);
        }
        assert_eq!(reads(&st), 0);
        w.down(10.0, 10.0, 1, 0);
        assert_eq!(reads(&st), 0);
        w.up(80.0, 10.0, 0);
        assert_eq!(reads(&st), 1);
    }

    #[test]
    fn ctrl_c_without_a_selection_looks_at_nothing() {
        let (mut w, st) = rig(true);
        w.copy_key(0);
        set(&st, "something");
        w.advance(150);
        w.advance(500);
        w.advance(1000);
        assert_eq!(announced(&st), 0);
    }

    #[test]
    fn a_copy_at_the_instant_of_release_is_heard() {
        let (mut w, st) = rig(true);
        w.down(10.0, 10.0, 1, 0);
        w.move_to(12.0, 10.0);
        assert_eq!(reads(&st), 0);
        w.move_to(40.0, 10.0);
        assert_eq!(reads(&st), 1);
        set(&st, "copied at once");
        w.up(80.0, 10.0, 0);
        w.advance(150);
        assert_eq!(announced(&st), 1);
    }
}
