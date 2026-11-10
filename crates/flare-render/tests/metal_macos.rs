//! macOS: the GPU painter draws on Metal, offscreen, with no window (port phase 2, step 4).
//!
//! `gpu_parity.rs` skips when no adapter exists; this one does not: a Mac always has Metal, so a
//! missing adapter here is a failure, not a skip.
#![cfg(target_os = "macos")]

use flare_render::color::Palette;
use flare_render::cpu::{RgbaImage, paint_scene};
use flare_render::glyph::GlyphSource;
use flare_render::gpu::{GpuPainter, NATIVE_BACKEND, headless_device, read_texture};
use flare_render::metrics::{cell_metrics, grid_dims};
use flare_render::raster::{DEFAULT_FAMILIES, Rasterizer};
use flare_render::sample::sample_grid;
use flare_render::scene::build_scene;

#[test]
fn the_native_backend_is_metal() {
    assert_eq!(NATIVE_BACKEND, wgpu::Backends::METAL);
}

#[test]
fn a_metal_device_paints_one_terminal_frame_offscreen_like_the_cpu_painter() {
    let (device, queue, info) = headless_device().expect("a Mac has a Metal adapter");
    assert_eq!(info.backend, wgpu::Backend::Metal, "{info:?}");

    // One frame of the sample grid in the macOS default face, at 100 and 200 percent (Retina).
    for scale in [1.0_f32, 2.0] {
        let mut glyphs = Rasterizer::new(DEFAULT_FAMILIES, scale);
        let m = cell_metrics(&glyphs.font_units(), scale);
        let w = (648.0 * scale).round() as u32;
        let h = (264.0 * scale).round() as u32;
        let (cols, rows) = grid_dims(w, h, &m);
        let scene = build_scene(&sample_grid(cols, rows), &Palette::pt_default(), &m, w, h);
        assert!(!scene.glyphs.is_empty(), "the frame has text at {scale}");

        let mut gpu = GpuPainter::new(&device, &queue);
        let tex = gpu.paint(&scene, &mut glyphs);
        assert_eq!((tex.width(), tex.height()), (w, h));
        let got = read_texture(&device, &queue, &tex);
        assert_eq!(got.len(), (w * h * 4) as usize);

        let mut cpu = RgbaImage::new(w, h);
        paint_scene(&scene, &mut glyphs, &mut cpu);
        let diff = got
            .iter()
            .zip(&cpu.data)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(diff <= 2, "max channel diff {diff} at {scale}");

        // Not a blank frame: the glyphs put ink of their own over the ground.
        let ground = Palette::pt_default().bg;
        let ink = got
            .chunks(4)
            .filter(|p| p[..3] != [ground.r, ground.g, ground.b])
            .count();
        assert!(ink > 500, "{ink} inked pixels at {scale}");
    }
}
