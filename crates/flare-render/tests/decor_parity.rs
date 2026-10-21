//! A decorated scene paints the same on the GPU and the CPU. Skips without an adapter.

use flare_render::color::Palette;
use flare_render::cpu::{RgbaImage, paint_scene};
use flare_render::decor::{Decor, RowSpan, build_scene_decor, hair_px};
use flare_render::glyph::GlyphSource;
use flare_render::gpu::{GpuPainter, headless_device, read_texture};
use flare_render::metrics::{cell_metrics, grid_dims};
use flare_render::raster::{DEFAULT_FAMILIES, Rasterizer};
use flare_render::sample::sample_grid;

fn span(row: usize, col: usize, width: usize) -> RowSpan {
    RowSpan { row, col, width }
}

#[test]
fn gpu_and_cpu_agree_with_a_decor() {
    let Ok((device, queue, info)) = headless_device() else {
        eprintln!("skipping: no wgpu adapter");
        return;
    };
    eprintln!("adapter: {} ({:?})", info.name, info.backend);
    for scale in [1.0f32, 2.25] {
        let mut glyphs = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        glyphs.set_scale(scale);
        let w = (648.0 * scale).round() as u32;
        let h = (264.0 * scale).round() as u32;
        let m = cell_metrics(&glyphs.font_units(), scale);
        let (cols, rows) = grid_dims(w, h, &m);
        let mut grid = sample_grid(cols, rows);
        let decor = Decor {
            selection: vec![span(0, 2, 6), span(1, 0, 9)],
            find: vec![span(2, 1, 4), span(3, 5, 3)],
            find_active: vec![span(3, 5, 3)],
            links: vec![span(4, 0, 12)],
            hover: vec![span(4, 0, 12)],
            ink: None,
        };
        let scene = build_scene_decor(
            &mut grid,
            &Palette::pt_default(),
            &m,
            w,
            h,
            &decor,
            hair_px(scale),
        );
        let mut gpu = GpuPainter::new(&device, &queue);
        let tex = gpu.paint(&scene, &mut glyphs);
        let got = read_texture(&device, &queue, &tex);
        let mut cpu = RgbaImage::new(scene.width, scene.height);
        paint_scene(&scene, &mut glyphs, &mut cpu);
        let d = got
            .iter()
            .zip(&cpu.data)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(d <= 2, "scale {scale}: max channel diff {d}");
    }
}
