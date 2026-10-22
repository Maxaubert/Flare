//! cfg(test) headless UI harness (Task 19b): drives a Slint component through the software
//! renderer with pointer and key events, a settable animation clock and pixel measuring helpers.
//! No OS window is ever created. Later tasks use pieces of it, hence the dead-code allowance.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, SharedString};

/// The eight display scalings every geometry test runs at.
pub const SCALES: [f32; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

thread_local! {
    /// The window the next component creation takes.
    static NEXT_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
    /// The platform clock in milliseconds; animations follow it.
    static CLOCK_MS: Cell<u64> = const { Cell::new(0) };
    static PLATFORM_SET: Cell<bool> = const { Cell::new(false) };
}

struct HarnessPlatform;

impl Platform for HarnessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        NEXT_WINDOW
            .with(|w| w.borrow_mut().take())
            .map(|w| w as Rc<dyn WindowAdapter>)
            .ok_or_else(|| slint::PlatformError::Other("no harness window is waiting".into()))
    }

    fn duration_since_start(&self) -> Duration {
        Duration::from_millis(CLOCK_MS.with(Cell::get))
    }
}

pub fn ensure_platform() {
    if !PLATFORM_SET.with(Cell::get) {
        slint::platform::set_platform(Box::new(HarnessPlatform))
            .expect("the harness platform is set once per thread");
        PLATFORM_SET.with(|s| s.set(true));
    }
}

/// Queues a software window for the next component made on this thread, without a Harness around it (a test that
/// drives code which makes its own component, like the dictation widget).
pub fn provide_window(css_w: u32, css_h: u32) -> Rc<MinimalSoftwareWindow> {
    ensure_platform();
    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    win.set_size(PhysicalSize::new(css_w, css_h));
    NEXT_WINDOW.with(|w| *w.borrow_mut() = Some(win.clone()));
    win
}

/// Moves the platform clock on and runs the timers that came due.
pub fn advance_clock(ms: u64) {
    CLOCK_MS.with(|c| c.set(c.get() + ms));
    slint::platform::update_timers_and_animations();
}

pub struct Harness<C: ComponentHandle> {
    pub comp: C,
    win: Rc<MinimalSoftwareWindow>,
    scale: f32,
    css: (u32, u32),
}

impl<C: ComponentHandle> Harness<C> {
    pub fn new(scale: f32, css_w: u32, css_h: u32, make: impl FnOnce() -> C) -> Self {
        ensure_platform();
        let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        NEXT_WINDOW.with(|w| *w.borrow_mut() = Some(win.clone()));
        let comp = make();
        win.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        let h = Self {
            comp,
            win,
            scale,
            css: (css_w, css_h),
        };
        h.win.set_size(PhysicalSize::new(
            h.phys(css_w as f32),
            h.phys(css_h as f32),
        ));
        h.comp.show().expect("the harness window shows");
        slint::platform::update_timers_and_animations();
        h
    }

    /// The display scaling changes under the window (it moved to another monitor, or the user
    /// changed it): the same css size, a new count of device pixels.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
        self.win.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        self.win.set_size(PhysicalSize::new(
            self.phys(self.css.0 as f32),
            self.phys(self.css.1 as f32),
        ));
        slint::platform::update_timers_and_animations();
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn phys(&self, css: f32) -> u32 {
        (css * self.scale).round() as u32
    }

    fn at(x: f32, y: f32) -> LogicalPosition {
        LogicalPosition::new(x, y)
    }

    pub fn move_to(&self, x: f32, y: f32) {
        self.win.dispatch_event(WindowEvent::PointerMoved {
            position: Self::at(x, y),
        });
    }

    /// The pointer leaves the window (winit's `CursorLeft`).
    pub fn leave(&self) {
        self.win.dispatch_event(WindowEvent::PointerExited);
    }

    pub fn press(&self, x: f32, y: f32, b: PointerEventButton) {
        self.win.dispatch_event(WindowEvent::PointerPressed {
            position: Self::at(x, y),
            button: b,
        });
    }

    pub fn release(&self, x: f32, y: f32, b: PointerEventButton) {
        self.win.dispatch_event(WindowEvent::PointerReleased {
            position: Self::at(x, y),
            button: b,
        });
    }

    fn click_with(&self, x: f32, y: f32, b: PointerEventButton) {
        self.move_to(x, y);
        self.press(x, y, b);
        self.release(x, y, b);
    }

    pub fn click(&self, x: f32, y: f32) {
        self.click_with(x, y, PointerEventButton::Left);
    }

    pub fn right_click(&self, x: f32, y: f32) {
        self.click_with(x, y, PointerEventButton::Right);
    }

    pub fn middle_click(&self, x: f32, y: f32) {
        self.click_with(x, y, PointerEventButton::Middle);
    }

    /// Press at `from`, `steps` moves along the line, release at `to`.
    pub fn drag(&self, from: (f32, f32), to: (f32, f32), steps: u32) {
        self.move_to(from.0, from.1);
        self.press(from.0, from.1, PointerEventButton::Left);
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            self.move_to(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        }
        self.release(to.0, to.1, PointerEventButton::Left);
    }

    /// Types each char of `text`; a special key is `slint::platform::Key::Escape.into()` text.
    pub fn key_text(&self, text: &str) {
        for ch in text.chars() {
            let s = SharedString::from(ch.to_string().as_str());
            self.win
                .dispatch_event(WindowEvent::KeyPressed { text: s.clone() });
            self.win
                .dispatch_event(WindowEvent::KeyReleased { text: s });
        }
    }

    /// Presses shift and ctrl as asked, types `text`, then releases the modifiers.
    pub fn key_with(&self, shift: bool, ctrl: bool, text: &str) {
        let mods: Vec<SharedString> = [
            (shift, slint::platform::Key::Shift),
            (ctrl, slint::platform::Key::Control),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, k)| SharedString::from(char::from(k)))
        .collect();
        for m in &mods {
            self.win
                .dispatch_event(WindowEvent::KeyPressed { text: m.clone() });
        }
        self.key_text(text);
        for m in mods.iter().rev() {
            self.win
                .dispatch_event(WindowEvent::KeyReleased { text: m.clone() });
        }
    }

    /// The window becomes the active one, as when a person is using it (a field draws its selection
    /// and its caret only then).
    pub fn activate(&self) {
        self.win
            .dispatch_event(slint::platform::WindowEvent::WindowActiveChanged(true));
    }

    pub fn advance(&self, ms: u64) {
        CLOCK_MS.with(|c| c.set(c.get() + ms));
        slint::platform::update_timers_and_animations();
    }

    pub fn render(&self) -> Frame {
        let w = self.phys(self.css.0 as f32);
        let h = self.phys(self.css.1 as f32);
        let mut buf = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
        self.win.request_redraw();
        self.win.draw_if_needed(|r| {
            r.render(&mut buf, w as usize);
        });
        Frame {
            w,
            h,
            rgba: buf.iter().flat_map(straight_rgba).collect(),
        }
    }
}

fn straight_rgba(p: &PremultipliedRgbaColor) -> [u8; 4] {
    let un = |c: u8| -> u8 {
        if p.alpha == 0 {
            0
        } else {
            (u32::from(c) * 255 / u32::from(p.alpha)).min(255) as u8
        }
    };
    [un(p.red), un(p.green), un(p.blue), p.alpha]
}

/// A rendered frame as straight RGBA8.
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    /// Counts matching pixels in the half-open box.
    pub fn count_where(
        &self,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
        f: impl Fn([u8; 4]) -> bool,
    ) -> usize {
        let mut n = 0;
        for y in y0..y1.min(self.h) {
            for x in x0..x1.min(self.w) {
                if f(self.px(x, y)) {
                    n += 1;
                }
            }
        }
        n
    }

    /// Consecutive matching pixels along row `y` from `x_from`.
    pub fn run_len_h(&self, y: u32, x_from: u32, f: impl Fn([u8; 4]) -> bool) -> u32 {
        let mut n = 0;
        let mut x = x_from;
        while x < self.w && f(self.px(x, y)) {
            n += 1;
            x += 1;
        }
        n
    }

    /// Consecutive matching pixels along column `x` from `y_from`.
    pub fn run_len_v(&self, x: u32, y_from: u32, f: impl Fn([u8; 4]) -> bool) -> u32 {
        let mut n = 0;
        let mut y = y_from;
        while y < self.h && f(self.px(x, y)) {
            n += 1;
            y += 1;
        }
        n
    }

    /// Min x, min y, max x, max y of the matching pixels in the half-open box.
    pub fn bbox_where(
        &self,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
        f: impl Fn([u8; 4]) -> bool,
    ) -> Option<(u32, u32, u32, u32)> {
        let mut out: Option<(u32, u32, u32, u32)> = None;
        for y in y0..y1.min(self.h) {
            for x in x0..x1.min(self.w) {
                if f(self.px(x, y)) {
                    out = Some(match out {
                        None => (x, y, x, y),
                        Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x), d.max(y)),
                    });
                }
            }
        }
        out
    }

    /// Every channel within `tol`.
    pub fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
        a.iter().zip(b.iter()).all(|(p, q)| p.abs_diff(*q) <= tol)
    }

    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path) // sync-ok: test and screenshot tooling
            .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.w, self.h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&self.rgba)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    slint::slint! {
        export component TestWin inherits Window {
            width: 200px;
            height: 100px;
            background: #102030;
            in-out property <int> clicks;
            in-out property <string> buttons;
            in-out property <bool> go;
            in-out property <string> last-key;
            out property <length> rx: r.x;
            r := Rectangle {
                x: root.go ? 100px : 0px;
                y: 80px;
                width: 10px;
                height: 10px;
                background: #ff0000;
                animate x { duration: 240ms; }
            }
            TouchArea {
                clicked => { root.clicks += 1; }
                pointer-event(e) => {
                    if e.kind == PointerEventKind.up {
                        if e.button == PointerEventButton.right { root.buttons += "right,"; }
                        if e.button == PointerEventButton.middle { root.buttons += "middle,"; }
                    }
                }
            }
            FocusScope {
                init => { self.focus(); }
                key-pressed(e) => { root.last-key = e.text; accept }
            }
        }
    }

    fn harness(scale: f32) -> Harness<TestWin> {
        Harness::new(scale, 200, 100, || TestWin::new().unwrap())
    }

    #[test]
    fn a_click_is_counted_at_every_scale() {
        for s in SCALES {
            let h = harness(s);
            h.click(50.0, 50.0);
            assert_eq!(h.comp.get_clicks(), 1, "scale {s}");
        }
    }

    #[test]
    fn a_right_and_a_middle_click_are_distinct_events() {
        let h = harness(1.0);
        h.right_click(50.0, 50.0);
        h.middle_click(50.0, 50.0);
        assert_eq!(h.comp.get_buttons().as_str(), "right,middle,");
    }

    #[test]
    fn animations_follow_the_clock() {
        let h = harness(1.0);
        assert_eq!(h.comp.get_rx(), 0.0);
        h.comp.set_go(true);
        h.advance(0);
        assert_eq!(h.comp.get_rx(), 0.0);
        h.advance(120);
        let mid = h.comp.get_rx();
        assert!(mid > 0.0 && mid < 100.0, "mid {mid}");
        h.advance(240);
        assert_eq!(h.comp.get_rx(), 100.0);
    }

    #[test]
    fn render_returns_the_physical_size() {
        let h = harness(2.25);
        let f = h.render();
        assert_eq!((f.w, f.h), (450, 225));
        assert!(Frame::near(f.px(300, 20), [0x10, 0x20, 0x30, 255], 1));
    }

    #[test]
    fn frame_helpers_measure_runs_and_boxes() {
        let (w, h) = (30u32, 20u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 4..9 {
            for x in 10..13 {
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[9, 9, 9, 255]);
            }
        }
        let f = Frame { w, h, rgba };
        let is = |p: [u8; 4]| p[3] == 255;
        assert_eq!(f.run_len_h(5, 10, is), 3);
        assert_eq!(f.run_len_v(11, 4, is), 5);
        assert_eq!(f.bbox_where(0, 0, w, h, is), Some((10, 4, 12, 8)));
        assert_eq!(f.count_where(0, 0, w, h, is), 15);
        assert_eq!(f.bbox_where(0, 10, w, h, is), None);
    }

    #[test]
    fn two_threads_have_two_platforms() {
        let threads: Vec<_> = (0..3)
            .map(|_| {
                std::thread::spawn(|| {
                    let h = harness(1.0);
                    h.click(50.0, 50.0);
                    h.comp.get_clicks()
                })
            })
            .collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), 1);
        }
    }

    #[test]
    fn key_text_reaches_a_focus_scope() {
        let h = harness(1.0);
        h.key_text("a");
        assert_eq!(h.comp.get_last_key().as_str(), "a");
    }
}
