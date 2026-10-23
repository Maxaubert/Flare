//! Who a process is: its user SID as a string. A small copy of what `flare-win` does for the
//! single-instance pipe, kept here so this crate depends on nothing of the app (the MCP server and
//! mods' helpers link it). Fold the two together when `flare-win` is next touched.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// The SID string (`S-1-5-21-...`) of the user this process runs as.
pub fn current_user_sid() -> Option<String> {
    // SAFETY: GetCurrentProcess returns a pseudo handle that needs no closing.
    token_sid(unsafe { GetCurrentProcess() })
}

/// The SID string of the user process `pid` runs as, or None when it cannot be read.
pub fn sid_of_pid(pid: u32) -> Option<String> {
    // SAFETY: plain call; the handle (if any) is closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let sid = token_sid(process);
    // SAFETY: the handle came from OpenProcess and is closed once.
    unsafe { CloseHandle(process) };
    sid
}

fn token_sid(process: HANDLE) -> Option<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `token` outlives the call; `process` is a valid process handle.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let result = (|| {
        let mut len: u32 = 0;
        // SAFETY: the size probe: a null buffer of length 0 and a valid `len`.
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len) };
        if len == 0 {
            return None;
        }
        // u64 elements keep the buffer 8-byte aligned for TOKEN_USER.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` is at least `len` bytes; `len` is a valid out-pointer.
        let ok = unsafe {
            GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
        };
        if ok == 0 {
            return None;
        }
        // SAFETY: the call above filled `buf` with a TOKEN_USER.
        let user = unsafe { &*(buf.as_ptr().cast::<TOKEN_USER>()) };
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: the SID is valid for the life of `buf`; `text` receives a LocalAlloc'd string.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 || text.is_null() {
            return None;
        }
        let mut n = 0;
        // SAFETY: `text` is NUL-terminated, as ConvertSidToStringSidW promises.
        while unsafe { *text.add(n) } != 0 {
            n += 1;
        }
        // SAFETY: `n` units were just walked.
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, n) });
        // SAFETY: freeing the string the call allocated, once.
        unsafe { LocalFree(text.cast()) };
        Some(s)
    })();
    // SAFETY: the token came from OpenProcessToken and is closed once.
    unsafe { CloseHandle(token) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_a_sid_and_it_matches_by_pid() {
        let sid = current_user_sid().expect("a SID");
        assert!(sid.starts_with("S-1-"), "{sid}");
        assert_eq!(sid_of_pid(std::process::id()), Some(sid));
    }
}
