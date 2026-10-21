//! The software painter: paints a [`Scene`] into an RGBA byte buffer on the CPU.

use crate::blend::{blend_mask, blend_premul};
use crate::color::Rgba;
use crate::glyph::{GlyphKind, GlyphSource};
use crate::scene::{RectFill, Scene};

/// A row-major RGBA image with no row padding.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, RGBA per pixel.
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// A transparent black image of `width` x `height`.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * 4],
        }
    }

    /// The pixel at (`x`, `y`) as RGBA bytes. Panics when outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }
}

/// Paints `scene` into `out` (resized to scene.width x scene.height if different). Clips to the image.
pub fn paint_scene(scene: &Scene, glyphs: &mut dyn GlyphSource, out: &mut RgbaImage) {
    if out.width != scene.width || out.height != scene.height {
        *out = RgbaImage::new(scene.width, scene.height);
    }
    fill_all(out, scene.clear);
    for rect in &scene.rects {
        fill_rect(out, rect);
    }
    for draw in &scene.glyphs {
        let Some(bitmap) = glyphs.glyph(&draw.key) else {
            continue;
        };
        let origin_x = i64::from(draw.pen_x) + i64::from(bitmap.left);
        let origin_y = i64::from(draw.baseline_y) - i64::from(bitmap.top);
        let w = bitmap.width as usize;
        for j in 0..bitmap.height as usize {
            for i in 0..w {
                let x = origin_x + i as i64;
                let y = origin_y + j as i64;
                if !in_bounds(out, x, y) {
                    continue;
                }
                let idx = pixel_index(out, x, y);
                let dst = [
                    out.data[idx],
                    out.data[idx + 1],
                    out.data[idx + 2],
                    out.data[idx + 3],
                ];
                let px = match bitmap.kind {
                    GlyphKind::Mask => {
                        let cov = bitmap.data[j * w + i];
                        blend_mask(dst, draw.color, cov)
                    }
                    GlyphKind::Color => {
                        let s = (j * w + i) * 4;
                        let src = [
                            bitmap.data[s],
                            bitmap.data[s + 1],
                            bitmap.data[s + 2],
                            bitmap.data[s + 3],
                        ];
                        blend_premul(dst, src)
                    }
                };
                out.data[idx..idx + 4].copy_from_slice(&px);
            }
        }
    }
    for overlay in &scene.overlays {
        fill_rect(out, overlay);
    }
}

fn fill_all(out: &mut RgbaImage, color: Rgba) {
    let px = [color.r, color.g, color.b, color.a];
    for chunk in out.data.as_chunks_mut::<4>().0 {
        *chunk = px;
    }
}

/// Writes a solid rectangle, clipped to the image.
fn fill_rect(out: &mut RgbaImage, rect: &RectFill) {
    let px = [rect.color.r, rect.color.g, rect.color.b, rect.color.a];
    let x0 = i64::from(rect.x).max(0);
    let y0 = i64::from(rect.y).max(0);
    let x1 = (i64::from(rect.x) + i64::from(rect.w)).min(i64::from(out.width));
    let y1 = (i64::from(rect.y) + i64::from(rect.h)).min(i64::from(out.height));
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = pixel_index(out, x, y);
            out.data[idx..idx + 4].copy_from_slice(&px);
        }
    }
}

fn in_bounds(out: &RgbaImage, x: i64, y: i64) -> bool {
    x >= 0 && y >= 0 && x < i64::from(out.width) && y < i64::from(out.height)
}

fn pixel_index(out: &RgbaImage, x: i64, y: i64) -> usize {
    (y as usize * out.width as usize + x as usize) * 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Palette;
    use crate::glyph::{FakeGlyphs, GlyphKey};
    use crate::scene::GlyphDraw;

    const WHITE: Rgba = Rgba::rgb(255, 255, 255);
    const RED: Rgba = Rgba::rgb(255, 0, 0);

    fn empty_scene(w: u32, h: u32, clear: Rgba) -> Scene {
        Scene {
            width: w,
            height: h,
            clear,
            rects: vec![],
            glyphs: vec![],
            overlays: vec![],
        }
    }

    fn glyph(text: &str, pen_x: i32, baseline_y: i32, color: Rgba) -> GlyphDraw {
        GlyphDraw {
            key: GlyphKey {
                text: text.to_string(),
                bold: false,
                italic: false,
            },
            pen_x,
            baseline_y,
            color,
        }
    }

    fn rect(x: i32, y: i32, w: u32, h: u32, color: Rgba) -> RectFill {
        RectFill { x, y, w, h, color }
    }

    #[test]
    fn clear_fills_everything() {
        let p = Palette::pt_default();
        let scene = empty_scene(10, 10, p.bg);
        let mut out = RgbaImage::new(0, 0);
        paint_scene(&scene, &mut FakeGlyphs::new(4, 6), &mut out);
        assert_eq!(out.width, 10);
        assert_eq!(out.height, 10);
        for y in 0..10 {
            for x in 0..10 {
                assert_eq!(out.pixel(x, y), [0x17, 0x17, 0x17, 255]);
            }
        }
    }

    #[test]
    fn rect_is_exact() {
        let p = Palette::pt_default();
        let mut scene = empty_scene(10, 10, p.bg);
        scene.rects.push(rect(2, 3, 4, 5, RED));
        let mut out = RgbaImage::new(10, 10);
        paint_scene(&scene, &mut FakeGlyphs::new(4, 6), &mut out);
        assert_eq!(out.pixel(2, 3), [255, 0, 0, 255]);
        assert_eq!(out.pixel(5, 7), [255, 0, 0, 255]);
        assert_eq!(out.pixel(6, 7), [0x17, 0x17, 0x17, 255]);
        assert_eq!(out.pixel(1, 3), [0x17, 0x17, 0x17, 255]);
    }

    #[test]
    fn mask_glyph_lands_at_pen_minus_top() {
        let p = Palette::pt_default();
        let mut scene = empty_scene(20, 20, p.bg);
        scene.glyphs.push(glyph("A", 4, 16, WHITE));
        let mut out = RgbaImage::new(20, 20);
        paint_scene(&scene, &mut FakeGlyphs::new(4, 6), &mut out);
        assert_eq!(out.pixel(4, 10), [255, 255, 255, 255]);
        assert_eq!(out.pixel(4, 9), [0x17, 0x17, 0x17, 255]);
        assert_eq!(out.pixel(8, 10), [0x17, 0x17, 0x17, 255]);
    }

    #[test]
    fn color_glyph_blends_premultiplied() {
        let p = Palette::pt_default();
        let mut scene = empty_scene(10, 10, p.bg);
        scene.glyphs.push(glyph("😀", 0, 2, WHITE));
        let mut out = RgbaImage::new(10, 10);
        paint_scene(&scene, &mut FakeGlyphs::new(4, 6), &mut out);
        let expected = crate::blend::blend_premul([0x17, 0x17, 0x17, 255], [128, 0, 0, 128]);
        assert_eq!(out.pixel(0, 0), expected);
    }

    #[test]
    fn clips_negative_and_overflow() {
        let p = Palette::pt_default();
        let mut scene = empty_scene(10, 10, p.bg);
        scene.glyphs.push(glyph("A", -2, -2, WHITE));
        scene.glyphs.push(glyph("A", 9, 9, WHITE));
        scene.rects.push(rect(8, 8, 10, 10, RED));
        scene.rects.push(rect(-5, -5, 7, 7, RED));
        let mut out = RgbaImage::new(10, 10);
        paint_scene(&scene, &mut FakeGlyphs::new(4, 6), &mut out);
        assert_eq!(out.pixel(9, 9), [255, 0, 0, 255]);
        assert_eq!(out.pixel(1, 1), [255, 0, 0, 255]);
    }

    #[test]
    fn overlay_after_glyph() {
        let p = Palette::pt_default();
        let mut scene = empty_scene(10, 10, p.bg);
        scene.glyphs.push(glyph("A", 0, 6, WHITE));
        scene.overlays.push(rect(0, 0, 4, 6, RED));
        let mut out = RgbaImage::new(10, 10);
        paint_scene(&scene, &mut FakeGlyphs::new(4, 6), &mut out);
        assert_eq!(out.pixel(0, 0), [255, 0, 0, 255]);
    }
}
