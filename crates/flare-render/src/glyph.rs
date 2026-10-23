//! Glyph types: what a rasteriser hands the painters, and a deterministic fake for tests.

use crate::metrics::FontUnits;
use std::collections::HashMap;

/// Identifies one shaped cluster to rasterise: its text plus the style it is drawn in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

/// Whether a bitmap is coverage (tinted by the cell colour) or premultiplied colour (emoji).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphKind {
    Mask,
    Color,
}

/// A rasterised cluster. `left`/`top` place the bitmap relative to the pen at the baseline:
/// top-left pixel = (pen_x + left, baseline_y - top). Mask: `data.len() == w*h` coverage bytes.
/// Color: `data.len() == w*h*4`, premultiplied RGBA.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBitmap {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub kind: GlyphKind,
    pub data: Vec<u8>,
}

/// Bytes a glyph source holds, for the memory breakdown (M10 Task 66).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphMemory {
    /// The face files the font database holds (memory mapped: they are mostly not private).
    pub fonts: usize,
    /// Bitmaps and shaping caches on the heap.
    pub caches: usize,
}

/// The one interface both painters get glyph bitmaps through.
pub trait GlyphSource {
    /// None for clusters with nothing to draw (space) or that no font can draw.
    fn glyph(&mut self, key: &GlyphKey) -> Option<&GlyphBitmap>;
    fn font_units(&self) -> FontUnits;
    /// Drops cached bitmaps.
    fn set_scale(&mut self, scale: f32);
    /// Changes the face (first installed of `families`, else a monospace face) and the base size
    /// in logical pixels. Default: nothing to change.
    fn set_font(&mut self, _families: &[&str], _base_px: f32) {}
    /// A lazily found fallback font has arrived and pply_fallbacks has something to take
    /// (audit M2). Default: never.
    fn fallback_ready(&self) -> bool {
        false
    }
    /// What the lazy fallback worker calls (from its own thread) after it answers, so the window
    /// asks for a frame. Default: nothing to call.
    fn set_fallback_waker(&mut self, _wake: std::sync::Arc<dyn Fn() + Send + Sync>) {}
    /// Takes the fallback fonts found off the UI thread; true when cells drawn blank before must be
    /// painted again (the caller drops every atlas copy). Default: nothing.
    fn apply_fallbacks(&mut self) -> bool {
        false
    }
    /// What this source holds. Default: nothing counted.
    fn memory(&self) -> GlyphMemory {
        GlyphMemory::default()
    }
}

/// Deterministic test source: every cluster that is not empty and not all spaces is a solid `w x h`
/// mask at (left 0, top h as i32) with coverage 255, except clusters starting with '😀' which are a
/// 2x2 Color glyph at (left 0, top 2) whose pixels are premultiplied (128,0,0,128). Empty or
/// all-space clusters return None. Bold/italic do not change the fake bitmap. `font_units()` returns
/// the Cascadia numbers from Task 4; `set_scale` only clears the cache (fake sizes do not scale).
pub struct FakeGlyphs {
    pub w: u32,
    pub h: u32,
    cache: HashMap<GlyphKey, GlyphBitmap>,
}

impl FakeGlyphs {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            cache: HashMap::new(),
        }
    }

    fn make(&self, key: &GlyphKey) -> GlyphBitmap {
        if key.text.starts_with('😀') {
            return GlyphBitmap {
                left: 0,
                top: 2,
                width: 2,
                height: 2,
                kind: GlyphKind::Color,
                data: [128u8, 0, 0, 128].repeat(4),
            };
        }
        GlyphBitmap {
            left: 0,
            top: self.h as i32,
            width: self.w,
            height: self.h,
            kind: GlyphKind::Mask,
            data: vec![255u8; (self.w * self.h) as usize],
        }
    }
}

impl GlyphSource for FakeGlyphs {
    fn glyph(&mut self, key: &GlyphKey) -> Option<&GlyphBitmap> {
        if key.text.chars().all(|c| c == ' ') {
            return None;
        }
        if !self.cache.contains_key(key) {
            let bitmap = self.make(key);
            self.cache.insert(key.clone(), bitmap);
        }
        self.cache.get(key)
    }

    fn font_units(&self) -> FontUnits {
        FontUnits {
            units_per_em: 2048.0,
            ascent: 1900.0,
            descent: -480.0,
            leading: 0.0,
            advance: 1200.0,
        }
    }

    fn set_scale(&mut self, _scale: f32) {
        self.cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> GlyphKey {
        GlyphKey {
            text: text.to_string(),
            bold: false,
            italic: false,
        }
    }

    #[test]
    fn fake_space_is_none() {
        let mut fake = FakeGlyphs::new(6, 9);
        assert!(fake.glyph(&key(" ")).is_none());
        assert!(fake.glyph(&key("   ")).is_none());
        assert!(fake.glyph(&key("")).is_none());
    }

    #[test]
    fn fake_mask_is_solid() {
        let mut fake = FakeGlyphs::new(6, 9);
        let g = fake.glyph(&key("a")).expect("mask glyph");
        assert_eq!(g.kind, GlyphKind::Mask);
        assert_eq!(g.width, 6);
        assert_eq!(g.height, 9);
        assert_eq!(g.left, 0);
        assert_eq!(g.top, 9);
        assert_eq!(g.data.len(), 6 * 9);
        assert!(g.data.iter().all(|&b| b == 255));
    }

    #[test]
    fn fake_emoji_is_color() {
        let mut fake = FakeGlyphs::new(6, 9);
        let g = fake.glyph(&key("😀")).expect("colour glyph");
        assert_eq!(g.kind, GlyphKind::Color);
        assert_eq!(g.width, 2);
        assert_eq!(g.height, 2);
        assert_eq!(g.top, 2);
        assert_eq!(g.data, [128u8, 0, 0, 128].repeat(4));
    }

    #[test]
    fn fake_units_are_cascadia() {
        let fake = FakeGlyphs::new(6, 9);
        assert_eq!(fake.font_units().advance, 1200.0);
    }
}
