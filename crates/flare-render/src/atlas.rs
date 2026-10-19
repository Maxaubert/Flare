//! Glyph atlas: shelf packing of rasterised glyphs into one page, with a pending upload list.
//! Pure CPU bookkeeping; the GPU side copies each `Upload` into its texture.

use crate::glyph::{GlyphKey, GlyphKind, GlyphSource};
use std::collections::HashMap;

#[derive(Debug)]
struct Shelf {
    y: u32,
    height: u32,
    x_used: u32,
}

#[derive(Debug)]
pub struct ShelfPacker {
    width: u32,
    height: u32,
    shelves: Vec<Shelf>,
    padding: u32,
}

impl ShelfPacker {
    pub fn new(width: u32, height: u32, padding: u32) -> Self {
        Self {
            width,
            height,
            shelves: Vec::new(),
            padding,
        }
    }

    /// Best-fit shelf (smallest height >= h that has room), else a new shelf; None when full.
    pub fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let fw = w + self.padding;
        let fh = h + self.padding;
        let mut best: Option<usize> = None;
        for (i, shelf) in self.shelves.iter().enumerate() {
            if shelf.x_used + fw <= self.width && fh <= shelf.height {
                match best {
                    Some(b) if self.shelves[b].height <= shelf.height => {}
                    _ => best = Some(i),
                }
            }
        }
        if let Some(i) = best {
            let shelf = &mut self.shelves[i];
            let pos = (shelf.x_used, shelf.y);
            shelf.x_used += fw;
            return Some(pos);
        }
        let y: u32 = self.shelves.iter().map(|s| s.height).sum();
        if y + fh <= self.height && fw <= self.width {
            self.shelves.push(Shelf {
                y,
                height: fh,
                x_used: fw,
            });
            return Some((0, y));
        }
        None
    }

    pub fn clear(&mut self) {
        self.shelves.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasSlot {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub left: i32,
    pub top: i32,
    pub color: bool,
}

#[derive(Debug, Clone)]
pub struct Upload {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AtlasError {
    Full,
    TooLarge,
}

pub struct GlyphAtlas {
    packer: ShelfPacker,
    slots: HashMap<GlyphKey, Option<AtlasSlot>>,
    pending: Vec<Upload>,
    pub generation: u64,
}

impl GlyphAtlas {
    pub const SIZE: u32 = 2048;
    const PADDING: u32 = 1;

    pub fn new() -> Self {
        Self {
            packer: ShelfPacker::new(Self::SIZE, Self::SIZE, Self::PADDING),
            slots: HashMap::new(),
            pending: Vec::new(),
            generation: 0,
        }
    }

    /// Looks up or inserts the glyph (rasterised through `src`). Ok(None) for nothing to draw.
    /// Mask bitmaps upload as (255,255,255,cov); Color as-is. Err(Full) leaves the atlas unchanged.
    pub fn get(
        &mut self,
        key: &GlyphKey,
        src: &mut dyn GlyphSource,
    ) -> Result<Option<AtlasSlot>, AtlasError> {
        if let Some(slot) = self.slots.get(key) {
            return Ok(*slot);
        }
        let Some(bitmap) = src.glyph(key) else {
            self.slots.insert(key.clone(), None);
            return Ok(None);
        };
        let (w, h) = (bitmap.width, bitmap.height);
        if w == 0 || h == 0 {
            self.slots.insert(key.clone(), None);
            return Ok(None);
        }
        if w + Self::PADDING > Self::SIZE || h + Self::PADDING > Self::SIZE {
            return Err(AtlasError::TooLarge);
        }
        let (x, y) = self.packer.alloc(w, h).ok_or(AtlasError::Full)?;
        let color = bitmap.kind == GlyphKind::Color;
        let rgba = match bitmap.kind {
            GlyphKind::Mask => bitmap
                .data
                .iter()
                .flat_map(|&c| [255u8, 255, 255, c])
                .collect(),
            GlyphKind::Color => bitmap.data.clone(),
        };
        let slot = AtlasSlot {
            x,
            y,
            w,
            h,
            left: bitmap.left,
            top: bitmap.top,
            color,
        };
        self.pending.push(Upload { x, y, w, h, rgba });
        self.slots.insert(key.clone(), Some(slot));
        Ok(Some(slot))
    }

    /// Bytes the CPU side holds: the slot table and the uploads not yet taken.
    pub fn cpu_bytes(&self) -> usize {
        let per_slot = size_of::<GlyphKey>() + size_of::<Option<AtlasSlot>>();
        let keys: usize = self.slots.keys().map(|k| k.text.capacity()).sum();
        let pending: usize = self.pending.iter().map(|u| u.rgba.capacity()).sum();
        self.slots.capacity() * per_slot + keys + pending
    }

    /// Bytes of the atlas texture on the GPU (RGBA, one page).
    pub const fn gpu_bytes() -> usize {
        Self::SIZE as usize * Self::SIZE as usize * 4
    }

    pub fn take_uploads(&mut self) -> Vec<Upload> {
        std::mem::take(&mut self.pending)
    }

    /// Empties slots and packer and bumps `generation` (the GPU side re-uploads nothing; new gets refill).
    pub fn clear(&mut self) {
        self.packer.clear();
        self.slots.clear();
        self.pending.clear();
        self.generation += 1;
    }
}

impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::FakeGlyphs;

    fn key(text: &str) -> GlyphKey {
        GlyphKey {
            text: text.to_string(),
            bold: false,
            italic: false,
        }
    }

    #[test]
    fn packs_left_to_right_then_new_shelf() {
        let mut p = ShelfPacker::new(100, 100, 0);
        assert_eq!(p.alloc(40, 10), Some((0, 0)));
        assert_eq!(p.alloc(40, 10), Some((40, 0)));
        assert_eq!(p.alloc(40, 10), Some((0, 10)));
    }

    #[test]
    fn padding_is_respected() {
        let mut p = ShelfPacker::new(100, 100, 1);
        assert_eq!(p.alloc(10, 10), Some((0, 0)));
        assert_eq!(p.alloc(10, 10), Some((11, 0)));
    }

    #[test]
    fn best_fit_reuses_shorter_shelf() {
        let mut p = ShelfPacker::new(100, 100, 0);
        assert_eq!(p.alloc(95, 10), Some((0, 0)));
        assert_eq!(p.alloc(10, 30), Some((0, 10)));
        assert_eq!(p.alloc(10, 8), Some((10, 10)));
        assert_eq!(p.alloc(5, 8), Some((95, 0)));
    }

    #[test]
    fn full_returns_none() {
        let mut p = ShelfPacker::new(20, 16, 0);
        assert_eq!(p.alloc(10, 10), Some((0, 0)));
        assert_eq!(p.alloc(10, 10), Some((10, 0)));
        assert_eq!(p.alloc(10, 10), None);
    }

    #[test]
    fn too_large_is_error() {
        let mut atlas = GlyphAtlas::new();
        let mut fake = FakeGlyphs::new(3000, 10);
        assert_eq!(atlas.get(&key("a"), &mut fake), Err(AtlasError::TooLarge));
    }

    #[test]
    fn get_is_cached() {
        let mut atlas = GlyphAtlas::new();
        let mut fake = FakeGlyphs::new(6, 9);
        let a = atlas.get(&key("a"), &mut fake).unwrap();
        assert_eq!(atlas.take_uploads().len(), 1);
        let b = atlas.get(&key("a"), &mut fake).unwrap();
        assert_eq!(a, b);
        assert!(atlas.take_uploads().is_empty());
    }

    #[test]
    fn mask_uploads_white_with_coverage() {
        let mut atlas = GlyphAtlas::new();
        let mut fake = FakeGlyphs::new(2, 2);
        let slot = atlas.get(&key("a"), &mut fake).unwrap().unwrap();
        assert!(!slot.color);
        assert_eq!((slot.w, slot.h, slot.top), (2, 2, 2));
        let ups = atlas.take_uploads();
        assert_eq!(ups.len(), 1);
        assert_eq!((ups[0].x, ups[0].y, ups[0].w, ups[0].h), (0, 0, 2, 2));
        assert_eq!(ups[0].rgba, [255u8, 255, 255, 255].repeat(4));
    }

    #[test]
    fn color_uploads_verbatim() {
        let mut atlas = GlyphAtlas::new();
        let mut fake = FakeGlyphs::new(6, 9);
        let slot = atlas.get(&key("😀"), &mut fake).unwrap().unwrap();
        assert!(slot.color);
        let ups = atlas.take_uploads();
        assert_eq!(ups[0].rgba, [128u8, 0, 0, 128].repeat(4));
    }

    #[test]
    fn clear_bumps_generation_and_refills() {
        let mut atlas = GlyphAtlas::new();
        let mut fake = FakeGlyphs::new(6, 9);
        atlas.get(&key("a"), &mut fake).unwrap();
        atlas.take_uploads();
        assert_eq!(atlas.generation, 0);
        atlas.clear();
        assert_eq!(atlas.generation, 1);
        let slot = atlas.get(&key("a"), &mut fake).unwrap().unwrap();
        assert_eq!((slot.x, slot.y), (0, 0));
        assert_eq!(atlas.take_uploads().len(), 1);
    }

    #[test]
    fn the_atlas_counts_its_bytes() {
        let mut atlas = GlyphAtlas::new();
        let before = atlas.cpu_bytes();
        let mut fake = FakeGlyphs::new(6, 9);
        atlas.get(&key("a"), &mut fake).unwrap();
        // The slot table and the upload waiting both count.
        assert!(atlas.cpu_bytes() > before + 6 * 9 * 4);
        assert_eq!(GlyphAtlas::gpu_bytes(), 2048 * 2048 * 4);
    }

    #[test]
    fn space_is_ok_none() {
        let mut atlas = GlyphAtlas::new();
        let mut fake = FakeGlyphs::new(6, 9);
        assert_eq!(atlas.get(&key(" "), &mut fake), Ok(None));
        assert!(atlas.take_uploads().is_empty());
    }
}
