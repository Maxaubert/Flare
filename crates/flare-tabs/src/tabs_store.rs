//! TabsStore: tabs.json over the Debounced file of flare-settings (Task 13).
//!
//! Source: PT/src/main/tabsStore.ts. Nothing is written before the restore answered (rule 37):
//! that is the caller's rule (`TabModel::saved()`), this type only stores what it is handed.

use crate::restore::{SavedTabs, parse_tabs, tabs_json};
use flare_settings::debounce::Debounced;
use std::path::PathBuf;
use std::time::Duration;

/// `tabs.json`, saved on a delay so a burst of cwd reports writes once.
pub struct TabsStore {
    file: PathBuf,
    out: Debounced,
}

impl TabsStore {
    /// 400 ms in the app.
    pub fn new(file: impl Into<PathBuf>, delay: Duration) -> Self {
        let file = file.into();
        Self {
            out: Debounced::new(file.clone(), delay),
            file,
        }
    }

    /// A save still inside its debounce IS the newest state; else the file; unreadable or missing: empty.
    pub fn load(&self) -> SavedTabs {
        if let Some(p) = self.out.pending() {
            return parse_tabs(&p);
        }
        match std::fs::read_to_string(&self.file) {
            // sync-ok: startup read
            Ok(raw) => parse_tabs(&raw),
            Err(_) => SavedTabs::default(),
        }
    }

    pub fn save(&self, s: &SavedTabs) {
        self.out.save(tabs_json(s));
    }

    /// Close, quit and session end: a pending save is written now.
    pub fn flush(&self) {
        self.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::restore::SavedTab;
    use flare_agent::kinds::AgentKind;
    use std::time::Instant;

    fn one(cwd: &str) -> SavedTab {
        SavedTab {
            cwd: cwd.into(),
            agent: None,
        }
    }

    #[test]
    fn a_store_loads_an_empty_strip_without_a_file() {
        let d = tempfile::tempdir().unwrap();
        let s = TabsStore::new(d.path().join("tabs.json"), Duration::from_secs(60));
        assert_eq!(s.load(), SavedTabs::default());
    }

    #[test]
    fn store_flush_writes_at_once() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("tabs.json");
        let s = TabsStore::new(&f, Duration::from_secs(60));
        s.save(&SavedTabs {
            tabs: vec![one("C:\\a")],
            active: 0,
        });
        s.flush();
        let raw = std::fs::read_to_string(&f).unwrap(); // sync-ok: test code only, never the app
        assert_eq!(parse_tabs(&raw).tabs, vec![one("C:\\a")]);
        assert_eq!(s.load().tabs.len(), 1);
    }

    #[test]
    fn store_writes_the_last_save_after_the_delay() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("tabs.json");
        let s = TabsStore::new(&f, Duration::from_millis(20));
        s.save(&SavedTabs {
            tabs: vec![one("C:\\a")],
            active: 0,
        });
        let want = SavedTabs {
            tabs: vec![
                one("C:\\a"),
                SavedTab {
                    cwd: "C:\\b".into(),
                    agent: Some(AgentKind::Codex),
                },
            ],
            active: 1,
        };
        s.save(&want);
        assert!(!f.exists());
        let t = Instant::now();
        while !f.exists() && t.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let raw = std::fs::read_to_string(&f).unwrap(); // sync-ok: test code only, never the app
        assert_eq!(parse_tabs(&raw), want);
    }

    #[test]
    fn store_loads_the_pending_save() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("tabs.json");
        std::fs::write(&f, r#"{"tabs":[{"cwd":"C:\\old"}],"active":0}"#).unwrap(); // sync-ok: test code only, never the app
        let s = TabsStore::new(&f, Duration::from_secs(60));
        s.save(&SavedTabs {
            tabs: vec![one("C:\\new")],
            active: 0,
        });
        assert_eq!(s.load().tabs, vec![one("C:\\new")]);
        s.flush();
    }

    #[test]
    fn store_flush_with_nothing_pending_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("tabs.json");
        let s = TabsStore::new(&f, Duration::from_secs(60));
        s.flush();
        assert!(!f.exists());
    }
}
