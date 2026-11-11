//! The path grammar seam: how this platform writes a path (rule 227). The shared logic (what a
//! click may open, the stat meter) lives in `path_open`; only the grammar differs per OS:
//!
//! - Windows (`path_grammar/windows.rs`): drive letters, `\` and `/` both separators, UNC refused.
//! - macOS (`macos/path_grammar.rs`): one root `/`, `~` for home, `/` the only separator (a `\` is a
//!   name character), automount points refused.
//!
//! Each grammar has the same functions; the shared code calls them through this module only. No
//! grammar reads paths out of terminal text any more: a file link is only what a session writes
//! between the marks (`file_marks`, 2026-11-11).

#[cfg(windows)]
#[path = "path_grammar/windows.rs"]
mod native;

#[cfg(not(windows))]
#[path = "macos/path_grammar.rs"]
mod native;

pub use native::{is_relative, resolve_term_path, volume_key};
