//! Restarts the system's shell so it shows the app icon Flare picked (rule 1263, 2026-11-14): the taskbar keeps the
//! icon it read when the window first showed, and the Dock keeps its own copy, until the shell starts again.
//!
//! - Windows: every `explorer.exe` of this user's session (never another session's) is ended, then the shell is
//!   started again. Winlogon starts it again by itself on many PCs; when no new Explorer shows within a short wait,
//!   `explorer.exe` is started by its absolute path under the system root.
//! - macOS: `/usr/bin/killall Dock`; launchd starts the Dock again at once.
//!
//! It blocks for up to a few seconds, so callers run it on a worker. Errors are short reasons with no path in them.
//! The tests never call it: Flare's controller reaches it through the `ShellRestart` seam, which tests fill with a
//! fake.

/// Restarts the shell. Ok once it was asked to start again.
pub fn restart_shell() -> Result<(), String> {
    imp::restart()
}

/// The real restart behind Settings' seam (`flare_settings::app_icon::ShellRestart`).
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemShellRestart;

impl flare_settings::app_icon::ShellRestart for SystemShellRestart {
    fn restart(&self) -> Result<(), String> {
        restart_shell()
    }
}

#[cfg(windows)]
mod imp {
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
        WaitForSingleObject,
    };

    /// How long each ended Explorer is waited for, and how long Winlogon gets to start the shell by itself.
    const END_WAIT_MS: u32 = 5_000;
    const COME_BACK: Duration = Duration::from_millis(2_500);
    const POLL: Duration = Duration::from_millis(100);

    /// The session of a process; None when Windows does not say.
    fn session_of(pid: u32) -> Option<u32> {
        let mut session = 0u32;
        // SAFETY: `session` is a valid, writable u32 for the call's out parameter.
        let ok = unsafe { ProcessIdToSessionId(pid, &mut session) } != 0;
        ok.then_some(session)
    }

    /// The ids of every `explorer.exe` in `session`.
    fn explorers_in(session: u32) -> Vec<u32> {
        let mut found = Vec::new();
        // SAFETY: the snapshot handle is checked against INVALID_HANDLE_VALUE and closed below.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE {
            return found;
        }
        // SAFETY: an all-zero PROCESSENTRY32W is a valid starting value; dwSize is set before use.
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        // SAFETY: `snap` is a live snapshot and `entry` is a writable, correctly sized struct.
        let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if name.eq_ignore_ascii_case("explorer.exe")
                && session_of(entry.th32ProcessID) == Some(session)
            {
                found.push(entry.th32ProcessID);
            }
            // SAFETY: as for Process32FirstW above.
            more = unsafe { Process32NextW(snap, &mut entry) } != 0;
        }
        // SAFETY: `snap` came from CreateToolhelp32Snapshot and is closed exactly once.
        unsafe { CloseHandle(snap) };
        found
    }

    /// Ends one process and waits for it to be gone. False when it could not be opened or ended.
    fn end(pid: u32) -> bool {
        // SAFETY: OpenProcess takes plain values; a null handle is checked before use.
        let h = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
        if h.is_null() {
            return false;
        }
        // SAFETY: `h` is a live process handle opened with PROCESS_TERMINATE.
        let ended = unsafe { TerminateProcess(h, 1) } != 0;
        if ended {
            // SAFETY: `h` was opened with SYNCHRONIZE; the wait is bounded.
            unsafe { WaitForSingleObject(h, END_WAIT_MS) };
        }
        // SAFETY: `h` came from OpenProcess and is closed exactly once.
        unsafe { CloseHandle(h) };
        ended
    }

    /// Starts `explorer.exe` by its absolute path under the system root (never the bare name).
    fn start_explorer() -> Result<(), String> {
        let sys_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        std::process::Command::new(crate::openers::explorer_path(&sys_root)) // sync-ok: the restart worker only
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("explorer did not start: {}", e.kind()))
    }

    pub fn restart() -> Result<(), String> {
        // SAFETY: GetCurrentProcessId takes nothing and cannot fail.
        let me = unsafe { GetCurrentProcessId() };
        let session = session_of(me).ok_or_else(|| "no session id".to_string())?;
        let before = explorers_in(session);
        let ended = before.iter().filter(|&&pid| end(pid)).count();
        if !before.is_empty() && ended == 0 {
            return Err("explorer could not be ended".to_string());
        }
        // Winlogon brings the shell back by itself on many PCs; a second shell would open a folder window instead.
        let start = Instant::now();
        while start.elapsed() < COME_BACK {
            if explorers_in(session)
                .iter()
                .any(|pid| !before.contains(pid))
            {
                return Ok(());
            }
            std::thread::sleep(POLL); // sync-ok: the restart worker only
        }
        start_explorer()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    pub fn restart() -> Result<(), String> {
        let status = std::process::Command::new("/usr/bin/killall") // sync-ok: the restart worker only
            .arg("Dock")
            .status()
            .map_err(|e| format!("killall did not start: {}", e.kind()))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("killall ended with {:?}", status.code()))
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    pub fn restart() -> Result<(), String> {
        Err("no shell to restart here".to_string())
    }
}
