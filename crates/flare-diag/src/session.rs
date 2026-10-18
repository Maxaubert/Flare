//! M9 Task 19: the session line and the quit line of the diagnostics log.
//!
//! Port of Prism Terminal's `core/main/diagnostics.ts`: the `session` line opens the file, `session-end`
//! is written when Windows is shutting down (flushed at once, no `quit` follows), and `quit` is the last
//! line of a normal run. A null log (no folder given) writes nothing and creates nothing.

use crate::writer::DiagLog;
use serde_json::{Map, Value};

/// The app's name on the session line (Flare's own name; `flare-diag` does not depend on `flare-win`).
pub const APP_NAME: &str = "Flare";

/// What the session line says about the run. `e2e` is written only when true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub version: String,
    pub runtime: String,
    pub gpu: String,
    pub windows: String,
    pub pid: u32,
    pub verbose: bool,
    pub cpus: u32,
    pub cpu: String,
    pub ram_gb: u32,
    pub e2e: bool,
}

/// The fields of the `session` line: app, version, runtime, gpu, windows, pid, verbose, cpus, cpu,
/// ramGb, and e2e ONLY when the run is an e2e run.
pub fn session_fields(i: &SessionInfo) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("app".into(), Value::String(APP_NAME.into()));
    m.insert("version".into(), Value::String(i.version.clone()));
    m.insert("runtime".into(), Value::String(i.runtime.clone()));
    m.insert("gpu".into(), Value::String(i.gpu.clone()));
    m.insert("windows".into(), Value::String(i.windows.clone()));
    m.insert("pid".into(), Value::from(i.pid));
    m.insert("verbose".into(), Value::Bool(i.verbose));
    m.insert("cpus".into(), Value::from(i.cpus));
    m.insert("cpu".into(), Value::String(i.cpu.clone()));
    m.insert("ramGb".into(), Value::from(i.ram_gb));
    if i.e2e {
        m.insert("e2e".into(), Value::Bool(true));
    }
    m
}

/// The running app's diagnostics: the session line at start, `session-end` and `quit` at the end.
pub struct Diagnostics {
    log: DiagLog,
}

impl Diagnostics {
    /// Writes the `session` line, unless the log is the null log (no folder): then nothing is written.
    pub fn start(log: DiagLog, info: &SessionInfo) -> Diagnostics {
        if !log.dir().as_os_str().is_empty() {
            log.write("main", "session", session_fields(info));
        }
        Diagnostics { log }
    }

    /// The log the run writes to.
    pub fn log(&self) -> &DiagLog {
        &self.log
    }

    /// Windows is shutting down or logging off: `session-end`, written at once (no `quit` follows).
    pub fn session_end(&self) {
        self.log.write("main", "session-end", Map::new());
        self.log.flush_sync();
    }

    /// The quit that goes ahead: `quit` is the last line, then the log closes.
    pub fn stop(&self) {
        self.log.write("main", "quit", Map::new());
        self.log.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::{DIAG_FILE, DiagLogOptions};
    use serde_json::json;
    use std::fs;

    fn info(e2e: bool) -> SessionInfo {
        SessionInfo {
            version: "0.1.0".into(),
            runtime: "rust 1.99".into(),
            gpu: "Test GPU".into(),
            windows: "10.0.26300".into(),
            pid: 4242,
            verbose: false,
            cpus: 8,
            cpu: "Test CPU".into(),
            ram_gb: 32,
            e2e,
        }
    }

    fn read_lines(path: &std::path::Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("a line is JSON"))
            .collect()
    }

    #[test]
    fn logs_nothing_for_a_host_that_gave_no_folder() {
        let d = Diagnostics::start(DiagLog::null(), &info(false));
        assert_eq!(d.log().queued(), 0);
        assert!(!d.log().verbose());
        assert!(d.log().file().as_os_str().is_empty());
        d.stop();
        assert_eq!(d.log().queued(), 0);
    }

    #[test]
    fn opens_with_a_session_line_and_closes_with_a_quit_line() {
        let dir = tempfile::tempdir().expect("a temp folder");
        let mut o = DiagLogOptions::new(dir.path().join("logs"));
        o.flush_ms = 3_600_000;
        let d = Diagnostics::start(DiagLog::start(o), &info(false));
        d.stop();
        let lines = read_lines(&dir.path().join("logs").join(DIAG_FILE));
        assert_eq!(lines[0]["k"], "session");
        assert_eq!(lines[0]["src"], "main");
        assert_eq!(lines[0]["app"], "Flare");
        assert_eq!(lines[0]["version"], "0.1.0");
        assert_eq!(lines[0]["verbose"], false);
        assert!(lines[0].get("e2e").is_none());
        assert_eq!(lines.last().expect("a last line")["k"], "quit");
    }

    #[test]
    fn the_session_fields_are_exactly_these_keys() {
        let mut keys: Vec<String> = session_fields(&info(false)).keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "app", "cpu", "cpus", "gpu", "pid", "ramGb", "runtime", "verbose", "version",
                "windows"
            ]
        );
        let with_e2e = session_fields(&info(true));
        assert_eq!(with_e2e.get("e2e"), Some(&json!(true)));
        assert_eq!(with_e2e.len(), 11);
    }

    #[test]
    fn session_end_flushes_at_once() {
        let dir = tempfile::tempdir().expect("a temp folder");
        let mut o = DiagLogOptions::new(dir.path().join("logs"));
        o.flush_ms = 3_600_000;
        let d = Diagnostics::start(DiagLog::start(o), &info(false));
        d.session_end();
        let lines = read_lines(&dir.path().join("logs").join(DIAG_FILE));
        assert_eq!(lines.last().expect("a last line")["k"], "session-end");
        d.stop();
    }
}
