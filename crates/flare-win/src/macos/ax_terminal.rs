//! The terminal as an accessibility element on macOS (issue #201, rule 1187), so Wind, Zoom's "follow keyboard
//! focus" and VoiceOver follow typing. The rules (when to claim the focus, what to post, the geometry) are pure in
//! [`crate::ax_caret`]; this module is their AppKit side.
//!
//! The window's content view (winit's) is already dynamically subclassed by AccessKit, which Slint uses for the
//! window's own controls: AccessKit answers the view's `accessibilityChildren`, `accessibilityFocusedUIElement` and
//! `accessibilityHitTest:`. Flare puts one more dynamic subclass on top of whatever class the view has (the same
//! technique, no instance variables, the state in a thread-local), which:
//! - adds the terminal element to the view's children, after AccessKit's;
//! - answers `accessibilityFocusedUIElement` with the terminal element while the terminal holds the keyboard, and
//!   hands everything else to the class below (AccessKit's Slint tree), so a field, the panel or a dialog keep
//!   their own focus.
//!
//! The element is an `NSAccessibilityElement` with the role `AXTextArea`. Its value is the cursor's line, its
//! `AXSelectedTextRange` the cursor (zero length) and its `AXBoundsForRange` the cursor's cell on the screen: the
//! cell `place_ime` gives the IME, in Cocoa's screen space (converted by the view and its window, so every display
//! and backing scale is right). A focus change posts `AXFocusedUIElementChanged`, a move of the cell
//! `AXSelectedTextChanged`, once per change.
//!
//! Every call is the main thread's; off it, or with no view, nothing happens.

use crate::ax_caret::{AxCaretPlan, AxCell, AxEvent, caret_line, range_rect, view_rect};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibilityElement, NSAccessibilityFocusedUIElementChangedNotification,
    NSAccessibilityPostNotification, NSAccessibilityRoleDescription,
    NSAccessibilitySelectedTextChangedNotification, NSAccessibilityTextAreaRole, NSView,
};
use objc2_foundation::{NSArray, NSCopying, NSObject, NSPoint, NSRange, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::ffi::CString;
use std::rc::Rc;
use std::sync::Mutex;

/// What the terminal shows this frame: the cursor's cell, the grid's area (both device pixels from the view's top
/// left), the cursor's line and its column.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AxFrame {
    pub cell: AxCell,
    pub grid: AxCell,
    pub line: String,
    pub col: usize,
}

/// The element's label: what VoiceOver says for it.
pub const LABEL: &str = "Terminal";

/// The name prefix of the view classes this module makes.
const CLASS_PREFIX: &str = "FlareAXTerminalOf";

struct Ax {
    view_ptr: isize,
    view: Weak<NSView>,
    element: Retained<TerminalAx>,
    plan: AxCaretPlan,
    grid: AxCell,
    text: String,
    index: usize,
    /// Whether the terminal holds the keys inside the window right now (no field, panel or menu does), read when a
    /// tracker asks, so a focus that moved with no frame yet is still told right.
    keys: Option<Rc<dyn Fn() -> bool>>,
}

thread_local! {
    static AX: RefCell<Option<Ax>> = const { RefCell::new(None) };
}

/// The view subclasses made, each (ours, the class it was made on).
static SUBCLASSES: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

define_class!(
    // SAFETY: NSAccessibilityElement may be subclassed; the class adds no instance variables and no `Drop`. Its
    // state is the thread-local `AX`, read on the main thread only.
    #[unsafe(super(NSAccessibilityElement, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "FlareTerminalAXElement"]
    pub struct TerminalAx;

    impl TerminalAx {
        #[unsafe(method(isAccessibilityElement))]
        fn is_element(&self) -> bool {
            true
        }

        #[unsafe(method(isAccessibilityEnabled))]
        fn is_enabled(&self) -> bool {
            true
        }

        #[unsafe(method_id(accessibilityRole))]
        fn role(&self) -> Retained<NSString> {
            // SAFETY: an AppKit constant string.
            unsafe { NSAccessibilityTextAreaRole }.copy()
        }

        #[unsafe(method_id(accessibilityRoleDescription))]
        fn role_description(&self) -> Option<Retained<NSString>> {
            // SAFETY: an AppKit constant string.
            NSAccessibilityRoleDescription(unsafe { NSAccessibilityTextAreaRole }, None)
        }

        #[unsafe(method_id(accessibilityLabel))]
        fn label(&self) -> Retained<NSString> {
            NSString::from_str(LABEL)
        }

        #[unsafe(method(isAccessibilityFocused))]
        fn is_focused(&self) -> bool {
            with_ax(false, claimed)
        }

        #[unsafe(method_id(accessibilityParent))]
        fn parent(&self) -> Option<Retained<AnyObject>> {
            with_ax(None, |a| a.view.load()).map(|v| Retained::into_super(Retained::into_super(Retained::into_super(v))))
        }

        #[unsafe(method_id(accessibilityWindow))]
        fn window(&self) -> Option<Retained<AnyObject>> {
            window_of_ax()
        }

        #[unsafe(method_id(accessibilityTopLevelUIElement))]
        fn top_level(&self) -> Option<Retained<AnyObject>> {
            window_of_ax()
        }

        #[unsafe(method(accessibilityFrame))]
        fn frame(&self) -> NSRect {
            with_ax(NSRect::ZERO, |a| {
                a.view.load().map_or(NSRect::ZERO, |v| screen_rect(&v, a.grid))
            })
        }

        #[unsafe(method_id(accessibilityValue))]
        fn value(&self) -> Option<Retained<AnyObject>> {
            let text = with_ax(String::new(), |a| a.text.clone());
            Some(Retained::into_super(Retained::into_super(NSString::from_str(&text))))
        }

        #[unsafe(method(accessibilityNumberOfCharacters))]
        fn number_of_characters(&self) -> isize {
            with_ax(0, |a| utf16_len(&a.text))
        }

        #[unsafe(method(accessibilitySelectedTextRange))]
        fn selected_range(&self) -> NSRange {
            with_ax(NSRange::new(0, 0), |a| NSRange::new(a.index, 0))
        }

        #[unsafe(method_id(accessibilitySelectedText))]
        fn selected_text(&self) -> Retained<NSString> {
            NSString::new()
        }

        #[unsafe(method(accessibilityVisibleCharacterRange))]
        fn visible_range(&self) -> NSRange {
            with_ax(NSRange::new(0, 0), |a| NSRange::new(0, utf16_len(&a.text) as usize))
        }

        #[unsafe(method(accessibilityInsertionPointLineNumber))]
        fn insertion_line(&self) -> isize {
            0
        }

        #[unsafe(method(accessibilityLineForIndex:))]
        fn line_for_index(&self, _index: isize) -> isize {
            0
        }

        #[unsafe(method(accessibilityRangeForLine:))]
        fn range_for_line(&self, _line: isize) -> NSRange {
            with_ax(NSRange::new(0, 0), |a| NSRange::new(0, utf16_len(&a.text) as usize))
        }

        #[unsafe(method_id(accessibilityStringForRange:))]
        fn string_for_range(&self, range: NSRange) -> Option<Retained<NSString>> {
            let text = with_ax(String::new(), |a| a.text.clone());
            let units: Vec<u16> = text.encode_utf16().collect();
            let from = range.location.min(units.len());
            let to = range.location.saturating_add(range.length).min(units.len());
            Some(NSString::from_str(&String::from_utf16_lossy(&units[from..to])))
        }

        #[unsafe(method(accessibilityFrameForRange:))]
        fn frame_for_range(&self, range: NSRange) -> NSRect {
            with_ax(NSRect::ZERO, |a| {
                let (Some(v), Some(cell)) = (a.view.load(), a.plan.cell()) else {
                    return NSRect::ZERO;
                };
                screen_rect(&v, range_rect(cell, a.index, range.location, range.length))
            })
        }
    }
);

impl TerminalAx {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initializer on a freshly allocated instance of a subclass of it.
        unsafe { msg_send![super(this), init] }
    }
}

fn utf16_len(s: &str) -> isize {
    s.encode_utf16().count() as isize
}

/// Runs `f` on the state, or answers `none` with no state (or while it is in use: AppKit may ask again from inside
/// a notification).
fn with_ax<T>(none: T, f: impl FnOnce(&Ax) -> T) -> T {
    AX.with(|slot| match slot.try_borrow() {
        Ok(s) => s.as_ref().map_or(none, f),
        Err(_) => none,
    })
}

fn claimed(a: &Ax) -> bool {
    a.plan.claimed() && a.keys.as_ref().is_none_or(|k| k())
}

fn window_of_ax() -> Option<Retained<AnyObject>> {
    let v = with_ax(None, |a| a.view.load())?;
    v.window()
        .map(|w| Retained::into_super(Retained::into_super(Retained::into_super(w))))
}

/// A cell of device pixels from the view's top left, as a rectangle in Cocoa's screen space (y up from the bottom
/// of the main display). Zero when the view has no window.
pub fn screen_rect(view: &NSView, cell: AxCell) -> NSRect {
    let Some(win) = view.window() else {
        return NSRect::ZERO;
    };
    let b = view.bounds();
    let (x, y, w, h) = view_rect(
        cell,
        win.backingScaleFactor(),
        b.size.height,
        view.isFlipped(),
    );
    let r = NSRect::new(
        NSPoint::new(b.origin.x + x, b.origin.y + y),
        NSSize::new(w, h),
    );
    win.convertRectToScreen(view.convertRect_toView(r, None))
}

// ----- the view's subclass -----

/// The class this module's subclass of `this`'s class was made on (the class below it), if the view is hooked.
fn below(this: &AnyObject) -> Option<&'static AnyClass> {
    let made = SUBCLASSES.lock().ok()?.clone();
    let mut c = Some(this.class());
    while let Some(cls) = c {
        let p = cls as *const AnyClass as usize;
        if let Some(&(_, prev)) = made.iter().find(|(ours, _)| *ours == p) {
            // SAFETY: a class pointer this module read from the runtime; classes are never freed.
            return Some(unsafe { &*(prev as *const AnyClass) });
        }
        c = cls.superclass();
    }
    None
}

/// The terminal element of `this` view while the terminal claims the focus.
fn claimed_element(this: &AnyObject) -> Option<Retained<TerminalAx>> {
    let ptr = this as *const AnyObject as isize;
    with_ax(None, |a| {
        (a.view_ptr == ptr && claimed(a)).then(|| a.element.clone())
    })
}

fn element_of(this: &AnyObject) -> Option<Retained<TerminalAx>> {
    let ptr = this as *const AnyObject as isize;
    with_ax(None, |a| (a.view_ptr == ptr).then(|| a.element.clone()))
}

unsafe extern "C-unwind" fn children(this: &AnyObject, _cmd: Sel) -> *mut NSArray {
    let Some(prev) = below(this) else {
        return std::ptr::null_mut();
    };
    // SAFETY: `accessibilityChildren` of the class below, which the view is an instance of.
    let base: Option<Retained<NSArray>> =
        unsafe { msg_send![super(this, prev), accessibilityChildren] };
    let out = match (base, element_of(this)) {
        (Some(b), Some(e)) => Some(b.arrayByAddingObject(&Retained::into_super(
            Retained::into_super(Retained::into_super(e)),
        ))),
        (None, Some(e)) => Some(NSArray::from_retained_slice(&[Retained::into_super(
            Retained::into_super(Retained::into_super(e)),
        )])),
        (b, None) => b,
    };
    out.map_or(std::ptr::null_mut(), Retained::autorelease_return)
}

unsafe extern "C-unwind" fn focused(this: &AnyObject, _cmd: Sel) -> *mut AnyObject {
    if let Some(e) = claimed_element(this) {
        return Retained::autorelease_return(Retained::into_super(Retained::into_super(
            Retained::into_super(e),
        )));
    }
    let Some(prev) = below(this) else {
        return std::ptr::null_mut();
    };
    // SAFETY: `accessibilityFocusedUIElement` of the class below, which the view is an instance of.
    let r: Option<Retained<AnyObject>> =
        unsafe { msg_send![super(this, prev), accessibilityFocusedUIElement] };
    r.map_or(std::ptr::null_mut(), Retained::autorelease_return)
}

/// Puts this module's subclass on top of the view's class, once. True when the view is hooked.
fn hook(view: &NSView) -> bool {
    let obj: &AnyObject = view.as_ref();
    if below(obj).is_some() {
        return true;
    }
    let prev = obj.class();
    let Ok(mut made) = SUBCLASSES.lock() else {
        return false;
    };
    let prev_ptr = prev as *const AnyClass as usize;
    let ours = match made.iter().find(|(_, p)| *p == prev_ptr) {
        Some(&(ours, _)) => ours,
        None => {
            let Ok(name) = CString::new(format!("{CLASS_PREFIX}{}", prev.name().to_string_lossy()))
            else {
                return false;
            };
            let cls = match ClassBuilder::new(&name, prev) {
                Some(mut b) => {
                    // SAFETY: the signatures are NSView's (NSAccessibility) for these selectors.
                    unsafe {
                        b.add_method(
                            sel!(accessibilityChildren),
                            children as unsafe extern "C-unwind" fn(_, _) -> _,
                        );
                        b.add_method(
                            sel!(accessibilityFocusedUIElement),
                            focused as unsafe extern "C-unwind" fn(_, _) -> _,
                        );
                    }
                    b.register()
                }
                // Already registered (by an earlier run of this module on another view of the same class).
                None => match AnyClass::get(&name) {
                    Some(c) => c,
                    None => return false,
                },
            };
            let ours = cls as *const AnyClass as usize;
            made.push((ours, prev_ptr));
            ours
        }
    };
    drop(made);
    // SAFETY: the subclass adds no instance variables, so the view's memory fits it; its methods find their state
    // in the thread-local, never in the object.
    unsafe {
        objc2::ffi::object_setClass(
            obj as *const AnyObject as *mut AnyObject,
            ours as *const AnyClass,
        );
    }
    true
}

// ----- the calls -----

/// Sets how the element learns whether the terminal holds the keys inside the window (no field, panel or menu).
pub fn set_keys_probe(f: impl Fn() -> bool + 'static) {
    AX.with(|slot| {
        if let Ok(mut s) = slot.try_borrow_mut()
            && let Some(a) = s.as_mut()
        {
            a.keys = Some(Rc::new(f));
        }
    });
}

/// Whether a keys probe is set for the hooked view.
pub fn has_keys_probe() -> bool {
    with_ax(false, |a| a.keys.is_some())
}

/// The state for `view` (an `NSView` pointer), made and the view hooked on the first call; None off the main thread.
fn ensure(view: isize) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    if view == 0 {
        return false;
    }
    // SAFETY: the caller's live view (raw-window-handle's AppKit view), read on the main thread.
    let v: &NSView = unsafe { &*(view as *const NSView) };
    if !hook(v) {
        return false;
    }
    AX.with(|slot| {
        let Ok(mut s) = slot.try_borrow_mut() else {
            return false;
        };
        if s.as_ref()
            .is_some_and(|a| a.view_ptr == view && a.view.load().is_some())
        {
            return true;
        }
        *s = Some(Ax {
            view_ptr: view,
            view: Weak::from(v),
            element: TerminalAx::new(mtm),
            plan: AxCaretPlan::default(),
            grid: AxCell::default(),
            text: String::new(),
            index: 0,
            keys: None,
        });
        true
    })
}

/// Each frame: whether the terminal holds the keyboard (the window has it and no field, panel or menu does) and what
/// it shows (None with no terminal in front). Posts the notifications this makes and returns them.
pub fn sync(view: isize, focused: bool, frame: Option<AxFrame>) -> Vec<AxEvent> {
    if !ensure(view) {
        return Vec::new();
    }
    let step = AX.with(|slot| {
        let mut s = slot.try_borrow_mut().ok()?;
        let a = s.as_mut()?;
        let cell = frame.as_ref().map(|f| f.cell);
        if let Some(f) = frame {
            let (text, index) = caret_line(&f.line, f.col);
            a.grid = f.grid;
            a.text = text;
            a.index = index;
        }
        let events = a.plan.update(focused, cell);
        Some((events, a.element.clone(), a.view.load()))
    });
    let Some((events, element, view)) = step else {
        return Vec::new();
    };
    post(&events, &element, view.as_deref());
    events
}

/// The window gained or lost the keyboard (`on` false), or gained it with the terminal holding the keys (`on`
/// true). Posts and returns the notifications this makes.
pub fn focus(view: isize, on: bool) -> Vec<AxEvent> {
    if !ensure(view) {
        return Vec::new();
    }
    let step = AX.with(|slot| {
        let mut s = slot.try_borrow_mut().ok()?;
        let a = s.as_mut()?;
        Some((a.plan.focus(on), a.element.clone(), a.view.load()))
    });
    let Some((events, element, view)) = step else {
        return Vec::new();
    };
    post(&events, &element, view.as_deref());
    events
}

/// The terminal element of the hooked view, for checks.
pub fn element(view: isize) -> Option<Retained<NSAccessibilityElement>> {
    with_ax(None, |a| {
        (a.view_ptr == view).then(|| Retained::into_super(a.element.clone()))
    })
}

/// Posts the events, with no state borrowed (AppKit may ask the element again from inside).
fn post(events: &[AxEvent], element: &TerminalAx, view: Option<&NSView>) {
    for e in events {
        // SAFETY: AppKit's notification names, posted on the main thread for live objects.
        unsafe {
            match e {
                AxEvent::FocusGained => {
                    NSAccessibilityPostNotification(
                        element,
                        NSAccessibilityFocusedUIElementChangedNotification,
                    );
                }
                AxEvent::FocusLost => {
                    if let Some(v) = view {
                        NSAccessibilityPostNotification(
                            v,
                            NSAccessibilityFocusedUIElementChangedNotification,
                        );
                    }
                }
                AxEvent::CaretMoved => {
                    NSAccessibilityPostNotification(
                        element,
                        NSAccessibilitySelectedTextChangedNotification,
                    );
                }
            }
        }
    }
}
