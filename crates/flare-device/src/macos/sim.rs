//! The simulator's screen in the sim host (spec "Frames: 60 fps with no CPU copy"): CoreSimulator's service context,
//! the device set, a device by udid, its device type (point size and scale, never guessed from pixels), its I/O
//! ports, the `SimScreenAdapter` LCD screen, `framebufferSurface`, and the screen callbacks. Every private call goes
//! through [`guarded`] (rule 1310). Proxies get plain message sends, never KVC.
//!
//! A [`Stream`] per attached device runs on its own serial queue: each frame callback bumps the "changed" count (sent
//! even while paused), passes the 60 fps cap ([`FrameCap`]), takes a free ring slot, fills it with the GPU pass and
//! sends `(device, slot, seq, t_us)`. With no free slot (Flare is behind) the frame is dropped and counted. If the
//! screen does not offer `registerScreenCallbacks...` (an older SimulatorKit), it falls back to
//! `registerCallbackWithUUID:ioSurfacesChangeCallback:` and the damage-rectangles callback, as idb does.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use block2::{ManualBlockEncoding, RcBlock};
use dispatch2::{DispatchQueue, DispatchRetained, DispatchTime};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, AnyProtocol, Bool};
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_foundation::{NSString, NSUUID};
use objc2_io_surface::IOSurfaceRef;

use super::guarded::{
    Broken, CALLBACK_LIMIT, answered, class, class_obj, guarded, responds, sel, send,
};
use super::ring::{Gpu, GpuRing, RingSet, read_row};
use crate::cap::FrameCap;
use crate::geom::{Orientation, Size};
use crate::source::Screen;
use crate::target::Target;
use crate::wire::{Changed, FrameRecord, Record};

/// Where the sim host's frames and rings go: Flare (over the mach channel and stdout) or the bench.
pub trait Sink: Send + Sync {
    /// A new ring for `target`: its number (`device`, carried by its records), its id and the screen.
    fn ring(
        &self,
        target: &Target,
        device: u32,
        ring: u32,
        screen: Screen,
        gpu_ring: &Arc<GpuRing>,
    );
    /// Sends a record: false when it did not reach the consumer (a frame's slot is then free again at once).
    fn record(&self, record: Record) -> bool;
    fn log(&self, line: &str);
}

/// The type encodings CoreSimulator's proxies need to forward a block over XPC ("Block is missing signature field"
/// otherwise): no arguments, one object, two objects; each returns void.
pub(crate) struct Enc0;
// SAFETY: the encoding matches the arguments and return type (64-bit: the block itself at 0, objects 8 bytes each).
unsafe impl ManualBlockEncoding for Enc0 {
    type Arguments = ();
    type Return = ();
    const ENCODING_CSTR: &'static std::ffi::CStr = c"v8@?0";
}
pub(crate) struct Enc1;
// SAFETY: as above.
unsafe impl ManualBlockEncoding for Enc1 {
    type Arguments = (*mut AnyObject,);
    type Return = ();
    const ENCODING_CSTR: &'static std::ffi::CStr = c"v16@?0@8";
}
pub(crate) struct Enc2;
// SAFETY: as above.
unsafe impl ManualBlockEncoding for Enc2 {
    type Arguments = (*mut AnyObject, *mut AnyObject);
    type Return = ();
    const ENCODING_CSTR: &'static std::ffi::CStr = c"v24@?0@8@16";
}

/// Monotonic microseconds since the sim host started (the records' `t_us`).
pub fn now_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    u64::try_from(START.get_or_init(Instant::now).elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// An object that crosses to the callback queue (the proxies are thread-safe; CoreSimulator calls them from its own
/// queues).
pub(crate) struct Sendable<T>(pub(crate) T);
// SAFETY: see the type's comment.
unsafe impl<T> Send for Sendable<T> {}

/// The text of an object (`description`), for errors and logs.
pub(crate) fn describe(obj: *mut AnyObject) -> String {
    text(obj)
}

/// The text of an object (`description`), for errors.
fn text(obj: *mut AnyObject) -> String {
    if obj.is_null() {
        return String::new();
    }
    guarded(obj, "description", |o, s| {
        let d = send!(o, s => *mut AnyObject);
        if d.is_null() {
            String::new()
        } else {
            // SAFETY: `description` returns an NSString.
            unsafe { &*d.cast::<NSString>() }.to_string()
        }
    })
    .unwrap_or_default()
}

/// Sends a getter that returns an object, retained; [`Broken`] for nil.
fn get(obj: *mut AnyObject, name: &str) -> Result<Retained<AnyObject>, Broken> {
    let p = guarded(obj, name, |o, s| send!(o, s => *mut AnyObject))?;
    // SAFETY: an object pointer the getter returned (+0), retained here.
    unsafe { Retained::retain(p) }.ok_or_else(|| Broken::nothing(name, ""))
}

fn ptr(o: &Retained<AnyObject>) -> *mut AnyObject {
    Retained::as_ptr(o).cast_mut()
}

/// An `NSArray`'s items (public API, but sent through the guard like the rest: the array may be a proxy's).
fn items(arr: *mut AnyObject) -> Vec<*mut AnyObject> {
    let n = guarded(arr, "count", |o, s| send!(o, s => usize)).unwrap_or(0);
    (0..n.min(256))
        .filter_map(|i| {
            guarded(
                arr,
                "objectAtIndex:",
                |o, s| send!(o, s => *mut AnyObject, i => usize),
            )
            .ok()
        })
        .filter(|p| !p.is_null())
        .collect()
}

/// An IOSurface returned as an object (`framebufferSurface`), retained, or None.
fn surface_of(obj: *mut AnyObject) -> Option<CFRetained<IOSurfaceRef>> {
    let p = NonNull::new(obj.cast::<IOSurfaceRef>())?;
    // SAFETY: the object is an IOSurface (toll-free bridged to IOSurfaceRef), retained here.
    Some(unsafe { CFRetained::retain(p) })
}

/// CoreSimulator's device set for one Xcode.
pub struct Sim {
    set: Retained<AnyObject>,
}

// SAFETY: the device set is used from the sim host's request threads one call at a time; CoreSimulator's objects are
// thread-safe.
unsafe impl Send for Sim {}
// SAFETY: as above.
unsafe impl Sync for Sim {}

impl Sim {
    /// The service context and default device set of `developer_dir`. BLOCKING (an XPC round trip).
    pub fn open(developer_dir: &Path) -> Result<Sim, Broken> {
        autoreleasepool(|_| {
            let ctx_class = class_obj(class("SimServiceContext")?);
            let name = "sharedServiceContextForDeveloperDir:error:";
            let dir = NSString::from_str(&developer_dir.to_string_lossy());
            let (ctx, err) = guarded(ctx_class, name, |o, s| {
                let mut err: *mut AnyObject = std::ptr::null_mut();
                let r = send!(o, s => *mut AnyObject,
                    Retained::as_ptr(&dir).cast_mut().cast::<AnyObject>() => *mut AnyObject,
                    &mut err => *mut *mut AnyObject);
                (r, err)
            })?;
            if ctx.is_null() {
                return Err(Broken::nothing(name, text(err)));
            }
            let name = "defaultDeviceSetWithError:";
            let (set, err) = guarded(ctx, name, |o, s| {
                let mut err: *mut AnyObject = std::ptr::null_mut();
                let r = send!(o, s => *mut AnyObject, &mut err => *mut *mut AnyObject);
                (r, err)
            })?;
            // SAFETY: the set the call returned (+0), retained here.
            let set =
                unsafe { Retained::retain(set) }.ok_or_else(|| Broken::nothing(name, text(err)))?;
            Ok(Sim { set })
        })
    }

    /// The device of `target`. BLOCKING.
    pub fn device(&self, target: &Target) -> Result<Device, Broken> {
        autoreleasepool(|_| {
            let by_udid = get(ptr(&self.set), "devicesByUDID")?;
            let uuid = NSUUID::from_string(&NSString::from_str(target.id()))
                .ok_or_else(|| Broken::nothing("NSUUID", target.id()))?;
            let dev = guarded(
                ptr(&by_udid),
                "objectForKey:",
                |o, s| send!(o, s => *mut AnyObject, Retained::as_ptr(&uuid).cast_mut().cast::<AnyObject>() => *mut AnyObject),
            )?;
            // SAFETY: the device the dictionary holds (+0), retained here.
            let obj = unsafe { Retained::retain(dev) }.ok_or_else(|| {
                Broken::nothing("devicesByUDID", format!("no device {}", target.id()))
            })?;
            Ok(Device { obj })
        })
    }
}

/// One simulator device.
pub struct Device {
    obj: Retained<AnyObject>,
}

// SAFETY: a CoreSimulator device object, used from the sim host's threads one call at a time; CoreSimulator's objects
// are thread-safe.
unsafe impl Send for Device {}
// SAFETY: as above.
unsafe impl Sync for Device {}

impl Device {
    /// The `SimDevice` itself, for the private calls of input, orientation and the UI tree.
    pub fn ptr(&self) -> *mut AnyObject {
        ptr(&self.obj)
    }

    /// "Booted", "Shutdown", ...
    pub fn state(&self) -> Result<String, Broken> {
        autoreleasepool(|_| Ok(text(ptr(&get(ptr(&self.obj), "stateString")?))))
    }

    /// The main screen's size in pixels (portrait) and its scale, from the device type.
    pub fn screen_facts(&self) -> Result<(Size, f64), Broken> {
        autoreleasepool(|_| {
            let dt = get(ptr(&self.obj), "deviceType")?;
            let px = guarded(ptr(&dt), "mainScreenSize", |o, s| send!(o, s => CGSize))?;
            let scale = guarded(ptr(&dt), "mainScreenScale", |o, s| send!(o, s => f32))?;
            if !(px.width > 0.0 && px.height > 0.0 && scale > 0.0) {
                return Err(Broken::nothing("mainScreenSize", "an empty screen"));
            }
            Ok((Size::new(px.width, px.height), f64::from(scale)))
        })
    }

    /// The I/O port descriptors (`io` -> `ioPorts` -> each `descriptor`).
    fn descriptors(&self) -> Result<Vec<Retained<AnyObject>>, Broken> {
        let io = get(ptr(&self.obj), "io")?;
        let ports = get(ptr(&io), "ioPorts")?;
        Ok(items(ptr(&ports))
            .into_iter()
            .filter_map(|p| get(p, "descriptor").ok())
            .collect())
    }
}

/// Whether `obj` conforms to the protocol `name` (asked of the object, so a proxy answers).
fn conforms(obj: *mut AnyObject, name: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(name) else {
        return false;
    };
    let Some(p) = AnyProtocol::get(&c) else {
        return false;
    };
    guarded(obj, "conformsToProtocol:", |o, s| {
        send!(o, s => Bool, (p as *const AnyProtocol).cast_mut().cast::<c_void>() => *mut c_void)
    })
    .is_ok_and(|b| b.as_bool())
}

/// The screen a device shows, and how its callbacks are registered.
enum Screen0 {
    /// `SimScreen` (Xcode 27): one call registers frame, surfaces and properties callbacks.
    Modern(Retained<AnyObject>),
    /// `SimDisplayIOSurfaceRenderable` and `SimDisplayRenderable` only (an older SimulatorKit, idb's fallback).
    Legacy(Retained<AnyObject>),
}

const REGISTER: &str = "registerScreenCallbacksWithUUID:callbackQueue:frameCallback:surfacesChangedCallback:propertiesChangedCallback:";
const UNREGISTER: &str = "unregisterScreenCallbacksWithUUID:";
const REGISTER_SURFACES: &str = "registerCallbackWithUUID:ioSurfacesChangeCallback:";
const UNREGISTER_SURFACES: &str = "unregisterIOSurfacesChangeCallbackWithUUID:";
const REGISTER_DAMAGE: &str = "registerCallbackWithUUID:damageRectanglesCallback:";
const UNREGISTER_DAMAGE: &str = "unregisterDamageRectanglesCallbackWithUUID:";
const ENUMERATE: &str = "enumerateScreensWithCompletionQueue:completionHandler:";

/// The LCD screen (the one whose framebuffer has a size) and its framebuffer.
fn lcd_screen(
    dev: &Device,
    queue: &DispatchQueue,
) -> Result<(Screen0, CFRetained<IOSurfaceRef>), Broken> {
    let descriptors = autoreleasepool(|_| dev.descriptors())?;
    if let Some(adapter) = descriptors
        .iter()
        .find(|d| conforms(ptr(d), "SimScreenAdapter"))
    {
        let adapter = Sendable(adapter.clone());
        let found = answered(ENUMERATE, CALLBACK_LIMIT, |done| {
            let done = Mutex::new(Some(done));
            let block = RcBlock::with_encoding::<_, _, _, Enc2>(
                move |screens: *mut AnyObject, err: *mut AnyObject| {
                    let pick = objc2::exception::catch(std::panic::AssertUnwindSafe(|| {
                        autoreleasepool(|_| {
                            items(screens).into_iter().find_map(|sc| {
                                let fb = guarded(
                                    sc,
                                    "framebufferSurface",
                                    |o, s| send!(o, s => *mut AnyObject),
                                )
                                .ok()
                                .and_then(surface_of)?;
                                if fb.width() == 0 || fb.height() == 0 {
                                    return None;
                                }
                                // SAFETY: the screen object from the array (+0), retained here.
                                let sc = unsafe { Retained::retain(sc) }?;
                                Some(Sendable((sc, fb)))
                            })
                        })
                    }))
                    .ok()
                    .flatten();
                    let why = if pick.is_none() {
                        text(err)
                    } else {
                        String::new()
                    };
                    if let Some(d) = done.lock().ok().and_then(|mut d| d.take()) {
                        d(pick.ok_or(why));
                    }
                },
            );
            let a = &adapter;
            guarded(ptr(&a.0), ENUMERATE, |o, s| {
                send!(o, s => (),
                    (queue as *const DispatchQueue).cast_mut().cast::<c_void>() => *mut c_void,
                    RcBlock::as_ptr(&block).cast::<c_void>() => *mut c_void)
            })
        })?;
        let Sendable((screen, fb)) = found.map_err(|why| Broken::nothing(ENUMERATE, why))?;
        if responds(ptr(&screen), sel(REGISTER)) {
            return Ok((Screen0::Modern(screen), fb));
        }
        if responds(ptr(&screen), sel(REGISTER_SURFACES)) {
            return Ok((Screen0::Legacy(screen), fb));
        }
        return Err(Broken::missing(REGISTER));
    }
    // No adapter: a port descriptor that is itself a renderable display (an older SimulatorKit).
    let legacy = descriptors
        .into_iter()
        .find(|d| conforms(ptr(d), "SimDisplayIOSurfaceRenderable"))
        .ok_or_else(|| Broken::missing("SimScreenAdapter"))?;
    let fb = autoreleasepool(|_| {
        guarded(
            ptr(&legacy),
            "framebufferSurface",
            |o, s| send!(o, s => *mut AnyObject),
        )
        .ok()
        .and_then(surface_of)
    })
    .ok_or_else(|| Broken::nothing("framebufferSurface", ""))?;
    Ok((Screen0::Legacy(legacy), fb))
}

/// What a stream has done, for the once-a-minute log line and the bench.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub callbacks: u64,
    pub sent: u64,
    pub dropped: u64,
    pub capped: u64,
    pub fill_errors: u64,
}

struct Inner {
    rings: RingSet<Arc<GpuRing>>,
    cap: FrameCap<u64>,
    fb: Option<CFRetained<IOSurfaceRef>>,
    seq: u64,
    changed: u64,
    paused: bool,
    timer: bool,
    ring_count: u32,
    stats: Stats,
    /// GPU ms per filled slot (the bench reads them; kept to the last 100 000).
    gpu_ms: Vec<f64>,
    /// The probe app's barcode of the last slot sent (`--probe-barcode`).
    barcode: Option<u16>,
}

/// One attached device's frame stream.
pub struct Stream {
    pub target: Target,
    inner: Mutex<Inner>,
    sink: Arc<dyn Sink>,
    gpu: Arc<Gpu>,
    ids: Arc<AtomicU32>,
    queue: DispatchRetained<DispatchQueue>,
    scale: f64,
    probe_barcode: bool,
    /// The interface's orientation (orient.rs keeps it), carried in each new ring's screen facts.
    orientation: Arc<Mutex<Orientation>>,
}

// SAFETY: every field is used under the mutex or is thread-safe (the queue, the GPU, the sink).
unsafe impl Send for Stream {}
// SAFETY: as above.
unsafe impl Sync for Stream {}

impl Stream {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The frame callback (the device's queue).
    fn on_frame(self: &Arc<Self>) {
        let mut g = self.lock();
        g.stats.callbacks += 1;
        g.changed += 1;
        if let Some(c) = g.rings.current() {
            let _ = self.sink.record(Record::Changed(Changed {
                device: c.device,
                count: g.changed,
            }));
        }
        if g.paused {
            return;
        }
        g.seq += 1;
        let now = now_us();
        let seq = g.seq;
        match g.cap.offer(seq, now) {
            Some(seq) => self.emit(&mut g, seq, now),
            None => self.arm(&mut g, now),
        }
    }

    /// Fires the cap's timer when the held frame is due.
    fn arm(self: &Arc<Self>, g: &mut Inner, now: u64) {
        if g.timer {
            return;
        }
        let Some(due) = g.cap.due_us() else {
            return;
        };
        let Ok(when) = DispatchTime::try_from(Duration::from_micros(due.saturating_sub(now)))
        else {
            return;
        };
        g.timer = true;
        let me = Arc::clone(self);
        let _ = self.queue.after(when, move || me.on_timer());
    }

    fn on_timer(self: &Arc<Self>) {
        let mut g = self.lock();
        g.timer = false;
        if g.paused {
            return;
        }
        let now = now_us();
        match g.cap.poll(now) {
            Some(seq) => self.emit(&mut g, seq, now),
            None => self.arm(&mut g, now),
        }
    }

    /// The screen facts of a framebuffer of `w` x `h` pixels.
    fn screen(&self, w: usize, h: usize) -> Screen {
        let px = Size::new(w as f64, h as f64);
        Screen {
            size_px: px,
            size_pt: Size::new(px.w / self.scale, px.h / self.scale),
            scale: self.scale,
            // The framebuffer stays portrait (spike S5); the interface's orientation is what orient.rs last read.
            orientation: *self.orientation.lock().unwrap_or_else(|p| p.into_inner()),
        }
    }

    /// A ring for a framebuffer of `w` x `h`, numbered anew, handed to the sink.
    fn new_ring(&self, g: &mut Inner, w: usize, h: usize) -> bool {
        let ring = match self.gpu.ring(w, h) {
            Ok(r) => Arc::new(r),
            Err(e) => {
                self.sink.log(&format!("{}: no ring: {e}", self.target));
                return false;
            }
        };
        let device = self.ids.fetch_add(1, Ordering::Relaxed);
        g.ring_count += 1;
        let _freed = g.rings.replace(device, g.ring_count, Arc::clone(&ring));
        self.sink
            .ring(&self.target, device, g.ring_count, self.screen(w, h), &ring);
        true
    }

    /// Fills a free slot from the framebuffer and sends its record.
    fn emit(&self, g: &mut Inner, seq: u64, t_us: u64) {
        let Some(fb) = g.fb.clone() else {
            return;
        };
        let (w, h) = (fb.width(), fb.height());
        let fits = g
            .rings
            .current()
            .is_some_and(|c| c.payload.width == w && c.payload.height == h);
        if !fits && !self.new_ring(g, w, h) {
            return;
        }
        let Some((slot, owned)) = g.rings.take_free() else {
            g.stats.dropped += 1;
            return;
        };
        let device = owned.device;
        let ring = Arc::clone(&owned.payload);
        match self.gpu.fill(&fb, &ring.textures[usize::from(slot)]) {
            Ok(ms) => {
                if g.gpu_ms.len() < 100_000 {
                    g.gpu_ms.push(ms);
                }
            }
            Err(e) => {
                g.stats.fill_errors += 1;
                if g.stats.fill_errors == 1 {
                    self.sink.log(&format!("{}: {e}", self.target));
                }
                // The slot holds an older frame: it is not sent, and is free again.
                g.rings.done(device, slot);
                return;
            }
        }
        if self.probe_barcode {
            g.barcode = barcode(&ring.surfaces[usize::from(slot)], self.scale);
        }
        let sent = self.sink.record(Record::Frame(FrameRecord {
            device,
            slot: u32::from(slot),
            seq,
            t_us,
        }));
        if sent {
            g.stats.sent += 1;
        } else {
            // Flare never heard of it, so it never says the slot is done: it is free again now, or the ring would
            // run out of slots for good after a few full ports (the sink counts the record it could not send).
            g.rings.done(device, slot);
        }
    }

    /// The surfaces callback (the device's queue): a new framebuffer (a new device size makes a new ring at the next
    /// frame).
    fn on_surfaces(&self, unmasked: *mut AnyObject, masked: *mut AnyObject) {
        let s = surface_of(if unmasked.is_null() { masked } else { unmasked });
        if let Some(s) = s.filter(|s| s.width() > 0 && s.height() > 0) {
            self.lock().fb = Some(s);
        }
    }

    /// Flare is done with a slot (the slot-done thread).
    pub fn slot_done(&self, device: u32, slot: u32) -> bool {
        let Ok(slot) = u8::try_from(slot) else {
            return false;
        };
        self.lock().rings.done(device, slot).0
    }

    /// Stops (true) or restarts (false) the frames; the changed count goes on. A restart sends the current screen at
    /// once.
    pub fn pause(self: &Arc<Self>, paused: bool) {
        self.lock().paused = paused;
        if !paused {
            let me = Arc::clone(self);
            self.queue.exec_async(move || {
                let mut g = me.lock();
                g.seq += 1;
                let (seq, now) = (g.seq, now_us());
                match g.cap.offer(seq, now) {
                    Some(seq) => me.emit(&mut g, seq, now),
                    None => me.arm(&mut g, now),
                }
            });
        }
    }

    pub fn stats(&self) -> Stats {
        let g = self.lock();
        Stats {
            dropped: g.rings.dropped().max(g.stats.dropped),
            capped: g.cap.capped(),
            ..g.stats
        }
    }

    /// The GPU times so far, in ms.
    pub fn gpu_ms(&self) -> Vec<f64> {
        self.lock().gpu_ms.clone()
    }

    /// The barcode of the last slot sent (`--probe-barcode`).
    pub fn last_barcode(&self) -> Option<u16> {
        self.lock().barcode
    }

    /// The device's framebuffer itself (fresh while paused: snapshots read it, not a ring slot).
    pub fn framebuffer(&self) -> Option<CFRetained<IOSurfaceRef>> {
        self.lock().fb.clone()
    }

    /// The screen's scale (pixels per point).
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// The screen's portrait size in points.
    pub fn portrait_pt(&self) -> Option<Size> {
        let fb = self.framebuffer()?;
        Some(Size::new(
            fb.width() as f64 / self.scale,
            fb.height() as f64 / self.scale,
        ))
    }
}

/// The probe app's 16-bit frame barcode (the band at 120 to 180 pt, read at 150 pt) in an RGBA or BGRA surface.
pub fn barcode(s: &IOSurfaceRef, scale: f64) -> Option<u16> {
    let y = (150.0 * scale) as usize;
    let row = read_row(s, y)?;
    let w = s.width();
    let mut v: u16 = 0;
    for i in 0..16 {
        let x = ((i as f64 + 0.5) * w as f64 / 16.0) as usize;
        let p = row.get(x * 4..x * 4 + 3)?;
        let lit = u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) > 384;
        v = (v << 1) | u16::from(lit);
    }
    Some(v)
}

/// An attached device: its stream and the registration to undo at detach.
pub struct Attached {
    pub stream: Arc<Stream>,
    screen: Screen0,
    uuid: Retained<NSUUID>,
    _blocks: Vec<Box<dyn std::any::Any>>,
}

// SAFETY: the screen proxy and the blocks are only touched at detach, from one thread.
unsafe impl Send for Attached {}

/// Options of an attach.
pub struct AttachOptions {
    pub sink: Arc<dyn Sink>,
    pub gpu: Arc<Gpu>,
    /// Hands out ring numbers (`device` in records), unique in the sim host.
    pub ids: Arc<AtomicU32>,
    pub probe_barcode: bool,
    /// The interface's orientation, shared with whoever reads it (orient.rs).
    pub orientation: Arc<Mutex<Orientation>>,
    /// Called on the screen's properties callback (it fires about 270 ms after a rotation, spike S5).
    pub on_props: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Attaches to a booted device: reads its screen, makes its ring, registers the callbacks. The ring goes to the sink
/// at once with the current framebuffer. BLOCKING (enumerate screens, up to 5 s).
pub fn attach(sim: &Sim, target: &Target, opts: AttachOptions) -> Result<Attached, Broken> {
    let dev = sim.device(target)?;
    let state = dev.state()?;
    if state != "Booted" {
        return Err(Broken::nothing(
            "stateString",
            format!("the device is {state}"),
        ));
    }
    let (_px, scale) = dev.screen_facts()?;
    let queue = DispatchQueue::new(&format!("com.flare.sim-host.{}", target.id()), None);
    let (screen, fb) = lcd_screen(&dev, &queue)?;
    let stream = Arc::new(Stream {
        target: target.clone(),
        inner: Mutex::new(Inner {
            rings: RingSet::default(),
            cap: FrameCap::default(),
            fb: Some(fb),
            seq: 0,
            changed: 0,
            paused: false,
            timer: false,
            ring_count: 0,
            stats: Stats::default(),
            gpu_ms: Vec::new(),
            barcode: None,
        }),
        sink: opts.sink,
        gpu: opts.gpu,
        ids: opts.ids,
        queue: queue.clone(),
        scale,
        probe_barcode: opts.probe_barcode,
        orientation: opts.orientation,
    });
    // The surfaces callback does not fire on attach: the ring is made from the framebuffer read above.
    {
        let mut g = stream.lock();
        let (w, h) =
            g.fb.as_ref()
                .map(|f| (f.width(), f.height()))
                .unwrap_or((0, 0));
        if !stream.new_ring(&mut g, w, h) {
            return Err(Broken::nothing("IOSurfaceCreate", "no ring"));
        }
    }
    let uuid = NSUUID::new();
    let uuid_p = Retained::as_ptr(&uuid).cast_mut().cast::<AnyObject>();
    let q = (&*queue as *const DispatchQueue)
        .cast_mut()
        .cast::<c_void>();
    let mut blocks: Vec<Box<dyn std::any::Any>> = Vec::new();
    let s1 = Arc::clone(&stream);
    let frame = RcBlock::with_encoding::<_, _, _, Enc0>(move || {
        let _ = objc2::exception::catch(std::panic::AssertUnwindSafe(|| s1.on_frame()));
    });
    let s2 = Arc::clone(&stream);
    let surfaces = RcBlock::with_encoding::<_, _, _, Enc2>(
        move |unmasked: *mut AnyObject, masked: *mut AnyObject| {
            let _ = objc2::exception::catch(std::panic::AssertUnwindSafe(|| {
                s2.on_surfaces(unmasked, masked)
            }));
        },
    );
    match &screen {
        Screen0::Modern(sc) => {
            let on_props = opts.on_props.clone();
            let props = RcBlock::with_encoding::<_, _, _, Enc1>(move |_props: *mut AnyObject| {
                if let Some(f) = &on_props {
                    let _ = objc2::exception::catch(std::panic::AssertUnwindSafe(|| f()));
                }
            });
            guarded(ptr(sc), REGISTER, |o, s| {
                send!(o, s => (),
                    uuid_p => *mut AnyObject,
                    q => *mut c_void,
                    RcBlock::as_ptr(&frame).cast::<c_void>() => *mut c_void,
                    RcBlock::as_ptr(&surfaces).cast::<c_void>() => *mut c_void,
                    RcBlock::as_ptr(&props).cast::<c_void>() => *mut c_void)
            })?;
            blocks.push(Box::new(props));
        }
        Screen0::Legacy(sc) => {
            let s3 = Arc::clone(&stream);
            let one_surface =
                RcBlock::with_encoding::<_, _, _, Enc1>(move |surface: *mut AnyObject| {
                    let _ = objc2::exception::catch(std::panic::AssertUnwindSafe(|| {
                        s3.on_surfaces(surface, std::ptr::null_mut())
                    }));
                });
            let s4 = Arc::clone(&stream);
            let damage = RcBlock::with_encoding::<_, _, _, Enc1>(move |_rects: *mut AnyObject| {
                let s = Arc::clone(&s4);
                // The damage callback comes on CoreSimulator's queue: the frame work runs on the device's.
                s4.queue.exec_async(move || {
                    let _ = objc2::exception::catch(std::panic::AssertUnwindSafe(|| s.on_frame()));
                });
            });
            guarded(ptr(sc), REGISTER_SURFACES, |o, s| {
                send!(o, s => (), uuid_p => *mut AnyObject,
                    RcBlock::as_ptr(&one_surface).cast::<c_void>() => *mut c_void)
            })?;
            guarded(ptr(sc), REGISTER_DAMAGE, |o, s| {
                send!(o, s => (), uuid_p => *mut AnyObject,
                    RcBlock::as_ptr(&damage).cast::<c_void>() => *mut c_void)
            })?;
            blocks.push(Box::new(one_surface));
            blocks.push(Box::new(damage));
        }
    }
    blocks.push(Box::new(frame));
    blocks.push(Box::new(surfaces));
    // The current screen goes out at once.
    stream.pause(false);
    Ok(Attached {
        stream,
        screen,
        uuid,
        _blocks: blocks,
    })
}

impl Attached {
    /// Unregisters the callbacks and waits for the device's queue to run dry; the rings go when Flare lets go of
    /// them. BLOCKING (briefly).
    pub fn detach(self) {
        let uuid_p = Retained::as_ptr(&self.uuid).cast_mut().cast::<AnyObject>();
        let calls: &[&str] = match &self.screen {
            Screen0::Modern(_) => &[UNREGISTER],
            Screen0::Legacy(_) => &[UNREGISTER_SURFACES, UNREGISTER_DAMAGE],
        };
        let sc = match &self.screen {
            Screen0::Modern(s) | Screen0::Legacy(s) => ptr(s),
        };
        for name in calls {
            let _ = guarded(sc, name, |o, s| send!(o, s => (), uuid_p => *mut AnyObject));
        }
        self.stream.lock().paused = true;
        self.stream.queue.exec_sync(|| {});
        self.stream.lock().rings.take_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macos::ring::{fill_surface, rgba_surface};

    /// A sink whose consumer never hears a record (Flare's port stayed full).
    struct Deaf;
    impl Sink for Deaf {
        fn ring(&self, _: &Target, _: u32, _: u32, _: Screen, _: &Arc<GpuRing>) {}
        fn record(&self, _: Record) -> bool {
            false
        }
        fn log(&self, _: &str) {}
    }

    #[test]
    fn a_frame_record_that_never_reaches_flare_frees_its_slot() {
        let gpu = Arc::new(Gpu::new().expect("a Metal device"));
        let fb = crate::macos::ring::surface(8, 4, u32::from_be_bytes(*b"BGRA")).unwrap();
        let stream = Stream {
            target: Target::sim("00000000-0000-0000-0000-000000000001").unwrap(),
            inner: Mutex::new(Inner {
                rings: RingSet::default(),
                cap: FrameCap::default(),
                fb: Some(fb),
                seq: 0,
                changed: 0,
                paused: false,
                timer: false,
                ring_count: 0,
                stats: Stats::default(),
                gpu_ms: Vec::new(),
                barcode: None,
            }),
            sink: Arc::new(Deaf),
            gpu,
            ids: Arc::new(AtomicU32::new(1)),
            queue: DispatchQueue::new("com.flare.sim-host.test", None),
            scale: 2.0,
            probe_barcode: false,
            orientation: Arc::new(Mutex::new(Orientation::Rot0)),
        };
        let mut g = stream.lock();
        for seq in 1..=10 {
            stream.emit(&mut g, seq, seq * 16_667);
        }
        assert_eq!(
            g.rings.dropped(),
            0,
            "no frame lacked a slot: every unheard frame gave its slot back"
        );
        assert_eq!(g.stats.sent, 0);
    }

    #[test]
    fn the_probe_barcode_reads_from_a_slot() {
        // 16 columns at 3x, bit pattern 0b1010_0000_1111_0001 on the row at 150 pt.
        let (w, h) = (64usize, 600usize);
        let s = rgba_surface(w, h).unwrap();
        fill_surface(&s, [0, 0, 0, 255]);
        let code: u16 = 0b1010_0000_1111_0001;
        // SAFETY: locked for writing; the row lies inside the surface.
        unsafe {
            s.lock(
                objc2_io_surface::IOSurfaceLockOptions(0),
                std::ptr::null_mut(),
            );
            let row = s
                .base_address()
                .as_ptr()
                .cast::<u8>()
                .add(450 * s.bytes_per_row());
            for x in 0..w {
                let bit = 15 - x * 16 / w;
                let v = if (code >> bit) & 1 == 1 { 255 } else { 0 };
                for c in 0..3 {
                    *row.add(x * 4 + c) = v;
                }
            }
            s.unlock(
                objc2_io_surface::IOSurfaceLockOptions(0),
                std::ptr::null_mut(),
            );
        }
        assert_eq!(barcode(&s, 3.0), Some(code));
        assert_eq!(barcode(&s, 10.0), None, "the row is outside the surface");
    }
}
