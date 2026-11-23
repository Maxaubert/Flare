//! The programs the emu host runs (adb, `emulator -accel-check`): never a window (rule 1370: `CREATE_NO_WINDOW` on
//! Windows), always a time limit, the output read whole. Called on blocking workers only, never on the control loop.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a program said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// A command that shows no window. adb's mDNS is off for every program the emu host runs (an adb it starts binds only
/// localhost, so Windows shows no Firewall dialog: `flare_device::emu::boot::ADB_MDNS`, rule 1370).
pub fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    let (k, v) = flare_device::emu::boot::ADB_MDNS;
    c.env(k, v);
    no_window(&mut c);
    c
}

/// No console window for a console program Flare's helper starts (Windows); nothing to do elsewhere.
pub fn no_window(c: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(flare_proc::job::NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = c;
}

/// BLOCKING, at most `limit`: runs `cmd` with no stdin and its output read; a program that runs over is killed and
/// answers Err.
pub fn run(mut cmd: Command, limit: Duration) -> Result<Ran, String> {
    let program = cmd.get_program().to_string_lossy().to_string();
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{program} could not start: {e}"))?;
    let read = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut s);
            }
            String::from_utf8_lossy(&s).to_string()
        })
    };
    let out = read(
        child
            .stdout
            .take()
            .map(|o| Box::new(o) as Box<dyn Read + Send>),
    );
    let err = read(
        child
            .stderr
            .take()
            .map(|o| Box::new(o) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code(),
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} took over {} s", limit.as_secs()));
            }
            Err(e) => return Err(format!("{program}: {e}")),
        }
    };
    Ok(Ran {
        code,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The emu host's source files, its generated code left out.
    pub(crate) fn sources() -> Vec<(String, String)> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        for e in std::fs::read_dir(&src).unwrap().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "rs") {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                out.push((name, std::fs::read_to_string(&p).unwrap()));
            }
        }
        assert!(out.len() > 5, "the sources were read");
        out
    }

    /// Rule 1370: every program the emu host runs is made here, with no console window.
    #[test]
    fn every_program_starts_without_a_window() {
        let needle = concat!("Command", "::new(");
        for (name, text) in sources() {
            let n = text.matches(needle).count();
            if name == "run.rs" {
                assert_eq!(n, 1, "one maker, `command`");
            } else {
                assert_eq!(n, 0, "{name} makes a program without `run::command`");
            }
        }
        #[cfg(windows)]
        assert_eq!(flare_proc::job::NO_WINDOW, 0x0800_0000);
        // adb stays off the network, so no Firewall dialog shows (rule 1370).
        let c = command(Path::new("adb"));
        assert!(
            c.get_envs()
                .any(|(k, v)| k == "ADB_MDNS" && v == Some(std::ffi::OsStr::new("0")))
        );
    }

    #[test]
    fn a_program_is_run_with_its_output_and_a_time_limit() {
        let (sh, a, say, sleep) = if cfg!(windows) {
            ("cmd", "/C", "echo hi", "ping -n 30 127.0.0.1 >NUL")
        } else {
            ("/bin/sh", "-c", "echo hi", "sleep 30")
        };
        let mut c = command(Path::new(sh));
        c.args([a, say]);
        let r = run(c, Duration::from_secs(20)).unwrap();
        assert!(r.ok());
        assert_eq!(r.stdout.trim(), "hi");
        let mut c = command(Path::new(sh));
        c.args([a, sleep]);
        let started = Instant::now();
        let e = run(c, Duration::from_millis(300)).unwrap_err();
        assert!(e.contains("took over"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(
            run(
                command(Path::new("/no/such/program")),
                Duration::from_secs(1)
            )
            .is_err()
        );
    }
}
