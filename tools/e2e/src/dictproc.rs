//! Process helpers of the dictation scenarios (M8 Task 28): the engine and host processes a scenario
//! starts, found and reaped by path.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The speech server the engine folder holds.
pub const SPEECH_SERVER: &str = "whisper-server.exe";
/// Parakeet's one-pass runner in the same folder.
pub const PARAKEET_CLI: &str = "parakeet-cli.exe";
/// Flare's resident Parakeet host, beside the Flare exe.
pub const PARAKEET_HOST: &str = "flare-parakeet-host.exe";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Speech servers started out of `engine_dir` (PT's `ourSpeechServers`). Other copies are not counted.
pub fn speech_servers(engine_dir: &Path) -> Result<usize, String> {
    Ok(count_under(&running(SPEECH_SERVER)?, engine_dir))
}

/// Parakeet one-pass runners started out of `engine_dir` (PT's `ourParakeetPasses`).
pub fn parakeet_passes(engine_dir: &Path) -> Result<usize, String> {
    Ok(count_under(&running(PARAKEET_CLI)?, engine_dir))
}

/// Resident Parakeet hosts started out of `exe_dir`.
pub fn parakeet_hosts(exe_dir: &Path) -> Result<usize, String> {
    Ok(count_under(&running(PARAKEET_HOST)?, exe_dir))
}

/// The paths of every running process named `image`. The name is one of this file's constants, never
/// caller text; the query runs hidden.
pub fn running(image: &str) -> Result<Vec<PathBuf>, String> {
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='{image}'\" | ForEach-Object {{ $_.ExecutablePath }}"
    );
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("the process list could not be read: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "the process list could not be read ({})",
            out.status
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// `dir` as a lowercase path with backslashes and one trailing backslash: a whole folder name, so
/// `whisper` is not `whisper2`.
fn folder_prefix(dir: &Path) -> String {
    let mut s = dir
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    while s.ends_with('\\') {
        s.pop();
    }
    s.push('\\');
    s
}

/// How many of `paths` lie under `dir` (case-insensitive, whole folder names only).
pub fn count_under(paths: &[PathBuf], dir: &Path) -> usize {
    let prefix = folder_prefix(dir);
    paths
        .iter()
        .filter(|p| {
            p.to_string_lossy()
                .replace('/', "\\")
                .to_ascii_lowercase()
                .starts_with(&prefix)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processes_outside_the_engine_folder_are_not_counted() {
        let engine = Path::new(r"C:\Users\Admin\Documents\Claude\Github\Flare\vendor\whisper");
        let paths = vec![
            PathBuf::from(
                r"C:\Users\Admin\Documents\Claude\Github\Flare\vendor\whisper\whisper-server.exe",
            ),
            // The owner's own dictation tool runs its own server: never counted, never touched.
            PathBuf::from(
                r"C:\Program Files\Another Dictation App\resources\bin\whisper\whisper-server.exe",
            ),
            // A folder whose name starts the same is a different folder.
            PathBuf::from(
                r"C:\Users\Admin\Documents\Claude\Github\Flare\vendor\whisper2\whisper-server.exe",
            ),
            PathBuf::from(
                r"C:\Users\Admin\Documents\Claude\Github\Flare\.claude\targets\t1\debug\whisper-server.exe",
            ),
        ];
        assert_eq!(count_under(&paths, engine), 1);
        // Case does not matter and a forward slash is the same separator.
        assert_eq!(
            count_under(
                &[PathBuf::from(
                    r"c:/users/admin/documents/claude/github/flare/vendor/WHISPER/whisper-server.exe"
                )],
                engine
            ),
            1
        );
        assert_eq!(count_under(&[], engine), 0);
    }

    #[test]
    fn the_process_names_are_the_shipped_binaries() {
        assert_eq!(SPEECH_SERVER, "whisper-server.exe");
        assert_eq!(PARAKEET_CLI, "parakeet-cli.exe");
        assert_eq!(PARAKEET_HOST, "flare-parakeet-host.exe");
    }
}
