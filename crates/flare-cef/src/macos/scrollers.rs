//! Overlay scrollbars in every page, as WebView2's Fluent overlay style draws them on Windows (thin bars over the
//! page that fade when it is still, no width taken from the layout). Chromium on a Mac takes the style from
//! `+[NSScroller preferredScrollerStyle]` in the browser process (sent to every renderer): with a mouse attached and
//! the system's "Show scroll bars: Automatically", that is the classic bar, which takes 15 px of the page's width.
//! So, before CEF starts, the method answers the overlay style for this process: no page is touched (no stylesheet
//! of Flare's in its DOM), and a page that styles its own scrollbars keeps them.

use std::sync::Once;

use objc2::ffi;
use objc2::runtime::{AnyClass, Imp, Sel};
use objc2::sel;

/// `NSScrollerStyleOverlay`.
pub const OVERLAY: isize = 1;

extern "C-unwind" fn overlay(_class: *const AnyClass, _sel: Sel) -> isize {
    OVERLAY
}

static ONCE: Once = Once::new();

/// Makes `+[NSScroller preferredScrollerStyle]` answer the overlay style in this process; once, before the engine
/// starts. False when AppKit has no such method (nothing changed).
pub fn prefer_overlay() -> bool {
    let mut done = false;
    ONCE.call_once(|| {
        let Some(class) = AnyClass::get(c"NSScroller") else {
            return;
        };
        // SAFETY: the method is a class method of NSScroller taking no argument and returning an NSInteger
        // (`NSScrollerStyle`); `overlay` has that signature (receiver, selector) -> isize.
        unsafe {
            let m = ffi::class_getClassMethod(class, sel!(preferredScrollerStyle));
            if m.is_null() {
                return;
            }
            let imp: Imp = std::mem::transmute::<
                extern "C-unwind" fn(*const AnyClass, Sel) -> isize,
                Imp,
            >(overlay);
            ffi::method_setImplementation(m, imp);
        }
        done = true;
    });
    done || preferred() == OVERLAY
}

/// What `+[NSScroller preferredScrollerStyle]` answers now.
pub fn preferred() -> isize {
    let Some(class) = AnyClass::get(c"NSScroller") else {
        return -1;
    };
    // SAFETY: a class method of NSScroller with no argument, returning an NSInteger.
    unsafe { objc2::msg_send![class, preferredScrollerStyle] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appkit_answers_the_overlay_style_once_flare_asked_for_it() {
        assert!(prefer_overlay());
        assert_eq!(preferred(), OVERLAY);
        // Asked again, it stays as it is.
        assert!(prefer_overlay());
        assert_eq!(preferred(), OVERLAY);
    }
}
