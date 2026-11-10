//! The web panel's browser (Surfaces stage 3): WebView2 behind the [`WebBackend`] seam.
//!
//! `host` is the real thing (COM, Windows only; `macos/host.rs` stands in elsewhere until macOS phase 5); `fake` is a stand-in with no browser for the
//! panel logic's tests; `backend` is the trait and the events between them and the window.

pub mod backend;
pub mod fake;
#[cfg(windows)]
pub mod host;
#[cfg(not(windows))]
#[path = "macos/host.rs"]
pub mod host;
// The Windows host keeps its pop-ups here; the Mac stand-in has none until phase 5 (its tests run everywhere).
#[cfg_attr(not(windows), allow(dead_code))]
mod popups;
pub mod seal;

pub use backend::*;
