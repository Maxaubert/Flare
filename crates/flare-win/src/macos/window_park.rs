//! Parking the --e2e window on macOS (port phase 3, rules 334 and 336). The Mac meaning of the Windows
//! park: the app becomes an accessory (no Dock icon, never the active app unless it asks, and it never
//! asks), and the window is clear: alpha 0, no shadow, every click passing through it to what is below.
//! It is born off every screen (and kept there by `keep_windows_off_screen` until it is clear), then put
//! on the main screen, because a Mac draws nothing for a window that is on no screen: the display link
//! that paces Slint's frames does not tick for it, and Metal hands out no drawable, so an off-screen
//! window never paints (the Mac E2E's first runs: one software frame, no GPU frame, no link colour). The
//! report reads the real frame and alpha; the Mac E2E itself runs in CI only (the port plan's "Ongoing" row).
//!
//! The handle is the window's `NSView` as an integer (raw-window-handle's `AppKit` handle).

use objc2::MainThreadMarker;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::sel;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSScreen};
use objc2_foundation::{NSPoint, NSRect};

/// Where a parked window sits: far outside every monitor's rectangle.
const PARK_AT: i32 = -32000;

/// The top-left corner of a parked window, in device pixels.
pub fn parked_origin() -> (i32, i32) {
    (PARK_AT, PARK_AT)
}

/// `constrainFrameRect:toScreen:` that keeps the frame it is given: a parked window stays where it was
/// put instead of being pulled back until a strip of it is on a screen (rule 334).
extern "C-unwind" fn keep_frame(
    _this: *mut AnyObject,
    _cmd: Sel,
    frame: NSRect,
    _screen: *mut AnyObject,
) -> NSRect {
    frame
}

/// The implementation `allow_off_screen` puts in.
fn keep_frame_imp() -> Imp {
    // SAFETY: an `extern "C-unwind"` function pointer as the runtime's untyped implementation pointer; the
    // type encoding given with it (`CONSTRAIN_TYPES`) says how the runtime calls it.
    unsafe {
        std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel, NSRect, *mut AnyObject) -> NSRect,
            Imp,
        >(keep_frame)
    }
}

/// The type encoding of `- (NSRect)constrainFrameRect:(NSRect)frame toScreen:(NSScreen *)screen`.
const CONSTRAIN_TYPES: &std::ffi::CStr =
    c"{CGRect={CGPoint=dd}{CGSize=dd}}@:{CGRect={CGPoint=dd}{CGSize=dd}}@";

/// macOS under `--e2e` only (rule 334): `class` (the parked window's own class, winit's window
/// subclass) stops constraining its frame to a screen. AppKit keeps a titled window on a screen when it
/// is ordered in or moved, so without this the parking spot became a 40 px strip at a screen's edge
/// (seen on the macOS runner: (-984, 673) to (40, 1347) on a 1024 by 768 display). Only this class
/// changes; NSWindow and every other window keep AppKit's rule.
pub fn allow_off_screen(class: &AnyClass) {
    let cls: *const AnyClass = class;
    // SAFETY: replacing (or adding, on this class only) an instance method with a function of the
    // signature its type encoding states; AppKit calls it on the main thread only.
    unsafe {
        objc2::ffi::class_replaceMethod(
            cls.cast_mut(),
            sel!(constrainFrameRect:toScreen:),
            keep_frame_imp(),
            CONSTRAIN_TYPES.as_ptr(),
        );
    }
}

/// Whether `class` keeps a frame off every screen (its `constrainFrameRect:toScreen:` is ours).
pub fn allows_off_screen(class: &AnyClass) -> bool {
    class
        .instance_method(sel!(constrainFrameRect:toScreen:))
        .is_some_and(|m| std::ptr::fn_addr_eq(m.implementation(), keep_frame_imp()))
}

/// macOS under `--e2e`, before the first window is made (rule 334): no window of the run is ever pulled
/// onto a screen, so the main window is born at the parking spot and stays there from its first order-in
/// (the park after `show()` would come after AppKit had already moved it). Every window of the process
/// inherits it from `NSWindow`; the real app never calls this.
pub fn keep_windows_off_screen() {
    if let Some(window) = AnyClass::get(c"NSWindow") {
        allow_off_screen(window);
        keep_clear(window);
    }
}

/// `NSWindow`'s own `setAlphaValue:` and `setIgnoresMouseEvents:`, kept by `keep_clear`.
static ORIGINAL_ALPHA: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
static ORIGINAL_IGNORE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// `setAlphaValue:` under `--e2e`: whatever is asked, the window stays at alpha 0.
extern "C-unwind" fn alpha_stays_clear(this: *mut AnyObject, cmd: Sel, _alpha: f64) {
    if let Some(&imp) = ORIGINAL_ALPHA.get() {
        // SAFETY: `imp` is NSWindow's own `setAlphaValue:` (a `void (id, SEL, CGFloat)`), saved when it
        // was replaced; it is called with the receiver and selector it was called with.
        let f: extern "C-unwind" fn(*mut AnyObject, Sel, f64) = unsafe { std::mem::transmute(imp) };
        f(this, cmd, 0.0);
    }
}

/// `setIgnoresMouseEvents:` under `--e2e`: every click goes through the window to what is below.
extern "C-unwind" fn clicks_stay_through(
    this: *mut AnyObject,
    cmd: Sel,
    _flag: objc2::runtime::Bool,
) {
    if let Some(&imp) = ORIGINAL_IGNORE.get() {
        // SAFETY: `imp` is NSWindow's own `setIgnoresMouseEvents:` (a `void (id, SEL, BOOL)`), saved when
        // it was replaced.
        let f: extern "C-unwind" fn(*mut AnyObject, Sel, objc2::runtime::Bool) =
            unsafe { std::mem::transmute(imp) };
        f(this, cmd, objc2::runtime::Bool::YES);
    }
}

/// macOS under `--e2e` only (rule 336): every window of the run stays clear and click-through, whoever
/// asks otherwise. Slint reveals a GPU window by setting its alpha back to 1 once the first frame is
/// drawn (its `RevealOnFirstFrame`), which put the parked window on the runner's display (the Mac
/// E2E's fourth run: "not clear and click-through" at every GPU launch). The original methods are kept
/// and called with 0 and YES. Once per process; the real app never calls this.
pub fn keep_clear(class: &AnyClass) {
    let cls: *const AnyClass = class;
    // SAFETY: as in `panels::guard`: function pointers given as the runtime's untyped `Imp` with the type
    // encodings that say how the runtime calls them.
    unsafe {
        let alpha: Imp = std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel, f64), Imp>(
            alpha_stays_clear,
        );
        let through: Imp = std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel, objc2::runtime::Bool),
            Imp,
        >(clicks_stay_through);
        if ORIGINAL_ALPHA.get().is_none()
            && let Some(old) = objc2::ffi::class_replaceMethod(
                cls.cast_mut(),
                sel!(setAlphaValue:),
                alpha,
                c"v@:d".as_ptr(),
            )
        {
            let _ = ORIGINAL_ALPHA.set(old as usize);
        }
        if ORIGINAL_IGNORE.get().is_none()
            && let Some(old) = objc2::ffi::class_replaceMethod(
                cls.cast_mut(),
                sel!(setIgnoresMouseEvents:),
                through,
                c"v@:B".as_ptr(),
            )
        {
            let _ = ORIGINAL_IGNORE.set(old as usize);
        }
    }
}

/// macOS under `--e2e` (rule 336): no App Nap for the run. An accessory app whose only window is clear
/// would be napped by the system (timers coalesced to seconds, I/O at background priority), and a
/// scenario's 8 s waits would time out on a Mac only. Held for the whole process; the real app never
/// calls this.
pub fn hold_off_app_nap() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
    let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInteractive,
        &NSString::from_str("Flare --e2e run"),
    );
    // The activity lasts as long as the token; the run's whole life.
    std::mem::forget(token);
}

/// Makes the app an accessory and the window clear (alpha 0, no shadow, click-through), then puts it on
/// the main screen's visible area so it paints (rule 336). Its class keeps the frame it is given (rule
/// 334). Never activates the app, never orders the window front, never resizes it. Nothing happens off
/// the main thread.
pub fn park(view: isize) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = crate::glass::window_of(view, mtm) else {
        return;
    };
    NSApplication::sharedApplication(mtm)
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let obj: &AnyObject = window.as_ref();
    allow_off_screen(obj.class());
    // Clear before it moves: it is never seen on the way.
    window.setAlphaValue(0.0);
    window.setHasShadow(false);
    window.setIgnoresMouseEvents(true);
    let at = NSScreen::mainScreen(mtm)
        .map(|s| s.visibleFrame().origin)
        .unwrap_or(NSPoint::new(0.0, 0.0));
    window.setFrameOrigin(at);
}

/// What the harness checks of an `--e2e` window (the same shape as on Windows).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParkReport {
    /// Left, top, right, bottom in device pixels (top-left origin, as winit has them).
    pub rect: (i32, i32, i32, i32),
    /// macOS: the app's activation policy (0 regular, 1 accessory, 2 prohibited), where Windows has
    /// the extended window style.
    pub ex_style: u32,
    /// The box round every screen: left, top, right, bottom.
    pub screen: (i32, i32, i32, i32),
    /// Whether Flare is the active app and this its key window.
    pub foreground: bool,
    /// macOS: the window is clear (alpha 0) and lets every click through, so on a screen it shows
    /// nothing and takes nothing (rule 336).
    pub clear: bool,
}

/// The accessory activation policy, as `ex_style` holds it.
pub const POLICY_ACCESSORY: u32 = 1;
/// The prohibited activation policy (never active at all).
pub const POLICY_PROHIBITED: u32 = 2;

/// Pure: whether two rectangles (left, top, right, bottom) share any pixel.
pub fn rects_overlap(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

impl ParkReport {
    /// Whether any part of the window lies on any screen.
    pub fn on_screen(&self) -> bool {
        rects_overlap(self.rect, self.screen)
    }

    /// Everything wrong with the window as an `--e2e` window; empty when it is safe.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.on_screen() && !self.clear {
            out.push(format!(
                "the window shows on a screen at {:?} (screens {:?}): it is not clear and click-through",
                self.rect, self.screen
            ));
        }
        if self.ex_style != POLICY_ACCESSORY && self.ex_style != POLICY_PROHIBITED {
            out.push("the app is not an accessory, it would show in the Dock".into());
        }
        if self.foreground {
            out.push("the window is the key window of the active app".into());
        }
        out
    }
}

/// Reads the report of a live window. Empty off the main thread or without a window.
pub fn report(view: isize) -> ParkReport {
    let Some(mtm) = MainThreadMarker::new() else {
        return ParkReport::default();
    };
    let Some(window) = crate::glass::window_of(view, mtm) else {
        return ParkReport::default();
    };
    let app = NSApplication::sharedApplication(mtm);
    let scale = window.backingScaleFactor();
    let screens = crate::chrome::screen_frames_at(scale);
    let primary_h = crate::chrome::primary_height();
    let a = crate::chrome::area_of(window.frame(), primary_h, scale);
    let screen = screens
        .iter()
        .fold(None, |acc: Option<(i32, i32, i32, i32)>, s| {
            let r = (s.x, s.y, s.x + s.width, s.y + s.height);
            Some(match acc {
                None => r,
                Some(b) => (b.0.min(r.0), b.1.min(r.1), b.2.max(r.2), b.3.max(r.3)),
            })
        });
    ParkReport {
        rect: (a.x, a.y, a.x + a.width, a.y + a.height),
        ex_style: app.activationPolicy().0.max(0) as u32,
        screen: screen.unwrap_or_default(),
        foreground: app.isActive() && window.isKeyWindow(),
        clear: window.alphaValue() == 0.0 && window.ignoresMouseEvents(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (i32, i32, i32, i32) = (0, 0, 3024, 1964);

    fn safe() -> ParkReport {
        ParkReport {
            rect: (-64000, -64000, -62000, -62720),
            ex_style: POLICY_ACCESSORY,
            screen: SCREEN,
            foreground: false,
            clear: false,
        }
    }

    #[test]
    fn a_parked_accessory_window_has_no_problems() {
        assert!(safe().problems().is_empty());
        let r = ParkReport {
            ex_style: POLICY_PROHIBITED,
            ..safe()
        };
        assert!(r.problems().is_empty());
    }

    #[test]
    fn a_window_on_a_screen_in_the_dock_or_in_front_is_a_problem() {
        let mut r = safe();
        r.rect = (3000, 100, 4000, 700);
        assert!(r.on_screen());
        assert!(r.problems().iter().any(|p| p.contains("not clear")));
        // Rule 336: on a screen, clear and click-through, it is safe.
        r.clear = true;
        assert!(r.problems().is_empty());
        let r = ParkReport {
            ex_style: 0,
            foreground: true,
            ..safe()
        };
        assert_eq!(r.problems().len(), 2);
    }

    // Rule 334: the parked window's class keeps the frame it is given, and only that class changes.
    #[test]
    fn a_parked_class_keeps_its_frame_off_every_screen_and_nswindow_keeps_the_rule() {
        use objc2::runtime::ClassBuilder;
        let window = AnyClass::get(c"NSWindow").expect("AppKit is linked");
        let class = ClassBuilder::new(c"FlareParkTestWindow", window)
            .expect("a new class name")
            .register();
        assert!(!allows_off_screen(class));
        allow_off_screen(class);
        assert!(allows_off_screen(class));
        assert!(!allows_off_screen(window), "NSWindow itself is untouched");
        let frame = NSRect::new(
            NSPoint::new(-32000.0, -32000.0),
            objc2_foundation::NSSize::new(1000.0, 640.0),
        );
        let kept = keep_frame(
            std::ptr::null_mut(),
            sel!(constrainFrameRect:toScreen:),
            frame,
            std::ptr::null_mut(),
        );
        assert_eq!(kept, frame);
    }

    #[test]
    fn off_the_main_thread_nothing_is_touched() {
        park(0);
        assert_eq!(report(0), ParkReport::default());
    }
}
