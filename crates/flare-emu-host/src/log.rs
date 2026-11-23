//! `emu-host.log` (`<profile>/logs/emu-host.log`), rotated like `sim-host.log`: 1 MB, then `.1` and `.2` kept. Without
//! a log file the lines go to stderr (Flare logs the emu host's stderr). Never a token (rule 1362): the callers log
//! what they choose, and no type that holds a token prints it.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The log's size before it rotates, and the copies kept.
const LOG_MAX: u64 = 1024 * 1024;
const LOG_KEEP: usize = 3;

struct HostLog {
    path: Option<PathBuf>,
    file: Option<File>,
    size: u64,
}

impl HostLog {
    fn open(path: Option<PathBuf>) -> HostLog {
        let file = path.as_ref().and_then(|p| {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            OpenOptions::new().create(true).append(true).open(p).ok()
        });
        let size = file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len());
        HostLog { path, file, size }
    }

    fn line(&mut self, text: &str) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let l = format!("{secs} [{}] {text}\n", std::process::id());
        let Some(f) = self.file.as_mut() else {
            eprint!("{l}");
            return;
        };
        let _ = f.write_all(l.as_bytes());
        self.size += l.len() as u64;
        if self.size >= LOG_MAX
            && let Some(p) = self.path.clone()
        {
            self.file = None;
            for i in (1..LOG_KEEP).rev() {
                let from = if i == 1 {
                    p.clone()
                } else {
                    p.with_extension(format!("log.{}", i - 1))
                };
                let _ = std::fs::rename(&from, p.with_extension(format!("log.{i}")));
            }
            *self = HostLog::open(Some(p));
        }
    }
}

/// The emu host's log, shared by its threads and tasks.
#[derive(Clone)]
pub struct Log(Arc<Mutex<HostLog>>);

impl Log {
    pub fn open(path: Option<PathBuf>) -> Log {
        Log(Arc::new(Mutex::new(HostLog::open(path))))
    }

    pub fn line(&self, text: &str) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).line(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_rotates_at_a_megabyte_and_keeps_two_copies() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("logs/emu-host.log");
        let log = Log::open(Some(p.clone()));
        let long = "x".repeat(4096);
        for _ in 0..(3 * 1024 / 4 + 10) {
            log.line(&long);
        }
        assert!(p.exists());
        assert!(p.with_extension("log.1").exists());
        assert!(p.with_extension("log.2").exists());
        assert!(!p.with_extension("log.3").exists());
        assert!(std::fs::metadata(&p).unwrap().len() < LOG_MAX);
    }
}
