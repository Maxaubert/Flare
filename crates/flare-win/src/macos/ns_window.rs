//! The main window's `NSWindow` on macOS (port phase 3): finding it from the content view's handle, and
//! its dark or light Aqua appearance, the Mac counterpart of DWM's immersive dark mode
//! (`dwm::set_immersive_dark` lands here). The see-through glass that lived here was removed on
//! 2026-11-28; Liquid Glass on the Mac comes later as its own feature.
//!
//! The handle every call takes is the window's `NSView` (raw-window-handle's `AppKit` handle), as
//! an integer. Every call is cosmetic (rule 74): no main thread, no window or a refusal answers
//! false and the window stays as it was.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSView, NSWindow,
};

/// The window an `NSView` handle sits in. None off the main thread, for 0 or a view without one.
pub(crate) fn window_of(view: isize, mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    let _ = mtm;
    if view == 0 {
        return None;
    }
    // SAFETY: the caller passes the live content view of one of this process's windows (the
    // AppKit handle raw-window-handle gave it), and we are on the main thread, where AppKit
    // objects may be touched.
    let view: &NSView = unsafe { &*(view as *const NSView) };
    view.window()
}

/// The window's light or dark, and the traffic lights' with it: the window wears the dark or the
/// light Aqua appearance, so it follows the theme's ground the way DWM's immersive dark does.
pub fn set_window_dark(window: &NSWindow, dark: bool) -> bool {
    // SAFETY: the appearance names are AppKit's own constant strings.
    let name = unsafe {
        if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    };
    let Some(appearance) = NSAppearance::appearanceNamed(name) else {
        return false;
    };
    window.setAppearance(Some(&appearance));
    true
}

/// `dwm::set_immersive_dark` on macOS.
pub fn set_dark(view: isize, dark: bool) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    match window_of(view, mtm) {
        Some(w) => set_window_dark(&w, dark),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_the_main_thread_nothing_is_touched() {
        // Unit tests run on worker threads: every call refuses rather than touch AppKit there.
        assert!(!set_dark(0, true));
    }
}
