//! The `NSApplication` CEF needs on macOS: one that implements `CefAppProtocol` (`isHandlingSendEvent` and
//! `setHandlingSendEvent:`, set around every `sendEvent:`), so Chromium knows when AppKit is dispatching an event.
//! It must be the process's shared application before anything else touches `NSApp`.
//!
//! For the engine's own browser processes (the probes) it is never shown: the activation policy is Prohibited (no
//! Dock tile, no menu bar, it cannot become active) and no window is ordered in ([`install`]). The real app makes it
//! the shared application before winit starts ([`install_for_app`]): winit 0.30 takes the shared `NSApplication`
//! as it finds it and swizzles `sendEvent:` on its class, so its own `sendEvent:` runs inside this one (the flag is
//! set around both), and the policy stays winit's. A quit from the system reaches `terminate:` as usual there.

use std::sync::atomic::{AtomicBool, Ordering};

use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol};
use objc2::{ClassType, MainThreadMarker, define_class, msg_send};
use objc2_app_kit::{NSApp, NSApplication, NSApplicationActivationPolicy, NSEvent};

/// Whether AppKit is inside `sendEvent:` (there is one shared application, so one flag).
static HANDLING_SEND_EVENT: AtomicBool = AtomicBool::new(false);

/// `terminate:` goes to AppKit (the real app) rather than being dropped (a probe, which ends on its own).
static TERMINATE_PASSES: AtomicBool = AtomicBool::new(false);

define_class!(
    /// `NSApplication` with the CEF protocols.
    #[unsafe(super(NSApplication))]
    #[name = "FlareCefApplication"]
    pub struct FlareCefApplication;

    impl FlareCefApplication {
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            let was = HANDLING_SEND_EVENT.swap(true, Ordering::SeqCst);
            // SAFETY: the superclass is NSApplication, whose sendEvent: takes an NSEvent.
            let _: () = unsafe { msg_send![super(self), sendEvent: event] };
            HANDLING_SEND_EVENT.store(was, Ordering::SeqCst);
        }

        /// A quit from the system (logout, Activity Monitor) must not `exit()` under a probe's Chromium: the probe
        /// ends on its own. In the real app it goes to AppKit, which asks the app's delegate as before.
        #[unsafe(method(terminate:))]
        fn terminate(&self, sender: Option<&AnyObject>) {
            if TERMINATE_PASSES.load(Ordering::SeqCst) {
                // SAFETY: the superclass is NSApplication, whose terminate: takes an optional sender.
                let _: () = unsafe { msg_send![super(self), terminate: sender] };
            }
        }
    }

    unsafe impl NSObjectProtocol for FlareCefApplication {}

    unsafe impl CrAppProtocol for FlareCefApplication {
        #[unsafe(method(isHandlingSendEvent))]
        unsafe fn _is_handling_send_event(&self) -> Bool {
            Bool::new(HANDLING_SEND_EVENT.load(Ordering::SeqCst))
        }
    }

    unsafe impl CrAppControlProtocol for FlareCefApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        unsafe fn _set_handling_send_event(&self, handling: Bool) {
            HANDLING_SEND_EVENT.store(handling.as_bool(), Ordering::SeqCst);
        }
    }

    unsafe impl CefAppProtocol for FlareCefApplication {}
);

fn make_shared(mtm: MainThreadMarker) -> Result<Retained<NSApplication>, String> {
    // SAFETY: sharedApplication on the class creates the shared instance of this class (the first call wins);
    // it returns the shared NSApplication, retained.
    let shared: Retained<NSApplication> =
        unsafe { msg_send![FlareCefApplication::class(), sharedApplication] };
    let app = NSApp(mtm);
    if !app.isKindOfClass(FlareCefApplication::class()) {
        return Err(format!(
            "NSApp is a {} already; CEF needs FlareCefApplication first",
            app.class().name().to_string_lossy()
        ));
    }
    Ok(shared)
}

/// Makes `FlareCefApplication` the real app's shared application, before winit (or anything) touches `NSApp`: the
/// policy and the windows stay winit's, `terminate:` goes to AppKit. Called once at the start of `main` on macOS.
pub fn install_for_app(mtm: MainThreadMarker) -> Result<Retained<NSApplication>, String> {
    TERMINATE_PASSES.store(true, Ordering::SeqCst);
    let app = make_shared(mtm)?;
    crate::macos::panels::guard();
    Ok(app)
}

/// The shared application, already a `FlareCefApplication` ([`install_for_app`] ran at the start), for an engine
/// that starts later in the real app. Fails when it is not: CEF would crash on its first event.
pub fn adopt(mtm: MainThreadMarker) -> Result<Retained<NSApplication>, String> {
    let app = NSApp(mtm);
    if !app.isKindOfClass(FlareCefApplication::class()) {
        return Err(format!(
            "NSApp is a {}, not FlareCefApplication: install_for_app must run before the window is made",
            app.class().name().to_string_lossy()
        ));
    }
    Ok(app)
}

/// Makes `FlareCefApplication` the shared application, headless: activation policy Prohibited. Fails when another
/// class already holds `NSApp` (something touched it first), since CEF would then crash on its first event.
pub fn install(mtm: MainThreadMarker) -> Result<Retained<NSApplication>, String> {
    let shared = make_shared(mtm)?;
    // No Dock tile, no menu bar, never active: the screen rule.
    shared.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    // No colour or print panel a page could ask for (rule 265).
    crate::macos::panels::guard();
    Ok(shared)
}

/// Adds Chromium's protocols to the class when it was registered before the framework was loaded (the real app
/// makes its application at the start of `main`, the framework loads with the first web panel): the protocol
/// objects only exist once the framework is in, and objc2 leaves out a protocol it cannot find.
pub fn ensure_protocols(app: &NSApplication) {
    for name in [c"CrAppProtocol", c"CrAppControlProtocol"] {
        if let Some(p) = objc2::runtime::AnyProtocol::get(name)
            && !app.conformsToProtocol(p)
        {
            let cls: *const objc2::runtime::AnyClass = FlareCefApplication::class();
            // SAFETY: adding a protocol whose methods the class already implements (`define_class!` above) to a
            // registered class, on the main thread, before CEF looks at it.
            unsafe { objc2::ffi::class_addProtocol(cls.cast_mut(), p) };
        }
    }
}

/// Whether the shared application implements what CEF asks of it (the probe reports it): it answers both
/// selectors, and it conforms to `CrAppProtocol` and `CrAppControlProtocol`, the protocols Chromium's message pump
/// checks (they exist at run time once the framework is loaded; `CefAppProtocol` itself is a header-only name with
/// no run-time protocol object, so it cannot be asked for).
pub fn conforms(app: &NSApplication) -> bool {
    conformance(app).iter().all(|(_, ok)| *ok)
}

/// [`conforms`], part by part.
pub fn conformance(app: &NSApplication) -> Vec<(&'static str, bool)> {
    let has = |name: &std::ffi::CStr| {
        // `conformsToProtocol:` walks the superclasses (AppKit may have put a KVO subclass in front of the class);
        // `class_conformsToProtocol` would not.
        objc2::runtime::AnyProtocol::get(name).is_some_and(|p| app.conformsToProtocol(p))
    };
    vec![
        ("CrAppProtocol", has(c"CrAppProtocol")),
        ("CrAppControlProtocol", has(c"CrAppControlProtocol")),
        (
            "FlareCefApplication",
            app.isKindOfClass(FlareCefApplication::class()),
        ),
        (
            "isHandlingSendEvent",
            app.respondsToSelector(objc2::sel!(isHandlingSendEvent)),
        ),
        (
            "setHandlingSendEvent:",
            app.respondsToSelector(objc2::sel!(setHandlingSendEvent:)),
        ),
    ]
}
