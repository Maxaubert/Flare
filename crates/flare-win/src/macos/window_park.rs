//! Parking the --e2e window on macOS (port phase 3, rules 1107 and 1109). The Mac meaning of the Windows
//! park: the app becomes an accessory (no Dock icon, never the active app unless it asks, and it never
//! asks), and the window is clear: alpha 0, no shadow, every click passing through it to what is below.
//! It is born off every screen (and kept there by `keep_windows_off_screen` until it is clear), then put
//! on the main screen, because a Mac draws nothing for a window that is on no screen: the display link
//! that paces Slint's frames does not tick for it, and Metal hands out no drawable, so an off-screen
//! window never paints (the Mac E2E's first runs: one software frame, no GPU frame, no link colour). The
//! report reads the real frame and alpha; the Mac E2E itself runs in CI only (the port plan's "Ongoing" row).
//!
//! A `--start-hidden` run (remote sessions, rule 302) is parked the same way from its first window, and its
//! first raise lets go of all of it (`unpark`): an `--e2e` run never does.
//!
//! The handle is the window's `NSView` as an integer (raw-window-handle's `AppKit` handle).

use objc2::MainThreadMarker;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::sel;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSScreen};
use objc2_foundation::{NSPoint, NSRect};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

/// Where a parked window sits: far outside every monitor's rectangle.
const PARK_AT: i32 = -32000;

/// The top-left corner of a parked window, in device pixels.
pub fn parked_origin() -> (i32, i32) {
    (PARK_AT, PARK_AT)
}

/// Whether the park still holds this run's windows clear, click-through and free of screens (rules 1107,
/// 1109, 1111). True from the start: an `--e2e` run never lets go. A `--start-hidden` run lets go at its
/// first raise (`unpark`, rule 302); from then on every stand-in passes on what it is asked.
static HELD: AtomicBool = AtomicBool::new(true);

/// Whether the park still holds the windows (see `HELD`).
pub fn is_held() -> bool {
    HELD.load(Ordering::SeqCst)
}

/// `NSWindow`'s own `constrainFrameRect:toScreen:`, kept by `allow_off_screen` for the time after `unpark`.
static ORIGINAL_CONSTRAIN: OnceLock<usize> = OnceLock::new();

/// `constrainFrameRect:toScreen:` that keeps the frame it is given: a parked window stays where it was
/// put instead of being pulled back until a strip of it is on a screen (rule 1107). Once the park lets
/// go (`unpark`, rule 302), AppKit's own rule applies again.
extern "C-unwind" fn keep_frame(
    this: *mut AnyObject,
    cmd: Sel,
    frame: NSRect,
    screen: *mut AnyObject,
) -> NSRect {
    if is_held() {
        return frame;
    }
    match ORIGINAL_CONSTRAIN.get() {
        Some(&imp) => {
            // SAFETY: `imp` is NSWindow's own `constrainFrameRect:toScreen:` of exactly this signature,
            // saved before it was replaced; it is called with the receiver and arguments it was given.
            let f: extern "C-unwind" fn(*mut AnyObject, Sel, NSRect, *mut AnyObject) -> NSRect =
                unsafe { std::mem::transmute(imp) };
            f(this, cmd, frame, screen)
        }
        None => frame,
    }
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

/// macOS under `--e2e` only (rule 1107): `class` (the parked window's own class, winit's window
/// subclass) stops constraining its frame to a screen. AppKit keeps a titled window on a screen when it
/// is ordered in or moved, so without this the parking spot became a 40 px strip at a screen's edge
/// (seen on the macOS runner: (-984, 673) to (40, 1347) on a 1024 by 768 display). Only this class
/// changes; NSWindow and every other window keep AppKit's rule.
pub fn allow_off_screen(class: &AnyClass) {
    let cls: *const AnyClass = class;
    // AppKit's own rule is kept for the time after `unpark` (rule 302); never our stand-in.
    if ORIGINAL_CONSTRAIN.get().is_none()
        && let Some(m) = class.instance_method(sel!(constrainFrameRect:toScreen:))
        && !std::ptr::fn_addr_eq(m.implementation(), keep_frame_imp())
    {
        let _ = ORIGINAL_CONSTRAIN.set(m.implementation() as usize);
    }
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

/// macOS under `--e2e` and `--start-hidden`, before the first window is made (rules 1107, 302): no window of
/// the run is ever pulled onto a screen, so the main window is born at the parking spot and stays there
/// from its first order-in (the park after `show()` would come after AppKit had already moved it). Every
/// window of the process inherits it from `NSWindow`; a normal launch never calls this, and a
/// `--start-hidden` run lets go of it at its first raise (`unpark`).
pub fn keep_windows_off_screen() {
    if let Some(window) = AnyClass::get(c"NSWindow") {
        allow_off_screen(window);
        keep_clear(window);
        born_clear(window);
    }
}

/// `NSWindow`'s own two content-rect initialisers, kept by `born_clear`.
static ORIGINAL_INIT4: OnceLock<usize> = OnceLock::new();
static ORIGINAL_INIT5: OnceLock<usize> = OnceLock::new();

/// What a held window's own code last asked of the two setters the park holds (rule 302), given back
/// at `unpark`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wanted {
    /// The alpha asked for.
    pub alpha: f64,
    /// Whether clicks were asked to pass through.
    pub through: bool,
}

impl Default for Wanted {
    /// Nothing asked yet: a window is opaque and takes its clicks.
    fn default() -> Self {
        Self {
            alpha: 1.0,
            through: false,
        }
    }
}

/// Every held window's `Wanted`, by its pointer. Read once, at `unpark`, for the windows still alive.
static WANTED: Mutex<Option<HashMap<usize, Wanted>>> = Mutex::new(None);

/// Runs `f` on the `WANTED` map, made on first use. A poisoned lock is taken as it is: plain values.
fn with_wanted<R>(f: impl FnOnce(&mut HashMap<usize, Wanted>) -> R) -> R {
    let mut guard = WANTED.lock().unwrap_or_else(PoisonError::into_inner);
    f(guard.get_or_insert_with(HashMap::new))
}

/// Pure (rules 1109, 302): the alpha a stand-in passes on. Held, always 0; let go, what was asked.
pub fn alpha_to_pass(held: bool, asked: f64) -> f64 {
    if held { 0.0 } else { asked }
}

/// Pure (rules 1109, 302): whether a stand-in lets clicks through. Held, always; let go, as asked.
pub fn through_to_pass(held: bool, asked: bool) -> bool {
    held || asked
}

/// Pure (rule 302): what a held window asked, with one ask written over it.
pub fn wanted_after(old: Option<Wanted>, alpha: Option<f64>, through: Option<bool>) -> Wanted {
    let old = old.unwrap_or_default();
    Wanted {
        alpha: alpha.unwrap_or(old.alpha),
        through: through.unwrap_or(old.through),
    }
}

/// Makes a just-initialised window clear before anything can order it in: alpha 0, no shadow, every
/// click through, no order-in animation (rule 1111). A null window (a failed init) is left alone, and
/// so is every window made after the park let go (`unpark`, rule 302).
fn make_clear(window: *mut AnyObject) {
    if window.is_null() || !is_held() {
        return;
    }
    // SAFETY: `window` is the NSWindow its initialiser just returned, alive and owned by the caller;
    // each message is a plain NSWindow setter with the argument type it declares.
    unsafe {
        let w: &AnyObject = &*window;
        let _: () = objc2::msg_send![w, setAlphaValue: 0.0f64];
        let _: () = objc2::msg_send![w, setHasShadow: objc2::runtime::Bool::NO];
        let _: () = objc2::msg_send![w, setIgnoresMouseEvents: objc2::runtime::Bool::YES];
        // NSWindowAnimationBehaviorNone: AppKit draws no zoom-in of the window on its first order-in.
        let _: () = objc2::msg_send![w, setAnimationBehavior: 2isize];
    }
    // The setters above were the park's own asks, not the window's (rule 302): nothing is asked yet.
    with_wanted(|m| m.insert(window as usize, Wanted::default()));
}

/// `initWithContentRect:styleMask:backing:defer:` under `--e2e`: NSWindow's own, then clear.
extern "C-unwind" fn init4_clear(
    this: *mut AnyObject,
    cmd: Sel,
    rect: NSRect,
    style: usize,
    backing: usize,
    defer: objc2::runtime::Bool,
) -> *mut AnyObject {
    let Some(&imp) = ORIGINAL_INIT4.get() else {
        return std::ptr::null_mut();
    };
    // SAFETY: `imp` is NSWindow's own initialiser of exactly this signature, saved when it was replaced.
    let f: extern "C-unwind" fn(
        *mut AnyObject,
        Sel,
        NSRect,
        usize,
        usize,
        objc2::runtime::Bool,
    ) -> *mut AnyObject = unsafe { std::mem::transmute(imp) };
    let w = f(this, cmd, rect, style, backing, defer);
    make_clear(w);
    w
}

/// `initWithContentRect:styleMask:backing:defer:screen:` under `--e2e`: NSWindow's own, then clear.
extern "C-unwind" fn init5_clear(
    this: *mut AnyObject,
    cmd: Sel,
    rect: NSRect,
    style: usize,
    backing: usize,
    defer: objc2::runtime::Bool,
    screen: *mut AnyObject,
) -> *mut AnyObject {
    let Some(&imp) = ORIGINAL_INIT5.get() else {
        return std::ptr::null_mut();
    };
    // SAFETY: as in `init4_clear`, with the screen argument passed on.
    let f: extern "C-unwind" fn(
        *mut AnyObject,
        Sel,
        NSRect,
        usize,
        usize,
        objc2::runtime::Bool,
        *mut AnyObject,
    ) -> *mut AnyObject = unsafe { std::mem::transmute(imp) };
    let w = f(this, cmd, rect, style, backing, defer, screen);
    make_clear(w);
    w
}

/// macOS under `--e2e` only (rule 1111): every window of the run is BORN clear. The park makes the
/// main window clear once it has a handle, but a window is ordered in before that (Slint shows it, the
/// park's timer comes a tick later), and the Mac E2E's screen guard saw such windows on the runner's
/// display at full alpha for a moment (18 scenarios of run 42569349881). Once per process; the real
/// app never calls this.
pub fn born_clear(class: &AnyClass) {
    let cls: *const AnyClass = class;
    // SAFETY: as in `keep_clear`: function pointers given as the runtime's untyped `Imp` with the type
    // encodings that say how the runtime calls them.
    unsafe {
        let four: Imp = std::mem::transmute::<
            extern "C-unwind" fn(
                *mut AnyObject,
                Sel,
                NSRect,
                usize,
                usize,
                objc2::runtime::Bool,
            ) -> *mut AnyObject,
            Imp,
        >(init4_clear);
        let five: Imp = std::mem::transmute::<
            extern "C-unwind" fn(
                *mut AnyObject,
                Sel,
                NSRect,
                usize,
                usize,
                objc2::runtime::Bool,
                *mut AnyObject,
            ) -> *mut AnyObject,
            Imp,
        >(init5_clear);
        // The original is saved before it is replaced, so the stand-in always has one to call.
        if ORIGINAL_INIT4.get().is_none()
            && let Some(m) =
                class.instance_method(sel!(initWithContentRect:styleMask:backing:defer:))
            && ORIGINAL_INIT4.set(m.implementation() as usize).is_ok()
        {
            let _ = objc2::ffi::class_replaceMethod(
                cls.cast_mut(),
                sel!(initWithContentRect:styleMask:backing:defer:),
                four,
                c"@@:{CGRect={CGPoint=dd}{CGSize=dd}}QQB".as_ptr(),
            );
        }
        // The original is saved before it is replaced, so the stand-in always has one to call.
        if ORIGINAL_INIT5.get().is_none()
            && let Some(m) =
                class.instance_method(sel!(initWithContentRect:styleMask:backing:defer:screen:))
            && ORIGINAL_INIT5.set(m.implementation() as usize).is_ok()
        {
            let _ = objc2::ffi::class_replaceMethod(
                cls.cast_mut(),
                sel!(initWithContentRect:styleMask:backing:defer:screen:),
                five,
                c"@@:{CGRect={CGPoint=dd}{CGSize=dd}}QQB@".as_ptr(),
            );
        }
    }
}

/// `NSWindow`'s own `setAlphaValue:` and `setIgnoresMouseEvents:`, kept by `keep_clear`.
static ORIGINAL_ALPHA: OnceLock<usize> = OnceLock::new();
static ORIGINAL_IGNORE: OnceLock<usize> = OnceLock::new();

/// `setAlphaValue:` while the park holds: whatever is asked, the window stays at alpha 0, and the ask is
/// kept for `unpark` (rule 302). Once the park lets go, the ask is passed on.
extern "C-unwind" fn alpha_stays_clear(this: *mut AnyObject, cmd: Sel, alpha: f64) {
    let held = is_held();
    if held && !this.is_null() {
        with_wanted(|m| {
            let now = wanted_after(m.get(&(this as usize)).copied(), Some(alpha), None);
            m.insert(this as usize, now);
        });
    }
    if let Some(&imp) = ORIGINAL_ALPHA.get() {
        // SAFETY: `imp` is NSWindow's own `setAlphaValue:` (a `void (id, SEL, CGFloat)`), saved when it
        // was replaced; it is called with the receiver and selector it was called with.
        let f: extern "C-unwind" fn(*mut AnyObject, Sel, f64) = unsafe { std::mem::transmute(imp) };
        f(this, cmd, alpha_to_pass(held, alpha));
    }
}

/// `setIgnoresMouseEvents:` while the park holds: every click goes through the window to what is below,
/// and the ask is kept for `unpark` (rule 302). Once the park lets go, the ask is passed on.
extern "C-unwind" fn clicks_stay_through(
    this: *mut AnyObject,
    cmd: Sel,
    flag: objc2::runtime::Bool,
) {
    let held = is_held();
    if held && !this.is_null() {
        with_wanted(|m| {
            let now = wanted_after(m.get(&(this as usize)).copied(), None, Some(flag.as_bool()));
            m.insert(this as usize, now);
        });
    }
    if let Some(&imp) = ORIGINAL_IGNORE.get() {
        // SAFETY: `imp` is NSWindow's own `setIgnoresMouseEvents:` (a `void (id, SEL, BOOL)`), saved when
        // it was replaced.
        let f: extern "C-unwind" fn(*mut AnyObject, Sel, objc2::runtime::Bool) =
            unsafe { std::mem::transmute(imp) };
        f(
            this,
            cmd,
            objc2::runtime::Bool::new(through_to_pass(held, flag.as_bool())),
        );
    }
}

/// macOS under `--e2e` and `--start-hidden` (rules 1109, 302): every window of the run stays clear and
/// click-through, whoever asks otherwise, until `unpark`. Slint reveals a GPU window by setting its alpha
/// back to 1 once the first frame is drawn (its `RevealOnFirstFrame`), which put the parked window on the
/// runner's display (the Mac E2E's fourth run: "not clear and click-through" at every GPU launch). The
/// original methods are kept and called with 0 and YES. Once per process; a normal launch never calls this.
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

/// macOS under `--e2e` and `--start-hidden` (rules 1109, 301): no App Nap for the run. An accessory app
/// whose only window is clear would be napped by the system (timers coalesced to seconds, I/O at
/// background priority), and a scenario's 8 s waits would time out on a Mac only, as would a hidden
/// host's live shells. Held for the whole process; a normal launch never calls this.
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
/// the main screen's visible area so it paints (rule 1109). Its class keeps the frame it is given (rule
/// 1107). Never activates the app, never orders the window front, never resizes it. Nothing happens off
/// the main thread, and nothing once the park has let go (`unpark`, rule 302): a raise that came first wins.
pub fn park(view: isize) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if !is_held() {
        return;
    }
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

/// A `--start-hidden` run's first raise (rule 302): the park lets go of every window for good. The app is a
/// regular app again (a Dock icon, it may be the active app); every window still alive gets back what its own
/// code last asked of its alpha and clicks (opaque and taking clicks when it asked nothing) and its order-in
/// animation; the main window (`view`) is opaque, takes clicks and has its shadow; and AppKit's own rules for
/// frames, alpha and clicks apply again. The caller moves the window to its place after this, so AppKit's
/// rule keeps it on a screen. Nothing happens off the main thread.
pub fn unpark(view: isize) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    HELD.store(false, Ordering::SeqCst);
    let wanted = with_wanted(std::mem::take);
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    for window in app.windows().iter() {
        let key = objc2::rc::Retained::as_ptr(&window) as usize;
        let Some(asked) = wanted.get(&key) else {
            continue;
        };
        window.setAlphaValue(asked.alpha);
        window.setIgnoresMouseEvents(asked.through);
        // Its shadow stays as it is: a window may have asked for none (the dictation widget does).
        window.setAnimationBehavior(objc2_app_kit::NSWindowAnimationBehavior::Default);
    }
    if let Some(window) = crate::glass::window_of(view, mtm) {
        window.setAlphaValue(1.0);
        window.setIgnoresMouseEvents(false);
        window.setHasShadow(true);
        window.setAnimationBehavior(objc2_app_kit::NSWindowAnimationBehavior::Default);
    }
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
    /// nothing and takes nothing (rule 1109).
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
        // Rule 1109: on a screen, clear and click-through, it is safe.
        r.clear = true;
        assert!(r.problems().is_empty());
        let r = ParkReport {
            ex_style: 0,
            foreground: true,
            ..safe()
        };
        assert_eq!(r.problems().len(), 2);
    }

    // Rule 1107: the parked window's class keeps the frame it is given, and only that class changes.
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

    // Rule 1111: a class made born-clear initialises through the stand-in, which calls NSWindow's own.
    #[test]
    fn a_born_clear_class_initialises_through_the_stand_in() {
        use objc2::runtime::ClassBuilder;
        let window = AnyClass::get(c"NSWindow").expect("AppKit is linked");
        let class = ClassBuilder::new(c"FlareBornClearTestWindow", window)
            .expect("a new class name")
            .register();
        let sel4 = sel!(initWithContentRect:styleMask:backing:defer:);
        let own = window
            .instance_method(sel4)
            .expect("NSWindow has it")
            .implementation();
        born_clear(class);
        let now = class
            .instance_method(sel4)
            .expect("inherited")
            .implementation();
        assert!(
            !std::ptr::fn_addr_eq(now, own),
            "the class now has the stand-in"
        );
        assert_eq!(ORIGINAL_INIT4.get().copied(), Some(own as usize));
        assert!(ORIGINAL_INIT5.get().is_some());
        make_clear(std::ptr::null_mut());
    }

    #[test]
    fn off_the_main_thread_nothing_is_touched() {
        park(0);
        assert_eq!(report(0), ParkReport::default());
        // Rule 302: an unpark off the main thread lets go of nothing.
        unpark(0);
        assert!(is_held());
    }

    // Rule 302: held, the stand-ins keep a window clear and click-through; let go, they pass on the ask.
    #[test]
    fn a_held_park_keeps_windows_clear_and_a_released_one_passes_the_ask_on() {
        assert_eq!(alpha_to_pass(true, 1.0), 0.0);
        assert!(through_to_pass(true, false));
        assert_eq!(alpha_to_pass(false, 1.0), 1.0);
        assert_eq!(alpha_to_pass(false, 0.5), 0.5);
        assert!(!through_to_pass(false, false));
        assert!(through_to_pass(false, true));
    }

    // Rule 302: the unpark gives a window back what its own code last asked, opaque and taking clicks if nothing.
    #[test]
    fn a_window_gets_back_its_last_ask_and_opaque_with_clicks_when_it_asked_nothing() {
        assert_eq!(wanted_after(None, None, None), Wanted::default());
        assert_eq!(
            Wanted::default(),
            Wanted {
                alpha: 1.0,
                through: false
            }
        );
        let revealed = wanted_after(None, Some(1.0), None);
        let passing = wanted_after(Some(revealed), None, Some(true));
        assert_eq!(
            passing,
            Wanted {
                alpha: 1.0,
                through: true
            }
        );
        assert_eq!(wanted_after(Some(passing), Some(0.25), None).alpha, 0.25);
        assert!(wanted_after(Some(passing), Some(0.25), None).through);
    }
}
