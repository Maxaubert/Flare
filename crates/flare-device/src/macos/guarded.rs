//! Every private call of the sim host goes through here (rule 1310): a class or selector is looked up and checked
//! before use, the call runs inside `objc2::exception::catch`, and a call that answers by callback gets a 5 s limit.
//! What fails is [`Broken`], with the symbol's name, never a crash.
//!
//! Private objects (CoreSimulator's `ROCKRemoteProxy` ports and screens among them) are sent plain messages through
//! `objc_msgSend` cast to the method's C signature ([`send!`]), never `msg_send!` (whose debug check refuses a method
//! a proxy only forwards) and never KVC (`valueForKey:` throws on a proxy).

use std::ffi::CString;
use std::fmt;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc;
use std::time::Duration;

use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};

/// The limit on a private call that answers by callback (enumerate screens, AX requests, liveness).
pub const CALLBACK_LIMIT: Duration = Duration::from_secs(5);

/// Why a private call could not be made or did not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// The class, protocol or selector is not there (a new Xcode renamed it).
    Missing,
    /// It raised an Objective-C exception: its name and reason.
    Exception(String),
    /// It answered by callback, but not within the limit.
    Timeout,
    /// It returned nil, or its callback an error, where a value was needed.
    Nothing(String),
}

/// A private call that failed: the symbol and why. Shown in the still line and the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Broken {
    pub symbol: String,
    pub why: Why,
}

impl Broken {
    pub fn missing(symbol: &str) -> Broken {
        Broken {
            symbol: symbol.to_string(),
            why: Why::Missing,
        }
    }

    pub fn nothing(symbol: &str, detail: impl Into<String>) -> Broken {
        Broken {
            symbol: symbol.to_string(),
            why: Why::Nothing(detail.into()),
        }
    }

    /// The still line's reason, with the loaded CoreSimulator's version: "SimDevice missing in CoreSimulator 1174.21".
    pub fn in_coresim(&self, coresim_version: &str) -> String {
        match &self.why {
            Why::Missing => format!("{} missing in CoreSimulator {coresim_version}", self.symbol),
            _ => format!("{self} (CoreSimulator {coresim_version})"),
        }
    }
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.why {
            Why::Missing => write!(f, "{} missing", self.symbol),
            Why::Exception(e) => write!(f, "{} raised {e}", self.symbol),
            Why::Timeout => write!(
                f,
                "{} gave no answer in {} s",
                self.symbol,
                CALLBACK_LIMIT.as_secs()
            ),
            Why::Nothing(d) if d.is_empty() => write!(f, "{} returned nothing", self.symbol),
            Why::Nothing(d) => write!(f, "{} returned nothing: {d}", self.symbol),
        }
    }
}

/// Sends `$sel` to `$obj` through `objc_msgSend` cast to the method's C signature:
/// `send!(obj, sel => Ret, arg => ArgType, ...)`. `obj` is a `*mut AnyObject` (a class too, cast). Unsafe in effect:
/// the caller vouches for the signature (each use names the private method's type encoding it matches) and runs it
/// inside [`guarded`].
macro_rules! send {
    ($obj:expr, $sel:expr => $ret:ty $(, $arg:expr => $aty:ty)* $(,)?) => {{
        type F = unsafe extern "C-unwind" fn(*mut ::objc2::runtime::AnyObject, ::objc2::runtime::Sel $(, $aty)*) -> $ret;
        // SAFETY: objc_msgSend is called with the receiver, the selector and the arguments of the method's own C
        // signature, which the caller states (the method's type encoding), on the platform's calling convention;
        // `guarded` checked that the receiver answers the selector.
        unsafe { ::std::mem::transmute::<unsafe extern "C-unwind" fn(), F>(::objc2::ffi::objc_msgSend)($obj, $sel $(, $arg)*) }
    }};
}
pub(crate) use send;

/// A class by name, or [`Broken`] naming it.
pub fn class(name: &str) -> Result<&'static AnyClass, Broken> {
    let c = CString::new(name).map_err(|_| Broken::missing(name))?;
    AnyClass::get(&c).ok_or_else(|| Broken::missing(name))
}

/// The selector of that name (registering a name never fails; whether an object answers it is [`responds`]).
pub fn sel(name: &str) -> Sel {
    let c = CString::new(name).unwrap_or_default();
    Sel::register(&c)
}

/// A class as a receiver.
pub(crate) fn class_obj(c: &AnyClass) -> *mut AnyObject {
    (c as *const AnyClass).cast_mut().cast()
}

/// True when `obj` answers `sel` (asked of the object itself, so a proxy answers for what it forwards).
pub(crate) fn responds(obj: *mut AnyObject, sel: Sel) -> bool {
    if obj.is_null() {
        return false;
    }
    let ask = Sel::register(c"respondsToSelector:");
    objc2::exception::catch(AssertUnwindSafe(|| {
        send!(obj, ask => Bool, sel => Sel).as_bool()
    }))
    .unwrap_or(false)
}

/// Calls `call` with the selector `name` on `obj`: [`Broken`] when `obj` is nil, does not answer the selector, or the
/// call raises an Objective-C exception.
pub(crate) fn guarded<R>(
    obj: *mut AnyObject,
    name: &str,
    call: impl FnOnce(*mut AnyObject, Sel) -> R,
) -> Result<R, Broken> {
    if obj.is_null() {
        return Err(Broken::nothing(name, "no receiver"));
    }
    let s = sel(name);
    if !responds(obj, s) {
        return Err(Broken::missing(name));
    }
    objc2::exception::catch(AssertUnwindSafe(|| call(obj, s))).map_err(|e| Broken {
        symbol: name.to_string(),
        why: Why::Exception(e.map_or_else(|| "an exception".to_string(), |e| e.to_string())),
    })
}

/// Calls a private method that answers by callback: `start` gets the one-shot `done` to call from the callback (on
/// any thread) and makes the call; the answer comes back here within `limit`, else [`Why::Timeout`]. BLOCKING, at
/// most `limit`: the sim host's own threads only.
pub fn answered<R: Send + 'static>(
    name: &str,
    limit: Duration,
    start: impl FnOnce(Box<dyn FnOnce(R) + Send>) -> Result<(), Broken>,
) -> Result<R, Broken> {
    let (tx, rx) = mpsc::sync_channel(1);
    start(Box::new(move |r| {
        let _ = tx.try_send(r);
    }))?;
    // sync-ok: the sim host's own threads, at most the callback limit
    rx.recv_timeout(limit).map_err(|_| Broken {
        symbol: name.to_string(),
        why: Why::Timeout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::rc::Retained;
    use objc2_foundation::{NSArray, NSObject};

    #[test]
    fn a_missing_selector_is_broken_with_its_name_not_a_crash() {
        let obj = NSObject::new();
        let p = Retained::as_ptr(&obj).cast_mut().cast::<AnyObject>();
        let r = guarded(p, "flareNoSuchSelector:", |_, _| 1);
        assert_eq!(r, Err(Broken::missing("flareNoSuchSelector:")));
        let b = r.unwrap_err();
        assert_eq!(b.to_string(), "flareNoSuchSelector: missing");
        assert_eq!(
            b.in_coresim("1174.21"),
            "flareNoSuchSelector: missing in CoreSimulator 1174.21"
        );
        // A missing class names itself too, and a nil receiver is refused before any send.
        assert_eq!(
            class("FlareNoSuchClass").unwrap_err(),
            Broken::missing("FlareNoSuchClass")
        );
        assert!(guarded(std::ptr::null_mut(), "description", |_, _| ()).is_err());
        // A selector it does answer goes through.
        let hash = guarded(p, "hash", |o, s| send!(o, s => usize));
        assert!(hash.is_ok());
        let ns_object = class_obj(class("NSObject").unwrap());
        assert!(
            guarded(ns_object, "new", |_, _| ()).is_ok(),
            "a class method"
        );
    }

    #[test]
    fn an_objc_exception_is_caught() {
        let arr = NSArray::<NSObject>::new();
        let p = Retained::as_ptr(&arr).cast_mut().cast::<AnyObject>();
        // objectAtIndex: past the end raises NSRangeException.
        let r = guarded(
            p,
            "objectAtIndex:",
            |o, s| send!(o, s => *mut AnyObject, 3usize => usize),
        );
        let b = r.unwrap_err();
        assert_eq!(b.symbol, "objectAtIndex:");
        let Why::Exception(e) = &b.why else {
            panic!("{b:?}");
        };
        assert!(e.contains("NSRangeException") || e.contains("index"), "{e}");
        // The process goes on: the next call works.
        assert!(guarded(p, "count", |o, s| send!(o, s => usize)) == Ok(0));
        // A callback that never comes is a timeout, not a hang.
        let t = answered::<u32>(
            "enumerateScreens",
            Duration::from_millis(50),
            |_done| Ok(()),
        );
        assert_eq!(t.unwrap_err().why, Why::Timeout);
        let ok = answered::<u32>("x", Duration::from_secs(1), |done| {
            std::thread::spawn(move || done(7));
            Ok(())
        });
        assert_eq!(ok, Ok(7));
    }
}
