//! Headless GPU painter against the CPU painter. Each test skips when no adapter exists.

use flare_render::color::Palette;
use flare_render::cpu::{RgbaImage, paint_scene};
use flare_render::glyph::{FakeGlyphs, GlyphKey, GlyphSource};
use flare_render::gpu::{GpuPainter, headless_device, read_texture};
use flare_render::metrics::{cell_metrics, grid_dims};
use flare_render::raster::{DEFAULT_FAMILIES, Rasterizer};
use flare_render::sample::sample_grid;
use flare_render::scene::{
    GlyphDraw, GroundPaint, RectFill, Scene, build_scene, build_scene_ground,
};

macro_rules! device_or_skip {
    () => {{
        let Ok((device, queue, info)) = headless_device() else {
            eprintln!("skipping: no wgpu adapter");
            return;
        };
        eprintln!("adapter: {} ({:?})", info.name, info.backend);
        (device, queue)
    }};
}

fn max_diff(a: &[u8], b: &[u8]) -> u8 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

fn sample_scene_ground(
    glyphs: &dyn GlyphSource,
    scale: f32,
    w: u32,
    h: u32,
    ground: GroundPaint,
) -> Scene {
    let m = cell_metrics(&glyphs.font_units(), scale);
    let (cols, rows) = grid_dims(w, h, &m);
    if ground == GroundPaint::InGrid {
        return build_scene(&sample_grid(cols, rows), &Palette::pt_default(), &m, w, h);
    }
    build_scene_ground(
        &sample_grid(cols, rows),
        &Palette::pt_default(),
        &m,
        w,
        h,
        ground,
    )
}

fn empty_scene(w: u32, h: u32) -> Scene {
    Scene {
        width: w,
        height: h,
        clear: Palette::pt_default().bg,
        rects: vec![],
        glyphs: vec![],
        overlays: vec![],
    }
}

fn parity(glyphs: &mut dyn GlyphSource, scale: f32) {
    parity_ground(glyphs, scale, GroundPaint::InGrid);
}

fn parity_ground(glyphs: &mut dyn GlyphSource, scale: f32, ground: GroundPaint) {
    let (device, queue) = device_or_skip!();
    glyphs.set_scale(scale);
    let w = (648.0 * scale).round() as u32;
    let h = (264.0 * scale).round() as u32;
    let scene = sample_scene_ground(glyphs, scale, w, h, ground);
    let mut gpu = GpuPainter::new(&device, &queue);
    let tex = gpu.paint(&scene, glyphs);
    let got = read_texture(&device, &queue, &tex);
    let mut cpu = RgbaImage::new(scene.width, scene.height);
    paint_scene(&scene, glyphs, &mut cpu);
    let d = max_diff(&got, &cpu.data);
    assert!(d <= 2, "max channel diff {d}");
}

#[test]
fn clear_colour_reads_back() {
    let (device, queue) = device_or_skip!();
    let mut gpu = GpuPainter::new(&device, &queue);
    let tex = gpu.paint(&empty_scene(64, 32), &mut FakeGlyphs::new(6, 12));
    let px = read_texture(&device, &queue, &tex);
    assert_eq!(px.len(), 64 * 32 * 4);
    assert!(px.chunks(4).all(|p| p == [0x17, 0x17, 0x17, 255]));
}

#[test]
fn rects_match_cpu_exactly() {
    let (device, queue) = device_or_skip!();
    let p = Palette::pt_default();
    let mut scene = empty_scene(64, 32);
    scene.rects = vec![
        RectFill {
            x: 2,
            y: 3,
            w: 10,
            h: 8,
            color: p.cursor,
        },
        RectFill {
            x: 40,
            y: 20,
            w: 100,
            h: 100,
            color: p.fg,
        },
    ];
    scene.overlays = vec![RectFill {
        x: 0,
        y: 30,
        w: 64,
        h: 1,
        color: p.cursor_accent,
    }];
    let mut fake = FakeGlyphs::new(6, 12);
    let mut gpu = GpuPainter::new(&device, &queue);
    let tex = gpu.paint(&scene, &mut fake);
    let got = read_texture(&device, &queue, &tex);
    let mut cpu = RgbaImage::new(64, 32);
    paint_scene(&scene, &mut fake, &mut cpu);
    assert_eq!(got, cpu.data);
}

#[test]
fn fake_glyphs_match_cpu_within_2() {
    parity(&mut FakeGlyphs::new(6, 12), 1.0);
}

#[test]
fn real_glyphs_match_cpu_within_2() {
    parity(&mut Rasterizer::new(DEFAULT_FAMILIES, 1.0), 1.0);
}

#[test]
fn real_glyphs_match_cpu_within_2_at_225_percent() {
    parity(&mut Rasterizer::new(DEFAULT_FAMILIES, 1.0), 2.25);
}

#[test]
fn gpu_matches_cpu_on_a_transparent_ground() {
    for scale in [1.0, 2.25] {
        parity_ground(
            &mut Rasterizer::new(DEFAULT_FAMILIES, 1.0),
            scale,
            GroundPaint::ByUi,
        );
    }
}

/// Slint reads an imported texture as straight alpha (docs/material.md E3): under glass the grid
/// goes through the un-premultiply pass; an opaque grid is handed over as painted.
#[test]
fn paint_straight_unpremultiplies_a_see_through_grid_only() {
    let (device, queue) = device_or_skip!();
    let mut fake = FakeGlyphs::new(6, 12);
    let mut gpu = GpuPainter::new(&device, &queue);
    let mut scene = empty_scene(32, 16);
    scene.clear = flare_render::color::Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    // A premultiplied half-alpha fill, as the painter leaves a half-covered glyph pixel.
    scene.rects.push(RectFill {
        x: 0,
        y: 0,
        w: 8,
        h: 8,
        color: flare_render::color::Rgba {
            r: 64,
            g: 32,
            b: 16,
            a: 128,
        },
    });
    let tex = gpu.paint_straight(&scene, &mut fake);
    let got = read_texture(&device, &queue, &tex);
    let px = |x: usize, y: usize| &got[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
    for (g, want) in px(2, 2).iter().zip([128u8, 64, 32, 128]) {
        assert!(g.abs_diff(want) <= 1, "{:?}", px(2, 2));
    }
    assert_eq!(px(20, 10), [0, 0, 0, 0], "a clear pixel stays clear");

    // Opaque: the same bytes as the painted grid.
    let opaque = empty_scene(32, 16);
    let a = read_texture(&device, &queue, &gpu.paint(&opaque, &mut fake));
    let b = read_texture(&device, &queue, &gpu.paint_straight(&opaque, &mut fake));
    assert_eq!(a, b);
}

#[test]
fn texture_is_importable() {
    let (device, queue) = device_or_skip!();
    let mut gpu = GpuPainter::new(&device, &queue);
    let tex = gpu.paint(&empty_scene(64, 32), &mut FakeGlyphs::new(6, 12));
    assert_eq!(tex.format(), wgpu::TextureFormat::Rgba8Unorm);
    assert!(
        tex.usage().contains(
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT
        )
    );
}

#[test]
fn resize_recreates_target() {
    let (device, queue) = device_or_skip!();
    let mut fake = FakeGlyphs::new(6, 12);
    let mut gpu = GpuPainter::new(&device, &queue);
    let a = gpu.paint(&empty_scene(64, 32), &mut fake);
    assert_eq!((a.width(), a.height()), (64, 32));
    let b = gpu.paint(&empty_scene(80, 40), &mut fake);
    assert_eq!((b.width(), b.height()), (80, 40));
}

#[test]
fn atlas_full_recovers() {
    let (device, queue) = device_or_skip!();
    let p = Palette::pt_default();
    let mut scene = empty_scene(2048, 2048);
    scene.glyphs = (0..3000)
        .map(|i| GlyphDraw {
            key: GlyphKey {
                text: format!("g{i}"),
                bold: false,
                italic: false,
            },
            pen_x: (i % 50) * 40,
            baseline_y: (i / 50) * 34 + 30,
            color: p.fg,
        })
        .collect();
    let mut gpu = GpuPainter::new(&device, &queue);
    let tex = gpu.paint(&scene, &mut FakeGlyphs::new(40, 40));
    let px = read_texture(&device, &queue, &tex);
    assert_eq!(px.len(), 2048 * 2048 * 4);
    // Glyph 0 alone covers (0,0); it has a valid slot only if the rebuild after the clear worked.
    assert_eq!(px[..4], [p.fg.r, p.fg.g, p.fg.b, 255]);
}

#[test]
fn the_statusline_pill_paints_the_same_on_both_painters() {
    // Owner bug 2026-10-21: chevron, left cap, bar cell, right cap, through the real rasteriser.
    let (device, queue) = device_or_skip!();
    for scale in [1.0f32, 1.5, 2.25, 3.0] {
        let mut glyphs = Rasterizer::new(DEFAULT_FAMILIES, scale);
        glyphs.set_scale(scale);
        let m = cell_metrics(&glyphs.font_units(), scale);
        let (w, h) = (m.cell_w * 6 + 2 * m.padding, m.cell_h + 2 * m.padding);
        let blue = flare_render::color::Rgba::rgb(64, 160, 255);
        let mut scene = empty_scene(w, h);
        let draw = |text: &str, col: u32| GlyphDraw {
            key: GlyphKey {
                text: text.to_string(),
                bold: false,
                italic: false,
            },
            pen_x: (m.padding + col * m.cell_w) as i32,
            baseline_y: (m.padding + m.baseline) as i32,
            color: blue,
        };
        scene.rects.push(RectFill {
            x: (m.padding + 2 * m.cell_w) as i32,
            y: m.padding as i32,
            w: m.cell_w,
            h: m.cell_h,
            color: blue,
        });
        scene.glyphs.push(draw("\u{E0B1}", 0));
        scene.glyphs.push(draw("\u{E0B6}", 1));
        scene.glyphs.push(draw("\u{E0B4}", 3));
        let mut gpu = GpuPainter::new(&device, &queue);
        let tex = gpu.paint(&scene, &mut glyphs);
        let got = read_texture(&device, &queue, &tex);
        let mut cpu = RgbaImage::new(w, h);
        paint_scene(&scene, &mut glyphs, &mut cpu);
        let d = max_diff(&got, &cpu.data);
        assert!(d <= 2, "pill at {scale}: max channel diff {d}");
        // The cap's flat side meets the bar cell: no ground-coloured column between them.
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [cpu.data[i], cpu.data[i + 1], cpu.data[i + 2]]
        };
        let bar_left = m.padding + 2 * m.cell_w;
        let y = m.padding + m.cell_h / 2;
        for dx in [
            bar_left - 1,
            bar_left,
            bar_left + m.cell_w - 1,
            bar_left + m.cell_w,
        ] {
            let p = at(dx, y);
            assert!(
                p[2] > 240 && p[0] < 80,
                "seam at x {dx} scale {scale}: {p:?}"
            );
        }
    }
}
