//! A path an engine can open, off Windows: the C runtime takes UTF-8 paths, so every path is already one.

use std::path::{Path, PathBuf};

/// Every name is clean: there is no ANSI code page to fall out of.
pub fn ansi_clean(_w: &[u16]) -> bool {
    true
}

/// The path as it is.
pub fn engine_safe_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_left_alone() {
        let p = Path::new("/models/\u{30e2}\u{30c7}\u{30eb}/ggml-base.bin");
        assert_eq!(engine_safe_path(p), p);
    }
}
