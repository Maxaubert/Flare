//! Rasteriser: cosmic-text shapes a cluster, swash rasterises its glyphs, and the result is handed
//! to the painters through `GlyphSource`.

use crate::blend::blend_premul;
use crate::font_catalog::{
    FALLBACK_FAMILIES, LAZY_FALLBACK_FAMILIES, database_for, files_for_family, installed_faces,
};
use crate::glyph::{GlyphBitmap, GlyphKey, GlyphKind, GlyphMemory, GlyphSource};
use crate::metrics::{BASE_FONT_PX, FontUnits, font_px_for};
use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent,
    SwashImage, Weight,
};
use std::collections::HashMap;

/// The locale the font system is built with (the fallback lists pick Han fonts by it).
const LOCALE: &str = "en-US";

/// PT's `TERM_FONTS` 'cascadia' stack, in order of preference.
pub const DEFAULT_FAMILIES: &[&str] = &["Cascadia Mono", "JetBrainsMono NF", "Consolas"];

pub struct Rasterizer {
    fs: FontSystem,
    swash: SwashCache,
    family: String,
    scale: f32,
    base_px: f32,
    generation: u64,
    units: FontUnits,
    cache: HashMap<GlyphKey, Option<GlyphBitmap>>,
    /// The lazy fallback families already asked for (loaded or found useless), once each.
    tried_fallbacks: Vec<&'static str>,
    /// The ones that were kept, so a rebuilt database brings them back.
    kept_fallbacks: Vec<&'static str>,
    /// (faces, bytes) of the last `font_bytes` count.
    font_bytes: std::cell::Cell<Option<(usize, usize)>>,
}

impl Rasterizer {
    /// Loads only the faces the terminal needs (the stack plus `FALLBACK_FAMILIES`, found through
    /// the registry's font list), picks the first family in `families` that exists (else the first
    /// monospace face), measures FontUnits from it.
    pub fn new(families: &[&str], scale: f32) -> Self {
        let db = database_for(families, FALLBACK_FAMILIES);
        Self::from_system(
            FontSystem::new_with_locale_and_db(LOCALE.into(), db),
            families,
            scale,
        )
    }

    fn from_system(mut fs: FontSystem, families: &[&str], scale: f32) -> Self {
        let family = pick_family(&fs, families);
        let units = measure(&mut fs, &family, BASE_FONT_PX);
        Self {
            fs,
            swash: SwashCache::new(),
            family,
            scale,
            base_px: BASE_FONT_PX,
            generation: 0,
            units,
            cache: HashMap::new(),
            tried_fallbacks: Vec::new(),
            kept_fallbacks: Vec::new(),
            font_bytes: std::cell::Cell::new(None),
        }
    }

    /// Bytes of the distinct face files the database holds, read once per database change.
    fn font_bytes(&self) -> usize {
        let faces = self.fs.db().len();
        if let Some((n, bytes)) = self.font_bytes.get()
            && n == faces
        {
            return bytes;
        }
        let mut seen: Vec<&std::path::Path> = Vec::new();
        let mut total = 0usize;
        for face in self.fs.db().faces() {
            match &face.source {
                cosmic_text::fontdb::Source::File(p)
                | cosmic_text::fontdb::Source::SharedFile(p, _) => {
                    if !seen.contains(&p.as_path()) {
                        seen.push(p.as_path());
                        // sync-ok: read once per database change, for the memory overlay only
                        total += std::fs::metadata(p).map_or(0, |m| m.len() as usize);
                    }
                }
                cosmic_text::fontdb::Source::Binary(data) => {
                    total += data.as_ref().as_ref().len();
                }
            }
        }
        self.font_bytes.set(Some((faces, total)));
        total
    }

    /// How many faces the font database holds.
    pub fn face_count(&self) -> usize {
        self.fs.db().len()
    }

    /// Whether the database holds a face of this family (case-insensitive).
    pub fn has_family_loaded(&self, name: &str) -> bool {
        has_family(&self.fs, name)
    }

    /// A character nothing loaded covers: loads, once per family, the next lazy fallback family
    /// that covers it. True when one was added.
    fn fallback_for(&mut self, ch: char) -> bool {
        let faces = installed_faces();
        for &family in LAZY_FALLBACK_FAMILIES {
            if self.tried_fallbacks.contains(&family) {
                continue;
            }
            self.tried_fallbacks.push(family);
            let files = files_for_family(&faces, family);
            let mut ids = Vec::new();
            for file in files {
                ids.extend(
                    self.fs
                        .db_mut()
                        .load_font_source(cosmic_text::fontdb::Source::File(file)),
                );
            }
            let covers = self.covers(ch);
            if covers {
                self.kept_fallbacks.push(family);
                return true;
            }
            for id in ids {
                self.fs.db_mut().remove_face(id);
            }
        }
        false
    }

    /// Whether shaping `ch` in the current stack draws a real glyph (not the missing-glyph box).
    fn covers(&mut self, ch: char) -> bool {
        let px = font_px_for(self.base_px, 1.0);
        let family = self.family.clone();
        let attrs = Attrs::new().family(Family::Name(&family));
        let mut buffer = Buffer::new_empty(Metrics::new(px, px));
        buffer.set_text(&ch.to_string(), &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);
        missing_char(&buffer).is_none()
    }

    /// Builds a fresh font system for a new stack, keeping the lazy fallbacks already needed.
    fn rebuild_for(&mut self, families: &[&str]) {
        let mut db = database_for(families, FALLBACK_FAMILIES);
        let faces = installed_faces();
        for &family in &self.kept_fallbacks {
            for file in files_for_family(&faces, family) {
                let _ = db.load_font_file(&file);
            }
        }
        self.fs = FontSystem::new_with_locale_and_db(LOCALE.into(), db);
        self.swash = SwashCache::new();
    }

    pub fn family(&self) -> &str {
        &self.family
    }

    /// The base font size in logical pixels.
    pub fn base_px(&self) -> f32 {
        self.base_px
    }

    /// Counts font changes; a repeat of the same face and size leaves it alone.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    fn cached(&self) -> usize {
        self.cache.len()
    }

    #[cfg(test)]
    fn is_monospaced(&self, name: &str) -> bool {
        self.fs
            .db()
            .faces()
            .any(|f| f.monospaced && f.families.iter().any(|(n, _)| n == name))
    }

    fn rasterize(&mut self, key: &GlyphKey) -> Option<GlyphBitmap> {
        if key.text.chars().all(|c| c == ' ') {
            return None;
        }
        let px = font_px_for(self.base_px, self.scale);
        let family = self.family.clone();
        let mut attrs = Attrs::new().family(Family::Name(&family));
        if key.bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if key.italic {
            attrs = attrs.style(Style::Italic);
        }
        let mut buffer = Buffer::new_empty(Metrics::new(px, px));
        buffer.set_text(&key.text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);
        // A glyph id of 0 is the missing-glyph box: ask for one more fallback family and shape
        // again, until the text is covered or no lazy family is left.
        while let Some(missing) = missing_char(&buffer) {
            if !self.fallback_for(missing) {
                break;
            }
            buffer.set_text(&key.text, &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.fs, false);
        }
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

    fn memory(&self) -> GlyphMemory {
        let caches = self
            .cache
            .iter()
            .map(|(k, v)| k.text.capacity() + v.as_ref().map_or(0, |b| b.data.capacity()))
            .sum::<usize>()
            + self.cache.capacity() * size_of::<(GlyphKey, Option<GlyphBitmap>)>();
        GlyphMemory {
            fonts: self.font_bytes(),
            caches,
        }
    }

    fn set_font(&mut self, families: &[&str], base_px: f32) {
        let installed = installed_faces();
        let wanted = families
            .iter()
            .find(|f| !files_for_family(&installed, f).is_empty());
        if wanted.is_some_and(|f| !has_family(&self.fs, f)) {
            // The stack's first installed family is not loaded: swap the database for the new stack.
            self.rebuild_for(families);
        }
        let family = pick_family(&self.fs, families);
        if family == self.family && (base_px - self.base_px).abs() < f32::EPSILON {
            return;
        }
        self.units = measure(&mut self.fs, &family, base_px);
        self.family = family;
        self.base_px = base_px;
        self.cache.clear();
        self.generation += 1;
    }
}

/// The first character of `text` that the shaped buffer drew as the missing-glyph box.
fn missing_char(buffer: &Buffer) -> Option<char> {
    let run = buffer.layout_runs().next()?;
    run.glyphs.iter().filter(|g| g.glyph_id == 0).find_map(|g| {
        run.text
            .get(g.start..g.end)?
            .chars()
            .find(|c| !c.is_whitespace())
    })
}

/// The first installed family of `families`, else the first monospace face, else "monospace".
fn pick_family(fs: &FontSystem, families: &[&str]) -> String {
    families
        .iter()
        .find(|name| has_family(fs, name))
        .map(|name| name.to_string())
        .or_else(|| {
            fs.db()
                .faces()
                .find(|f| f.monospaced)
                .and_then(|f| f.families.first())
                .map(|(name, _)| name.clone())
        })
        .unwrap_or_else(|| "monospace".to_string())
}

/// Whether the font database has a face with this family name.
fn has_family(fs: &FontSystem, name: &str) -> bool {
    fs.db()
        .faces()
        .any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
}

/// Shapes "M" at the base size and reads the font's metrics, in font units.
fn measure(fs: &mut FontSystem, family: &str, base_px: f32) -> FontUnits {
    let px = font_px_for(base_px, 1.0);
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
    fn a_new_stack_changes_the_family_name() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        r.set_font(&["Consolas"], 13.0);
        if installed("Consolas") {
            assert_eq!(r.family(), "Consolas");
        } else {
            assert!(!r.family().is_empty());
        }
    }

    #[test]
    fn an_unknown_stack_falls_to_a_monospace_face() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        r.set_font(&["No Such Face"], 13.0);
        assert!(
            r.is_monospaced(r.family()),
            "{} is not monospaced",
            r.family()
        );
    }

    #[test]
    // Measured at 400 percent so whole-pixel rounding cannot hide the ratio.
    fn a_bigger_base_gives_bigger_cells() {
        use crate::metrics::cell_metrics_at;
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        let w13 = cell_metrics_at(&r.font_units(), r.base_px(), 4.0).cell_w as f32;
        r.set_font(DEFAULT_FAMILIES, 16.0);
        assert_eq!(r.base_px(), 16.0);
        let w16 = cell_metrics_at(&r.font_units(), r.base_px(), 4.0).cell_w as f32;
        assert!(w16 > w13);
        assert!((w16 / w13 - 16.0 / 13.0).abs() < 0.1, "{w13} -> {w16}");
    }

    #[test]
    fn the_same_look_twice_changes_nothing() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        r.glyph(&key("M"));
        let g = r.generation();
        r.set_font(DEFAULT_FAMILIES, 13.0);
        assert_eq!(r.generation(), g);
        assert_eq!(r.cached(), 1, "an unchanged look keeps the cache");
        r.set_font(DEFAULT_FAMILIES, 14.0);
        assert_eq!(r.generation(), g + 1);
    }

    #[test]
    fn a_font_change_clears_the_cache() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        r.glyph(&key("M"));
        assert_eq!(r.cached(), 1);
        r.set_font(DEFAULT_FAMILIES, 20.0);
        assert_eq!(r.cached(), 0);
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

    /// The sample of every script the plan names, plus the theme sample file.
    fn sample_text() -> String {
        let mut s = String::from(include_str!("../../../docs/theme-sample.txt"));
        s.push_str(
            "Hello Wxyz 0123 \u{2500}\u{2502}\u{250C}\u{2588}\u{2591} \u{2800}\u{28FF} \
             \u{3B1}\u{3B2}\u{3B3} \u{416}\u{44F} \u{4E2D}\u{6587}\u{65E5}\u{672C} \
             \u{1F600}\u{1F680}\u{2764} \u{D55C}\u{AE00} \u{905}\u{915}\u{93E} \u{2713}\u{2192}",
        );
        s
    }

    #[test]
    fn glyph_images_are_identical_to_the_all_fonts_build_for_the_theme_sample() {
        let text = sample_text();
        for scale in [1.0f32, 2.25] {
            let mut old = Rasterizer::from_system(FontSystem::new(), DEFAULT_FAMILIES, scale);
            let mut new = Rasterizer::new(DEFAULT_FAMILIES, scale);
            assert_eq!(old.family(), new.family());
            let mut checked = 0;
            for ch in text.chars().filter(|c| !c.is_whitespace()) {
                let k = key(&ch.to_string());
                assert_eq!(
                    old.glyph(&k).cloned(),
                    new.glyph(&k).cloned(),
                    "U+{:04X} differs at scale {scale}",
                    ch as u32
                );
                checked += 1;
            }
            assert!(checked > 100);
            assert!(new.face_count() < old.face_count());
        }
    }

    #[test]
    fn a_missing_character_loads_one_fallback_once() {
        use crate::font_catalog::{files_for_family, installed_faces};
        if files_for_family(&installed_faces(), "Javanese Text").is_empty() {
            eprintln!("skipped: Javanese Text is not installed");
            return;
        }
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        assert!(!r.has_family_loaded("Javanese Text"));
        let before = r.face_count();
        let java = key("\u{A9B2}");
        assert!(r.glyph(&java).is_some());
        assert!(r.has_family_loaded("Javanese Text"));
        let after = r.face_count();
        assert!(after > before);
        // The same family is not loaded again, by this character or another of its script.
        r.glyph(&key("\u{A9B3}"));
        r.glyph(&java);
        assert_eq!(r.face_count(), after);
        let tried = r.tried_fallbacks.len();
        // A character nothing covers tries every lazy family once and keeps none.
        r.glyph(&key("\u{10FFFD}"));
        let after_tofu = r.face_count();
        assert_eq!(after_tofu, after);
        let tried_after = r.tried_fallbacks.len();
        assert!(tried_after >= tried);
        r.glyph(&key("\u{10FFFC}"));
        assert_eq!(r.tried_fallbacks.len(), tried_after);
        assert_eq!(r.face_count(), after);
    }

    #[test]
    fn changing_the_font_swaps_the_database() {
        use crate::font_catalog::{files_for_family, installed_faces};
        if files_for_family(&installed_faces(), "Arial").is_empty() {
            eprintln!("skipped: Arial is not installed");
            return;
        }
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        assert!(!r.has_family_loaded("Arial"));
        let g = r.generation();
        r.set_font(&["Arial"], 13.0);
        assert_eq!(r.family(), "Arial");
        assert!(r.has_family_loaded("Arial"));
        assert_eq!(r.generation(), g + 1);
        assert!(r.glyph(&key("M")).is_some());
        // Back to a family the fallbacks already hold: no reload needed, the family changes.
        r.set_font(&["Consolas"], 13.0);
        assert_eq!(r.family(), "Consolas");
    }

    #[test]
    #[ignore = "release timing: cargo test --release -- --ignored startup_font_load"]
    fn startup_font_load_is_under_150_ms() {
        let start = std::time::Instant::now();
        let r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        let new_ms = start.elapsed().as_millis();
        let start = std::time::Instant::now();
        let all = FontSystem::new();
        let old_ms = start.elapsed().as_millis();
        eprintln!(
            "new {new_ms} ms ({} faces), all fonts {old_ms} ms ({} faces)",
            r.face_count(),
            all.db().len()
        );
        assert!(new_ms < 150, "{new_ms} ms");
    }
}
