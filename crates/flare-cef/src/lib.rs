//! The macOS web engine: CEF (the Chromium Embedded Framework), windowless and headless. The phase 1 verdict
//! (`docs/superpowers/specs/2026-11-08-macos-web-verdict.md`, on the phase 1 branch `feat/macos-1-web-spike`; plan in
//! `docs/superpowers/specs/2026-11-08-macos-port.md`) ruled WKWebView out: it cannot stop a runaway script. CEF can,
//! through the same DevTools Protocol WebView2 speaks on Windows, so `flare_surfaces::agent_job` runs unchanged
//! (regression rules 1020 to 1022).
//!
//! macOS only (rule 227's mirror image): on Windows WebView2 stays the engine, this crate is empty and its binaries
//! only say so.
//!
//! - [`pin`]: which CEF build, and where it is cached.
//! - [`status`]: Chromium's numbers in WebView2's words (error statuses, zoom levels, CSS pixels), portable.
//! - `macos::app`: the `NSApplication` subclass CEF needs (`CefAppProtocol`): headless for a probe (activation
//!   policy Prohibited), the real app's shared application before winit.
//! - `macos::engine`: loading the framework, `cef_initialize` with windowless rendering, the message pump.
//! - `macos::view`: one windowless browser, its pixels (`on_paint` into a buffer), its DevTools channel
//!   (`SendDevToolsMessage`), its walls (navigation, requests, new windows, downloads), its input, and the handlers
//!   that keep every native popup off the screen.
//! - `macos::context`: the profiles (request contexts) and the walls a whole profile holds (proxy, WebRTC).
//! - `macos::helper`: the helper processes' `main`.
//! - `macos::bundle`: the dev `.app` (the framework in `Contents/Frameworks`, five helper apps, an ad-hoc
//!   signature) built under the target dir.
//! - `macos::screen`: the window server's list, the screen rule's evidence.
//! - `macos::probe`: the scenarios the engine tests run in a bundled browser process.

pub mod pin;
pub mod status;

#[cfg(target_os = "macos")]
pub mod macos;
