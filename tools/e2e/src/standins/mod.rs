//! Everything a scenario needs that Prism Terminal's run.mjs builds inline, written once (Task 15).

pub mod agent_cmd;
pub mod flood;
pub mod hooklines;
pub mod progs;
pub mod sonara;

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// A folder of downloaded fixtures (`.e2e-cache`). A file is used only when its SHA-256 is the
/// pinned one; a download whose hash differs is removed and refused. Never called before M8: no
/// fixture is needed yet, so the dictation audio is not fetched here.
#[derive(Clone, Debug)]
pub struct FixtureCache {
    pub dir: PathBuf,
}

impl FixtureCache {
    /// The cached file `name`, fetched from `url` first when it is missing or its hash is wrong.
    pub fn fetch(&self, name: &str, url: &str, sha256: &str) -> Result<PathBuf, String> {
        if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
            return Err(format!("{name}: not a plain file name"));
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("the cache folder: {e}"))?;
        let file = self.dir.join(name);
        if file.is_file() && verify(&file, sha256).is_ok() {
            return Ok(file);
        }
        download(url, &file)?;
        verify(&file, sha256)
    }
}

/// Accepts `file` only when its SHA-256 equals `expected` (lowercase or uppercase hex). A file that
/// does not match is removed, so a refused fixture is never used later.
pub fn verify(file: &Path, expected: &str) -> Result<PathBuf, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let got: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got.eq_ignore_ascii_case(expected) {
        return Ok(file.to_path_buf());
    }
    let _ = std::fs::remove_file(file);
    Err(format!(
        "{}: SHA-256 mismatch ({got})",
        file.file_name().unwrap_or_default().to_string_lossy()
    ))
}

/// Downloads `url` into `file` with Windows PowerShell. The URL and the path travel in environment
/// variables, so no quoting is involved. Test code: this blocks, and it is never on the app's path.
fn download(url: &str, file: &Path) -> Result<(), String> {
    let out = Command::new("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
        .arg("Invoke-WebRequest -UseBasicParsing -Uri $env:FLARE_FIXTURE_URL -OutFile $env:FLARE_FIXTURE_OUT")
        .env("FLARE_FIXTURE_URL", url)
        .env("FLARE_FIXTURE_OUT", file)
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(file);
        Err(format!(
            "download failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}
