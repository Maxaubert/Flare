//! flare-dictation: dictation for Flare, everything that is not UI (M8). The pure core (catalogue,
//! reducer, cleaning, WAV, preferences, cues, send rules, key feed, controller), the machinery (store,
//! engine, microphone, media, hook, link) and the service. Each module is filled by its own task.

pub mod ansi_path;
pub mod catalog;
pub mod clean;
pub mod controller;
pub mod cues;
pub mod engine;
pub mod fetch_ureq;
pub mod gpu;
pub mod hmac;
pub mod hotkey;
pub mod job;
pub mod keyfeed;
pub mod keyhook;
pub mod link;
pub mod media;
pub mod mic;
pub mod prefs;
pub mod privacy;
pub mod route;
pub mod service;
pub mod store;
pub mod wav;
