//! macOS notifications through `UNUserNotificationCenter` (port phase 8, step 2).
//!
//! [`SystemCenter`] is the real [`crate::toast::NotificationCenter`]: what Flare's permission is
//! (`getNotificationSettingsWithCompletionHandler:`, which never prompts), the one system prompt
//! (`requestAuthorizationWithOptions:completionHandler:`, asked by [`crate::toast::CenterBackend`] only, once a run,
//! the first time Flare has something to say), a request per tab (`addNotificationRequest:`, the identifier is the
//! tab's, so a newer one replaces the older), and the click (`FlareNotificationDelegate`, the center's delegate,
//! set only once the first notification is posted).
//!
//! `UNUserNotificationCenter` raises an Objective-C exception in a process that is not an app bundle (a `cargo run`,
//! a test binary), which would abort Flare. So nothing here is called unless [`bundle_facts`] says Flare runs from
//! its own signed `Flare.app` (`toast::bundle_verdict`); the tests use a fake center and never reach this file's
//! center calls (a real permission request is a system prompt on the person's screen).
//!
//! Every call waits on its completion handler with a bounded wait on the notification worker, never on the UI
//! thread.

use std::ffi::c_void;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use block2::{Block, RcBlock};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject};
use objc2::{AllocAnyThread, define_class, msg_send};
use objc2_foundation::{NSBundle, NSDictionary, NSError, NSObjectProtocol, NSString};

use crate::toast::{
    ClickRoute, Clicked, KEY_SESSION, KEY_TAB, NotificationCenter, Permission, Posted, THREAD,
};

// The framework is loaded with Flare, so its classes are there to be looked up by name.
#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecCodeCopySelf(flags: u32, code: *mut *mut c_void) -> i32;
    fn SecCodeCheckValidity(code: *mut c_void, flags: u32, requirement: *mut c_void) -> i32;
    fn SecRequirementCreateWithString(
        text: *const c_void,
        flags: u32,
        req: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// How long a call waits for the center's answer. The permission prompt waits for the person, so it gets longer.
const ANSWER: Duration = Duration::from_secs(10);
const PROMPT: Duration = Duration::from_secs(600);

/// `UNAuthorizationOptionSound | UNAuthorizationOptionAlert`: a banner with Windows' default sound, no badge (the
/// Dock badge is Flare's own, `dock.rs`).
const ASK_FOR: usize = (1 << 1) | (1 << 2);
/// `UNNotificationPresentationOptionSound | List | Banner`: shown while Flare is in front too, as Windows does.
const PRESENT: usize = (1 << 1) | (1 << 3) | (1 << 4);
/// The click on the notification itself (not a dismiss or an action of its own; Flare's have none).
const DEFAULT_ACTION: &str = "com.apple.UNNotificationDefaultActionIdentifier";

/// What the process knows about the bundle it runs from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleFacts {
    /// The main bundle's folder (a plain executable's own folder when it is not in a bundle).
    pub path: Option<String>,
    pub id: Option<String>,
    /// `SecCodeCheckValidity` on the running code against [`REQUIREMENT`]: Ok, or the OSStatus.
    pub signature: Result<(), i32>,
}

/// Reads the main bundle and checks this process's own signature. No prompt, no window, no keychain. It can take
/// half a second on a cold start (measured 2026-11-09 on a debug test binary: about 0.5 s for `NSBundle`, 0.5 s for
/// the signature), so it runs on a worker, never on the UI thread (`toast::warm_up`).
pub fn bundle_facts() -> BundleFacts {
    autoreleasepool(|_| {
        let bundle = NSBundle::mainBundle();
        let path = Some(bundle.bundlePath().to_string());
        let id = bundle.bundleIdentifier().map(|s| s.to_string());
        BundleFacts {
            path,
            id,
            signature: own_signature(),
        }
    })
}

/// What the running code's signature must say: Flare's identifier, and an Info.plist bound into the signature that
/// names Flare (the linker's own signature of a bare executable has neither, so a bundle that was never signed is
/// refused).
pub const REQUIREMENT: &str =
    "identifier \"com.flare.terminal\" and info [CFBundleIdentifier] = \"com.flare.terminal\"";

fn own_signature() -> Result<(), i32> {
    let text = NSString::from_str(REQUIREMENT);
    let mut req: *mut c_void = std::ptr::null_mut();
    // SAFETY: an NSString is a CFString (toll-free bridged); `req` is a valid out pointer, released below.
    let st = unsafe { SecRequirementCreateWithString(Retained::as_ptr(&text).cast(), 0, &mut req) };
    if st != 0 || req.is_null() {
        return Err(st);
    }
    let r = check_self(req);
    // SAFETY: released once, it came from a Create call.
    unsafe { CFRelease(req) };
    r
}

fn check_self(req: *mut c_void) -> Result<(), i32> {
    let mut code: *mut c_void = std::ptr::null_mut();
    // SAFETY: `code` is a valid out pointer; a returned SecCodeRef is released below.
    let st = unsafe { SecCodeCopySelf(0, &mut code) };
    if st != 0 || code.is_null() {
        return Err(st);
    }
    // SAFETY: `code` and `req` are live Security objects.
    let st = unsafe { SecCodeCheckValidity(code, 0, req) };
    // SAFETY: released once, it came from a Copy call.
    unsafe { CFRelease(code) };
    if st == 0 { Ok(()) } else { Err(st) }
}

/// The route a click takes, set once [`SystemCenter::route_clicks`] has run.
static ROUTE: Mutex<Option<ClickRoute>> = Mutex::new(None);
/// The delegate object, kept alive for the run (the center holds its delegate weakly).
static DELEGATE: OnceLock<usize> = OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Delegate` has no `Drop`; its methods only read their
    // arguments and call the completion handler once.
    #[unsafe(super(NSObject))]
    #[name = "FlareNotificationDelegate"]
    struct Delegate;

    impl Delegate {
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(&self, _center: *mut AnyObject, response: *mut AnyObject, done: &Block<dyn Fn()>) {
            // SAFETY: the center hands a live UNNotificationResponse (or nil, which `clicked_from` refuses).
            if let Some(c) = unsafe { clicked_from(response) } {
                let route = ROUTE.lock().ok().and_then(|r| r.clone());
                if let Some(route) = route {
                    route(c);
                }
            }
            done.call(());
        }

        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(&self, _center: *mut AnyObject, _n: *mut AnyObject, done: &Block<dyn Fn(usize)>) {
            done.call((PRESENT,));
        }
    }

    unsafe impl NSObjectProtocol for Delegate {}
);

/// The click's notification id, its session and its tab, or None for anything but a click on one of Flare's.
///
/// # Safety
/// `response` is nil or a live `UNNotificationResponse`.
unsafe fn clicked_from(response: *mut AnyObject) -> Option<Clicked> {
    // SAFETY: the caller's promise; every object read is a property of the response the center owns.
    unsafe {
        let response = response.as_ref()?;
        let action: Option<Retained<NSString>> = msg_send![response, actionIdentifier];
        if action?.to_string() != DEFAULT_ACTION {
            return None;
        }
        let n: Option<Retained<AnyObject>> = msg_send![response, notification];
        let n = n?;
        let req: Option<Retained<AnyObject>> = msg_send![&*n, request];
        let req = req?;
        let id: Option<Retained<NSString>> = msg_send![&*req, identifier];
        let content: Option<Retained<AnyObject>> = msg_send![&*req, content];
        let info: Option<Retained<NSDictionary<NSString, AnyObject>>> =
            msg_send![&*content?, userInfo];
        let info = info?;
        let text = |key: &str| -> Option<String> {
            let v = info.objectForKey(&NSString::from_str(key))?;
            v.downcast::<NSString>().ok().map(|s| s.to_string())
        };
        Some(Clicked {
            id: id?.to_string(),
            session: text(KEY_SESSION)?,
            tab: text(KEY_TAB)?,
        })
    }
}

/// The real center. Make one only after `toast::bundle_verdict` said yes: the center aborts an unbundled process.
pub struct SystemCenter;

fn class(name: &std::ffi::CStr) -> Result<&'static AnyClass, String> {
    AnyClass::get(name).ok_or_else(|| format!("{} is not loaded", name.to_string_lossy()))
}

fn center() -> Result<Retained<AnyObject>, String> {
    let cls = class(c"UNUserNotificationCenter")?;
    // SAFETY: a class method with no arguments that returns the shared center.
    let c: Option<Retained<AnyObject>> = unsafe { msg_send![cls, currentNotificationCenter] };
    c.ok_or_else(|| "no notification center".to_string())
}

fn error_text(e: *mut NSError) -> Option<String> {
    // SAFETY: nil or the live NSError the completion handler was given.
    let e = unsafe { e.as_ref() }?;
    Some(e.localizedDescription().to_string())
}

impl NotificationCenter for SystemCenter {
    fn permission(&self) -> Result<Permission, String> {
        autoreleasepool(|_| {
            let c = center()?;
            let (tx, rx) = mpsc::sync_channel::<isize>(1);
            let block = RcBlock::new(move |settings: *mut AnyObject| {
                // SAFETY: nil or the live UNNotificationSettings the center hands the handler.
                let s = match unsafe { settings.as_ref() } {
                    // SAFETY: a property of the settings object, an NSInteger.
                    Some(s) => unsafe { msg_send![s, authorizationStatus] },
                    None => -1,
                };
                let _ = tx.try_send(s);
            });
            // SAFETY: the method takes one block; the block lives until the center has called it.
            let _: () =
                unsafe { msg_send![&*c, getNotificationSettingsWithCompletionHandler: &*block] };
            let s = rx.recv_timeout(ANSWER); // sync-ok: the notification worker waits for the settings, at most ANSWER
            let s = s.map_err(|_| "the notification settings did not come".to_string())?;
            Ok(permission_of(s))
        })
    }

    fn ask_permission(&self) -> Result<bool, String> {
        autoreleasepool(|_| {
            let c = center()?;
            let (tx, rx) = mpsc::sync_channel::<Result<bool, String>>(1);
            let block = RcBlock::new(move |granted: Bool, err: *mut NSError| {
                let _ = tx.try_send(match error_text(err) {
                    Some(e) if !granted.as_bool() => Err(e),
                    _ => Ok(granted.as_bool()),
                });
            });
            // SAFETY: options (NSUInteger) and a block; the block lives until the center has called it.
            let _: () = unsafe {
                msg_send![&*c, requestAuthorizationWithOptions: ASK_FOR, completionHandler: &*block]
            };
            // sync-ok: the notification worker waits for the person's answer to the prompt, at most PROMPT
            rx.recv_timeout(PROMPT)
                .map_err(|_| "no answer to the notification permission".to_string())?
        })
    }

    fn post(&self, n: &Posted) -> Result<(), String> {
        autoreleasepool(|_| {
            let c = center()?;
            let content_cls = class(c"UNMutableNotificationContent")?;
            let request_cls = class(c"UNNotificationRequest")?;
            let sound_cls = class(c"UNNotificationSound")?;
            let keys = [
                &*NSString::from_str(KEY_SESSION),
                &*NSString::from_str(KEY_TAB),
            ];
            let session = NSString::from_str(&n.session);
            let tab = NSString::from_str(&n.tab);
            let info = NSDictionary::<NSString, NSString>::from_slices(&keys, &[&*session, &*tab]);
            let (tx, rx) = mpsc::sync_channel::<Result<(), String>>(1);
            let block = RcBlock::new(move |err: *mut NSError| {
                let _ = tx.try_send(match error_text(err) {
                    Some(e) => Err(e),
                    None => Ok(()),
                });
            });
            // SAFETY: plain property setters and class constructors of UserNotifications, each with an object of
            // the type it takes; the request's trigger is nil (deliver now).
            unsafe {
                let content: Option<Retained<AnyObject>> = msg_send![content_cls, new];
                let content = content.ok_or("no notification content")?;
                let _: () = msg_send![&*content, setTitle: &*NSString::from_str(&n.title)];
                let _: () = msg_send![&*content, setBody: &*NSString::from_str(&n.body)];
                let _: () = msg_send![&*content, setThreadIdentifier: &*NSString::from_str(THREAD)];
                let _: () = msg_send![&*content, setUserInfo: &*info];
                let sound: Option<Retained<AnyObject>> = msg_send![sound_cls, defaultSound];
                if let Some(sound) = sound {
                    let _: () = msg_send![&*content, setSound: &*sound];
                }
                let req: Option<Retained<AnyObject>> = msg_send![
                    request_cls,
                    requestWithIdentifier: &*NSString::from_str(&n.id),
                    content: &*content,
                    trigger: std::ptr::null_mut::<AnyObject>()
                ];
                let req = req.ok_or("no notification request")?;
                let _: () =
                    msg_send![&*c, addNotificationRequest: &*req, withCompletionHandler: &*block];
            }
            // sync-ok: the notification worker waits for the center's answer, at most ANSWER
            rx.recv_timeout(ANSWER)
                .map_err(|_| "the notification center did not answer".to_string())?
        })
    }

    fn route_clicks(&self, route: ClickRoute) {
        if let Ok(mut r) = ROUTE.lock() {
            *r = Some(route);
        }
        autoreleasepool(|_| {
            let Ok(c) = center() else { return };
            let d = DELEGATE.get_or_init(|| {
                let this = Delegate::alloc().set_ivars(());
                // SAFETY: NSObject's plain init on a freshly allocated object.
                let d: Retained<Delegate> = unsafe { msg_send![super(this), init] };
                Retained::into_raw(d) as usize
            });
            // SAFETY: the delegate was leaked above and lives for the run; the center keeps it weakly.
            let _: () = unsafe { msg_send![&*c, setDelegate: *d as *mut AnyObject] };
        });
    }
}

/// `UNAuthorizationStatus` as Flare reads it: provisional and ephemeral are allowed (they deliver).
pub fn permission_of(status: isize) -> Permission {
    match status {
        0 => Permission::NotAsked,
        2..=4 => Permission::Allowed,
        _ => Permission::Denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::{ClassType, sel};

    #[test]
    fn the_framework_is_loaded_and_the_delegate_answers_both_center_calls() {
        // Looked up only; no center is made (a test binary is no bundle, and asking would be a prompt).
        for name in [
            c"UNUserNotificationCenter",
            c"UNMutableNotificationContent",
            c"UNNotificationRequest",
            c"UNNotificationSound",
        ] {
            assert!(class(name).is_ok(), "{name:?}");
        }
        let cls = Delegate::class();
        assert_eq!(cls.name().to_str().unwrap(), "FlareNotificationDelegate");
        for s in [
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
        ] {
            assert!(cls.instance_method(s).is_some(), "{s:?}");
        }
        // SAFETY: nil is allowed.
        assert_eq!(unsafe { clicked_from(std::ptr::null_mut()) }, None);
    }

    #[test]
    fn the_status_is_read_as_flare_uses_it_and_the_requirement_names_flare() {
        assert_eq!(permission_of(0), Permission::NotAsked);
        assert_eq!(permission_of(1), Permission::Denied);
        for s in 2..=4 {
            assert_eq!(permission_of(s), Permission::Allowed);
        }
        assert_eq!(permission_of(-1), Permission::Denied);
        assert_eq!(REQUIREMENT.matches(crate::names::AUMID).count(), 2);
        // A banner, the list and the sound, also while Flare is in front; no badge (the Dock badge is Flare's).
        assert_eq!(PRESENT, 0b11010);
        assert_eq!(ASK_FOR & 1, 0);
    }

    #[test]
    fn a_test_binary_reads_its_bundle_facts_without_a_prompt() {
        let f = bundle_facts();
        assert!(f.path.is_some());
        assert_ne!(f.id.as_deref(), Some(crate::names::AUMID));
        // The test binary carries the linker's signature only: Flare's requirement is not met.
        assert!(f.signature.is_err());
    }
}
