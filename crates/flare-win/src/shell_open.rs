//! open_url: ShellExecuteW; the caller checks http(s) (Task 17).

use std::ptr;

use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::clipboard::utf16_with_nul;

/// Opens an http or https link in the default browser. Anything else is refused before Windows
/// sees it (a file, a script or an executable never reaches ShellExecuteW from here).
pub fn open_url(url: &str) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("only http and https links open".to_string());
    }
    let verb = utf16_with_nul("open");
    let target = utf16_with_nul(url);
    // SAFETY: both strings are zero-terminated UTF-16 buffers that live across the call; the
    // null window, parameters and directory are all allowed by ShellExecuteW.
    let code = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed with {code}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_refuses_anything_but_http_and_https() {
        for bad in [
            "file:///C:/x",
            "javascript:alert(1)",
            "C:\\x.exe",
            "ftp://x",
            "",
        ] {
            assert!(open_url(bad).is_err(), "{bad:?} must be refused");
        }
    }
}
