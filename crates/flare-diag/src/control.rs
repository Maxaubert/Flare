//! The diagnostics control: the verbose flag file, the folder path and the mark (M7 Task 29).
//!
//! `diag.json` in the profile folder holds `{"verbose": true}` and nothing else. A missing, corrupt
//! or non-bool file reads as off. Reading and constructing never write: an idle control leaves the
//! profile folder as it found it. The mark goes to the sink once and writes no file of its own.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use flare_settings::atomic::write_atomic;

use crate::agent_diag::{DiagSink, DiagValue};

const FLAG_FILE: &str = "diag.json";

/// The verbose flag, the log folder and the mark. `sink` is M6's diagnostics sink (`None` in the
/// app until M9 owns the writer).
pub struct DiagControl {
    profile: PathBuf,
    sink: Option<Arc<dyn DiagSink>>,
}

impl DiagControl {
    pub fn new(profile: &Path, sink: Option<Arc<dyn DiagSink>>) -> Self {
        Self {
            profile: profile.to_path_buf(),
            sink,
        }
    }

    /// The log folder: `<profile>/logs`. Never created here.
    pub fn dir(&self) -> PathBuf {
        self.profile.join("logs")
    }

    /// Reads `diag.json`; a missing, corrupt or non-bool file is off.
    pub fn verbose(&self) -> bool {
        let Ok(text) = std::fs::read_to_string(self.profile.join(FLAG_FILE)) else {
            return false; // sync-ok: one small flag file, read when the page opens
        };
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("verbose").and_then(serde_json::Value::as_bool))
            .unwrap_or(false)
    }

    /// Writes the flag atomically, then re-reads it and returns what is stored, so the page shows
    /// the truth even when the write was refused.
    pub fn set_verbose(&self, on: bool) -> bool {
        let body = serde_json::json!({ "verbose": on }).to_string();
        let _ = write_atomic(&self.profile.join(FLAG_FILE), &body); // sync-ok: a user click on the diagnostics page, one small file
        self.verbose()
    }

    /// Forwards one `mark` record to the sink. No sink: nothing happens.
    pub fn mark(&self) {
        if let Some(sink) = &self.sink {
            let at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
                .unwrap_or(0);
            let fields: [(&str, DiagValue); 0] = [];
            sink.record("mark", &fields, at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_diag::{DiagSink, DiagValue};
    use std::sync::Mutex;

    /// A scratch folder under the system temp folder, removed on drop (flare-diag has no tempfile).
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir()
                .join(format!("flare-diag-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("scratch folder"); // sync-ok: test code
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0); // sync-ok: test code
        }
    }

    #[derive(Default)]
    struct Recording(Mutex<Vec<(String, usize, u64)>>);

    impl DiagSink for Recording {
        fn record(&self, kind: &str, fields: &[(&str, DiagValue)], at_ms: u64) {
            let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
            v.push((kind.to_string(), fields.len(), at_ms));
        }
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir) // sync-ok: test code
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn verbose_is_off_until_set() {
        let s = Scratch::new("off");
        let c = DiagControl::new(&s.0, None);
        assert!(!c.verbose());
    }

    #[test]
    fn verbose_survives_a_new_instance() {
        let s = Scratch::new("survive");
        assert!(DiagControl::new(&s.0, None).set_verbose(true));
        assert!(DiagControl::new(&s.0, None).verbose());
    }

    #[test]
    fn a_corrupt_diag_json_reads_false() {
        let s = Scratch::new("corrupt");
        for body in ["{nope", r#"{"verbose":"yes"}"#] {
            std::fs::write(s.0.join("diag.json"), body).expect("write"); // sync-ok: test code
            assert!(!DiagControl::new(&s.0, None).verbose(), "{body}");
        }
    }

    #[test]
    fn set_returns_what_is_stored_not_what_was_asked() {
        let s = Scratch::new("refused");
        let blocker = s.0.join("not-a-folder");
        std::fs::write(&blocker, "x").expect("write"); // sync-ok: test code
        let c = DiagControl::new(&blocker, None);
        assert!(!c.set_verbose(true));
        assert!(!c.verbose());
    }

    #[test]
    fn the_write_is_atomic() {
        let s = Scratch::new("atomic");
        assert!(DiagControl::new(&s.0, None).set_verbose(true));
        assert_eq!(entries(&s.0), vec!["diag.json".to_string()]);
    }

    #[test]
    fn mark_forwards_one_record_and_writes_no_file() {
        let s = Scratch::new("mark");
        let sink = Arc::new(Recording::default());
        let before = entries(&s.0);
        let c = DiagControl::new(&s.0, Some(sink.clone()));
        c.mark();
        let got = sink.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "mark");
        assert_eq!(got[0].1, 0);
        assert_eq!(entries(&s.0), before);
    }

    #[test]
    fn an_idle_control_writes_nothing() {
        let s = Scratch::new("idle");
        let c = DiagControl::new(&s.0, None);
        let _ = c.verbose();
        let _ = c.dir();
        assert!(entries(&s.0).is_empty());
    }

    #[test]
    fn no_sink_no_panic() {
        let s = Scratch::new("nosink");
        DiagControl::new(&s.0, None).mark();
    }
}
