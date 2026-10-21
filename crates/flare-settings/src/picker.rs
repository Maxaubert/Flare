//! The colour popover's state: HSV, coalesced writes, revert and placement (M7 Task 16).
//!
//! Port of Prism Terminal's `ColourPicker.tsx` state (`ColourPopover`, the three sliders, `keptPicker`, the
//! code field). No window: the Slint side reports pointer and key events, this module answers with the
//! stored colour to write, if any.

use flare_theme::colour::{
    ColourFormat, Hsva, Rgba, alpha_pct, format, hsv_to_rgb, parse_colour, rgb_to_hsv, to_stored,
};
use flare_theme::colour_contrast::{
    AlphaRange, SnapOpts, colour_commit, fit_alpha, snap_alpha_step,
};

pub const SQUARE_W: f64 = 200.0;
pub const SQUARE_H: f64 = 140.0;
pub const POPOVER_W: f64 = 226.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SliderKey {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Closed {
    Kept,
    Reverted,
}

/// What a slider says to assistive technology.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderAria {
    pub min: u32,
    pub max: u32,
    pub now: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PickerAria {
    pub sv: SliderAria,
    pub hue: SliderAria,
    pub alpha: SliderAria,
}

const BLACK: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

fn clamp(n: f64, lo: f64, hi: f64) -> f64 {
    n.max(lo).min(hi)
}

/// JavaScript's `Math.round` (half up).
fn round(n: f64) -> f64 {
    (n + 0.5).floor()
}

pub struct Picker {
    hsv: Hsva,
    start: Rgba,
    range: AlphaRange,
    wrote: bool,
    last_sent: String,
    pending: Option<String>,
    opened: String,
    snap: Option<fn(f64) -> f64>,
    alpha_disabled: bool,
}

impl Picker {
    /// `opened_stored` is what Escape puts back; `now_stored` the colour in force when it differs (a picker
    /// opened again after a tab switch shows where it was left).
    pub fn open(opened_stored: &str, now_stored: Option<&str>, range: AlphaRange) -> Picker {
        let start = fit_alpha(&parse_colour(opened_stored).unwrap_or(BLACK), &range);
        let shown = fit_alpha(
            &parse_colour(now_stored.unwrap_or(opened_stored)).unwrap_or(start),
            &range,
        );
        let last_sent = to_stored(&shown);
        Picker {
            hsv: rgb_to_hsv(&shown),
            wrote: last_sent != to_stored(&start),
            start,
            range,
            last_sent,
            pending: None,
            opened: opened_stored.to_string(),
            snap: None,
            alpha_disabled: false,
        }
    }

    /// The host's own alpha steps (Prism Primary's glass levels).
    pub fn set_snap(&mut self, snap: Option<fn(f64) -> f64>) {
        self.snap = snap;
    }

    /// The alpha bar shows, faded and inert (acrylic off).
    pub fn set_alpha_disabled(&mut self, disabled: bool) {
        self.alpha_disabled = disabled;
    }

    pub fn hsv(&self) -> Hsva {
        self.hsv
    }

    pub fn rgba(&self) -> Rgba {
        fit_alpha(&hsv_to_rgb(&self.hsv), &self.range)
    }

    pub fn start(&self) -> Rgba {
        self.start
    }

    pub fn opened(&self) -> &str {
        &self.opened
    }

    fn stored_of(&self, c: &Hsva) -> String {
        let rgb = fit_alpha(&hsv_to_rgb(c), &self.range);
        match self.snap {
            Some(snap) if self.range.alpha => to_stored(&Rgba {
                a: snap(rgb.a),
                ..rgb
            }),
            _ => to_stored(&rgb),
        }
    }

    fn send(&mut self, stored: String) -> Option<String> {
        if stored == self.last_sent {
            return None;
        }
        self.last_sent.clone_from(&stored);
        self.wrote = true;
        Some(stored)
    }

    fn queue(&mut self, next: Hsva) {
        self.hsv = next;
        self.pending = Some(self.stored_of(&next));
    }

    fn now(&mut self, next: Hsva) -> Option<String> {
        self.hsv = next;
        self.pending = None;
        let stored = self.stored_of(&next);
        self.send(stored)
    }

    fn opts(&self) -> SnapOpts<'_> {
        SnapOpts {
            min: self.range.alpha_min,
            max: self.range.alpha_max,
            snap: self.snap.as_ref().map(|f| f as &dyn Fn(f64) -> f64),
        }
    }

    pub fn drag_sv(&mut self, x: f64, y: f64) {
        let (x, y) = (clamp(x, 0.0, 1.0), clamp(y, 0.0, 1.0));
        self.queue(Hsva {
            s: x * 100.0,
            v: (1.0 - y) * 100.0,
            ..self.hsv
        });
    }

    pub fn drag_hue(&mut self, x: f64) {
        self.queue(Hsva {
            h: clamp(x, 0.0, 1.0) * 360.0,
            ..self.hsv
        });
    }

    pub fn drag_alpha(&mut self, x: f64) {
        if self.alpha_disabled || !self.range.alpha {
            return;
        }
        let a = snap_alpha_step(clamp(x, 0.0, 1.0), self.hsv.a, false, &self.opts());
        self.queue(Hsva { a, ..self.hsv });
    }

    pub fn key_sv(&mut self, key: SliderKey, shift: bool) -> Option<String> {
        let d = if shift { 10.0 } else { 1.0 };
        let (s, v) = (round(self.hsv.s), round(self.hsv.v));
        let next = match key {
            SliderKey::Left => Hsva {
                s: clamp(s - d, 0.0, 100.0),
                ..self.hsv
            },
            SliderKey::Right => Hsva {
                s: clamp(s + d, 0.0, 100.0),
                ..self.hsv
            },
            SliderKey::Down => Hsva {
                v: clamp(v - d, 0.0, 100.0),
                ..self.hsv
            },
            SliderKey::Up => Hsva {
                v: clamp(v + d, 0.0, 100.0),
                ..self.hsv
            },
            SliderKey::Home | SliderKey::End => return None,
        };
        self.now(next)
    }

    pub fn key_hue(&mut self, key: SliderKey, shift: bool) -> Option<String> {
        let d = if shift { 10.0 } else { 1.0 };
        let h = round(self.hsv.h);
        let next = match key {
            SliderKey::Left | SliderKey::Down => h - d,
            SliderKey::Right | SliderKey::Up => h + d,
            SliderKey::Home => 0.0,
            SliderKey::End => 360.0,
        };
        self.now(Hsva {
            h: clamp(next, 0.0, 360.0),
            ..self.hsv
        })
    }

    pub fn key_alpha(&mut self, key: SliderKey, shift: bool) -> Option<String> {
        if self.alpha_disabled || !self.range.alpha {
            return None;
        }
        let d = if shift { 10.0 } else { 1.0 };
        let v = round(self.hsv.a * 100.0);
        let next = match key {
            SliderKey::Left | SliderKey::Down => v - d,
            SliderKey::Right | SliderKey::Up => v + d,
            SliderKey::Home => f64::from(alpha_pct(self.range.alpha_min)),
            SliderKey::End => f64::from(alpha_pct(self.range.alpha_max)),
        };
        let a = snap_alpha_step(next / 100.0, self.hsv.a, true, &self.opts());
        self.now(Hsva { a, ..self.hsv })
    }

    /// The eyedropper: the RGB is the screen's, the alpha stays the colour's own.
    pub fn set_rgb_keep_alpha(&mut self, c: Rgba) -> Option<String> {
        let h = rgb_to_hsv(&Rgba { a: 1.0, ..c });
        self.now(Hsva { a: self.hsv.a, ..h })
    }

    /// Once per frame: the queued colour, when it differs from the last one sent.
    pub fn frame(&mut self) -> Option<String> {
        let p = self.pending.take()?;
        self.send(p)
    }

    /// A press outside or focus away: flush the queue, keep the colour.
    pub fn close(&mut self) -> Option<String> {
        self.frame()
    }

    /// Escape: drops the queued colour; puts back the opening colour when something was written.
    pub fn cancel(&mut self) -> (Closed, Option<String>) {
        self.pending = None;
        if self.wrote {
            (Closed::Reverted, Some(to_stored(&self.start)))
        } else {
            (Closed::Kept, None)
        }
    }

    pub fn aria(&self) -> PickerAria {
        let s = round(self.hsv.s) as u32;
        let v = round(self.hsv.v) as u32;
        let h = round(self.hsv.h) as u32;
        let a = alpha_pct(self.hsv.a);
        PickerAria {
            sv: SliderAria {
                min: 0,
                max: 100,
                now: s,
                text: format!("saturation {s} percent, brightness {v} percent"),
            },
            hue: SliderAria {
                min: 0,
                max: 360,
                now: h,
                text: format!("{h} degrees"),
            },
            alpha: SliderAria {
                min: alpha_pct(self.range.alpha_min),
                max: alpha_pct(self.range.alpha_max),
                now: a,
                text: format!("{a} percent opaque"),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Under the swatch, or above it when there is no room below; inside the window either way.
pub fn place_popover(anchor: Rect, size: (f64, f64), window: (f64, f64)) -> (f64, f64) {
    place_popover_at(anchor, size, window, 0.0)
}

/// `place_popover` with the size and the corner rounded to the device grid at `scale` (0 leaves
/// them as they are): the right edge stays on the swatch's, the borders stay crisp.
pub fn place_popover_at(
    anchor: Rect,
    size: (f64, f64),
    window: (f64, f64),
    scale: f64,
) -> (f64, f64) {
    let sn = |v: f64| {
        if scale > 0.0 {
            (v * scale).round() / scale
        } else {
            v
        }
    };
    let (w, h) = (sn(size.0), sn(size.1));
    let left = clamp(anchor.x + anchor.w - w, 8.0, window.0 - w - 8.0);
    let below = anchor.y + anchor.h + 6.0;
    let top = if below + h <= window.1 - 8.0 {
        below
    } else {
        (anchor.y - h - 6.0).max(8.0)
    };
    (sn(left), sn(top))
}

/// The key of a picker kept open: the field's label, and whether it is in the theme editor.
pub fn picker_key(in_editor: bool, label: &str) -> String {
    format!("{}:{label}", if in_editor { "editor" } else { "page" })
}

/// The picker left open comes back when Settings is shown again.
#[derive(Default)]
pub struct KeptPicker {
    kept: Option<(String, String)>,
}

impl KeptPicker {
    pub fn keep(&mut self, key: &str, opened: &str) {
        self.kept = Some((key.to_string(), opened.to_string()));
    }

    pub fn forget(&mut self) {
        self.kept = None;
    }

    /// The opening colour, for the same key only.
    pub fn recall(&self, key: &str) -> Option<&str> {
        self.kept
            .as_ref()
            .filter(|(k, _)| k == key)
            .map(|(_, o)| o.as_str())
    }
}

/// The code field beside the swatch: a draft while typing, the colour the rest of the time.
#[derive(Default)]
pub struct CodeField {
    draft: Option<String>,
}

impl CodeField {
    pub fn edit(&mut self, text: &str) {
        self.draft = Some(text.to_string());
    }

    pub fn shown(&self, value: &str, range: &AlphaRange, f: ColourFormat) -> String {
        if let Some(d) = &self.draft {
            return d.clone();
        }
        match parse_colour(value) {
            Some(c) => format(&fit_alpha(&c, range), f),
            None => value.to_string(),
        }
    }

    pub fn commit(&mut self, value: &str, range: &AlphaRange) -> Option<String> {
        let draft = self.draft.take();
        colour_commit(draft.as_deref(), value, range)
    }

    /// A draft is dropped and Escape consumed; with none, Escape is the host's.
    pub fn escape(&mut self) -> bool {
        self.draft.take().is_some()
    }

    pub fn width(f: ColourFormat) -> f64 {
        if f == ColourFormat::Hex { 76.0 } else { 172.0 }
    }

    pub fn uppercase(&self, f: ColourFormat) -> bool {
        f == ColourFormat::Hex && self.draft.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: AlphaRange = AlphaRange {
        alpha: true,
        alpha_min: 0.0,
        alpha_max: 1.0,
    };

    fn steps95(a: f64) -> f64 {
        if a >= 1.0 { 1.0 } else { a.min(0.95) }
    }

    #[test]
    fn open_shows_the_colour_in_force_and_remembers_the_opening_colour() {
        let mut p = Picker::open("#112233", Some("#445566"), ALL);
        assert_eq!(p.opened(), "#112233");
        assert_eq!(to_stored(&p.rgba()), "#445566");
        assert_eq!(to_stored(&p.start()), "#112233");
        assert_eq!(p.cancel(), (Closed::Reverted, Some("#112233".to_string())));
    }

    #[test]
    fn a_drag_is_one_write_per_frame() {
        let mut p = Picker::open("#112233", None, ALL);
        for i in 1..=5 {
            p.drag_sv(f64::from(i) / 10.0, 0.2);
        }
        assert!(p.frame().is_some());
        assert_eq!(p.frame(), None);
    }

    #[test]
    fn a_key_writes_at_once() {
        let mut p = Picker::open("#112233", None, ALL);
        assert!(p.key_hue(SliderKey::Right, false).is_some());
        let v = round(p.hsv().v);
        p.key_sv(SliderKey::Up, true);
        assert_eq!(round(p.hsv().v), (v + 10.0).min(100.0));
        assert_eq!(p.key_sv(SliderKey::Home, false), None);
    }

    #[test]
    fn grey_keeps_its_hue_while_you_drag_hue() {
        let mut p = Picker::open("#808080", None, ALL);
        assert_eq!(p.hsv().s, 0.0);
        p.drag_hue(0.5);
        p.drag_sv(0.0, 0.5);
        p.drag_sv(0.0, 0.4);
        assert_eq!(p.hsv().h, 180.0);
    }

    #[test]
    fn escape_reverts_exactly_the_opening_value_only_when_something_was_written() {
        let mut p = Picker::open("#112233", None, ALL);
        assert_eq!(p.cancel(), (Closed::Kept, None));
        p.key_hue(SliderKey::Right, false);
        assert_eq!(p.cancel(), (Closed::Reverted, Some("#112233".to_string())));
    }

    #[test]
    fn a_queued_colour_dropped_by_escape_is_never_written() {
        let mut p = Picker::open("#112233", None, ALL);
        p.drag_hue(0.9);
        assert_eq!(p.cancel(), (Closed::Kept, None));
        assert_eq!(p.frame(), None);
    }

    #[test]
    fn a_press_outside_keeps_the_colour_and_flushes_the_queue() {
        let mut p = Picker::open("#112233", None, ALL);
        p.drag_sv(0.9, 0.1);
        assert!(p.close().is_some());
        assert_eq!(p.close(), None);
    }

    #[test]
    fn alpha_keys_walk_in_steps_from_solid_95_then_one_percent_below_the_steps() {
        let mut p = Picker::open("#112233", None, ALL);
        p.set_snap(Some(steps95));
        let pct = |p: &Picker| alpha_pct(p.hsv().a);
        assert_eq!(pct(&p), 100);
        p.key_alpha(SliderKey::Left, false);
        assert_eq!(pct(&p), 95);
        p.key_alpha(SliderKey::Left, false);
        assert_eq!(pct(&p), 94);
        p.key_alpha(SliderKey::Left, true);
        assert_eq!(pct(&p), 84);
        p.key_alpha(SliderKey::End, false);
        assert_eq!(pct(&p), 100);
        // No host steps: one percent a press.
        let mut q = Picker::open("#112233", None, ALL);
        q.key_alpha(SliderKey::Left, false);
        assert_eq!(pct(&q), 99);
    }

    #[test]
    fn alpha_respects_the_range() {
        let r = AlphaRange {
            alpha: true,
            alpha_min: 0.1,
            alpha_max: 1.0,
        };
        let mut p = Picker::open("#112233", None, r);
        p.key_alpha(SliderKey::Home, false);
        assert_eq!(alpha_pct(p.hsv().a), 10);
        p.key_alpha(SliderKey::End, false);
        assert_eq!(alpha_pct(p.hsv().a), 100);
        p.set_alpha_disabled(true);
        assert_eq!(p.key_alpha(SliderKey::Home, false), None);
        let mut q = Picker::open(
            "#112233",
            None,
            AlphaRange {
                alpha: false,
                ..ALL
            },
        );
        assert_eq!(q.key_alpha(SliderKey::Left, false), None);
    }

    #[test]
    fn the_eyedropper_keeps_the_alpha() {
        let mut p = Picker::open("#00ff0080", None, ALL);
        let red = Rgba {
            r: 255.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        assert_eq!(p.set_rgb_keep_alpha(red), Some("#ff000080".to_string()));
    }

    #[test]
    fn aria_names_and_values() {
        let r = AlphaRange {
            alpha: true,
            alpha_min: 0.1,
            alpha_max: 1.0,
        };
        let p = Picker::open("#ff000080", None, r);
        let a = p.aria();
        assert_eq!(a.sv.now, 100);
        assert_eq!(a.sv.text, "saturation 100 percent, brightness 100 percent");
        assert_eq!((a.hue.now, a.hue.text.as_str()), (0, "0 degrees"));
        assert_eq!(a.alpha.text, "50 percent opaque");
        assert_eq!((a.alpha.min, a.alpha.max, a.alpha.now), (10, 100, 50));
    }

    #[test]
    fn place_popover_matches_prism_terminals_rules() {
        let win = (800.0, 600.0);
        let size = (POPOVER_W, 240.0);
        let at_right = Rect {
            x: 760.0,
            y: 100.0,
            w: 36.0,
            h: 24.0,
        };
        assert_eq!(
            place_popover(at_right, size, win),
            (800.0 - POPOVER_W - 8.0, 130.0)
        );
        let left_edge = Rect {
            x: 10.0,
            y: 100.0,
            w: 36.0,
            h: 24.0,
        };
        assert_eq!(place_popover(left_edge, size, win).0, 8.0);
        let low = Rect {
            x: 400.0,
            y: 500.0,
            w: 36.0,
            h: 24.0,
        };
        assert_eq!(place_popover(low, size, win).1, 500.0 - 240.0 - 6.0);
        let tiny = place_popover(
            Rect {
                x: 0.0,
                y: 50.0,
                w: 36.0,
                h: 24.0,
            },
            size,
            (300.0, 200.0),
        );
        assert_eq!(tiny.1, 8.0);
    }

    #[test]
    fn the_kept_picker_comes_back_for_the_same_key_only() {
        let mut k = KeptPicker::default();
        let key = picker_key(false, "Accent colour");
        assert_eq!(key, "page:Accent colour");
        assert_eq!(picker_key(true, "Accent colour"), "editor:Accent colour");
        k.keep(&key, "#101010");
        assert_eq!(k.recall(&key), Some("#101010"));
        assert_eq!(k.recall("editor:Accent colour"), None);
        k.forget();
        assert_eq!(k.recall(&key), None);
    }

    #[test]
    fn code_field_commits_only_what_changed() {
        let mut f = CodeField::default();
        assert_eq!(f.commit("#aabbcc", &ALL), None);
        f.edit("ABC");
        assert_eq!(f.commit("#aabbcc", &ALL), None);
        f.edit("#112233");
        assert_eq!(f.commit("#aabbcc", &ALL), Some("#112233".to_string()));
        f.edit("zz");
        assert_eq!(f.commit("#aabbcc", &ALL), None);
        let no = AlphaRange {
            alpha: false,
            ..ALL
        };
        f.edit("123");
        assert_eq!(f.commit("#aabbcc", &no), Some("#112233".to_string()));
        f.edit("#11223380");
        assert_eq!(f.commit("#aabbcc", &no), Some("#112233".to_string()));
        f.edit("rgba(170, 187, 204, 0.5)");
        assert_eq!(f.commit("#aabbcc", &no), None);
    }

    #[test]
    fn code_field_escape_is_consumed_only_with_a_draft() {
        let mut f = CodeField::default();
        assert!(!f.escape());
        f.edit("x");
        assert!(f.escape());
        assert!(!f.escape());
    }

    #[test]
    fn code_field_width_and_case() {
        let mut f = CodeField::default();
        assert_eq!(CodeField::width(ColourFormat::Hex), 76.0);
        assert_eq!(CodeField::width(ColourFormat::Rgba), 172.0);
        assert_eq!(CodeField::width(ColourFormat::Hsla), 172.0);
        assert!(f.uppercase(ColourFormat::Hex));
        assert!(!f.uppercase(ColourFormat::Rgba));
        f.edit("a");
        assert!(!f.uppercase(ColourFormat::Hex));
    }

    #[test]
    fn formats_are_shown_as_typed_back_into_the_same_colour() {
        for value in ["#336699", "#33669980", "#ff0000"] {
            for fmt in [ColourFormat::Hex, ColourFormat::Rgba, ColourFormat::Hsla] {
                let mut f = CodeField::default();
                let shown = f.shown(value, &ALL, fmt);
                f.edit(&shown);
                assert_eq!(f.commit(value, &ALL), None, "{value} as {fmt:?}: {shown}");
            }
        }
    }
}
