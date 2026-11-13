//! The path openers: `WinOpeners` in production, `RecordingOpeners` under `--e2e` and in tests.
#![allow(dead_code)] // wired by TermUi (Task 23) and the window (Task 24); remove this line then

use std::sync::Mutex;

use flare_term::path_open::PathOpeners;

/// Production openers: they call Windows through `flare_win::openers`. Errors are logged without
/// the path, and each one is an `open-failed` crumb in the diagnostics log (owner, 2026-11-13).
pub struct WinOpeners;

/// One `open-failed` crumb: which opener, the file's extension as a fixed word, and the opener's own
/// error (fixed text such as `ShellExecuteW failed with 2`; never the path).
fn failed_crumb(via: &str, abs: &str, err: &str) {
    let err: String = err.chars().take(80).collect();
    crate::diag_ctl::crumb(
        "open-failed",
        &[
            ("via", crate::diag_ctl::txt(via)),
            (
                "ext",
                crate::diag_ctl::txt(flare_term::path_open::ext_label(abs)),
            ),
            ("err", crate::diag_ctl::txt(&err)),
        ],
    );
}

impl PathOpeners for WinOpeners {
    fn open_path(&self, abs: &str) {
        if let Err(e) = flare_win::openers::shell_open_file(abs, &flare_term::path_open::may_open) {
            log::warn!("open failed: {e}");
            failed_crumb("file", abs, &e);
        }
    }

    fn open_folder(&self, abs: &str) -> bool {
        match flare_win::openers::explorer_open_folder(abs) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("folder open failed: {e}");
                failed_crumb("folder", abs, &e);
                false
            }
        }
    }

    fn reveal_path(&self, abs: &str) {
        if let Err(e) = flare_win::openers::explorer_reveal(abs) {
            log::warn!("reveal failed: {e}");
            failed_crumb("reveal", abs, &e);
        }
    }
}

/// Every call any recorder took, in order: the test hook's `open.log` probe reads it (the app has
/// one recorder under `--e2e`).
static ALL: Mutex<Vec<OpenedRecord>> = Mutex::new(Vec::new());

/// Every call the recorders took so far.
pub fn recorded() -> Vec<OpenedRecord> {
    ALL.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Records an open the window itself took under `--e2e` (a folder or a file), so the `open.log`
/// probe sees it like any recorder's call.
pub fn record_window_open(path: &std::path::Path) {
    let text = path.to_string_lossy().into_owned();
    let rec = if path.is_dir() {
        OpenedRecord::Folder(text)
    } else {
        OpenedRecord::Open(text)
    };
    ALL.lock().unwrap_or_else(|e| e.into_inner()).push(rec);
}

/// One call recorded by `RecordingOpeners`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenedRecord {
    Open(String),
    Reveal(String),
    Folder(String),
}

/// Records calls instead of acting: under `--e2e` and in every test.
#[derive(Default)]
pub struct RecordingOpeners {
    calls: Mutex<Vec<OpenedRecord>>,
}

impl RecordingOpeners {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, rec: OpenedRecord) {
        ALL.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(rec.clone());
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(rec);
    }

    /// The calls so far, in order.
    pub fn records(&self) -> Vec<OpenedRecord> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl PathOpeners for RecordingOpeners {
    fn open_path(&self, abs: &str) {
        self.push(OpenedRecord::Open(abs.to_string()));
    }

    fn open_folder(&self, abs: &str) -> bool {
        self.push(OpenedRecord::Folder(abs.to_string()));
        true
    }

    fn reveal_path(&self, abs: &str) {
        self.push(OpenedRecord::Reveal(abs.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_term::path_open::{OpenMode, open_term_path};

    #[test]
    fn recording_openers_record_in_order() {
        let rec = RecordingOpeners::new();
        rec.open_path("a");
        rec.reveal_path("b");
        assert!(rec.open_folder("c"));
        assert_eq!(
            rec.records(),
            vec![
                OpenedRecord::Open("a".into()),
                OpenedRecord::Reveal("b".into()),
                OpenedRecord::Folder("c".into()),
            ]
        );
    }

    #[test]
    fn a_folder_open_goes_through_open_folder_never_open_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        let parent = dir.path().to_str().unwrap().to_string();
        let rec = RecordingOpeners::new();
        let opened = open_term_path(&parent, "sub", OpenMode::Open, &rec, "");
        assert!(opened.is_some());
        let records = rec.records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert!(matches!(records[0], OpenedRecord::Folder(_)), "{records:?}");
    }
}
