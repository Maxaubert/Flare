//! Everything a scenario needs that Prism Terminal's run.mjs builds inline, written once (Task 15).

pub mod agent_cmd;
pub mod agent_proc;
pub mod flood;
pub mod hooklines;
pub mod progs;
pub mod sonara;

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// The PowerShell that runs the scenarios' scripts: Windows PowerShell on Windows; pwsh on macOS
/// (rule 336), where it is found where the app's shell detection looks.
#[cfg(windows)]
pub fn powershell() -> String {
    "powershell.exe".to_string()
}

/// macOS: pwsh by its absolute path (rule 336); `pwsh` on PATH when none of the usual places has it.
#[cfg(not(windows))]
pub fn powershell() -> String {
    [
        "/opt/homebrew/bin/pwsh",
        "/usr/local/bin/pwsh",
        "/usr/local/microsoft/powershell/7/pwsh",
    ]
    .iter()
    .find(|p| Path::new(p).is_file())
    .map_or_else(|| "pwsh".to_string(), |p| (*p).to_string())
}

/// The line a PowerShell tab runs to start the script `ps1` in a PowerShell of its own: the Windows
/// text unchanged; on macOS pwsh by its path (rule 336).
pub fn ps_file_line(ps1: &Path) -> String {
    #[cfg(windows)]
    {
        format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -File '{}'",
            ps1.display()
        )
    }
    #[cfg(not(windows))]
    {
        format!("& '{}' -NoProfile -File '{}'", powershell(), ps1.display())
    }
}

/// Writes the runner of `<dir>/<name>.ps1` and returns it, the program a shell runs: `<name>.cmd`
/// under Windows PowerShell on Windows (the same text as before); on macOS an executable `<name>`
/// shell script that runs it under pwsh (rule 336).
pub fn ps_runner(dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    #[cfg(windows)]
    {
        let cmd = dir.join(format!("{name}.cmd"));
        std::fs::write(
            &cmd,
            format!("@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \"%~dp0{name}.ps1\"\r\n"),
        )?;
        Ok(cmd)
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let run = dir.join(name);
        let ps1 = dir.join(format!("{name}.ps1"));
        let quote = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
        std::fs::write(
            &run,
            format!(
                "#!/bin/sh\nexec {} -NoProfile -File {} \"$@\"\n",
                quote(Path::new(&powershell())),
                quote(&ps1)
            ),
        )?;
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755))?;
        Ok(run)
    }
}

/// A folder of downloaded fixtures (`.e2e-cache`). A file is used only when its SHA-256 is the
/// pinned one; a download whose hash differs is removed and refused. The dictation fixtures use their
/// own cache (`fixtures.rs`, M8 Task 28), filled only by `--fetch-fixtures`; this one is the test's.
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

/// macOS: downloads `url` into `file` with curl (rule 336). Test code: this blocks.
#[cfg(not(windows))]
fn download(url: &str, file: &Path) -> Result<(), String> {
    let out = Command::new("/usr/bin/curl")
        .args(["-fsSL", "-o"])
        .arg(file)
        .arg(url)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
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

/// Downloads `url` into `file` with Windows PowerShell. The URL and the path travel in environment
/// variables, so no quoting is involved. Test code: this blocks, and it is never on the app's path.
#[cfg(windows)]
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
