//! The native panels Chromium would put on the person's screen for a page, taken out of the process (rule 261).
//! A windowless browser has no window to hang a sheet on, so on a Mac Chromium shows AppKit's shared panels as
//! windows of their own: `<input type=color>` orders in the shared `NSColorPanel`, and printing (`window.print()`,
//! Cmd+P) runs `NSPrintPanel` modally. Printing is also off in every profile (`printing.enabled`), so it is never
//! asked for; the colour picker has no switch. [`guard`] replaces the methods that would show these panels, on
//! those classes only (every other window, the app's own, is untouched): the panel stays out, the page's picker
//! simply never opens, a print is cancelled. [`installed`] says whether every replacement is in place, so a test
//! asks for a panel only once it cannot appear.

use std::ffi::CStr;
use std::sync::Once;
use std::sync::atomic::{AtomicU32, Ordering};

use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::sel;

/// How many times a colour panel was asked to come on screen and stayed off.
static COLOR_BLOCKED: AtomicU32 = AtomicU32::new(0);
/// How many print, page layout or print operation runs were cancelled before a panel showed.
static PRINT_BLOCKED: AtomicU32 = AtomicU32::new(0);

static GUARD: Once = Once::new();

extern "C-unwind" fn color_no_arg(_this: *mut AnyObject, _cmd: Sel) {
    COLOR_BLOCKED.fetch_add(1, Ordering::SeqCst);
}

extern "C-unwind" fn color_one_arg(_this: *mut AnyObject, _cmd: Sel, _sender: *mut AnyObject) {
    COLOR_BLOCKED.fetch_add(1, Ordering::SeqCst);
}

extern "C-unwind" fn color_order(_this: *mut AnyObject, _cmd: Sel, _mode: isize, _other: isize) {
    COLOR_BLOCKED.fetch_add(1, Ordering::SeqCst);
}

extern "C-unwind" fn color_visible(_this: *mut AnyObject, _cmd: Sel, flag: Bool) {
    if flag.as_bool() {
        COLOR_BLOCKED.fetch_add(1, Ordering::SeqCst);
    }
}

/// `NSModalResponseCancel` (0): the panel was dismissed.
extern "C-unwind" fn print_cancel_no_arg(_this: *mut AnyObject, _cmd: Sel) -> isize {
    PRINT_BLOCKED.fetch_add(1, Ordering::SeqCst);
    0
}

extern "C-unwind" fn print_cancel_one_arg(
    _this: *mut AnyObject,
    _cmd: Sel,
    _info: *mut AnyObject,
) -> isize {
    PRINT_BLOCKED.fetch_add(1, Ordering::SeqCst);
    0
}

extern "C-unwind" fn print_run_refused(_this: *mut AnyObject, _cmd: Sel) -> Bool {
    PRINT_BLOCKED.fetch_add(1, Ordering::SeqCst);
    Bool::NO
}

/// One replacement: the class, the selector, the implementation and its type encoding.
struct Replacement {
    class: &'static CStr,
    sel: Sel,
    imp: Imp,
    types: &'static CStr,
}

fn replacements() -> Vec<Replacement> {
    // SAFETY: every transmute turns an `extern "C-unwind"` function pointer into `Imp`, the runtime's untyped
    // implementation pointer; the type encoding given with it says how the runtime calls it.
    unsafe {
        let no_arg: Imp =
            std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel), Imp>(color_no_arg);
        let one_arg: Imp = std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject),
            Imp,
        >(color_one_arg);
        let order: Imp = std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel, isize, isize),
            Imp,
        >(color_order);
        let visible: Imp = std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel, Bool),
            Imp,
        >(color_visible);
        let cancel0: Imp = std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel) -> isize,
            Imp,
        >(print_cancel_no_arg);
        let cancel1: Imp = std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> isize,
            Imp,
        >(print_cancel_one_arg);
        let refused: Imp = std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool,
            Imp,
        >(print_run_refused);
        let r = |class, sel, imp, types| Replacement {
            class,
            sel,
            imp,
            types,
        };
        vec![
            r(c"NSColorPanel", sel!(orderFront:), one_arg, c"v@:@"),
            r(
                c"NSColorPanel",
                sel!(makeKeyAndOrderFront:),
                one_arg,
                c"v@:@",
            ),
            r(c"NSColorPanel", sel!(orderFrontRegardless), no_arg, c"v@:"),
            r(c"NSColorPanel", sel!(makeKeyWindow), no_arg, c"v@:"),
            r(
                c"NSColorPanel",
                sel!(orderWindow:relativeTo:),
                order,
                c"v@:qq",
            ),
            r(c"NSColorPanel", sel!(setIsVisible:), visible, c"v@:B"),
            r(c"NSPrintPanel", sel!(runModal), cancel0, c"q@:"),
            r(
                c"NSPrintPanel",
                sel!(runModalWithPrintInfo:),
                cancel1,
                c"q@:@",
            ),
            r(c"NSPageLayout", sel!(runModal), cancel0, c"q@:"),
            r(
                c"NSPageLayout",
                sel!(runModalWithPrintInfo:),
                cancel1,
                c"q@:@",
            ),
            r(c"NSPrintOperation", sel!(runOperation), refused, c"B@:"),
        ]
    }
}

/// Puts the replacements in, once per process (on the main thread, before CEF is started).
pub fn guard() {
    GUARD.call_once(|| {
        for r in replacements() {
            if let Some(cls) = AnyClass::get(r.class) {
                let cls: *const AnyClass = cls;
                // SAFETY: replacing (or adding, on this class only) an instance method of an AppKit class with a
                // function of the signature its type encoding states, before anything calls it.
                unsafe {
                    objc2::ffi::class_replaceMethod(cls.cast_mut(), r.sel, r.imp, r.types.as_ptr());
                }
            }
        }
    });
}

/// Whether every replacement is the method the class answers with now.
pub fn installed() -> bool {
    replacements().iter().all(|r| {
        AnyClass::get(r.class)
            .and_then(|c| c.instance_method(r.sel))
            .is_some_and(|m| std::ptr::fn_addr_eq(m.implementation(), r.imp))
    })
}

/// (colour panels kept off the screen, print runs cancelled) so far in this process.
pub fn blocked() -> (u32, u32) {
    (
        COLOR_BLOCKED.load(Ordering::SeqCst),
        PRINT_BLOCKED.load(Ordering::SeqCst),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panels_are_replaced_on_their_own_classes_and_nothing_else() {
        // AppKit in: its classes are what is replaced.
        let _ = <objc2_app_kit::NSApplication as objc2::ClassType>::class();
        guard();
        assert!(installed(), "every replacement is in place");
        // NSWindow itself keeps AppKit's own ordering: the app's windows are untouched.
        let window = AnyClass::get(c"NSWindow").expect("AppKit is loaded");
        let own = window
            .instance_method(sel!(orderFront:))
            .expect("NSWindow orders windows in")
            .implementation();
        let ours = replacements()[0].imp;
        assert!(!std::ptr::fn_addr_eq(own, ours));
    }
}
