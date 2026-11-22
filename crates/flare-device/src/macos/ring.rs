//! The frame ring in the sim host (rule 1311): [`RING_SLOTS`] RGBA IOSurfaces, each wrapped as an `MTLTexture`
//! (storage Shared, `ShaderRead | RenderTarget`, spike S2), filled by one small render pass per frame that reads the
//! simulator's BGRA framebuffer, forces A to 255 and converts Display P3 to sRGB in linear light (spike S6), so the
//! colours match Simulator.app.
//!
//! [`RingSet`] keeps the slots' ownership (`cap::Ring`) per ring. A new ring (a different screen size) gets a new
//! `device` number, so a slot-done record always names its ring; an old ring is freed only once Flare has said every
//! one of its slots is done.

use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CFRetained;
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use objc2_io_surface::{IOSurfaceLockOptions, IOSurfaceRef};
use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder,
    MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLStorageMode,
    MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

pub use crate::cap::RING_SLOTS;
use crate::cap::Ring as Slots;

/// One ring and its slots' ownership.
#[derive(Debug)]
pub struct Owned<R> {
    /// The number its records carry (fresh per ring).
    pub device: u32,
    pub ring: u32,
    pub payload: R,
    pub slots: Slots,
}

/// The current ring of a device and the old ones still held by Flare.
#[derive(Debug)]
pub struct RingSet<R> {
    current: Option<Owned<R>>,
    old: Vec<Owned<R>>,
    /// Frames dropped for want of a free slot.
    dropped: u64,
}

impl<R> Default for RingSet<R> {
    fn default() -> Self {
        Self {
            current: None,
            old: Vec::new(),
            dropped: 0,
        }
    }
}

impl<R> RingSet<R> {
    /// A new ring takes over. The one it replaces is kept until Flare is done with each of its slots; it is returned
    /// at once (to be freed) when Flare holds none of them.
    pub fn replace(&mut self, device: u32, ring: u32, payload: R) -> Vec<R> {
        let new = Owned {
            device,
            ring,
            payload,
            slots: Slots::default(),
        };
        if let Some(prev) = self.current.replace(new) {
            self.old.push(prev);
        }
        self.sweep()
    }

    pub fn current(&self) -> Option<&Owned<R>> {
        self.current.as_ref()
    }

    /// A free slot of the current ring for the next frame (now Sent), with the ring. None drops the frame (counted).
    pub fn take_free(&mut self) -> Option<(u8, &Owned<R>)> {
        let cur = self.current.as_mut()?;
        let Some(slot) = cur.slots.take_free() else {
            self.dropped += 1;
            return None;
        };
        Some((slot, cur))
    }

    /// Flare is done with `slot` of the ring numbered `device`: true when it was one Flare held. Returns the old rings
    /// now wholly free (to be freed).
    pub fn done(&mut self, device: u32, slot: u8) -> (bool, Vec<R>) {
        let mut ok = false;
        if let Some(c) = self.current.as_mut().filter(|c| c.device == device) {
            ok = c.slots.done(slot);
        }
        for o in self.old.iter_mut().filter(|o| o.device == device) {
            ok |= o.slots.done(slot);
        }
        (ok, self.sweep())
    }

    /// Rings still waiting for Flare (old ones not wholly done).
    pub fn old_count(&self) -> usize {
        self.old.len()
    }

    /// Frames dropped for want of a free slot, over every ring.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Every ring, current and old (detach frees them all; Flare's copies stay valid, the surfaces are refcounted).
    pub fn take_all(&mut self) -> Vec<R> {
        self.current
            .take()
            .into_iter()
            .chain(self.old.drain(..))
            .map(|o| o.payload)
            .collect()
    }

    fn sweep(&mut self) -> Vec<R> {
        let (free, keep): (Vec<_>, Vec<_>) = self.old.drain(..).partition(|o| o.slots.all_done());
        self.old = keep;
        free.into_iter().map(|o| o.payload).collect()
    }
}

/// A texture wrapping one of the ring's surfaces.
pub type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

/// One ring on the GPU: its surfaces (handed to Flare) and the textures that wrap them (written by the pass).
pub struct GpuRing {
    pub width: usize,
    pub height: usize,
    pub surfaces: Vec<CFRetained<IOSurfaceRef>>,
    pub textures: Vec<Texture>,
}

impl std::fmt::Debug for GpuRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GpuRing({}x{}, {} slots)",
            self.width,
            self.height,
            self.surfaces.len()
        )
    }
}

// SAFETY: IOSurfaces are refcounted kernel objects usable from any thread, and MTLTexture is Send + Sync; the ring
// is only written by its device's serial queue.
unsafe impl Send for GpuRing {}
// SAFETY: as above.
unsafe impl Sync for GpuRing {}

/// The pass that fills a slot: a full-screen triangle that reads the framebuffer pixel for pixel.
const SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct V { float4 pos [[position]]; };

vertex V vs(uint vid [[vertex_id]]) {
    float2 p = float2((vid << 1) & 2, vid & 2);
    V v;
    v.pos = float4(p * 2.0 - 1.0, 0.0, 1.0);
    return v;
}

static float3 to_linear(float3 c) {
    return select(pow((c + 0.055) / 1.055, 2.4), c / 12.92, c <= 0.04045);
}

static float3 to_srgb(float3 c) {
    return select(1.055 * pow(c, 1.0 / 2.4) - 0.055, c * 12.92, c <= 0.0031308);
}

// Display P3 (D65) to sRGB, in linear light.
constant float3x3 P3_TO_SRGB = float3x3(
    float3( 1.2249401, -0.0420569, -0.0196376),
    float3(-0.2249404,  1.0420571, -0.0786361),
    float3( 0.0,        0.0,        1.0982735));

fragment float4 fs(V in [[stage_in]], texture2d<float, access::read> src [[texture(0)]]) {
    float3 c = src.read(uint2(in.pos.xy)).rgb;
    float3 s = to_srgb(clamp(P3_TO_SRGB * to_linear(c), 0.0, 1.0));
    return float4(s, 1.0);
}

// The snapshot pass: each target pixel is the k x k block of framebuffer pixels under it, averaged in linear light
// (the framebuffer at point size, spec "device_look").
fragment float4 fs_box(V in [[stage_in]], texture2d<float, access::read> src [[texture(0)]],
                       constant uint &k [[buffer(0)]]) {
    uint2 base = uint2(in.pos.xy) * k;
    uint2 last = uint2(src.get_width() - 1, src.get_height() - 1);
    float3 acc = float3(0.0);
    for (uint j = 0; j < k; j++) {
        for (uint i = 0; i < k; i++) {
            acc += to_linear(src.read(min(base + uint2(i, j), last)).rgb);
        }
    }
    float3 s = to_srgb(clamp(P3_TO_SRGB * (acc / float(k * k)), 0.0, 1.0));
    return float4(s, 1.0);
}
"#;

/// Display P3 to sRGB for one colour (0..1, gamma-encoded both sides), as the pass does it: the tests' reference.
pub fn p3_to_srgb(c: [f32; 3]) -> [f32; 3] {
    let lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let enc = |v: f32| {
        if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    };
    let [r, g, b] = c.map(lin);
    let m = [
        [1.224_940_1, -0.224_940_4, 0.0],
        [-0.042_056_9, 1.042_057_1, 0.0],
        [-0.019_637_6, -0.078_636_1, 1.098_273_5],
    ];
    let out = m.map(|row| (row[0] * r + row[1] * g + row[2] * b).clamp(0.0, 1.0));
    out.map(enc)
}

/// The sim host's GPU: one Metal device, a queue and the pass.
pub struct Gpu {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// The snapshot's box pass.
    shrink: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

impl Gpu {
    pub fn new() -> Result<Gpu, String> {
        let device = MTLCreateSystemDefaultDevice().ok_or("no Metal device")?;
        let queue = device.newCommandQueue().ok_or("no Metal command queue")?;
        let lib = device
            .newLibraryWithSource_options_error(&NSString::from_str(SHADER), None)
            .map_err(|e| format!("the ring pass did not compile: {e}"))?;
        let vs = lib
            .newFunctionWithName(&NSString::from_str("vs"))
            .ok_or("no vertex function")?;
        let fs = lib
            .newFunctionWithName(&NSString::from_str("fs"))
            .ok_or("no fragment function")?;
        let fs_box = lib
            .newFunctionWithName(&NSString::from_str("fs_box"))
            .ok_or("no snapshot function")?;
        let make = |fs: &ProtocolObject<dyn objc2_metal::MTLFunction>| {
            let desc = MTLRenderPipelineDescriptor::new();
            desc.setVertexFunction(Some(&vs));
            desc.setFragmentFunction(Some(fs));
            // SAFETY: index 0 is inside the attachments array (it has eight).
            unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) }
                .setPixelFormat(MTLPixelFormat::RGBA8Unorm);
            device
                .newRenderPipelineStateWithDescriptor_error(&desc)
                .map_err(|e| format!("the ring pass has no pipeline: {e}"))
        };
        let pipeline = make(&fs)?;
        let shrink = make(&fs_box)?;
        Ok(Gpu {
            device,
            queue,
            pipeline,
            shrink,
        })
    }

    /// A texture over `surface` (its plane 0), as `format`.
    fn wrap(
        &self,
        surface: &IOSurfaceRef,
        format: MTLPixelFormat,
        usage: MTLTextureUsage,
    ) -> Option<Texture> {
        // SAFETY: a 2D descriptor of the surface's own size; nothing is passed that the call keeps.
        let desc = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                format,
                surface.width(),
                surface.height(),
                false,
            )
        };
        desc.setStorageMode(MTLStorageMode::Shared);
        desc.setUsage(usage);
        self.device
            .newTextureWithDescriptor_iosurface_plane(&desc, surface, 0)
    }

    /// A ring of [`RING_SLOTS`] RGBA surfaces of `width` x `height`, wrapped for the pass.
    pub fn ring(&self, width: usize, height: usize) -> Result<GpuRing, String> {
        let mut surfaces = Vec::with_capacity(RING_SLOTS);
        let mut textures = Vec::with_capacity(RING_SLOTS);
        for _ in 0..RING_SLOTS {
            let s = rgba_surface(width, height).ok_or("IOSurfaceCreate failed")?;
            let t = self
                .wrap(
                    &s,
                    MTLPixelFormat::RGBA8Unorm,
                    MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget,
                )
                .ok_or("the ring surface did not wrap as a texture")?;
            surfaces.push(s);
            textures.push(t);
        }
        Ok(GpuRing {
            width,
            height,
            surfaces,
            textures,
        })
    }

    /// Fills `dst` from the framebuffer `src` (BGRA, Display P3) and waits for the GPU: the GPU time in ms. BLOCKING
    /// (under 2 ms): the device's serial queue.
    pub fn fill(&self, src: &IOSurfaceRef, dst: &Texture) -> Result<f64, String> {
        self.pass(src, dst, None)
    }

    /// Fills the RGBA surface `dst` with the framebuffer `src` made `k` times smaller (each pixel the average of a
    /// `k` x `k` block, in linear light), converted to sRGB. BLOCKING (a few ms): the snapshot worker.
    pub fn shrink(&self, src: &IOSurfaceRef, dst: &IOSurfaceRef, k: u32) -> Result<f64, String> {
        let t = self
            .wrap(
                dst,
                MTLPixelFormat::RGBA8Unorm,
                MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget,
            )
            .ok_or("the snapshot surface did not wrap as a texture")?;
        self.pass(src, &t, Some(k.max(1)))
    }

    fn pass(&self, src: &IOSurfaceRef, dst: &Texture, k: Option<u32>) -> Result<f64, String> {
        let src_tex = self
            .wrap(src, MTLPixelFormat::BGRA8Unorm, MTLTextureUsage::ShaderRead)
            .ok_or("the framebuffer did not wrap as a texture")?;
        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: index 0 is inside the attachments array.
        let att = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        att.setTexture(Some(dst));
        att.setLoadAction(MTLLoadAction::DontCare);
        att.setStoreAction(MTLStoreAction::Store);
        let cb = self.queue.commandBuffer().ok_or("no command buffer")?;
        let enc = cb
            .renderCommandEncoderWithDescriptor(&pass)
            .ok_or("no render encoder")?;
        match k {
            None => enc.setRenderPipelineState(&self.pipeline),
            Some(k) => {
                enc.setRenderPipelineState(&self.shrink);
                // SAFETY: four bytes the shader reads as its `k` at buffer 0, copied by Metal at once.
                unsafe {
                    enc.setFragmentBytes_length_atIndex(
                        NonNull::from(&k).cast(),
                        std::mem::size_of::<u32>(),
                        0,
                    )
                };
            }
        }
        // SAFETY: the texture is bound at the index the shader reads (0) and lives until the buffer completes.
        unsafe { enc.setFragmentTexture_atIndex(Some(&src_tex), 0) };
        // SAFETY: three vertices, no buffers: the full-screen triangle.
        unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
        enc.endEncoding();
        cb.commit();
        cb.waitUntilCompleted(); // sync-ok: under 2 ms, the device's own queue
        if let Some(e) = cb.error() {
            return Err(format!("the ring pass failed: {e}"));
        }
        Ok((cb.GPUEndTime() - cb.GPUStartTime()) * 1000.0)
    }
}

impl std::fmt::Debug for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Gpu")
    }
}

/// A `width` x `height` RGBA IOSurface ('RGBA', 4 bytes a pixel).
pub fn rgba_surface(width: usize, height: usize) -> Option<CFRetained<IOSurfaceRef>> {
    surface(width, height, u32::from_be_bytes(*b"RGBA"))
}

/// A `width` x `height` surface of a 4-byte pixel format.
pub fn surface(width: usize, height: usize, format: u32) -> Option<CFRetained<IOSurfaceRef>> {
    let keys = [
        NSString::from_str("IOSurfaceWidth"),
        NSString::from_str("IOSurfaceHeight"),
        NSString::from_str("IOSurfaceBytesPerElement"),
        NSString::from_str("IOSurfacePixelFormat"),
    ];
    let values = [
        NSNumber::new_usize(width),
        NSNumber::new_usize(height),
        NSNumber::new_usize(4),
        NSNumber::new_u32(format),
    ];
    let k: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let v: Vec<&NSNumber> = values.iter().map(|v| &**v).collect();
    let d = NSDictionary::from_slices(&k, &v);
    // SAFETY: an NSDictionary is toll-free bridged to CFDictionary; the keys are IOSurface's property names and the
    // values their types.
    unsafe { IOSurfaceRef::new(&*Retained::as_ptr(&d).cast()) }
}

/// Reads row `y` of a 4-byte surface (locked read-only while it is copied). None outside the surface.
pub fn read_row(s: &IOSurfaceRef, y: usize) -> Option<Vec<u8>> {
    if y >= s.height() {
        return None;
    }
    let len = s.width() * 4;
    // SAFETY: the surface is locked read-only for the copy; the row lies inside it (y < height, bytes per row is at
    // least width * 4 for a 4-byte format).
    unsafe {
        if s.lock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut()) != 0 {
            return None;
        }
        let base: NonNull<u8> = s.base_address().cast();
        let row =
            std::slice::from_raw_parts(base.as_ptr().add(y * s.bytes_per_row()), len).to_vec();
        s.unlock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut());
        Some(row)
    }
}

/// Reads the pixels at `points` (x, y) of a 4-byte surface, locked once read-only. None for a point outside it.
pub fn read_pixels(s: &IOSurfaceRef, points: &[(usize, usize)]) -> Option<Vec<[u8; 4]>> {
    let (w, h) = (s.width(), s.height());
    if points.iter().any(|&(x, y)| x >= w || y >= h) {
        return None;
    }
    // SAFETY: the surface is locked read-only for the reads; every point lies inside it (checked above), and bytes
    // per row is at least width * 4 for a 4-byte format.
    unsafe {
        if s.lock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut()) != 0 {
            return None;
        }
        let base: NonNull<u8> = s.base_address().cast();
        let bpr = s.bytes_per_row();
        let out = points
            .iter()
            .map(|&(x, y)| {
                let p = base.as_ptr().add(y * bpr + x * 4);
                [*p, *p.add(1), *p.add(2), *p.add(3)]
            })
            .collect();
        s.unlock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut());
        Some(out)
    }
}

/// Writes `px` (4 bytes) over the whole of a 4-byte surface (tests and the self-test).
pub fn fill_surface(s: &IOSurfaceRef, px: [u8; 4]) {
    // SAFETY: locked for writing; every write lies inside height x bytes per row.
    unsafe {
        s.lock(IOSurfaceLockOptions(0), std::ptr::null_mut());
        let base: NonNull<u8> = s.base_address().cast();
        for y in 0..s.height() {
            let row = base.as_ptr().add(y * s.bytes_per_row());
            for x in 0..s.width() {
                std::ptr::copy_nonoverlapping(px.as_ptr(), row.add(x * 4), 4);
            }
        }
        s.unlock(IOSurfaceLockOptions(0), std::ptr::null_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_ring_is_freed_only_after_every_slot_is_done() {
        let mut set: RingSet<&str> = RingSet::default();
        assert!(set.replace(1, 1, "first").is_empty());
        let (a, r) = set.take_free().map(|(s, r)| (s, r.device)).unwrap();
        let (b, _) = set.take_free().map(|(s, r)| (s, r.device)).unwrap();
        assert_eq!(r, 1);
        assert_ne!(a, b);
        // The screen changes size: a new ring, numbered anew. Flare still holds two slots of the first.
        assert!(
            set.replace(2, 2, "second").is_empty(),
            "not freed while Flare holds slots"
        );
        assert_eq!(set.old_count(), 1);
        assert_eq!(set.current().map(|c| c.device), Some(2));
        // New frames go to the new ring.
        let (_, cur) = set.take_free().unwrap();
        assert_eq!(cur.device, 2);
        // A slot-done record for the old ring frees that slot only.
        let (ok, freed) = set.done(1, a);
        assert!(ok);
        assert!(freed.is_empty(), "one slot of the old ring is still held");
        // A stray or repeated record changes nothing.
        assert_eq!(set.done(1, a), (false, vec![]));
        assert_eq!(set.done(9, 0), (false, vec![]));
        let (ok, freed) = set.done(1, b);
        assert!(ok);
        assert_eq!(freed, vec!["first"], "freed once every slot is done");
        assert_eq!(set.old_count(), 0);
        // A ring Flare holds nothing of is freed as soon as it is replaced.
        while set.take_free().is_some() {}
        for s in 0..RING_SLOTS as u8 {
            set.done(2, s);
        }
        assert_eq!(set.replace(3, 3, "third"), vec!["second"]);
        assert_eq!(set.dropped(), 1, "the frame with no free slot was counted");
        assert_eq!(set.take_all(), vec!["third"]);
    }

    #[test]
    fn the_pass_writes_opaque_srgb_from_a_p3_bgra_framebuffer() {
        let gpu = Gpu::new().expect("a Metal device");
        let (w, h) = (6, 4);
        let ring = gpu.ring(w, h).unwrap();
        assert_eq!(ring.surfaces.len(), RING_SLOTS);
        // A BGRA framebuffer of one P3 colour, with a transparent alpha the pass must not keep.
        let fb = surface(w, h, u32::from_be_bytes(*b"BGRA")).unwrap();
        let (r, g, b) = (200u8, 100u8, 40u8);
        fill_surface(&fb, [b, g, r, 0]);
        let ms = gpu.fill(&fb, &ring.textures[1]).unwrap();
        assert!(ms >= 0.0);
        let row = read_row(&ring.surfaces[1], 2).unwrap();
        let want =
            p3_to_srgb([r, g, b].map(|v| f32::from(v) / 255.0)).map(|v| (v * 255.0).round() as i32);
        for x in 0..w {
            let px = &row[x * 4..x * 4 + 4];
            for c in 0..3 {
                assert!(
                    (i32::from(px[c]) - want[c]).abs() <= 2,
                    "{px:?} vs {want:?}"
                );
            }
            assert_eq!(px[3], 255, "opaque");
        }
        // Greys stay grey (white is white, black is black), so the probe's barcode reads the same.
        for v in [0u8, 128, 255] {
            fill_surface(&fb, [v, v, v, 255]);
            gpu.fill(&fb, &ring.textures[0]).unwrap();
            let px = read_row(&ring.surfaces[0], 0).unwrap();
            for c in 0..3 {
                assert!((i32::from(px[c]) - i32::from(v)).abs() <= 1, "{v}: {px:?}");
            }
        }
        // P3's pure red is out of sRGB: it is clipped to sRGB's red.
        assert_eq!(
            p3_to_srgb([1.0, 0.0, 0.0]).map(|v| (v * 255.0).round() as u8),
            [255, 0, 0]
        );
    }
}
