//! The dictation widget's window on macOS (rule 1139): the small second window that shows "Listening",
//! "Transcribing" and the session picker ("Send to") while another app is in front. The Windows widget is a
//! `WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW` popup shown with `SW_SHOWNOACTIVATE`; this is the Mac
//! meaning of the same rules, on the window winit made for the Slint card:
//!
//! - a floating panel: the status-window level (above normal and floating windows), on every Space and over a
//!   full-screen app (`CanJoinAllSpaces | FullScreenAuxiliary | Stationary | IgnoresCycle`), never hidden when
//!   Flare is not the active app;
//! - it never activates Flare: AppKit's own non-activating-panel switch (`_setPreventsActivation:`, what an
//!   `NSPanel` with `NSWindowStyleMaskNonactivatingPanel` sets) when the system has it, so a click on a row reaches
//!   the card while the app in front stays in front; shown with `orderFrontRegardless`, never made key;
//! - clicks pass through it (`setIgnoresMouseEvents`) except while it is picking;
//! - placed in points from the top-left of the main screen (`Bounds` in the widget's terms, scale 1), at the
//!   bottom centre of the visible frame of the screen under the pointer.
//!
//! The handle every call takes is the window's content `NSView` (raw-window-handle's `AppKit` handle) as an
//! integer. Every call refuses (false, `None`) off the main thread or for 0.

use crate::glass::window_of;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSApplicationActivationOptions, NSEvent, NSEventMask, NSRunningApplication, NSScreen, NSWindow,
    NSWindowCollectionBehavior, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::cell::RefCell;
use std::ptr::NonNull;

/// `kCGStatusWindowLevel`: above normal and floating windows, below the menu bar's own menus.
pub const PANEL_LEVEL: isize = 25;

/// How the panel joins Spaces and full-screen apps.
pub fn panel_behavior() -> NSWindowCollectionBehavior {
    NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::IgnoresCycle
}

/// A rectangle in points, from the top-left of the main screen, y down (the widget's `Bounds` at scale 1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TopLeft {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Pure: an AppKit rectangle (origin at the main screen's bottom-left, y up) as a top-left one; `main_h` is the
/// main screen's height in points.
pub fn top_left_of(r: NSRect, main_h: f64) -> TopLeft {
    TopLeft {
        x: r.origin.x,
        y: main_h - (r.origin.y + r.size.height),
        w: r.size.width,
        h: r.size.height,
    }
}

/// Pure: the reverse of [`top_left_of`].
pub fn appkit_rect_of(t: TopLeft, main_h: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(t.x, main_h - (t.y + t.h)),
        NSSize::new(t.w, t.h),
    )
}

fn window(view: isize) -> Option<Retained<NSWindow>> {
    let mtm = MainThreadMarker::new()?;
    window_of(view, mtm)
}

fn responds(w: &NSWindow, s: Sel) -> bool {
    // SAFETY: `respondsToSelector:` is NSObject's, answered by every window.
    let r: Bool = unsafe { objc2::msg_send![w, respondsToSelector: s] };
    r.as_bool()
}

/// Whether AppKit has the non-activating switch on this system.
fn prevents_activation(w: &NSWindow) -> Option<bool> {
    if !responds(w, sel!(_preventsActivation)) {
        return None;
    }
    // SAFETY: a getter answering BOOL, checked above.
    let r: Bool = unsafe { objc2::msg_send![w, _preventsActivation] };
    Some(r.as_bool())
}

/// Gives the window the panel's level, Spaces behaviour and never-activate switch. False with no window.
pub fn prepare(view: isize) -> bool {
    let Some(w) = window(view) else {
        return false;
    };
    w.setLevel(PANEL_LEVEL);
    w.setCollectionBehavior(panel_behavior());
    w.setHidesOnDeactivate(false);
    if responds(&w, sel!(_setPreventsActivation:)) {
        // SAFETY: a BOOL setter, checked above; it only tags the window for the window server.
        let _: () = unsafe { objc2::msg_send![&*w, _setPreventsActivation: Bool::YES] };
    }
    true
}

/// Whether the window has every part of the panel's style (a missing switch counts as had: the system has none).
pub fn is_prepared(view: isize) -> bool {
    window(view).is_some_and(|w| {
        w.level() == PANEL_LEVEL
            && w.collectionBehavior().contains(panel_behavior())
            && prevents_activation(&w).unwrap_or(true)
    })
}

/// Whether a click on the window leaves the app in front where it is.
pub fn never_activates(view: isize) -> bool {
    window(view).is_some_and(|w| prevents_activation(&w) == Some(true))
}

/// Whether the window floats above other apps' windows.
pub fn is_floating(view: isize) -> bool {
    window(view).is_some_and(|w| w.level() >= PANEL_LEVEL)
}

/// Lets clicks fall through the window (pass) or catches them.
pub fn set_click_through(view: isize, pass: bool) {
    if let Some(w) = window(view) {
        w.setIgnoresMouseEvents(pass);
    }
}

/// Whether clicks fall through the window.
pub fn click_through(view: isize) -> bool {
    window(view).is_some_and(|w| w.ignoresMouseEvents())
}

/// The main screen's height in points (the top-left frame's flip).
fn main_height(mtm: MainThreadMarker) -> Option<f64> {
    NSScreen::screens(mtm)
        .firstObject()
        .map(|s| s.frame().size.height)
}

/// Moves and sizes the window to `t` (top-left points).
pub fn place(view: isize, t: TopLeft) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let (Some(w), Some(main_h)) = (window_of(view, mtm), main_height(mtm)) else {
        return;
    };
    w.setFrame_display(appkit_rect_of(t, main_h), true);
}

/// Where the window is now, top-left points.
pub fn frame(view: isize) -> Option<TopLeft> {
    let mtm = MainThreadMarker::new()?;
    let w = window_of(view, mtm)?;
    Some(top_left_of(w.frame(), main_height(mtm)?))
}

/// Shows the window above every app's windows without activating Flare or making the window key.
pub fn show(view: isize) {
    if let Some(w) = window(view) {
        w.orderFrontRegardless();
    }
}

pub fn hide(view: isize) {
    if let Some(w) = window(view) {
        w.orderOut(None);
    }
}

pub fn is_visible(view: isize) -> bool {
    window(view).is_some_and(|w| w.isVisible())
}

/// The visible frame (no menu bar, no Dock) of the screen under the pointer, top-left points. None off the main
/// thread or with no screen.
pub fn work_area_under_pointer() -> Option<TopLeft> {
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    let main_h = screens.firstObject()?.frame().size.height;
    let p = NSEvent::mouseLocation();
    let inside = |r: NSRect| {
        p.x >= r.origin.x
            && p.x < r.origin.x + r.size.width
            && p.y >= r.origin.y
            && p.y < r.origin.y + r.size.height
    };
    let screen = screens
        .iter()
        .find(|s| inside(s.frame()))
        .or_else(|| NSScreen::mainScreen(mtm))?;
    Some(top_left_of(screen.visibleFrame(), main_h))
}

// ----- the app in front -----

/// The process of the app in front (`NSWorkspace.frontmostApplication`).
pub fn front_app_pid() -> Option<u32> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    u32::try_from(app.processIdentifier())
        .ok()
        .filter(|p| *p != 0)
}

/// Whether Flare is the active app.
pub fn flare_active() -> bool {
    NSRunningApplication::currentApplication().isActive()
}

/// Gives the focus back to the app `pid` (after a click activated Flare on a system without the never-activate
/// switch). Only its main and key windows come forward. False when it is gone.
pub fn activate(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .is_some_and(|a| a.activateWithOptions(NSApplicationActivationOptions::empty()))
}

// ----- a click in another app -----

thread_local! {
    static AWAY: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

/// While on, a mouse press in another app calls `f` once (on the main thread); a press on Flare's own windows is
/// not seen. A global mouse monitor needs no permission. Off removes it. False when it could not be set.
pub fn watch_clicks_away(on: bool, f: impl Fn() + 'static) -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    AWAY.with(|slot| {
        if let Some(old) = slot.borrow_mut().take() {
            // SAFETY: the monitor this module added and has not removed.
            unsafe { NSEvent::removeMonitor(&old) };
        }
        if !on {
            return true;
        }
        let block = block2::RcBlock::new(move |_: NonNull<NSEvent>| f());
        let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown;
        let m = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &block);
        let ok = m.is_some();
        *slot.borrow_mut() = m;
        ok
    })
}

/// Whether a click-away watch is set.
pub fn watching_clicks_away() -> bool {
    AWAY.with(|s| s.borrow().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_left_and_appkit_rects_are_each_others_flip() {
        // A 1440 x 900 main screen; a card 480 x 196 at the foot of a visible frame.
        let r = NSRect::new(NSPoint::new(480.0, 16.0), NSSize::new(480.0, 196.0));
        let t = top_left_of(r, 900.0);
        assert_eq!(
            t,
            TopLeft {
                x: 480.0,
                y: 900.0 - 212.0,
                w: 480.0,
                h: 196.0
            }
        );
        assert_eq!(appkit_rect_of(t, 900.0), r);
    }

    #[test]
    fn a_screen_above_the_main_one_has_a_negative_top() {
        let above = NSRect::new(NSPoint::new(0.0, 900.0), NSSize::new(1920.0, 1080.0));
        assert_eq!(top_left_of(above, 900.0).y, -1080.0);
    }

    #[test]
    fn the_panel_joins_every_space_and_full_screen_apps_and_stays_out_of_the_cycle() {
        let b = panel_behavior();
        for want in [
            NSWindowCollectionBehavior::CanJoinAllSpaces,
            NSWindowCollectionBehavior::FullScreenAuxiliary,
            NSWindowCollectionBehavior::Stationary,
            NSWindowCollectionBehavior::IgnoresCycle,
        ] {
            assert!(b.contains(want), "{want:?}");
        }
        const { assert!(PANEL_LEVEL > 3, "above the floating level") };
    }
}
