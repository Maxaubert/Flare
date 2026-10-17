//! GridHost: owns the grid, the glyph source and both painters, and turns them into a Slint image.

use std::time::Instant;

use flare_render::color::Palette;
use flare_render::cpu::{RgbaImage, paint_scene};
use flare_render::glyph::GlyphSource;
use flare_render::gpu::{GpuPainter, read_texture};
use flare_render::grid::Grid;
use flare_render::metrics::{CellMetrics, cell_metrics, grid_dims};
use flare_render::sample::sample_grid;
use flare_render::scene::{Scene, build_scene};
use flare_render::stats::FrameStats;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// Where the grid on screen comes from: the sample, or a live terminal.
pub trait GridSource {
    /// The grid area's size in cells changed (called before the next `grid()`, once per change).
    fn resize(&mut self, cols: usize, rows: usize);
    /// The grid to paint now, exactly cols x rows as last given to `resize`.
    fn grid(&mut self) -> Grid;
    /// Something changed since the last `grid()` (output arrived). Default false.
    fn dirty(&self) -> bool {
        false
    }
}

/// The M1 sample grid, the default source.
pub struct SampleSource {
    cols: usize,
    rows: usize,
}

impl SampleSource {
    pub fn new() -> Self {
        Self { cols: 0, rows: 0 }
    }
}

impl Default for SampleSource {
    fn default() -> Self {
        Self::new()
    }
}

impl GridSource for SampleSource {
    fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
    }
    fn grid(&mut self) -> Grid {
        sample_grid(self.cols, self.rows)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridPainter {
    Gpu,
    Cpu,
}

pub struct GridHost {
    source: Box<dyn GridSource>,
    /// The cols x rows last given to the source (None = not yet told).
    dims: Option<(usize, usize)>,
    palette: Palette,
    glyphs: Box<dyn GlyphSource>,
    painter: GridPainter,
    /// The painter asked for (at start or by the overlay switch), restored when a GPU attaches.
    wanted: GridPainter,
    gpu: Option<GpuPainter>,
    /// The device and queue behind `gpu`, kept so `render_rgba` can read the texture back.
    readback: Option<(wgpu::Device, wgpu::Queue)>,
    cpu_image: RgbaImage,
    size: (u32, u32),
    scale: f32,
    dirty: bool,
    pub stats: FrameStats,
}

impl GridHost {
    pub fn new(glyphs: Box<dyn GlyphSource>, painter: GridPainter) -> Self {
        Self {
            source: Box::new(SampleSource::new()),
            dims: None,
            palette: Palette::pt_default(),
            glyphs,
            painter,
            wanted: painter,
            gpu: None,
            readback: None,
            cpu_image: RgbaImage::new(0, 0),
            size: (0, 0),
            scale: 1.0,
            dirty: true,
            stats: FrameStats::new(120),
        }
    }

    pub fn attach_gpu(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.gpu = Some(GpuPainter::new(device, queue));
        self.readback = Some((device.clone(), queue.clone()));
        // After a teardown and a new setup the grid goes back on the GPU if that was asked for.
        self.painter = self.wanted;
        self.dirty = true;
    }

    /// Drops the GPU painter; the host paints on the CPU until a GPU attaches again.
    pub fn detach_gpu(&mut self) {
        if self.gpu.take().is_some() {
            self.dirty = true;
        }
        self.readback = None;
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
        self.wanted = self.painter;
        self.dirty = true;
        self.painter
    }

    pub fn painter(&self) -> GridPainter {
        self.painter
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty || self.source.dirty()
    }

    /// Replaces the grid source; the next render tells it the size again.
    #[allow(dead_code)] // used by the live tab (Task 27)
    pub fn set_source(&mut self, source: Box<dyn GridSource>) {
        self.source = source;
        self.dims = None;
        self.dirty = true;
    }

    /// Cell metrics for the current scale.
    #[allow(dead_code)] // used by the live tab (Task 27)
    pub fn metrics(&self) -> CellMetrics {
        cell_metrics(&self.glyphs.font_units(), self.scale)
    }

    /// Cols and rows of the last render ((0, 0) before any).
    #[allow(dead_code)] // used by the live tab (Task 27)
    pub fn dims(&self) -> (usize, usize) {
        self.dims.unwrap_or((0, 0))
    }

    #[allow(dead_code)] // used by the live tab (Task 27)
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Lays out the source's grid for the current size and scale. None for a 0x0 size.
    fn layout(&mut self) -> Option<Scene> {
        let (w, h) = self.size;
        if w == 0 || h == 0 {
            return None;
        }
        let metrics = cell_metrics(&self.glyphs.font_units(), self.scale);
        let (cols, rows) = grid_dims(w, h, &metrics);
        if self.dims != Some((cols, rows)) {
            self.source.resize(cols, rows);
            self.dims = Some((cols, rows));
        }
        let grid = self.source.grid();
        Some(build_scene(&grid, &self.palette, &metrics, w, h))
    }

    /// Lays out and paints one frame with the active painter. None for a 0x0 size.
    pub fn render(&mut self) -> Option<Image> {
        let start = Instant::now();
        let scene = self.layout()?;

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

    /// Lays out and paints one frame into plain RGBA bytes, for the headless tools. The GPU
    /// path reads its texture back; the CPU path paints its own image. None for a 0x0 size.
    pub fn render_rgba(&mut self) -> Option<&RgbaImage> {
        let start = Instant::now();
        let scene = self.layout()?;
        match (self.painter, self.gpu.as_mut(), self.readback.as_ref()) {
            (GridPainter::Gpu, Some(gpu), Some((device, queue))) => {
                let texture = gpu.paint(&scene, self.glyphs.as_mut());
                let data = read_texture(device, queue, &texture);
                self.cpu_image = RgbaImage {
                    width: texture.width(),
                    height: texture.height(),
                    data,
                };
            }
            _ => paint_scene(&scene, self.glyphs.as_mut(), &mut self.cpu_image),
        }
        self.stats.push_ms(start.elapsed().as_secs_f64() * 1000.0);
        self.dirty = false;
        Some(&self.cpu_image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_render::color::Color;
    use flare_render::glyph::FakeGlyphs;
    use flare_render::gpu::headless_device;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    type Calls = Rc<RefCell<Vec<(usize, usize)>>>;

    struct Counting {
        calls: Calls,
        size: (usize, usize),
        x: bool,
        flag: Option<Arc<AtomicBool>>,
    }

    impl GridSource for Counting {
        fn resize(&mut self, cols: usize, rows: usize) {
            self.calls.borrow_mut().push((cols, rows));
            self.size = (cols, rows);
        }
        fn grid(&mut self) -> Grid {
            if let Some(f) = &self.flag {
                f.store(false, Ordering::SeqCst);
            }
            let mut g = Grid::new(self.size.0, self.size.1);
            g.cursor.visible = false;
            if self.x {
                let c = g.cell_mut(0, 0);
                c.ch = 'X';
                c.fg = Color::DefaultFg;
                c.bg = Color::DefaultBg;
            }
            g
        }
        fn dirty(&self) -> bool {
            self.flag.as_ref().is_some_and(|f| f.load(Ordering::SeqCst))
        }
    }

    fn counting(calls: &Calls) -> Box<Counting> {
        Box::new(Counting {
            calls: calls.clone(),
            size: (0, 0),
            x: false,
            flag: None,
        })
    }

    #[test]
    fn source_gets_resize_once_per_dims_change() {
        let calls = Calls::default();
        let mut h = host(GridPainter::Cpu);
        h.set_source(counting(&calls));
        h.resize(200, 100, 1.0);
        h.render();
        h.render();
        assert_eq!(*calls.borrow(), vec![(24, 5)]);
        h.resize(240, 100, 1.0);
        h.render();
        assert_eq!(*calls.borrow(), vec![(24, 5), (29, 5)]);
    }

    #[test]
    fn the_source_grid_is_what_is_painted() {
        let calls = Calls::default();
        let mut src = counting(&calls);
        src.x = true;
        let mut h = host(GridPainter::Cpu);
        h.set_source(src);
        h.resize(200, 100, 1.0);
        let img = h.render_rgba().unwrap();
        assert_eq!(img.pixel(8, 12), [0xde, 0xda, 0xcf, 0xff]);
    }

    #[test]
    fn dirty_follows_the_source() {
        let calls = Calls::default();
        let mut src = counting(&calls);
        src.flag = Some(Arc::new(AtomicBool::new(true)));
        let mut h = host(GridPainter::Cpu);
        h.set_source(src);
        h.resize(200, 100, 1.0);
        assert!(h.is_dirty());
        h.render();
        assert!(!h.is_dirty());
    }

    #[test]
    fn set_source_forces_a_resize_call() {
        let calls = Calls::default();
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        h.render();
        h.set_source(counting(&calls));
        h.render();
        assert_eq!(calls.borrow().len(), 1);
    }

    #[test]
    fn metrics_follow_the_scale() {
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 2.25);
        assert_eq!(h.metrics().cell_w, 17);
    }

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
    fn render_rgba_cpu_returns_image_of_size() {
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        let img = h.render_rgba().expect("image");
        assert_eq!((img.width, img.height), (200, 100));
        assert_eq!(img.data.len(), 200 * 100 * 4);
    }

    #[test]
    fn render_rgba_zero_size_is_none() {
        let mut h = host(GridPainter::Cpu);
        h.resize(0, 0, 1.0);
        assert!(h.render_rgba().is_none());
    }

    #[test]
    fn render_rgba_gpu_reads_back_same_size() {
        let Ok((device, queue, _)) = headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let mut h = host(GridPainter::Gpu);
        h.attach_gpu(&device, &queue);
        h.resize(200, 100, 1.0);
        let img = h.render_rgba().expect("image");
        assert_eq!((img.width, img.height), (200, 100));
        assert_eq!(img.data.len(), 200 * 100 * 4);
        assert_eq!(h.painter(), GridPainter::Gpu);
    }

    #[test]
    fn reattach_after_teardown_restores_the_gpu() {
        let Ok((device, queue, _)) = headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let mut h = host(GridPainter::Gpu);
        h.attach_gpu(&device, &queue);
        h.detach_gpu();
        assert_eq!(h.painter(), GridPainter::Cpu);
        h.attach_gpu(&device, &queue);
        assert_eq!(h.painter(), GridPainter::Gpu);
        // A grid switched to the CPU by hand stays there across a new setup.
        h.toggle_painter();
        h.detach_gpu();
        h.attach_gpu(&device, &queue);
        assert_eq!(h.painter(), GridPainter::Cpu);
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
