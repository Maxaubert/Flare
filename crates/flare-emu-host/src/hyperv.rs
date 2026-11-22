//! Turning on Windows Hypervisor Platform (Android plan PR A2b, spec "Setup", state 4): the Android emulator needs it on
//! Windows. Run only for the person's click on [Turn on] (the `turn_on_hypervisor` setup step): `dism` elevated through
//! `ShellExecuteExW` with the `runas` verb, so Windows shows its own UAC prompt and the person decides. Its console
//! window is hidden (`SW_HIDE`). This is the only step of Android setup outside Flare.

/// The program, under the Windows folder.
pub const DISM: &str = "System32\\dism.exe";
/// Its arguments.
pub const DISM_ARGS: &str =
    "/online /enable-feature /featurename:HypervisorPlatform /all /norestart";
/// dism's "done, restart to finish".
pub const RESTART_NEEDED: u32 = 3010;
/// The person said no to the UAC prompt (`ERROR_CANCELLED`).
#[cfg(windows)]
const CANCELLED: u32 = 1223;

/// What dism's exit code means: Ok(true) when Windows must restart to finish.
pub fn outcome(code: u32) -> Result<bool, String> {
    match code {
        0 | RESTART_NEEDED => Ok(true),
        c => Err(format!(
            "Windows could not turn on the hypervisor (dism {c})"
        )),
    }
}

/// BLOCKING until dism ends (the person may wait at the UAC prompt): turns the feature on. Ok(true): restart Windows.
#[cfg(windows)]
pub fn turn_on() -> Result<bool, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let windir = std::env::var_os("SystemRoot").map_or_else(
        || std::path::PathBuf::from("C:\\Windows"),
        std::path::PathBuf::from,
    );
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain([0]).collect::<Vec<u16>>();
    let file = wide(windir.join(DISM).as_os_str());
    let params = wide(std::ffi::OsStr::new(DISM_ARGS));
    let verb = wide(std::ffi::OsStr::new("runas"));
    // SAFETY: an all-zero SHELLEXECUTEINFOW is a valid empty one (null pointers and handles, zero flags).
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE;
    // SAFETY: `info` is filled as above and its strings outlive the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // SAFETY: reads this thread's last error, right after the failed call.
        let e = unsafe { GetLastError() };
        return Err(if e == CANCELLED {
            "Windows Hypervisor Platform was not turned on (the prompt was cancelled).".to_string()
        } else {
            format!("dism could not start (error {e})")
        });
    }
    if info.hProcess.is_null() {
        return Err("dism started without a process to wait for".to_string());
    }
    let mut code: u32 = 0;
    // SAFETY: `hProcess` is the live handle SEE_MASK_NOCLOSEPROCESS gave; it is closed once, here.
    let got = unsafe {
        let waited = WaitForSingleObject(info.hProcess, INFINITE);
        let got = waited == WAIT_OBJECT_0 && GetExitCodeProcess(info.hProcess, &mut code) != 0;
        CloseHandle(info.hProcess);
        got
    };
    if !got {
        return Err("dism's result could not be read".to_string());
    }
    outcome(code)
}

/// Only Windows has it.
#[cfg(not(windows))]
pub fn turn_on() -> Result<bool, String> {
    Err("Windows Hypervisor Platform is a Windows feature".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The command is fixed and needs a restart to finish; nothing here runs it (its UAC prompt is a window).
    #[test]
    fn the_hypervisor_command_is_dism_with_no_restart() {
        assert_eq!(DISM, "System32\\dism.exe");
        assert_eq!(
            DISM_ARGS,
            "/online /enable-feature /featurename:HypervisorPlatform /all /norestart"
        );
        assert_eq!(outcome(3010), Ok(true));
        assert_eq!(outcome(0), Ok(true));
        assert!(outcome(87).unwrap_err().contains("dism 87"));
    }
}
