//! The web panel's browser (Surfaces stage 3): WebView2 (Windows) or CEF (macOS) behind the [`WebBackend`] seam.
//!
//! `host` is the real thing: WebView2 on Windows (`host.rs`, COM), CEF on macOS (`macos/host.rs`, windowless, port
//! phase 5); `fake` is a stand-in with no browser for the panel logic's tests; `backend` is the trait and the events
//! between them and the window.

pub mod backend;
/// The web host tests' browser process (macOS: run inside a dev bundle by `tests/cef_macos.rs`).
#[cfg(target_os = "macos")]
#[path = "macos/probe/mod.rs"]
pub mod cef_probe;
pub mod fake;
#[cfg(windows)]
pub mod host;
#[cfg(target_os = "macos")]
#[path = "macos/host.rs"]
pub mod host;
#[cfg(not(any(windows, target_os = "macos")))]
#[path = "stub_host.rs"]
pub mod host;
pub mod seal;
pub mod walls;

pub use backend::*;
