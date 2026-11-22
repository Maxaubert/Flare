//! DeviceFrames: the device panel's picture with no CPU copy (issue #248, device panel PR 4b, rule 1311; Android plan
//! PR A4, rule 1364).
//!
//! Two kinds of frames reach the window here, both shown by the render notifier in `BeforeRendering`
//! (`frame_image`) and handed back from `Queue::on_submitted_work_done` (`after_render`):
//! - **The iOS simulator's ring** (macOS, `SimRing` below): the sim host blits each simulator frame into one slot of
//!   an RGBA IOSurface ring and hands Flare a frame record (`flare_device::client::SimFrames`). Each slot is wrapped
//!   once as a wgpu texture on wgpu's own `MTLDevice` (`wgpu::hal::metal::Device::texture_from_raw` +
//!   `create_texture_from_hal`, `Rgba8Unorm`, storage mode Shared, `TEXTURE_BINDING | RENDER_ATTACHMENT | COPY_SRC`).
//!   A slot is never rewritten while Flare samples it: the slot shown before is handed back one frame late. When the
//!   import is refused (a renderer with no wgpu device, `Image::try_from` failing), the CPU path takes over for the
//!   session: the slot is locked read-only (`IOSurfaceLock`), copied into a `SharedPixelBuffer` and freed at once, at
//!   most [`CPU_FPS`] a second, and the panel wears the "slow" mark (spec "Fallbacks", 1). The picture stays portrait
//!   (the framebuffer never turns, spike S5): Slint turns it by the interface's orientation (`panel-device-turn`).
//! - **The Android emulator's region** (Windows and macOS, `crate::device_region`): the emulator writes each frame into
//!   a region file Flare maps, and a frame thread copies it into a ring of textures on the GPU at once.
//!
//! The notifier reaches this object through a weak slot set at startup and borrows it with `try_borrow_mut` only; it
//! never borrows the `AppCtl`. The fake source's CPU pictures (`--e2e`) never come here: `DeviceCtl` hands them to
//! Slint itself.

use slint::Image;

use flare_device::source::SourceKind;
use flare_device::target::Target;

use crate::device_region::RegionFrames;

/// The CPU path shows at most this many frames a second (spec "Fallbacks", 1: about 12 MB a frame).
pub const CPU_FPS: u64 = 30;

/// The CPU path's spacing between two copies, in ms.
pub const CPU_SPACING_MS: u64 = 1000 / CPU_FPS;

/// How the shown picture reached the screen (the probe, the slow mark, the log).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FramePath {
    /// No ring frame shown yet.
    #[default]
    None,
    /// Zero copy: the ring slot itself is the image (iOS), or the region was copied on the GPU (Android).
    Gpu,
    /// Android: the region was uploaded by the CPU into the ring (`write_texture`, the zero-copy import refused).
    Upload,
    /// The import was refused: each frame is copied by the CPU (the slow mark).
    Cpu,
}

impl FramePath {
    pub fn name(self) -> &'static str {
        match self {
            FramePath::None => "none",
            FramePath::Gpu => "gpu",
            FramePath::Upload => "upload",
            FramePath::Cpu => "cpu",
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::RingSource;

/// The device panel's frames: the iOS simulator's ring (macOS) and the Android region frames, routed by the kind of
/// the device on show.
#[derive(Default)]
pub struct DeviceFrames {
    #[cfg(target_os = "macos")]
    sim: mac::SimRing,
    region: RegionFrames,
    /// The device on show is an Android one.
    emu: bool,
}

impl DeviceFrames {
    /// The sim host's frame half, or none (the sim host stopped).
    #[cfg(target_os = "macos")]
    pub fn set_source(&mut self, source: Option<std::sync::Arc<dyn RingSource>>) {
        self.sim.set_source(source);
    }

    /// The Android frames' counters: copied, dropped, the copy's CPU time p50 and p99 in ms (`device.state`).
    pub fn region_stats(&self) -> (u64, u64, f64, f64) {
        self.region.stats()
    }

    /// The Android frames (the emu host's sink, a detached device's release).
    pub fn region(&mut self) -> &mut RegionFrames {
        &mut self.region
    }

    pub fn attach_gpu(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        #[cfg(target_os = "macos")]
        self.sim.attach_gpu(device, queue);
        self.region.attach_gpu(device, queue);
    }

    pub fn detach_gpu(&mut self) {
        #[cfg(target_os = "macos")]
        self.sim.detach_gpu();
        self.region.detach_gpu();
    }

    /// The device the window shows now (the front panel's, live), or None.
    pub fn show(&mut self, target: Option<Target>) {
        self.emu = target
            .as_ref()
            .is_some_and(|t| t.source() == SourceKind::Emu);
        let (sim, emu) = if self.emu {
            (None, target)
        } else {
            (target, None)
        };
        #[cfg(target_os = "macos")]
        self.sim.show(sim);
        #[cfg(not(target_os = "macos"))]
        let _ = sim;
        self.region.show(emu);
    }

    pub fn frame_image(&mut self, now_ms: u64) -> Option<Image> {
        if self.emu {
            return self.region.frame_image(now_ms);
        }
        #[cfg(target_os = "macos")]
        return self.sim.frame_image(now_ms);
        #[cfg(not(target_os = "macos"))]
        {
            let _ = now_ms;
            None
        }
    }

    pub fn after_render(&mut self) {
        #[cfg(target_os = "macos")]
        self.sim.after_render();
        self.region.after_render();
    }

    pub fn wants_redraw(&self) -> bool {
        #[cfg(target_os = "macos")]
        if self.sim.wants_redraw() {
            return true;
        }
        self.region.wants_redraw()
    }

    pub fn path(&self) -> FramePath {
        if self.emu {
            return self.region.path();
        }
        #[cfg(target_os = "macos")]
        return self.sim.path();
        #[cfg(not(target_os = "macos"))]
        FramePath::None
    }

    pub fn showing(&self, target: &Target) -> bool {
        if target.source() == SourceKind::Emu {
            return self.region.showing(target);
        }
        #[cfg(target_os = "macos")]
        return self.sim.showing(target);
        #[cfg(not(target_os = "macos"))]
        false
    }

    pub fn frames(&self) -> u64 {
        #[cfg(target_os = "macos")]
        return self.sim.frames() + self.region.frames();
        #[cfg(not(target_os = "macos"))]
        self.region.frames()
    }

    pub fn held(&self) -> usize {
        #[cfg(target_os = "macos")]
        return self.sim.held() + self.region.held();
        #[cfg(not(target_os = "macos"))]
        self.region.held()
    }

    /// Why the shown kind's import was refused (the slow mark's or the upload's reason), if it was.
    pub fn refused(&self) -> Option<String> {
        if self.emu {
            return self.region.refused();
        }
        #[cfg(target_os = "macos")]
        return self.sim.refused().map(str::to_string);
        #[cfg(not(target_os = "macos"))]
        None
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex, MutexGuard};

    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_core_foundation::CFRetained;
    use objc2_io_surface::IOSurfaceRef;
    use objc2_metal::{
        MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor,
        MTLTextureType, MTLTextureUsage,
    };
    use slint::{Rgba8Pixel, SharedPixelBuffer};

    use flare_device::client::SimFrames;
    use flare_device::macos::ring::copy_rgba;
    use flare_device::wire::FrameRecord;

    use super::*;

    /// Where the frames come from: the sim host's frame half, or a test's own ring.
    pub trait RingSource: Send + Sync {
        /// The newest frame of `target`, taken: its ring number and record. Its slot is Flare's until `slot_done`.
        fn take_frame(&self, target: &Target) -> Option<(u32, FrameRecord)>;
        /// The surfaces of ring `device`, in slot order.
        fn surfaces(&self, device: u32) -> Option<Vec<CFRetained<IOSurfaceRef>>>;
        /// The slot is free again (never blocks).
        fn slot_done(&self, device: u32, slot: u32);
    }

    impl RingSource for SimFrames {
        fn take_frame(&self, target: &Target) -> Option<(u32, FrameRecord)> {
            SimFrames::take_frame(self, target)
        }

        fn surfaces(&self, device: u32) -> Option<Vec<CFRetained<IOSurfaceRef>>> {
            self.ring(device)
        }

        fn slot_done(&self, device: u32, slot: u32) {
            SimFrames::slot_done(self, device, slot);
        }
    }

    /// How a ring texture becomes a Slint image: `Image::try_from` (tests refuse it to reach the CPU path).
    pub type Import = Box<dyn Fn(wgpu::Texture) -> Result<Image, String>>;

    fn slint_import() -> Import {
        Box::new(|t| Image::try_from(t).map_err(|e| format!("{e:?}")))
    }

    struct Gpu {
        device: wgpu::Device,
        queue: wgpu::Queue,
    }

    /// One ring: its surfaces and, once wrapped on the current wgpu device, its textures.
    struct Ring {
        surfaces: Vec<CFRetained<IOSurfaceRef>>,
        textures: Option<Vec<wgpu::Texture>>,
    }

    /// The slot on show: Flare holds it until the next one replaces it and the GPU is done with it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Shown {
        target: Target,
        device: u32,
        slot: u32,
    }

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub struct SimRing {
        source: Option<Arc<dyn RingSource>>,
        gpu: Option<Gpu>,
        import: Import,
        /// The device whose frames the window shows (the front panel's, live), or None.
        want: Option<Target>,
        rings: HashMap<u32, Ring>,
        shown: Option<Shown>,
        /// Slots replaced since the last `after_render`: freed once the GPU work that sampled them is done.
        retiring: Vec<(u32, u32)>,
        /// Every slot Flare holds (shown, retiring, or waiting for its work-done callback), shared with the callbacks.
        held: Arc<Mutex<HashSet<(u32, u32)>>>,
        /// The import was refused: why (the CPU path for the session).
        refused: Option<String>,
        /// The CPU path: when the last copy was made, and a frame taken too soon, kept for the next render.
        cpu_last_ms: Option<u64>,
        cpu_waiting: Option<(u32, u32)>,
        path: FramePath,
        frames: u64,
    }

    impl Default for SimRing {
        fn default() -> Self {
            Self::new()
        }
    }

    impl SimRing {
        pub fn new() -> Self {
            Self {
                source: None,
                gpu: None,
                import: slint_import(),
                want: None,
                rings: HashMap::new(),
                shown: None,
                retiring: Vec::new(),
                held: Arc::default(),
                refused: None,
                cpu_last_ms: None,
                cpu_waiting: None,
                path: FramePath::None,
                frames: 0,
            }
        }

        /// The frames' source (the sim host's frame half), or none (the sim host stopped). What the old one held is
        /// let go: its slots belong to a ring that is gone with it.
        pub fn set_source(&mut self, source: Option<Arc<dyn RingSource>>) {
            self.source = source;
            self.rings.clear();
            self.shown = None;
            self.retiring.clear();
            self.cpu_waiting = None;
            lock(&self.held).clear();
        }

        /// How a texture becomes an image (tests).
        #[cfg(test)]
        pub fn set_import(&mut self, import: Import) {
            self.import = import;
        }

        /// The renderer's wgpu device (`RenderingSetup`): the rings are wrapped again on it at their next frame.
        pub fn attach_gpu(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
            self.gpu = Some(Gpu {
                device: device.clone(),
                queue: queue.clone(),
            });
            for r in self.rings.values_mut() {
                r.textures = None;
            }
        }

        /// The renderer is torn down (`RenderingTeardown`): the textures go, and with no GPU left to sample them the
        /// slots Flare holds are free at once.
        pub fn detach_gpu(&mut self) {
            self.gpu = None;
            for r in self.rings.values_mut() {
                r.textures = None;
            }
            if let Some(s) = self.shown.take() {
                self.retiring.push((s.device, s.slot));
            }
            self.free_retiring_now();
        }

        /// The device the window shows now (the front panel's, live), or None. A slot shown for another device is
        /// handed back after the next frame is drawn without it.
        pub fn show(&mut self, target: Option<Target>) {
            if self.want == target {
                return;
            }
            self.want = target;
            if let Some(s) = self.shown.take() {
                self.retiring.push((s.device, s.slot));
            }
            if let Some((d, s)) = self.cpu_waiting.take() {
                self.free_now(d, s);
            }
        }

        /// `BeforeRendering`: the newest frame of the shown device as a Slint image, or None when there is no new one
        /// (the image on show stays). The slot shown before is retired (freed after this frame's GPU work).
        pub fn frame_image(&mut self, now_ms: u64) -> Option<Image> {
            let source = self.source.clone()?;
            let target = self.want.clone()?;
            // A frame the CPU path kept for later goes first, unless a newer one came.
            let (device, slot) = match source.take_frame(&target) {
                Some((device, rec)) => {
                    if let Some((d, s)) = self.cpu_waiting.take() {
                        self.free_now(d, s);
                    }
                    (device, rec.slot)
                }
                None => self.cpu_waiting.take()?,
            };
            lock(&self.held).insert((device, slot));
            // A ring this source no longer streams is let go (a new ring after a new screen size).
            let shown_device = self.shown.as_ref().map(|s| s.device);
            self.rings
                .retain(|d, _| *d == device || Some(*d) == shown_device);
            if let std::collections::hash_map::Entry::Vacant(e) = self.rings.entry(device) {
                let Some(surfaces) = source.surfaces(device) else {
                    // The record came before its ring (cannot happen on one mach channel): skip it.
                    log::debug!("device frames: a frame of ring {device} before the ring");
                    self.free_now(device, slot);
                    return None;
                };
                e.insert(Ring {
                    surfaces,
                    textures: None,
                });
            }
            if self.rings[&device].surfaces.len() <= slot as usize {
                self.free_now(device, slot);
                return None;
            }
            if self.refused.is_none()
                && let Some(img) = self.gpu_image(device, slot)
            {
                self.hold(target, device, slot);
                self.path = FramePath::Gpu;
                self.frames += 1;
                return Some(img);
            }
            self.cpu_image(device, slot, now_ms)
        }

        /// The zero-copy image of `slot`, or None (the import refused: `refused` says why from then on).
        fn gpu_image(&mut self, device: u32, slot: u32) -> Option<Image> {
            // No wgpu device (yet, or a software renderer): this frame is copied, the next may be imported.
            let gpu = self.gpu.as_ref()?;
            let ring = self.rings.get_mut(&device)?;
            if ring.textures.is_none() {
                match wrap(gpu, &ring.surfaces) {
                    Ok(t) => ring.textures = Some(t),
                    Err(e) => {
                        self.refuse(e);
                        return None;
                    }
                }
            }
            let texture = ring.textures.as_ref()?.get(slot as usize)?.clone();
            match (self.import)(texture) {
                Ok(img) => Some(img),
                Err(e) => {
                    self.refuse(format!("Slint refused the texture: {e}"));
                    None
                }
            }
        }

        fn refuse(&mut self, why: String) {
            if self.refused.is_none() {
                log::warn!("device panel: the frames go through the CPU: {why}");
                self.refused = Some(why);
            }
        }

        /// The CPU path: the slot copied (at most `CPU_FPS` a second) and freed at once.
        fn cpu_image(&mut self, device: u32, slot: u32, now_ms: u64) -> Option<Image> {
            if let Some(last) = self.cpu_last_ms
                && now_ms.saturating_sub(last) < CPU_SPACING_MS
            {
                // Too soon: kept for a later render (`wants_redraw`), replaced by a newer one meanwhile.
                self.cpu_waiting = Some((device, slot));
                return None;
            }
            let surface = self
                .rings
                .get(&device)?
                .surfaces
                .get(slot as usize)?
                .clone();
            let (w, h) = (surface.width(), surface.height());
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
            let ok = copy_rgba(&surface, buf.make_mut_bytes());
            self.free_now(device, slot);
            // A slot the GPU path showed before is retired as usual.
            if let Some(s) = self.shown.take() {
                self.retiring.push((s.device, s.slot));
            }
            if !ok {
                return None;
            }
            self.cpu_last_ms = Some(now_ms);
            self.path = FramePath::Cpu;
            self.frames += 1;
            Some(Image::from_rgba8(buf))
        }

        fn hold(&mut self, target: Target, device: u32, slot: u32) {
            let next = Shown {
                target,
                device,
                slot,
            };
            if let Some(old) = self.shown.replace(next)
                && (old.device, old.slot) != (device, slot)
            {
                self.retiring.push((old.device, old.slot));
            }
        }

        fn free_now(&self, device: u32, slot: u32) {
            lock(&self.held).remove(&(device, slot));
            if let Some(s) = &self.source {
                s.slot_done(device, slot);
            }
        }

        fn free_retiring_now(&mut self) {
            for (d, s) in std::mem::take(&mut self.retiring) {
                self.free_now(d, s);
            }
        }

        /// `AfterRendering`: the frame that no longer samples the retired slots is submitted; each is freed from the
        /// queue's work-done callback (it runs at a later submit or poll, so a slot comes back one frame late).
        pub fn after_render(&mut self) {
            if self.retiring.is_empty() {
                return;
            }
            let (Some(gpu), Some(source)) = (&self.gpu, self.source.clone()) else {
                self.free_retiring_now();
                return;
            };
            for (d, s) in std::mem::take(&mut self.retiring) {
                let source = Arc::clone(&source);
                let held = Arc::clone(&self.held);
                // untimed: the callback only frees a slot (a lock and a non-blocking mach send)
                gpu.queue.on_submitted_work_done(move || {
                    lock(&held).remove(&(d, s));
                    source.slot_done(d, s);
                });
            }
        }

        /// The CPU path kept a frame for a later render: the notifier asks for one.
        pub fn wants_redraw(&self) -> bool {
            self.cpu_waiting.is_some()
        }

        pub fn path(&self) -> FramePath {
            self.path
        }

        /// A ring frame of `target` is on show.
        pub fn showing(&self, target: &Target) -> bool {
            self.shown.as_ref().is_some_and(|s| &s.target == target)
                || (self.path == FramePath::Cpu && self.want.as_ref() == Some(target))
        }

        /// Ring frames shown so far.
        pub fn frames(&self) -> u64 {
            self.frames
        }

        /// Slots Flare holds now.
        pub fn held(&self) -> usize {
            lock(&self.held).len()
        }

        /// Why the import was refused (the slow mark's reason), if it was.
        pub fn refused(&self) -> Option<&str> {
            self.refused.as_deref()
        }
    }

    /// Wraps each surface as an `MTLTexture` on wgpu's own Metal device, then as a wgpu texture (spike S2's path,
    /// with the corrections the spikes took: storage mode Shared, `ShaderRead | RenderTarget`, and `COPY_SRC`).
    fn wrap(
        gpu: &Gpu,
        surfaces: &[CFRetained<IOSurfaceRef>],
    ) -> Result<Vec<wgpu::Texture>, String> {
        let mut hal = Vec::with_capacity(surfaces.len());
        {
            // SAFETY: the hal device is only read (its raw MTLDevice) while the guard lives.
            let dev = unsafe { gpu.device.as_hal::<wgpu::hal::api::Metal>() }
                .ok_or("the renderer's device is not Metal")?;
            let raw: &Retained<ProtocolObject<dyn MTLDevice>> = dev.raw_device();
            for s in surfaces {
                let (w, h) = (s.width(), s.height());
                if w == 0 || h == 0 {
                    return Err("an empty ring surface".into());
                }
                // SAFETY: a plain 2D descriptor of a positive size.
                let desc = unsafe {
                    MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                        MTLPixelFormat::RGBA8Unorm,
                        w,
                        h,
                        false,
                    )
                };
                desc.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
                desc.setStorageMode(MTLStorageMode::Shared);
                let tex: Retained<ProtocolObject<dyn MTLTexture>> = raw
                    .newTextureWithDescriptor_iosurface_plane(&desc, s, 0)
                    .ok_or("Metal refused the ring surface")?;
                // SAFETY: the texture was made on this wgpu device's own MTLDevice with the format, type and size
                // given here (spike S2).
                let t = unsafe {
                    wgpu::hal::metal::Device::texture_from_raw(
                        tex,
                        wgpu::TextureFormat::Rgba8Unorm,
                        MTLTextureType::Type2D,
                        1,
                        1,
                        wgpu::hal::CopyExtent {
                            width: w as u32,
                            height: h as u32,
                            depth: 1,
                        },
                        None,
                    )
                };
                hal.push((t, w as u32, h as u32));
            }
        }
        Ok(hal
            .into_iter()
            .map(|(t, width, height)| {
                // SAFETY: the hal texture belongs to this device (above); the descriptor matches it.
                unsafe {
                    gpu.device.create_texture_from_hal::<wgpu::hal::api::Metal>(
                        t,
                        &wgpu::TextureDescriptor {
                            label: Some("device ring"),
                            size: wgpu::Extent3d {
                                width,
                                height,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING
                                | wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::COPY_SRC,
                            view_formats: &[],
                        },
                        wgpu::TextureUses::UNINITIALIZED,
                    )
                }
            })
            .collect())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use flare_device::macos::ring::{fill_surface, rgba_surface};

        /// A ring of three test surfaces, its frames queued by the test, its slot-done records counted.
        #[derive(Default)]
        struct TestRing {
            surfaces: Vec<CFRetained<IOSurfaceRef>>,
            next: Mutex<Vec<FrameRecord>>,
            done: Mutex<Vec<(u32, u32)>>,
        }

        // SAFETY: IOSurfaces are refcounted kernel objects usable from any thread; the rest is behind locks.
        unsafe impl Send for TestRing {}
        // SAFETY: as above.
        unsafe impl Sync for TestRing {}

        impl TestRing {
            fn new(w: usize, h: usize, colours: [[u8; 4]; 3]) -> Arc<TestRing> {
                let surfaces = colours
                    .iter()
                    .map(|c| {
                        let s = rgba_surface(w, h).expect("an IOSurface");
                        fill_surface(&s, *c);
                        s
                    })
                    .collect();
                Arc::new(TestRing {
                    surfaces,
                    ..TestRing::default()
                })
            }

            fn frame(&self, slot: u32, seq: u64) {
                lock(&self.next).push(FrameRecord {
                    device: 7,
                    slot,
                    seq,
                    t_us: seq * 16_000,
                });
            }

            fn done(&self) -> Vec<(u32, u32)> {
                lock(&self.done).clone()
            }
        }

        impl RingSource for TestRing {
            fn take_frame(&self, _target: &Target) -> Option<(u32, FrameRecord)> {
                lock(&self.next).pop().map(|r| (r.device, r))
            }

            fn surfaces(&self, device: u32) -> Option<Vec<CFRetained<IOSurfaceRef>>> {
                (device == 7).then(|| self.surfaces.clone())
            }

            fn slot_done(&self, device: u32, slot: u32) {
                lock(&self.done).push((device, slot));
            }
        }

        fn target() -> Target {
            Target::sim("00000000-0000-4000-8000-0000000000aa").expect("a udid")
        }

        fn frames_on(ring: &Arc<TestRing>) -> SimRing {
            let mut f = SimRing::new();
            f.set_source(Some(Arc::clone(ring) as Arc<dyn RingSource>));
            f.show(Some(target()));
            f
        }

        /// Reads a texture back through the GPU: its first pixel.
        fn first_pixel(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> [u8; 4] {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut enc = device.create_command_encoder(&Default::default());
            enc.copy_texture_to_buffer(
                tex.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([enc.finish()]);
            buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("the copy finishes");
            let data = buf.slice(..).get_mapped_range().expect("mapped");
            [data[0], data[1], data[2], data[3]]
        }

        #[test]
        fn an_iosurface_imports_as_a_slint_image_and_reads_back() {
            let Ok((device, queue, _)) = flare_render::gpu::headless_device() else {
                eprintln!("skipping: no wgpu adapter");
                return;
            };
            let ring = TestRing::new(
                64,
                128,
                [[200, 10, 20, 255], [10, 200, 20, 255], [10, 20, 200, 255]],
            );
            let mut f = frames_on(&ring);
            f.attach_gpu(&device, &queue);
            ring.frame(1, 1);
            let img = f.frame_image(0).expect("the slot as a Slint image");
            assert_eq!((img.size().width, img.size().height), (64, 128));
            assert_eq!(f.path(), FramePath::Gpu, "zero copy, no slow mark");
            assert!(f.showing(&target()));
            // The texture is the surface itself: the GPU reads the colour the surface was filled with.
            let tex = f.rings[&7].textures.as_ref().expect("wrapped")[1].clone();
            assert_eq!(first_pixel(&device, &queue, &tex), [10, 200, 20, 255]);
            // Nothing was copied: the slot is still Flare's.
            assert!(ring.done().is_empty());
            assert_eq!(f.held(), 1);
        }

        #[test]
        fn a_slot_is_freed_only_from_the_work_done_callback() {
            let Ok((device, queue, _)) = flare_render::gpu::headless_device() else {
                eprintln!("skipping: no wgpu adapter");
                return;
            };
            let ring = TestRing::new(16, 16, [[1, 2, 3, 255]; 3]);
            let mut f = frames_on(&ring);
            f.attach_gpu(&device, &queue);
            ring.frame(0, 1);
            assert!(f.frame_image(0).is_some());
            f.after_render();
            ring.frame(1, 2);
            assert!(f.frame_image(16).is_some(), "the next slot replaces it");
            // Replaced, but the frame that sampled slot 0 may still run: not freed in BeforeRendering...
            assert!(
                ring.done().is_empty(),
                "never freed before the frame is submitted"
            );
            assert_eq!(f.held(), 2);
            f.after_render();
            // ...nor when the callback is registered: only once the GPU says the work is done.
            assert!(ring.done().is_empty(), "registered, not yet run");
            queue.submit([]);
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("the queue drains");
            assert_eq!(
                ring.done(),
                vec![(7, 0)],
                "slot 0 freed by the work-done callback"
            );
            assert_eq!(f.held(), 1, "slot 1 is still on show");
            // Closing the panel retires the shown slot the same way.
            f.show(None);
            f.after_render();
            assert_eq!(ring.done(), vec![(7, 0)]);
            queue.submit([]);
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("the queue drains");
            assert_eq!(ring.done(), vec![(7, 0), (7, 1)]);
            assert_eq!(f.held(), 0);
        }

        #[test]
        fn a_refused_import_falls_back_to_the_cpu_path() {
            let Ok((device, queue, _)) = flare_render::gpu::headless_device() else {
                eprintln!("skipping: no wgpu adapter");
                return;
            };
            let ring = TestRing::new(8, 4, [[9, 8, 7, 255], [40, 50, 60, 255], [1, 1, 1, 255]]);
            let mut f = frames_on(&ring);
            f.attach_gpu(&device, &queue);
            f.set_import(Box::new(|_| Err("TextureImportError".into())));
            ring.frame(1, 1);
            let img = f.frame_image(0).expect("a CPU copy instead");
            assert_eq!(f.path(), FramePath::Cpu, "the slow mark's path");
            assert!(
                f.refused()
                    .is_some_and(|w| w.contains("TextureImportError"))
            );
            let px = img.to_rgba8().expect("a pixel buffer");
            assert_eq!((px.width(), px.height()), (8, 4));
            assert_eq!(px.as_bytes()[..4], [40, 50, 60, 255]);
            // Copied, so freed at once: nothing waits for the GPU.
            assert_eq!(ring.done(), vec![(7, 1)]);
            assert_eq!(f.held(), 0);
            // At most 30 a second: a frame 10 ms later waits for a later render, a newer one replaces it.
            ring.frame(2, 2);
            assert!(f.frame_image(10).is_none());
            assert!(f.wants_redraw());
            ring.frame(0, 3);
            assert!(f.frame_image(20).is_none());
            assert_eq!(
                ring.done(),
                vec![(7, 1), (7, 2)],
                "the one replaced is freed"
            );
            let img = f
                .frame_image(40)
                .expect("the kept frame, once 33 ms passed");
            assert_eq!(
                img.to_rgba8().expect("pixels").as_bytes()[..4],
                [9, 8, 7, 255]
            );
            assert!(!f.wants_redraw());
            assert_eq!(f.held(), 0);
            // A renderer with no wgpu device takes the same path.
            let mut g = frames_on(&ring);
            ring.frame(1, 4);
            assert!(g.frame_image(0).is_some());
            assert_eq!(g.path(), FramePath::Cpu);
        }

        /// The real path end to end (opt-in, `FLARE_DEVICE_REAL=1`, the sim host built first with
        /// `cargo build -p flare-device --bin flare-sim-host`): a simulator made for the test (deleted after) boots
        /// headless, `SimSource` attaches, and its ring frames import on this process's own wgpu device as Slint images
        /// with no CPU copy, slots coming back through the work-done callback.
        #[test]
        fn a_real_simulator_ring_imports_with_no_copy() {
            use flare_device::client::{SimConfig, SimSource};
            use flare_device::macos::simctl::Simctl;
            use flare_device::source::{DeviceSource, SourceEvent};
            use std::time::{Duration, Instant};
            if std::env::var("FLARE_DEVICE_REAL").as_deref() != Ok("1") {
                eprintln!(
                    "skipped: set FLARE_DEVICE_REAL=1 to run against a simulator made for the test"
                );
                return;
            }
            let Some(xcode) = flare_device::xcode::find() else {
                eprintln!("skipped: no Xcode");
                return;
            };
            let exe = std::env::current_exe()
                .unwrap()
                .parent()
                .and_then(|d| d.parent())
                .map(|d| d.join("flare-sim-host"))
                .unwrap();
            assert!(exe.exists(), "build the sim host first: {}", exe.display());
            let (device, queue, _) = flare_render::gpu::headless_device().expect("a wgpu device");
            let dev = xcode.developer_dir();
            let simctl = Simctl::new(&dev);
            let ty = simctl.iphone_type().expect("an iPhone type");
            let target = simctl
                .create("Flare Frames Test", &ty)
                .expect("simctl create");
            struct Delete<'a>(&'a Simctl, Target);
            impl Drop for Delete<'_> {
                fn drop(&mut self) {
                    let _ = self.0.shutdown(&self.1);
                    let _ = self.0.delete(&self.1);
                }
            }
            let _delete = Delete(&simctl, target.clone());
            simctl.boot(&target).expect("boot");
            flare_proc::guard::use_test_entry(
                "appctl::tests::mods_tests::plugin_install::guard_entry",
            )
            .unwrap();
            let logs = tempfile::tempdir().unwrap();
            let mut src =
                SimSource::start(SimConfig::new(exe, dev, logs.path().join("sim-host.log")));
            let mut f = SimRing::new();
            f.attach_gpu(&device, &queue);
            f.set_source(Some(Arc::new(src.frames()) as Arc<dyn RingSource>));
            f.show(Some(target.clone()));
            src.attach(&target);
            let end = Instant::now() + Duration::from_secs(90);
            let mut shown = 0;
            let mut size = None;
            while Instant::now() < end && shown < 5 {
                // The home screen is still, so a tap-free wait may see few frames: any frame will do.
                let ev = src.events().recv_timeout(Duration::from_millis(200)); // sync-ok: test code only
                if let Ok(SourceEvent::Frame { .. }) = ev
                    && let Some(img) = f.frame_image(shown * 20)
                {
                    size = Some(img.size());
                    shown += 1;
                    f.after_render();
                    queue.submit([]);
                    let _ = device.poll(wgpu::PollType::Poll);
                }
            }
            eprintln!(
                "ring frames shown: {shown}, size {size:?}, path {:?}, refused {:?}",
                f.path(),
                f.refused()
            );
            assert!(shown >= 1, "a ring frame imported");
            assert_eq!(f.path(), FramePath::Gpu, "no CPU copy");
            let s = size.unwrap();
            assert!(s.width > 0 && s.height > s.width, "portrait: {s:?}");
            assert!(f.held() <= 2, "slots come back: {} held", f.held());
        }

        /// Open check 7: a full iPhone 18 Pro frame (1206 x 2622, about 12.6 MB) copies well inside the CPU path's
        /// 33 ms budget. Measured, not asserted tightly: a loaded CI machine is slower.
        #[test]
        fn a_full_frame_copies_inside_the_cpu_budget() {
            let s = rgba_surface(1206, 2622).expect("an IOSurface");
            fill_surface(&s, [1, 2, 3, 255]);
            let mut out = vec![0u8; 1206 * 2622 * 4];
            let start = std::time::Instant::now();
            let n = 10;
            for _ in 0..n {
                assert!(copy_rgba(&s, &mut out));
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
            eprintln!("IOSurfaceLock + copy of 1206x2622: {ms:.2} ms a frame");
            assert_eq!(out[..4], [1, 2, 3, 255]);
            assert!(ms < 100.0, "{ms} ms a frame");
        }
    }
}
