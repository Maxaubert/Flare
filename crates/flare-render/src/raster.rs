//! Rasteriser: cosmic-text shapes a cluster, swash rasterises its glyphs, and the result is handed
//! to the painters through `GlyphSource`.

use crate::blend::blend_premul;
use crate::fallback;
use crate::font_catalog::{
    EMOJI_FAMILIES, FALLBACK_FAMILIES, LAZY_FALLBACK_FAMILIES, MONOSPACE_FAMILIES,
    TEXT_SYMBOL_FAMILIES, database_for, files_for_family, installed_faces,
};
use crate::glyph::{GlyphBitmap, GlyphKey, GlyphKind, GlyphMemory, GlyphSource};
use crate::metrics::{BASE_FONT_PX, FontUnits, cell_metrics_at, font_px_for};
use crate::presentation;
use crate::shapes;
use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent,
    SwashImage, Weight,
};
use std::collections::{HashMap, HashSet};

/// Rasterised clusters kept before the cache starts over. A screen needs a few thousand at most;
/// the cap is what stops a stream of new clusters growing it for the session (audit M1).
const CACHE_MAX: usize = 8192;

/// Characters remembered as covered by no family; the set starts over past this (audit R5).
const UNCOVERED_MAX: usize = 4096;

/// The locale the font system is built with (the fallback lists pick Han fonts by it).
const LOCALE: &str = "en-US";

/// PT's `TERM_FONTS` 'cascadia' stack, in order of preference.
#[cfg(windows)]
pub const DEFAULT_FAMILIES: &[&str] = &["Cascadia Mono", "JetBrainsMono NF", "Consolas"];
/// macOS: the same stack with the system monospace faces in Consolas' place (`macos/font_catalog.rs`).
#[cfg(not(windows))]
pub use crate::font_catalog::DEFAULT_FAMILIES;

pub struct Rasterizer {
    fs: FontSystem,
    swash: SwashCache,
    family: String,
    /// The whole stack, in order: a private-use character is looked up in each of these and never\n    /// in a symbol font that happens to map it to something else.
    stack: Vec<String>,
    scale: f32,
    base_px: f32,
    generation: u64,
    units: FontUnits,
    cache: HashMap<GlyphKey, Option<GlyphBitmap>>,
    /// Entries the cache holds before it starts over (the atlas keeps what is on screen).
    cache_cap: usize,
    /// The (lazy family, character) pairs already asked for and found useless: a family is tried
    /// again for a character it was not asked about, never twice for the same one.
    rejected: HashSet<(&'static str, char)>,
    /// Characters every lazy family was asked about and none covers, never walked again.
    uncovered: HashSet<char>,
    /// The ones that were kept, so a rebuilt database brings them back.
    kept_fallbacks: Vec<&'static str>,
    /// (faces, bytes) of the last `font_bytes` count.
    font_bytes: std::cell::Cell<Option<(usize, usize)>>,
    /// Characters sent to the fallback worker and not answered yet (audit M2).
    pending: HashSet<char>,
    /// The worker that reads cmaps and parses font files off the UI thread, started on first need.
    worker: Option<fallback::Worker>,
    /// What the worker shares: the answer-waiting flag, its cmap read count, the test gate.
    shared: fallback::Shared,
    /// How many font files were loaded into the database for a fallback walk.
    family_loads: usize,
    /// Installed families (lowercase) whose files did not load even after a rebuild: a stale
    /// registry entry or an unreadable file. Never asked for again, so a rebuild cannot repeat
    /// on every font apply (every tab switch and look change).
    unloadable: HashSet<String>,
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
        let units = measure(&mut fs, &family, BASE_FONT_PX).unwrap_or(FALLBACK_UNITS);
        Self {
            fs,
            swash: SwashCache::new(),
            family,
            stack: families.iter().map(|f| (*f).to_string()).collect(),
            scale,
            base_px: BASE_FONT_PX,
            generation: 0,
            units,
            cache: HashMap::new(),
            cache_cap: CACHE_MAX,
            rejected: HashSet::new(),
            uncovered: HashSet::new(),
            kept_fallbacks: Vec::new(),
            font_bytes: std::cell::Cell::new(None),
            pending: HashSet::new(),
            worker: None,
            shared: fallback::Shared::default(),
            family_loads: 0,
            unloadable: HashSet::new(),
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

    #[cfg(test)]
    fn set_cache_cap(&mut self, cap: usize) {
        self.cache_cap = cap;
    }

    /// Asks the worker for a family for a character nothing loaded covers. Never blocks: the cell
    /// stays blank until `apply_fallbacks` has the answer. Each character is walked once.
    fn request_fallback(&mut self, ch: char) {
        if self.uncovered.contains(&ch) || self.pending.contains(&ch) {
            return;
        }
        self.pending.insert(ch);
        self.send_request(ch);
    }

    fn send_request(&mut self, ch: char) {
        let mut skip: Vec<&'static str> = self.kept_fallbacks.clone();
        skip.extend(
            LAZY_FALLBACK_FAMILIES
                .iter()
                .copied()
                .filter(|f| self.rejected.contains(&(*f, ch))),
        );
        let shared = self.shared.clone();
        self.worker
            .get_or_insert_with(|| fallback::Worker::spawn(shared))
            .send(fallback::Request { ch, skip });
    }

    /// Takes the worker's answers: pushes a found family's parsed faces into the database, keeps it
    /// when it shapes the character, and drops the cached blanks of that character. True when
    /// something changed and the cells must be painted again.
    fn take_fallbacks(&mut self) -> bool {
        self.shared
            .ready
            .store(false, std::sync::atomic::Ordering::Release);
        let mut changed = false;
        while let Some(answer) = self.worker.as_ref().and_then(|w| w.try_recv()) {
            match answer {
                fallback::Answer::Found { ch, family, faces } => {
                    if self.kept_fallbacks.contains(&family) {
                        self.evict(ch);
                        self.pending.remove(&ch);
                        changed = true;
                        continue;
                    }
                    self.rejected.insert((family, ch));
                    let ids: Vec<_> = faces
                        .into_iter()
                        .map(|f| self.fs.db_mut().push_face_info(f))
                        .collect();
                    self.family_loads += ids.len();
                    if self.covers(ch) {
                        self.kept_fallbacks.push(family);
                        self.evict(ch);
                        self.pending.remove(&ch);
                        changed = true;
                    } else {
                        for id in ids {
                            self.fs.db_mut().remove_face(id);
                        }
                        // Next family for the same character.
                        self.send_request(ch);
                    }
                }
                fallback::Answer::None { ch } => {
                    if self.uncovered.len() >= UNCOVERED_MAX {
                        self.uncovered.clear();
                    }
                    if self.rejected.len() >= UNCOVERED_MAX {
                        self.rejected.clear();
                    }
                    self.uncovered.insert(ch);
                    self.pending.remove(&ch);
                    // It draws the missing-glyph box from here on.
                    self.evict(ch);
                    changed = true;
                }
            }
        }
        changed
    }

    /// Forgets the cached bitmaps of clusters holding `ch` (they were drawn before its font came).
    fn evict(&mut self, ch: char) {
        self.cache.retain(|k, _| !k.text.contains(ch));
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
        // The first character decides; a trailing variation selector or combining mark is ignored.
        let mut chars = key.text.chars();
        if let Some(ch) = chars.next()
            && chars.all(is_mark)
            && shapes::has_shape(ch)
        {
            // Drawn here, cell-exact, never from a font (owner bug 2026-10-21).
            let m = cell_metrics_at(&self.units, self.base_px, self.scale);
            return shapes::shape_bitmap(ch, m.cell_w, m.cell_h, m.baseline, self.scale.max(1.0));
        }
        if key.text.chars().next().is_some_and(is_private_use) {
            return self.rasterize_private_use(key);
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
        // A glyph id of 0 is the missing-glyph box. The family that covers it is found on the
        // worker (audit M2): until it answers the cell stays blank, and the paint goes on.
        if let Some(missing) = missing_char(&buffer) {
            self.request_fallback(missing);
            if self.pending.contains(&missing) {
                return None;
            }
        }
        let run = buffer.layout_runs().next()?;
        // Presentation (UTS #51), the way Terminal.app, kitty and Ghostty pick: a text-presentation
        // character (U+23FA, U+2764 without VS16) that cosmic-text's common fallback sent to the
        // colour emoji face is drawn from a text face when one has it, and an emoji asked for with
        // VS16 that a text face took (Menlo has U+2764) comes from the emoji face. macOS only:
        // Windows has no such lists, so its glyphs stay as cosmic-text picks them.
        let from_emoji_face = run
            .glyphs
            .iter()
            .any(|g| is_emoji_face(&self.fs, g.font_id));
        let wants_emoji = presentation::wants_emoji(&key.text);
        if wants_emoji && !from_emoji_face {
            for &name in EMOJI_FAMILIES {
                if let Some(images) = self.own_images(&key.text, key, name) {
                    return compose(&images);
                }
            }
        }
        if !TEXT_SYMBOL_FAMILIES.is_empty() && !wants_emoji && from_emoji_face {
            let text: String = key.text.chars().filter(|&c| !is_variation(c)).collect();
            let families: Vec<String> = self
                .stack
                .iter()
                .cloned()
                .chain(TEXT_SYMBOL_FAMILIES.iter().map(|f| (*f).to_string()))
                .filter(|f| !f.contains("Emoji"))
                .collect();
            for name in families {
                if let Some(images) = self.own_images(&text, key, &name) {
                    return compose(&images);
                }
            }
        }

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

    /// `text` shaped in `name` alone: the images of its glyphs when that loaded family has every
    /// one of them, else None.
    fn own_images(
        &mut self,
        text: &str,
        key: &GlyphKey,
        name: &str,
    ) -> Option<Vec<(i32, i32, SwashImage)>> {
        if !has_family(&self.fs, name) {
            return None;
        }
        let px = font_px_for(self.base_px, self.scale);
        let mut attrs = Attrs::new().family(Family::Name(name));
        if key.bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if key.italic {
            attrs = attrs.style(Style::Italic);
        }
        let mut buffer = Buffer::new_empty(Metrics::new(px, px));
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);
        let run = buffer.layout_runs().next()?;
        let own =
            run.glyphs.iter().all(|g| {
                g.glyph_id != 0
                    && self.fs.db().face(g.font_id).is_some_and(|f| {
                        f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
                    })
            });
        if !own {
            return None;
        }
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
        Some(images)
    }
}

/// A colour emoji face (Apple Color Emoji), told by its PostScript name as cosmic-text does.
fn is_emoji_face(fs: &FontSystem, id: cosmic_text::fontdb::ID) -> bool {
    fs.db()
        .face(id)
        .is_some_and(|f| f.post_script_name.contains("Emoji"))
}

/// VS15 or VS16, dropped before a text face shapes the cluster.
fn is_variation(c: char) -> bool {
    matches!(c, '\u{FE0E}' | '\u{FE0F}')
}

impl Rasterizer {
    /// A private-use character (Nerd Font icons and the like) has no meaning outside the font that
    /// was made for it, and a symbol font in the fallback list maps some of them to unrelated
    /// drawings (a star for a Powerline cap). Like Prism Terminal's stack, it is looked up in the
    /// stack's own faces in order; where none has it, the cell stays blank.
    fn rasterize_private_use(&mut self, key: &GlyphKey) -> Option<GlyphBitmap> {
        for name in self.stack.clone() {
            if let Some(images) = self.own_images(&key.text, key, &name) {
                return compose(&images);
            }
        }
        None
    }
}

/// A variation selector, joiner or combining mark: it adds nothing a cell-exact shape could draw.
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x200D | 0xFE00..=0xFE0F | 0x20D0..=0x20FF)
}

/// The private-use areas: U+E000 to U+F8FF and the two supplementary planes.
fn is_private_use(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD)
}

impl GlyphSource for Rasterizer {
    fn glyph(&mut self, key: &GlyphKey) -> Option<&GlyphBitmap> {
        if !self.cache.contains_key(key) {
            let bitmap = self.rasterize(key);
            if self.cache.len() >= self.cache_cap {
                self.cache.clear();
            }
            self.cache.insert(key.clone(), bitmap);
        }
        self.cache.get(key).and_then(|bitmap| bitmap.as_ref())
    }

    fn font_units(&self) -> FontUnits {
        self.units
    }

    fn fallback_ready(&self) -> bool {
        self.shared.ready.load(std::sync::atomic::Ordering::Acquire)
    }

    fn apply_fallbacks(&mut self) -> bool {
        self.take_fallbacks()
    }

    fn set_fallback_waker(&mut self, wake: std::sync::Arc<dyn Fn() + Send + Sync>) {
        self.shared.wake = Some(wake);
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
        let _ = wanted;
        let missing = !missing_families(
            families,
            &installed,
            &|f| has_family(&self.fs, f),
            &self.unloadable,
        )
        .is_empty();
        if missing {
            // An installed family of the stack is not loaded (a Nerd Font the old stack lacked):
            // swap the database for the new stack.
            self.rebuild_for(families);
            // What still is not there will never load: do not rebuild for it again.
            let still = missing_families(
                families,
                &installed,
                &|f| has_family(&self.fs, f),
                &self.unloadable,
            );
            self.unloadable
                .extend(still.iter().map(|f| f.to_ascii_lowercase()));
        }
        let stack: Vec<String> = families.iter().map(|f| (*f).to_string()).collect();
        let stack_changed = stack != self.stack;
        self.stack = stack;
        let family = pick_family(&self.fs, families);
        if family == self.family && (base_px - self.base_px).abs() < f32::EPSILON {
            if stack_changed || missing {
                // Same first face, other private-use sources: what was cached may now differ.
                self.cache.clear();
                self.generation += 1;
            }
            return;
        }
        self.units = measure(&mut self.fs, &family, base_px).unwrap_or(FALLBACK_UNITS);
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

/// The first installed family of `families`, else the platform's monospace faces (none on
/// Windows), else the first monospace face, else "monospace".
fn pick_family(fs: &FontSystem, families: &[&str]) -> String {
    families
        .iter()
        .chain(MONOSPACE_FAMILIES.iter())
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
/// The families of `stack` that the registry lists with files, that are not in the database, and
/// that did not already fail to load once.
fn missing_families<'a>(
    stack: &[&'a str],
    installed: &[crate::font_catalog::FaceFile],
    loaded: &dyn Fn(&str) -> bool,
    unloadable: &HashSet<String>,
) -> Vec<&'a str> {
    stack
        .iter()
        .copied()
        .filter(|f| {
            !unloadable.contains(&f.to_ascii_lowercase())
                && !files_for_family(installed, f).is_empty()
                && !loaded(f)
        })
        .collect()
}

fn has_family(fs: &FontSystem, name: &str) -> bool {
    fs.db()
        .faces()
        .any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
}

/// The metrics used when no font can be measured (the registry yielded nothing, or no face shapes
/// "M"): a monospace face's usual proportions, so the grid still lays out (audit L8).
const FALLBACK_UNITS: FontUnits = FontUnits {
    units_per_em: 2048.0,
    ascent: 1900.0,
    descent: -480.0,
    leading: 0.0,
    advance: 1200.0,
};

/// Shapes "M" at the base size and reads the font's metrics, in font units. `None` when no font
/// shapes it.
fn measure(fs: &mut FontSystem, family: &str, base_px: f32) -> Option<FontUnits> {
    // cosmic-text panics ("no default font found") when shaping has no face at all, so an empty
    // database is answered here, before any shaping.
    if fs.db().is_empty() {
        return None;
    }
    let px = font_px_for(base_px, 1.0);
    let attrs = Attrs::new().family(Family::Name(family));
    let mut buffer = Buffer::new_empty(Metrics::new(px, px));
    buffer.set_text("M", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fs, false);
    let run = buffer.layout_runs().next()?;
    let glyph = run.glyphs.first()?;
    let font = fs.get_font(glyph.font_id, Weight::NORMAL)?;
    let metrics = font.metrics();
    let upem = metrics.units_per_em as f32;
    Some(FontUnits {
        units_per_em: upem,
        ascent: metrics.ascent,
        descent: metrics.descent,
        leading: metrics.leading,
        advance: glyph.w / px * upem,
    })
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

    /// A lazy fallback family on this platform, and two characters of its script that nothing
    /// loaded at start-up covers: Javanese on Windows, Egyptian hieroglyphs on macOS.
    const JAVANESE: &str = if cfg!(windows) {
        "Javanese Text"
    } else {
        "Noto Sans Egyptian Hieroglyphs"
    };
    const JAVANESE_A: &str = if cfg!(windows) {
        "\u{A9B2}"
    } else {
        "\u{13000}"
    };
    const JAVANESE_B: &str = if cfg!(windows) {
        "\u{A9B3}"
    } else {
        "\u{13001}"
    };
    /// A monospace face the start-up fallbacks hold on this platform.
    const HELD_MONO: &str = if cfg!(windows) { "Consolas" } else { "Menlo" };
    /// An installed face the start-up fallbacks do not hold on this platform.
    const UNHELD: &str = if cfg!(windows) { "Arial" } else { "Verdana" };

    fn key(text: &str) -> GlyphKey {
        GlyphKey {
            text: text.to_string(),
            bold: false,
            italic: false,
        }
    }

    #[test]
    fn a_font_system_with_no_faces_measures_the_fallback() {
        // Audit L8: the registry yielded nothing, so "M" shapes to no run and measure() used to
        // panic. It must fall back to the default metrics instead.
        let fs =
            FontSystem::new_with_locale_and_db(LOCALE.into(), cosmic_text::fontdb::Database::new());
        let r = Rasterizer::from_system(fs, &["No Such Font"], 1.0);
        assert_eq!(r.units, FALLBACK_UNITS);
        assert!(r.units.units_per_em > 0.0);
    }

    /// Waits for every fallback the worker was asked for and applies the answers (the UI thread
    /// does this on its next frame).
    impl Rasterizer {
        fn settle(&mut self) {
            let start = std::time::Instant::now();
            while !self.pending.is_empty() {
                assert!(
                    start.elapsed().as_secs() < 60,
                    "the fallback worker never answered"
                );
                self.take_fallbacks();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }

    /// The glyph as it is once any fallback font has arrived.
    fn glyph_settled(r: &mut Rasterizer, k: &GlyphKey) -> Option<GlyphBitmap> {
        r.glyph(k);
        r.settle();
        r.glyph(k).cloned()
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

    /// Rule 1131: U+23FA (Claude Code's bullet) and other text-presentation characters draw from a
    /// text face on macOS, and turn into the colour emoji only with VS16 or when they are emoji by
    /// default.
    #[cfg(not(windows))]
    #[test]
    fn a_text_presentation_symbol_draws_from_a_text_face_and_vs16_makes_it_an_emoji() {
        if !installed("Apple Color Emoji") {
            eprintln!("skipped: no Apple Color Emoji on this machine");
            return;
        }
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        for text in ["\u{23FA}", "\u{2764}", "\u{25B6}", "\u{23FA}\u{FE0E}"] {
            let g = glyph_settled(&mut r, &key(text)).expect(text);
            assert_eq!(g.kind, GlyphKind::Mask, "{text:?} came from a colour face");
            assert!(g.data.iter().any(|&a| a > 0), "{text:?} drew nothing");
        }
        for text in [
            "\u{23FA}\u{FE0F}",
            "\u{2764}\u{FE0F}",
            "\u{1F600}",
            "\u{2B50}",
        ] {
            let g = glyph_settled(&mut r, &key(text)).expect(text);
            assert_eq!(g.kind, GlyphKind::Color, "{text:?} is an emoji");
        }
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
        match glyph_settled(&mut r, &key("中")).map(|g| g.width) {
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
        match glyph_settled(&mut r, &key("😀")).map(|g| g.kind) {
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
            let mut differing = Vec::new();
            // Control characters (the sample holds raw ESC bytes) draw no glyph: a terminal never rasterises
            // them. On a machine whose installed fonts cover one (the CI runner does U+001B), the all-fonts
            // build finds that font and the lazy catalogue fallback does not consult it, so they differ there.
            // Every printable character is compared, and all differences are listed, not just the first.
            for ch in text
                .chars()
                .filter(|c| !c.is_whitespace() && !c.is_control())
            {
                let k = key(&ch.to_string());
                if glyph_settled(&mut old, &k) != glyph_settled(&mut new, &k) {
                    differing.push(format!("U+{:04X}", ch as u32));
                }
                checked += 1;
            }
            assert!(
                differing.is_empty(),
                "differ at scale {scale}: {differing:?}"
            );
            assert!(checked > 100);
            assert!(new.face_count() < old.face_count());
        }
    }

    #[test]
    fn a_missing_character_loads_one_fallback_once() {
        use crate::font_catalog::{files_for_family, installed_faces};
        if files_for_family(&installed_faces(), JAVANESE).is_empty() {
            eprintln!("skipped: {JAVANESE} is not installed");
            return;
        }
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        assert!(!r.has_family_loaded(JAVANESE));
        let before = r.face_count();
        let java = key(JAVANESE_A);
        assert!(glyph_settled(&mut r, &java).is_some());
        assert!(r.has_family_loaded(JAVANESE));
        let after = r.face_count();
        assert!(after > before);
        // The same family is not loaded again, by this character or another of its script.
        glyph_settled(&mut r, &key(JAVANESE_B));
        glyph_settled(&mut r, &java);
        assert_eq!(r.face_count(), after);
        let tried = r.rejected.len();
        // A character nothing covers tries every lazy family once and keeps none.
        glyph_settled(&mut r, &key("\u{3FFFD}"));
        let after_tofu = r.face_count();
        assert_eq!(after_tofu, after);
        let tried_after = r.rejected.len();
        assert!(tried_after >= tried);
        // The same character again walks nothing (audit M2: a different one may, see the test
        // a_char_no_family_covers_does_not_burn_the_families_for_another_script).
        glyph_settled(&mut r, &key("\u{3FFFD}"));
        glyph_settled(&mut r, &key("a\u{3FFFD}"));
        assert_eq!(r.rejected.len(), tried_after);
        assert_eq!(r.face_count(), after);
    }

    #[test]
    fn many_uncovered_characters_load_no_font_file() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        // Unassigned codepoints: no lazy family has them in its cmap (audit R5), so a flood of
        // them reads each family's cmap at most once and loads nothing into the database.
        let faces = r.face_count();
        for i in 0..40u32 {
            r.glyph(&key(&char::from_u32(0x10FF00 + i).unwrap().to_string()));
        }
        r.settle();
        assert_eq!(
            r.family_loads, 0,
            "a family was loaded for an uncovered char"
        );
        assert_eq!(r.face_count(), faces);
        assert!(
            r.shared
                .cmap_reads
                .load(std::sync::atomic::Ordering::Relaxed)
                <= LAZY_FALLBACK_FAMILIES.len()
        );
    }

    #[test]
    fn the_uncovered_set_is_bounded() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        for i in 0..(UNCOVERED_MAX as u32 + 50) {
            r.glyph(&key(&char::from_u32(0x100000 + i).unwrap().to_string()));
        }
        r.settle();
        assert!(r.uncovered.len() <= UNCOVERED_MAX);
    }

    #[test]
    fn the_glyph_cache_is_bounded() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        r.set_cache_cap(8);
        for i in 0..40u32 {
            // Longer and longer clusters, the shape of a mark flood.
            let text = format!("e{}", "\u{301}".repeat(i as usize));
            r.glyph(&key(&text));
            assert!(r.cached() <= 8, "{} cached after {i}", r.cached());
        }
        assert!(r.glyph(&key("M")).is_some(), "still draws after evictions");
    }

    #[test]
    fn a_char_no_family_covers_does_not_burn_the_families_for_another_script() {
        use crate::font_catalog::{files_for_family, installed_faces};
        if files_for_family(&installed_faces(), JAVANESE).is_empty() {
            eprintln!("skipped: {JAVANESE} is not installed");
            return;
        }
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        // Nothing covers this one: every lazy family is asked about it.
        glyph_settled(&mut r, &key("\u{3FFFD}"));
        assert!(!r.has_family_loaded(JAVANESE));
        let walked = r.rejected.len();
        assert!(r.uncovered.contains(&'\u{3FFFD}'));
        // The same character is never walked again (a prompt icon redrawn every frame).
        glyph_settled(&mut r, &key("x\u{3FFFD}"));
        assert_eq!(r.rejected.len(), walked);
        // A different script still reaches its family.
        assert!(glyph_settled(&mut r, &key(JAVANESE_A)).is_some());
        assert!(r.has_family_loaded(JAVANESE));
    }

    /// A gate that holds the fallback worker, standing in for a slow disk.
    fn gated() -> (Rasterizer, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        let gate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        r.shared.gate = Some(gate.clone());
        (r, gate)
    }

    #[test]
    fn a_family_that_will_not_load_is_asked_for_once() {
        let installed = vec![crate::font_catalog::FaceFile {
            family: "Ghost Mono".into(),
            path: "C:/gone/ghost.ttf".into(),
            index: 0,
        }];
        let never = |_: &str| false;
        let mut bad = HashSet::new();
        let first = missing_families(&["Ghost Mono", "Absent"], &installed, &never, &bad);
        assert_eq!(first, vec!["Ghost Mono"]);
        bad.extend(first.iter().map(|f| f.to_ascii_lowercase()));
        assert!(missing_families(&["Ghost Mono"], &installed, &never, &bad).is_empty());
        assert!(
            missing_families(&["Ghost Mono"], &installed, &|_| true, &HashSet::new()).is_empty()
        );
    }

    #[test]
    fn a_fallback_answer_wakes_the_window() {
        let (mut r, gate) = gated();
        let woken = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = woken.clone();
        r.set_fallback_waker(std::sync::Arc::new(move || {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        assert!(r.glyph(&key("\u{3FFFD}")).is_none());
        assert_eq!(
            woken.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "held shut"
        );
        gate.store(true, std::sync::atomic::Ordering::Release);
        let start = std::time::Instant::now();
        while woken.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            assert!(
                start.elapsed().as_secs() < 60,
                "the worker never woke the window"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn paint_never_blocks_on_a_family_load() {
        // Audit M2: a character no loaded font covers used to read cmaps and load a family inside
        // glyph(), on the UI thread. With the worker held shut, glyph() must still return, draw
        // blank, load nothing and read nothing; it only asks.
        let (mut r, gate) = gated();
        let faces = r.face_count();
        assert!(r.glyph(&key("\u{3FFFD}")).is_none(), "blank while it waits");
        r.glyph(&key(JAVANESE_A));
        assert_eq!(r.face_count(), faces, "nothing was loaded by the paint");
        assert_eq!(r.family_loads, 0);
        assert_eq!(
            r.shared
                .cmap_reads
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert!(r.pending.contains(&'\u{3FFFD}'));
        assert!(!r.fallback_ready(), "nothing has answered yet");
        // The same character asked again while it waits is not sent twice.
        r.glyph(&key("x\u{3FFFD}"));
        assert_eq!(r.pending.len(), 2);
        gate.store(true, std::sync::atomic::Ordering::Release);
        r.settle();
        assert!(r.uncovered.contains(&'\u{3FFFD}'));
    }

    #[test]
    fn a_family_that_arrives_repaints_the_blank_cell() {
        use crate::font_catalog::{files_for_family, installed_faces};
        if files_for_family(&installed_faces(), JAVANESE).is_empty() {
            eprintln!("skipped: {JAVANESE} is not installed");
            return;
        }
        let (mut r, gate) = gated();
        let java = key(JAVANESE_A);
        assert!(r.glyph(&java).is_none());
        assert!(!r.has_family_loaded(JAVANESE));
        gate.store(true, std::sync::atomic::Ordering::Release);
        let start = std::time::Instant::now();
        while !r.fallback_ready() {
            assert!(start.elapsed().as_secs() < 60, "no answer");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(r.apply_fallbacks(), "the host is told to paint again");
        assert!(r.has_family_loaded(JAVANESE));
        assert!(!r.fallback_ready(), "the flag is taken with the answers");
        assert!(r.glyph(&java).is_some(), "the cell now draws");
    }

    #[test]
    fn changing_the_font_swaps_the_database() {
        use crate::font_catalog::{files_for_family, installed_faces};
        if files_for_family(&installed_faces(), UNHELD).is_empty() {
            eprintln!("skipped: {UNHELD} is not installed");
            return;
        }
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        assert!(!r.has_family_loaded(UNHELD));
        let g = r.generation();
        r.set_font(&[UNHELD], 13.0);
        assert_eq!(r.family(), UNHELD);
        assert!(r.has_family_loaded(UNHELD));
        assert_eq!(r.generation(), g + 1);
        assert!(r.glyph(&key("M")).is_some());
        // Back to a family the fallbacks already hold: no reload needed, the family changes.
        r.set_font(&[HELD_MONO], 13.0);
        assert_eq!(r.family(), HELD_MONO);
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

    #[test]
    fn powerline_shapes_are_drawn_to_the_cell_at_every_scale() {
        // Owner bug 2026-10-21: U+E0B1 was a tofu box and U+E0B4 / U+E0B6 a star (Segoe UI Symbol).
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        for scale in [1.0f32, 1.25, 1.5, 2.0, 2.25, 3.0] {
            r.set_scale(scale);
            let m = cell_metrics_at(&r.font_units(), r.base_px(), scale);
            for ch in ["\u{E0B0}", "\u{E0B1}", "\u{E0B4}", "\u{E0B6}", "\u{E0BF}"] {
                let b = r.glyph(&key(ch)).cloned().expect("a built-in shape");
                assert_eq!(b.kind, GlyphKind::Mask);
                assert_eq!(
                    (b.width, b.height, b.left),
                    (m.cell_w, m.cell_h, 0),
                    "{ch:?} at {scale}"
                );
                assert_eq!(b.top, m.baseline as i32);
                assert!(
                    b.data.iter().any(|&v| v >= 100),
                    "{ch:?} has solid or stroke ink"
                );
            }
        }
    }

    #[test]
    fn the_statusline_pill_has_no_seam_between_cap_and_bar() {
        // chevron, cap, bar cell, cap: the cap's flat side is solid, so painted over the bar's
        // background it leaves no gap column.
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 2.25);
        let m = cell_metrics_at(&r.font_units(), r.base_px(), 2.25);
        let open = r.glyph(&key("\u{E0B6}")).cloned().unwrap();
        let close = r.glyph(&key("\u{E0B4}")).cloned().unwrap();
        for y in 0..m.cell_h {
            let i = (y * open.width + open.width - 1) as usize;
            assert!(open.data[i] >= 200, "left cap meets the bar at row {y}");
            assert!(
                close.data[(y * close.width) as usize] >= 200,
                "right cap meets the bar at row {y}"
            );
        }
    }

    #[test]
    fn a_private_use_character_is_never_a_symbol_fonts_unrelated_glyph() {
        // U+E0C0 is not a built-in shape. Segoe UI Symbol must not answer it; the stack's own
        // faces may (JetBrainsMono NF has it), and with none that has it the cell is blank.
        let mut plain = Rasterizer::new(&["Cascadia Mono"], 1.0);
        assert!(glyph_settled(&mut plain, &key("\u{E0C0}")).is_none());
        assert!(glyph_settled(&mut plain, &key("\u{F8FF}")).is_none());
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        if r.has_family_loaded("JetBrainsMono NF") {
            assert!(glyph_settled(&mut r, &key("\u{E0C0}")).is_some());
        } else {
            eprintln!("skipped: JetBrainsMono NF is not installed");
        }
    }

    #[test]
    fn a_shape_with_a_trailing_variation_selector_is_still_the_shape() {
        let mut r = Rasterizer::new(&["Cascadia Mono"], 1.0);
        let plain = r.glyph(&key("\u{E0B4}")).cloned().unwrap();
        let sel = r.glyph(&key("\u{E0B4}\u{FE0F}")).cloned().unwrap();
        assert_eq!(plain.data, sel.data);
    }

    #[test]
    fn switching_the_stack_loads_its_nerd_font() {
        let nf = ["JetBrainsMono NF", "Cascadia Mono"];
        if files_for_family(&installed_faces(), "JetBrainsMono NF").is_empty() {
            eprintln!("skipped: JetBrainsMono NF is not installed");
            return;
        }
        let mut r = Rasterizer::new(&["Consolas", "Cascadia Mono"], 1.0);
        r.set_font(&nf, 14.0);
        assert!(r.has_family_loaded("JetBrainsMono NF"));
        assert!(glyph_settled(&mut r, &key("\u{E0C0}")).is_some());
    }

    #[test]
    fn box_drawing_and_shades_still_come_from_the_font() {
        let mut r = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
        for ch in ["\u{2500}", "\u{253C}", "\u{2591}", "\u{2593}"] {
            assert!(glyph_settled(&mut r, &key(ch)).is_some(), "{ch:?}");
        }
    }
}
