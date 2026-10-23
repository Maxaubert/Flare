//! Which paths on screen exist (Prism Terminal's termPathLinks.ts).
//!
//! The painter and the click ask `find_path`; a path nobody has asked about is queued and asked
//! about in pieces, and the tabs that asked hear when one turns out to exist, and only those
//! (#167). The JS module was global state with timers and callbacks; this is an owned struct
//! with an explicit clock and no callbacks: the operations that used to call listeners return
//! the owners to wake instead. Per folder, since `docs/a.pdf` differs in every shell. A "no" is
//! forgotten after `NO_TTL_MS`, a "yes" stays. Text positions are char indices.

use crate::links::{LinkSpan, char_slice, find_links_in, segments_of};
use crate::path_open::{PATHS_MAX, PathHit, PathKind};
use crate::paths::{PathCandidate, find_paths};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

pub const NO_TTL_MS: u64 = 15_000;
pub const MAX_ENTRIES: usize = 5000;
pub const BATCH_MS: u64 = 40;
/// Folders kept per tab for trying a relative path (newest first).
pub const ANCHORS_MAX: usize = 4;
/// How many folders above an anchor a path with a separator is tried in.
pub const ANCESTORS: usize = 4;
/// Folders a tab may ever be taught (each wakes it once).
pub const TAUGHT_MAX: usize = 256;

/// A hit, and the folder it was read from: what the host resolves it in again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathFound {
    pub hit: PathHit,
    pub base: String,
}

/// One range that wears the link colour. `path` is None for a web link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRange {
    pub start: usize,
    pub end: usize,
    pub path: Option<String>,
}

/// One piece of a question for the host: `paths` resolved in `base`, at most `PATHS_MAX`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub base: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Hit(PathFound),
    No,
    Unknown,
}

/// What the cache knows about one path in one folder.
enum Known {
    Hit(PathHit),
    No,
    Unknown,
}

fn is_relative(p: &str) -> bool {
    let mut it = p.chars();
    match it.next() {
        None => true,
        Some('\\' | '/' | '~') => false,
        Some(c) if c.is_ascii_alphabetic() => it.next() != Some(':'),
        Some(_) => true,
    }
}

fn has_separator(p: &str) -> bool {
    p.contains(['\\', '/'])
}

/// Whether `dir` is `cwd` or a folder above it. Such a folder found from a shown folder is the
/// one the shell is already in said another way (a status line prints it on every redraw).
pub fn holds_cwd(dir: &str, cwd: &str) -> bool {
    let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let d = norm(dir);
    let c = norm(cwd);
    c == d || c.starts_with(&format!("{d}\\"))
}

/// The folder above `dir`, or None at a drive's root.
pub fn parent_dir(dir: &str) -> Option<String> {
    let d = dir.trim_end_matches(['\\', '/']);
    let i = d.rfind(['\\', '/'])?;
    let up = &d[..i];
    let b = up.as_bytes();
    if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        None
    } else {
        Some(up.to_string())
    }
}

/// The path candidates in `text` that are not inside a web link, and not a PROMPT: a path that
/// runs straight into `>` (`PS C:\work>`, `C:\work>`) is left alone.
pub fn path_candidates(text: &str) -> Vec<PathCandidate> {
    path_candidates_in(text, None, &[])
}

/// `path_candidates`, with `joins` saying where rows broken by hand start (`cells::by_hand`): a
/// path is read ACROSS each join first, then within each piece as before (PT termPathLinks.ts
/// `pathCandidates(text, links, joins)`). `links` are the web links already found, if any.
pub fn path_candidates_in(
    text: &str,
    links: Option<&[LinkSpan]>,
    joins: &[usize],
) -> Vec<PathCandidate> {
    let chars: Vec<char> = text.chars().collect();
    let found;
    let web: &[LinkSpan] = match links {
        Some(l) => l,
        None => {
            found = find_links_in(text, joins);
            &found
        }
    };
    let keep = |c: &PathCandidate| {
        chars.get(c.end) != Some(&'>') && !web.iter().any(|l| c.start < l.end && l.start < c.end)
    };
    if joins.is_empty() {
        return find_paths(text).into_iter().filter(keep).collect();
    }
    let across = find_paths(text)
        .into_iter()
        .filter(|c| joins.iter().any(|&j| c.start < j && j < c.end));
    let pieces = segments_of(chars.len(), joins)
        .into_iter()
        .flat_map(|(s, e)| {
            find_paths(char_slice(text, s, e))
                .into_iter()
                .map(move |c| PathCandidate {
                    start: c.start + s,
                    end: c.end + s,
                    path: c.path,
                })
        });
    across.chain(pieces).filter(keep).collect()
}

fn key(cwd: &str, path: &str) -> String {
    format!("{cwd}\n{path}")
}

#[derive(Default)]
struct QueuedGroup {
    base: String,
    order: Vec<String>,
    owners: HashMap<String, BTreeSet<String>>,
}

pub struct PathLinks {
    cache: HashMap<String, (Option<PathHit>, u64)>,
    queued: Vec<QueuedGroup>,
    first_queued: Option<u64>,
    in_flight: HashMap<String, BTreeSet<String>>,
    ready: VecDeque<Ask>,
    asking: bool,
    anchors: HashMap<String, Vec<String>>,
    taught: HashMap<String, HashSet<String>>,
    asked: HashSet<String>,
    subscribed: HashSet<String>,
    deferred: Vec<String>,
}

impl Default for PathLinks {
    fn default() -> Self {
        Self::new()
    }
}

impl PathLinks {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            queued: Vec::new(),
            first_queued: None,
            in_flight: HashMap::new(),
            ready: VecDeque::new(),
            asking: false,
            anchors: HashMap::new(),
            taught: HashMap::new(),
            asked: HashSet::new(),
            subscribed: HashSet::new(),
            deferred: Vec::new(),
        }
    }

    pub fn subscribe(&mut self, owner: &str) {
        self.subscribed.insert(owner.to_string());
    }

    pub fn unsubscribe(&mut self, owner: &str) {
        self.subscribed.remove(owner);
    }

    fn known(&self, cwd: &str, path: &str, now_ms: u64) -> Known {
        match self.cache.get(&key(cwd, path)) {
            None => Known::Unknown,
            Some((Some(h), _)) => Known::Hit(h.clone()),
            Some((None, at)) => {
                if now_ms.saturating_sub(*at) > NO_TTL_MS {
                    Known::Unknown
                } else {
                    Known::No
                }
            }
        }
    }

    fn learn_anchor(&mut self, owner: &str, hit: &PathHit) {
        let dir = if hit.kind == PathKind::Dir {
            Some(hit.abs.clone())
        } else {
            parent_dir(&hit.abs)
        };
        let Some(dir) = dir else { return };
        let list = self.anchors.entry(owner.to_string()).or_default();
        if list.first() == Some(&dir) {
            return;
        }
        list.retain(|d| d != &dir);
        list.insert(0, dir.clone());
        list.truncate(ANCHORS_MAX);
        // A relative path already shown plain may name something from here: the tab looks
        // again, ONCE per folder in its life, or a tab showing many folders repaints forever.
        let seen = self.taught.entry(owner.to_string()).or_default();
        if seen.contains(&dir) || seen.len() >= TAUGHT_MAX {
            return;
        }
        seen.insert(dir);
        self.deferred.push(owner.to_string());
    }

    /// Where a relative `path` is tried after `cwd`, in order.
    fn anchor_bases(&self, owner: &str, cwd: &str, path: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let climb = if has_separator(path) { ANCESTORS } else { 0 };
        let cwd_lower = cwd.to_lowercase();
        for a in self.anchors.get(owner).into_iter().flatten() {
            let mut d = Some(a.clone());
            let mut up = 0;
            while let Some(cur) = d {
                if up > climb {
                    break;
                }
                if cur.to_lowercase() != cwd_lower && !out.contains(&cur) {
                    out.push(cur.clone());
                }
                d = parent_dir(&cur);
                up += 1;
            }
        }
        out
    }

    fn find_inner(
        &self,
        owner: &str,
        cwd: &str,
        path: &str,
        now_ms: u64,
        ask: &mut Vec<(String, String)>,
    ) -> Found {
        match self.known(cwd, path, now_ms) {
            Known::Hit(hit) => {
                return Found::Hit(PathFound {
                    hit,
                    base: cwd.to_string(),
                });
            }
            Known::Unknown => {
                ask.push((cwd.to_string(), path.to_string()));
                return Found::Unknown;
            }
            Known::No => {}
        }
        if !is_relative(path) {
            return Found::No;
        }
        let mut unknown = false;
        for base in self.anchor_bases(owner, cwd, path) {
            match self.known(&base, path, now_ms) {
                Known::Hit(b) => {
                    if !(b.kind == PathKind::Dir && holds_cwd(&b.abs, cwd)) {
                        return Found::Hit(PathFound { hit: b, base });
                    }
                }
                Known::Unknown => {
                    unknown = true;
                    ask.push((base, path.to_string()));
                }
                Known::No => {}
            }
        }
        if unknown { Found::Unknown } else { Found::No }
    }

    /// What `path` names for `owner`: from `cwd`, else (a relative path only) from the folders
    /// the tab has shown. Read only: `link_ranges` is what queues the questions.
    pub fn find_path(&self, owner: &str, cwd: &str, path: &str, now_ms: u64) -> Found {
        self.find_inner(owner, cwd, path, now_ms, &mut Vec::new())
    }

    fn request(&mut self, base: &str, paths: &[String], owner: &str, now_ms: u64) {
        let mut queued_any = false;
        for p in paths {
            if let Some(flying) = self.in_flight.get_mut(&key(base, p)) {
                flying.insert(owner.to_string());
                continue;
            }
            let idx = match self.queued.iter().position(|g| g.base == base) {
                Some(i) => i,
                None => {
                    self.queued.push(QueuedGroup {
                        base: base.to_string(),
                        ..Default::default()
                    });
                    self.queued.len() - 1
                }
            };
            let g = &mut self.queued[idx];
            if !g.owners.contains_key(p) {
                g.order.push(p.clone());
            }
            g.owners
                .entry(p.clone())
                .or_default()
                .insert(owner.to_string());
            queued_any = true;
        }
        if queued_any && self.first_queued.is_none() {
            self.first_queued = Some(now_ms);
        }
    }

    /// Everything in `text` that wears the link colour: the web links, and the paths known to
    /// exist from `cwd` (or, for a relative one, from the folders `owner` has shown). Unknown
    /// paths are queued for `owner` on the way. No cwd, or no `\ / .` in the text: web links only.
    pub fn link_ranges(
        &mut self,
        text: &str,
        cwd: Option<&str>,
        owner: &str,
        now_ms: u64,
    ) -> Vec<LinkRange> {
        self.link_ranges_in(text, cwd, owner, now_ms, &[])
    }

    /// The candidates that name something, each with what it names (PT `pathsFound`). A path
    /// across a row broken by hand comes first and, when it exists, covers its pieces:
    /// `C:\a\scrat` + `chpad\b.exe` is one file, and neither piece is a link. When it names
    /// nothing, its pieces are read as they always were. Read only, like `find_path`.
    pub fn paths_found(
        &self,
        owner: &str,
        cwd: &str,
        candidates: Vec<PathCandidate>,
        now_ms: u64,
    ) -> Vec<(PathCandidate, PathFound)> {
        self.paths_found_inner(owner, cwd, candidates, now_ms, &mut Vec::new())
    }

    fn paths_found_inner(
        &self,
        owner: &str,
        cwd: &str,
        candidates: Vec<PathCandidate>,
        now_ms: u64,
        ask: &mut Vec<(String, String)>,
    ) -> Vec<(PathCandidate, PathFound)> {
        let mut out: Vec<(PathCandidate, PathFound)> = Vec::new();
        for c in candidates {
            if out.iter().any(|(o, _)| o.start < c.end && c.start < o.end) {
                continue;
            }
            if let Found::Hit(f) = self.find_inner(owner, cwd, &c.path, now_ms, ask) {
                out.push((c, f));
            }
        }
        out
    }

    /// `link_ranges`, with `joins` saying where rows broken by hand start in `text`
    /// (`cells::by_hand`): a path and a web link are both read across them.
    pub fn link_ranges_in(
        &mut self,
        text: &str,
        cwd: Option<&str>,
        owner: &str,
        now_ms: u64,
        joins: &[usize],
    ) -> Vec<LinkRange> {
        self.asked.remove(owner);
        let links = find_links_in(text, joins);
        let mut out: Vec<LinkRange> = links
            .iter()
            .map(|l| LinkRange {
                start: l.start,
                end: l.end,
                path: None,
            })
            .collect();
        let Some(cwd) = cwd else { return out };
        if !text.contains(['\\', '/', '.']) {
            return out;
        }
        let mut ask: Vec<(String, String)> = Vec::new();
        let candidates = path_candidates_in(text, Some(&links), joins);
        for (c, f) in self.paths_found_inner(owner, cwd, candidates, now_ms, &mut ask) {
            if !is_relative(&c.path) {
                self.learn_anchor(owner, &f.hit);
            }
            out.push(LinkRange {
                start: c.start,
                end: c.end,
                path: Some(c.path),
            });
        }
        if !ask.is_empty() {
            let mut by_base: Vec<(String, Vec<String>)> = Vec::new();
            for (base, p) in ask {
                match by_base.iter_mut().find(|(b, _)| *b == base) {
                    Some((_, l)) => l.push(p),
                    None => by_base.push((base, vec![p])),
                }
            }
            for (base, list) in by_base {
                self.request(&base, &list, owner, now_ms);
            }
            self.asked.insert(owner.to_string());
        }
        out.sort_by_key(|r| r.start);
        out
    }

    /// Whether the last `link_ranges` for `owner` left a question open. Reading clears it.
    pub fn take_asked(&mut self, owner: &str) -> bool {
        self.asked.remove(owner)
    }

    /// Moves queued asks to ready once `BATCH_MS` has passed since the first queued one, and
    /// returns the subscribed owners to wake for a folder they were taught (deferred to this
    /// advance, the old `setTimeout 0`), each once.
    pub fn advance(&mut self, now_ms: u64) -> Vec<String> {
        let mut wakes: Vec<String> = Vec::new();
        for o in std::mem::take(&mut self.deferred) {
            if self.subscribed.contains(&o) && !wakes.contains(&o) {
                wakes.push(o);
            }
        }
        if let Some(first) = self.first_queued
            && now_ms.saturating_sub(first) >= BATCH_MS
        {
            self.first_queued = None;
            for g in std::mem::take(&mut self.queued) {
                for p in &g.order {
                    let owners = g.owners.get(p).cloned().unwrap_or_default();
                    self.in_flight.insert(key(&g.base, p), owners);
                }
                for part in g.order.chunks(PATHS_MAX) {
                    self.ready.push_back(Ask {
                        base: g.base.clone(),
                        paths: part.to_vec(),
                    });
                }
            }
        }
        wakes
    }

    /// The next piece to send, or None when one is in flight or nothing is ready.
    pub fn take_ask(&mut self) -> Option<Ask> {
        if self.asking {
            return None;
        }
        let a = self.ready.pop_front()?;
        self.asking = true;
        Some(a)
    }

    /// The host's answer for `ask` (same order). None: unanswered, nothing is cached and the
    /// keys leave flight (asked again on the next paint). Some(empty): REFUSED, every path is
    /// cached as "no". A short list: indices past its end are not cached. Returns the
    /// subscribed owners of the paths that turned out to exist, each once.
    pub fn answer(
        &mut self,
        ask: &Ask,
        hits: Option<Vec<Option<PathHit>>>,
        now_ms: u64,
    ) -> Vec<String> {
        self.asking = false;
        let mut wake: Vec<String> = Vec::new();
        if let Some(hits) = &hits {
            if self.cache.len() > MAX_ENTRIES {
                self.cache.clear();
            }
            let refused = hits.is_empty();
            for (i, p) in ask.paths.iter().enumerate() {
                if !refused && i >= hits.len() {
                    continue;
                }
                let hit = if refused { None } else { hits[i].clone() };
                let k = key(&ask.base, p);
                if hit.is_some() {
                    for o in self.in_flight.get(&k).into_iter().flatten() {
                        if self.subscribed.contains(o) && !wake.contains(o) {
                            wake.push(o.clone());
                        }
                    }
                }
                self.cache.insert(k, (hit, now_ms));
            }
        }
        for p in &ask.paths {
            self.in_flight.remove(&key(&ask.base, p));
        }
        wake
    }

    /// A tab closed: forget what it taught and what it waits on. The cache is shared and stays.
    pub fn forget_owner(&mut self, owner: &str) {
        self.anchors.remove(owner);
        self.taught.remove(owner);
        self.asked.remove(owner);
        for g in &mut self.queued {
            for s in g.owners.values_mut() {
                s.remove(owner);
            }
        }
        for s in self.in_flight.values_mut() {
            s.remove(owner);
        }
    }

    /// How many tabs anything here is kept for.
    pub fn owners_kept(&self) -> usize {
        let mut s: HashSet<&String> = HashSet::new();
        s.extend(self.anchors.keys());
        s.extend(self.taught.keys());
        s.extend(self.asked.iter());
        s.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Resolver = Box<dyn Fn(&str, &str) -> Option<PathHit>>;

    fn file(abs: &str) -> PathHit {
        PathHit {
            kind: PathKind::File,
            abs: abs.to_string(),
        }
    }

    fn texts(text: &str) -> Vec<String> {
        path_candidates(text).into_iter().map(|c| c.path).collect()
    }

    fn join(base: &str, text: &str) -> String {
        format!("{base}\\{}", text.replace('/', "\\"))
    }

    fn drive_form(text: &str) -> bool {
        text.chars().nth(1) == Some(':')
    }

    /// Drives the clock by 50 ms per round and answers every ask with `resolve`.
    struct Pump {
        pl: PathLinks,
        now: u64,
        wakes: Vec<String>,
        asks: Vec<Ask>,
        resolve: Resolver,
    }

    impl Pump {
        fn new(resolve: Resolver) -> Self {
            Self {
                pl: PathLinks::new(),
                now: 0,
                wakes: Vec::new(),
                asks: Vec::new(),
                resolve,
            }
        }
        fn lr(&mut self, text: &str, cwd: &str, owner: &str) -> Vec<LinkRange> {
            self.pl.link_ranges(text, Some(cwd), owner, self.now)
        }
        fn pump(&mut self) {
            self.now += 50;
            let w = self.pl.advance(self.now);
            self.wakes.extend(w);
            while let Some(ask) = self.pl.take_ask() {
                let hits = ask
                    .paths
                    .iter()
                    .map(|p| (self.resolve)(&ask.base, p))
                    .collect();
                let w = self.pl.answer(&ask, Some(hits), self.now);
                self.wakes.extend(w);
                self.asks.push(ask);
            }
        }
    }

    fn set_resolver(files: &[&str]) -> Resolver {
        let set: HashSet<String> = files.iter().map(|s| s.to_string()).collect();
        Box::new(move |base, text| {
            let abs = if drive_form(text) {
                text.to_string()
            } else {
                join(base, text)
            };
            set.contains(&abs).then(|| file(&abs))
        })
    }

    fn real_ask(pl: &mut PathLinks, text: &str, owner: &str) -> Ask {
        pl.link_ranges(text, Some("C:\\w"), owner, 0);
        pl.advance(40);
        pl.take_ask().expect("an ask")
    }

    fn hits_for(ask: &Ask, real: &[&str]) -> Vec<Option<PathHit>> {
        ask.paths
            .iter()
            .map(|p| {
                real.contains(&p.as_str())
                    .then(|| file(&format!("C:\\w\\{p}")))
            })
            .collect()
    }

    #[test]
    fn path_candidates_leave_the_prompt_alone() {
        assert_eq!(
            texts("PS C:\\Users\\me\\work> ls docs/a.pdf"),
            vec!["docs/a.pdf"]
        );
        assert!(texts("C:\\Users\\me\\work>dir").is_empty());
        assert_eq!(
            texts("it is in C:\\Users\\me\\work now"),
            vec!["C:\\Users\\me\\work"]
        );
    }

    #[test]
    fn path_candidates_leave_urls_to_the_link() {
        assert_eq!(
            texts("see https://github.com/a/b/blob/main/README.md and README.md"),
            vec!["README.md"]
        );
    }

    // Owner, 2026-10-21 (PT #250): a long path Claude printed was not a link. Claude Code breaks
    // its own lines at the window's width, so `C:\...\scrat` and `chpad\...\Flare-win-Setup.exe`
    // each named nothing.
    const EXE: &str = "C:\\t\\scratchpad\\flare-pack\\Flare-win-Setup.exe";

    fn hand_ranges(p: &mut Pump, text: &str, joins: &[usize]) -> Vec<(usize, usize)> {
        for _ in 0..3 {
            p.pl.link_ranges_in(text, Some("C:\\w"), "A", p.now, joins);
            p.pump();
        }
        p.pl.link_ranges_in(text, Some("C:\\w"), "A", p.now, joins)
            .into_iter()
            .map(|r| (r.start, r.end))
            .collect()
    }

    #[test]
    fn a_path_broken_by_hand_is_one_link_over_both_rows() {
        let top = "- Location: C:\\t\\scrat";
        let text = format!("{top}chpad\\flare-pack\\Flare-win-Setup.exe");
        let n = top.chars().count();
        let mut p = Pump::new(set_resolver(&[EXE, "C:\\t\\a.txt"]));
        assert_eq!(
            hand_ranges(&mut p, &text, &[n]),
            vec![(12, text.chars().count())]
        );
        // Without the break known, the top half alone is all there is: no link.
        let mut p = Pump::new(set_resolver(&[EXE, "C:\\t\\a.txt"]));
        assert!(hand_ranges(&mut p, top, &[]).is_empty());
    }

    #[test]
    fn a_path_ending_at_the_edge_keeps_its_link_when_the_joined_text_names_nothing() {
        let top = "see C:\\t\\a.txt";
        let n = top.chars().count();
        let mut p = Pump::new(set_resolver(&[EXE, "C:\\t\\a.txt"]));
        assert_eq!(
            hand_ranges(&mut p, &format!("{top}\\more"), &[n]),
            vec![(4, n)]
        );
    }

    #[test]
    fn a_web_link_is_carried_across_a_hand_break_review_37() {
        let top = "go https://a.io/x";
        let n = top.chars().count();
        let text = format!("{top}/y/z");
        let mut pl = PathLinks::new();
        let r: Vec<(usize, usize)> = pl
            .link_ranges_in(&text, Some("C:\\w"), "A", 0, &[n])
            .into_iter()
            .map(|r| (r.start, r.end))
            .collect();
        assert_eq!(r, vec![(3, text.chars().count())]);
        // The second row is part of the address, not a path of its own.
        assert!(path_candidates_in(&text, None, &[n]).is_empty());
    }

    #[test]
    fn a_hit_wakes_the_tab_that_asked_and_no_other() {
        let mut pl = PathLinks::new();
        pl.subscribe("A");
        pl.subscribe("B");
        let ask = real_ask(&mut pl, "see a.txt", "A");
        let hits = hits_for(&ask, &["a.txt", "b.txt"]);
        assert_eq!(pl.answer(&ask, Some(hits), 50), vec!["A".to_string()]);
    }

    #[test]
    fn an_in_flight_path_wakes_every_asker_once() {
        let mut pl = PathLinks::new();
        pl.subscribe("A");
        pl.subscribe("B");
        let ask = real_ask(&mut pl, "see b.txt", "A");
        pl.link_ranges("see b.txt", Some("C:\\w"), "B", 40);
        pl.advance(80);
        assert!(pl.take_ask().is_none());
        let mut w = pl.answer(&ask, Some(hits_for(&ask, &["b.txt"])), 90);
        w.sort();
        assert_eq!(w, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn a_path_that_names_nothing_wakes_nobody() {
        let mut pl = PathLinks::new();
        pl.subscribe("A");
        let ask = real_ask(&mut pl, "see nope.txt", "A");
        assert!(pl.answer(&ask, Some(vec![None]), 50).is_empty());
    }

    #[test]
    fn an_unsubscribed_owner_hears_nothing() {
        let mut pl = PathLinks::new();
        pl.subscribe("A");
        pl.unsubscribe("A");
        let ask = real_ask(&mut pl, "see a.txt", "A");
        assert!(
            pl.answer(&ask, Some(hits_for(&ask, &["a.txt"])), 50)
                .is_empty()
        );
    }

    #[test]
    fn take_asked_says_once() {
        let mut p = Pump::new(set_resolver(&["C:\\w\\a.txt"]));
        p.lr("see a.txt", "C:\\w", "A");
        assert!(p.pl.take_asked("A"));
        assert!(!p.pl.take_asked("A"));
        assert!(!p.pl.take_asked("B"));
        p.pump();
        let r = p.lr("see a.txt", "C:\\w", "A");
        assert_eq!(
            r,
            vec![LinkRange {
                start: 4,
                end: 9,
                path: Some("a.txt".into())
            }]
        );
        assert!(!p.pl.take_asked("A"));
    }

    #[test]
    fn a_line_waits_for_a_path_in_flight() {
        let mut pl = PathLinks::new();
        pl.link_ranges("see a.txt", Some("C:\\w"), "A", 0);
        assert!(pl.take_asked("A"));
        pl.advance(40);
        assert!(pl.take_ask().is_some());
        pl.link_ranges("again a.txt", Some("C:\\w"), "A", 41);
        assert!(pl.take_asked("A"));
    }

    #[test]
    fn parent_dir_climbs_and_stops_at_the_drive() {
        assert_eq!(parent_dir("C:\\a\\b\\"), Some("C:\\a".to_string()));
        assert_eq!(parent_dir("C:\\a"), None);
        assert_eq!(parent_dir("x"), None);
    }

    const REPO: &str = "C:\\c\\Github\\Repo";
    const MISC: &str = "C:\\c\\zz-Misc";

    fn misc_pump() -> Pump {
        Pump::new(set_resolver(&[
            "C:\\c\\zz-Misc\\dto\\b\\out\\b.mp4",
            "C:\\c\\zz-Misc\\dto\\a\\out\\a.mp4",
            "C:\\c\\zz-Misc\\dto\\b\\out\\notes.md",
        ]))
    }

    fn teach_full_path(p: &mut Pump) {
        p.pl.subscribe("A");
        let full = "at C:\\c\\zz-Misc\\dto\\b\\out\\b.mp4.";
        p.lr(full, REPO, "A");
        p.pump();
        p.lr(full, REPO, "A");
        p.pump();
    }

    #[test]
    fn a_relative_path_lights_up_from_a_shown_folder() {
        let mut p = misc_pump();
        let rel = "see dto\\a\\out\\a.mp4 now";
        p.lr(rel, REPO, "A");
        p.pump();
        assert!(p.lr(rel, REPO, "A").is_empty());
        teach_full_path(&mut p);
        assert!(p.wakes.contains(&"A".to_string()));
        p.lr(rel, REPO, "A");
        p.pump();
        assert_eq!(
            p.lr(rel, REPO, "A"),
            vec![LinkRange {
                start: 4,
                end: 19,
                path: Some("dto\\a\\out\\a.mp4".into())
            }]
        );
        match p.pl.find_path("A", REPO, "dto\\a\\out\\a.mp4", p.now) {
            Found::Hit(f) => assert_eq!(f.base, MISC),
            other => panic!("{other:?}"),
        }
        p.lr(rel, REPO, "B");
        p.pump();
        assert!(p.lr(rel, REPO, "B").is_empty());
    }

    #[test]
    fn a_bare_name_is_tried_in_the_shown_folder_only() {
        let mut p = misc_pump();
        teach_full_path(&mut p);
        p.lr("see notes.md", REPO, "A");
        p.pump();
        p.lr("see notes.md", REPO, "A");
        p.pump();
        let out = "C:\\c\\zz-Misc\\dto\\b\\out";
        match p.pl.find_path("A", REPO, "notes.md", p.now) {
            Found::Hit(f) => assert_eq!(f.base, out),
            other => panic!("{other:?}"),
        }
        let bases: Vec<&str> = p
            .asks
            .iter()
            .filter(|a| a.paths.iter().any(|x| x == "notes.md"))
            .map(|a| a.base.as_str())
            .collect();
        assert_eq!(bases, vec![REPO, out]);
    }

    #[test]
    fn holds_cwd_folder_and_above() {
        let cwd = "C:\\c\\Github-loftspace\\Stutter";
        assert!(holds_cwd("C:\\c\\Github-loftspace\\Stutter", cwd));
        assert!(holds_cwd("c:/C/github-loftspace", cwd));
        assert!(!holds_cwd("C:\\c\\Github", cwd));
        assert!(!holds_cwd("C:\\c\\Github-loftspace\\Stutter\\src", cwd));
    }

    #[test]
    fn the_status_line_never_becomes_a_link() {
        let root = "C:\\c";
        let cwd = "C:\\c\\Github-loftspace\\Stutter";
        let dirs: HashSet<String> = [
            root.to_string(),
            format!("{root}\\Github-loftspace"),
            cwd.to_string(),
            format!("{root}\\Github"),
        ]
        .into_iter()
        .collect();
        let files: HashSet<String> = [format!("{cwd}\\src\\a.ts")].into_iter().collect();
        let mut p = Pump::new(Box::new(move |base, text| {
            let abs = if drive_form(text) {
                text.to_string()
            } else {
                join(base, text)
            };
            if dirs.contains(&abs) {
                Some(PathHit {
                    kind: PathKind::Dir,
                    abs,
                })
            } else if files.contains(&abs) {
                Some(file(&abs))
            } else {
                None
            }
        }));
        p.pl.subscribe("A");
        let full = format!("see {cwd}\\src\\a.ts");
        p.lr(&full, cwd, "A");
        p.pump();
        p.lr(&full, cwd, "A");
        p.pump();
        let status = "Github-loftspace/Stutter (Sonnet 5.5)";
        for _ in 0..3 {
            p.lr(status, cwd, "A");
            p.pump();
        }
        assert!(p.lr(status, cwd, "A").is_empty());
        assert_eq!(
            p.pl.find_path("A", cwd, "Github-loftspace/Stutter", p.now),
            Found::No
        );
    }

    #[test]
    fn many_folders_wake_once_per_folder_then_stop() {
        let mut p = Pump::new(Box::new(|_, text| Some(file(text))));
        p.pl.subscribe("A");
        let lines: Vec<String> = (1..=9)
            .map(|n| format!("see C:\\d{n}\\f.txt and rel\\x.txt"))
            .collect();
        let mut wakes = 0usize;
        let mut seen = 0usize;
        for l in &lines {
            p.lr(l, "C:\\cwd", "A");
        }
        let mut settled = 0usize;
        for round in 0..200 {
            p.pump();
            let new: Vec<String> = p.wakes[seen..].to_vec();
            seen = p.wakes.len();
            for o in new {
                if o == "A" {
                    wakes += 1;
                    if wakes < 1000 {
                        for l in &lines {
                            p.lr(l, "C:\\cwd", "A");
                        }
                    }
                }
            }
            if round == 99 {
                settled = wakes;
            }
        }
        assert_eq!(wakes, settled);
        assert!(wakes <= 1 + 9, "{wakes}");
    }

    #[test]
    fn a_long_list_is_asked_in_pieces_of_300() {
        let text = (0..650)
            .map(|i| format!("f{i}.txt"))
            .collect::<Vec<_>>()
            .join(" ");
        let mut pl = PathLinks::new();
        pl.link_ranges(&text, Some("C:\\w"), "A", 0);
        pl.advance(40);
        let mut sizes = Vec::new();
        while let Some(ask) = pl.take_ask() {
            assert!(pl.take_ask().is_none());
            sizes.push(ask.paths.len());
            let hits = ask
                .paths
                .iter()
                .map(|p| Some(file(&format!("C:\\w\\{p}"))))
                .collect();
            pl.answer(&ask, Some(hits), 50);
        }
        assert_eq!(sizes, vec![300, 300, 50]);
        match pl.find_path("A", "C:\\w", "f400.txt", 60) {
            Found::Hit(f) => assert_eq!(f.hit.abs, "C:\\w\\f400.txt"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_short_answer_is_not_read_as_missing() {
        let mut pl = PathLinks::new();
        let ask = real_ask(&mut pl, "a.txt b.txt", "A");
        pl.answer(&ask, Some(vec![Some(file("C:\\w\\a.txt"))]), 50);
        match pl.find_path("A", "C:\\w", "a.txt", 60) {
            Found::Hit(f) => assert_eq!(f.hit.kind, PathKind::File),
            other => panic!("{other:?}"),
        }
        assert_eq!(pl.find_path("A", "C:\\w", "b.txt", 60), Found::Unknown);
    }

    #[test]
    fn forget_owner_drops_what_the_tab_taught() {
        let mut pl = PathLinks::new();
        pl.link_ranges("see C:\\d\\f.txt", Some("C:\\cwd"), "A", 0);
        pl.advance(50);
        let ask = pl.take_ask().expect("ask");
        let hits = ask.paths.iter().map(|p| Some(file(p))).collect();
        pl.answer(&ask, Some(hits), 50);
        pl.link_ranges("see C:\\d\\f.txt and rel\\x.txt", Some("C:\\cwd"), "A", 60);
        assert_eq!(pl.owners_kept(), 1);
        pl.forget_owner("A");
        assert_eq!(pl.owners_kept(), 0);
    }
}
