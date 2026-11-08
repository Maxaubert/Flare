//! The CEF build Flare is pinned to on macOS, and where its binary is cached. Portable text, so the pin can be read
//! (and checked against `Cargo.toml`) on any machine.

/// The `cef` and `cef-dll-sys` crate version (workspace `Cargo.toml`, both pinned with `=` to the part before `+`:
/// Cargo ignores build metadata in a requirement, and `Cargo.lock` holds the full version).
pub const CRATE_VERSION: &str = "156.2.0+156.0.14";
/// The CEF build those crates were generated for (the part after `+`), the folder name under the cache.
pub const CEF_VERSION: &str = "156.0.14";
/// The Chromium inside that CEF build (the archive's name: `cef_binary_156.0.14+g4fa4ab5+chromium-156.0.8078.48`).
pub const CHROMIUM_VERSION: &str = "156.0.8078.48";

/// Where the CEF binary goes (`CEF_PATH`): a cache outside the repo, under the user's Library. cef-dll-sys's build
/// script downloads the archive into `<this>/<CEF_VERSION>/cef_macos_aarch64` when it is missing; without `CEF_PATH`
/// it falls back to its own `OUT_DIR` in the target dir. Never the repo.
pub const CACHE_DIR_UNDER_HOME: &str = "Library/Caches/flare-cef";

/// The name of the macOS framework inside the CEF folder and the bundle's `Contents/Frameworks`.
pub const FRAMEWORK: &str = "Chromium Embedded Framework.framework";

/// The five helper apps CEF starts its child processes from, as suffixes of the app's name (`Flare Helper (GPU)`).
/// The order is the one `codesign` must follow: helpers before the app that holds them.
pub const HELPER_SUFFIXES: [&str; 5] = [
    "Helper",
    "Helper (GPU)",
    "Helper (Renderer)",
    "Helper (Plugin)",
    "Helper (Alerts)",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pin_names_one_build_and_the_workspace_pins_both_crates_to_it() {
        assert_eq!(
            CRATE_VERSION.split_once('+').map(|(_, b)| b),
            Some(CEF_VERSION)
        );
        let ws = include_str!("../../../Cargo.toml");
        let lock = include_str!("../../../Cargo.lock");
        let semver = CRATE_VERSION
            .split_once('+')
            .map_or(CRATE_VERSION, |(v, _)| v);
        for krate in ["cef", "cef-dll-sys"] {
            let line = ws
                .lines()
                .find(|l| l.starts_with(&format!("{krate} = ")))
                .unwrap_or_else(|| panic!("{krate} is in the workspace dependencies"));
            assert!(
                line.contains(&format!("version = \"={semver}\"")),
                "{krate} is pinned exactly to {semver}: {line}"
            );
            assert!(
                lock.contains(&format!(
                    "name = \"{krate}\"\nversion = \"{CRATE_VERSION}\""
                )),
                "Cargo.lock holds {krate} {CRATE_VERSION}"
            );
            assert!(line.contains("default-features = false"), "{line}");
        }
    }

    #[test]
    fn the_cache_is_outside_the_repo_and_the_helpers_are_the_five_cef_looks_for() {
        assert!(!CACHE_DIR_UNDER_HOME.starts_with('.'));
        assert!(CACHE_DIR_UNDER_HOME.starts_with("Library/Caches/"));
        assert_eq!(HELPER_SUFFIXES.len(), 5);
        assert!(HELPER_SUFFIXES.iter().all(|h| h.starts_with("Helper")));
        assert!(HELPER_SUFFIXES.contains(&"Helper (Renderer)"));
        assert!(HELPER_SUFFIXES.contains(&"Helper (GPU)"));
        assert!(HELPER_SUFFIXES.contains(&"Helper (Plugin)"));
    }
}
