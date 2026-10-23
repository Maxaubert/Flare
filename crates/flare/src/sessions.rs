//! ClaudeSessionLookup: the Claude session lookup of the restore (M6, Task 30).
//!
//! The restore's worker asks it for the session ids of a folder, newest first. It keeps ONLY ids
//! that pass `valid_resume`, BEFORE the plan hands them out by position, so a transcript named
//! `notes & calc.jsonl` is skipped and never takes a tab's slot. This is the first of the three
//! places a resume id is validated (the lookup, `TabHost::spawn`, `SessionManager::spawn`).

use flare_agent::resume::{claude_sessions_in, store_root, valid_resume};
use flare_tabs::restore::SessionLookup;
use std::path::PathBuf;

/// The reader of Claude's transcript store under a home folder.
pub struct ClaudeSessionLookup {
    pub home: Option<PathBuf>,
    /// `CLAUDE_CONFIG_DIR`: where Claude keeps its store when the user moved it.
    pub config_dir: Option<std::ffi::OsString>,
}

impl ClaudeSessionLookup {
    pub fn new(home: Option<PathBuf>) -> Self {
        Self {
            home,
            config_dir: None,
        }
    }

    /// The same lookup, reading the store Claude was told to use.
    pub fn with_config_dir(mut self, config_dir: Option<std::ffi::OsString>) -> Self {
        self.config_dir = config_dir;
        self
    }
}

impl SessionLookup for ClaudeSessionLookup {
    /// BLOCKING (the restore worker).
    fn sessions(&self, cwd: &str) -> Vec<String> {
        let Some(home) = &self.home else {
            return Vec::new();
        };
        claude_sessions_in(&store_root(home, self.config_dir.as_deref()), cwd)
            .into_iter()
            .filter(|id| valid_resume(Some(id)).is_some())
            .collect()
    }
}

/// A lookup that answers with a fixed list for every folder and counts the questions.
#[cfg(test)]
pub struct FixedSessions {
    pub ids: Vec<String>,
    pub asked: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl FixedSessions {
    pub fn new(ids: &[&str]) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            ids: ids.iter().map(|s| (*s).to_string()).collect(),
            asked: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
impl SessionLookup for FixedSessions {
    fn sessions(&self, cwd: &str) -> Vec<String> {
        if let Ok(mut g) = self.asked.lock() {
            g.push(cwd.to_string());
        }
        self.ids.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const CLI_HEAD: &str = "{\"type\":\"last-prompt\",\"lastPrompt\":\"hello\"}\n{\"parentUuid\":null,\"isSidechain\":false,\"userType\":\"external\",\"entrypoint\":\"cli\",\"type\":\"user\"}\n";
    const GOOD: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";

    #[test]
    fn the_claude_lookup_skips_ids_that_are_not_session_ids() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join(".claude").join("projects").join("C--w");
        fs::create_dir_all(&dir).expect("dir");
        let good = dir.join(format!("{GOOD}.jsonl"));
        fs::write(&good, CLI_HEAD).expect("good");
        let bad = dir.join("a & calc.jsonl");
        fs::write(&bad, CLI_HEAD).expect("bad");
        // The bad one is the newest.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(&bad)
            .expect("open")
            .set_modified(later)
            .expect("mtime");
        let lookup = ClaudeSessionLookup::new(Some(tmp.path().to_path_buf()));
        assert_eq!(lookup.sessions("C:\\w"), vec![GOOD.to_string()]);
        // A moved store (CLAUDE_CONFIG_DIR) is the one read.
        let cfg = tmp.path().join("moved");
        let moved = cfg.join("projects").join("C--w");
        fs::create_dir_all(&moved).expect("moved");
        fs::write(
            moved.join("0f0e0d0c-2222-4333-8444-955566667777.jsonl"),
            CLI_HEAD,
        )
        .expect("m");
        let lookup = ClaudeSessionLookup::new(Some(tmp.path().to_path_buf()))
            .with_config_dir(Some(cfg.into_os_string()));
        assert_eq!(
            lookup.sessions("C:\\w"),
            vec!["0f0e0d0c-2222-4333-8444-955566667777".to_string()]
        );
        let none = ClaudeSessionLookup::new(None);
        assert!(none.sessions("C:\\w").is_empty());
    }
}
