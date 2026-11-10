//! M9 Task 36: the e2e sandbox (a throwaway profile, a throwaway HKCU subtree).

use std::fs;
use std::path::{Path, PathBuf};

use flare_win::names::PROFILE_FOLDER;
use sha2::{Digest, Sha256};

#[cfg(windows)]
use crate::regsandbox::SandboxRoot;

pub struct Sandbox {
    dir: Option<tempfile::TempDir>,
    pub root: PathBuf,
    pub roaming: PathBuf,
    pub local: PathBuf,
    /// `roaming\Flare`
    pub profile: PathBuf,
    /// `Software\FlareE2E\<uuid>`
    #[cfg(windows)]
    pub reg_root: String,
    #[cfg(windows)]
    reg: SandboxRoot,
    /// macOS (rule 1104): the app's HOME, so `~/Library/...` and the shells' rc files are the world's.
    #[cfg(not(windows))]
    pub home: PathBuf,
}

impl Sandbox {
    #[cfg(windows)]
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

    /// macOS (rule 1104): the world is a HOME of its own (`home/`), its `Library/Application Support`
    /// both the roaming and the local folder (the Mac app keeps both there) and the profile `Flare` in it.
    /// The root is the canonical path (`/private/var/...`, not the `/var` link), so the app, the hook's
    /// socket name and the reaper all see one spelling of it.
    #[cfg(not(windows))]
    pub fn new() -> Sandbox {
        let dir = tempfile::Builder::new()
            .prefix("flare-e2e-")
            .tempdir()
            .expect("temp dir");
        let root = fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_path_buf());
        let home = root.join("home");
        let roaming = home.join("Library").join("Application Support");
        let local = roaming.clone();
        let profile = roaming.join(PROFILE_FOLDER);
        for d in [&home, &roaming, &profile] {
            fs::create_dir_all(d).expect("sandbox folder");
        }
        write_pwsh_profile(&home);
        Sandbox {
            dir: Some(dir),
            root,
            roaming,
            local,
            profile,
            home,
        }
    }

    /// macOS: the world's HOME for a launched child (there is no registry to sandbox), and pwsh as
    /// the tab's shell when it is installed (`FLARE_E2E_SHELL`, read only under `--e2e`, rule 1106): the
    /// scenarios read PowerShell's prompt, as on Windows. A profile that saves a shell still wins.
    #[cfg(not(windows))]
    pub fn env(&self) -> Vec<(String, String)> {
        let mut env = vec![("HOME".into(), self.home.to_string_lossy().into_owned())];
        if mac_pwsh().is_some() {
            env.push(("FLARE_E2E_SHELL".into(), "pwsh".into()));
        }
        env
    }

    /// APPDATA, LOCALAPPDATA and the verb registry root, for a launched child.
    #[cfg(windows)]
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
    #[cfg(windows)]
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

/// macOS (rule 1112): the world's pwsh profile puts PSReadLine in its Windows edit mode. pwsh on a Mac
/// starts in Emacs mode, where Escape is a prefix and not "clear the line", so a scenario's Escape
/// swallowed the next paste's escape sequence and left the line it meant to clear. The scenarios are
/// written against PowerShell as it is on Windows; this is the shell's setting, never the app's. Clear-Host
/// clears the scrollback too, as it does on Windows: pwsh on a Mac sends the terminfo `clear` alone, the
/// screen went into the scrollback, and a question box "cleared" that way was still the last thing
/// written (agentHooks, run 42587853627). `cls` is its alias, as on Windows.
#[cfg(not(windows))]
pub const PWSH_PROFILE: &str = "try { Set-PSReadLineOption -EditMode Windows } catch { }\n\
function Clear-Host { [Console]::Write([string][char]27 + '[H' + [char]27 + '[2J' + [char]27 + '[3J') }\n\
Set-Alias -Name cls -Value Clear-Host -Force\n";

#[cfg(not(windows))]
fn write_pwsh_profile(home: &Path) {
    let dir = home.join(".config").join("powershell");
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(dir.join("Microsoft.PowerShell_profile.ps1"), PWSH_PROFILE);
    }
}

/// macOS: where pwsh is, the places the app's shell detection looks (Homebrew, the package).
#[cfg(not(windows))]
pub fn mac_pwsh() -> Option<PathBuf> {
    [
        "/opt/homebrew/bin/pwsh",
        "/usr/local/bin/pwsh",
        "/usr/local/microsoft/powershell/7/pwsh",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
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
