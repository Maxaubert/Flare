//! The path grammar seam: how this platform writes a path (rule 227). The shared logic (what a
//! click may open, the stat meter, the link cache) lives in `path_open`, `paths` and `path_links`;
//! only the grammar differs per OS:
//!
//! - Windows (`path_grammar/windows.rs`): drive letters, `\` and `/` both separators, UNC refused,
//!   case-insensitive folders.
//! - macOS (`macos/path_grammar.rs`): one root `/`, `~` for home, `/` the only separator (a `\` is a
//!   name character), backslash-escaped spaces and `file://` URLs in terminal text, case kept.
//!
//! Each grammar has the same functions; the shared code calls them through this module only.

#[cfg(windows)]
#[path = "path_grammar/windows.rs"]
mod native;

#[cfg(not(windows))]
#[path = "macos/path_grammar.rs"]
mod native;

pub use native::{
    is_cwd, is_relative, parent_dir, resolve_term_path, special_candidates, volume_key,
};

/// One path the grammar reads out of terminal text before the word pass (macOS: an escaped or
/// `file://` path). Char indices, end exclusive; `path` is what the text names, decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Special {
    pub start: usize,
    pub end: usize,
    pub path: String,
    /// The whole word it was read from, blanked for the word pass.
    pub mask: (usize, usize),
}
