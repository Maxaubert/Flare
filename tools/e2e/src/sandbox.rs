//! M9 Task 36: the e2e sandbox (a throwaway profile, a throwaway HKCU subtree).

use std::fs;
use std::path::{Path, PathBuf};

use flare_win::names::PROFILE_FOLDER;
use sha2::{Digest, Sha256};

use crate::regsandbox::SandboxRoot;

pub struct Sandbox {
    dir: Option<tempfile::TempDir>,
    pub root: PathBuf,
    pub roaming: PathBuf,
    pub local: PathBuf,
    /// `roaming\Flare`
    pub profile: PathBuf,
    /// `Software\FlareE2E\<uuid>`
    pub reg_root: String,
    reg: SandboxRoot,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let dir = tempfile::Builder::new()
            .prefix("flare-e2e-")
            .tempdir()
            .expect("temp dir");
        let root = dir.path().to_path_buf();
        let roaming = root.join("Roaming");
        let local = root.join("Local");
        let profile = roaming.join(PROFILE_FOLDER);
        for d in [&roaming, &local, &profile] {
            fs::create_dir_all(d).expect("sandbox folder");
        }
        let reg = SandboxRoot::new();
        let reg_root = reg.key().to_string();
        Sandbox {
            dir: Some(dir),
            root,
            roaming,
            local,
            profile,
            reg_root,
            reg,
        }
    }

    /// APPDATA, LOCALAPPDATA and the verb registry root, for a launched child.
    pub fn env(&self) -> Vec<(String, String)> {
        vec![
            (
                "APPDATA".into(),
                self.roaming.to_string_lossy().into_owned(),
            ),
            (
                "LOCALAPPDATA".into(),
                self.local.to_string_lossy().into_owned(),
            ),
            ("FLARE_E2E_VERB_ROOT".into(), self.reg_root.clone()),
        ]
    }

    /// The sandbox registry subtree (for scenarios that read or seed it).
    pub fn registry(&self) -> &SandboxRoot {
        &self.reg
    }

    pub fn snapshot(&self, sub: &Path) -> TreeHash {
        let base = if sub.is_absolute() {
            sub.to_path_buf()
        } else {
            self.root.join(sub)
        };
        let mut lines = Vec::new();
        collect(&base, &base, &mut lines);
        lines.sort();
        let mut h = Sha256::new();
        for l in &lines {
            h.update(l.as_bytes());
            h.update(b"\n");
        }
        TreeHash {
            hash: hex(&h.finalize()),
            listing: lines,
        }
    }
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // The folder goes first (explicitly, so a failure is not swallowed by TempDir's drop);
        // the registry subtree goes when `reg` drops after this body.
        if let Some(d) = self.dir.take() {
            let _ = d.close();
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(base, &p, out);
        } else if let Ok(bytes) = fs::read(&p) {
            let rel = p
                .strip_prefix(base)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(format!(
                "{rel}|{}|{}",
                bytes.len(),
                hex(&Sha256::digest(&bytes))
            ));
        }
    }
}

/// SHA-256 over the sorted (relative path, size, content hash) of a tree.
#[derive(Debug, Clone)]
pub struct TreeHash {
    hash: String,
    listing: Vec<String>,
}

impl PartialEq for TreeHash {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl Eq for TreeHash {}

impl TreeHash {
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// The debug listing, one `path|size|hash` line per file.
    pub fn listing(&self) -> &[String] {
        &self.listing
    }
}
