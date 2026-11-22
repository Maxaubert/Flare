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

/// A command that shows no window.
pub fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
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
    let out = read(child.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|o| Box::new(o) as Box<dyn Read + Send>));
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
mod tests {
    use super::*;

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
        assert!(run(command(Path::new("/no/such/program")), Duration::from_secs(1)).is_err());
    }
}
