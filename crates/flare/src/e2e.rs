//! test flag and env seams (Task 29).
//!
//! `E2eFlag` is the ONLY way to read a `FLARE_E2E_*` variable: without `--e2e` they read as absent,
//! so a stray variable in the owner's environment can never change the real app.

#![allow(dead_code)] // wired by AppCtl (a later task); remove this line then

use std::sync::OnceLock;

pub struct E2eFlag(bool);

impl E2eFlag {
    pub fn new(on: bool) -> Self {
        Self(on)
    }

    pub fn on(&self) -> bool {
        self.0
    }

    /// The process variable `name`, None unless the e2e flag is on.
    pub fn env(&self, name: &str) -> Option<String> {
        if !self.0 {
            return None;
        }
        std::env::var(name).ok()
    }
}

static FLAG: OnceLock<E2eFlag> = OnceLock::new();

/// Sets the process-wide flag once (main, after parsing the arguments); later calls are ignored.
pub fn init(on: bool) {
    let _ = FLAG.set(E2eFlag::new(on));
}

/// The flag; off when `init` never ran.
pub fn global() -> &'static E2eFlag {
    FLAG.get_or_init(|| E2eFlag::new(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2e_env_reads_none_unless_the_flag_is_on() {
        let name = ["FLARE_E2E", "_SEAM_TEST"].concat();
        // SAFETY: a variable only this test reads; no other thread touches it.
        unsafe { std::env::set_var(&name, "yes") };
        assert_eq!(E2eFlag::new(false).env(&name), None);
        assert_eq!(E2eFlag::new(true).env(&name).as_deref(), Some("yes"));
        assert!(!E2eFlag::new(false).on());
        assert!(E2eFlag::new(true).on());
    }

    #[test]
    fn no_source_file_reads_an_e2e_variable_directly() {
        let needle = ["\"FLARE_E2E", "_"].concat();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in std::fs::read_dir(&dir).unwrap() {
            // sync-ok: test
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("e2e.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap(); // sync-ok: test
            // Only the code before the test module counts.
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            assert!(
                !code.contains(&needle),
                "{} reads an e2e variable directly",
                path.display()
            );
        }
    }
}
