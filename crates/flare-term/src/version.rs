//! The version strings the replies carry (XTVERSION names Flare and the emulator under it).

/// Flare's own version, from the workspace.
pub const FLARE_TERM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The `alacritty_terminal` this build links (Cargo.lock pins it; a test checks).
pub const ALACRITTY_TERMINAL_VERSION: &str = "0.26.0";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn read(rel: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn flare_term_version_matches_the_workspace_version() {
        let manifest = read("../../Cargo.toml");
        let mut in_package = false;
        let mut found = None;
        for line in manifest.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                in_package = t == "[workspace.package]";
                continue;
            }
            if let (true, None, Some(v)) = (in_package, &found, t.strip_prefix("version = \"")) {
                found = v.strip_suffix('"').map(str::to_string);
            }
        }
        assert_eq!(found.as_deref(), Some(FLARE_TERM_VERSION));
    }

    #[test]
    fn alacritty_version_matches_cargo_lock() {
        let lock = read("../../Cargo.lock");
        let mut lines = lock.lines();
        let mut found = None;
        while let Some(line) = lines.next() {
            if line == "name = \"alacritty_terminal\"" {
                let next = lines.next().unwrap_or_default();
                found = next
                    .strip_prefix("version = \"")
                    .and_then(|v| v.strip_suffix('"'))
                    .map(str::to_string);
                break;
            }
        }
        assert_eq!(found.as_deref(), Some(ALACRITTY_TERMINAL_VERSION));
    }
}
