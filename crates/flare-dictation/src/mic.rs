//! The microphone: WASAPI capture endpoints only (Task 16).
//!
//! Dictation's ears. Prism Terminal got its microphone from Chromium's `getUserMedia`; Flare opens a
//! WASAPI shared-mode CAPTURE endpoint on its own thread, event driven, and hands the caller 16 kHz
//! mono `f32` frames. Rules this file holds (and `privacy.rs` scans for):
//!
//! * Only capture endpoints (`eCapture`) are ever opened. The one mention of a render endpoint is
//!   reading the default speaker's id, to name it as the echo reference (D7); nothing is opened on it.
//! * Nothing is written to disk. The clip lives in memory until the caller takes it with `stop`.
//! * Opening Settings never opens the microphone; only a press or the Test button calls
//!   `start_capture`. Listing the devices (`list_mics`) opens none of them.
//! * A device id that no longer resolves retries ONCE with the default endpoint (owner, 2026-10-17:
//!   "it says now it cant find the mic, but mics connected and should be the system default"). A
//!   refusal is never retried, and neither is a failure when no device was chosen.
//!
//! `start_capture` waits for the capture thread to say whether the endpoint opened (bounded by
//! `OPEN_TIMEOUT`), so it belongs on a worker. The UI thread calls `start_capture_async` instead: it
//! returns at once and answers through a callback on its own worker (D1 runs the mic effect that way).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::wav::downsample;

/// The rate every frame is delivered at.
pub const TARGET_RATE: u32 = 16_000;
/// One frame is ten milliseconds of audio: 160 samples at `TARGET_RATE`.
pub const FRAME_MS: u64 = 10;
/// Samples in one fake frame.
pub const FRAME_SAMPLES: usize = (TARGET_RATE as u64 * FRAME_MS / 1000) as usize;
/// How long `start_capture` waits for the capture thread to report whether the endpoint opened.
const OPEN_TIMEOUT: Duration = Duration::from_secs(8);

/// Called with each frame of 16 kHz mono samples as it arrives (on the capture thread).
pub type FrameFn = Box<dyn FnMut(&[f32]) + Send>;
type SharedFrameFn = Arc<Mutex<FrameFn>>;

/// One capture endpoint, for the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicInfo {
    pub id: String,
    pub name: String,
}

/// Why a microphone could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFailure {
    NoDevice,
    Denied,
    Failed,
    Unsupported,
}

impl CaptureFailure {
    /// The words the pill shows (Prism Terminal's, `dictation.ts`).
    pub fn text(self) -> &'static str {
        match self {
            CaptureFailure::NoDevice => "No microphone found",
            CaptureFailure::Denied => "Microphone access was refused",
            CaptureFailure::Unsupported | CaptureFailure::Failed => {
                "The microphone could not be opened"
            }
        }
    }
}

/// A running capture. `stop` ends it and returns everything heard, at 16 kHz mono.
pub trait Capture: Send {
    fn stop(self: Box<Self>) -> Vec<f32>;
}

/// Where the audio comes from. `File` is the e2e seam: `FLARE_E2E_MIC` names a WAV that plays as the
/// microphone, in real time. The seam is read in `crates/flare` and passed in here **[seam]**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicSource {
    Device,
    File(PathBuf),
}

/// What one attempt to open an endpoint can say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// The chosen device (or, with none chosen, any device) is not there.
    NotFound,
    Denied,
    Other,
}

/// The endpoint layer, so the retry rule is testable without a sound card.
pub trait Endpoints {
    fn open(&self, device: Option<&str>) -> Result<Box<dyn Capture>, OpenError>;
}

/// Opens `device`; when it no longer resolves, opens the default endpoint ONCE (N4). A refusal is final,
/// and so is a failure when no device was asked for.
pub fn open_with_fallback(
    ep: &dyn Endpoints,
    device: Option<&str>,
) -> Result<Box<dyn Capture>, CaptureFailure> {
    match ep.open(device) {
        Ok(c) => Ok(c),
        Err(OpenError::NotFound) if device.is_some() => ep.open(None).map_err(failure_of),
        Err(e) => Err(failure_of(e)),
    }
}

fn failure_of(e: OpenError) -> CaptureFailure {
    match e {
        OpenError::NotFound => CaptureFailure::NoDevice,
        OpenError::Denied => CaptureFailure::Denied,
        OpenError::Other => CaptureFailure::Failed,
    }
}

/// Raw endpoint rows in, picker rows out: no `default` or `communications` aliases, and an unnamed one
/// is "Microphone N" by its place in the list.
pub fn build_list(raw: Vec<(String, Option<String>)>) -> Vec<MicInfo> {
    raw.into_iter()
        .enumerate()
        .filter(|(_, (id, _))| id != "default" && id != "communications")
        .map(|(i, (id, name))| {
            let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            MicInfo {
                id,
                name: name.unwrap_or_else(|| format!("Microphone {}", i + 1)),
            }
        })
        .collect()
}

/// The active capture endpoints. Empty on any failure. Opens none of them.
pub fn list_mics() -> Vec<MicInfo> {
    // Own thread, own COM apartment: the caller's may be anything.
    let Ok(h) = std::thread::Builder::new()
        .name("flare-mic-list".into())
        .spawn(wasapi::list_raw)
    else {
        return Vec::new();
    };
    // sync-ok: a worker-only call (the picker fills on its own thread); bounded by the endpoint enumeration.
    build_list(h.join().unwrap_or_default())
}

/// Starts the real microphone (`device` is an endpoint id from `list_mics`, or `None` for the default).
pub fn start_capture(
    device: Option<&str>,
    on_frame: FrameFn,
) -> Result<Box<dyn Capture>, CaptureFailure> {
    start_capture_from(&MicSource::Device, device, on_frame)
}

thread_local! {
    /// Microphone opens asked for from this thread (a test reads it to prove a page never opens one).
    static OPENS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn count_open() {
    OPENS.with(|o| o.set(o.get() + 1));
}

/// How many microphone opens (`start_capture_async`, `start_capture_from`, `start_capture`) this thread has asked for.
pub fn opens_on_this_thread() -> usize {
    OPENS.with(std::cell::Cell::get)
}

/// The answer to `start_capture_async`.
pub type DoneFn = Box<dyn FnOnce(Result<Box<dyn Capture>, CaptureFailure>) + Send>;

/// Opens the microphone on a worker and answers through `done`, so the caller (the UI thread) never waits
/// on a slow or hung endpoint or on the fake's file read. `done` runs on the worker.
pub fn start_capture_async(
    source: MicSource,
    device: Option<String>,
    on_frame: FrameFn,
    done: DoneFn,
) {
    count_open();
    let spawned = std::thread::Builder::new()
        .name("flare-mic-open".into())
        .spawn(move || done(start_capture_from(&source, device.as_deref(), on_frame)));
    // A thread that cannot even be created is dropped with its callback; nothing else can answer.
    drop(spawned);
}

/// `start_capture` with the source named; `MicSource::File` never touches WASAPI.
pub fn start_capture_from(
    source: &MicSource,
    device: Option<&str>,
    on_frame: FrameFn,
) -> Result<Box<dyn Capture>, CaptureFailure> {
    count_open();
    match source {
        MicSource::File(path) => start_file(path, on_frame),
        MicSource::Device => {
            let shared = Arc::new(Mutex::new(on_frame));
            let ep = wasapi::Real { on_frame: shared };
            open_with_fallback(&ep, device)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// A capture thread's shared state.

/// The clip so far, and the stop flag, shared between a capture thread and its handle.
struct Thread {
    stop: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<f32>>>,
    join: Option<JoinHandle<()>>,
}

impl Capture for Thread {
    fn stop(mut self: Box<Self>) -> Vec<f32> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        let mut g = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *g)
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The fake: a WAV played as the microphone.

/// A decoded WAV: channel 0 only, at its own rate.
pub fn read_wav(bytes: &[u8]) -> Option<(Vec<f32>, u32)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let u16_at = |i: usize| {
        bytes
            .get(i..i + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let u32_at = |i: usize| {
        bytes
            .get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (mut tag, mut channels, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(at + 4)? as usize;
        let body = at + 8;
        if id == b"fmt " {
            tag = u16_at(body)?;
            channels = u16_at(body + 2)?;
            rate = u32_at(body + 4)?;
            bits = u16_at(body + 14)?;
        } else if id == b"data" {
            let end = (body + size).min(bytes.len());
            let data = &bytes[body..end];
            if channels == 0 || rate == 0 {
                return None;
            }
            let step = (bits as usize / 8) * channels as usize;
            if step == 0 {
                return None;
            }
            let mut out = Vec::with_capacity(data.len() / step);
            for frame in data.chunks_exact(step) {
                let s = match (tag, bits) {
                    (1, 16) => i16::from_le_bytes([frame[0], frame[1]]) as f32 / 32768.0,
                    (3, 32) => f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]),
                    _ => return None,
                };
                out.push(s);
            }
            return Some((out, rate));
        }
        at = body + size + (size & 1);
    }
    None
}

fn start_file(path: &Path, mut on_frame: FrameFn) -> Result<Box<dyn Capture>, CaptureFailure> {
    let bytes = std::fs::read(path).map_err(|_| CaptureFailure::NoDevice)?;
    let (pcm, rate) = read_wav(&bytes).ok_or(CaptureFailure::Failed)?;
    let clip = downsample(&pcm, rate, TARGET_RATE);
    let stop = Arc::new(AtomicBool::new(false));
    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let (st, sm) = (stop.clone(), samples.clone());
    let join = std::thread::Builder::new()
        .name("flare-mic-fake".into())
        .spawn(move || {
            let start = Instant::now();
            let mut frame_no: u64 = 0;
            while !st.load(Ordering::SeqCst) {
                // Frame k is due at (k + 1) frames after the start, never earlier: real time.
                let due = start + Duration::from_millis(FRAME_MS * (frame_no + 1));
                loop {
                    if st.load(Ordering::SeqCst) {
                        return;
                    }
                    let now = Instant::now();
                    if now >= due {
                        break;
                    }
                    std::thread::sleep((due - now).min(Duration::from_millis(5)));
                }
                let from = (frame_no as usize).saturating_mul(FRAME_SAMPLES);
                let mut frame = [0.0f32; FRAME_SAMPLES];
                if from < clip.len() {
                    let end = (from + FRAME_SAMPLES).min(clip.len());
                    frame[..end - from].copy_from_slice(&clip[from..end]);
                }
                sm.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(&frame);
                on_frame(&frame);
                frame_no += 1;
            }
        })
        .map_err(|_| CaptureFailure::Failed)?;
    Ok(Box::new(Thread {
        stop,
        samples,
        join: Some(join),
    }))
}

// ---------------------------------------------------------------------------------------------
// The real thing.

mod wasapi {
    use super::*;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, eRender, AudioCategory_Communications, AudioClientProperties,
        IAcousticEchoCancellationControl, IAudioCaptureClient, IAudioClient, IAudioClient2,
        IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, DEVICE_STATE_ACTIVE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    };
    use windows::Win32::System::Com::StructuredStorage::{
        PropVariantClear, PropVariantToStringAlloc,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
    };

    const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    const AUDCLNT_E_DEVICE_INVALIDATED: i32 = 0x8889_0004_u32 as i32;
    const AUDCLNT_E_DEVICE_IN_USE: i32 = 0x8889_000A_u32 as i32;
    const E_NOTFOUND: i32 = 0x8007_0490_u32 as i32;
    const AUDCLNT_E_ENDPOINT_CREATE_FAILED: i32 = 0x8889_0008_u32 as i32;

    /// An HRESULT, as the open outcome it means.
    pub fn classify(hr: i32) -> OpenError {
        match hr {
            E_ACCESSDENIED => OpenError::Denied,
            E_NOTFOUND | AUDCLNT_E_DEVICE_INVALIDATED | AUDCLNT_E_ENDPOINT_CREATE_FAILED => {
                OpenError::NotFound
            }
            _ => OpenError::Other,
        }
    }
    #[allow(dead_code)]
    const _KEEP: i32 = AUDCLNT_E_DEVICE_IN_USE;

    fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
        // SAFETY: plain COM activation of the system's device enumerator on a COM-initialised thread.
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
    }

    fn init_com() {
        // SAFETY: CoInitializeEx takes no pointers we own; a changed-mode answer is fine, COM still works.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }

    pub fn list_raw() -> Vec<(String, Option<String>)> {
        init_com();
        let run = || -> windows::core::Result<Vec<(String, Option<String>)>> {
            let en = enumerator()?;
            // SAFETY: enumeration of active CAPTURE endpoints; every call returns owned COM objects.
            unsafe {
                let coll = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
                let n = coll.GetCount()?;
                let mut out = Vec::new();
                for i in 0..n {
                    let Ok(dev) = coll.Item(i) else { continue };
                    let Ok(id) = dev.GetId() else { continue };
                    let id_s = id.to_string().unwrap_or_default();
                    windows::Win32::System::Com::CoTaskMemFree(Some(id.0 as *const _));
                    if id_s.is_empty() {
                        continue;
                    }
                    let name = dev
                        .OpenPropertyStore(STGM_READ)
                        .and_then(|ps| ps.GetValue(&PKEY_Device_FriendlyName))
                        .ok()
                        .and_then(|mut pv| {
                            let s = PropVariantToStringAlloc(&pv).ok().map(|p| {
                                let s = p.to_string().unwrap_or_default();
                                windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
                                s
                            });
                            let _ = PropVariantClear(&mut pv);
                            s
                        });
                    out.push((id_s, name));
                }
                Ok(out)
            }
        };
        run().unwrap_or_default()
    }

    pub struct Real {
        pub on_frame: SharedFrameFn,
    }

    impl Endpoints for Real {
        fn open(&self, device: Option<&str>) -> Result<Box<dyn Capture>, OpenError> {
            let stop = Arc::new(AtomicBool::new(false));
            let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
            let (tx, rx) = channel::<Result<(), OpenError>>();
            let (st, sm, cb) = (stop.clone(), samples.clone(), self.on_frame.clone());
            let dev = device.map(str::to_string);
            let join = std::thread::Builder::new()
                .name("flare-mic".into())
                .spawn(move || capture_thread(dev, st, sm, cb, tx))
                .map_err(|_| OpenError::Other)?;
            // sync-ok: worker-only call; the capture thread answers as soon as the endpoint is open or refused.
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

    fn hr_of(e: &windows::core::Error) -> OpenError {
        classify(e.code().0)
    }

    struct Opened {
        client: IAudioClient,
        capture: IAudioCaptureClient,
        event: HANDLE,
        rate: u32,
        channels: usize,
        kind: Sample,
        block_align: usize,
    }

    #[derive(Clone, Copy)]
    enum Sample {
        F32,
        I16,
        I24,
        I32,
    }

    /// The default speaker's id, only to NAME it as the echo reference (D7). Nothing is opened on it. It is the
    /// console default render device, the one the start cue (`PlaySoundW`) actually plays on.
    fn echo_reference_id(en: &IMMDeviceEnumerator) -> Option<Vec<u16>> {
        // SAFETY: reads an endpoint id string and frees it.
        unsafe {
            let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let id = dev.GetId().ok()?;
            let mut v: Vec<u16> = id.as_wide().to_vec();
            v.push(0);
            windows::Win32::System::Com::CoTaskMemFree(Some(id.0 as *const _));
            Some(v)
        }
    }

    fn open_endpoint(device: Option<&str>, stop: &AtomicBool) -> Result<Opened, OpenError> {
        let en = enumerator().map_err(|e| hr_of(&e))?;
        // SAFETY: standard shared-mode WASAPI capture set-up; every pointer comes from the API itself and
        // the mix format is freed after use.
        unsafe {
            let dev: IMMDevice = match device {
                Some(id) => {
                    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
                    en.GetDevice(PCWSTR(wide.as_ptr()))
                        .map_err(|_| OpenError::NotFound)?
                }
                None => en
                    .GetDefaultAudioEndpoint(eCapture, eConsole)
                    .map_err(|_| OpenError::NotFound)?,
            };
            let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(|e| hr_of(&e))?;
            // D7: the communications category, so Windows applies the device's communications processing.
            if let Ok(c2) = client.cast::<IAudioClient2>() {
                let props = AudioClientProperties {
                    cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
                    eCategory: AudioCategory_Communications,
                    ..Default::default()
                };
                let _ = c2.SetClientProperties(&props);
            }
            let fmt = client.GetMixFormat().map_err(|e| hr_of(&e))?;
            let f: WAVEFORMATEX = std::ptr::read_unaligned(fmt);
            let kind = sample_kind(fmt);
            let init = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                1_000_000,
                0,
                fmt,
                None,
            );
            windows::Win32::System::Com::CoTaskMemFree(Some(fmt as *const _));
            init.map_err(|e| hr_of(&e))?;
            let kind = kind.ok_or(OpenError::Other)?;
            let event = windows_sys::Win32::System::Threading::CreateEventW(
                std::ptr::null(),
                0,
                0,
                std::ptr::null(),
            );
            if event.is_null() {
                return Err(OpenError::Other);
            }
            let event = HANDLE(event);
            client.SetEventHandle(event).map_err(|e| {
                windows_sys::Win32::Foundation::CloseHandle(event.0);
                hr_of(&e)
            })?;
            let capture: IAudioCaptureClient = client.GetService().map_err(|e| {
                windows_sys::Win32::Foundation::CloseHandle(event.0);
                hr_of(&e)
            })?;
            // Windows 11 22H2+: name the default speaker as the echo reference when the endpoint offers it.
            if let (Ok(aec), Some(id)) = (
                client.GetService::<IAcousticEchoCancellationControl>(),
                echo_reference_id(&en),
            ) {
                let _ = aec.SetEchoCancellationRenderEndpoint(PCWSTR(id.as_ptr()));
            }
            // An open that outlived its caller's wait (timeout) is told to stop: it must not start the mic.
            if stop.load(Ordering::SeqCst) {
                windows_sys::Win32::Foundation::CloseHandle(event.0);
                return Err(OpenError::Other);
            }
            client.Start().map_err(|e| {
                windows_sys::Win32::Foundation::CloseHandle(event.0);
                hr_of(&e)
            })?;
            Ok(Opened {
                client,
                capture,
                event,
                rate: f.nSamplesPerSec,
                channels: (f.nChannels as usize).max(1),
                kind,
                block_align: (f.nBlockAlign as usize).max(1),
            })
        }
    }

    /// What the mix format's samples are, or `None` for a layout this does not read.
    ///
    /// # Safety
    /// `fmt` must point to a valid `WAVEFORMATEX` (with its extension when the tag says so).
    unsafe fn sample_kind(fmt: *const WAVEFORMATEX) -> Option<Sample> {
        let f: WAVEFORMATEX = std::ptr::read_unaligned(fmt);
        let tag = f.wFormatTag;
        let float = match tag {
            3 => true,
            1 => false,
            0xFFFE => {
                let ext: WAVEFORMATEXTENSIBLE =
                    std::ptr::read_unaligned(fmt as *const WAVEFORMATEXTENSIBLE);
                ext.SubFormat.data1 == 3
            }
            _ => return None,
        };
        match (float, f.wBitsPerSample) {
            (true, 32) => Some(Sample::F32),
            (false, 16) => Some(Sample::I16),
            (false, 24) => Some(Sample::I24),
            (false, 32) => Some(Sample::I32),
            _ => None,
        }
    }

    fn plane0(data: &[u8], frames: usize, o: &Opened) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames);
        for i in 0..frames {
            let at = i * o.block_align;
            let Some(b) = data.get(at..at + o.block_align) else {
                break;
            };
            let s = match o.kind {
                Sample::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                Sample::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
                Sample::I24 => {
                    (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0
                }
                Sample::I32 => {
                    i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0
                }
            };
            out.push(s);
        }
        let _ = o.channels;
        out
    }

    fn capture_thread(
        device: Option<String>,
        stop: Arc<AtomicBool>,
        samples: Arc<Mutex<Vec<f32>>>,
        on_frame: SharedFrameFn,
        tx: std::sync::mpsc::Sender<Result<(), OpenError>>,
    ) {
        init_com();
        if stop.load(Ordering::SeqCst) {
            let _ = tx.send(Err(OpenError::Other));
            return;
        }
        let opened = match open_endpoint(device.as_deref(), &stop) {
            Ok(o) => o,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        let _ = tx.send(Ok(()));
        // SAFETY: the event and clients are owned by this thread for the whole loop and released after it.
        unsafe {
            while !stop.load(Ordering::SeqCst) {
                // A short wait, so a stop is seen quickly even when the device goes quiet.
                windows_sys::Win32::System::Threading::WaitForSingleObject(opened.event.0, 50);
                loop {
                    let Ok(next) = opened.capture.GetNextPacketSize() else {
                        break;
                    };
                    if next == 0 {
                        break;
                    }
                    let mut data: *mut u8 = std::ptr::null_mut();
                    let mut frames = 0u32;
                    let mut flags = 0u32;
                    if opened
                        .capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                        .is_err()
                    {
                        break;
                    }
                    let n = frames as usize;
                    let raw = if flags & 0x2 != 0 || data.is_null() {
                        vec![0.0f32; n] // AUDCLNT_BUFFERFLAGS_SILENT
                    } else {
                        plane0(
                            std::slice::from_raw_parts(data, n * opened.block_align),
                            n,
                            &opened,
                        )
                    };
                    let _ = opened.capture.ReleaseBuffer(frames);
                    let out = downsample(&raw, opened.rate, TARGET_RATE);
                    if out.is_empty() {
                        continue;
                    }
                    samples
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .extend_from_slice(&out);
                    if let Ok(mut f) = on_frame.lock() {
                        f(&out);
                    }
                }
            }
            let _ = opened.client.Stop();
            windows_sys::Win32::Foundation::CloseHandle(opened.event.0);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn hresults_map_to_the_open_outcomes() {
            assert_eq!(classify(E_ACCESSDENIED), OpenError::Denied);
            assert_eq!(classify(E_NOTFOUND), OpenError::NotFound);
            assert_eq!(classify(AUDCLNT_E_DEVICE_INVALIDATED), OpenError::NotFound);
            assert_eq!(classify(0x8000_4005_u32 as i32), OpenError::Other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::encode_wav;
    use std::cell::RefCell;

    struct Stub;
    impl Capture for Stub {
        fn stop(self: Box<Self>) -> Vec<f32> {
            vec![0.5]
        }
    }

    /// Answers each `open` from a script and records the device it was asked for.
    struct Script {
        answers: RefCell<Vec<Result<(), OpenError>>>,
        asked: RefCell<Vec<Option<String>>>,
    }
    impl Script {
        fn new(answers: Vec<Result<(), OpenError>>) -> Self {
            Script {
                answers: RefCell::new(answers),
                asked: RefCell::new(Vec::new()),
            }
        }
    }
    impl Endpoints for Script {
        fn open(&self, device: Option<&str>) -> Result<Box<dyn Capture>, OpenError> {
            self.asked.borrow_mut().push(device.map(str::to_string));
            self.answers
                .borrow_mut()
                .remove(0)
                .map(|_| Box::new(Stub) as Box<dyn Capture>)
        }
    }

    #[test]
    fn falls_back_to_the_default_mic_when_the_saved_one_is_gone() {
        let ep = Script::new(vec![Err(OpenError::NotFound), Err(OpenError::NotFound)]);
        let r = open_with_fallback(&ep, Some("stale"));
        assert_eq!(r.err(), Some(CaptureFailure::NoDevice));
        assert_eq!(*ep.asked.borrow(), vec![Some("stale".to_string()), None]);

        let ep = Script::new(vec![Err(OpenError::NotFound), Ok(())]);
        let r = open_with_fallback(&ep, Some("stale"));
        assert_eq!(r.ok().map(|c| c.stop()), Some(vec![0.5]));
        assert_eq!(ep.asked.borrow().len(), 2);
    }

    #[test]
    fn does_not_retry_a_refusal_nor_when_no_mic_was_chosen() {
        let ep = Script::new(vec![Err(OpenError::Denied)]);
        assert_eq!(
            open_with_fallback(&ep, Some("dji")).err(),
            Some(CaptureFailure::Denied)
        );
        assert_eq!(ep.asked.borrow().len(), 1);
        let ep = Script::new(vec![Err(OpenError::NotFound)]);
        assert_eq!(
            open_with_fallback(&ep, None).err(),
            Some(CaptureFailure::NoDevice)
        );
        assert_eq!(ep.asked.borrow().len(), 1);
        let ep = Script::new(vec![Err(OpenError::Other)]);
        assert_eq!(
            open_with_fallback(&ep, Some("x")).err(),
            Some(CaptureFailure::Failed)
        );
        assert_eq!(ep.asked.borrow().len(), 1);
    }

    #[test]
    fn failure_texts_are_pts() {
        assert_eq!(CaptureFailure::NoDevice.text(), "No microphone found");
        assert_eq!(
            CaptureFailure::Denied.text(),
            "Microphone access was refused"
        );
        assert_eq!(
            CaptureFailure::Unsupported.text(),
            "The microphone could not be opened"
        );
        assert_eq!(
            CaptureFailure::Failed.text(),
            "The microphone could not be opened"
        );
    }

    #[test]
    fn the_list_drops_aliases_and_names_the_unnamed() {
        let raw = vec![
            ("default".to_string(), Some("Default".to_string())),
            ("{a}".to_string(), Some("DJI Mic".to_string())),
            ("{b}".to_string(), None),
            ("{c}".to_string(), Some("  ".to_string())),
            ("communications".to_string(), None),
        ];
        let list = build_list(raw);
        let names: Vec<_> = list.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["DJI Mic", "Microphone 3", "Microphone 4"]);
        assert_eq!(list[0].id, "{a}");
    }

    #[test]
    fn the_fake_plays_in_real_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mic.wav");
        // Half a second of a ramp, so a frame can be told from its neighbours.
        let clip: Vec<f32> = (0..8000).map(|i| (i as f32 / 8000.0) * 0.5).collect();
        std::fs::write(&path, encode_wav(&clip, 16_000)).unwrap();

        let (tx, rx) = channel::<usize>();
        let started = Instant::now();
        let cap = start_capture_from(
            &MicSource::File(path),
            None,
            Box::new(move |f| {
                let _ = tx.send(f.len());
            }),
        )
        .map_err(|e| e.text())
        .unwrap();
        for _ in 0..20 {
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(10)).unwrap(),
                FRAME_SAMPLES
            );
        }
        // Twenty 10 ms frames cannot arrive faster than the audio plays.
        assert!(
            started.elapsed() >= Duration::from_millis(190),
            "faster than real time: {:?}",
            started.elapsed()
        );
        let heard = cap.stop();
        assert!(heard.len() >= 20 * FRAME_SAMPLES && heard.len().is_multiple_of(FRAME_SAMPLES));
        for (a, b) in heard.iter().zip(clip.iter()).take(160) {
            assert!((a - b).abs() < 1.0 / 16_000.0, "{a} vs {b}");
        }
    }

    #[test]
    fn the_fake_goes_quiet_after_the_file_and_stops_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.wav");
        std::fs::write(&path, encode_wav(&[0.25; 320], 16_000)).unwrap();
        let (tx, rx) = channel::<f32>();
        let cap = start_capture_from(
            &MicSource::File(path),
            None,
            Box::new(move |f| {
                let _ = tx.send(f.iter().fold(0.0f32, |m, s| m.max(s.abs())));
            }),
        )
        .map_err(|e| e.text())
        .unwrap();
        let peaks: Vec<f32> = (0..4)
            .map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect();
        assert!(peaks[0] > 0.2 && peaks[1] > 0.2, "{peaks:?}");
        assert_eq!(peaks[3], 0.0, "{peaks:?}");
        drop(cap.stop());
    }

    #[test]
    fn a_missing_or_broken_file_is_a_failure_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let none = start_capture_from(
            &MicSource::File(dir.path().join("nope.wav")),
            None,
            Box::new(|_| {}),
        );
        assert_eq!(none.err(), Some(CaptureFailure::NoDevice));
        let bad = dir.path().join("bad.wav");
        std::fs::write(&bad, b"not a wav at all").unwrap();
        assert_eq!(
            start_capture_from(&MicSource::File(bad), None, Box::new(|_| {})).err(),
            Some(CaptureFailure::Failed)
        );
    }

    #[test]
    fn read_wav_takes_channel_zero_and_both_sample_kinds() {
        // Stereo PCM16: left 0.5, right -0.5.
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        for x in [1u16, 2] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&32000u32.to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&8u32.to_le_bytes());
        for s in [16384i16, -16384, 16384, -16384] {
            v.extend_from_slice(&s.to_le_bytes());
        }
        let (pcm, rate) = read_wav(&v).unwrap();
        assert_eq!(rate, 8000);
        assert_eq!(pcm, vec![0.5, 0.5]);
    }

    #[test]
    fn the_async_open_returns_at_once_and_answers_by_callback() {
        // The callback waits for a gate the caller opens only AFTER `start_capture_async` has returned:
        // a call that opened the endpoint inline would run the callback first and never see the gate.
        let dir = tempfile::tempdir().unwrap();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<(bool, Option<CaptureFailure>)>();
        start_capture_async(
            MicSource::File(dir.path().join("nope.wav")),
            None,
            Box::new(|_| {}),
            Box::new(move |r| {
                let gated = gate_rx.recv_timeout(Duration::from_secs(5)).is_ok();
                let _ = out_tx.send((gated, r.err()));
            }),
        );
        gate_tx.send(()).unwrap();
        let (gated, err) = out_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(gated, "the call blocked until the open finished");
        assert_eq!(err, Some(CaptureFailure::NoDevice));
    }

    #[test]
    fn the_async_open_hands_over_a_running_capture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.wav");
        std::fs::write(&path, encode_wav(&[0.25; 320], 16_000)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        start_capture_async(
            MicSource::File(path),
            None,
            Box::new(|_| {}),
            Box::new(move |r| {
                let _ = tx.send(r.is_ok());
            }),
        );
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    }
}
