//! M9 Task 36: the e2e registry sandbox under HKCU\Software\FlareE2E\<uuid>.
//!
//! `SandboxRoot` is the only type here that writes or deletes, and only below its own
//! `Software\FlareE2E\<uuid>` key. `RegProbe` (below it) only reads the real classes hive.

use flare_win::names::{E2E_REG_PREFIX, VERB_KEY};
use flare_win::registry::{Registry, WinRegistry};

/// A throwaway subtree. Drop removes it (and the parent when it is empty).
pub struct SandboxRoot {
    key: String,
}

impl SandboxRoot {
    pub fn new() -> Self {
        Self {
            key: format!("{E2E_REG_PREFIX}{}", uuid::Uuid::new_v4()),
        }
    }

    /// The key path below HKCU, always under the sandbox prefix.
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn set(&self, sub: &str, name: Option<&str>, value: &str) {
        let k = format!("{}\\{sub}", self.key);
        let _ = WinRegistry::current_user().set_string(&k, name, value);
    }

    pub fn exists(&self) -> bool {
        WinRegistry::current_user()
            .key_exists(&self.key)
            .unwrap_or(false)
    }
}

impl Default for SandboxRoot {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SandboxRoot {
    fn drop(&mut self) {
        if !self.key.starts_with(E2E_REG_PREFIX) {
            return;
        }
        let reg = WinRegistry::current_user();
        let _ = reg.delete_tree(&self.key);
        let _ = reg.delete_if_empty(E2E_REG_PREFIX.trim_end_matches('\\'));
    }
}

/// What the real verb keys hold, for a before/after comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealVerbs {
    /// (key path below HKCU, default value or None when the key or value is missing)
    pub entries: Vec<(String, Option<String>)>,
}

pub struct RegProbe;

impl RegProbe {
    /// Reads, never writes: the Flare verb and its neighbour's, folder and folder background.
    pub fn real_verb_values() -> RealVerbs {
        let reg = WinRegistry::current_user();
        let neighbour = ["Pri", "smTerminal"].concat();
        let mut entries = Vec::new();
        for name in [VERB_KEY, neighbour.as_str()] {
            for base in [
                "Software\\Classes\\Directory\\shell",
                "Software\\Classes\\Directory\\Background\\shell",
            ] {
                for sub in ["", "\\command"] {
                    let key = format!("{base}\\{name}{sub}");
                    let v = reg.get_string(&key, None).ok().flatten();
                    entries.push((key, v));
                }
            }
        }
        RealVerbs { entries }
    }
}
