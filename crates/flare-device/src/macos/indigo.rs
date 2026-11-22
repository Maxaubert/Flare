//! Legacy Indigo input, for CoreSimulator older than 1155.4 (Xcode 26; decision 8: best effort, untested by us):
//! SimulatorKit's `SimDeviceLegacyHIDClient` sends Indigo mach messages built by SimulatorKit's own
//! `IndigoHIDMessageFor...` functions. A single touch is rebuilt by hand as idb does (the builder's message carries a
//! mouse event the guest does not take as a touch).
//!
//! Never mixed with DTUHID in one boot (rule 1313): from 1155.4 the guest drops legacy Indigo for the whole boot, so
//! the path is picked once per boot from the loaded CoreSimulator's version ([`PathLock`]) and a failing DTUHID never
//! falls back to Indigo.
//!
//! Ported from idb (`FBSimulatorControl/HID/SimulatorIndigoHID.swift`, `SimulatorIndigoHIDClient.swift` and
//! `PrivateHeaders/SimulatorApp/Indigo.h`, facebook/idb, MIT):
//!
//! Copyright (c) Meta Platforms, Inc. and affiliates.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
//! documentation files (the "Software"), to deal in the Software without restriction, including without limitation
//! the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and
//! to permit persons to whom the Software is furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all copies or substantial portions of
//! the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO
//! THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
//! CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
//! IN THE SOFTWARE.

use std::collections::HashMap;
use std::ffi::{CString, c_void};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2_core_foundation::{CGPoint, CGSize};

use super::dtuhid::{Contacts, EventType};
use super::guarded::{Broken, class, class_obj, guarded, send};
use crate::geom::{Orientation, Size, to_portrait_ratio};
use crate::source::{Button, Touch};
use crate::target::Target;
use crate::versions::{self, InputPath};

/// The input path each booted device uses, picked once per boot.
#[derive(Debug, Default)]
pub struct PathLock {
    picked: HashMap<Target, InputPath>,
}

impl PathLock {
    /// The path of `target`'s boot: the one picked first, else the one the CoreSimulator version gives.
    pub fn pick(&mut self, target: &Target, coresim_version: &str) -> InputPath {
        *self
            .picked
            .entry(target.clone())
            .or_insert_with(|| versions::input_path(coresim_version))
    }

    /// Whether `path` may be used for `target` now: only the one its boot picked.
    pub fn may_use(&self, target: &Target, path: InputPath) -> bool {
        self.picked.get(target).is_none_or(|p| *p == path)
    }

    /// The device shut down: its next boot picks again.
    pub fn forget(&mut self, target: &Target) {
        self.picked.remove(target);
    }
}

/// The Swift class of the legacy client (it moved between Xcodes, so it is looked up by name).
pub const CLIENT_CLASS: &str = "SimulatorKit.SimDeviceLegacyHIDClient";
/// SimulatorKit's message builders the legacy path calls.
pub const BUILDERS: [&str; 4] = [
    "IndigoHIDMessageForMouseNSEvent",
    "IndigoHIDMessageForKeyboardArbitrary",
    "IndigoHIDMessageForButton",
    "IndigoHIDMessageForHIDArbitrary",
];

type MouseFn = unsafe extern "C" fn(*mut CGPoint, *mut CGPoint, u32, usize, CGSize, u32) -> *mut u8;
type KeyboardFn = unsafe extern "C" fn(i32, i32) -> *mut u8;
type ButtonFn = unsafe extern "C" fn(i32, i32, i32) -> *mut u8;
type HidArbitraryFn = unsafe extern "C" fn(i32, u32, u32, i32) -> *mut u8;

/// Indigo constants (idb's `Indigo.h`).
const MOUSE_TARGET: u32 = 0x32;
const TARGET_HARDWARE: i32 = 0x33;
const TARGET_DIGITIZER: i32 = 0x32;
const DOWN: i32 = 1;
const UP: i32 = 2;
const SOURCE_HOME: i32 = 0;
const SOURCE_LOCK: i32 = 1;
/// The single-touch message's layout: the header is 0x20 bytes, one payload 0x90, the touch at 0x30.
const PAYLOAD: usize = 0x90;
const MESSAGE: usize = 0x20 + PAYLOAD;
const TOUCH_AT: usize = 0x30;
const TOUCH_LEN: usize = 0x70;
const X_RATIO_AT: usize = 0x3C;
const Y_RATIO_AT: usize = 0x44;
/// The two-finger builder's second and third payloads.
const SECOND_AT: usize = 0xC0;
const THIRD_AT: usize = 0xC0 + 0xA0;

struct Fns {
    mouse: MouseFn,
    keyboard: KeyboardFn,
    button: ButtonFn,
    hid: HidArbitraryFn,
}

fn sym(name: &str) -> Result<*mut c_void, Broken> {
    let c = CString::new(name).map_err(|_| Broken::missing(name))?;
    // SAFETY: a lookup by name in the images already loaded (SimulatorKit).
    let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()) };
    if p.is_null() {
        Err(Broken::missing(name))
    } else {
        Ok(p)
    }
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
}

/// One device's legacy client.
pub struct Indigo {
    client: Retained<AnyObject>,
    f: Fns,
    queue: DispatchRetained<DispatchQueue>,
    contacts: Contacts,
    portrait: Size,
}

// SAFETY: the client is only used from the device's input worker.
unsafe impl Send for Indigo {}

/// Writes `v` at `at` of a malloc'd message (unaligned: Indigo is packed to 4 bytes).
///
/// # Safety
/// `m` points to at least `at + size_of::<T>()` writable bytes.
unsafe fn put<T: Copy>(m: *mut u8, at: usize, v: T) {
    // SAFETY: the caller's promise.
    unsafe { std::ptr::write_unaligned(m.add(at).cast::<T>(), v) };
}

impl Indigo {
    /// The legacy client of `device` and SimulatorKit's builders. BLOCKING (briefly).
    pub fn open(device: &super::sim::Device, portrait: Size) -> Result<Indigo, Broken> {
        let device = device.ptr();
        // SAFETY: SimulatorKit's builders have these C signatures (idb's declarations).
        let f = unsafe {
            Fns {
                mouse: std::mem::transmute::<*mut c_void, MouseFn>(sym(BUILDERS[0])?),
                keyboard: std::mem::transmute::<*mut c_void, KeyboardFn>(sym(BUILDERS[1])?),
                button: std::mem::transmute::<*mut c_void, ButtonFn>(sym(BUILDERS[2])?),
                hid: std::mem::transmute::<*mut c_void, HidArbitraryFn>(sym(BUILDERS[3])?),
            }
        };
        let cls = class_obj(class(CLIENT_CLASS)?);
        let alloc = guarded(cls, "alloc", |o, s| send!(o, s => *mut AnyObject))?;
        let (obj, err) = guarded(alloc, "initWithDevice:error:", |o, s| {
            let mut err: *mut AnyObject = std::ptr::null_mut();
            // `-[SimDeviceLegacyHIDClient initWithDevice:error:]`: `@32@0:8@16^@24`.
            let r = send!(o, s => *mut AnyObject, device => *mut AnyObject, &mut err => *mut *mut AnyObject);
            (r, err)
        })?;
        // SAFETY: `init` returned a +1 object (or nil).
        let client = unsafe { Retained::from_raw(obj) }
            .ok_or_else(|| Broken::nothing("initWithDevice:error:", super::sim::describe(err)))?;
        Ok(Indigo {
            client,
            f,
            queue: DispatchQueue::new("com.flare.sim-host.indigo", None),
            contacts: Contacts::default(),
            portrait,
        })
    }

    /// Hands a malloc'd message to the client, which frees it when sent.
    fn deliver(&self, m: *mut u8) -> bool {
        if m.is_null() {
            return false;
        }
        let done = RcBlock::with_encoding::<_, _, _, super::sim::Enc1>(|_err: *mut AnyObject| {});
        let q = (&*self.queue as *const DispatchQueue)
            .cast_mut()
            .cast::<c_void>();
        let r = guarded(
            Retained::as_ptr(&self.client).cast_mut(),
            "sendWithMessage:freeWhenDone:completionQueue:completion:",
            |o, s| {
                // `v48@0:8^v16B24@28@?36` style: a pointer, a BOOL, the queue and the block.
                send!(o, s => (), m.cast::<c_void>() => *mut c_void, Bool::YES => Bool, q => *mut c_void,
                    RcBlock::as_ptr(&done).cast::<c_void>() => *mut c_void)
            },
        );
        if r.is_err() {
            // SAFETY: the client did not take the message.
            unsafe { libc::free(m.cast()) };
        }
        r.is_ok()
    }

    /// A touch in device points (current orientation). Starts and moves are touches down at the point, an end is
    /// the touch up.
    pub fn touch(&mut self, t: &Touch, o: Orientation) -> bool {
        let Some(ev) = self.contacts.event(t.second.is_some(), t.phase, 0) else {
            return true;
        };
        let dir = if ev == EventType::End { UP } else { DOWN };
        let one = to_portrait_ratio(t.x, t.y, self.portrait, o);
        let m = match t.second {
            None => self.single(one, dir),
            Some((x, y)) => self.double(one, to_portrait_ratio(x, y, self.portrait, o), dir),
        };
        self.deliver(m)
    }

    /// idb's single-touch rebuild: the builder's touch copied into a fresh two-payload message.
    fn single(&self, r: (f64, f64), dir: i32) -> *mut u8 {
        let mut p = CGPoint { x: r.0, y: r.1 };
        // SAFETY: SimulatorKit's builder with a point, no second point, the touch target, the event type, a unit
        // screen and no edge; it returns a malloc'd message at least `TOUCH_AT + TOUCH_LEN` long, or null.
        unsafe {
            let src = (self.f.mouse)(
                &mut p,
                std::ptr::null_mut(),
                MOUSE_TARGET,
                dir as usize,
                CGSize {
                    width: 1.0,
                    height: 1.0,
                },
                0,
            );
            if src.is_null() {
                return std::ptr::null_mut();
            }
            put(src, X_RATIO_AT, r.0);
            put(src, Y_RATIO_AT, r.1);
            let m = libc::calloc(1, MESSAGE + PAYLOAD).cast::<u8>();
            if m.is_null() {
                libc::free(src.cast());
                return m;
            }
            put(m, 0x18, PAYLOAD as u32);
            put(m, 0x1C, 2u8);
            put(m, 0x20, 0x0Bu32);
            put(m, 0x24, mach_absolute_time());
            std::ptr::copy_nonoverlapping(src.add(TOUCH_AT), m.add(TOUCH_AT), TOUCH_LEN);
            libc::free(src.cast());
            std::ptr::copy_nonoverlapping(m.add(0x20), m.add(0x20 + PAYLOAD), PAYLOAD);
            put(m, 0x20 + PAYLOAD + 0x10, 1u32);
            put(m, 0x20 + PAYLOAD + 0x14, 2u32);
            m
        }
    }

    /// Two fingers: the builder's three-payload message with both points written in.
    fn double(&self, a: (f64, f64), b: (f64, f64), dir: i32) -> *mut u8 {
        let mut p1 = CGPoint { x: a.0, y: a.1 };
        let mut p2 = CGPoint { x: b.0, y: b.1 };
        // SAFETY: the builder with two points returns a malloc'd message with three payloads (idb), or null.
        unsafe {
            let m = (self.f.mouse)(
                &mut p1,
                &mut p2,
                MOUSE_TARGET,
                dir as usize,
                CGSize {
                    width: 1.0,
                    height: 1.0,
                },
                0,
            );
            if m.is_null() || libc::malloc_size(m.cast()) < THIRD_AT - 0x20 + Y_RATIO_AT + 8 {
                if !m.is_null() {
                    libc::free(m.cast());
                }
                return std::ptr::null_mut();
            }
            for (at, r) in [(0usize, a), (SECOND_AT - 0x20, a), (THIRD_AT - 0x20, b)] {
                put(m, at + X_RATIO_AT, r.0);
                put(m, at + Y_RATIO_AT, r.1);
            }
            m
        }
    }

    pub fn key(&self, usage: u32, down: bool) -> bool {
        // SAFETY: the builder takes the usage and the direction and returns a malloc'd message.
        let m = unsafe { (self.f.keyboard)(usage as i32, if down { DOWN } else { UP }) };
        self.deliver(m)
    }

    pub fn button(&self, b: Button, down: bool) -> bool {
        let dir = if down { DOWN } else { UP };
        // SAFETY: the builders' own arguments (idb's identities); each returns a malloc'd message.
        let m = unsafe {
            match b {
                Button::Home => (self.f.button)(SOURCE_HOME, dir, TARGET_HARDWARE),
                Button::Lock => (self.f.button)(SOURCE_LOCK, dir, TARGET_HARDWARE),
                Button::VolumeUp => (self.f.hid)(TARGET_DIGITIZER, 0x0C, 0xE9, dir),
                Button::VolumeDown => (self.f.hid)(TARGET_DIGITIZER, 0x0C, 0xEA, dir),
                Button::Shake
                | Button::RotateLeft
                | Button::RotateRight
                | Button::Back
                | Button::Recents
                | Button::Power => return true,
            }
        };
        self.deliver(m)
    }

    /// Ends every contact still down.
    pub fn release(&mut self) {
        let (one, two) = self.contacts.down();
        self.contacts.clear();
        if one {
            let m = self.single((0.5, 0.5), UP);
            self.deliver(m);
        }
        if two {
            let m = self.double((0.5, 0.5), (0.5, 0.5), UP);
            self.deliver(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indigo_is_never_used_after_dtuhid_in_one_boot() {
        let t = Target::sim("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0").unwrap();
        let other = Target::sim("00000000-0000-0000-0000-000000000002").unwrap();
        let mut lock = PathLock::default();
        assert!(lock.may_use(&t, InputPath::Indigo), "nothing picked yet");
        assert_eq!(lock.pick(&t, "1174.21"), InputPath::Dtuhid);
        assert!(lock.may_use(&t, InputPath::Dtuhid));
        assert!(
            !lock.may_use(&t, InputPath::Indigo),
            "never mixed in one boot"
        );
        // Asking again in the same boot keeps the first pick, whatever version is passed.
        assert_eq!(lock.pick(&t, "1051.9"), InputPath::Dtuhid);
        // Another device picks for itself; an old CoreSimulator gets Indigo only.
        assert_eq!(lock.pick(&other, "1051.9"), InputPath::Indigo);
        assert!(!lock.may_use(&other, InputPath::Dtuhid));
        // The boundary is 1155.4, compared part by part.
        let mut l = PathLock::default();
        assert_eq!(l.pick(&t, "1155.3"), InputPath::Indigo);
        l.forget(&t);
        assert_eq!(
            l.pick(&t, "1155.4"),
            InputPath::Dtuhid,
            "a new boot picks again"
        );
        l.forget(&t);
        assert_eq!(l.pick(&t, "1155.10"), InputPath::Dtuhid);
    }
}
