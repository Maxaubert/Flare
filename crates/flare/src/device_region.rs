//! Android frames (Android plan PR A4, rules 1363 and 1364): the emulator writes each frame into a region file Flare
//! owns (`flare_device::emu::region`), and its record (`RegionFrame`) reaches the frame thread here straight from the
//! emu host's reader. The frame thread copies the region into the next free slot of a ring of [`RING_SLOTS`] textures
//! on the GPU at once, off the UI thread, and the render notifier shows the newest finished slot in `BeforeRendering`
//! (`frame_image`), as the iOS ring does. The region is single-buffered: the copy is submitted well before the
//! emulator writes the next frame (spec "Frames", finding 3). No frame is queued: a record that comes while an older
//! one waits replaces it (counted as dropped).
//!
//! The copy (Step 0 S3, 2026-11-22, `probes/s3-import`):
//! - **Zero copy**: the mapped region itself becomes a GPU buffer. Windows: `ID3D12Device3::OpenExistingHeapFromAddress`
//!   on the mapped view (the `FromFileMapping` form refuses a file-backed mapping), a buffer placed on that heap, then
//!   `buffer_from_raw` + `create_buffer_from_hal`. macOS: `newBufferWithBytesNoCopy` on the mapping (unified memory),
//!   wrapped the same way. A small compute pass then copies it into the ring texture: rows of 4320 bytes break the
//!   256-byte row rule of `copy_buffer_to_texture`.
//! - **Upload** (`write_texture` from the mapped bytes, one CPU copy): when the import or the compute pass is refused on
//!   this machine (another backend, the device's limits, a validation error). Logged once.
//! - **CPU** (the slow mark): with no wgpu device (the software renderer), or when Slint refuses the ring texture, the
//!   region is copied into a `SharedPixelBuffer` at most [`CPU_FPS`](crate::device_frames::CPU_FPS) a second.
//!
//! Slot ownership: a slot is written only while it is free (or holds a ready frame nobody took yet); the slot on show
//! is handed back only from `Queue::on_submitted_work_done`, registered in `AfterRendering` (`after_render`). The
//! emulator's rows are top-down (spike S1): no Y flip. A turned device's frames come turned (a landscape frame is
//! 2400x1080), so Slint shows them as they are.
//!
//! The mapping and the import belong to the frame thread: a detached device's region is let go there (buffer, heap,
//! then the view), so the emu host's client can delete the file (Windows refuses to delete a mapped file).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use flare_device::emu_client::{OnRegion, RegionFrame};
use flare_device::target::Target;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::device_frames::{CPU_SPACING_MS, FramePath};

/// Textures in each device's ring: one on show, one the GPU may still sample, one to write.
pub const RING_SLOTS: usize = 3;

/// The compute pass's workgroup is this many pixels square.
const WG: u32 = 16;

/// Copies the region (RGBA bytes, rows of the texture's width, top-down) into the ring texture.
const SHADER: &str = r"
@group(0) @binding(0) var<storage, read> src: array<u32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if (id.x >= size.x || id.y >= size.y) { return; }
    textureStore(dst, vec2<i32>(i32(id.x), i32(id.y)), unpack4x8unorm(src[id.y * size.x + id.x]));
}
";

/// Called on the frame thread after a slot is ready: the window draws it (one coalesced wake).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// How a ring texture becomes a Slint image: `Image::try_from` (tests keep the texture to read it back).
pub type Import = Box<dyn Fn(wgpu::Texture) -> Result<Image, String>>;

fn slint_import() -> Import {
    Box::new(|t| Image::try_from(t).map_err(|e| format!("{e:?}")))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Clone)]
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// A new one for each wgpu device the renderer hands over: what was made on an older one is let go.
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Free,
    /// The frame thread records and submits its copy.
    Writing,
    /// Submitted: the frame copied as number N on this frame thread (the queue runs the copy before any later render
    /// samples it). Not the emulator's `seq`: that starts again with each new stream (a resume, an emu host restart),
    /// so a frame left from before would look newer than the frames after it.
    Ready(u64),
    /// On show.
    Shown,
    /// Replaced on show: free once the frame that sampled it is done on the GPU.
    Retiring,
}

/// A device's ring on the current wgpu device.
struct Ring {
    /// A new id for each ring made, so a late work-done callback never frees a slot of a newer ring.
    id: u64,
    gpu: u64,
    w: u32,
    h: u32,
    textures: Vec<wgpu::Texture>,
    slots: Vec<Slot>,
}

/// What the UI thread and the frame thread share.
#[derive(Default)]
struct Shared {
    gpu: Option<Gpu>,
    gpu_made: u64,
    rings: HashMap<Target, Ring>,
    rings_made: u64,
    /// The CPU path's newest picture per device.
    cpu: HashMap<Target, (u64, SharedPixelBuffer<Rgba8Pixel>)>,
    /// Slint refused a ring texture: the CPU path for the session.
    cpu_only: bool,
    /// The route of the newest copy.
    route: FramePath,
    /// Why the route is not zero copy, once.
    refused: Option<String>,
    /// A slot became ready since the window last asked.
    new_frame: bool,
    copied: u64,
    dropped: u64,
    /// The frame thread's CPU time per copy (record to submit), microseconds, the newest 600.
    copy_us: Vec<u32>,
    /// Tests: the zero-copy route is refused, as on a machine whose import fails.
    no_import: bool,
}

impl Shared {
    fn refuse(&mut self, why: String) {
        if self.refused.is_none() {
            log::warn!("device panel: Android frames are uploaded by the CPU: {why}");
            self.refused = Some(why);
        }
    }
}

enum Msg {
    Frame(RegionFrame),
    Release(Target),
    /// The renderer's wgpu device changed (or went).
    Gpu,
    /// Tests: answered once every message before it is handled.
    #[cfg(test)]
    Sync(Sender<()>),
}

/// The Android frames: the UI thread's half (the frame thread's runs from [`RegionFrames::sink`]).
pub struct RegionFrames {
    shared: Arc<Mutex<Shared>>,
    tx: Option<Sender<Msg>>,
    wake: Option<Wake>,
    import: Import,
    /// The device the window shows (the front panel's, live, an Android device), or None.
    want: Option<Target>,
    /// The slot on show: device, ring id, slot.
    shown: Option<(Target, u64, usize)>,
    retiring: Vec<(Target, u64, usize)>,
    path: FramePath,
    frames: u64,
    cpu_last_ms: Option<u64>,
}

impl Default for RegionFrames {
    fn default() -> Self {
        Self::new()
    }
}

impl RegionFrames {
    pub fn new() -> Self {
        Self {
            shared: Arc::default(),
            tx: None,
            wake: None,
            import: slint_import(),
            want: None,
            shown: None,
            retiring: Vec::new(),
            path: FramePath::None,
            frames: 0,
            cpu_last_ms: None,
        }
    }

    /// The wake the frame thread calls once a slot is ready (set before the first `sink`).
    pub fn set_wake(&mut self, wake: Wake) {
        self.wake = Some(wake);
    }

    /// How a texture becomes an image (tests).
    #[cfg(test)]
    pub fn set_import(&mut self, import: Import) {
        self.import = import;
    }

    /// Tests: refuse the zero-copy route, as a machine whose import fails does.
    #[cfg(test)]
    pub fn refuse_import(&self) {
        lock(&self.shared).no_import = true;
    }

    fn sender(&mut self) -> Sender<Msg> {
        if let Some(tx) = &self.tx {
            return tx.clone();
        }
        let (tx, rx) = channel();
        let shared = Arc::clone(&self.shared);
        let wake = self.wake.clone();
        let spawned = std::thread::Builder::new()
            .name("flare-device-frames".into())
            .spawn(move || Worker::new(shared, wake).run(&rx));
        if let Err(e) = spawned {
            log::warn!("device panel: no frame thread: {e}");
        }
        self.tx = Some(tx.clone());
        tx
    }

    /// Where the emu host's reader hands each frame record (`EmuConfig::on_region`). Starts the frame thread.
    pub fn sink(&mut self) -> OnRegion {
        let tx = self.sender();
        Arc::new(move |f| {
            let _ = tx.send(Msg::Frame(f));
        })
    }

    fn tell(&mut self, m: Msg) {
        if self.tx.is_some() {
            let _ = self.sender().send(m);
        }
    }

    /// The renderer's wgpu device (`RenderingSetup`): rings and imports are made again on it.
    pub fn attach_gpu(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        {
            let mut s = lock(&self.shared);
            s.gpu_made += 1;
            s.gpu = Some(Gpu {
                device: device.clone(),
                queue: queue.clone(),
                generation: s.gpu_made,
            });
            s.rings.clear();
        }
        self.shown = None;
        self.retiring.clear();
        self.tell(Msg::Gpu);
    }

    /// The renderer is torn down (or has no wgpu device): the CPU path from now on.
    pub fn detach_gpu(&mut self) {
        {
            let mut s = lock(&self.shared);
            s.gpu = None;
            s.rings.clear();
        }
        self.shown = None;
        self.retiring.clear();
        self.tell(Msg::Gpu);
    }

    /// A device is detached: its ring goes, and the frame thread lets go of its region (the file can then go).
    pub fn release(&mut self, target: &Target) {
        {
            let mut s = lock(&self.shared);
            s.rings.remove(target);
            s.cpu.remove(target);
        }
        if self.shown.as_ref().is_some_and(|(t, ..)| t == target) {
            self.shown = None;
        }
        self.retiring.retain(|(t, ..)| t != target);
        self.tell(Msg::Release(target.clone()));
    }

    /// The Android device the window shows now, or None. A slot shown before is handed back after the next frame.
    pub fn show(&mut self, target: Option<Target>) {
        if self.want == target {
            return;
        }
        self.want = target;
        if let Some(s) = self.shown.take() {
            self.retiring.push(s);
        }
    }

    /// `BeforeRendering`: the newest finished frame of the shown device as a Slint image, or None (the image on show
    /// stays). The slot shown before is retired.
    pub fn frame_image(&mut self, now_ms: u64) -> Option<Image> {
        let target = self.want.clone()?;
        let mut s = lock(&self.shared);
        // The CPU path: a picture the frame thread made, at most CPU_FPS a second.
        if s.gpu.is_none() || s.cpu_only {
            if self
                .cpu_last_ms
                .is_some_and(|l| now_ms.saturating_sub(l) < CPU_SPACING_MS)
            {
                return None;
            }
            let (_, buf) = s.cpu.remove(&target)?;
            drop(s);
            self.cpu_last_ms = Some(now_ms);
            self.path = FramePath::Cpu;
            self.frames += 1;
            return Some(Image::from_rgba8(buf));
        }
        let ring = s.rings.get_mut(&target)?;
        let (slot, _) = ring
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, sl)| match sl {
                Slot::Ready(seq) => Some((i, *seq)),
                _ => None,
            })
            .max_by_key(|(_, seq)| *seq)?;
        ring.slots[slot] = Slot::Shown;
        let texture = ring.textures[slot].clone();
        let id = ring.id;
        let route = s.route;
        drop(s);
        match (self.import)(texture) {
            Ok(img) => {
                if let Some(old) = self.shown.replace((target, id, slot)) {
                    self.retiring.push(old);
                }
                self.path = route;
                self.frames += 1;
                Some(img)
            }
            Err(e) => {
                let mut s = lock(&self.shared);
                s.cpu_only = true;
                s.refuse(format!("Slint refused the texture: {e}"));
                if let Some(r) = s.rings.get_mut(&target)
                    && r.id == id
                {
                    r.slots[slot] = Slot::Free;
                }
                None
            }
        }
    }

    /// `AfterRendering`: each slot this frame no longer samples is freed from the queue's work-done callback.
    pub fn after_render(&mut self) {
        if self.retiring.is_empty() {
            return;
        }
        let mut s = lock(&self.shared);
        let gpu = s.gpu.clone();
        for (t, id, slot) in std::mem::take(&mut self.retiring) {
            let Some(r) = s.rings.get_mut(&t).filter(|r| r.id == id) else {
                continue;
            };
            r.slots[slot] = Slot::Retiring;
            match &gpu {
                Some(g) => {
                    let shared = Arc::clone(&self.shared);
                    // untimed: the callback only frees a slot (a lock)
                    g.queue.on_submitted_work_done(move || {
                        if let Some(r) = lock(&shared).rings.get_mut(&t).filter(|r| r.id == id)
                            && r.slots[slot] == Slot::Retiring
                        {
                            r.slots[slot] = Slot::Free;
                        }
                    });
                }
                None => r.slots[slot] = Slot::Free,
            }
        }
    }

    /// A slot became ready since the last call (the window's wake asks for a redraw).
    pub fn take_new(&self) -> bool {
        std::mem::take(&mut lock(&self.shared).new_frame)
    }

    /// The CPU path holds a picture for a later render (at most CPU_FPS a second).
    pub fn wants_redraw(&self) -> bool {
        let s = lock(&self.shared);
        self.want
            .as_ref()
            .is_some_and(|t| (s.gpu.is_none() || s.cpu_only) && s.cpu.contains_key(t))
    }

    pub fn path(&self) -> FramePath {
        self.path
    }

    pub fn showing(&self, target: &Target) -> bool {
        self.want.as_ref() == Some(target) && (self.shown.is_some() || self.path == FramePath::Cpu)
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Slots the window holds (on show or retiring).
    pub fn held(&self) -> usize {
        lock(&self.shared)
            .rings
            .values()
            .flat_map(|r| r.slots.iter())
            .filter(|s| matches!(s, Slot::Shown | Slot::Retiring))
            .count()
    }

    pub fn refused(&self) -> Option<String> {
        lock(&self.shared).refused.clone()
    }

    /// Frames copied, frames dropped (a newer record came first), and the copy's CPU time p50 and p99 in ms.
    pub fn stats(&self) -> (u64, u64, f64, f64) {
        let s = lock(&self.shared);
        let mut v = s.copy_us.clone();
        v.sort_unstable();
        let at = |p: f64| {
            v.get(((v.len() as f64 * p) as usize).min(v.len().saturating_sub(1)))
                .map_or(0.0, |us| f64::from(*us) / 1000.0)
        };
        (s.copied, s.dropped, at(0.5), at(0.99))
    }

    /// Tests: waits until the frame thread handled everything sent so far.
    #[cfg(test)]
    pub fn sync(&mut self) {
        let (tx, rx) = channel();
        if self.tx.is_none() {
            return;
        }
        let _ = self.sender().send(Msg::Sync(tx));
        let _ = rx.recv_timeout(std::time::Duration::from_secs(30)); // sync-ok: test code only
    }
}

impl Drop for RegionFrames {
    fn drop(&mut self) {
        // The frame thread ends when its channel closes; it lets go of every region on the way out.
        self.tx = None;
    }
}

// ---- the frame thread -----------------------------------------------------------------------------------------

/// The zero-copy import: the region as a GPU buffer, and what must outlive it.
///
/// Dropping it is the only way an import goes, on every path (a release, a new wgpu device, a refused first copy):
/// the buffer goes, then the frame thread waits until the device the import was made on is idle, then the heap goes.
/// The wait is on the import's own device, not the renderer's current one: after a device change, copies submitted on
/// the old device may still read the region, and the mapping must not go before they end.
struct Imported {
    /// Always Some until the drop.
    buffer: Option<wgpu::Buffer>,
    /// The device and queue the buffer was made on.
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Held only so it outlives the buffer placed on it (it goes last, after the wait).
    #[cfg(windows)]
    _heap: windows::Win32::Graphics::Direct3D12::ID3D12Heap,
}

impl Drop for Imported {
    fn drop(&mut self) {
        drop(self.buffer.take());
        self.queue.submit([]);
        // The frame thread may wait: the copies that read the region finish first (no UI thread here).
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        // The heap (Windows) goes after this, with the struct's other fields.
    }
}

/// A mapped region and, once tried, its import on one wgpu device.
///
/// `import` is declared before `map`, so a dropped `Mapped` lets go of the import (and waits for the GPU) before it
/// unmaps the view.
struct Mapped {
    path: PathBuf,
    import: Option<Imported>,
    map: memmap2::MmapMut,
    /// The wgpu device generation the import (or its refusal) belongs to.
    gpu: u64,
    refused: bool,
    cpu_last: Option<Instant>,
}

struct Pipe {
    gpu: u64,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

struct Worker {
    shared: Arc<Mutex<Shared>>,
    wake: Option<Wake>,
    regions: HashMap<Target, Mapped>,
    pipe: Option<Result<Pipe, ()>>,
}

impl Worker {
    fn new(shared: Arc<Mutex<Shared>>, wake: Option<Wake>) -> Self {
        Self {
            shared,
            wake,
            regions: HashMap::new(),
            pipe: None,
        }
    }

    fn run(mut self, rx: &Receiver<Msg>) {
        while let Ok(first) = rx.recv() {
            // sync-ok: the frame thread waits for its next record
            // The newest record per device wins: older ones still waiting are dropped (counted).
            let mut frames: HashMap<Target, RegionFrame> = HashMap::new();
            let mut msgs = vec![first];
            msgs.extend(rx.try_iter());
            let mut dropped = 0;
            for m in msgs {
                match m {
                    Msg::Frame(f) => {
                        if frames.insert(f.target.clone(), f).is_some() {
                            dropped += 1;
                        }
                    }
                    Msg::Release(t) => {
                        frames.remove(&t);
                        self.release(&t);
                    }
                    Msg::Gpu => self.gpu_changed(),
                    #[cfg(test)]
                    Msg::Sync(done) => {
                        for (_, f) in frames.drain() {
                            self.frame(&f);
                        }
                        let _ = done.send(());
                    }
                }
            }
            if dropped > 0 {
                lock(&self.shared).dropped += dropped;
            }
            for (_, f) in frames {
                self.frame(&f);
            }
        }
        // The window is gone: every region is let go.
        let targets: Vec<Target> = self.regions.keys().cloned().collect();
        for t in targets {
            self.release(&t);
        }
    }

    fn gpu(&self) -> Option<Gpu> {
        lock(&self.shared).gpu.clone()
    }

    /// The renderer's device changed: imports and the pipeline of the old one go (the old device is gone or idle).
    fn gpu_changed(&mut self) {
        let generation = self.gpu().map_or(0, |g| g.generation);
        for m in self.regions.values_mut() {
            if m.gpu != generation {
                m.import = None;
                m.refused = false;
                m.gpu = generation;
            }
        }
        self.pipe = None;
    }

    /// Lets go of a device's region: the buffer, then (once the GPU is idle) the heap, then the view.
    fn release(&mut self, t: &Target) {
        let Some(mut m) = self.regions.remove(t) else {
            return;
        };
        // The import first (it waits for its own device), then the view.
        drop(m.import.take());
        let path = std::mem::take(&mut m.path);
        drop(m);
        log::debug!("device frames: {t}: let go of {}", path.display());
    }

    /// The region of `f`, mapped (again if its file changed).
    fn mapped(&mut self, f: &RegionFrame) -> Option<&mut Mapped> {
        if self
            .regions
            .get(&f.target)
            .is_some_and(|m| m.path != f.path)
        {
            self.release(&f.target);
        }
        if !self.regions.contains_key(&f.target) {
            match map_region(&f.path) {
                Ok(map) => {
                    let gpu = self.gpu().map_or(0, |g| g.generation);
                    self.regions.insert(
                        f.target.clone(),
                        Mapped {
                            path: f.path.clone(),
                            map,
                            import: None,
                            gpu,
                            refused: false,
                            cpu_last: None,
                        },
                    );
                }
                Err(e) => {
                    log::warn!("device frames: {}: the region does not map: {e}", f.target);
                    return None;
                }
            }
        }
        self.regions.get_mut(&f.target)
    }

    fn frame(&mut self, f: &RegionFrame) {
        let started = Instant::now();
        let bytes = u64::from(f.w) * u64::from(f.h) * 4;
        let gpu = self.gpu();
        let cpu_only = lock(&self.shared).cpu_only;
        let shared = Arc::clone(&self.shared);
        let Some(m) = self.mapped(f) else { return };
        if f.w == 0 || f.h == 0 || bytes > m.map.len() as u64 {
            log::debug!(
                "device frames: {}: a frame of {}x{} does not fit its region",
                f.target,
                f.w,
                f.h
            );
            return;
        }
        let Some(gpu) = gpu.filter(|_| !cpu_only) else {
            cpu_copy(&shared, m, f);
            self.woken();
            return;
        };
        let Some((ring_id, slot, texture)) = take_slot(&self.shared, &gpu, f) else {
            lock(&self.shared).dropped += 1;
            return;
        };
        let route = self.copy(&gpu, f, &texture);
        let us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
        {
            let mut s = lock(&self.shared);
            s.copied += 1;
            let n = s.copied;
            if let Some(r) = s.rings.get_mut(&f.target).filter(|r| r.id == ring_id) {
                // An older frame nobody took is replaced by this one.
                for sl in &mut r.slots {
                    if matches!(sl, Slot::Ready(q) if *q < n) {
                        *sl = Slot::Free;
                    }
                }
                r.slots[slot] = Slot::Ready(n);
            }
            s.route = route;
            if s.copy_us.len() >= 600 {
                s.copy_us.remove(0);
            }
            s.copy_us.push(us);
            s.new_frame = true;
        }
        self.woken();
    }

    fn woken(&self) {
        if let Some(w) = &self.wake {
            w();
        }
    }

    /// Records and submits the copy of `f`'s region into `texture`: zero copy when this machine allows it, else an
    /// upload. Returns the route taken.
    fn copy(&mut self, gpu: &Gpu, f: &RegionFrame, texture: &wgpu::Texture) -> FramePath {
        if self.zero_copy(gpu, f, texture) {
            return FramePath::Gpu;
        }
        let Some(m) = self.regions.get(&f.target) else {
            return FramePath::Upload;
        };
        let n = (f.w as usize) * (f.h as usize) * 4;
        gpu.queue.write_texture(
            texture.as_image_copy(),
            &m.map[..n],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(f.w * 4),
                rows_per_image: Some(f.h),
            },
            wgpu::Extent3d {
                width: f.w,
                height: f.h,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([]);
        FramePath::Upload
    }

    /// The compute copy from the imported region. False (and the reason kept) when the route is refused.
    fn zero_copy(&mut self, gpu: &Gpu, f: &RegionFrame, texture: &wgpu::Texture) -> bool {
        let no_import = lock(&self.shared).no_import;
        // The pipeline, once per wgpu device.
        if self
            .pipe
            .as_ref()
            .is_some_and(|p| p.as_ref().is_ok_and(|p| p.gpu != gpu.generation))
        {
            self.pipe = None;
        }
        if self.pipe.is_none() {
            let made = make_pipe(gpu);
            if let Err(why) = &made {
                lock(&self.shared).refuse(why.clone());
            }
            self.pipe = Some(made.map_err(|_| ()));
        }
        let Some(Ok(pipe)) = &self.pipe else {
            return false;
        };
        let Some(m) = self.regions.get_mut(&f.target) else {
            return false;
        };
        if m.gpu != gpu.generation {
            m.import = None;
            m.refused = false;
            m.gpu = gpu.generation;
        }
        if m.refused {
            return false;
        }
        let first = m.import.is_none();
        if first {
            let tried = if no_import {
                Err("the zero-copy import is turned off (test)".to_string())
            } else {
                import(gpu, m.map.as_mut_ptr(), m.map.len() as u64)
            };
            match tried {
                Ok(i) => m.import = Some(i),
                Err(why) => {
                    m.refused = true;
                    lock(&self.shared).refuse(why);
                    return false;
                }
            }
        }
        let Some(buffer) = m.import.as_ref().and_then(|i| i.buffer.as_ref()) else {
            return false;
        };
        // The first copy from a new import runs in an error scope: a refusal falls back to the upload.
        let scope = first.then(|| gpu.device.push_error_scope(wgpu::ErrorFilter::Validation));
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("device region"),
            layout: &pipe.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("device region copy"),
            });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipe.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(f.w.div_ceil(WG), f.h.div_ceil(WG), 1);
        }
        let cmd = enc.finish();
        if let Some(scope) = scope
            && let Some(e) = pollster::block_on(scope.pop())
        {
            m.refused = true;
            m.import = None;
            lock(&self.shared).refuse(format!("the GPU refused the region copy: {e}"));
            return false;
        }
        gpu.queue.submit([cmd]);
        true
    }
}

/// BLOCKING (opens and maps a file): the frame thread's own.
fn map_region(path: &Path) -> std::io::Result<memmap2::MmapMut> {
    let file = std::fs::OpenOptions::new() // sync-ok: the frame thread
        .read(true)
        .write(true)
        .open(path)?;
    // SAFETY: the file is Flare's own region; the emulator writes into it while it is mapped, which only changes the
    // pixels a copy reads (a torn frame at worst, never memory safety: the bytes are only copied, never read as Rust
    // values with invariants).
    unsafe { memmap2::MmapMut::map_mut(&file) }
}

/// A free slot of `f`'s ring (made, or made again for a new size or device), marked Writing; None when every slot is
/// held (the frame is dropped).
fn take_slot(
    shared: &Mutex<Shared>,
    gpu: &Gpu,
    f: &RegionFrame,
) -> Option<(u64, usize, wgpu::Texture)> {
    let mut s = lock(shared);
    let fits = s
        .rings
        .get(&f.target)
        .is_some_and(|r| r.w == f.w && r.h == f.h && r.gpu == gpu.generation);
    if !fits {
        s.rings_made += 1;
        let id = s.rings_made;
        let textures = (0..RING_SLOTS)
            .map(|_| ring_texture(&gpu.device, f.w, f.h))
            .collect();
        s.rings.insert(
            f.target.clone(),
            Ring {
                id,
                gpu: gpu.generation,
                w: f.w,
                h: f.h,
                textures,
                slots: vec![Slot::Free; RING_SLOTS],
            },
        );
    }
    let r = s.rings.get_mut(&f.target)?;
    let slot = r.slots.iter().position(|s| *s == Slot::Free).or_else(|| {
        // No free slot: the oldest ready frame nobody took is written over.
        r.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s {
                Slot::Ready(q) => Some((i, *q)),
                _ => None,
            })
            .min_by_key(|(_, q)| *q)
            .map(|(i, _)| i)
    })?;
    r.slots[slot] = Slot::Writing;
    Some((r.id, slot, r.textures[slot].clone()))
}

/// One ring texture: what Slint imports (`Rgba8Unorm`, `TEXTURE_BINDING | RENDER_ATTACHMENT`), what the compute pass
/// writes (`STORAGE_BINDING`) and the upload writes (`COPY_DST`).
fn ring_texture(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("device region ring"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// The compute pipeline on `gpu`, after checking its limits, in an error scope.
fn make_pipe(gpu: &Gpu) -> Result<Pipe, String> {
    let l = gpu.device.limits();
    if l.max_storage_textures_per_shader_stage < 1
        || l.max_storage_buffers_per_shader_stage < 1
        || l.max_compute_invocations_per_workgroup < WG * WG
        || l.max_compute_workgroup_size_x < WG
        || l.max_compute_workgroup_size_y < WG
    {
        return Err("the GPU device has no room for the region's compute copy".into());
    }
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("device region copy"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
    let layout = gpu
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("device region copy"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
    let pl = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("device region copy"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
    let pipeline = gpu
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("device region copy"),
            layout: Some(&pl),
            module: &module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(format!("the region's compute copy did not build: {e}"));
    }
    Ok(Pipe {
        gpu: gpu.generation,
        layout,
        pipeline,
    })
}

/// The CPU path: the region copied into a picture, at most CPU_FPS a second.
fn cpu_copy(shared: &Mutex<Shared>, m: &mut Mapped, f: &RegionFrame) {
    let now = Instant::now();
    if m.cpu_last
        .is_some_and(|l| now.duration_since(l).as_millis() < u128::from(CPU_SPACING_MS))
    {
        lock(shared).dropped += 1;
        return;
    }
    m.cpu_last = Some(now);
    let n = (f.w as usize) * (f.h as usize) * 4;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(f.w, f.h);
    buf.make_mut_bytes().copy_from_slice(&m.map[..n]);
    let mut s = lock(shared);
    s.cpu.insert(f.target.clone(), (f.seq, buf));
    s.route = FramePath::Cpu;
    s.copied += 1;
    s.new_frame = true;
}

/// The zero-copy import (Windows, D3D12): a heap opened on the mapped view, and a buffer placed on it.
#[cfg(windows)]
fn import(gpu: &Gpu, base: *mut u8, size: u64) -> Result<Imported, String> {
    let device = &gpu.device;
    use windows::Win32::Graphics::Direct3D12::{
        D3D12_HEAP_FLAG_SHARED_CROSS_ADAPTER, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
        D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER, D3D12_RESOURCE_FLAG_NONE,
        D3D12_RESOURCE_STATE_COMMON, D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ID3D12Device3, ID3D12Heap,
        ID3D12Resource,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
    use windows::core::Interface;
    if base.is_null() || size == 0 {
        return Err("no region to import".into());
    }
    let (heap, res) = {
        // SAFETY: only the raw D3D12 device is read while the guard lives.
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }
            .ok_or("the renderer's device is not DX12")?;
        let dev3: ID3D12Device3 = hal
            .raw_device()
            .cast()
            .map_err(|e| format!("no ID3D12Device3: {e}"))?;
        drop(hal);
        // SAFETY: `base` is the start of a live read-write file view of `size` bytes (64 KiB aligned, rule 1363's
        // region), which outlives the heap: the frame thread drops the heap before the view.
        let heap: ID3D12Heap =
            unsafe { dev3.OpenExistingHeapFromAddress(base.cast_const().cast()) }
                .map_err(|e| format!("OpenExistingHeapFromAddress: {e}"))?;
        // SAFETY: a plain query of the heap just opened.
        let hd = unsafe { heap.GetDesc() };
        if hd.SizeInBytes < size {
            return Err(format!(
                "the region's heap holds {} bytes, not {size}",
                hd.SizeInBytes
            ));
        }
        let cross = (hd.Flags.0 & D3D12_HEAP_FLAG_SHARED_CROSS_ADAPTER.0) != 0;
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: size,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: if cross {
                D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER
            } else {
                D3D12_RESOURCE_FLAG_NONE
            },
        };
        let mut res: Option<ID3D12Resource> = None;
        // SAFETY: a buffer of the heap's size at offset 0 (spike S3's call).
        unsafe {
            dev3.CreatePlacedResource(&heap, 0, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut res)
        }
        .map_err(|e| format!("CreatePlacedResource: {e}"))?;
        (heap, res.ok_or("CreatePlacedResource gave no buffer")?)
    };
    // SAFETY: the resource is a buffer of `size` bytes made on this wgpu device's own D3D12 device.
    let hal_buf = unsafe { wgpu::hal::dx12::Device::buffer_from_raw(res, size) };
    // SAFETY: the hal buffer belongs to this device; the descriptor matches it (read-only storage).
    let buffer = unsafe {
        device.create_buffer_from_hal::<wgpu::hal::api::Dx12>(
            hal_buf,
            &wgpu::BufferDescriptor {
                label: Some("device region"),
                size,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            },
        )
    };
    Ok(Imported {
        buffer: Some(buffer),
        device: gpu.device.clone(),
        queue: gpu.queue.clone(),
        _heap: heap,
    })
}

/// The zero-copy import (macOS, Metal): a no-copy buffer on the mapping (unified memory).
#[cfg(target_os = "macos")]
fn import(gpu: &Gpu, base: *mut u8, size: u64) -> Result<Imported, String> {
    let device = &gpu.device;
    use objc2_metal::{MTLDevice, MTLResourceOptions};
    let ptr =
        std::ptr::NonNull::new(base.cast::<std::ffi::c_void>()).ok_or("no region to import")?;
    let len = usize::try_from(size).map_err(|_| "the region is too big")?;
    let raw = {
        // SAFETY: only the raw Metal device is read while the guard lives.
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }
            .ok_or("the renderer's device is not Metal")?;
        // SAFETY: `ptr` is the page-aligned start of a live read-write mapping of `len` bytes (a whole number of
        // 64 KiB blocks), which outlives the buffer: the frame thread drops the buffer and waits for the GPU before
        // it unmaps. No deallocator: the mapping is Flare's own.
        unsafe {
            hal.raw_device()
                .newBufferWithBytesNoCopy_length_options_deallocator(
                    ptr,
                    len,
                    MTLResourceOptions::StorageModeShared,
                    None,
                )
        }
        .ok_or("Metal refused the region")?
    };
    // SAFETY: the buffer was made on this wgpu device's own MTLDevice with this size.
    let hal_buf = unsafe { wgpu::hal::metal::Device::buffer_from_raw(raw, size) };
    // SAFETY: the hal buffer belongs to this device; the descriptor matches it (read-only storage).
    let buffer = unsafe {
        device.create_buffer_from_hal::<wgpu::hal::api::Metal>(
            hal_buf,
            &wgpu::BufferDescriptor {
                label: Some("device region"),
                size,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            },
        )
    };
    Ok(Imported {
        buffer: Some(buffer),
        device: gpu.device.clone(),
        queue: gpu.queue.clone(),
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn import(_gpu: &Gpu, _base: *mut u8, _size: u64) -> Result<Imported, String> {
    Err("no zero-copy import on this system".into())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use flare_device::emu::region;
    use flare_device::geom::Orientation;

    use super::*;

    fn target() -> Target {
        Target::emu("Flare_Pixel").expect("an AVD name")
    }

    /// A region file of a 1080-wide screen's size class, filled with a pattern: pixel (x, y) is (x, y, seq, 255).
    /// 270 pixels wide: rows of 1080 bytes, which `copy_buffer_to_texture` could not take (256-byte rows).
    struct TestRegion {
        _dir: tempfile::TempDir,
        path: PathBuf,
        w: u32,
        h: u32,
    }

    impl TestRegion {
        fn new(w: u32, h: u32) -> TestRegion {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("1-1.rgba");
            let f = std::fs::File::create(&path).unwrap();
            f.set_len(region::bytes_for(w, h)).unwrap();
            TestRegion {
                _dir: dir,
                path,
                w,
                h,
            }
        }

        /// The emulator's write: the whole frame of `seq`, `w` by `h` (already turned).
        fn write(&self, w: u32, h: u32, seq: u8) {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.path)
                .unwrap();
            // SAFETY: a test's own file.
            let mut map = unsafe { memmap2::MmapMut::map_mut(&file) }.unwrap();
            for y in 0..h {
                for x in 0..w {
                    let o = ((y * w + x) * 4) as usize;
                    map[o..o + 4].copy_from_slice(&[x as u8, y as u8, seq, 255]);
                }
            }
            map.flush().unwrap();
        }

        fn frame(&self, seq: u64) -> RegionFrame {
            self.frame_of(self.w, self.h, seq)
        }

        fn frame_of(&self, w: u32, h: u32, seq: u64) -> RegionFrame {
            RegionFrame {
                target: target(),
                seq,
                t_us: seq * 16_000,
                w,
                h,
                rotation: if w > h {
                    Orientation::Rot270
                } else {
                    Orientation::Rot0
                },
                path: self.path.clone(),
            }
        }
    }

    /// The pattern's check: a few pixels of the texture read back through the GPU.
    fn check(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture, seq: u8) {
        let (w, h) = (tex.width(), tex.height());
        let px = flare_render::gpu::read_texture(device, queue, tex);
        for (x, y) in [
            (0, 0),
            (w - 1, 0),
            (w / 2, h / 3),
            (w - 1, h - 1),
            (3, h - 1),
        ] {
            let o = ((y * w + x) * 4) as usize;
            assert_eq!(
                px[o..o + 4],
                [x as u8, y as u8, seq, 255],
                "pixel ({x}, {y}) of {w}x{h}"
            );
        }
    }

    /// Frames whose import keeps each texture it is given (to read it back) and makes a plain image.
    fn frames_keeping(kept: &Rc<RefCell<Vec<wgpu::Texture>>>) -> RegionFrames {
        let mut f = RegionFrames::new();
        let k = Rc::clone(kept);
        f.set_import(Box::new(move |t| {
            k.borrow_mut().push(t.clone());
            Image::try_from(t).map_err(|e| format!("{e:?}"))
        }));
        f
    }

    /// Rule 1364: a frame record makes the frame thread copy the region into a ring slot on the GPU (zero copy where
    /// this machine allows it), and the notifier's image is that slot with the region's pixels. Slint takes the ring
    /// texture (`Image::try_from`).
    #[test]
    fn a_region_frame_is_copied_by_the_gpu_into_the_ring() {
        let Ok((device, queue, info)) = flare_render::gpu::headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let r = TestRegion::new(270, 600);
        r.write(270, 600, 7);
        let kept = Rc::new(RefCell::new(Vec::new()));
        let mut f = frames_keeping(&kept);
        f.attach_gpu(&device, &queue);
        let sink = f.sink();
        f.show(Some(target()));
        sink(r.frame(1));
        f.sync();
        assert!(f.take_new(), "the frame thread said a slot is ready");
        let img = f.frame_image(0).expect("the slot as a Slint image");
        assert_eq!((img.size().width, img.size().height), (270, 600));
        eprintln!(
            "adapter {} ({:?}): route {:?}, refused {:?}",
            info.name,
            info.backend,
            f.path(),
            f.refused()
        );
        // The zero-copy route on DX12 and Metal; another adapter (WARP may refuse the import) uploads instead.
        assert!(matches!(f.path(), FramePath::Gpu | FramePath::Upload));
        if f.path() == FramePath::Upload {
            assert!(f.refused().is_some(), "an upload says why");
        }
        check(&device, &queue, &kept.borrow()[0], 7);
        assert!(f.showing(&target()));
        assert_eq!(f.held(), 1);
        let (copied, dropped, ..) = f.stats();
        assert_eq!((copied, dropped), (1, 0));
    }

    /// Spike S3's real size: a 1080x2400 frame in a region of `bytes_for(1080, 2400)` (64 KiB aligned) imports and
    /// copies; a turned frame (2400x1080) makes the ring again at the new size.
    #[test]
    fn a_full_size_frame_and_a_turned_one_copy_into_the_ring() {
        let Ok((device, queue, info)) = flare_render::gpu::headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let r = TestRegion::new(1080, 2400);
        r.write(1080, 2400, 3);
        let kept = Rc::new(RefCell::new(Vec::new()));
        let mut f = frames_keeping(&kept);
        f.attach_gpu(&device, &queue);
        let sink = f.sink();
        f.show(Some(target()));
        sink(r.frame(1));
        f.sync();
        assert!(f.frame_image(0).is_some());
        eprintln!(
            "1080x2400 on {} ({:?}): route {:?}, stats {:?}",
            info.name,
            info.backend,
            f.path(),
            f.stats()
        );
        check(&device, &queue, &kept.borrow()[0], 3);
        // Turned: the emulator sends the landscape picture itself.
        r.write(2400, 1080, 4);
        sink(r.frame_of(2400, 1080, 2));
        f.sync();
        let img = f.frame_image(16).expect("the turned frame");
        assert_eq!((img.size().width, img.size().height), (2400, 1080));
        check(&device, &queue, kept.borrow().last().unwrap(), 4);
    }

    /// The fallback: with the zero-copy route refused (as on a machine whose import fails), the region is uploaded
    /// with `write_texture`, the reason is kept, and the pixels are the same.
    #[test]
    fn a_refused_import_uploads_the_region_instead() {
        let Ok((device, queue, _)) = flare_render::gpu::headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let r = TestRegion::new(270, 600);
        r.write(270, 600, 9);
        let kept = Rc::new(RefCell::new(Vec::new()));
        let mut f = frames_keeping(&kept);
        f.refuse_import();
        f.attach_gpu(&device, &queue);
        let sink = f.sink();
        f.show(Some(target()));
        sink(r.frame(1));
        f.sync();
        assert!(f.frame_image(0).is_some());
        assert_eq!(f.path(), FramePath::Upload);
        assert!(f.refused().is_some_and(|w| w.contains("turned off")));
        check(&device, &queue, &kept.borrow()[0], 9);
    }

    /// Slot ownership: the slot on show is never written; it is freed only from the work-done callback after the
    /// frame that replaced it, and a record that finds every slot held is dropped, never queued.
    #[test]
    fn a_shown_slot_is_freed_only_after_the_gpu_is_done() {
        let Ok((device, queue, _)) = flare_render::gpu::headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let r = TestRegion::new(64, 32);
        r.write(64, 32, 1);
        let kept = Rc::new(RefCell::new(Vec::new()));
        let mut f = frames_keeping(&kept);
        f.attach_gpu(&device, &queue);
        let sink = f.sink();
        f.show(Some(target()));
        sink(r.frame(1));
        f.sync();
        assert!(f.frame_image(0).is_some());
        f.after_render();
        sink(r.frame(2));
        f.sync();
        assert!(f.frame_image(16).is_some(), "the next slot replaces it");
        assert_eq!(
            f.held(),
            2,
            "replaced, but the frame that sampled it may still run"
        );
        f.after_render();
        assert_eq!(f.held(), 2, "registered, not yet run");
        queue.submit([]);
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the queue drains");
        assert_eq!(f.held(), 1, "freed by the work-done callback");
        // Two ready frames nobody took: the newer is shown, the older is written over.
        sink(r.frame(3));
        f.sync();
        sink(r.frame(4));
        f.sync();
        assert!(f.frame_image(32).is_some());
        let (copied, ..) = f.stats();
        assert_eq!(copied, 4);
        // Closing the panel hands the slot back the same way.
        f.show(None);
        f.after_render();
        queue.submit([]);
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the queue drains");
        assert_eq!(f.held(), 0);
    }

    /// A new stream starts the emulator's `seq` again (a resume, an emu host restart): the newest copy shows, never a
    /// frame left from before whose `seq` is higher.
    #[test]
    fn the_newest_copy_shows_when_a_new_stream_starts_its_seq_again() {
        let Ok((device, queue, _)) = flare_render::gpu::headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let r = TestRegion::new(64, 32);
        let kept = Rc::new(RefCell::new(Vec::new()));
        let mut f = frames_keeping(&kept);
        f.attach_gpu(&device, &queue);
        let sink = f.sink();
        // A frame nobody took (the panel was hidden), then the new stream's first frame.
        r.write(64, 32, 1);
        sink(r.frame(500));
        f.sync();
        r.write(64, 32, 2);
        sink(r.frame(1));
        f.sync();
        f.show(Some(target()));
        assert!(f.frame_image(0).is_some());
        check(&device, &queue, kept.borrow().last().unwrap(), 2);
    }

    /// With no wgpu device (the software renderer), the CPU path copies the region into a picture at most 30 a
    /// second; a refused texture import switches to it too.
    #[test]
    fn without_a_gpu_the_region_is_copied_by_the_cpu() {
        let r = TestRegion::new(8, 4);
        r.write(8, 4, 5);
        let mut f = RegionFrames::new();
        let sink = f.sink();
        f.show(Some(target()));
        sink(r.frame(1));
        f.sync();
        let img = f.frame_image(100).expect("a CPU picture");
        assert_eq!(f.path(), FramePath::Cpu);
        let px = img.to_rgba8().expect("pixels");
        assert_eq!((px.width(), px.height()), (8, 4));
        assert_eq!(px.as_bytes()[4 * 3..4 * 4], [3, 0, 5, 255]);
        // At most 30 a second: a frame 10 ms later waits.
        sink(r.frame(2));
        f.sync();
        // The frame thread itself spaces its copies too, so this one may be dropped there; either way none shows yet.
        assert!(f.frame_image(110).is_none());
        // Slint refusing the texture: the CPU path for the session.
        let Ok((device, queue, _)) = flare_render::gpu::headless_device() else {
            return;
        };
        let mut g = RegionFrames::new();
        g.set_import(Box::new(|_| Err("TextureImportError".into())));
        g.attach_gpu(&device, &queue);
        let sink = g.sink();
        g.show(Some(target()));
        sink(r.frame(1));
        g.sync();
        assert!(g.frame_image(0).is_none(), "refused");
        assert!(
            g.refused()
                .is_some_and(|w| w.contains("TextureImportError"))
        );
        std::thread::sleep(Duration::from_millis(40));
        sink(r.frame(2));
        g.sync();
        assert!(g.frame_image(100).is_some(), "the CPU path from now on");
        assert_eq!(g.path(), FramePath::Cpu);
    }

    /// Rule 1363: once a device is detached the frame thread lets go of its region (buffer, heap, view), so the
    /// file can be deleted, also on Windows, which refuses to delete a mapped file.
    #[test]
    fn a_released_region_can_be_deleted() {
        let r = TestRegion::new(64, 32);
        r.write(64, 32, 1);
        let mut f = RegionFrames::new();
        if let Ok((device, queue, _)) = flare_render::gpu::headless_device() {
            f.attach_gpu(&device, &queue);
        }
        let sink = f.sink();
        f.show(Some(target()));
        sink(r.frame(1));
        f.sync();
        f.release(&target());
        f.sync();
        std::fs::remove_file(&r.path).expect("the region file goes once it is let go");
    }

    /// PR A7 review: a new wgpu device (the renderer made again) drops the import made on the old one. That drop waits
    /// on the old device, not the current one, so the old copies end before the view can go; frames copy on the new
    /// device from a fresh import, and a release then lets the file go.
    #[test]
    fn a_new_gpu_device_lets_go_of_the_old_import_first() {
        let Ok((device, queue, _)) = flare_render::gpu::headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let Ok((device2, queue2, _)) = flare_render::gpu::headless_device() else {
            return;
        };
        let r = TestRegion::new(64, 32);
        r.write(64, 32, 1);
        let kept = Rc::new(RefCell::new(Vec::new()));
        let mut f = frames_keeping(&kept);
        f.attach_gpu(&device, &queue);
        let sink = f.sink();
        f.show(Some(target()));
        sink(r.frame(1));
        f.sync();
        assert!(f.frame_image(0).is_some());
        // The renderer's device changes, a frame comes at once (both in one batch on the frame thread), and then the
        // old device is gone.
        f.attach_gpu(&device2, &queue2);
        r.write(64, 32, 2);
        sink(r.frame(2));
        f.sync();
        drop((device, queue));
        let path = f.path();
        assert!(f.frame_image(16).is_some(), "a frame on the new device");
        assert_eq!(f.path(), path, "the same route on the new device");
        check(&device2, &queue2, kept.borrow().last().unwrap(), 2);
        f.release(&target());
        f.sync();
        std::fs::remove_file(&r.path).expect("the region file goes once it is let go");
    }

    /// The real path end to end (opt-in, `FLARE_DEVICE_REAL=1`, `cargo build -p flare-emu-host` first; by hand only):
    /// an AVD written for the test into its own folder (deleted after; the person's AVDs are never listed) boots
    /// headless with the person's SDK (read only), `EmuSource` attaches, and the frame thread copies its region on this
    /// process's own wgpu device while swipes make frames. Then the region file is gone after detach, and quit's own
    /// command (`flare-emu-host --shutdown`) ends the emulator. `FLARE_DEVICE_IMAGE` names another image package.
    #[test]
    fn a_real_emulator_region_copies_on_the_gpu() {
        use flare_device::emu::discovery;
        use flare_device::emu::{Places, flare_avd};
        use flare_device::emu_client::{EmuConfig, EmuSource};
        use flare_device::source::{DeviceSource, Phase, SourceEvent, Touch};
        use std::time::Instant;
        if std::env::var("FLARE_DEVICE_REAL").as_deref() != Ok("1") {
            eprintln!(
                "skipped: set FLARE_DEVICE_REAL=1 to run against an emulator made for the test"
            );
            return;
        }
        let exe = std::env::current_exe()
            .unwrap()
            .parent()
            .and_then(|d| d.parent())
            .map(|d| d.join(flare_win::names::EMU_HOST_EXE))
            .unwrap();
        assert!(exe.exists(), "build the emu host first: {}", exe.display());
        let (device, queue, info) = flare_render::gpu::headless_device().expect("a wgpu device");
        let home = tempfile::tempdir().unwrap();
        let abi = if cfg!(target_arch = "aarch64") {
            "arm64-v8a"
        } else {
            "x86_64"
        };
        let image = std::env::var("FLARE_DEVICE_IMAGE")
            .unwrap_or_else(|_| format!("system-images;android-37.0;google_apis;{abi}"));
        let img = flare_avd::Image::from_path(&image).expect("an image package path");
        flare_avd::write(home.path(), &img).expect("the AVD is written");
        let mut places = Places::from_env();
        places.android_avd_home = Some(home.path().to_path_buf());
        let running_dir = places.discovery_dir().expect("a discovery folder");
        let logs = tempfile::tempdir().unwrap();
        let mut cfg = EmuConfig::new(exe.clone(), logs.path().join("emu-host.log"), places);
        cfg.logs_dir = Some(logs.path().to_path_buf());
        cfg.env = vec![(
            "ANDROID_AVD_HOME".into(),
            home.path().to_string_lossy().to_string(),
        )];
        let kept = Rc::new(RefCell::new(Vec::new()));
        let mut f = frames_keeping(&kept);
        f.attach_gpu(&device, &queue);
        cfg.on_region = Some(f.sink());
        let mut src = EmuSource::start(cfg);
        let t = Target::emu(flare_avd::NAME).unwrap();
        /// Ends the emulator through quit's own command, whatever happens, and waits for it to go.
        struct Down(PathBuf, PathBuf, PathBuf);
        impl Drop for Down {
            fn drop(&mut self) {
                let started = Instant::now();
                let _ = std::process::Command::new(&self.0)
                    .args(["--shutdown", flare_avd::NAME])
                    .env("ANDROID_AVD_HOME", &self.1)
                    .status();
                while started.elapsed() < Duration::from_secs(120)
                    && discovery::of_avd(&discovery::read_all(&self.2), flare_avd::NAME).is_some()
                {
                    std::thread::sleep(Duration::from_millis(500));
                }
                eprintln!(
                    "--shutdown: gone after {:.1} s: {}",
                    started.elapsed().as_secs_f64(),
                    discovery::of_avd(&discovery::read_all(&self.2), flare_avd::NAME).is_none()
                );
            }
        }
        let _down = Down(exe.clone(), home.path().to_path_buf(), running_dir.clone());
        let wait = |src: &EmuSource, id, limit: u64| {
            let end = Instant::now() + Duration::from_secs(limit);
            while Instant::now() < end {
                if let Ok(SourceEvent::Answer { id: i, result }) =
                    src.events().recv_timeout(Duration::from_millis(500))
                    && i == id
                {
                    return result.map(|_| ()).map_err(|e| e.message);
                }
            }
            Err("no answer in time".into())
        };
        let booted_at = Instant::now();
        let id = src.boot(&t);
        wait(&src, id, 600).expect("booted");
        eprintln!("booted in {:.0} s", booted_at.elapsed().as_secs_f64());
        let id = src.attach(&t);
        wait(&src, id, 60).expect("attached");
        f.show(Some(t.clone()));
        let started = Instant::now();
        let mut last_swipe = Instant::now() - Duration::from_secs(2);
        let mut shown = 0;
        while started.elapsed() < Duration::from_secs(20) {
            while src.events().try_recv().is_ok() {}
            if last_swipe.elapsed() > Duration::from_millis(700) {
                last_swipe = Instant::now();
                for (phase, y) in [
                    (Phase::Start, 700.0),
                    (Phase::Move, 500.0),
                    (Phase::End, 300.0),
                ] {
                    src.touch(&t, Touch::one(phase, 200.0, y));
                }
            }
            if f.frame_image(started.elapsed().as_millis() as u64)
                .is_some()
            {
                shown += 1;
            }
            f.after_render();
            queue.submit([]);
            let _ = device.poll(wgpu::PollType::Poll);
            std::thread::sleep(Duration::from_millis(16));
        }
        let (copied, dropped, p50, p99) = f.stats();
        eprintln!(
            "{} ({:?}): route {:?}, refused {:?}, {copied} copied, {dropped} dropped, {shown} shown in 20 s, copy CPU p50 {p50:.3} ms p99 {p99:.3} ms",
            info.name,
            info.backend,
            f.path(),
            f.refused()
        );
        assert!(copied > 0, "frames came through the region");
        let last = kept.borrow().last().cloned().expect("a ring texture shown");
        let px = flare_render::gpu::read_texture(&device, &queue, &last);
        let lit = px
            .chunks(4)
            .filter(|p| p[0] > 16 || p[1] > 16 || p[2] > 16)
            .count();
        eprintln!("{}x{}, {lit} lit pixels", last.width(), last.height());
        assert!(lit > 1000, "the picture is the device's screen, not black");
        // Android PR A7: a still screen opened again (a hide and a show with no swipe) gives its first frame at once, so
        // the shared memory stays the path: no fallback line in the emu host's log.
        src.pause(&t, true);
        std::thread::sleep(Duration::from_secs(1));
        src.pause(&t, false);
        std::thread::sleep(Duration::from_secs(4));
        let log = std::fs::read_to_string(logs.path().join("emu-host.log")).unwrap_or_default();
        let fell: Vec<&str> = log.lines().filter(|l| l.contains("fall back")).collect();
        eprintln!(
            "emu host: {} streams opened on the shared memory, fallbacks {fell:?}",
            log.matches("frames streaming (the shared memory)").count()
        );
        assert!(
            log.matches("frames streaming (the shared memory)").count() >= 2,
            "{log}"
        );
        assert!(fell.is_empty(), "{fell:?}");
        let region = src.region_path(&t).expect("a region");
        src.detach(&t);
        f.release(&t);
        f.sync();
        let end = Instant::now() + Duration::from_secs(5);
        while region.exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!region.exists(), "the region is deleted after detach");
    }
}
