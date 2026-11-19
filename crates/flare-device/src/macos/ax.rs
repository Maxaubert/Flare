//! The device's UI tree (rule 1315, sim host half): `AXPTranslator` (AccessibilityPlatformTranslation, a macOS
//! private framework) turns the simulator's accessibility into macOS accessibility elements. Its requests reach the
//! device through a bridge-token delegate this file defines: for a token, a block that sends each request with
//! `-[SimDevice sendAccessibilityRequestAsync:completionQueue:completionHandler:]` and waits for the answer (5 s).
//!
//! One request is in flight at a time ([`Serial`]): the translator is shared by every device. The walk is bounded
//! (depth [`MAX_DEPTH`], [`MAX_NODES`] elements, [`WALK_LIMIT`]), and returns `{role, label, value, frame, enabled,
//! children}` in device points of the current orientation (spike S4: Settings 16 elements in 15 to 39 ms warm, 3.3 s
//! on the first request after a boot, so the sim host sends one right after attach to warm it). `AXIsProcessTrusted`
//! is never called: it asks TCC about Flare (S4).
//!
//! The approach follows idb (`FBSimulatorAccessibilityCommands`, facebook/idb, MIT, Meta Platforms) and spike S4's
//! `ax.m`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use block2::{ManualBlockEncoding, RcBlock};
use dispatch2::DispatchQueue;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool, NSObject};
use objc2::{AllocAnyThread, define_class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::NSString;

use super::guarded::{Broken, CALLBACK_LIMIT, class, class_obj, guarded, send};
use super::sim::Sendable;
use crate::tree::{Rect, UiNode};

/// The framework, loaded by the sim host only.
pub const FRAMEWORK: &str = "/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation";
/// The deepest element walked (each level is two levels of JSON, under serde's 128).
pub const MAX_DEPTH: usize = 50;
/// The most elements one walk reads.
pub const MAX_NODES: usize = 1500;
/// The longest one walk may take; what was read by then is returned.
pub const WALK_LIMIT: Duration = Duration::from_secs(15);
/// Requests waiting behind the one in flight; more are answered `busy`.
pub const MAX_WAITING: usize = 8;

/// Runs jobs one at a time, in order, on a thread of its own.
pub struct Serial<J: Send + 'static> {
    tx: Mutex<Sender<J>>,
    waiting: Arc<AtomicUsize>,
}

impl<J: Send + 'static> Serial<J> {
    pub fn start(name: &str, mut work: impl FnMut(J) + Send + 'static) -> Serial<J> {
        let (tx, rx) = mpsc::channel::<J>();
        let waiting = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&waiting);
        let _ = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                for j in rx {
                    work(j);
                    w.fetch_sub(1, Ordering::Relaxed);
                }
            });
        Serial {
            tx: Mutex::new(tx),
            waiting,
        }
    }

    /// Queues a job: false (not queued) when [`MAX_WAITING`] already wait.
    pub fn submit(&self, j: J) -> bool {
        if self.waiting.fetch_add(1, Ordering::Relaxed) > MAX_WAITING {
            self.waiting.fetch_sub(1, Ordering::Relaxed);
            return false;
        }
        let sent = self.tx.lock().is_ok_and(|tx| tx.send(j).is_ok());
        if !sent {
            self.waiting.fetch_sub(1, Ordering::Relaxed);
        }
        sent
    }
}

/// The bridge callback block's type encoding: takes the request, returns the response.
struct EncCallback;
// SAFETY: the block takes one object and returns one (64-bit: the block at 0, the object at 8).
unsafe impl ManualBlockEncoding for EncCallback {
    type Arguments = (*mut AnyObject,);
    type Return = *mut AnyObject;
    const ENCODING_CSTR: &'static std::ffi::CStr = c"@16@?0@8";
}

type Callback = RcBlock<dyn Fn(*mut AnyObject) -> *mut AnyObject>;

/// A token's callback block, which holds its device (kept here: the translator holds the block only while it calls
/// it).
struct Registered {
    block: Sendable<Callback>,
}

fn registry() -> &'static Mutex<HashMap<String, Registered>> {
    static R: OnceLock<Mutex<HashMap<String, Registered>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `+[AXPTranslatorResponse emptyResponse]`, the answer when the device did not give one.
fn empty_response() -> *mut AnyObject {
    class("AXPTranslatorResponse")
        .ok()
        .and_then(|c| {
            guarded(
                class_obj(c),
                "emptyResponse",
                |o, s| send!(o, s => *mut AnyObject),
            )
            .ok()
        })
        .unwrap_or(std::ptr::null_mut())
}

/// Sends one translator request to the device and waits for its response (+0, autoreleased), at most 5 s.
fn forward(device: *mut AnyObject, request: *mut AnyObject) -> *mut AnyObject {
    if request.is_null() {
        return empty_response();
    }
    let queue = DispatchQueue::new("com.flare.sim-host.ax-reply", None);
    let (tx, rx) = mpsc::sync_channel::<Option<Sendable<Retained<AnyObject>>>>(1);
    let done = RcBlock::with_encoding::<_, _, _, super::sim::Enc1>(move |r: *mut AnyObject| {
        // SAFETY: the response object (+0), retained to cross threads.
        let r = unsafe { Retained::retain(r) }.map(Sendable);
        let _ = tx.try_send(r);
    });
    let q = (&*queue as *const DispatchQueue)
        .cast_mut()
        .cast::<c_void>();
    let sent = guarded(
        device,
        "sendAccessibilityRequestAsync:completionQueue:completionHandler:",
        |o, s| {
            // `v40@0:8@16@24@?32`: the request, the queue, the completion block.
            send!(o, s => (), request => *mut AnyObject, q => *mut c_void,
            RcBlock::as_ptr(&done).cast::<c_void>() => *mut c_void)
        },
    );
    if sent.is_err() {
        return empty_response();
    }
    match rx.recv_timeout(CALLBACK_LIMIT) {
        // sync-ok: the AX worker, at most 5 s per request
        Ok(Some(r)) => Retained::autorelease_return(r.0),
        _ => empty_response(),
    }
}

define_class!(
    /// The translator's bridge-token delegate: a token is a device.
    #[unsafe(super(NSObject))]
    #[name = "FlareSimHostAXBridge"]
    struct Bridge;

    impl Bridge {
        #[unsafe(method(accessibilityTranslationDelegateBridgeCallbackWithToken:))]
        fn callback(&self, token: *mut AnyObject) -> *mut AnyObject {
            let token = name_of(token);
            registry()
                .lock()
                .ok()
                .and_then(|r| r.get(&token).map(|e| RcBlock::as_ptr(&e.block.0).cast::<AnyObject>()))
                .unwrap_or(std::ptr::null_mut())
        }

        #[unsafe(method(accessibilityTranslationConvertPlatformFrameToSystem:withToken:))]
        fn convert(&self, r: CGRect, _token: *mut AnyObject) -> CGRect {
            r
        }

        #[unsafe(method(accessibilityTranslationRootParentWithToken:))]
        fn root_parent(&self, _token: *mut AnyObject) -> *mut AnyObject {
            std::ptr::null_mut()
        }
    }
);

impl Bridge {
    fn new() -> Retained<Bridge> {
        let this = Bridge::alloc().set_ivars(());
        // SAFETY: NSObject's init on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }
}

/// An NSString's text ("" for nil or anything else).
fn name_of(s: *mut AnyObject) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: a live object; checked to be a string before it is read as one.
    let o = unsafe { &*s };
    if o.downcast_ref::<NSString>().is_some() {
        o.downcast_ref::<NSString>()
            .map(|n| n.to_string())
            .unwrap_or_default()
    } else {
        String::new()
    }
}

/// The text of an attribute value: a string, an attributed string's string, a number's value, else nothing.
fn text_of(v: *mut AnyObject) -> String {
    if v.is_null() {
        return String::new();
    }
    let attributed = guarded(v, "isKindOfClass:", |o, sel| {
        let c = class("NSAttributedString")
            .map(class_obj)
            .unwrap_or(std::ptr::null_mut());
        send!(o, sel => Bool, c => *mut AnyObject).as_bool()
    }) == Ok(true);
    if attributed
        && let Ok(s) = guarded(v, "string", |o, s| send!(o, s => *mut AnyObject))
        && !s.is_null()
    {
        return name_of(s);
    }
    // SAFETY: a live object.
    let o = unsafe { &*v };
    if let Some(n) = o.downcast_ref::<NSString>() {
        return n.to_string();
    }
    guarded(v, "stringValue", |o, s| send!(o, s => *mut AnyObject))
        .map(name_of)
        .unwrap_or_default()
}

/// The UI tree reader: the translator, set up once.
pub struct Ax {
    translator: Sendable<Retained<AnyObject>>,
    _bridge: Sendable<Retained<Bridge>>,
}

/// Loads the framework (BLOCKING, dlopen): the sim host's start.
pub fn load() -> Result<(), String> {
    // SAFETY: an Apple-signed framework; its initialisers only register its classes.
    let lib = unsafe { libloading::Library::new(FRAMEWORK) }
        .map_err(|e| format!("AccessibilityPlatformTranslation did not load: {e}"))?;
    std::mem::forget(lib);
    Ok(())
}

/// Makes `token` stand for `device` (each attached device has one).
pub fn register(token: &str, device: &super::sim::Device) {
    // SAFETY: the device object (+0), retained for as long as the token is known.
    let Some(dev) = (unsafe { Retained::retain(device.ptr()) }) else {
        return;
    };
    let d = Sendable(dev);
    let block: Callback =
        RcBlock::with_encoding::<_, _, _, EncCallback>(move |req: *mut AnyObject| {
            let d = &d;
            objc2::exception::catch(AssertUnwindSafe(|| {
                forward(Retained::as_ptr(&d.0).cast_mut(), req)
            }))
            .unwrap_or(std::ptr::null_mut())
        });
    if let Ok(mut r) = registry().lock() {
        r.insert(
            token.to_string(),
            Registered {
                block: Sendable(block),
            },
        );
    }
}

pub fn forget(token: &str) {
    if let Ok(mut r) = registry().lock() {
        r.remove(token);
    }
}

/// Sets a BOOL property through its setter, else through KVC (the translator is a local object, not a proxy).
fn set_flag(obj: *mut AnyObject, setter: &str, key: &str) -> Result<(), Broken> {
    if guarded(obj, setter, |o, s| send!(o, s => (), Bool::YES => Bool)).is_ok() {
        return Ok(());
    }
    let yes = objc2_foundation::NSNumber::new_bool(true);
    let k = NSString::from_str(key);
    guarded(obj, "setValue:forKey:", |o, s| {
        send!(o, s => (), Retained::as_ptr(&yes).cast_mut().cast::<AnyObject>() => *mut AnyObject,
            Retained::as_ptr(&k).cast_mut().cast::<AnyObject>() => *mut AnyObject)
    })
}

impl Ax {
    /// The shared translator with Flare's bridge as its delegate. The framework must be loaded ([`load`]).
    pub fn open() -> Result<Ax, Broken> {
        let cls = class_obj(class("AXPTranslator")?);
        let t = guarded(cls, "sharedInstance", |o, s| send!(o, s => *mut AnyObject))?;
        // SAFETY: the shared instance (+0), retained here.
        let translator =
            unsafe { Retained::retain(t) }.ok_or_else(|| Broken::nothing("sharedInstance", ""))?;
        let bridge = Bridge::new();
        let tp = Retained::as_ptr(&translator).cast_mut();
        guarded(
            tp,
            "setBridgeTokenDelegate:",
            |o, s| send!(o, s => (), Retained::as_ptr(&bridge).cast_mut().cast::<AnyObject>() => *mut AnyObject),
        )?;
        set_flag(tp, "setSupportsDelegateTokens:", "supportsDelegateTokens")?;
        set_flag(tp, "setAccessibilityEnabled:", "accessibilityEnabled")?;
        Ok(Ax {
            translator: Sendable(translator),
            _bridge: Sendable(bridge),
        })
    }

    /// The UI tree of the app in front of `token`'s device (or the element at a point). BLOCKING (15 ms to 3.3 s):
    /// the AX worker only.
    pub fn tree(&self, token: &str, at: Option<(f64, f64)>) -> Result<UiNode, Broken> {
        if !registry().lock().is_ok_and(|r| r.contains_key(token)) {
            return Err(Broken::nothing("ui_tree", "the device is not attached"));
        }
        autoreleasepool(|_| {
            let tp = Retained::as_ptr(&self.translator.0).cast_mut();
            let tok = NSString::from_str(token);
            let tok_p = Retained::as_ptr(&tok).cast_mut().cast::<AnyObject>();
            let ask = || match at {
                None => guarded(
                    tp,
                    "frontmostApplicationWithDisplayId:bridgeDelegateToken:",
                    |o, s| send!(o, s => *mut AnyObject, 0u32 => u32, tok_p => *mut AnyObject),
                ),
                Some((x, y)) => guarded(
                    tp,
                    "objectAtPoint:displayId:bridgeDelegateToken:",
                    |o, s| send!(o, s => *mut AnyObject, CGPoint { x, y } => CGPoint, 0u32 => u32, tok_p => *mut AnyObject),
                ),
            };
            let mut translation = ask()?;
            // The first request after a boot comes back empty after about 3 s, and so do the next ones for about a
            // second more (measured 2026-11-19): up to three more tries, 700 ms apart.
            for _ in 0..3 {
                if !translation.is_null() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(700)); // sync-ok: the AX worker, a cold device only
                translation = ask()?;
            }
            if translation.is_null() {
                return Err(Broken::nothing(
                    "frontmostApplicationWithDisplayId:",
                    "nothing in front",
                ));
            }
            let _ = guarded(
                translation,
                "setBridgeDelegateToken:",
                |o, s| send!(o, s => (), tok_p => *mut AnyObject),
            );
            let root = guarded(
                tp,
                "macPlatformElementFromTranslation:",
                |o, s| send!(o, s => *mut AnyObject, translation => *mut AnyObject),
            )?;
            if root.is_null() {
                return Err(Broken::nothing("macPlatformElementFromTranslation:", ""));
            }
            let mut walk = Walk {
                token: tok_p,
                nodes: 0,
                end: Instant::now() + WALK_LIMIT,
            };
            Ok(walk.node(root, 0))
        })
    }
}

/// One walk's budget.
struct Walk {
    token: *mut AnyObject,
    nodes: usize,
    end: Instant,
}

impl Walk {
    fn node(&mut self, el: *mut AnyObject, depth: usize) -> UiNode {
        self.nodes += 1;
        // The element's own translation must carry the token, or its children's requests have no device.
        if let Ok(tr) = guarded(el, "translation", |o, s| send!(o, s => *mut AnyObject)) {
            let tok = self.token;
            let _ = guarded(
                tr,
                "setBridgeDelegateToken:",
                |o, s| send!(o, s => (), tok => *mut AnyObject),
            );
        }
        let get = |name: &str| {
            guarded(el, name, |o, s| send!(o, s => *mut AnyObject)).unwrap_or(std::ptr::null_mut())
        };
        let frame = objc2::exception::catch(AssertUnwindSafe(|| {
            // SAFETY: an accessibility element, which answers `accessibilityFrame` with an NSRect (`msg_send!`
            // handles a struct returned in memory where the platform needs it).
            let r: CGRect = unsafe { msg_send![&*el, accessibilityFrame] };
            r
        }))
        .unwrap_or_default();
        let enabled = guarded(el, "isAccessibilityEnabled", |o, s| send!(o, s => Bool))
            .map(|b| b.as_bool())
            .unwrap_or(true);
        let mut node = UiNode {
            role: name_of(get("accessibilityRole")),
            label: text_of(get("accessibilityLabel")),
            value: text_of(get("accessibilityValue")),
            frame: Rect {
                x: frame.origin.x,
                y: frame.origin.y,
                w: frame.size.width,
                h: frame.size.height,
            },
            enabled,
            children: Vec::new(),
        };
        if depth + 1 >= MAX_DEPTH {
            return node;
        }
        let kids = get("accessibilityChildren");
        let n = guarded(kids, "count", |o, s| send!(o, s => usize)).unwrap_or(0);
        for i in 0..n {
            if self.nodes >= MAX_NODES || Instant::now() >= self.end {
                break;
            }
            let c = guarded(
                kids,
                "objectAtIndex:",
                |o, s| send!(o, s => *mut AnyObject, i => usize),
            )
            .unwrap_or(std::ptr::null_mut());
            if !c.is_null() {
                let child = self.node(c, depth + 1);
                node.children.push(child);
            }
        }
        node
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn one_request_is_in_flight_at_a_time() {
        let in_flight = Arc::new(AtomicU32::new(0));
        let most = Arc::new(AtomicU32::new(0));
        let (done_tx, done_rx) = mpsc::channel::<u32>();
        let (f, m) = (Arc::clone(&in_flight), Arc::clone(&most));
        let serial = Arc::new(Serial::start(
            "ax-test",
            move |(n, tx): (u32, Sender<u32>)| {
                let now = f.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(15));
                f.fetch_sub(1, Ordering::SeqCst);
                let _ = tx.send(n);
            },
        ));
        // Four callers at once, from four threads.
        let threads: Vec<_> = (0..4)
            .map(|n| {
                let s = Arc::clone(&serial);
                let tx = done_tx.clone();
                std::thread::spawn(move || assert!(s.submit((n, tx))))
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let mut got: Vec<u32> = (0..4)
            .map(|_| done_rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        got.sort();
        assert_eq!(got, [0, 1, 2, 3], "every request answered");
        assert_eq!(most.load(Ordering::SeqCst), 1, "never two at once");
        // A flood beyond the waiting room is refused (the caller answers busy), not queued without end.
        let (block_tx, block_rx) = mpsc::channel::<()>();
        let block_rx = Mutex::new(block_rx);
        let s = Serial::start("ax-test-2", move |_: u32| {
            let _ = block_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
        });
        let accepted = (0..20).filter(|n| s.submit(*n)).count();
        assert_eq!(accepted, MAX_WAITING + 1, "one in flight and eight waiting");
        for _ in 0..accepted {
            let _ = block_tx.send(());
        }
    }

    #[test]
    fn the_bridge_answers_the_translators_questions() {
        let b = Bridge::new();
        let p = Retained::as_ptr(&b).cast_mut().cast::<AnyObject>();
        let tok = NSString::from_str("no-such-token");
        let tp = Retained::as_ptr(&tok).cast_mut().cast::<AnyObject>();
        let r = guarded(
            p,
            "accessibilityTranslationDelegateBridgeCallbackWithToken:",
            |o, s| send!(o, s => *mut AnyObject, tp => *mut AnyObject),
        );
        assert_eq!(r, Ok(std::ptr::null_mut()), "an unknown token has no block");
        let rect = CGRect {
            origin: CGPoint { x: 1.0, y: 2.0 },
            size: objc2_core_foundation::CGSize {
                width: 3.0,
                height: 4.0,
            },
        };
        // SAFETY: the method the class defines, with its own types.
        let back: CGRect = unsafe {
            msg_send![&*b, accessibilityTranslationConvertPlatformFrameToSystem: rect, withToken: tp]
        };
        assert_eq!(back, rect, "frames are device points already");
        assert_eq!(name_of(tp), "no-such-token");
        assert_eq!(text_of(std::ptr::null_mut()), "");
    }

    /// The real simulator (opt-in, `FLARE_DEVICE_REAL=1`): Settings' tree comes through the bridge with its rows,
    /// labelled, in screen points.
    #[test]
    fn the_ui_tree_of_settings_has_its_labelled_rows() {
        let Some(d) = crate::macos::simctl::TestDevice::boot("Flare Test iPhone") else {
            return;
        };
        load().expect("AccessibilityPlatformTranslation loads");
        let sim = crate::macos::sim::Sim::open(&d.developer_dir).expect("the device set");
        let dev = sim.device(&d.target).expect("the device");
        d.simctl
            .launch(&d.target, "com.apple.Preferences", &[])
            .expect("launch Settings");
        std::thread::sleep(Duration::from_secs(3));
        let ax = Ax::open().expect("the translator");
        register(d.target.udid(), &dev);
        // The first request after a boot is slow (S4: 3.3 s), and may come back empty: the sim host warms the tree
        // right after attach for this reason.
        let started = Instant::now();
        let mut first = ax.tree(d.target.udid(), None);
        for i in 0..2 {
            eprintln!(
                "try {i}: {} ms {:?}",
                started.elapsed().as_millis(),
                first.as_ref().map(all_count)
            );
            if first.is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
            first = ax.tree(d.target.udid(), None);
        }
        let first = first.expect("a tree");
        let cold = started.elapsed();
        let started = Instant::now();
        let root = ax.tree(d.target.udid(), None).expect("a tree");
        let warm = started.elapsed();
        fn all(n: &UiNode, out: &mut Vec<UiNode>) {
            out.push(UiNode {
                children: Vec::new(),
                ..n.clone()
            });
            n.children.iter().for_each(|c| all(c, out));
        }
        let mut nodes = Vec::new();
        all(&root, &mut nodes);
        eprintln!(
            "Settings: {} elements, first {} ms, warm {} ms; {:?}",
            nodes.len(),
            cold.as_millis(),
            warm.as_millis(),
            nodes
                .iter()
                .filter(|n| !n.label.is_empty())
                .map(|n| &n.label)
                .take(8)
                .collect::<Vec<_>>()
        );
        assert_eq!(all_count(&first), nodes.len(), "the same screen twice");
        assert!(nodes.len() >= 8, "{nodes:?}");
        assert!(
            nodes.iter().filter(|n| !n.label.is_empty()).count() >= 5,
            "{nodes:?}"
        );
        assert_eq!(
            (root.frame.w, root.frame.h),
            (402.0, 874.0),
            "screen points"
        );
        assert!(
            nodes
                .iter()
                .all(|n| n.frame.x >= -1.0 && n.frame.x + n.frame.w <= 403.0),
            "every frame inside the screen"
        );
        // An element at a point: the hit test answers too.
        let row = nodes
            .iter()
            .find(|n| !n.label.is_empty() && n.frame.w > 100.0)
            .expect("a labelled row");
        let (cx, cy) = (
            row.frame.x + row.frame.w / 2.0,
            row.frame.y + row.frame.h / 2.0,
        );
        let hit = ax.tree(d.target.udid(), Some((cx, cy))).expect("a hit");
        assert!(hit.frame.w > 0.0, "{hit:?}");
        forget(d.target.udid());
        assert!(
            ax.tree(d.target.udid(), None).is_err(),
            "a forgotten token has no device"
        );
    }

    fn all_count(n: &UiNode) -> usize {
        1 + n.children.iter().map(all_count).sum::<usize>()
    }
}
