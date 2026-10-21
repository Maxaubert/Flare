//! Rasteriser: cosmic-text shapes a cluster, swash rasterises its glyphs, and the result is handed
//! to the painters through `GlyphSource`.

use crate::blend::blend_premul;
use crate::glyph::{GlyphBitmap, GlyphKey, GlyphKind, GlyphSource};
use crate::metrics::{FontUnits, font_px};
use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent,
    SwashImage, Weight,
};
use std::collections::HashMap;

/// PT's `TERM_FONTS` 'cascadia' stack, in order of preference.
pub const DEFAULT_FAMILIES: &[&str] = &["Cascadia Mono", "JetBrainsMono NF", "Consolas"];

pub struct Rasterizer {
    fs: FontSystem,
    swash: SwashCache,
    family: String,
    scale: f32,
    units: FontUnits,
    cache: HashMap<GlyphKey, Option<GlyphBitmap>>,
}

impl Rasterizer {
    /// Loads system fonts once (about 60 ms), picks the first family in `families` that exists
    /// (else the first monospace face), measures FontUnits from it.
    pub fn new(families: &[&str], scale: f32) -> Self {
        let mut fs = FontSystem::new();
        let family = families
            .iter()
            .find(|name| has_family(&fs, name))
            .map(|name| name.to_string())
            .or_else(|| {
                fs.db()
                    .faces()
                    .find(|f| f.monospaced)
                    .and_then(|f| f.families.first())
                    .map(|(name, _)| name.clone())
            })
            .unwrap_or_else(|| "monospace".to_string());
        let units = measure(&mut fs, &family);
        Self {
            fs,
            swash: SwashCache::new(),
            family,
            scale,
            units,
            cache: HashMap::new(),
        }
    }

    pub fn family(&self) -> &str {
        &self.family
    }

    fn rasterize(&mut self, key: &GlyphKey) -> Option<GlyphBitmap> {
        if key.text.chars().all(|c| c == ' ') {
            return None;
        }
        let px = font_px(self.scale);
        let mut attrs = Attrs::new().family(Family::Name(&self.family));
        if key.bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if key.italic {
            attrs = attrs.style(Style::Italic);
        }
        let mut buffer = Buffer::new_empty(Metrics::new(px, px));
        buffer.set_text(&key.text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);
        let run = buffer.layout_runs().next()?;

        // Every glyph of the first run, with its physical position and rasterised image.
        let mut images: Vec<(i32, i32, SwashImage)> = Vec::new();
        for glyph in run.glyphs.iter() {
            let physical = glyph.physical((0.0, 0.0), 1.0);
            if let Some(image) = self
                .swash
                .get_image(&mut self.fs, physical.cache_key)
                .clone()
            {
                images.push((physical.x, physical.y, image));
            }
        }
        compose(&images)
    }
}

impl GlyphSource for Rasterizer {
    fn glyph(&mut self, key: &GlyphKey) -> Option<&GlyphBitmap> {
        if !self.cache.contains_key(key) {
            let bitmap = self.rasterize(key);
            self.cache.insert(key.clone(), bitmap);
        }
        self.cache.get(key).and_then(|bitmap| bitmap.as_ref())
    }

    fn font_units(&self) -> FontUnits {
        self.units
    }

    fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
        self.cache.clear();
    }
}

/// Whether the font database has a face with this family name.
fn has_family(fs: &FontSystem, name: &str) -> bool {
    fs.db()
        .faces()
        .any(|f| f.families.iter().any(|(n, _)| n == name))
}

/// Shapes "M" at the base size and reads the font's metrics, in font units.
fn measure(fs: &mut FontSystem, family: &str) -> FontUnits {
    let px = font_px(1.0);
    let attrs = Attrs::new().family(Family::Name(family));
    let mut buffer = Buffer::new_empty(Metrics::new(px, px));
    buffer.set_text("M", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fs, false);
    let run = buffer.layout_runs().next().expect("a layout run for M");
    let glyph = &run.glyphs[0];
    let font = fs
        .get_font(glyph.font_id, Weight::NORMAL)
        .expect("the font that shaped M");
    let metrics = font.metrics();
    let upem = metrics.units_per_em as f32;
    FontUnits {
        units_per_em: upem,
        ascent: metrics.ascent,
        descent: metrics.descent,
        leading: metrics.leading,
        advance: glyph.w / px * upem,
    }
}

/// Composes the glyph images of one cluster into one bitmap. Positions are relative to the pen at
/// the baseline: `left` is pixels right of the pen, `top` is pixels above the baseline. Any colour
/// image makes the result Color (premultiplied RGBA, masks become white with their coverage);
/// otherwise the result is a coverage mask. Overlapping images are composed source-over.
fn compose(images: &[(i32, i32, SwashImage)]) -> Option<GlyphBitmap> {
    // Each image as (left, top, width, height) in pen-relative pixels, y up.
    let boxes: Vec<(i32, i32, u32, u32)> = images
        .iter()
        .map(|(x, y, image)| {
            let p = image.placement;
            (x + p.left, -y + p.top, p.width, p.height)
        })
        .collect();
    let left = boxes.iter().map(|b| b.0).min()?;
    let top = boxes.iter().map(|b| b.1).max()?;
    let right = boxes.iter().map(|b| b.0 + b.2 as i32).max()?;
    let bottom = boxes.iter().map(|b| b.1 - b.3 as i32).min()?;
    let width = (right - left) as u32;
    let height = (top - bottom) as u32;
    if width == 0 || height == 0 {
        return None;
    }

    let any_color = images
        .iter()
        .any(|(_, _, image)| image.content == SwashContent::Color);
    let mut canvas = vec![0u8; (width * height * 4) as usize];
    for ((_, _, image), b) in images.iter().zip(&boxes) {
        let (w, h) = (b.2 as usize, b.3 as usize);
        let origin_x = (b.0 - left) as usize;
        let origin_y = (top - b.1) as usize;
        for j in 0..h {
            for i in 0..w {
                let src = match image.content {
                    SwashContent::Mask => {
                        let c = image.data[j * w + i];
                        [c, c, c, c]
                    }
                    SwashContent::Color => {
                        let at = (j * w + i) * 4;
                        [
                            image.data[at],
                            image.data[at + 1],
                            image.data[at + 2],
                            image.data[at + 3],
                        ]
                    }
                    SwashContent::SubpixelMask => {
                        let at = (j * w + i) * 4;
                        let c = ((image.data[at] as u32
                            + image.data[at + 1] as u32
                            + image.data[at + 2] as u32)
                            / 3) as u8;
                        [c, c, c, c]
                    }
                };
                let at = ((origin_y + j) * width as usize + origin_x + i) * 4;
                let dst = [canvas[at], canvas[at + 1], canvas[at + 2], canvas[at + 3]];
                let out = blend_premul(dst, src);
                canvas[at..at + 4].copy_from_slice(&out);
            }
        }
    }

    let (kind, data) = if any_color {
        (GlyphKind::Color, canvas)
    } else {
        (
            GlyphKind::Mask,
            canvas.iter().skip(3).step_by(4).copied().collect(),
        )
    };
    Some(GlyphBitmap {
        left,
        top,
        width,
        height,
        kind,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::GlyphKind;

    fn key(text: &str) -> GlyphKey {
        GlyphKey {
            text: text.to_string(),
            bold: false,
            italic: false,
        }
    }

    fn bold_key(text: &str) -> GlyphKey {
        GlyphKey {
            text: text.to_string(),
            bold: true,
            italic: false,
        }
    }

    /// Whether the system has a face with this family name (the test-side check, so tests can skip
    /// on machines without the font).
    fn installed(name: &str) -> bool {
        let fs = FontSystem::new();
        fs.db()
            .faces()
            .any(|f| f.families.iter().any(|(n, _)| n == name))
    }

    #[test]
    fn picks_cascadia_when_installed() {
        let r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        if !installed("Cascadia Mono") {
            eprintln!("skipped: Cascadia Mono is not installed");
            return;
        }
        assert_eq!(r.family(), "Cascadia Mono");
        let u = r.font_units();
        assert_eq!(u.units_per_em, 2048.0);
        assert!((u.ascent - 1900.0).abs() <= 0.5);
        assert!((u.descent - -480.0).abs() <= 0.5);
        assert!(u.leading.abs() <= 0.5);
        assert!((u.advance - 1200.0).abs() <= 1.0);
    }

    #[test]
    fn ascii_is_mask_with_ink() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        let g = r.glyph(&key("M")).expect("M has a bitmap");
        assert_eq!(g.kind, GlyphKind::Mask);
        assert!(g.data.iter().any(|&b| b > 200), "M has solid ink");
    }

    #[test]
    fn space_is_none() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        assert!(r.glyph(&key(" ")).is_none());
        assert!(r.glyph(&key("")).is_none());
    }

    #[test]
    fn cjk_falls_back() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        let m_width = r.glyph(&key("M")).expect("M").width;
        match r.glyph(&key("中")).map(|g| g.width) {
            Some(w) => assert!(w > m_width, "CJK is wider than M"),
            None => {
                if installed("Microsoft YaHei UI") {
                    panic!("a CJK fallback font is installed but 中 did not draw");
                }
                eprintln!("skipped: no CJK fallback font on this machine");
            }
        }
    }

    #[test]
    fn emoji_is_color() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        match r.glyph(&key("😀")).map(|g| g.kind) {
            Some(kind) => assert_eq!(kind, GlyphKind::Color),
            None => {
                if installed("Segoe UI Emoji") {
                    panic!("Segoe UI Emoji is installed but the emoji did not draw");
                }
                eprintln!("skipped: no Segoe UI Emoji on this machine");
            }
        }
    }

    #[test]
    fn bold_differs_from_regular() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        let regular = r.glyph(&key("M")).expect("regular M").clone();
        let bold = r.glyph(&bold_key("M")).expect("bold M").clone();
        assert_ne!(regular.data, bold.data);
    }

    #[test]
    fn scale_changes_size() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        let h1 = r.glyph(&key("M")).expect("M at 1.0").height;
        r.set_scale(2.25);
        let h2 = r.glyph(&key("M")).expect("M at 2.25").height;
        assert!(
            h2 >= 2 * h1,
            "2.25x M is at least twice as tall ({h1} -> {h2})"
        );
    }
}
