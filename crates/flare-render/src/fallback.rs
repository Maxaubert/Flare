//! The lazy fallback worker (audit M2): finds which lazy family covers a character, and parses its
//! font files, on a thread of its own so a paint never waits on the disk. The rasteriser sends a
//! character and gets back the family's parsed faces; it only pushes them into its database.

use crate::font_catalog::{LAZY_FALLBACK_FAMILIES, files_for_family, installed_faces};
use cosmic_text::fontdb::{Database, FaceInfo};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// One character to find a family for, and the families that must not be offered again.
pub struct Request {
    pub ch: char,
    pub skip: Vec<&'static str>,
}

pub enum Answer {
    /// The next family whose cmap has the character, with its faces parsed.
    Found {
        ch: char,
        family: &'static str,
        faces: Vec<FaceInfo>,
    },
    /// No family left that has it.
    None { ch: char },
}

/// Shared with the worker: how many cmaps it read, and the flag that says an answer waits.
#[derive(Clone, Default)]
pub struct Shared {
    pub ready: Arc<AtomicBool>,
    pub cmap_reads: Arc<AtomicUsize>,
    /// Test hook: while set and false the worker holds every request (a slow disk).
    pub gate: Option<Arc<AtomicBool>>,
    /// Called from the worker after each answer, so an idle window asks for a frame (on the GPU
    /// path nothing else would: the dirty flag is read only while a frame is being made).
    pub wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

pub struct Worker {
    tx: Sender<Request>,
    rx: Mutex<Receiver<Answer>>,
}

impl Worker {
    pub fn spawn(shared: Shared) -> Self {
        let (tx, req_rx) = channel::<Request>();
        let (ans_tx, rx) = channel::<Answer>();
        let spawned = std::thread::Builder::new()
            .name("flare-font-fallback".into())
            .spawn(move || run(req_rx, ans_tx, shared));
        if let Err(e) = spawned {
            log::warn!("the font fallback worker did not start: {e}");
        }
        Self {
            tx,
            rx: Mutex::new(rx),
        }
    }

    pub fn send(&self, req: Request) {
        let _ = self.tx.send(req);
    }

    pub fn try_recv(&self) -> Option<Answer> {
        self.rx.lock().ok()?.try_recv().ok()
    }
}

fn run(rx: Receiver<Request>, tx: Sender<Answer>, shared: Shared) {
    let mut faces = None;
    let mut coverage: HashMap<&'static str, Vec<(u32, u32)>> = HashMap::new();
    while let Ok(req) = rx.recv() {
        if let Some(gate) = &shared.gate {
            while !gate.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        let installed = faces.get_or_insert_with(installed_faces);
        let answer = find(&req, installed, &mut coverage, &shared);
        if tx.send(answer).is_err() {
            return;
        }
        shared.ready.store(true, Ordering::Release);
        if let Some(wake) = &shared.wake {
            wake();
        }
    }
}

fn find(
    req: &Request,
    installed: &[crate::font_catalog::FaceFile],
    coverage: &mut HashMap<&'static str, Vec<(u32, u32)>>,
    shared: &Shared,
) -> Answer {
    let c = req.ch as u32;
    for &family in LAZY_FALLBACK_FAMILIES {
        if req.skip.contains(&family) {
            continue;
        }
        let files = files_for_family(installed, family);
        let ranges = coverage.entry(family).or_insert_with(|| {
            shared.cmap_reads.fetch_add(1, Ordering::Relaxed);
            read_ranges(&files)
        });
        if !covers(ranges, c) {
            continue;
        }
        let mut db = Database::new();
        for file in &files {
            let _ = db.load_font_file(file);
        }
        return Answer::Found {
            ch: req.ch,
            family,
            faces: db.faces().cloned().collect(),
        };
    }
    Answer::None { ch: req.ch }
}

fn covers(ranges: &[(u32, u32)], c: u32) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if c < lo {
                std::cmp::Ordering::Greater
            } else if c > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Codepoint ranges the font files cover, from their cmaps.
fn read_ranges(files: &[std::path::PathBuf]) -> Vec<(u32, u32)> {
    let mut points: Vec<u32> = Vec::new();
    for file in files {
        // sync-ok: on the fallback worker thread, never on the UI thread
        let Ok(data) = std::fs::read(file) else {
            continue;
        };
        let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
        for index in 0..count {
            let Ok(face) = ttf_parser::Face::parse(&data, index) else {
                continue;
            };
            let Some(cmap) = face.tables().cmap else {
                continue;
            };
            for sub in cmap.subtables {
                if sub.is_unicode() {
                    sub.codepoints(|c| points.push(c));
                }
            }
        }
    }
    points.sort_unstable();
    points.dedup();
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for c in points {
        match ranges.last_mut() {
            Some(last) if last.1 + 1 == c => last.1 = c,
            _ => ranges.push((c, c)),
        }
    }
    ranges
}
