//! The window server's list of on-screen windows: the screen rule's evidence. A run of the engine must own 0
//! on-screen windows at its start and its end (and the tests sample it in between, for every process of the
//! bundle, helpers included).

use objc2::rc::Retained;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

/// One on-screen window: its owner's pid and name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub pid: i64,
    pub owner: String,
}

/// Every on-screen window the window server lists, with its owner.
pub fn on_screen() -> Vec<Shown> {
    let Some(arr) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0) else {
        return Vec::new();
    };
    let ptr = CFRetained::as_ptr(&arr).as_ptr();
    // SAFETY: CFArray and NSArray are toll-free bridged; `arr` stays alive (and retained) for the whole borrow.
    let list: &NSArray = unsafe { &*(ptr.cast::<NSArray>()) };
    let pid_key = NSString::from_str("kCGWindowOwnerPID");
    let name_key = NSString::from_str("kCGWindowOwnerName");
    list.iter()
        .filter_map(|d| {
            let d: Retained<NSDictionary> = d.downcast().ok()?;
            let pid = d
                .objectForKey(&pid_key)
                .and_then(|p| p.downcast::<NSNumber>().ok())?
                .longLongValue();
            let owner = d
                .objectForKey(&name_key)
                .and_then(|p| p.downcast::<NSString>().ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            Some(Shown { pid, owner })
        })
        .collect()
}

/// How many on-screen windows this process owns.
pub fn windows_of_self() -> usize {
    let me = i64::from(std::process::id());
    on_screen().iter().filter(|w| w.pid == me).count()
}

/// The on-screen windows whose owner's name starts with `prefix` (the bundle's app and its helpers).
pub fn windows_named(prefix: &str) -> Vec<Shown> {
    on_screen()
        .into_iter()
        .filter(|w| w.owner.starts_with(prefix))
        .collect()
}
