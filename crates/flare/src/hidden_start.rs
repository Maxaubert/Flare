//! Starting this machine's Flare hidden when none runs (remote sessions PR 5).
//!
//! Two callers. The serve process (`flare --serve-remote`, started by ssh for the other machine) makes sure a Flare runs
//! on this machine before it answers `remote.hello`; `flare open --on` makes sure one runs here before it asks it to
//! open a remote tab. Neither ever shows a window: the started Flare is `<this exe> --start-hidden`, a normal launch (the
//! lock, the pipe, the tab restore and live shells as ever) whose window is parked off every screen with the e2e's own
//! park code and never takes the focus (rule 301). A later normal launch brings it forward.
//!
//! The decision is pure over a [`Starter`] (a test fakes one): already running, start and wait for the pipe, no
//! desktop to start one in, or a pipe that never opened. The last two answer `host_not_ready` with one line.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_ctl::{CtlError, code};

/// How long a hidden start may take before its pipe answers.
pub const PIPE_WAIT: Duration = Duration::from_secs(60);
/// Between two looks for the pipe.
pub const PIPE_POLL: Duration = Duration::from_millis(250);

/// What the hidden start needs from the system. BLOCKING (a pipe connect, a process start): a worker's or the command's
/// own thread, never the UI thread.
pub trait Starter {
    /// Whether the running Flare's pipe takes a connection now.
    fn running(&self) -> bool;
    /// Whether this process is in a session with a desktop a window could live on (Windows: not session 0, where sshd
    /// runs its logins and no window is ever seen).
    fn has_desktop(&self) -> bool;
    /// Starts `<this exe> --start-hidden` for this profile and does not wait for it.
    fn start(&self) -> std::io::Result<()>;
}

/// How Flare came to be running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ready {
    /// It was running already: nothing was started.
    Running,
    /// It was started hidden and its pipe answered.
    Started,
}

fn not_ready(why: &str) -> CtlError {
    CtlError::new(code::HOST_NOT_READY, why)
}

/// The words of a refusal when this login has no desktop: one line, what to do.
pub const NO_DESKTOP: &str = "Flare is not running on this machine, and this login has no desktop to start it on; open Flare there once and try again";

/// Makes sure a Flare runs: `Running` when its pipe answers, else a hidden start and a wait of up to `wait` for the
/// pipe (looked at every `poll`). Never starts one where no window could live, and never starts two.
pub fn ensure_running(s: &dyn Starter, wait: Duration, poll: Duration) -> Result<Ready, CtlError> {
    if s.running() {
        return Ok(Ready::Running);
    }
    if !s.has_desktop() {
        return Err(not_ready(NO_DESKTOP));
    }
    s.start().map_err(|e| {
        not_ready(&format!(
            "Flare is not running on this machine and could not be started: {e}"
        ))
    })?;
    let end = Instant::now() + wait;
    loop {
        if s.running() {
            return Ok(Ready::Started);
        }
        if Instant::now() >= end {
            return Err(not_ready(&format!(
                "Flare was started on this machine but did not answer within {} s",
                wait.as_secs()
            )));
        }
        std::thread::sleep(poll); // sync-ok: the serve process or the `flare open` command waits for the pipe
    }
}

/// Pure: the started Flare's arguments: `--start-hidden`, the profile when it is not the default one, and `--e2e` when
/// the caller runs under the e2e (so a test's host is a test app too, parked and hooked, never the owner's look).
pub fn background_args(user_data_dir: Option<&Path>, e2e: bool) -> Vec<String> {
    let mut v = vec!["--start-hidden".to_string()];
    if let Some(d) = user_data_dir {
        v.push(format!("--user-data-dir={}", d.display()));
    }
    if e2e {
        v.push("--e2e".to_string());
    }
    v
}

/// The real system: this profile's pipe, this exe.
pub struct SystemStarter {
    pub pipe: Option<String>,
    pub user_data_dir: Option<PathBuf>,
    pub e2e: bool,
}

impl Starter for SystemStarter {
    fn running(&self) -> bool {
        let Some(pipe) = &self.pipe else {
            return false;
        };
        flare_ctl::connect(pipe, Duration::from_millis(500)).is_ok()
    }

    fn has_desktop(&self) -> bool {
        has_desktop()
    }

    fn start(&self) -> std::io::Result<()> {
        let exe = std::env::current_exe()?;
        let args = background_args(self.user_data_dir.as_deref(), self.e2e);
        spawn_detached(&exe, &args)
    }
}

/// Windows: whether this process runs in a session other than 0 (sshd's logins run in session 0, which has no desktop
/// anyone sees: a Flare started there could never be brought forward).
#[cfg(windows)]
pub fn has_desktop() -> bool {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    let mut session = 0u32;
    // SAFETY: this process's own id and a valid out pointer; a failure leaves `session` at 0.
    let ok = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } != 0;
    ok && session != 0
}

/// macOS: not known before the start (an ssh login of the person who is logged in at the screen can start the app,
/// one with nobody logged in cannot): the start is tried, and a pipe that never opens answers `host_not_ready`.
#[cfg(not(windows))]
pub fn has_desktop() -> bool {
    true
}

/// Starts `exe args` with no console, no stdio and nothing tying it to this process: it outlives the serve process and
/// the ssh session that started it.
fn spawn_detached(exe: &Path, args: &[String]) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(exe); // sync-ok: the serve process or the command's own thread
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // Out of the caller's job when the job allows it (sshd ends its job with the login), else as it is.
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        match cmd.spawn() {
            Ok(_) => return Ok(()),
            Err(e) if e.raw_os_error() == Some(5) => {
                cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
            }
            Err(e) => return Err(e),
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group: the hang-up of the ssh login does not reach it.
        cmd.process_group(0);
    }
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A system whose pipe answers after `up_after` looks once started (never when `None`).
    struct Fake {
        running: Cell<bool>,
        desktop: bool,
        up_after: Option<u32>,
        looks: Cell<u32>,
        starts: Cell<u32>,
    }

    impl Fake {
        fn new(running: bool, desktop: bool, up_after: Option<u32>) -> Fake {
            Fake {
                running: Cell::new(running),
                desktop,
                up_after,
                looks: Cell::new(0),
                starts: Cell::new(0),
            }
        }
    }

    impl Starter for Fake {
        fn running(&self) -> bool {
            if self.starts.get() > 0 {
                self.looks.set(self.looks.get() + 1);
                if self.up_after.is_some_and(|n| self.looks.get() >= n) {
                    self.running.set(true);
                }
            }
            self.running.get()
        }
        fn has_desktop(&self) -> bool {
            self.desktop
        }
        fn start(&self) -> std::io::Result<()> {
            self.starts.set(self.starts.get() + 1);
            Ok(())
        }
    }

    const POLL: Duration = Duration::from_millis(1);

    #[test]
    fn a_running_flare_is_used_and_nothing_is_started() {
        let f = Fake::new(true, true, None);
        assert_eq!(
            ensure_running(&f, Duration::from_secs(1), POLL).unwrap(),
            Ready::Running
        );
        assert_eq!(f.starts.get(), 0);
    }

    #[test]
    fn with_none_running_one_is_started_once_and_its_pipe_awaited() {
        let f = Fake::new(false, true, Some(3));
        assert_eq!(
            ensure_running(&f, Duration::from_secs(5), POLL).unwrap(),
            Ready::Started
        );
        assert_eq!(f.starts.get(), 1);
    }

    #[test]
    fn a_pipe_that_never_opens_is_host_not_ready_in_one_line() {
        let f = Fake::new(false, true, None);
        let e = ensure_running(&f, Duration::from_millis(30), POLL).unwrap_err();
        assert_eq!(e.code, code::HOST_NOT_READY);
        assert!(!e.message.contains('\n'));
        assert_eq!(f.starts.get(), 1, "never a second start");
    }

    /// No desktop (Windows session 0, where sshd's logins run): nothing is started, one line says what to do.
    #[test]
    fn with_no_desktop_nothing_is_started_and_the_line_says_what_to_do() {
        let f = Fake::new(false, false, Some(1));
        let e = ensure_running(&f, Duration::from_secs(1), POLL).unwrap_err();
        assert_eq!(e.code, code::HOST_NOT_READY);
        assert_eq!(e.message, NO_DESKTOP);
        assert_eq!(f.starts.get(), 0);
    }

    #[test]
    fn the_hidden_start_is_background_with_the_profile_and_the_e2e_passed_on() {
        assert_eq!(background_args(None, false), ["--start-hidden"]);
        let d = Path::new("C:\\p");
        assert_eq!(
            background_args(Some(d), true),
            ["--start-hidden", "--user-data-dir=C:\\p", "--e2e"]
        );
    }
}
