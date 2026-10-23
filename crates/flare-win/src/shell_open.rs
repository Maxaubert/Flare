//! open_url: ShellExecuteW; the caller checks http(s) (Task 17).

use std::ptr;

use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::clipboard::utf16_with_nul;

/// The link as the browser's command line may safely carry it: a host after the scheme, and every
/// byte that could end a quoted argument or start another (a quote, a space, a control byte, a
/// backslash, a bracket, anything non-ASCII) percent-encoded, as Electron's openExternal does.
/// An OSC 8 uri comes from untrusted output, so it is never passed raw.
pub fn escape_web_url(url: &str) -> Result<String, String> {
    let rest = url
        .split_once("://")
        .map(|(_, r)| r)
        .ok_or_else(|| "not a web link".to_string())?;
    if rest.is_empty() || rest.starts_with(['/', '?', '#']) {
        return Err("a web link needs a host".to_string());
    }
    let mut out = String::with_capacity(url.len());
    for &b in url.as_bytes() {
        let keep = b.is_ascii_graphic() && !b"\"<>\\^`{|}".contains(&b);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(out)
}

/// Opens an http or https link in the default browser. Anything else is refused before Windows
/// sees it (a file, a script or an executable never reaches ShellExecuteW from here).
pub fn open_url(url: &str) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("only http and https links open".to_string());
    }
    let verb = utf16_with_nul("open");
    let escaped = escape_web_url(url)?;
    let target = utf16_with_nul(&escaped);
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

    #[test]
    fn an_osc8_uri_cannot_carry_quotes_or_arguments_to_the_browser() {
        let evil = "https://x.example/\" --some-flag \"https://y";
        let e = escape_web_url(evil).unwrap();
        assert!(!e.contains('"') && !e.contains(' '), "{e}");
        assert_eq!(e, "https://x.example/%22%20--some-flag%20%22https://y");
        assert_eq!(
            escape_web_url("https://a.example/p?q=1&r=%20#f").unwrap(),
            "https://a.example/p?q=1&r=%20#f"
        );
        assert_eq!(
            escape_web_url("https://a.example/\u{e5}").unwrap(),
            "https://a.example/%C3%A5"
        );
        assert!(escape_web_url("https:///nohost").is_err());
    }
}
