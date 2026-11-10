//! The microphone on macOS (port phase 7): CoreAudio. The same seams as WASAPI on Windows (`Endpoints`, `pump`,
//! `Thread`), so the fallback, the lost-device rule and the file seam are the shared code in `mic.rs`.
//!
//! - **Listing** (`list_raw`): the HAL's devices that have input streams, by UID and name. Reading properties
//!   opens no device and records nothing.
//! - **Access first**: before anything is opened the authorization is read with
//!   `AVCaptureDevice authorizationStatusForMediaType:`, which never asks. Off (denied) or restricted is
//!   `OpenError::Denied` at once, without touching CoreAudio: macOS would otherwise hand a denied app silence,
//!   and dictation would say it heard nothing. Never asked yet: the open goes ahead and the system asks once (the
//!   bundle's `NSMicrophoneUsageDescription`, phase 8's Info.plist, words it).
//! - **Capture**: an `AudioQueue` input on the chosen device (by UID; the system default input when none is
//!   chosen, read at each open, so a change of default is followed at the next press), mono `f32` at the
//!   device's own rate, cut to 50 ms buffers and brought to 16 kHz by the shared `downsample`.
//! - **Lost**: the device's `DeviceIsAlive` is read while the queue runs; a device that is gone (unplugged)
//!   ends the capture through `pump`'s `Poll::Lost`, once.

use std::ffi::{c_char, c_void};
use std::sync::mpsc::{Receiver, Sender};

use super::*;

type OSStatus = i32;
type AudioObjectID = u32;
type CFStringRef = *const c_void;
type AudioQueueRef = *mut c_void;

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const SYSTEM_OBJECT: AudioObjectID = 1;
const UNKNOWN_OBJECT: AudioObjectID = 0;
const HW_DEVICES: u32 = fourcc(b"dev#");
const HW_DEFAULT_INPUT: u32 = fourcc(b"dIn ");
const HW_TRANSLATE_UID: u32 = fourcc(b"uidd");
const DEV_STREAMS: u32 = fourcc(b"stm#");
const DEV_UID: u32 = fourcc(b"uid ");
const DEV_ALIVE: u32 = fourcc(b"livn");
const DEV_RATE: u32 = fourcc(b"nsrt");
const OBJ_NAME: u32 = fourcc(b"lnam");
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const SCOPE_INPUT: u32 = fourcc(b"inpt");
const ELEMENT_MAIN: u32 = 0;
const AQ_CURRENT_DEVICE: u32 = fourcc(b"aqcd");
const FORMAT_LPCM: u32 = fourcc(b"lpcm");
const FLAG_FLOAT: u32 = 1;
const FLAG_PACKED: u32 = 8;
const UTF8: u32 = 0x0800_0100;

/// `kAudioHardwareBadDeviceError`.
const BAD_DEVICE: OSStatus = fourcc(b"!dev") as OSStatus;
/// `kAudioQueueErr_InvalidDevice`.
const AQ_INVALID_DEVICE: OSStatus = -66680;
/// `kAudioQueueErr_Permissions`.
const AQ_PERMISSIONS: OSStatus = -66676;

/// How much audio one queue buffer holds: 50 ms (800 samples at 16 kHz from 48 or 44.1 kHz alike).
const BUFFER_MS: f64 = 50.0;
const BUFFERS: usize = 4;
/// How often a running capture asks whether its device is still there.
const ALIVE_EVERY: Duration = Duration::from_millis(250);

#[repr(C)]
#[derive(Clone, Copy)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct StreamFormat {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct QueueBuffer {
    capacity: u32,
    data: *mut c_void,
    byte_size: u32,
    user_data: *mut c_void,
    packet_capacity: u32,
    packets: *mut c_void,
    packet_count: u32,
}

type InputCallback =
    extern "C" fn(*mut c_void, AudioQueueRef, *mut QueueBuffer, *const c_void, u32, *const c_void);

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyDataSize(
        id: AudioObjectID,
        addr: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        out_size: *mut u32,
    ) -> OSStatus;
    fn AudioObjectGetPropertyData(
        id: AudioObjectID,
        addr: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        io_size: *mut u32,
        out: *mut c_void,
    ) -> OSStatus;
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioQueueNewInput(
        format: *const StreamFormat,
        callback: InputCallback,
        user: *mut c_void,
        run_loop: *const c_void,
        run_loop_mode: *const c_void,
        flags: u32,
        out: *mut AudioQueueRef,
    ) -> OSStatus;
    fn AudioQueueAllocateBuffer(
        aq: AudioQueueRef,
        size: u32,
        out: *mut *mut QueueBuffer,
    ) -> OSStatus;
    fn AudioQueueEnqueueBuffer(
        aq: AudioQueueRef,
        buf: *mut QueueBuffer,
        n: u32,
        descs: *const c_void,
    ) -> OSStatus;
    fn AudioQueueSetProperty(
        aq: AudioQueueRef,
        id: u32,
        data: *const c_void,
        size: u32,
    ) -> OSStatus;
    fn AudioQueueStart(aq: AudioQueueRef, at: *const c_void) -> OSStatus;
    fn AudioQueueStop(aq: AudioQueueRef, immediate: u8) -> OSStatus;
    fn AudioQueueDispose(aq: AudioQueueRef, immediate: u8) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringGetLength(s: CFStringRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(len: isize, enc: u32) -> isize;
    fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: isize, enc: u32) -> u8;
    fn CFStringCreateWithBytes(
        alloc: *const c_void,
        bytes: *const u8,
        len: isize,
        enc: u32,
        external: u8,
    ) -> CFStringRef;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVMediaTypeAudio: *const objc2::runtime::AnyObject;
}

// ---------------------------------------------------------------------------------------------
// Access: read, never asked for
// ---------------------------------------------------------------------------------------------

/// `AVAuthorizationStatus` as a number, read with the one call that never asks (`authorizationStatusForMediaType:`).
/// None when AVFoundation is not there to say.
fn authorization_status() -> Option<isize> {
    let cls = objc2::runtime::AnyClass::get(c"AVCaptureDevice")?;
    // SAFETY: a framework constant (an NSString), read once; null only if the framework did not load.
    let media = unsafe { AVMediaTypeAudio };
    if media.is_null() {
        return None;
    }
    // SAFETY: `+[AVCaptureDevice authorizationStatusForMediaType:]` takes an AVMediaType (NSString) and returns
    // an NSInteger; it reads the TCC state and shows nothing.
    let status: isize = unsafe { objc2::msg_send![cls, authorizationStatusForMediaType: media] };
    Some(status)
}

/// The access the system gives this app now (never asks).
pub fn access() -> MicAccess {
    access_of(authorization_status())
}

/// `AVAuthorizationStatus` to the access it means: 0 not determined, 1 restricted, 2 denied, 3 authorized.
pub fn access_of(status: Option<isize>) -> MicAccess {
    match status {
        Some(3) => MicAccess::Allowed,
        Some(0) => MicAccess::NotAsked,
        Some(1) => MicAccess::Restricted,
        Some(2) => MicAccess::Off,
        _ => MicAccess::Unknown,
    }
}

// ---------------------------------------------------------------------------------------------
// HAL properties
// ---------------------------------------------------------------------------------------------

fn addr(selector: u32, scope: u32) -> PropertyAddress {
    PropertyAddress {
        selector,
        scope,
        element: ELEMENT_MAIN,
    }
}

/// One plain value of `id`'s property.
fn get<T: Copy + Default>(id: AudioObjectID, a: PropertyAddress) -> Option<T> {
    let mut v = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: `v` is writable for `size` bytes; the HAL writes at most that much.
    let r = unsafe {
        AudioObjectGetPropertyData(id, &a, 0, std::ptr::null(), &mut size, (&raw mut v).cast())
    };
    (r == 0 && size as usize == std::mem::size_of::<T>()).then_some(v)
}

/// The size of `id`'s property, in bytes.
fn size_of_prop(id: AudioObjectID, a: PropertyAddress) -> Option<u32> {
    let mut size = 0u32;
    // SAFETY: one out-value.
    let r = unsafe { AudioObjectGetPropertyDataSize(id, &a, 0, std::ptr::null(), &mut size) };
    (r == 0).then_some(size)
}

/// A CFString property of `id`, as text (the HAL hands over a retained string, released here).
fn get_string(id: AudioObjectID, selector: u32) -> Option<String> {
    let s: usize = get(id, addr(selector, SCOPE_GLOBAL))?;
    let s = s as CFStringRef;
    if s.is_null() {
        return None;
    }
    let text = cf_to_string(s);
    // SAFETY: the HAL returned a +1 reference; it is released once.
    unsafe { CFRelease(s) };
    text
}

fn cf_to_string(s: CFStringRef) -> Option<String> {
    // SAFETY: `s` is a live CFString; the buffer is as long as the encoding can need, plus the NUL.
    unsafe {
        let len = CFStringGetLength(s);
        let cap = CFStringGetMaximumSizeForEncoding(len, UTF8).max(0) as usize + 1;
        let mut buf = vec![0u8; cap];
        if CFStringGetCString(s, buf.as_mut_ptr().cast(), cap as isize, UTF8) == 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        buf.truncate(end);
        String::from_utf8(buf).ok()
    }
}

/// A CFString made from `text` (+1, the caller releases it).
fn cf_string(text: &str) -> Option<CFStringRef> {
    // SAFETY: the bytes are valid UTF-8 of the stated length.
    let s = unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            text.as_ptr(),
            text.len() as isize,
            UTF8,
            0,
        )
    };
    (!s.is_null()).then_some(s)
}

fn has_input(id: AudioObjectID) -> bool {
    size_of_prop(id, addr(DEV_STREAMS, SCOPE_INPUT)).is_some_and(|n| n > 0)
}

fn all_devices() -> Vec<AudioObjectID> {
    let a = addr(HW_DEVICES, SCOPE_GLOBAL);
    let Some(size) = size_of_prop(SYSTEM_OBJECT, a) else {
        return Vec::new();
    };
    let mut ids = vec![0 as AudioObjectID; size as usize / std::mem::size_of::<AudioObjectID>()];
    let mut size = (ids.len() * std::mem::size_of::<AudioObjectID>()) as u32;
    // SAFETY: `ids` is writable for `size` bytes.
    let r = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            &a,
            0,
            std::ptr::null(),
            &mut size,
            ids.as_mut_ptr().cast(),
        )
    };
    if r != 0 {
        return Vec::new();
    }
    ids.truncate(size as usize / std::mem::size_of::<AudioObjectID>());
    ids
}

/// The input devices, as (UID, name). Opens none of them.
pub fn list_raw() -> Vec<(String, Option<String>)> {
    all_devices()
        .into_iter()
        .filter(|&d| has_input(d))
        .filter_map(|d| Some((get_string(d, DEV_UID)?, get_string(d, OBJ_NAME))))
        .collect()
}

/// The device a UID names now, if it is an input device.
fn device_of_uid(uid: &str) -> Option<AudioObjectID> {
    let s = cf_string(uid)?;
    let a = addr(HW_TRANSLATE_UID, SCOPE_GLOBAL);
    let mut dev: AudioObjectID = UNKNOWN_OBJECT;
    let mut size = std::mem::size_of::<AudioObjectID>() as u32;
    // SAFETY: the qualifier is one CFStringRef that outlives the call; `dev` is writable for `size` bytes.
    let r = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            &a,
            std::mem::size_of::<CFStringRef>() as u32,
            (&raw const s).cast(),
            &mut size,
            (&raw mut dev).cast(),
        )
    };
    // SAFETY: made above, released once.
    unsafe { CFRelease(s) };
    (r == 0 && dev != UNKNOWN_OBJECT && has_input(dev)).then_some(dev)
}

fn default_input() -> Option<AudioObjectID> {
    get::<AudioObjectID>(SYSTEM_OBJECT, addr(HW_DEFAULT_INPUT, SCOPE_GLOBAL))
        .filter(|&d| d != UNKNOWN_OBJECT)
}

fn alive(dev: AudioObjectID) -> bool {
    get::<u32>(dev, addr(DEV_ALIVE, SCOPE_GLOBAL)).is_some_and(|v| v != 0)
}

/// An OSStatus, as the open outcome it means.
pub fn classify(status: OSStatus) -> OpenError {
    match status {
        AQ_PERMISSIONS => OpenError::Denied,
        BAD_DEVICE | AQ_INVALID_DEVICE => OpenError::NotFound,
        _ => OpenError::Other,
    }
}

/// The queue's format: mono, 32-bit float, packed, at the device's rate.
fn mono_float(rate: f64) -> StreamFormat {
    StreamFormat {
        sample_rate: rate,
        format_id: FORMAT_LPCM,
        format_flags: FLAG_FLOAT | FLAG_PACKED,
        bytes_per_packet: 4,
        frames_per_packet: 1,
        bytes_per_frame: 4,
        channels_per_frame: 1,
        bits_per_channel: 32,
        reserved: 0,
    }
}

/// A device rate the shared `downsample` can take (a whole, positive number), else 48 kHz.
fn usable_rate(rate: Option<f64>) -> u32 {
    match rate {
        Some(r) if r.is_finite() && (8000.0..=768_000.0).contains(&r) => r.round() as u32,
        _ => 48_000,
    }
}

// ---------------------------------------------------------------------------------------------
// The queue
// ---------------------------------------------------------------------------------------------

/// What the queue's callback holds: where samples go, and whether to hand the buffer back.
struct Tap {
    tx: Mutex<Sender<Vec<f32>>>,
    running: AtomicBool,
}

extern "C" fn on_input(
    user: *mut c_void,
    aq: AudioQueueRef,
    buf: *mut QueueBuffer,
    _at: *const c_void,
    _packets: u32,
    _descs: *const c_void,
) {
    if user.is_null() || buf.is_null() {
        return;
    }
    // SAFETY: `user` is the `Tap` the capture thread keeps alive until the queue is disposed; `buf` is a live
    // buffer of this queue whose `data` holds `byte_size` bytes of packed f32.
    unsafe {
        let tap = &*(user as *const Tap);
        let b = &*buf;
        let n = b.byte_size as usize / std::mem::size_of::<f32>();
        if n > 0 && !b.data.is_null() {
            let s = std::slice::from_raw_parts(b.data as *const f32, n).to_vec();
            if let Ok(tx) = tap.tx.lock() {
                let _ = tx.send(s);
            }
        }
        if tap.running.load(Ordering::SeqCst) {
            AudioQueueEnqueueBuffer(aq, buf, 0, std::ptr::null());
        }
    }
}

/// A queue that is made, and disposed of on drop (which stops it first and waits for its callback).
struct Queue {
    aq: AudioQueueRef,
    tap: Box<Tap>,
}

impl Drop for Queue {
    fn drop(&mut self) {
        self.tap.running.store(false, Ordering::SeqCst);
        // SAFETY: a live queue of ours; stopping and disposing at once returns after its last callback, so the
        // `Tap` it points at is not used after this.
        unsafe {
            AudioQueueStop(self.aq, 1);
            AudioQueueDispose(self.aq, 1);
        }
    }
}

fn check(r: OSStatus) -> Result<(), OpenError> {
    if r == 0 {
        Ok(())
    } else {
        Err(classify(r))
    }
}

/// Makes and starts the queue on `dev` (named by `uid`), or says why not. Nothing starts once `stop` is set.
fn open_queue(
    uid: &str,
    rate: u32,
    stop: &AtomicBool,
) -> Result<(Queue, Receiver<Vec<f32>>), OpenError> {
    let (tx, rx) = channel();
    let tap = Box::new(Tap {
        tx: Mutex::new(tx),
        running: AtomicBool::new(true),
    });
    let fmt = mono_float(f64::from(rate));
    let mut aq: AudioQueueRef = std::ptr::null_mut();
    // SAFETY: the format and the out-pointer outlive the call; the user pointer is the boxed `Tap`, which the
    // `Queue` keeps until after dispose. No run loop: the queue calls back on its own thread.
    check(unsafe {
        AudioQueueNewInput(
            &fmt,
            on_input,
            (&raw const *tap) as *mut c_void,
            std::ptr::null(),
            std::ptr::null(),
            0,
            &mut aq,
        )
    })?;
    let q = Queue { aq, tap };
    let s = cf_string(uid).ok_or(OpenError::Other)?;
    // SAFETY: the property's value is one CFStringRef, read by the queue during the call.
    let r = unsafe {
        AudioQueueSetProperty(
            q.aq,
            AQ_CURRENT_DEVICE,
            (&raw const s).cast(),
            std::mem::size_of::<CFStringRef>() as u32,
        )
    };
    // SAFETY: made above, released once.
    unsafe { CFRelease(s) };
    check(r)?;
    let bytes = ((f64::from(rate) * BUFFER_MS / 1000.0).round() as u32).max(1) * 4;
    for _ in 0..BUFFERS {
        let mut b: *mut QueueBuffer = std::ptr::null_mut();
        // SAFETY: a live queue and one out-pointer; the buffer belongs to the queue from here on.
        check(unsafe { AudioQueueAllocateBuffer(q.aq, bytes, &mut b) })?;
        // SAFETY: a buffer of this queue, handed to it.
        check(unsafe { AudioQueueEnqueueBuffer(q.aq, b, 0, std::ptr::null()) })?;
    }
    // An open that outlived its caller's wait (timeout) is told to stop: it must not start the mic.
    if stop.load(Ordering::SeqCst) {
        return Err(OpenError::Other);
    }
    // SAFETY: a live, primed queue; null start time is "now".
    check(unsafe { AudioQueueStart(q.aq, std::ptr::null()) })?;
    Ok((q, rx))
}

pub struct Real {
    pub on_frame: SharedFrameFn,
    pub on_lost: Arc<Mutex<Option<LostFn>>>,
}

impl Endpoints for Real {
    fn open(&self, device: Option<&str>) -> Result<Box<dyn Capture>, OpenError> {
        open_checked(access(), device, |dev| self.start(dev))
    }
}

/// The access rule ahead of any device: off or restricted is a refusal at once and `start` is never called.
fn open_checked(
    access: MicAccess,
    device: Option<&str>,
    start: impl FnOnce(Option<&str>) -> Result<Box<dyn Capture>, OpenError>,
) -> Result<Box<dyn Capture>, OpenError> {
    match access {
        MicAccess::Off | MicAccess::Restricted => Err(OpenError::Denied),
        MicAccess::Allowed | MicAccess::NotAsked | MicAccess::Unknown => start(device),
    }
}

impl Real {
    fn start(&self, device: Option<&str>) -> Result<Box<dyn Capture>, OpenError> {
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
        let (tx, rx) = channel::<Result<(), OpenError>>();
        let (st, sm, cb) = (stop.clone(), samples.clone(), self.on_frame.clone());
        let lost = self.on_lost.clone();
        let dev = device.map(str::to_string);
        let join = std::thread::Builder::new()
            .name("flare-mic".into())
            .spawn(move || capture_thread(dev, &st, &sm, &cb, &lost, &tx))
            .map_err(|_| OpenError::Other)?;
        // sync-ok: worker-only call; the capture thread answers as soon as the queue is running or refused.
        match rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(())) => Ok(Box::new(Thread {
                stop,
                samples,
                join: Some(join),
            })),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                stop.store(true, Ordering::SeqCst);
                Err(OpenError::Other)
            }
        }
    }
}

fn capture_thread(
    device: Option<String>,
    stop: &AtomicBool,
    samples: &Mutex<Vec<f32>>,
    on_frame: &SharedFrameFn,
    on_lost: &Mutex<Option<LostFn>>,
    tx: &std::sync::mpsc::Sender<Result<(), OpenError>>,
) {
    // The device: the one named, or the default input as it is now.
    let dev = match device.as_deref() {
        Some(uid) => device_of_uid(uid),
        None => default_input(),
    };
    let Some(dev) = dev else {
        let _ = tx.send(Err(OpenError::NotFound));
        return;
    };
    let Some(uid) = device
        .clone()
        .or_else(|| get_string(dev, DEV_UID))
        .filter(|u| !u.is_empty())
    else {
        let _ = tx.send(Err(OpenError::NotFound));
        return;
    };
    let rate = usable_rate(get::<f64>(dev, addr(DEV_RATE, SCOPE_GLOBAL)));
    let (queue, audio) = match open_queue(&uid, rate, stop) {
        Ok(q) => q,
        Err(e) => {
            let _ = tx.send(Err(e));
            return;
        }
    };
    let _ = tx.send(Ok(()));
    let mut looked = Instant::now();
    pump(stop, samples, on_frame, on_lost, || {
        match audio.recv_timeout(Duration::from_millis(50)) {
            Ok(raw) => return Poll::Frames(downsample(&raw, rate, TARGET_RATE)),
            Err(RecvTimeoutError::Disconnected) => return Poll::Lost(CaptureFailure::Failed),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if looked.elapsed() >= ALIVE_EVERY {
            looked = Instant::now();
            if !alive(dev) {
                return Poll::Lost(CaptureFailure::NoDevice);
            }
        }
        Poll::Quiet
    });
    drop(queue);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_statuses_map_to_the_open_outcomes() {
        assert_eq!(classify(AQ_PERMISSIONS), OpenError::Denied);
        assert_eq!(classify(BAD_DEVICE), OpenError::NotFound);
        assert_eq!(classify(AQ_INVALID_DEVICE), OpenError::NotFound);
        assert_eq!(classify(-50), OpenError::Other);
        assert_eq!(BAD_DEVICE, 0x2164_6576);
    }

    #[test]
    fn the_c_layouts_are_the_frameworks_own() {
        assert_eq!(std::mem::size_of::<StreamFormat>(), 40);
        assert_eq!(std::mem::size_of::<PropertyAddress>(), 12);
        assert_eq!(std::mem::size_of::<QueueBuffer>(), 56);
        assert_eq!(std::mem::offset_of!(QueueBuffer, byte_size), 16);
        let f = mono_float(48_000.0);
        assert_eq!((f.channels_per_frame, f.bits_per_channel), (1, 32));
        assert_eq!(f.format_flags & FLAG_FLOAT, FLAG_FLOAT);
    }

    #[test]
    fn the_authorization_status_means_what_apple_says() {
        assert_eq!(access_of(Some(3)), MicAccess::Allowed);
        assert_eq!(access_of(Some(0)), MicAccess::NotAsked);
        assert_eq!(access_of(Some(1)), MicAccess::Restricted);
        assert_eq!(access_of(Some(2)), MicAccess::Off);
        assert_eq!(access_of(Some(9)), MicAccess::Unknown);
        assert_eq!(access_of(None), MicAccess::Unknown);
    }

    #[test]
    fn reading_the_access_asks_nothing_and_answers() {
        // The non-prompting read itself, on this machine: whatever it says, it is one of the five.
        let a = access();
        assert!(
            matches!(
                a,
                MicAccess::Allowed
                    | MicAccess::NotAsked
                    | MicAccess::Off
                    | MicAccess::Restricted
                    | MicAccess::Unknown
            ),
            "{a:?}"
        );
        assert_ne!(
            a,
            MicAccess::Unknown,
            "AVFoundation did not answer (the class or the media type was not found)"
        );
    }

    #[test]
    fn access_that_is_off_refuses_before_any_device_is_touched() {
        for a in [MicAccess::Off, MicAccess::Restricted] {
            let mut started = false;
            let r = open_checked(a, Some("uid"), |_| {
                started = true;
                Err(OpenError::Other)
            });
            assert_eq!(r.err(), Some(OpenError::Denied));
            assert!(!started, "{a:?} touched the device");
        }
        for a in [MicAccess::Allowed, MicAccess::NotAsked, MicAccess::Unknown] {
            let mut asked = None;
            let r = open_checked(a, Some("uid"), |d| {
                asked = d.map(str::to_string);
                Err(OpenError::NotFound)
            });
            assert_eq!(r.err(), Some(OpenError::NotFound));
            assert_eq!(asked.as_deref(), Some("uid"));
        }
    }

    #[test]
    fn a_device_rate_the_resampler_cannot_take_becomes_48_khz() {
        assert_eq!(usable_rate(Some(44_100.0)), 44_100);
        assert_eq!(usable_rate(Some(48_000.0)), 48_000);
        assert_eq!(usable_rate(Some(0.0)), 48_000);
        assert_eq!(usable_rate(Some(f64::NAN)), 48_000);
        assert_eq!(usable_rate(None), 48_000);
        // 50 ms of 48 or 44.1 kHz is exactly 800 samples at 16 kHz: no sample is lost between buffers.
        for r in [48_000u32, 44_100, 96_000] {
            let n = (f64::from(r) * BUFFER_MS / 1000.0).round() as usize;
            assert_eq!(downsample(&vec![0.0; n], r, TARGET_RATE).len(), 800, "{r}");
        }
    }

    /// By hand only (`docs/dictation-by-hand.md`, macOS check M3): lists the real input devices and records one
    /// second from the default one. The first run makes macOS ask for the microphone for the terminal it runs in.
    #[test]
    #[ignore = "opens the real microphone (a system prompt the first time); run by hand"]
    fn the_real_default_microphone_lists_and_records_a_second() {
        let list = crate::mic::list_mics();
        eprintln!("access: {:?}", access());
        for m in &list {
            eprintln!("input: {} ({})", m.name, m.id);
        }
        let heard = Arc::new(Mutex::new(0usize));
        let h = Arc::clone(&heard);
        let cap = crate::mic::start_capture(None, Box::new(move |f| *h.lock().unwrap() += f.len()))
            .map_err(CaptureFailure::text)
            .expect("the default microphone opened");
        std::thread::sleep(Duration::from_secs(1));
        let clip = cap.stop();
        eprintln!(
            "heard {} samples at 16 kHz, level {}",
            clip.len(),
            crate::wav::level(&clip)
        );
        assert!(clip.len() >= 12_000, "{} samples", clip.len());
        assert_eq!(*heard.lock().unwrap(), clip.len());
    }

    #[test]
    fn fourccs_are_big_endian_as_the_headers_write_them() {
        assert_eq!(HW_DEVICES, 0x6465_7623);
        assert_eq!(SCOPE_INPUT, 0x696e_7074);
        assert_eq!(FORMAT_LPCM, 0x6c70_636d);
    }
}
