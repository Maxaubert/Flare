//! TabsStore: tabs.json over the Debounced file of flare-settings (Task 13).
//!
//! Source: PT/src/main/tabsStore.ts. Nothing is written before the restore answered (rule 37):
//! that is the caller's rule (`TabModel::saved()`), this type only stores what it is handed.

use crate::restore::{SavedTabs, parse_tabs, tabs_json};
use flare_settings::atomic::{Opened, open_text};
use flare_settings::debounce::Debounced;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// `tabs.json`, saved on a delay so a burst of cwd reports writes once.
pub struct TabsStore {
    file: PathBuf,
    out: Debounced,
    /// The file exists but could not be read, and could not be set aside: nothing is saved this session,
    /// so the real tabs.json is never replaced by an empty strip (audit 2026-10-20, M3).
    read_only: AtomicBool,
}

impl TabsStore {
    /// 400 ms in the app.
    pub fn new(file: impl Into<PathBuf>, delay: Duration) -> Self {
        let file = file.into();
        Self {
            out: Debounced::new(file.clone(), delay),
            file,
            read_only: AtomicBool::new(false),
        }
    }

    /// True when tabs.json could not be read or set aside: saves are dropped for the session.
    pub fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::SeqCst)
    }

    /// A save still inside its debounce IS the newest state; else the file; unreadable or missing: empty.
    pub fn load(&self) -> SavedTabs {
        if let Some(p) = self.out.pending() {
            return parse_tabs(&p);
        }
        // Only a missing file is an empty strip. A file that cannot be read is set aside as `.bad`;
        // when even that fails the store stops saving, so the real file survives.
        match open_text(&self.file) {
            // sync-ok: startup read
            Opened::Text(raw) => parse_tabs(&raw),
            Opened::Missing => SavedTabs::default(),
            Opened::Unreadable { moved_aside } => {
                if !moved_aside {
                    self.read_only.store(true, Ordering::SeqCst);
                }
                SavedTabs::default()
            }
        }
    }

    pub fn save(&self, s: &SavedTabs) {
        if self.is_read_only() {
            return;
        }
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
            remote: None,
        }
    }

    #[test]
    fn a_store_loads_an_empty_strip_without_a_file() {
        let d = tempfile::tempdir().unwrap();
        let s = TabsStore::new(d.path().join("tabs.json"), Duration::from_secs(60));
        assert_eq!(s.load(), SavedTabs::default());
    }

    // Windows-only: the lock is a share_mode(0) handle; Unix has no mandatory locks.
    #[cfg(windows)]
    #[test]
    fn an_unreadable_tabs_file_is_set_aside_and_a_locked_one_is_never_overwritten() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("tabs.json");
        std::fs::write(&f, [0xff, 0xfe]).unwrap();
        let s = TabsStore::new(&f, Duration::from_secs(60));
        assert_eq!(s.load(), SavedTabs::default());
        assert!(!s.is_read_only());
        assert_eq!(
            std::fs::read(d.path().join("tabs.json.bad")).unwrap(),
            vec![0xff, 0xfe]
        );

        let g = d.path().join("locked.json");
        std::fs::write(&g, r#"{"tabs":[{"cwd":"C:\\keep"}],"active":0}"#).unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&g)
            .unwrap();
        let s = TabsStore::new(&g, Duration::from_secs(60));
        assert_eq!(s.load(), SavedTabs::default());
        assert!(s.is_read_only());
        s.save(&SavedTabs {
            tabs: vec![one("C:\\lost")],
            active: 0,
        });
        s.flush();
        drop(held);
        assert_eq!(
            parse_tabs(&std::fs::read_to_string(&g).unwrap()).tabs,
            vec![one("C:\\keep")]
        );
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
                    remote: None,
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
