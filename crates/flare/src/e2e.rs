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

    /// A dialog's seam: None without --e2e (the real dialog is the user's). Under --e2e, the value
    /// of `name` when `usable` takes it, else `cancel`: an e2e run never shows a real dialog or pick
    /// window, whatever a scenario forgot to set.
    pub fn dialog_seam(
        &self,
        name: &str,
        cancel: &str,
        usable: impl Fn(&str) -> bool,
    ) -> Option<String> {
        if !self.0 {
            return None;
        }
        Some(
            self.env(name)
                .filter(|v| usable(v))
                .unwrap_or_else(|| cancel.to_string()),
        )
    }
}

static FLAG: OnceLock<E2eFlag> = OnceLock::new();

/// Sets the process-wide flag once (main, after parsing the arguments); later calls are ignored.
pub fn init(on: bool) {
    let _ = FLAG.set(E2eFlag::new(on));
    // The seams below the app crate (dictation folders, NVIDIA answer, plugin folder) share the gate.
    flare_win::seam::set_open(on);
}

/// The flag; off when `init` never ran.
pub fn global() -> &'static E2eFlag {
    FLAG.get_or_init(|| E2eFlag::new(false))
}

/// Whether the window's animations run. Without --e2e the OS setting decides (Windows' "Animation
/// effects", a Mac's "Reduce motion"). Under --e2e the OS is not read: the window animates unless
/// FLARE_E2E_REDUCED_MOTION=1 asks for still, so a runner whose OS reduces motion (GitHub's macOS
/// images do) sees the same motion as any other (rule 1153).
pub fn animations_enabled() -> bool {
    let flag = global();
    animations_from(
        flag.on(),
        flag.env(&["FLARE_E2E", "_REDUCED_MOTION"].concat())
            .as_deref(),
        flare_win::motion::animations_enabled,
    )
}

/// Pure rule of [`animations_enabled`]: under --e2e only the seam decides (still when it is "1"),
/// otherwise `os` is asked.
fn animations_from(e2e: bool, seam: Option<&str>, os: impl FnOnce() -> bool) -> bool {
    if e2e {
        return seam != Some("1");
    }
    os()
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
    fn under_e2e_motion_follows_the_seam_and_never_the_os() {
        let os_still = || false;
        let os_moves = || true;
        // Without --e2e the OS decides, whatever the seam says.
        assert!(!animations_from(false, None, os_still));
        assert!(animations_from(false, None, os_moves));
        assert!(animations_from(false, Some("1"), os_moves));
        // Under --e2e a Reduce motion on the runner is not read: the window animates.
        assert!(animations_from(true, None, os_still));
        assert!(animations_from(true, Some("0"), os_still));
        // And a scenario that asks for still gets it.
        assert!(!animations_from(true, Some("1"), os_moves));
        // The OS is never even asked under --e2e.
        assert!(animations_from(true, None, || unreachable!(
            "the OS is read under --e2e"
        )));
    }

    #[test]
    fn under_e2e_a_dialog_without_its_seam_is_cancelled_never_shown() {
        let name = ["FLARE_E2E", "_DIALOG_SEAM_TEST"].concat();
        let any = |_: &str| true;
        // SAFETY: a variable only this test reads; no other thread touches it.
        unsafe { std::env::remove_var(&name) };
        // Without --e2e there is no seam: the real dialog is the user's.
        assert_eq!(E2eFlag::new(false).dialog_seam(&name, "cancel", any), None);
        // Under --e2e an unset seam is a cancel, so no real dialog or pick window ever shows.
        assert_eq!(
            E2eFlag::new(true)
                .dialog_seam(&name, "cancel", any)
                .as_deref(),
            Some("cancel")
        );
        // SAFETY: as above.
        unsafe { std::env::set_var(&name, "#00ff00") };
        assert_eq!(
            E2eFlag::new(true).dialog_seam(&name, "", any).as_deref(),
            Some("#00ff00")
        );
        // A value the seam cannot use is a cancel too, never the real thing.
        assert_eq!(
            E2eFlag::new(true)
                .dialog_seam(&name, "cancel", |v| v == "ok")
                .as_deref(),
            Some("cancel")
        );
        assert_eq!(E2eFlag::new(false).dialog_seam(&name, "", any), None);
    }

    #[test]
    fn no_source_file_reads_an_e2e_variable_directly() {
        let needle = ["\"FLARE_E2E", "_"].concat();
        // Every folder under src, the probes and the UI tests included.
        let mut dirs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut files = Vec::new();
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                // sync-ok: test code only, never the app
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        assert!(files.iter().any(|p| p.ends_with("e2e_probes/core.rs")));
        for path in files {
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("e2e.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap(); // sync-ok: test code only, never the app
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
