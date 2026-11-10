//! Image thumbnails on hover (#17): the rules, the memory and the worker, with no window in them.
//!
//! [`ThumbHover`] is the clock: the 350 ms rest, what brings the picture down, and what keeps it down.
//! [`ThumbCache`] is memory only. [`ThumbWorker`] runs one job at a time, latest wins. Paths are vetted exactly as a
//! click's are (rules 13, 14), on the worker (rule 34), and only the path under the pointer is ever read.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::UNIX_EPOCH;

use flare_render::metrics::{CellMetrics, cell_origin};
use flare_surfaces::PanelKind;
use flare_surfaces::preview::{self, classify_resolved, loadable};
use flare_tabs::tip_place::{BoxF, TIP_GAP, TIP_MARGIN, TipSide, TipSpot, place_tip};
use flare_term::link_scan::Piece;
use flare_term::path_open::resolve_openable_file;

/// How long the pointer must rest on an image path.
pub const DWELL_MS: u64 = 350;
/// The most pictures remembered.
pub const CACHE_ENTRIES: usize = 32;
/// About the most bytes of pixels remembered.
pub const CACHE_BYTES: usize = 8 * 1024 * 1024;
/// A picture squeezed to fit a small window is not shown when its longest side would be under this (css px).
pub const PIC_MIN: f64 = 48.0;

/// A thumbnail ready to draw: straight RGBA, with what its caption says.
#[derive(Debug, PartialEq, Eq)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub name: String,
    pub source_width: u32,
    pub source_height: u32,
}

impl Thumb {
    /// The file name and the picture's own size in pixels.
    pub fn caption(&self) -> String {
        format!(
            "{}  {} x {}",
            self.name, self.source_width, self.source_height
        )
    }
}

/// What the window draws: the picture and the path's box, in css px from the grid's corner.
pub struct ThumbView {
    pub thumb: Arc<Thumb>,
    pub item: BoxF,
}

/// One version of one file: an edited picture has another length or time, so it is read again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbKey {
    pub path: String,
    pub len: u64,
    pub modified_ns: u128,
}

/// The remembered thumbnails, least recently used first. Memory only, and gone with the setting.
#[derive(Default)]
pub struct ThumbCache {
    rows: Vec<(ThumbKey, Arc<Thumb>)>,
    bytes: usize,
}

impl ThumbCache {
    pub fn get(&mut self, key: &ThumbKey) -> Option<Arc<Thumb>> {
        let i = self.rows.iter().position(|(k, _)| k == key)?;
        let row = self.rows.remove(i);
        let thumb = Arc::clone(&row.1);
        self.rows.push(row);
        Some(thumb)
    }

    /// Remembers a picture. An older version of the same file goes, then the least recently used until it fits.
    pub fn put(&mut self, key: ThumbKey, thumb: Arc<Thumb>) {
        self.rows.retain(|(k, _)| k.path != key.path);
        self.rows.push((key, thumb));
        self.bytes = self.rows.iter().map(|(_, t)| t.rgba.len()).sum();
        while self.rows.len() > 1 && (self.rows.len() > CACHE_ENTRIES || self.bytes > CACHE_BYTES) {
            let (_, gone) = self.rows.remove(0);
            self.bytes -= gone.rgba.len();
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.rows.len()
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.bytes
    }
}

enum Phase<K> {
    Idle,
    Dwell { key: K, since: u64 },
    Loading { key: K, seq: u64 },
    Shown { key: K },
}

/// What a pointer step did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pointed {
    /// A picture that was up came down.
    pub came_down: bool,
    /// A new rest began.
    pub started: bool,
}

/// The hover's clock, over any key that says "the same link in the same place". Time is handed in.
pub struct ThumbHover<K> {
    phase: Phase<K>,
    /// Sent away by a key, a press or a refusal: this link stays quiet until the pointer has left it.
    cooled: Option<K>,
}

impl<K: Clone + PartialEq> Default for ThumbHover<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Clone + PartialEq> ThumbHover<K> {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            cooled: None,
        }
    }

    fn key(&self) -> Option<&K> {
        match &self.phase {
            Phase::Idle => None,
            Phase::Dwell { key, .. } | Phase::Loading { key, .. } | Phase::Shown { key } => {
                Some(key)
            }
        }
    }

    /// Something is waiting, loading or up, so the tick has work.
    pub fn active(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    pub fn shown(&self) -> bool {
        matches!(self.phase, Phase::Shown { .. })
    }

    /// The job the hover still wants, if one is out.
    pub fn loading_seq(&self) -> Option<u64> {
        match self.phase {
            Phase::Loading { seq, .. } => Some(seq),
            _ => None,
        }
    }

    /// The pointer moved: `key` is the image link under it, if any. Moving along the same link changes nothing.
    pub fn point(&mut self, key: Option<K>, now: u64) -> Pointed {
        if key.is_some() && self.key() == key.as_ref() {
            return Pointed::default();
        }
        let came_down = self.shown();
        self.phase = Phase::Idle;
        let Some(key) = key else {
            self.cooled = None;
            return Pointed {
                came_down,
                started: false,
            };
        };
        if self.cooled.as_ref() == Some(&key) {
            return Pointed {
                came_down,
                started: false,
            };
        }
        self.cooled = None;
        self.phase = Phase::Dwell { key, since: now };
        Pointed {
            came_down,
            started: true,
        }
    }

    /// Asked every tick with what is under the cell NOW, never trusting the stored key: output or a scroll under a
    /// still pointer takes the picture down. True when one was up.
    pub fn recheck(&mut self, here: Option<&K>) -> bool {
        if self.key().is_none() || self.key() == here {
            return false;
        }
        let came_down = self.shown();
        self.phase = Phase::Idle;
        self.cooled = None;
        came_down
    }

    /// A key, a press, a menu or a resize: down, and this link stays quiet until the pointer has left it.
    pub fn dismiss(&mut self) -> bool {
        let came_down = self.shown();
        if let Some(k) = self.key().cloned() {
            self.cooled = Some(k);
        }
        self.phase = Phase::Idle;
        came_down
    }

    /// The rest is over: the sequence number to load under, once.
    pub fn fire(&mut self, now: u64) -> Option<u64> {
        let Phase::Dwell { key, since } = &self.phase else {
            return None;
        };
        if now.saturating_sub(*since) < DWELL_MS {
            return None;
        }
        let key = key.clone();
        let seq = next_seq();
        self.phase = Phase::Loading { key, seq };
        Some(seq)
    }

    /// The job answered. True when the picture goes up now; a refused path stays quiet until the pointer leaves.
    pub fn arrived(&mut self, seq: u64, ok: bool) -> bool {
        match &self.phase {
            Phase::Loading { key, seq: s } if *s == seq => {
                let key = key.clone();
                if ok {
                    self.phase = Phase::Shown { key };
                    true
                } else {
                    self.cooled = Some(key);
                    self.phase = Phase::Idle;
                    false
                }
            }
            _ => false,
        }
    }
}

/// Numbers are never reused, so an answer from before the setting was turned off and on again is nobody's.
fn next_seq() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// One picture to fetch: what the link says and the folder it was found from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbJob {
    pub seq: u64,
    pub base: String,
    pub text: String,
}

/// Where an answer goes: the job's number and the picture, or None for anything refused, abandoned or broken.
pub type ThumbPost = Arc<dyn Fn(u64, Option<Arc<Thumb>>) + Send + Sync>;

/// One job at a time, and only the newest waits. A thread lives only for its job, so nothing runs while nothing is hovered.
pub struct ThumbWorker {
    cache: Arc<Mutex<ThumbCache>>,
    current: Arc<AtomicU64>,
    running: Option<u64>,
    next: Option<ThumbJob>,
    home: String,
    post: ThumbPost,
}

impl ThumbWorker {
    pub fn new(home: String, post: ThumbPost) -> Self {
        Self {
            cache: Arc::default(),
            current: Arc::new(AtomicU64::new(0)),
            running: None,
            next: None,
            home,
            post,
        }
    }

    /// Which job is still wanted; anything else gives up at its next look.
    pub fn want(&mut self, seq: Option<u64>) {
        self.current.store(seq.unwrap_or(0), Ordering::SeqCst);
    }

    pub fn submit(&mut self, job: ThumbJob) {
        self.current.store(job.seq, Ordering::SeqCst);
        if self.running.is_some() {
            self.next = Some(job);
        } else {
            self.spawn(job);
        }
    }

    /// A job's answer reached the UI thread: the next one may start.
    pub fn finished(&mut self, seq: u64) {
        if self.running != Some(seq) {
            return;
        }
        self.running = None;
        if let Some(job) = self.next.take()
            && job.seq == self.current.load(Ordering::SeqCst)
        {
            self.spawn(job);
        }
    }

    fn spawn(&mut self, job: ThumbJob) {
        self.running = Some(job.seq);
        let seq = job.seq;
        let cache = Arc::clone(&self.cache);
        let current = Arc::clone(&self.current);
        let home = self.home.clone();
        let post = Arc::clone(&self.post);
        let spawned = thread::Builder::new()
            .name("flare-thumb".into())
            .spawn(move || {
                let stale = || current.load(Ordering::SeqCst) != seq;
                // A picture that breaks the decoder costs one thumbnail, never the app.
                let got = catch_unwind(AssertUnwindSafe(|| {
                    load_thumb(&job.base, &job.text, &home, &cache, &stale)
                }))
                .ok()
                .flatten();
                post(seq, got);
            });
        if spawned.is_err() {
            (self.post)(seq, None);
        }
    }

    #[cfg(test)]
    fn cache(&self) -> Arc<Mutex<ThumbCache>> {
        Arc::clone(&self.cache)
    }
}

/// The picture under a vetted path may be read: the name as written and as the file system names it are both pictures,
/// on a local drive, with no NUL. What a click asks before it shows a file in the panel.
pub fn thumb_target(abs: &str, real: &str) -> bool {
    classify_resolved(abs, real) == Some(PanelKind::Image) && loadable(real, &PanelKind::Image)
}

/// On a worker only: vet the path as a click does, answer from memory when this version is known, else decode, sharing
/// the panel's one-at-a-time lock.
fn load_thumb(
    base: &str,
    text: &str,
    home: &str,
    cache: &Mutex<ThumbCache>,
    stale: &dyn Fn() -> bool,
) -> Option<Arc<Thumb>> {
    let (abs, real) = resolve_openable_file(base, text, home)?;
    if !thumb_target(&abs, &real) || stale() {
        return None;
    }
    let key = key_of(&real)?;
    if let Some(hit) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return Some(hit);
    }
    let _one = crate::panel_ctl::DECODE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if stale() {
        return None;
    }
    let d = preview::decode_thumb(Path::new(&real), stale).ok()?;
    let name = real.rsplit(['\\', '/']).next().unwrap_or(&real).to_string();
    let thumb = Arc::new(Thumb {
        width: d.width,
        height: d.height,
        rgba: d.rgba,
        name,
        source_width: d.source_width,
        source_height: d.source_height,
    });
    cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .put(key, Arc::clone(&thumb));
    Some(thumb)
}

fn key_of(real: &str) -> Option<ThumbKey> {
    // sync-ok: runs on a worker thread only
    let meta = std::fs::metadata(real).ok()?;
    let modified_ns = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(ThumbKey {
        path: real.to_string(),
        len: meta.len(),
        modified_ns,
    })
}

/// The box around a link's cells that are on the screen, in css px from the grid's corner. A wrapped path is the box
/// around all its rows, so the picture covers none of them.
pub fn item_box(
    pieces: &[Piece],
    top: usize,
    rows: usize,
    m: &CellMetrics,
    scale: f32,
) -> Option<BoxF> {
    let scale = f64::from(scale);
    let mut edges: Option<(i32, i32, i32, i32)> = None;
    for p in pieces.iter().filter(|p| p.row >= top && p.row < top + rows) {
        let row = p.row - top;
        let (l, t) = cell_origin(p.col, row, m);
        let (r, _) = cell_origin(p.col + p.width, row, m);
        let b = t + m.cell_h as i32;
        edges = Some(edges.map_or((l, t, r, b), |e| {
            (e.0.min(l), e.1.min(t), e.2.max(r), e.3.max(b))
        }));
    }
    let (l, t, r, b) = edges?;
    Some(BoxF {
        left: f64::from(l) / scale,
        top: f64::from(t) / scale,
        width: f64::from(r - l) / scale,
        height: f64::from(b - t) / scale,
    })
}

/// The size to draw the picture so the bubble fits beside the line, on the side with more room. None when so little
/// room is left that a thumbnail would be useless. `chrome` is what the bubble adds around the picture.
pub fn fit_pic(
    pic: (f64, f64),
    chrome: (f64, f64),
    item: BoxF,
    view: (f64, f64),
) -> Option<(f64, f64)> {
    let below = view.1 - TIP_MARGIN - (item.top + item.height + TIP_GAP);
    let above = item.top - TIP_GAP - TIP_MARGIN;
    let room_h = below.max(above) - chrome.1;
    let room_w = view.0 - 2.0 * TIP_MARGIN - chrome.0;
    if room_h <= 0.0 || room_w <= 0.0 || pic.0 <= 0.0 || pic.1 <= 0.0 {
        return None;
    }
    let k = (room_w / pic.0).min(room_h / pic.1).min(1.0);
    // Down to whole pixels, so the rounding can never make it too big for the room it was fitted to.
    let (w, h) = ((pic.0 * k + 1e-6).floor(), (pic.1 * k + 1e-6).floor());
    (w >= 1.0 && h >= 1.0 && (k >= 1.0 || w.max(h) >= PIC_MIN)).then_some((w, h))
}

/// Above or below the line. None rather than over it: `place_tip` keeps a tip inside the window even when that lands
/// on the item, and a picture must never hide the path it belongs to.
pub fn thumb_spot(item: BoxF, size: (f64, f64), view: (f64, f64)) -> Option<TipSpot> {
    let spot = place_tip(item, size, view, TipSide::Below);
    let covers = spot.y < item.top + item.height && spot.y + size.1 > item.top;
    (!covers).then_some(spot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_render::glyph::{FakeGlyphs, GlyphSource};
    use flare_render::metrics::cell_metrics;
    use std::time::{Duration, Instant};

    fn thumb(bytes: usize) -> Arc<Thumb> {
        Arc::new(Thumb {
            width: 1,
            height: 1,
            rgba: vec![0; bytes],
            name: "a.png".into(),
            source_width: 1,
            source_height: 1,
        })
    }

    fn key(path: &str, len: u64) -> ThumbKey {
        ThumbKey {
            path: path.into(),
            len,
            modified_ns: 7,
        }
    }

    #[test]
    fn the_caption_is_the_file_name_and_the_pictures_own_size() {
        let t = Thumb {
            width: 256,
            height: 128,
            rgba: Vec::new(),
            name: "photo.png".into(),
            source_width: 1920,
            source_height: 960,
        };
        assert_eq!(t.caption(), "photo.png  1920 x 960");
    }

    #[test]
    fn the_cache_keeps_32_and_lets_the_least_recently_used_go() {
        let mut c = ThumbCache::default();
        for i in 0..32 {
            c.put(key(&format!("{i}.png"), 1), thumb(10));
        }
        assert!(c.get(&key("0.png", 1)).is_some(), "used, so now the newest");
        c.put(key("new.png", 1), thumb(10));
        assert_eq!(c.len(), 32);
        assert!(c.get(&key("0.png", 1)).is_some());
        assert!(c.get(&key("1.png", 1)).is_none(), "the oldest went");
    }

    #[test]
    fn the_cache_stays_near_its_byte_budget() {
        let mut c = ThumbCache::default();
        for i in 0..32 {
            c.put(key(&format!("{i}.png"), 1), thumb(1024 * 1024));
        }
        assert_eq!(c.bytes(), CACHE_BYTES);
        assert_eq!(c.len(), 8);
    }

    #[test]
    fn an_edited_picture_is_a_miss_and_its_old_version_goes() {
        let mut c = ThumbCache::default();
        c.put(key("a.png", 1), thumb(10));
        assert!(c.get(&key("a.png", 2)).is_none(), "another length");
        c.put(key("a.png", 2), thumb(20));
        assert_eq!(c.len(), 1);
        assert_eq!(c.bytes(), 20);
        assert!(c.get(&key("a.png", 1)).is_none());
    }

    #[test]
    fn the_picture_waits_for_the_full_rest() {
        let mut h = ThumbHover::<u32>::new();
        assert!(h.point(Some(1), 1000).started);
        assert_eq!(h.fire(1349), None);
        let seq = h.fire(1350).expect("the rest is over");
        assert_eq!(h.loading_seq(), Some(seq));
        assert_eq!(h.fire(2000), None, "fired once");
        assert!(h.arrived(seq, true));
        assert!(h.shown());
    }

    #[test]
    fn moving_along_the_same_link_does_not_restart_the_rest() {
        let mut h = ThumbHover::<u32>::new();
        h.point(Some(1), 1000);
        assert_eq!(h.point(Some(1), 1300), Pointed::default());
        assert!(h.fire(1350).is_some());
    }

    #[test]
    fn another_link_restarts_the_rest_and_takes_the_picture_down() {
        let mut h = ThumbHover::<u32>::new();
        h.point(Some(1), 0);
        let seq = h.fire(350).unwrap();
        h.arrived(seq, true);
        let p = h.point(Some(2), 400);
        assert!(p.came_down && p.started);
        assert_eq!(h.fire(749), None);
        assert!(h.fire(750).is_some());
    }

    #[test]
    fn a_late_answer_for_a_link_the_pointer_left_shows_nothing() {
        let mut h = ThumbHover::<u32>::new();
        h.point(Some(1), 0);
        let seq = h.fire(350).unwrap();
        h.point(Some(2), 360);
        assert!(!h.arrived(seq, true));
        assert!(!h.shown());
    }

    #[test]
    fn a_still_pointer_loses_the_picture_when_the_link_under_it_changes() {
        let mut h = ThumbHover::<u32>::new();
        h.point(Some(1), 0);
        let seq = h.fire(350).unwrap();
        h.arrived(seq, true);
        assert!(!h.recheck(Some(&1)), "the same link keeps it");
        assert!(h.shown());
        assert!(h.recheck(Some(&2)), "another link took its place");
        assert!(!h.active());
    }

    #[test]
    fn a_key_sends_it_away_until_the_pointer_has_left_the_link() {
        let mut h = ThumbHover::<u32>::new();
        h.point(Some(1), 0);
        let seq = h.fire(350).unwrap();
        h.arrived(seq, true);
        assert!(h.dismiss());
        let p = h.point(Some(1), 500);
        assert!(
            !p.started,
            "a nudge on the same link does not bring it back"
        );
        assert_eq!(h.fire(5000), None);
        h.point(None, 600);
        assert!(h.point(Some(1), 700).started, "back again after leaving");
    }

    #[test]
    fn a_refused_path_is_not_asked_about_again_while_the_pointer_stays() {
        let mut h = ThumbHover::<u32>::new();
        h.point(Some(1), 0);
        let seq = h.fire(350).unwrap();
        assert!(!h.arrived(seq, false));
        assert!(!h.point(Some(1), 400).started);
    }

    #[test]
    fn the_vetting_is_a_clicks_for_a_network_path_a_nul_and_a_link_that_changes_kind() {
        assert!(thumb_target(r"C:\a\p.png", r"C:\a\p.png"));
        assert!(thumb_target(r"C:\a\link.png", r"C:\b\real.JPG"));
        assert!(!thumb_target(r"\\srv\share\p.png", r"\\srv\share\p.png"));
        assert!(!thumb_target(r"C:\a\p.png", r"\\srv\share\p.png"));
        assert!(!thumb_target("C:\\a\\p\0.png", "C:\\a\\p\0.png"));
        assert!(!thumb_target(r"C:\a\link.png", r"C:\b\run.exe"));
        assert!(!thumb_target(r"C:\a\link.png", r"C:\b\notes.txt"));
        assert!(!thumb_target(r"C:\a\notes.txt", r"C:\a\notes.txt"));
    }

    fn png(path: &Path, w: u32, h: u32) {
        image::RgbaImage::from_pixel(w, h, image::Rgba([9, 8, 7, 255]))
            .save(path)
            .unwrap();
    }

    #[derive(Default)]
    struct Answers(Mutex<Vec<(u64, Option<Arc<Thumb>>)>>);

    fn worker(dir: &Path, answers: &Arc<Answers>) -> (ThumbWorker, String) {
        let a = Arc::clone(answers);
        let w = ThumbWorker::new(
            std::env::temp_dir().to_string_lossy().to_string(),
            Arc::new(move |seq, t| a.0.lock().unwrap().push((seq, t))),
        );
        (w, dir.to_string_lossy().to_string())
    }

    fn wait_answers(answers: &Answers, n: usize) -> Vec<(u64, Option<Arc<Thumb>>)> {
        let t = Instant::now();
        while answers.0.lock().unwrap().len() < n {
            assert!(t.elapsed() < Duration::from_secs(10), "no answer");
            thread::sleep(Duration::from_millis(5));
        }
        answers.0.lock().unwrap().clone()
    }

    #[cfg_attr(
        not(windows),
        ignore = "Windows path grammar in path links and openers; macOS phase 2 of the port plan (POSIX paths)"
    )]
    #[test]
    fn a_picture_is_decoded_once_and_comes_from_memory_until_it_is_edited() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("p.png");
        png(&file, 600, 300);
        let answers = Arc::<Answers>::default();
        let (mut w, base) = worker(dir.path(), &answers);
        let job = |seq| ThumbJob {
            seq,
            base: base.clone(),
            text: "p.png".into(),
        };
        w.submit(job(1));
        let got = wait_answers(&answers, 1);
        let first = got[0].1.clone().expect("a picture");
        assert_eq!((first.width, first.height), (256, 128));
        assert_eq!(first.caption(), "p.png  600 x 300");
        w.finished(1);

        w.submit(job(2));
        let got = wait_answers(&answers, 2);
        assert!(
            Arc::ptr_eq(&first, got[1].1.as_ref().unwrap()),
            "the same pixels, from memory"
        );
        w.finished(2);

        png(&file, 100, 100);
        w.submit(job(3));
        let got = wait_answers(&answers, 3);
        assert_eq!(got[2].1.as_ref().unwrap().caption(), "p.png  100 x 100");
        assert_eq!(w.cache().lock().unwrap().len(), 1);
    }

    #[test]
    fn a_job_nobody_wants_answers_nothing_to_show() {
        let dir = tempfile::tempdir().unwrap();
        png(&dir.path().join("p.png"), 4, 4);
        let answers = Arc::<Answers>::default();
        let (mut w, base) = worker(dir.path(), &answers);
        w.want(Some(99));
        // Stale from its first look: `spawn` is given a job that is not the wanted one.
        w.spawn(ThumbJob {
            seq: 1,
            base,
            text: "p.png".into(),
        });
        assert!(wait_answers(&answers, 1)[0].1.is_none());
    }

    #[cfg_attr(
        not(windows),
        ignore = "Windows path grammar in path links and openers; macOS phase 2 of the port plan (POSIX paths)"
    )]
    #[test]
    fn only_the_newest_job_waits_behind_a_running_one() {
        let answers = Arc::<Answers>::default();
        let dir = tempfile::tempdir().unwrap();
        for n in ["a.png", "b.png", "c.png"] {
            png(&dir.path().join(n), 4, 4);
        }
        let (mut w, base) = worker(dir.path(), &answers);
        let job = |seq, text: &str| ThumbJob {
            seq,
            base: base.clone(),
            text: text.into(),
        };
        w.submit(job(1, "a.png"));
        w.submit(job(2, "b.png"));
        w.submit(job(3, "c.png"));
        wait_answers(&answers, 1);
        w.finished(1);
        let got = wait_answers(&answers, 2);
        assert_eq!(got[1].0, 3, "the middle job never ran");
        assert_eq!(got[1].1.as_ref().unwrap().name, "c.png");
    }

    #[test]
    fn a_folder_called_like_a_picture_and_a_broken_file_answer_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("x.png")).unwrap();
        std::fs::write(dir.path().join("bad.png"), b"not a picture").unwrap();
        let answers = Arc::<Answers>::default();
        let (mut w, base) = worker(dir.path(), &answers);
        for (seq, text) in [(1, "x.png"), (2, "bad.png"), (3, "gone.png")] {
            w.submit(ThumbJob {
                seq,
                base: base.clone(),
                text: text.into(),
            });
            assert!(
                wait_answers(&answers, seq as usize)[seq as usize - 1]
                    .1
                    .is_none()
            );
            w.finished(seq);
        }
    }

    fn metrics(scale: f32) -> CellMetrics {
        cell_metrics(&FakeGlyphs::new(8, 16).font_units(), scale)
    }

    #[test]
    fn the_box_covers_every_row_of_a_wrapped_path_on_the_screen() {
        let m = metrics(1.0);
        let pieces = [
            Piece {
                row: 10,
                col: 5,
                width: 3,
            },
            Piece {
                row: 11,
                col: 0,
                width: 2,
            },
        ];
        let b = item_box(&pieces, 10, 6, &m, 1.0).unwrap();
        let (x0, y0) = cell_origin(0, 0, &m);
        let (x8, _) = cell_origin(8, 0, &m);
        assert_eq!(b.left, f64::from(x0));
        assert_eq!(b.top, f64::from(y0));
        assert_eq!(b.width, f64::from(x8 - x0));
        assert_eq!(b.height, f64::from(m.cell_h) * 2.0);
        // Only the second row is on the screen when the view starts there.
        let b = item_box(&pieces, 11, 6, &m, 1.0).unwrap();
        assert_eq!(b.height, f64::from(m.cell_h));
        assert!(item_box(&pieces, 20, 6, &m, 1.0).is_none());
    }

    #[test]
    fn the_box_is_in_css_pixels_at_every_scale() {
        for scale in [1.0_f32, 1.25, 1.5, 2.25] {
            let m = metrics(scale);
            let p = [Piece {
                row: 3,
                col: 2,
                width: 4,
            }];
            let b = item_box(&p, 0, 6, &m, scale).unwrap();
            let (l, t) = cell_origin(2, 3, &m);
            assert_eq!(b.left, f64::from(l) / f64::from(scale), "{scale}");
            assert_eq!(b.top, f64::from(t) / f64::from(scale), "{scale}");
        }
    }

    const VIEW: (f64, f64) = (1200.0, 800.0);

    fn line_at(top: f64) -> BoxF {
        BoxF {
            left: 100.0,
            top,
            width: 200.0,
            height: 18.0,
        }
    }

    #[test]
    fn a_full_size_picture_is_kept_when_there_is_room() {
        assert_eq!(
            fit_pic((256.0, 128.0), (16.0, 34.0), line_at(100.0), VIEW),
            Some((256.0, 128.0))
        );
    }

    #[test]
    fn a_picture_shrinks_to_the_side_with_more_room_and_keeps_its_shape() {
        // 188 px of room over the line, 170 px under it.
        let view = (1200.0, 400.0);
        let (w, h) = fit_pic((256.0, 256.0), (16.0, 34.0), line_at(200.0), view).unwrap();
        assert_eq!(h, 200.0 - TIP_GAP - TIP_MARGIN - 34.0);
        assert_eq!(w, h);
    }

    #[test]
    fn a_window_too_small_for_a_useful_thumbnail_shows_none() {
        let view = (1200.0, 110.0);
        assert_eq!(
            fit_pic((256.0, 256.0), (16.0, 34.0), line_at(50.0), view),
            None
        );
    }

    #[test]
    fn a_tiny_picture_is_not_refused_for_being_tiny() {
        assert_eq!(
            fit_pic((16.0, 16.0), (16.0, 34.0), line_at(100.0), VIEW),
            Some((16.0, 16.0))
        );
    }

    #[test]
    fn the_bubble_goes_below_the_line_and_above_it_at_the_bottom() {
        let below = thumb_spot(line_at(100.0), (272.0, 162.0), VIEW).unwrap();
        assert_eq!(below.side, TipSide::Below);
        assert!(below.y >= 118.0);
        let above = thumb_spot(line_at(760.0), (272.0, 162.0), VIEW).unwrap();
        assert_eq!(above.side, TipSide::Above);
        assert!(above.y + 162.0 <= 760.0);
    }

    #[test]
    fn a_bubble_with_no_room_either_side_never_lands_on_the_line() {
        assert!(thumb_spot(line_at(100.0), (272.0, 400.0), (1200.0, 300.0)).is_none());
    }
}
