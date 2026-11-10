//! App facts (version, home folder): read once, asked of this one place (docs/parity.md line 55).

use std::sync::OnceLock;

/// The running build's version, fixed at compile time.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The user's home folder, read from the environment on first use and cached for the run:
/// `USERPROFILE` on Windows, `HOME` on macOS. `.` when the environment has none.
pub fn home() -> &'static str {
    static HOME: OnceLock<String> = OnceLock::new();
    HOME.get_or_init(|| std::env::var(HOME_VAR).unwrap_or_else(|_| ".".to_string()))
}

/// The variable that names the home folder on this platform.
const HOME_VAR: &str = if cfg!(windows) { "USERPROFILE" } else { "HOME" };

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn app_facts_are_read_once() {
        // One cached read: the same allocation every time.
        assert_eq!(home().as_ptr(), home().as_ptr());
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));

        // No other file reads them itself.
        let needles = [
            ["env!(\"CARGO_PKG", "_VERSION\")"].concat(),
            ["\"USER", "PROFILE\""].concat(),
        ];
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files: Vec<_> = std::fs::read_dir(&src)
            .unwrap()
            .chain(std::fs::read_dir(src.join("e2e_probes")).unwrap())
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            .filter(|p| p.file_name().is_some_and(|n| n != "app_facts.rs"))
            .collect();
        files.sort();
        let mut hits = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            for n in &needles {
                if text.contains(n.as_str()) {
                    hits.push(format!("{} reads {n}", f.display()));
                }
            }
        }
        assert!(hits.is_empty(), "read app facts via app_facts: {hits:#?}");
    }
}
