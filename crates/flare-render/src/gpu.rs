//! GPU painter: paints a `Scene` into a `wgpu::Texture` with instanced quads and a glyph atlas.
//! Blending is in encoded byte space (`Rgba8Unorm`, never sRGB), matching the CPU painter.

use crate::atlas::{AtlasError, GlyphAtlas};
use crate::glyph::GlyphSource;
use crate::scene::{RectFill, Scene};
use crate::stats::{TracePhase, TraceSpan};
use bytemuck::{Pod, Zeroable};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RectInstance {
    rect: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlyphInstance {
    dst: [f32; 4],
    color: [f32; 4],
    uv: [f32; 2],
    is_color: f32,
    pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 2],
    pad: [f32; 2],
}

/// A vertex buffer that grows by doubling.
struct GrowBuf {
    buf: wgpu::Buffer,
    cap: u64,
}

impl GrowBuf {
    fn new(device: &wgpu::Device, label: &str) -> Self {
        let cap = 4096;
        Self {
            buf: Self::make(device, label, cap),
            cap,
        }
    }

    fn make(device: &wgpu::Device, label: &str, cap: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: cap,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, bytes: &[u8]) {
        let need = bytes.len() as u64;
        if need == 0 {
            return;
        }
        if need > self.cap {
            while self.cap < need {
                self.cap *= 2;
            }
            self.buf = Self::make(device, label, self.cap);
        }
        queue.write_buffer(&self.buf, 0, bytes);
    }
}

pub struct GpuPainter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    rect_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    atlas: GlyphAtlas,
    atlas_tex: wgpu::Texture,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    rect_buf: GrowBuf,
    glyph_buf: GrowBuf,
    target: Option<wgpu::Texture>,
    warned_too_large: bool,
    straight_pipeline: wgpu::RenderPipeline,
    straight_bgl: wgpu::BindGroupLayout,
    straight_target: Option<wgpu::Texture>,
}

/// The un-premultiply pass: one fullscreen triangle reading the painted grid texel for texel.
fn straight_pipeline(device: &wgpu::Device) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("flare straight"),
        source: wgpu::ShaderSource::Wgsl(include_str!("straight.wgsl").into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("flare straight bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("flare straight layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("flare straight"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_full"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_straight"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    (pipeline, bgl)
}

fn vertex_layout<'a>(
    stride: usize,
    attributes: &'a [wgpu::VertexAttribute],
) -> wgpu::VertexBufferLayout<'a> {
    wgpu::VertexBufferLayout {
        array_stride: stride as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes,
    }
}

impl GpuPainter {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let device = device.clone();
        let queue = queue.clone();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flare grid"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grid.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flare grid bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flare grid layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let rect_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let glyph_attrs =
            wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x2, 3 => Float32];
        let rect_buffers = [Some(vertex_layout(size_of::<RectInstance>(), &rect_attrs))];
        let glyph_buffers = [Some(vertex_layout(
            size_of::<GlyphInstance>(),
            &glyph_attrs,
        ))];

        let make_pipeline = |label: &str,
                             vs: &str,
                             fs: &str,
                             buffers: &[Option<wgpu::VertexBufferLayout>],
                             blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let rect_pipeline = make_pipeline("flare rects", "vs_rect", "fs_rect", &rect_buffers, None);
        let glyph_pipeline = make_pipeline(
            "flare glyphs",
            "vs_glyph",
            "fs_glyph",
            &glyph_buffers,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );

        let atlas_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("flare atlas"),
            size: wgpu::Extent3d {
                width: GlyphAtlas::SIZE,
                height: GlyphAtlas::SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flare globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let atlas_view = atlas_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flare grid bind group"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
            ],
        });
        let (straight_pipeline, straight_bgl) = straight_pipeline(&device);
        let rect_buf = GrowBuf::new(&device, "flare rect instances");
        let glyph_buf = GrowBuf::new(&device, "flare glyph instances");
        Self {
            device,
            queue,
            rect_pipeline,
            glyph_pipeline,
            atlas: GlyphAtlas::new(),
            atlas_tex,
            globals,
            bind_group,
            rect_buf,
            glyph_buf,
            target: None,
            warned_too_large: false,
            straight_pipeline,
            straight_bgl,
            straight_target: None,
        }
    }

    /// Paints like [`GpuPainter::paint`] and hands back the texture in STRAIGHT alpha, which is how
    /// Slint reads an imported wgpu texture (docs/material.md E3, Task 20b). An opaque scene is the
    /// same bytes either way, so only a see-through ground pays for the extra pass.
    pub fn paint_straight(&mut self, scene: &Scene, glyphs: &mut dyn GlyphSource) -> wgpu::Texture {
        let painted = self.paint(scene, glyphs);
        if scene.clear.a == 255 {
            return painted;
        }
        let _straight = TraceSpan::new(TracePhase::Paint);
        let (w, h) = (painted.width(), painted.height());
        let stale = self
            .straight_target
            .as_ref()
            .is_none_or(|t| t.width() != w || t.height() != h);
        if stale {
            self.straight_target = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("flare grid straight"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            }));
        }
        let out = self
            .straight_target
            .clone()
            .expect("straight target was just created");
        let src_view = painted.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flare straight bind group"),
            layout: &self.straight_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&src_view),
            }],
        });
        let view = out.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("flare straight"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flare straight pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.straight_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        out
    }

    /// Paints the scene into a texture of exactly `scene.width x scene.height` (recreated on size
    /// change, clamped to the device limit), submits, and returns a clone of it.
    pub fn paint(&mut self, scene: &Scene, glyphs: &mut dyn GlyphSource) -> wgpu::Texture {
        let _span = TraceSpan::new(TracePhase::Paint);
        let max = self.device.limits().max_texture_dimension_2d;
        let (w, h) = (scene.width.clamp(1, max), scene.height.clamp(1, max));
        let stale = self
            .target
            .as_ref()
            .is_none_or(|t| t.width() != w || t.height() != h);
        if stale {
            self.target = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("flare grid target"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            }));
        }
        let target = self.target.clone().expect("target was just created");

        let uploading = TraceSpan::new(TracePhase::GlyphUpload);
        let glyph_instances = self.build_glyph_instances(scene, glyphs);
        for up in self.atlas.take_uploads() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: up.x,
                        y: up.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &up.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(up.w * 4),
                    rows_per_image: Some(up.h),
                },
                wgpu::Extent3d {
                    width: up.w,
                    height: up.h,
                    depth_or_array_layers: 1,
                },
            );
        }
        drop(uploading);

        let rect_instances: Vec<RectInstance> = scene
            .rects
            .iter()
            .chain(scene.overlays.iter())
            .map(rect_instance)
            .collect();
        let n_rects = scene.rects.len() as u32;
        let n_all = rect_instances.len() as u32;
        self.rect_buf.write(
            &self.device,
            &self.queue,
            "flare rect instances",
            bytemuck::cast_slice(&rect_instances),
        );
        self.glyph_buf.write(
            &self.device,
            &self.queue,
            "flare glyph instances",
            bytemuck::cast_slice(&glyph_instances),
        );
        self.queue.write_buffer(
            &self.globals,
            0,
            bytemuck::bytes_of(&Globals {
                viewport: [w as f32, h as f32],
                pad: [0.0; 2],
            }),
        );

        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let clear = scene.clear;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("flare grid"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flare grid pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(clear.r) / 255.0,
                            g: f64::from(clear.g) / 255.0,
                            b: f64::from(clear.b) / 255.0,
                            a: f64::from(clear.a) / 255.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            if n_all > 0 {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_vertex_buffer(0, self.rect_buf.buf.slice(..));
                if n_rects > 0 {
                    pass.draw(0..6, 0..n_rects);
                }
            }
            if !glyph_instances.is_empty() {
                pass.set_pipeline(&self.glyph_pipeline);
                pass.set_vertex_buffer(0, self.glyph_buf.buf.slice(..));
                pass.draw(0..6, 0..glyph_instances.len() as u32);
            }
            if n_all > n_rects {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_vertex_buffer(0, self.rect_buf.buf.slice(..));
                pass.draw(0..6, n_rects..n_all);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        target
    }

    /// The font scale changed, so every cached bitmap is wrong.
    pub fn on_scale_change(&mut self) {
        self.atlas.clear();
    }

    /// The face or base size changed, so every cached bitmap is wrong.
    pub fn on_font_change(&mut self) {
        self.atlas.clear();
    }

    /// One instance per drawable glyph. A full atlas is cleared once and the list rebuilt; if it is
    /// full again the remaining glyphs are skipped for this frame.
    fn build_glyph_instances(
        &mut self,
        scene: &Scene,
        glyphs: &mut dyn GlyphSource,
    ) -> Vec<GlyphInstance> {
        for attempt in 0..2 {
            let mut out = Vec::with_capacity(scene.glyphs.len());
            let mut full = false;
            let mut skipped = 0usize;
            for draw in &scene.glyphs {
                match self.atlas.get(&draw.key, glyphs) {
                    Ok(Some(s)) => out.push(GlyphInstance {
                        dst: [
                            (draw.pen_x + s.left) as f32,
                            (draw.baseline_y - s.top) as f32,
                            s.w as f32,
                            s.h as f32,
                        ],
                        color: [
                            f32::from(draw.color.r) / 255.0,
                            f32::from(draw.color.g) / 255.0,
                            f32::from(draw.color.b) / 255.0,
                            1.0,
                        ],
                        uv: [s.x as f32, s.y as f32],
                        is_color: if s.color { 1.0 } else { 0.0 },
                        pad: 0.0,
                    }),
                    Ok(None) => {}
                    Err(AtlasError::TooLarge) => {
                        if !self.warned_too_large {
                            self.warned_too_large = true;
                            log::warn!("glyph larger than the atlas page, skipped");
                        }
                    }
                    Err(AtlasError::Full) => {
                        if attempt == 0 {
                            full = true;
                            break;
                        }
                        skipped += 1;
                    }
                }
            }
            if !full {
                if skipped > 0 {
                    log::warn!(
                        "glyph atlas full after a clear, {skipped} glyphs skipped this frame"
                    );
                }
                return out;
            }
            self.atlas.clear();
        }
        Vec::new()
    }
}

fn rect_instance(r: &RectFill) -> RectInstance {
    RectInstance {
        rect: [r.x as f32, r.y as f32, r.w as f32, r.h as f32],
        color: [
            f32::from(r.color.r) / 255.0,
            f32::from(r.color.g) / 255.0,
            f32::from(r.color.b) / 255.0,
            f32::from(r.color.a) / 255.0,
        ],
    }
}

/// A DX12 device for tests and tools: high-performance adapter, then WARP; full default limits.
pub fn headless_device() -> Result<(wgpu::Device, wgpu::Queue, wgpu::AdapterInfo), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        ..Default::default()
    }))
    .or_else(|_| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::None,
            force_fallback_adapter: true,
            compatible_surface: None,
            ..Default::default()
        }))
    })
    .map_err(|e| format!("no wgpu adapter: {e}"))?;
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("flare headless"),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .map_err(|e| format!("request_device failed: {e}"))?;
    Ok((device, queue, info))
}

/// Copies a texture to tightly packed RGBA (undoing the 256-byte row alignment) and waits.
pub fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> Vec<u8> {
    let (w, h) = (tex.width(), tex.height());
    let unpadded = w * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("flare readback"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("flare readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll");
    let data = buffer.slice(..).get_mapped_range().expect("mapped range");
    let mut out = Vec::with_capacity(unpadded as usize * h as usize);
    for row in 0..h as usize {
        let start = row * padded as usize;
        out.extend_from_slice(&data[start..start + unpadded as usize]);
    }
    drop(data);
    buffer.unmap();
    out
}
