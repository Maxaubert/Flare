//! macOS phase 1: the WKWebView spike. A runnable proof against the agent browsing contract, not
//! production code. It runs headless: the app's activation policy is Prohibited (it can never become
//! active, has no Dock icon or menu bar), its window is never ordered in, and every event goes straight
//! to the view. Evidence (JSON report and PNG stills) goes to the folder given as the first argument.
//!
//! `cargo run -p flare-web --example wk_spike -- <out dir>`
//!
//! The verdict it feeds: `docs/superpowers/specs/2026-11-08-macos-web-verdict.md`.

// SAFETY (for every unsafe block in this example): each one is an objc2 call into AppKit, WebKit,
// CoreGraphics or Network.framework, made on the main thread (MainThreadMarker) with objects this code
// holds alive (Retained) and argument types taken from the objc2 bindings; raw pointers from completion
// handlers are null-checked before use. This is a throwaway proof, not production code, so the blocks
// are explained once here instead of one by one.
#![allow(clippy::undocumented_unsafe_blocks)]

#[cfg(target_os = "macos")]
mod adv;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod pages;
#[cfg(target_os = "macos")]
mod server;
#[cfg(target_os = "macos")]
mod shim;
#[cfg(target_os = "macos")]
mod tests;

#[cfg(target_os = "macos")]
fn main() {
    let out = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("wkspike"));
    let only = std::env::args().nth(2);
    tests::run(&out, only.as_deref());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("the WKWebView spike runs on macOS only");
}
