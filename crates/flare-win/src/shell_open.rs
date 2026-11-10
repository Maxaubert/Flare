//! open_url: ShellExecuteW; the caller checks http(s) (Task 17). open_app_uri: an app link the person allowed, on its
//! own, with no parameters (rule 252).

#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
use windows_sys::Win32::UI::Shell::ShellExecuteW;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

#[cfg(windows)]
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
    let escaped = escape_web_url(url)?;
    open_escaped(&escaped)
}

#[cfg(windows)]
fn open_escaped(escaped: &str) -> Result<(), String> {
    let verb = utf16_with_nul("open");
    let target = utf16_with_nul(escaped);
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

/// macOS: `/usr/bin/open` hands the (already escaped) web link to the default browser.
#[cfg(not(windows))]
fn open_escaped(escaped: &str) -> Result<(), String> {
    // sync-ok: `open` hands the link over and returns, like ShellExecuteW
    let status = std::process::Command::new("/usr/bin/open")
        .arg(escaped)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status() // sync-ok: `open` returns as soon as the browser has the link
        .map_err(|e| format!("open failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open failed with {status}"))
    }
}

/// An app link as Windows may safely be handed it: the scheme as it is and every byte that could end a quoted argument
/// or start another (a quote, a space, a control byte, a backslash, a bracket, anything non-ASCII) percent-encoded. A
/// handler's command line puts the link in `"%1"`, so a quote in it must never reach Windows raw. Refused: no scheme
/// of two or more letters, digits, `+`, `-` or `.` before the colon.
pub fn escape_app_uri(uri: &str) -> Result<String, String> {
    let (scheme, _) = uri
        .split_once(':')
        .ok_or_else(|| "not an app link".to_string())?;
    let proper = scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !proper {
        return Err("not an app link".to_string());
    }
    let mut out = String::with_capacity(uri.len());
    for &b in uri.as_bytes() {
        let keep = b.is_ascii_graphic() && !b"\"<>\\^`{|}".contains(&b);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(out)
}

/// Opens an app link (`bankid:`, `mailto:`) with the app Windows has for its scheme, after the person said Open in Flare's
/// own question (rule 252). The link alone is handed to `ShellExecuteW` with the verb `open`: no parameters, no working
/// folder, never through `cmd` or any shell of the user's (rule 188). The caller has classified it
/// (`flare_surfaces::web::external_scheme`); a web address is refused here, as `open_url` is its path.
#[cfg(windows)]
pub fn open_app_uri(uri: &str) -> Result<(), String> {
    let lower = uri.to_ascii_lowercase();
    if lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("file:") {
        return Err("not an app link".to_string());
    }
    let escaped = escape_app_uri(uri)?;
    let verb = utf16_with_nul("open");
    let target = utf16_with_nul(&escaped);
    // SAFETY: both strings are zero-terminated UTF-16 buffers that live across the call; the null window, parameters
    // and directory are all allowed by ShellExecuteW.
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

/// [`open_app_uri`] as a worker thread of its own runs it: the shell may hand the link to a COM or DDE handler, so the
/// thread is a single-threaded apartment with DDE off for the call, as `ShellExecute`'s documentation asks. Never on
/// the UI thread (rule 34): the call does not return until the handler has answered.
#[cfg(windows)]
pub fn open_app_uri_on_worker(uri: &str) -> Result<(), String> {
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    // SAFETY: no pointer is passed; the thread is the caller's own worker, so its apartment is set here only.
    let hr = unsafe {
        CoInitializeEx(
            ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    let r = open_app_uri(uri);
    if hr >= 0 {
        // SAFETY: balances the successful CoInitializeEx above on the same thread.
        unsafe { CoUninitialize() };
    }
    r
}

/// Is an app registered for `scheme` (`HKEY_CLASSES_ROOT\<scheme>` with a `URL Protocol` value)? A read of the
/// registry only; a scheme that is not letters, digits, `+`, `-` and `.` is never looked up.
#[cfg(windows)]
pub fn url_handler_registered(scheme: &str) -> bool {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY_CLASSES_ROOT, RRF_RT_ANY, RegGetValueW};
    let proper = !scheme.is_empty()
        && scheme.len() <= 32
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !proper {
        return false;
    }
    let key = utf16_with_nul(scheme);
    let name = utf16_with_nul("URL Protocol");
    // SAFETY: both strings are zero-terminated and outlive the call; no data is asked for (null buffer and size).
    let rc = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_ANY,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    rc == ERROR_SUCCESS
}

/// macOS: no app link opens yet; the Mac web panel (phase 5 of the port plan) brings the real one (port rule 1).
#[cfg(not(windows))]
pub fn open_app_uri_on_worker(_uri: &str) -> Result<(), String> {
    Err("app links open on Windows only in this build".to_string())
}

/// macOS: no handler lookup yet (phase 5 of the port plan), so every scheme reads as having no app.
#[cfg(not(windows))]
pub fn url_handler_registered(_scheme: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_link_cannot_carry_quotes_or_arguments_to_its_app() {
        let evil = "bankid:///?x=\" --flag \"y";
        let e = escape_app_uri(evil).unwrap();
        assert!(!e.contains('"') && !e.contains(' '), "{e}");
        assert_eq!(e, "bankid:///?x=%22%20--flag%20%22y");
        assert_eq!(
            escape_app_uri("bankid:///?autostarttoken=a-1&redirect=null").unwrap(),
            "bankid:///?autostarttoken=a-1&redirect=null"
        );
        assert_eq!(
            escape_app_uri("mailto:\u{e5}@x").unwrap(),
            "mailto:%C3%A5@x"
        );
        assert_eq!(escape_app_uri("tel:1\\2").unwrap(), "tel:1%5C2");
        for bad in ["", "nocolon", "C:\\x.exe", ":x", "1a:x"] {
            assert!(escape_app_uri(bad).is_err(), "{bad:?}");
        }
    }

    #[cfg(windows)] // the Mac twin comes with the web panel (port phase 5)
    #[test]
    fn open_app_uri_refuses_a_web_address_a_file_and_a_path_before_windows_sees_it() {
        for bad in [
            "https://a.example/",
            "HTTP://a.example/",
            "file:///C:/x.exe",
            "C:\\x.exe",
            "",
        ] {
            assert!(open_app_uri(bad).is_err(), "{bad:?} must be refused");
            assert!(
                open_app_uri_on_worker(bad).is_err(),
                "{bad:?} must be refused on a worker too"
            );
        }
    }

    #[cfg(windows)] // the Mac twin comes with the web panel (port phase 5)
    #[test]
    fn a_scheme_that_is_not_one_is_never_looked_up() {
        for bad in ["", "a\\b", "x y", "..\\..\\x", &"a".repeat(40)] {
            assert!(!url_handler_registered(bad), "{bad:?}");
        }
        // `http` has a handler on every Windows; reading it changes nothing.
        assert!(url_handler_registered("http"));
    }

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
