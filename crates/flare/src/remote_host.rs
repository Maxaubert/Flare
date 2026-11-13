//! The host side of remote sessions (PR 2): what the running Flare keeps for the remote viewers that follow its tabs.
//!
//! A viewer on the other machine runs `ssh <host> flare --serve-remote`; that serve process (`serve_remote.rs`) is a
//! bridge: it says `hello` on this Flare's pipe with the **bridge key** and relays the link's `remote.*` calls. The key
//! is made here, new at every start, and written into the profile owner-only (`remote-bridge.key`): it is the only way a
//! pipe connection gets the bridge grant (rule 284).
//!
//! A followed tab is kept here with its terminal handle and its output tail, so `remote.output` and `remote.snapshot`
//! are answered on the control channel's worker and never wait on the UI thread (rule 34). Following costs the tab one
//! copy of each batch into its 4 MiB ring and never blocks it (rule 281). A viewer that detaches, or whose link drops,
//! only stops following: the tab and its shell go on (rule 280).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use flare_term::handle::TermHandle;
use flare_term::tail::TailHandle;
use serde_json::Value;

/// The file the bridge key is kept in, in the profile folder.
pub const BRIDGE_KEY_FILE: &str = "remote-bridge.key";

/// Where the profile's bridge key is.
pub fn bridge_key_path(profile: &Path) -> PathBuf {
    profile.join(BRIDGE_KEY_FILE)
}

/// Writes the bridge key into the profile: whole or not at all (a temporary file renamed over it, rule 36), readable by
/// this user only (macOS mode 0600; on Windows the profile folder's own ACL, which is the user's). Startup only.
pub fn write_bridge_key(profile: &Path, key: &str) -> std::io::Result<()> {
    let path = bridge_key_path(profile);
    let tmp = path.with_extension("key.tmp");
    {
        let mut o = std::fs::OpenOptions::new(); // sync-ok: startup, before any window
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?; // sync-ok: startup, before any window
        std::io::Write::write_all(&mut f, key.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path) // sync-ok: startup, before any window
}

/// Reads the profile's bridge key (the serve process, before it connects). None when no Flare wrote one.
pub fn read_bridge_key(profile: &Path) -> Option<String> {
    let text = std::fs::read_to_string(bridge_key_path(profile)).ok()?; // sync-ok: the headless serve process
    let key = text.trim();
    (!key.is_empty() && key.len() <= 256).then(|| key.to_string())
}

/// Compares two secrets in a time that does not say how much of them matched.
fn same_secret(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

struct Follow {
    handle: Arc<TermHandle>,
    tail: TailHandle,
    /// The connections (bridge or remote link) that follow the tab.
    conns: HashSet<u64>,
}

/// The bridge key and the tabs remote viewers follow. Shared by the control channel's dispatcher (any worker thread)
/// and the window (the UI thread): every method takes the one short lock and never waits on anything else.
pub struct RemoteHost {
    key: String,
    follows: Mutex<HashMap<String, Follow>>,
    /// The followed tabs' numbered conversations (`remote.convo`, PR 4).
    convo: crate::remote_convo::ConvoFeeds,
    /// The number of the host's status and the status it numbered (`remote.status`, PR 4): it grows when the status
    /// changes, so a viewer sees at a glance that nothing did.
    status: Mutex<(u64, Value)>,
}

impl RemoteHost {
    /// A host with a fresh bridge key.
    pub fn new() -> Arc<Self> {
        Self::with_key(flare_term::session::new_ctl_token())
    }

    pub fn with_key(key: String) -> Arc<Self> {
        Arc::new(Self {
            key,
            follows: Mutex::new(HashMap::new()),
            convo: crate::remote_convo::ConvoFeeds::new(),
            status: Mutex::new((0, Value::Null)),
        })
    }

    fn lk(&self) -> MutexGuard<'_, HashMap<String, Follow>> {
        self.follows.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The key the bridge says `hello` with (written into the profile at start).
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Whether `token` is this run's bridge key.
    pub fn is_bridge_key(&self, token: &str) -> bool {
        !token.is_empty() && same_secret(&self.key, token)
    }

    /// Connection `conn` follows `tab` (the UI thread, at `remote.attach`).
    pub fn follow(&self, tab: &str, conn: u64, handle: Arc<TermHandle>, tail: TailHandle) {
        let mut map = self.lk();
        let f = map.entry(tab.to_string()).or_insert_with(|| Follow {
            handle: Arc::clone(&handle),
            tail: tail.clone(),
            conns: HashSet::new(),
        });
        // A tab whose shell was started again since has a new handle and tail.
        if !f.tail.same(&tail) {
            f.handle = handle;
            f.tail = tail;
        }
        f.conns.insert(conn);
    }

    /// Connection `conn` stops following `tab`. True when nobody follows it any more (the size goes back to the
    /// window). Never touches the tab's shell (rule 280).
    pub fn unfollow(&self, tab: &str, conn: u64) -> bool {
        let mut map = self.lk();
        let Some(f) = map.get_mut(tab) else {
            return false;
        };
        if !f.conns.remove(&conn) {
            return false;
        }
        if f.conns.is_empty() {
            map.remove(tab);
            drop(map);
            self.convo.forget(tab);
            return true;
        }
        false
    }

    /// A connection ended (its link dropped, or the bridge quit): the tabs nobody follows any more.
    pub fn gone(&self, conn: u64) -> Vec<String> {
        let mut map = self.lk();
        let mut last = Vec::new();
        map.retain(|tab, f| {
            if f.conns.remove(&conn) && f.conns.is_empty() {
                last.push(tab.clone());
                return false;
            }
            true
        });
        drop(map);
        for tab in &last {
            self.convo.forget(tab);
        }
        last.sort();
        last
    }

    /// The tab is closed, or its shell ended: nobody follows it any more.
    pub fn forget_tab(&self, tab: &str) {
        self.lk().remove(tab);
        self.convo.forget(tab);
    }

    /// A followed tab's handle and tail (any thread).
    pub fn followed(&self, tab: &str) -> Option<(Arc<TermHandle>, TailHandle)> {
        self.lk()
            .get(tab)
            .map(|f| (Arc::clone(&f.handle), f.tail.clone()))
    }

    /// Whether anyone follows `tab`: the host's own question notification is held back for it (rule 295).
    pub fn is_followed(&self, tab: &str) -> bool {
        self.lk().contains_key(tab)
    }

    /// The tabs remote viewers follow, sorted: the keep-awake counts the ones that work (rule 296).
    pub fn followed_tabs(&self) -> Vec<String> {
        let mut tabs: Vec<String> = self.lk().keys().cloned().collect();
        tabs.sort();
        tabs
    }

    /// The followed tabs' numbered conversations (`remote.convo`).
    pub fn convo(&self) -> &crate::remote_convo::ConvoFeeds {
        &self.convo
    }

    /// The number of `tabs` as the host's status: the last number while nothing changed, the next one when it did.
    pub fn status_seq(&self, tabs: &Value) -> u64 {
        let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
        if st.0 == 0 || st.1 != *tabs {
            st.0 += 1;
            st.1 = tabs.clone();
        }
        st.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_term::fake::{FakeShells, FakeSpawner};
    use flare_term::session::{SessionManager, SessionOptions};

    fn handle() -> Arc<TermHandle> {
        let mgr = SessionManager::new(
            FakeSpawner::new(),
            FakeShells::new(),
            SessionOptions::default(),
        );
        TermHandle::new("t1", mgr, 80, 24)
    }

    #[test]
    fn only_this_runs_key_is_the_bridge_key() {
        let h = RemoteHost::with_key("abc123".into());
        assert!(h.is_bridge_key("abc123"));
        assert!(!h.is_bridge_key("abc124"));
        assert!(!h.is_bridge_key("abc12"));
        assert!(!h.is_bridge_key(""));
        let a = RemoteHost::new();
        let b = RemoteHost::new();
        assert_ne!(a.key(), b.key(), "a new key every start");
        assert!(a.key().len() >= 32);
    }

    /// Rule 280: following and unfollowing is bookkeeping only; the last follower gone says the size goes back.
    #[test]
    fn a_tab_is_followed_until_its_last_connection_detaches_or_goes() {
        let host = RemoteHost::with_key("k".into());
        let tail = TailHandle::new(64);
        host.follow("t1", 1, handle(), tail.clone());
        host.follow("t1", 2, handle(), tail.clone());
        host.follow("t2", 1, handle(), TailHandle::new(64));
        assert!(host.is_followed("t1"));
        assert!(!host.unfollow("t1", 1), "connection 2 still follows");
        assert!(!host.unfollow("t1", 9), "9 never followed it");
        assert!(host.followed("t1").is_some_and(|(_, t)| t.same(&tail)));
        assert_eq!(host.gone(2), ["t1"]);
        assert!(!host.is_followed("t1"));
        assert!(host.unfollow("t2", 1));
        assert!(host.followed("t2").is_none());
        host.follow("t3", 5, handle(), TailHandle::new(8));
        host.forget_tab("t3");
        assert!(host.gone(5).is_empty());
    }

    /// Rule 294: the status's number stays while nothing changed and grows when something did.
    #[test]
    fn the_status_number_grows_only_when_the_status_changes() {
        let host = RemoteHost::with_key("k".into());
        let a = serde_json::json!([{"tab": "t1", "state": "idle"}]);
        let b = serde_json::json!([{"tab": "t1", "state": "working"}]);
        assert_eq!(host.status_seq(&a), 1);
        assert_eq!(host.status_seq(&a), 1);
        assert_eq!(host.status_seq(&b), 2);
        assert_eq!(host.status_seq(&a), 3);
        host.follow("t2", 1, handle(), TailHandle::new(8));
        host.follow("t1", 1, handle(), TailHandle::new(8));
        assert_eq!(host.followed_tabs(), ["t1", "t2"]);
    }

    #[test]
    fn the_bridge_key_is_written_whole_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_bridge_key(dir.path()), None);
        write_bridge_key(dir.path(), "secret-1").unwrap();
        assert_eq!(read_bridge_key(dir.path()).as_deref(), Some("secret-1"));
        write_bridge_key(dir.path(), "secret-2").unwrap();
        assert_eq!(read_bridge_key(dir.path()).as_deref(), Some("secret-2"));
        assert!(!dir.path().join("remote-bridge.key.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(bridge_key_path(dir.path()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
