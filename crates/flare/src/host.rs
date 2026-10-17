//! GridHost: owns the grid, the glyph source and both painters, and turns them into a Slint image.

use std::time::Instant;

use flare_render::color::Palette;
use flare_render::cpu::{RgbaImage, paint_scene};
use flare_render::glyph::GlyphSource;
use flare_render::gpu::GpuPainter;
use flare_render::grid::Grid;
use flare_render::metrics::{cell_metrics, grid_dims};
use flare_render::sample::sample_grid;
use flare_render::scene::build_scene;
use flare_render::stats::FrameStats;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridPainter {
    Gpu,
    Cpu,
}

pub struct GridHost {
    grid_src: fn(usize, usize) -> Grid,
    palette: Palette,
    glyphs: Box<dyn GlyphSource>,
    painter: GridPainter,
    gpu: Option<GpuPainter>,
    cpu_image: RgbaImage,
    size: (u32, u32),
    scale: f32,
    dirty: bool,
    pub stats: FrameStats,
}

impl GridHost {
    pub fn new(glyphs: Box<dyn GlyphSource>, painter: GridPainter) -> Self {
        Self {
            grid_src: sample_grid,
            palette: Palette::pt_default(),
            glyphs,
            painter,
            gpu: None,
            cpu_image: RgbaImage::new(0, 0),
            size: (0, 0),
            scale: 1.0,
            dirty: true,
            stats: FrameStats::new(120),
        }
    }

    pub fn attach_gpu(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.gpu = Some(GpuPainter::new(device, queue));
        self.dirty = true;
    }

    /// Drops the GPU painter; the host paints on the CPU from now on.
    pub fn detach_gpu(&mut self) {
        if self.gpu.take().is_some() {
            self.dirty = true;
        }
        self.painter = GridPainter::Cpu;
    }

    pub fn resize(&mut self, w: u32, h: u32, scale: f32) {
        let scale_changed = (scale - self.scale).abs() > f32::EPSILON;
        if (w, h) == self.size && !scale_changed {
            return;
        }
        if scale_changed {
            self.glyphs.set_scale(scale);
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.on_scale_change();
            }
            self.scale = scale;
        }
        self.size = (w, h);
        self.dirty = true;
    }

    /// Switches the grid painter; Gpu only when a GpuPainter is attached.
    pub fn toggle_painter(&mut self) -> GridPainter {
        self.painter = match self.painter {
            GridPainter::Gpu => GridPainter::Cpu,
            GridPainter::Cpu if self.gpu.is_some() => GridPainter::Gpu,
            GridPainter::Cpu => GridPainter::Cpu,
        };
        self.dirty = true;
        self.painter
    }

    pub fn painter(&self) -> GridPainter {
        self.painter
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Lays out and paints one frame with the active painter. None for a 0x0 size.
    pub fn render(&mut self) -> Option<Image> {
        let (w, h) = self.size;
        if w == 0 || h == 0 {
            return None;
        }
        let start = Instant::now();
        let metrics = cell_metrics(&self.glyphs.font_units(), self.scale);
        let (cols, rows) = grid_dims(w, h, &metrics);
        let grid = (self.grid_src)(cols, rows);
        let scene = build_scene(&grid, &self.palette, &metrics, w, h);

        let mut image = None;
        if self.painter == GridPainter::Gpu
            && let Some(gpu) = self.gpu.as_mut()
        {
            let texture = gpu.paint(&scene, self.glyphs.as_mut());
            match Image::try_from(texture) {
                Ok(img) => image = Some(img),
                Err(e) => {
                    log::warn!("importing the grid texture failed: {e:?}; painting on the CPU");
                    self.painter = GridPainter::Cpu;
                }
            }
        }
        let image = match image {
            Some(img) => img,
            None => {
                paint_scene(&scene, self.glyphs.as_mut(), &mut self.cpu_image);
                let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                    &self.cpu_image.data,
                    self.cpu_image.width,
                    self.cpu_image.height,
                );
                Image::from_rgba8(buffer)
            }
        };
        self.stats.push_ms(start.elapsed().as_secs_f64() * 1000.0);
        self.dirty = false;
        Some(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_render::glyph::FakeGlyphs;
    use flare_render::gpu::headless_device;

    fn host(painter: GridPainter) -> GridHost {
        GridHost::new(Box::new(FakeGlyphs::new(8, 16)), painter)
    }

    #[test]
    fn cpu_render_returns_image_of_size() {
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        let img = h.render().expect("image");
        assert_eq!(img.size().width, 200);
        assert_eq!(img.size().height, 100);
    }

    #[test]
    fn resize_same_size_is_not_dirty() {
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        h.render();
        h.resize(200, 100, 1.0);
        assert!(!h.is_dirty());
        h.resize(201, 100, 1.0);
        assert!(h.is_dirty());
    }

    #[test]
    fn scale_change_marks_dirty() {
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        h.render();
        h.resize(200, 100, 2.0);
        assert!(h.is_dirty());
    }

    #[test]
    fn toggle_without_gpu_stays_cpu() {
        let mut h = host(GridPainter::Cpu);
        assert_eq!(h.toggle_painter(), GridPainter::Cpu);
        assert_eq!(h.painter(), GridPainter::Cpu);
    }

    #[test]
    fn zero_size_renders_none() {
        let mut h = host(GridPainter::Cpu);
        h.resize(0, 0, 1.0);
        assert!(h.render().is_none());
        h.resize(100, 0, 1.0);
        assert!(h.render().is_none());
    }

    #[test]
    fn render_records_a_frame() {
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        assert!(h.stats.last().is_none());
        h.render();
        assert!(h.stats.last().is_some());
    }

    #[test]
    fn render_clears_dirty() {
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        assert!(h.is_dirty());
        h.render();
        assert!(!h.is_dirty());
    }

    #[test]
    fn gpu_requested_without_device_renders_on_cpu() {
        let mut h = host(GridPainter::Gpu);
        h.resize(200, 100, 1.0);
        let img = h.render().expect("image");
        assert_eq!(img.size().width, 200);
        assert_eq!(img.size().height, 100);
    }

    #[test]
    fn gpu_attach_and_render_imports() {
        let Ok((device, queue, info)) = headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        eprintln!("adapter: {} ({:?})", info.name, info.backend);
        let mut h = host(GridPainter::Gpu);
        h.attach_gpu(&device, &queue);
        h.resize(200, 100, 1.0);
        let img = h.render();
        assert!(img.is_some());
        assert_eq!(h.painter(), GridPainter::Gpu);
    }
}
