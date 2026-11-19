//! XPC to the simulator's services (rule 1313): the libxpc calls the sim host makes, the two private `_4sim`
//! functions found with `dlsym` (`xpc_endpoint_create_mach_port_4sim`, `xpc_connection_enable_sim2host_4sim`), a
//! connection's life (made from `-[SimDevice lookup:error:]`, interrupted, cancelled), the liveness barrier (a reply
//! within 4 s, five tries 4 s apart, then 200 ms before the first event) and requests that wait for a reply.
//!
//! Messages are built as plain values ([`XVal`]) first, so what goes on the wire is tested without a simulator, and
//! turned into XPC objects only when sent. Integers are always `uint64` (the services refuse other integer types).
//!
//! The connection path follows idb (`FBSimulatorControl/XPC/SimulatorXPCConnection.swift` and
//! `HID/SimulatorDTUHIDConnection.swift`, facebook/idb, MIT, Meta Platforms); spike S1 measured it (2026-11-19).

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::runtime::AnyObject;
use objc2_foundation::NSString;

use super::guarded::{Broken, Why, guarded, send};

/// A reply to the liveness barrier must come within this.
pub const LIVENESS_LIMIT: Duration = Duration::from_secs(4);
/// How many times the connection is made before input is broken.
pub const LIVENESS_TRIES: u32 = 5;
/// The pause between two tries.
pub const LIVENESS_BACKOFF: Duration = Duration::from_secs(4);
/// The wait after the liveness reply, before the first event (idb's reply tail).
pub const SETTLE: Duration = Duration::from_millis(200);
/// The longest wait for a send barrier (the event left the sim host).
pub const BARRIER_LIMIT: Duration = Duration::from_secs(2);

/// A value of an XPC message.
#[derive(Clone, Debug, PartialEq)]
pub enum XVal {
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(String),
    Arr(Vec<XVal>),
    Dict(Vec<(String, XVal)>),
}

impl XVal {
    /// A dictionary from `(key, value)` pairs.
    pub fn dict<const N: usize>(pairs: [(&str, XVal); N]) -> XVal {
        XVal::Dict(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// The value at `key` of a dictionary.
    pub fn get(&self, key: &str) -> Option<&XVal> {
        match self {
            XVal::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Every value in this one, itself included, with its key (`""` for the top and array items).
    pub fn walk<'a>(&'a self, key: &'a str, out: &mut Vec<(&'a str, &'a XVal)>) {
        out.push((key, self));
        match self {
            XVal::Dict(d) => d.iter().for_each(|(k, v)| v.walk(k, out)),
            XVal::Arr(a) => a.iter().for_each(|v| v.walk("", out)),
            _ => {}
        }
    }
}

/// An `xpc_object_t`.
pub type XObj = *mut c_void;

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    fn xpc_dictionary_create(keys: *const *const c_char, values: *const XObj, count: usize)
    -> XObj;
    fn xpc_dictionary_set_value(d: XObj, key: *const c_char, value: XObj);
    fn xpc_dictionary_get_value(d: XObj, key: *const c_char) -> XObj;
    fn xpc_array_create(objects: *const XObj, count: usize) -> XObj;
    fn xpc_array_append_value(a: XObj, value: XObj);
    fn xpc_array_get_count(a: XObj) -> usize;
    fn xpc_array_get_value(a: XObj, index: usize) -> XObj;
    fn xpc_uint64_create(v: u64) -> XObj;
    fn xpc_int64_create(v: i64) -> XObj;
    fn xpc_double_create(v: f64) -> XObj;
    fn xpc_bool_create(v: bool) -> XObj;
    fn xpc_string_create(s: *const c_char) -> XObj;
    fn xpc_string_get_string_ptr(s: XObj) -> *const c_char;
    fn xpc_uint64_get_value(o: XObj) -> u64;
    fn xpc_int64_get_value(o: XObj) -> i64;
    fn xpc_double_get_value(o: XObj) -> f64;
    fn xpc_bool_get_value(o: XObj) -> bool;
    fn xpc_get_type(o: XObj) -> *const c_void;
    fn xpc_retain(o: XObj) -> XObj;
    fn xpc_release(o: XObj);
    fn xpc_copy_description(o: XObj) -> *mut c_char;
    fn xpc_connection_create_from_endpoint(endpoint: XObj) -> XObj;
    fn xpc_connection_set_target_queue(c: XObj, q: *mut c_void);
    fn xpc_connection_set_event_handler(c: XObj, handler: *mut c_void);
    fn xpc_connection_resume(c: XObj);
    fn xpc_connection_cancel(c: XObj);
    fn xpc_connection_send_message(c: XObj, message: XObj);
    fn xpc_connection_send_barrier(c: XObj, barrier: *mut c_void);
    fn xpc_connection_send_message_with_reply(
        c: XObj,
        message: XObj,
        q: *mut c_void,
        handler: *mut c_void,
    );
    static _xpc_type_dictionary: c_void;
    static _xpc_type_array: c_void;
    static _xpc_type_string: c_void;
    static _xpc_type_uint64: c_void;
    static _xpc_type_int64: c_void;
    static _xpc_type_double: c_void;
    static _xpc_type_bool: c_void;
    static _xpc_type_error: c_void;
    static _xpc_error_connection_interrupted: c_void;
    static _xpc_error_connection_invalid: c_void;
    fn free(p: *mut c_void);
}

/// `xpc_endpoint_create_mach_port_4sim(port, 0, 0)` (private, CoreSimulator's host side).
type EndpointFromPort = unsafe extern "C" fn(u32, u64, u64) -> XObj;
/// `xpc_connection_enable_sim2host_4sim(connection)` (private).
type EnableSim2Host = unsafe extern "C" fn(XObj);

/// The private `_4sim` functions, looked up once.
struct Sim4 {
    endpoint: EndpointFromPort,
    enable: EnableSim2Host,
}

fn dlsym(name: &str) -> Option<*mut c_void> {
    let c = CString::new(name).ok()?;
    // SAFETY: RTLD_DEFAULT and a NUL-terminated name; the result is only a lookup.
    let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()) };
    (!p.is_null()).then_some(p)
}

/// The private symbols the input path needs (checked by `--self-test`).
pub const SIM4_SYMBOLS: [&str; 2] = [
    "xpc_endpoint_create_mach_port_4sim",
    "xpc_connection_enable_sim2host_4sim",
];

fn sim4() -> Result<&'static Sim4, Broken> {
    static S: std::sync::OnceLock<Option<Sim4>> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        let e = dlsym(SIM4_SYMBOLS[0])?;
        let n = dlsym(SIM4_SYMBOLS[1])?;
        // SAFETY: the symbols have these C signatures (idb's declarations; spike S1 called them so).
        Some(unsafe {
            Sim4 {
                endpoint: std::mem::transmute::<*mut c_void, EndpointFromPort>(e),
                enable: std::mem::transmute::<*mut c_void, EnableSim2Host>(n),
            }
        })
    })
    .as_ref()
    .ok_or_else(|| Broken::missing("xpc_endpoint_create_mach_port_4sim"))
}

/// Every `_4sim` symbol that is missing in this macOS.
pub fn missing_symbols() -> Vec<Broken> {
    SIM4_SYMBOLS
        .iter()
        .filter(|s| dlsym(s).is_none())
        .map(|s| Broken::missing(s))
        .collect()
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// An owned XPC object (released when dropped).
pub struct Owned(XObj);

// SAFETY: XPC objects are reference counted and thread-safe.
unsafe impl Send for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we hold one reference.
            unsafe { xpc_release(self.0) };
        }
    }
}

impl Owned {
    /// Takes a +1 object.
    fn take(o: XObj) -> Option<Owned> {
        (!o.is_null()).then_some(Owned(o))
    }

    /// Retains a +0 object.
    fn retain(o: XObj) -> Option<Owned> {
        // SAFETY: a live object, retained once more.
        (!o.is_null()).then(|| Owned(unsafe { xpc_retain(o) }))
    }

    pub fn ptr(&self) -> XObj {
        self.0
    }

    /// The object back as a value: arrays, strings, numbers and booleans (a dictionary is read by key through
    /// [`Reply`]; anything else is None).
    pub fn value(&self) -> Option<XVal> {
        read(self.0, 8)
    }

    /// Its description (logs).
    pub fn describe(&self) -> String {
        // SAFETY: a live object; the copy is freed here.
        unsafe {
            let d = xpc_copy_description(self.0);
            if d.is_null() {
                return String::new();
            }
            let s = CStr::from_ptr(d).to_string_lossy().to_string();
            free(d.cast());
            s
        }
    }
}

fn is(o: XObj, t: &c_void) -> bool {
    // SAFETY: a live object; the type is a static of libxpc.
    std::ptr::eq(unsafe { xpc_get_type(o) }, t)
}

fn dict_type() -> &'static c_void {
    // SAFETY: a static of libxpc, only its address is used.
    unsafe { &_xpc_type_dictionary }
}

fn array_type() -> &'static c_void {
    // SAFETY: as above.
    unsafe { &_xpc_type_array }
}

fn read(o: XObj, depth: u32) -> Option<XVal> {
    if o.is_null() || depth == 0 {
        return None;
    }
    // SAFETY: each getter is called on an object of its own type (checked first).
    unsafe {
        if is(o, &_xpc_type_array) {
            let n = xpc_array_get_count(o).min(64);
            return Some(XVal::Arr(
                (0..n)
                    .filter_map(|i| read(xpc_array_get_value(o, i), depth - 1))
                    .collect(),
            ));
        }
        if is(o, &_xpc_type_string) {
            let p = xpc_string_get_string_ptr(o);
            return (!p.is_null())
                .then(|| XVal::Str(CStr::from_ptr(p).to_string_lossy().to_string()));
        }
        if is(o, &_xpc_type_uint64) {
            return Some(XVal::U64(xpc_uint64_get_value(o)));
        }
        if is(o, &_xpc_type_int64) {
            return Some(XVal::I64(xpc_int64_get_value(o)));
        }
        if is(o, &_xpc_type_double) {
            return Some(XVal::F64(xpc_double_get_value(o)));
        }
        if is(o, &_xpc_type_bool) {
            return Some(XVal::Bool(xpc_bool_get_value(o)));
        }
    }
    None
}

/// A reply read by key.
pub struct Reply(pub Owned);

impl Reply {
    /// True for a dictionary (an error reply is not one).
    pub fn is_dict(&self) -> bool {
        is(self.0.ptr(), dict_type())
    }

    /// The value at a path of dictionary keys, retained.
    pub fn at(&self, path: &[&str]) -> Option<Owned> {
        let mut cur = Owned::retain(self.0.ptr())?;
        for k in path {
            if !is(cur.ptr(), dict_type()) {
                return None;
            }
            let key = cstr(k);
            // SAFETY: a dictionary and a NUL-terminated key; the result is +0, retained by `Owned::retain`.
            cur = Owned::retain(unsafe { xpc_dictionary_get_value(cur.ptr(), key.as_ptr()) })?;
        }
        Some(cur)
    }

    /// The items of the array at `path`, each retained.
    pub fn items(&self, path: &[&str]) -> Vec<Owned> {
        let Some(a) = self.at(path) else {
            return Vec::new();
        };
        if !is(a.ptr(), array_type()) {
            return Vec::new();
        }
        // SAFETY: an array; each item is +0, retained.
        let n = unsafe { xpc_array_get_count(a.ptr()) }.min(64);
        (0..n)
            // SAFETY: an array and an index under its count; the item is +0, retained.
            .filter_map(|i| Owned::retain(unsafe { xpc_array_get_value(a.ptr(), i) }))
            .collect()
    }
}

/// A value as a new XPC object (+1).
pub fn to_xpc(v: &XVal) -> Owned {
    // SAFETY: each constructor returns a new +1 object; dictionaries and arrays retain what is put in them, and the
    // item's own reference is released when its `Owned` drops.
    unsafe {
        let o = match v {
            XVal::U64(n) => xpc_uint64_create(*n),
            XVal::I64(n) => xpc_int64_create(*n),
            XVal::F64(n) => xpc_double_create(*n),
            XVal::Bool(b) => xpc_bool_create(*b),
            XVal::Str(s) => {
                let c = cstr(s);
                xpc_string_create(c.as_ptr())
            }
            XVal::Arr(items) => {
                let a = xpc_array_create(std::ptr::null(), 0);
                for i in items {
                    let item = to_xpc(i);
                    xpc_array_append_value(a, item.ptr());
                }
                a
            }
            XVal::Dict(pairs) => {
                let d = xpc_dictionary_create(std::ptr::null(), std::ptr::null(), 0);
                for (k, item) in pairs {
                    let key = cstr(k);
                    let item = to_xpc(item);
                    xpc_dictionary_set_value(d, key.as_ptr(), item.ptr());
                }
                d
            }
        };
        Owned(o)
    }
}

/// A connection to one of the simulator's services.
pub struct Conn {
    conn: Owned,
    queue: dispatch2::DispatchRetained<DispatchQueue>,
    pub service: String,
    /// How many times the connection was interrupted.
    interrupted: Arc<AtomicU32>,
    /// The connection is invalid (the service went away for good).
    invalid: Arc<AtomicBool>,
    _handler: RcBlock<dyn Fn(XObj)>,
}

// SAFETY: XPC connections are thread-safe; the block only touches atomics.
unsafe impl Send for Conn {}
// SAFETY: as above.
unsafe impl Sync for Conn {}

impl Drop for Conn {
    fn drop(&mut self) {
        // SAFETY: a live connection; cancelling twice is allowed.
        unsafe { xpc_connection_cancel(self.conn.ptr()) };
    }
}

/// The mach port of a service inside the device (`-[SimDevice lookup:error:]`), or why not. BLOCKING (briefly).
pub fn lookup(device: *mut AnyObject, service: &str) -> Result<u32, Broken> {
    let name = NSString::from_str(service);
    let (port, err) = guarded(device, "lookup:error:", |o, s| {
        let mut err: *mut AnyObject = std::ptr::null_mut();
        // `-[SimDevice lookup:error:]`: `I32@0:8@16^@24` (a mach_port_t).
        let p = send!(o, s => u32,
            objc2::rc::Retained::as_ptr(&name).cast_mut().cast::<AnyObject>() => *mut AnyObject,
            &mut err => *mut *mut AnyObject);
        (p, err)
    })?;
    if port == 0 {
        return Err(Broken::nothing(
            "lookup:error:",
            format!("{service}: {}", super::sim::describe(err)),
        ));
    }
    Ok(port)
}

impl Conn {
    /// Connects to `service` inside `device` and resumes the connection. Nothing is known yet about whether the
    /// service answers: see [`Conn::live`]. BLOCKING (briefly).
    pub fn open(device: *mut AnyObject, service: &str) -> Result<Conn, Broken> {
        let sim4 = sim4()?;
        let port = lookup(device, service)?;
        // SAFETY: the private functions' own signatures; the endpoint is +1 and released after the connection is
        // made from it.
        let conn = unsafe {
            let ep = Owned::take((sim4.endpoint)(port, 0, 0))
                .ok_or_else(|| Broken::nothing("xpc_endpoint_create_mach_port_4sim", service))?;
            Owned::take(xpc_connection_create_from_endpoint(ep.ptr()))
                .ok_or_else(|| Broken::nothing("xpc_connection_create_from_endpoint", service))?
        };
        // SAFETY: as above.
        unsafe { (sim4.enable)(conn.ptr()) };
        let queue = DispatchQueue::new(&format!("com.flare.sim-host.xpc.{service}"), None);
        let interrupted = Arc::new(AtomicU32::new(0));
        let invalid = Arc::new(AtomicBool::new(false));
        let (i, v) = (Arc::clone(&interrupted), Arc::clone(&invalid));
        let handler = RcBlock::new(move |e: XObj| {
            // SAFETY: comparing pointers with libxpc's statics only.
            unsafe {
                if e == (&_xpc_error_connection_interrupted as *const c_void).cast_mut() {
                    i.fetch_add(1, Ordering::Relaxed);
                } else if e == (&_xpc_error_connection_invalid as *const c_void).cast_mut() {
                    v.store(true, Ordering::Relaxed);
                }
            }
        });
        let handler: RcBlock<dyn Fn(XObj)> = handler;
        // SAFETY: a new connection, its queue and its handler (copied by libxpc), then resumed once.
        unsafe {
            xpc_connection_set_target_queue(
                conn.ptr(),
                (&*queue as *const DispatchQueue).cast_mut().cast(),
            );
            xpc_connection_set_event_handler(conn.ptr(), RcBlock::as_ptr(&handler).cast());
            xpc_connection_resume(conn.ptr());
        }
        Ok(Conn {
            conn,
            queue,
            service: service.to_string(),
            interrupted,
            invalid,
            _handler: handler,
        })
    }

    /// How many times it was interrupted so far.
    pub fn interruptions(&self) -> u32 {
        self.interrupted.load(Ordering::Relaxed)
    }

    pub fn is_invalid(&self) -> bool {
        self.invalid.load(Ordering::Relaxed)
    }

    /// Sends a message, then waits for its send barrier (the message left this process), at most
    /// [`BARRIER_LIMIT`]. Never waits for a reply. BLOCKING (the input worker).
    pub fn send(&self, msg: &XVal) -> bool {
        let m = to_xpc(msg);
        // SAFETY: a live connection and message; libxpc retains the message.
        unsafe { xpc_connection_send_message(self.conn.ptr(), m.ptr()) };
        let (tx, rx) = mpsc::sync_channel::<()>(1);
        let b = RcBlock::new(move || {
            let _ = tx.try_send(());
        });
        // SAFETY: a live connection; libxpc copies the block.
        unsafe { xpc_connection_send_barrier(self.conn.ptr(), RcBlock::as_ptr(&b).cast()) };
        rx.recv_timeout(BARRIER_LIMIT).is_ok() // sync-ok: the input worker, at most 2 s
    }

    /// Sends a message and waits for its reply, at most `limit`. An error object (interrupted, invalid) is
    /// [`Why::Nothing`] with its description. BLOCKING (the sim host's workers).
    pub fn request(&self, msg: &XVal, limit: Duration) -> Result<Reply, Broken> {
        let m = to_xpc(msg);
        let (tx, rx) = mpsc::sync_channel::<Option<Owned>>(1);
        let h = RcBlock::new(move |r: XObj| {
            let _ = tx.try_send(Owned::retain(r));
        });
        // SAFETY: a live connection, message and queue; libxpc copies the block and retains the message.
        unsafe {
            xpc_connection_send_message_with_reply(
                self.conn.ptr(),
                m.ptr(),
                (&*self.queue as *const DispatchQueue).cast_mut().cast(),
                RcBlock::as_ptr(&h).cast(),
            );
        }
        let r = rx.recv_timeout(limit).map_err(|_| Broken {
            // sync-ok: the sim host's workers, at most `limit`
            symbol: self.service.clone(),
            why: Why::Timeout,
        })?;
        let r = Reply(r.ok_or_else(|| Broken::nothing(&self.service, "no reply"))?);
        if !r.is_dict() {
            return Err(Broken::nothing(&self.service, r.0.describe()));
        }
        Ok(r)
    }
}

/// Makes a connection that answers: `try_once` makes one and waits for its liveness reply (attempt 1 to
/// [`LIVENESS_TRIES`]), `sleep` waits between tries ([`LIVENESS_BACKOFF`]) and once after the reply ([`SETTLE`]).
/// After five tries with no reply the device has no input: [`Broken`] with the last reason. Spike S1: on 2 of 3 cold
/// boots the first try timed out; a second one worked.
pub fn until_live<C>(
    mut try_once: impl FnMut(u32) -> Result<C, String>,
    mut sleep: impl FnMut(Duration),
) -> Result<C, Broken> {
    let mut last = String::new();
    for attempt in 1..=LIVENESS_TRIES {
        match try_once(attempt) {
            Ok(c) => {
                sleep(SETTLE); // sync-ok: the caller's sleep (the input worker's, or a test's)
                return Ok(c);
            }
            Err(e) => last = e,
        }
        if attempt < LIVENESS_TRIES {
            sleep(LIVENESS_BACKOFF); // sync-ok: the caller's sleep (the input worker's, or a test's)
        }
    }
    Err(Broken::nothing(
        "dtuhidd",
        format!("no liveness reply in {LIVENESS_TRIES} tries: {last}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_liveness_reply_after_five_tries_is_broken_input() {
        let mut tries = Vec::new();
        let mut sleeps = Vec::new();
        let r = until_live::<()>(
            |a| {
                tries.push(a);
                Err(format!("try {a} timed out after 4 s"))
            },
            |d| sleeps.push(d),
        );
        assert_eq!(tries, [1, 2, 3, 4, 5], "five tries");
        assert_eq!(
            sleeps,
            vec![LIVENESS_BACKOFF; 4],
            "4 s apart, no pause after the last"
        );
        let b = r.unwrap_err();
        assert_eq!(b.symbol, "dtuhidd");
        let msg = b.to_string();
        assert!(
            msg.contains("no liveness reply in 5 tries") && msg.contains("try 5"),
            "{msg}"
        );
        // A reply on the second try: live, with the 200 ms settle before the first event.
        let mut sleeps = Vec::new();
        let r = until_live(
            |a| {
                if a == 2 {
                    Ok(a)
                } else {
                    Err("timed out".into())
                }
            },
            |d| sleeps.push(d),
        );
        assert_eq!(r, Ok(2));
        assert_eq!(sleeps, [LIVENESS_BACKOFF, SETTLE]);
        assert_eq!(
            (LIVENESS_LIMIT, LIVENESS_TRIES),
            (Duration::from_secs(4), 5)
        );
    }

    #[test]
    fn values_become_xpc_objects_and_back() {
        let v = XVal::dict([
            ("n", XVal::U64(u64::MAX)),
            ("s", XVal::Str("caf\u{e9}".into())),
            (
                "a",
                XVal::Arr(vec![XVal::F64(0.5), XVal::Bool(true), XVal::I64(-3)]),
            ),
        ]);
        let o = to_xpc(&v);
        let r = Reply(Owned::retain(o.ptr()).unwrap());
        assert!(r.is_dict());
        assert_eq!(
            r.at(&["n"]).and_then(|o| o.value()),
            Some(XVal::U64(u64::MAX))
        );
        assert_eq!(
            r.at(&["s"]).and_then(|o| o.value()),
            Some(XVal::Str("caf\u{e9}".into()))
        );
        assert_eq!(
            r.items(&["a"])
                .iter()
                .filter_map(|o| o.value())
                .collect::<Vec<_>>(),
            [XVal::F64(0.5), XVal::Bool(true), XVal::I64(-3)]
        );
        assert!(r.at(&["missing"]).is_none());
        assert!(r.at(&["n", "deeper"]).is_none(), "not a dictionary");
        assert!(o.describe().contains("caf"));
        // The `_4sim` functions are there on this macOS.
        assert!(missing_symbols().is_empty(), "{:?}", missing_symbols());
    }
}
