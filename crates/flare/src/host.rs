//! GridHost: owns the grid, the glyph source and both painters, and turns them into a Slint image.

use std::time::Instant;

use flare_render::color::{Palette, Rgba};
use flare_render::cpu::{RgbaImage, paint_scene};
use flare_render::decor::{Decor, build_scene_decor, hair_px};
use flare_render::glyph::GlyphSource;
use flare_render::gpu::{GpuPainter, read_texture};
use flare_render::grid::Grid;
use flare_render::metrics::{BASE_FONT_PX, CellMetrics, cell_metrics_at, grid_dims};
use flare_render::sample::sample_grid;
use flare_render::scene::{GroundPaint, Scene};
use flare_render::stats::{FrameStats, FrameTrace, TracePhase};
use slint::{Image, Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer};

/// Where the grid on screen comes from: the sample, or a live terminal.
pub trait GridSource {
    /// The grid area's size in cells changed (called before the next `grid()`, once per change).
    fn resize(&mut self, cols: usize, rows: usize);
    /// The grid to paint now, exactly cols x rows as last given to `resize`.
    fn grid(&mut self) -> Grid;
    /// [`GridSource::grid`] into a buffer the host keeps from frame to frame. A source that can copy
    /// without allocating overrides this; the default builds a fresh grid.
    fn grid_into(&mut self, out: &mut Grid) {
        *out = self.grid();
    }
    /// Something changed since the last `grid()` (output arrived). Default false.
    fn dirty(&self) -> bool {
        false
    }
    /// The decoration to paint over the grid now (selection, find, links). Default none.
    fn decor(&mut self) -> Decor {
        Decor::default()
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
    /// Who paints the ground behind the cells: the grid, or the UI (glass).
    ground: GroundPaint,
    glyphs: Box<dyn GlyphSource>,
    painter: GridPainter,
    /// The painter asked for (at start or by the overlay switch), restored when a GPU attaches.
    wanted: GridPainter,
    gpu: Option<GpuPainter>,
    /// The device and queue behind `gpu`, kept so `render_rgba` can read the texture back.
    readback: Option<(wgpu::Device, wgpu::Queue)>,
    cpu_image: RgbaImage,
    /// The grid the source fills each frame (reused, so a frame allocates no cell storage).
    grid_buf: Grid,
    size: (u32, u32),
    scale: f32,
    /// The face stack and base size (logical px) last given to the glyph source.
    families: Vec<String>,
    base_px: f32,
    dirty: bool,
    /// The caret is an outline (the keyboard is on a popup or a field): audit V11.
    hollow_cursor: bool,
    pub stats: FrameStats,
}

impl GridHost {
    pub fn new(glyphs: Box<dyn GlyphSource>, painter: GridPainter) -> Self {
        Self {
            source: Box::new(SampleSource::new()),
            dims: None,
            palette: Palette::pt_default(),
            ground: GroundPaint::InGrid,
            glyphs,
            painter,
            wanted: painter,
            gpu: None,
            readback: None,
            cpu_image: RgbaImage::new(0, 0),
            grid_buf: Grid::new(1, 1),
            size: (0, 0),
            scale: 1.0,
            families: Vec::new(),
            base_px: BASE_FONT_PX,
            dirty: true,
            hollow_cursor: false,
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

    /// Sets the palette and who paints the ground; marks dirty only when something differs.
    #[allow(dead_code)] // used by the look controller (Task 23)
    pub fn set_look(&mut self, palette: Palette, ground: GroundPaint) {
        if self.palette == palette && self.ground == ground {
            return;
        }
        self.palette = palette;
        self.ground = ground;
        self.dirty = true;
    }

    /// Draws the caret as an outline (on) or a block; marks dirty only when it changes.
    pub fn set_hollow_cursor(&mut self, on: bool) {
        if self.hollow_cursor == on {
            return;
        }
        self.hollow_cursor = on;
        self.dirty = true;
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

    /// Changes the face and base size. None when unchanged; else the glyph source and the GPU atlas
    /// are told, and the new `(cols, rows)` is returned when they differ (the caller refits).
    #[allow(dead_code)] // used by the live tab (Task 27)
    pub fn set_font(&mut self, families: &[&str], base_px: f32) -> Option<(usize, usize)> {
        let same = self.families.len() == families.len()
            && self.families.iter().zip(families).all(|(a, b)| a == b)
            && (base_px - self.base_px).abs() < f32::EPSILON;
        if same {
            return None;
        }
        let before = self.dims;
        self.families = families.iter().map(|f| f.to_string()).collect();
        self.base_px = base_px;
        self.glyphs.set_font(families, base_px);
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.on_font_change();
        }
        self.dirty = true;
        let (w, h) = self.size;
        if w == 0 || h == 0 {
            return None;
        }
        let metrics = cell_metrics_at(&self.glyphs.font_units(), self.base_px, self.scale);
        let now = grid_dims(w, h, &metrics);
        (before != Some(now)).then_some(now)
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

    /// Adds what the grid host holds to a memory breakdown (the painters, the glyph source).
    pub fn fill_breakdown(&self, b: &mut crate::memory::Breakdown) {
        if let Some(gpu) = &self.gpu {
            let m = gpu.memory();
            b.atlas_cpu += m.atlas_cpu;
            b.atlas_gpu += m.atlas_gpu;
            b.grid_texture += m.grid_texture;
        }
        b.cpu_frame_buffer += self.cpu_image.data.capacity() + self.grid_buf.bytes();
        let g = self.glyphs.memory();
        b.glyph_cache += g.caches;
        b.font_database += g.fonts;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty || self.source.dirty() || self.glyphs.fallback_ready()
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
        cell_metrics_at(&self.glyphs.font_units(), self.base_px, self.scale)
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
        // A fallback font found off the UI thread (audit M2): cells drawn blank get their glyphs,
        // and the GPU atlas forgets the blanks it holds. The CPU painter reads the same source.
        if self.glyphs.apply_fallbacks()
            && let Some(gpu) = self.gpu.as_mut()
        {
            gpu.on_font_change();
        }
        let metrics = cell_metrics_at(&self.glyphs.font_units(), self.base_px, self.scale);
        let (cols, rows) = grid_dims(w, h, &metrics);
        if self.dims != Some((cols, rows)) {
            self.source.resize(cols, rows);
            self.dims = Some((cols, rows));
        }
        self.source.grid_into(&mut self.grid_buf);
        self.grid_buf.cursor.hollow = self.hollow_cursor;
        let decor = self.source.decor();
        let mut scene = FrameTrace::global().time(TracePhase::Scene, || {
            build_scene_decor(
                &mut self.grid_buf,
                &self.palette,
                &metrics,
                w,
                h,
                &decor,
                hair_px(self.scale),
            )
        });
        // By UI: the grid clears to transparent so the glass shows through (build_scene_ground's
        // rule for the clear colour; the cell rects follow the same default-background test).
        if self.ground == GroundPaint::ByUi {
            scene.clear = Rgba::default();
        }
        Some(scene)
    }

    /// Lays out and paints one frame with the active painter. None for a 0x0 size.
    pub fn render(&mut self) -> Option<Image> {
        let start = Instant::now();
        let scene = self.layout()?;

        let mut image = None;
        if self.painter == GridPainter::Gpu
            && let Some(gpu) = self.gpu.as_mut()
        {
            // Slint reads an imported texture as straight alpha (docs/material.md E3): under glass
            // the grid is un-premultiplied first, or text would be darker at its edges.
            let texture = gpu.paint_straight(&scene, self.glyphs.as_mut());
            match Image::try_from(texture) {
                Ok(img) => {
                    // The first frames were painted on the CPU: that frame buffer is not needed
                    // once the GPU paints (a window-sized buffer, M10 Task 66).
                    if self.cpu_image.data.capacity() > 0 {
                        self.cpu_image = RgbaImage::new(0, 0);
                    }
                    image = Some(img);
                }
                Err(e) => {
                    log::warn!("importing the grid texture failed: {e:?}; painting on the CPU");
                    self.painter = GridPainter::Cpu;
                }
            }
        }
        let image = match image {
            Some(img) => img,
            None => {
                FrameTrace::global().time(TracePhase::Paint, || {
                    paint_scene(&scene, self.glyphs.as_mut(), &mut self.cpu_image);
                });
                slint_image(&self.cpu_image)
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

/// The grid image Slint shows. A picture with every pixel opaque (the software renderer's, which has no
/// glass) goes over as RGB: Slint's software renderer copies an RGB image and blends an RGBA one pixel by
/// pixel, which is the cost of repainting a 2.25x window on every frame of the sidebar slide (audit V4).
/// A picture with any see-through pixel stays premultiplied RGBA.
fn slint_image(img: &RgbaImage) -> Image {
    let (w, h) = (img.width, img.height);
    if img.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255) {
        let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
        let out = buffer.make_mut_bytes();
        for (dst, src) in out
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .zip(img.data.as_chunks::<4>().0)
        {
            dst.copy_from_slice(&src[..3]);
        }
        return Image::from_rgb8(buffer);
    }
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&img.data, w, h);
    // Premultiplied: at alpha 255 identical to straight, right on a clear ground.
    Image::from_rgba8_premultiplied(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_render::color::Color;
    use flare_render::decor::{DecorInk, RowSpan};
    use flare_render::glyph::FakeGlyphs;
    use flare_render::gpu::headless_device;
    use flare_render::metrics::cell_origin;
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
    fn an_opaque_grid_image_goes_to_slint_as_rgb() {
        let mut img = RgbaImage::new(3, 2);
        for p in img.data.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&[10, 20, 30, 255]);
        }
        let s = slint_image(&img);
        let rgb = s.to_rgb8().expect("an opaque picture is an RGB image");
        assert_eq!((rgb.width(), rgb.height()), (3, 2));
        assert_eq!(rgb.as_bytes()[..3], [10, 20, 30]);
    }

    #[test]
    fn a_see_through_grid_image_stays_premultiplied_rgba() {
        let mut img = RgbaImage::new(3, 2);
        for p in img.data.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&[10, 20, 30, 255]);
        }
        img.data[3] = 128;
        let s = slint_image(&img);
        assert!(s.to_rgb8().is_none());
        assert_eq!(s.to_rgba8_premultiplied().expect("rgba").as_bytes()[3], 128);
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

    /// A source that can only fill the host's buffer: asking it for a fresh grid fails the test.
    struct FillOnly {
        size: (usize, usize),
        fills: Rc<RefCell<usize>>,
    }

    impl GridSource for FillOnly {
        fn resize(&mut self, cols: usize, rows: usize) {
            self.size = (cols, rows);
        }
        fn grid(&mut self) -> Grid {
            panic!("the host must ask for the buffer fill, not a new grid");
        }
        fn grid_into(&mut self, out: &mut Grid) {
            *self.fills.borrow_mut() += 1;
            out.reshape(self.size.0, self.size.1);
            out.cursor.visible = false;
            out.cell_mut(0, 0).ch = 'X';
        }
    }

    #[test]
    fn the_host_asks_the_source_to_fill_its_own_buffer() {
        let fills = Rc::new(RefCell::new(0));
        let mut h = host(GridPainter::Cpu);
        h.set_source(Box::new(FillOnly {
            size: (0, 0),
            fills: fills.clone(),
        }));
        h.resize(200, 100, 1.0);
        let img = h.render_rgba().expect("image");
        assert_eq!(img.pixel(8, 12), [0xde, 0xda, 0xcf, 0xff]);
        h.render();
        assert_eq!(*fills.borrow(), 2);
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
    fn the_hollow_caret_redraws_only_when_it_changes() {
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        let _ = h.render();
        assert!(!h.is_dirty());
        h.set_hollow_cursor(false);
        assert!(!h.is_dirty(), "the same state is no change");
        h.set_hollow_cursor(true);
        assert!(h.is_dirty(), "a popup took the keys: the caret redraws");
        let _ = h.render();
        h.set_hollow_cursor(true);
        assert!(!h.is_dirty());
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

    /// A source that paints a blank grid of the given size and one fixed decoration.
    struct DecorSource {
        cols: usize,
        rows: usize,
        decor: Decor,
    }

    impl GridSource for DecorSource {
        fn resize(&mut self, cols: usize, rows: usize) {
            self.cols = cols;
            self.rows = rows;
        }
        fn grid(&mut self) -> Grid {
            let mut g = Grid::new(self.cols, self.rows);
            g.cursor.visible = false;
            g
        }
        fn decor(&mut self) -> Decor {
            self.decor.clone()
        }
    }

    fn decor_source(decor: Decor) -> Box<DecorSource> {
        Box::new(DecorSource {
            cols: 0,
            rows: 0,
            decor,
        })
    }

    fn span(row: usize, col: usize, width: usize) -> RowSpan {
        RowSpan { row, col, width }
    }

    /// The centre pixel of a cell in a rendered frame.
    fn centre(img: &RgbaImage, m: &CellMetrics, col: usize, row: usize) -> [u8; 4] {
        let (x, y) = cell_origin(col, row, m);
        img.pixel(x as u32 + m.cell_w / 2, y as u32 + m.cell_h / 2)
    }

    const SCALES: [f32; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

    #[test]
    fn a_selection_span_paints_at_every_scale() {
        for scale in SCALES {
            let mut h = host(GridPainter::Cpu);
            h.set_source(decor_source(Decor {
                selection: vec![span(1, 1, 3)],
                ..Default::default()
            }));
            h.resize(1600, 1000, scale);
            let m = h.metrics();
            let img = h.render_rgba().expect("image");
            // The opaque blend of the selection colour over the pt-default ground (decor.rs test).
            assert_eq!(centre(img, &m, 2, 1), [78, 78, 78, 255], "scale {scale}");
            assert_ne!(centre(img, &m, 5, 1), [78, 78, 78, 255], "scale {scale}");
        }
    }

    #[test]
    fn the_underline_hair_follows_the_scale() {
        for (scale, hair) in [(1.0_f32, 1_usize), (2.25, 2)] {
            let mut h = host(GridPainter::Cpu);
            h.set_source(decor_source(Decor {
                links: vec![span(1, 1, 3)],
                ..Default::default()
            }));
            h.resize(1600, 1000, scale);
            let m = h.metrics();
            let img = h.render_rgba().expect("image");
            let (x, y) = cell_origin(2, 1, &m);
            let cx = x as u32 + m.cell_w / 2;
            let bottom = y as u32 + m.cell_h;
            let ink = DecorInk::pt_default();
            let bg = Palette::pt_default().bg;
            let faint = flare_render::blend::blend_mask(
                [bg.r, bg.g, bg.b, 255],
                ink.link,
                ink.link_underline_alpha,
            );
            for r in 1..=hair as u32 {
                let px = img.pixel(cx, bottom - r);
                assert_eq!(
                    &px[..3],
                    &faint[..3],
                    "scale {scale}, row {r} from the bottom"
                );
            }
            assert_ne!(
                &img.pixel(cx, bottom - hair as u32 - 1)[..3],
                &faint[..3],
                "scale {scale}: the row above the underline is not ink"
            );
        }
    }

    #[test]
    fn rendering_clears_dirty_with_a_decor() {
        let mut h = host(GridPainter::Cpu);
        h.set_source(decor_source(Decor {
            selection: vec![span(0, 0, 2)],
            ..Default::default()
        }));
        h.resize(1600, 1000, 1.0);
        assert!(h.is_dirty());
        h.render();
        assert!(!h.is_dirty());
    }

    fn host(painter: GridPainter) -> GridHost {
        GridHost::new(Box::new(FakeGlyphs::new(8, 16)), painter)
    }

    /// A glyph source whose fallback font arrives later (audit M2).
    struct Late {
        inner: FakeGlyphs,
        ready: Rc<std::cell::Cell<bool>>,
        applied: Rc<std::cell::Cell<u32>>,
    }

    impl flare_render::glyph::GlyphSource for Late {
        fn glyph(
            &mut self,
            key: &flare_render::glyph::GlyphKey,
        ) -> Option<&flare_render::glyph::GlyphBitmap> {
            self.inner.glyph(key)
        }
        fn font_units(&self) -> flare_render::metrics::FontUnits {
            self.inner.font_units()
        }
        fn set_scale(&mut self, scale: f32) {
            self.inner.set_scale(scale);
        }
        fn fallback_ready(&self) -> bool {
            self.ready.get()
        }
        fn apply_fallbacks(&mut self) -> bool {
            self.applied.set(self.applied.get() + 1);
            self.ready.replace(false)
        }
    }

    #[test]
    fn a_fallback_font_that_arrives_repaints_without_any_other_change() {
        let ready = Rc::new(std::cell::Cell::new(false));
        let applied = Rc::new(std::cell::Cell::new(0));
        let late = Late {
            inner: FakeGlyphs::new(8, 16),
            ready: ready.clone(),
            applied: applied.clone(),
        };
        let mut h = GridHost::new(Box::new(late), GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        h.render();
        assert!(!h.is_dirty());
        ready.set(true);
        assert!(h.is_dirty(), "the worker's answer asks for a frame");
        h.render();
        assert_eq!(applied.get(), 2, "the frame took the answer");
        assert!(!h.is_dirty());
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
    fn the_breakdown_counts_the_cpu_frame_buffer_and_no_gpu_parts_on_the_cpu_painter() {
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        h.render();
        let mut b = crate::memory::Breakdown::default();
        h.fill_breakdown(&mut b);
        assert!(b.cpu_frame_buffer >= 200 * 100 * 4);
        assert_eq!((b.atlas_gpu, b.grid_texture), (0, 0));
    }

    #[test]
    fn the_breakdown_counts_the_grid_texture_and_the_atlas_on_the_gpu_painter() {
        let Ok((device, queue, _)) = headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let mut h = host(GridPainter::Gpu);
        h.attach_gpu(&device, &queue);
        h.resize(200, 100, 1.0);
        h.render_rgba().expect("image");
        let mut b = crate::memory::Breakdown::default();
        h.fill_breakdown(&mut b);
        assert_eq!(b.grid_texture, 200 * 100 * 4);
        assert_eq!(b.atlas_gpu, 2048 * 2048 * 4);
    }
    #[test]
    fn the_grid_texture_matches_the_window() {
        // The texture is the size of the grid area, never the device limit (8192).
        let Ok((device, queue, _)) = headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let mut h = host(GridPainter::Gpu);
        h.attach_gpu(&device, &queue);
        for (w, hgt) in [(400u32, 200u32), (1180, 744), (2655, 1674)] {
            h.resize(w, hgt, 1.0);
            h.render_rgba().expect("image");
            let mut b = crate::memory::Breakdown::default();
            h.fill_breakdown(&mut b);
            assert_eq!(b.grid_texture, (w * hgt * 4) as usize, "{w}x{hgt}");
        }
    }
    #[test]
    fn no_cpu_buffer_after_the_switch_on_gpu() {
        let Ok((device, queue, _)) = headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        let mut h = host(GridPainter::Gpu);
        h.resize(400, 200, 1.0);
        // The first frames are painted on the CPU, before the GPU is attached.
        h.render();
        let mut b = crate::memory::Breakdown::default();
        h.fill_breakdown(&mut b);
        assert!(b.cpu_frame_buffer >= 400 * 200 * 4, "the CPU frame is held");
        h.attach_gpu(&device, &queue);
        h.render();
        assert_eq!(h.painter(), GridPainter::Gpu);
        let mut after = crate::memory::Breakdown::default();
        h.fill_breakdown(&mut after);
        assert!(
            after.cpu_frame_buffer < 400 * 200,
            "the CPU frame buffer is still held on the GPU painter: {} bytes",
            after.cpu_frame_buffer
        );
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

    #[test]
    fn render_rgba_in_by_ui_is_clear_where_the_ground_is() {
        let mut h = host(GridPainter::Cpu);
        h.set_look(Palette::pt_default(), GroundPaint::ByUi);
        h.resize(200, 100, 1.0);
        let img = h.render_rgba().expect("image");
        // The sample grid has its cursor and text; the far corner is a blank cell.
        assert_eq!(img.pixel(199, 99)[3], 0);
        assert!(
            img.data.chunks(4).any(|px| px[3] == 255),
            "a coloured block cell is opaque"
        );
    }

    #[test]
    fn host_set_font_returns_the_new_grid_size() {
        let mut h = host(GridPainter::Cpu);
        h.resize(648, 264, 1.0);
        h.render();
        let (c13, r13) = h.dims();
        let (cols, rows) = h.set_font(&["Consolas"], 26.0).expect("the grid changes");
        // The fake face is 8 px wide at 13 and 15 at 26 (rounding), so columns halve within 2.
        assert!(
            (cols as i64 - (c13 / 2) as i64).abs() <= 2,
            "{c13} -> {cols}"
        );
        assert!(
            (rows as i64 - (r13 / 2) as i64).abs() <= 1,
            "{r13} -> {rows}"
        );
        assert!(h.is_dirty());
    }

    #[test]
    fn host_set_font_twice_is_none() {
        let mut h = host(GridPainter::Cpu);
        h.resize(648, 264, 1.0);
        assert!(h.set_font(&["Consolas"], 26.0).is_some());
        assert!(h.set_font(&["Consolas"], 26.0).is_none());
    }

    #[test]
    fn a_font_change_reaches_the_atlas() {
        let Ok((device, queue, _)) = headless_device() else {
            eprintln!("skipped: no GPU adapter");
            return;
        };
        let mut h = host(GridPainter::Gpu);
        h.attach_gpu(&device, &queue);
        h.resize(200, 100, 1.0);
        assert!(h.render().is_some());
        h.set_font(&["Consolas"], 20.0);
        assert!(h.render().is_some());
    }

    #[test]
    fn set_look_marks_dirty_only_on_a_change() {
        let mut h = host(GridPainter::Cpu);
        h.resize(200, 100, 1.0);
        h.render();
        h.set_look(Palette::pt_default(), GroundPaint::InGrid);
        assert!(!h.is_dirty());
        h.set_look(Palette::pt_default(), GroundPaint::ByUi);
        assert!(h.is_dirty());
        h.render();
        h.set_look(Palette::pt_default(), GroundPaint::ByUi);
        assert!(!h.is_dirty());
        let mut p = Palette::pt_default();
        p.fg = flare_render::color::Rgba::rgb(1, 1, 1);
        h.set_look(p, GroundPaint::ByUi);
        assert!(h.is_dirty());
    }

    slint::slint! {
        export component PremulWin inherits Window {
            in property <image> grid;
            width: 20px;
            height: 20px;
            background: white;
            Image { source: root.grid; width: 20px; height: 20px; }
        }
    }

    #[test]
    fn a_half_covered_pixel_over_white_is_lighter_than_straight_would_be() {
        use crate::ui_harness::{Frame, Harness};
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(4, 4);
        for px in buf.make_mut_slice() {
            *px = Rgba8Pixel {
                r: 128,
                g: 0,
                b: 0,
                a: 128,
            };
        }
        for scale in [1.0, 2.25] {
            let hn = Harness::new(scale, 20, 20, || PremulWin::new().unwrap());
            hn.comp
                .set_grid(Image::from_rgba8_premultiplied(buf.clone()));
            let f = hn.render();
            let c = f.px(f.w / 2, f.h / 2);
            assert!(
                Frame::near(c, [255, 127, 127, 255], 2),
                "scale {scale}: {c:?}"
            );
        }
    }
}
