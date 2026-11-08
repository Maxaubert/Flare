//! Chromium's numbers in WebView2's words: the portable arithmetic the macOS host needs to report what the Windows
//! host reports. Plain functions, so they are tested on every platform.

/// WebView2's `COREWEBVIEW2_WEB_ERROR_STATUS` for a Chromium net error (`net::Error`, negative), the number
/// `WebEvent::Loaded` carries in `status` on Windows, so `flare_surfaces::web::failed_load_notice` reads a CEF load
/// the way it reads a WebView2 one. 0 (unknown) for success and for an error WebView2 has no word for.
pub fn web_error_status(net_error: i32) -> i32 {
    match net_error {
        0 => 0,
        // ERR_CERT_COMMON_NAME_INVALID, ERR_CERT_DATE_INVALID, ERR_BAD_SSL_CLIENT_AUTH_CERT, ERR_CERT_REVOKED.
        -200 => 1,
        -201 => 2,
        -117 => 3,
        -206 => 4,
        // Every other certificate error: CERTIFICATE_IS_INVALID.
        -299..=-202 => 5,
        // ERR_ADDRESS_UNREACHABLE: SERVER_UNREACHABLE.
        -109 => 6,
        // ERR_TIMED_OUT, ERR_CONNECTION_TIMED_OUT: TIMEOUT.
        -7 | -118 => 7,
        // ERR_INVALID_RESPONSE, ERR_EMPTY_RESPONSE, ERR_INVALID_HTTP_RESPONSE: ERROR_HTTP_INVALID_SERVER_RESPONSE.
        -320 | -324 | -370 => 8,
        // ERR_CONNECTION_ABORTED, ERR_CONNECTION_RESET, ERR_INTERNET_DISCONNECTED.
        -103 => 9,
        -101 => 10,
        -106 => 11,
        // ERR_CONNECTION_REFUSED, ERR_CONNECTION_FAILED, ERR_PROXY_CONNECTION_FAILED: CANNOT_CONNECT.
        -102 | -104 | -130 => 12,
        // ERR_NAME_NOT_RESOLVED, ERR_NAME_RESOLUTION_FAILED: HOST_NAME_NOT_RESOLVED.
        -105 | -137 => 13,
        // ERR_ABORTED (a cancelled or replaced navigation): OPERATION_CANCELED.
        -3 => 14,
        // ERR_TOO_MANY_REDIRECTS, ERR_UNSAFE_REDIRECT: REDIRECT_FAILED.
        -310 | -311 => 15,
        // ERR_FAILED, ERR_UNEXPECTED: UNEXPECTED_ERROR.
        -2 | -9 => 16,
        _ => 0,
    }
}

/// Chromium's zoom level for a zoom factor: each level is a factor of 1.2 (`blink::ZoomFactorToZoomLevel`), so 1.0
/// is level 0, 1.2 is level 1 and 0.5 is about -3.8. A factor that is not a positive number is level 0.
pub fn zoom_level(factor: f64) -> f64 {
    if factor.is_finite() && factor > 0.0 {
        factor.ln() / 1.2f64.ln()
    } else {
        0.0
    }
}

/// The CEF event flags (`cef_event_flags_t`) a key or a pointer event carries.
pub mod flags {
    pub const SHIFT: u32 = 1 << 1;
    pub const CONTROL: u32 = 1 << 2;
    pub const ALT: u32 = 1 << 3;
    pub const LEFT_BUTTON: u32 = 1 << 4;
    pub const MIDDLE_BUTTON: u32 = 1 << 5;
    pub const RIGHT_BUTTON: u32 = 1 << 6;
    pub const COMMAND: u32 = 1 << 7;
    pub const IS_REPEAT: u32 = 1 << 13;
}

/// CEF's view size in CSS pixels for a rectangle of `w` by `h` device pixels at `scale`: never below 1 by 1, and
/// rounded, so a 1966 by 1400 rectangle at 2x is 983 by 700.
pub fn css_size(w: i32, h: i32, scale: f64) -> (i32, i32) {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let c = |v: i32| ((f64::from(v.max(1)) / s).round() as i32).max(1);
    (c(w), c(h))
}

/// A point of the view in device pixels as CEF's CSS pixels at `scale`.
pub fn css_point(x: f64, y: f64, scale: f64) -> (i32, i32) {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    ((x / s).floor() as i32, (y / s).floor() as i32)
}

/// The file a held download is saved as once the person said yes: `name` (only its last part, never a path, never
/// a dot file or empty) in `folder`, with ` (2)`, ` (3)` and so on before the extension while a file of that name is
/// there. `taken` says whether a path is taken (the disk, in the host).
pub fn download_target(
    folder: &std::path::Path,
    name: &str,
    taken: impl Fn(&std::path::Path) -> bool,
) -> std::path::PathBuf {
    let last = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('.');
    let clean: String = last.chars().filter(|c| !c.is_control()).collect();
    let clean = if clean.is_empty() {
        "download".to_string()
    } else {
        clean
    };
    let (stem, ext) = match clean.rfind('.') {
        Some(i) if i > 0 => (&clean[..i], &clean[i..]),
        _ => (clean.as_str(), ""),
    };
    let first = folder.join(&clean);
    if !taken(&first) {
        return first;
    }
    (2..10_000)
        .map(|n| folder.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .unwrap_or(first)
}

/// DevTools' key modifiers (`Input.dispatchKeyEvent`'s `modifiers`): Alt 1, Ctrl 2, Meta (Cmd) 4, Shift 8.
pub mod cdp_mods {
    pub const ALT: u64 = 1;
    pub const CTRL: u64 = 2;
    pub const META: u64 = 4;
    pub const SHIFT: u64 = 8;
}

/// The editing command Chromium runs on a Mac for an agent's chord, which on Windows the renderer runs from the key
/// itself. On a Mac the text system turns a chord into a command (`selectAll:`) before the page's renderer sees it,
/// and a DevTools key event skips that step: `Control+a` (the chord the tool's text teaches) and `Meta+a` would select
/// nothing. Both the Windows chord (Control) and the Mac one (Cmd) name the same command, so an agent's `Control+a`
/// does on a Mac what it does on Windows: select all, undo (`z`), redo (`Shift+z`, `y`), copy, cut, paste; with
/// Control, the arrows move by word, Backspace and Delete take a word, Home and End go to the document's ends (the
/// Windows meanings; Shift extends the selection). A chord with Alt, or both Control and Meta, is the page's alone.
pub fn mac_edit_command(key: &str, modifiers: u64) -> Option<String> {
    use cdp_mods::{ALT, CTRL, META, SHIFT};
    let ctrl = modifiers & CTRL != 0;
    let meta = modifiers & META != 0;
    if modifiers & ALT != 0 || ctrl == meta {
        return None;
    }
    let shift = modifiers & SHIFT != 0;
    let pick = |base: &str| {
        if shift {
            format!("{base}AndModifySelection")
        } else {
            base.to_string()
        }
    };
    let lower = key.to_ascii_lowercase();
    let cmd = match lower.as_str() {
        "a" if !shift => "selectAll".to_string(),
        "z" if shift => "redo".to_string(),
        "z" => "undo".to_string(),
        "y" if !shift => "redo".to_string(),
        "c" if !shift => "copy".to_string(),
        "x" if !shift => "cut".to_string(),
        "v" if !shift => "paste".to_string(),
        // Moves and deletions by word or to the document's ends: the Control meanings of Windows only (with Cmd these
        // keys mean line ends on a Mac, which Chromium's own handling already gives).
        "arrowleft" if ctrl => pick("moveWordLeft"),
        "arrowright" if ctrl => pick("moveWordRight"),
        "home" if ctrl => pick("moveToBeginningOfDocument"),
        "end" if ctrl => pick("moveToEndOfDocument"),
        "backspace" if ctrl && !shift => "deleteWordBackward".to_string(),
        "delete" if ctrl && !shift => "deleteWordForward".to_string(),
        _ => return None,
    };
    Some(cmd)
}

/// A DevTools call as it goes to CEF on a Mac: an `Input.dispatchKeyEvent` key down of an editing chord carries its
/// command ([`mac_edit_command`]), the way Chromium's own Mac input does; everything else goes as it came. A call
/// that names its own `commands` keeps them.
pub fn mac_devtools_params(method: &str, params: &serde_json::Value) -> serde_json::Value {
    let mut p = params.clone();
    if method != "Input.dispatchKeyEvent" || p.get("commands").is_some() {
        return p;
    }
    let down = matches!(p["type"].as_str(), Some("keyDown" | "rawKeyDown"));
    let key = p["key"].as_str().unwrap_or_default().to_string();
    let modifiers = p["modifiers"].as_u64().unwrap_or(0);
    if down && let Some(cmd) = mac_edit_command(&key, modifiers) {
        p["commands"] = serde_json::json!([cmd]);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_chromium_error_reads_as_the_webview2_status_the_failure_notice_knows() {
        assert_eq!(web_error_status(0), 0);
        assert_eq!(web_error_status(-105), 13, "name not resolved");
        assert_eq!(web_error_status(-102), 12, "refused");
        assert_eq!(web_error_status(-118), 7, "timed out");
        assert_eq!(web_error_status(-3), 14, "cancelled");
        assert_eq!(web_error_status(-202), 5, "a certificate");
        assert_eq!(web_error_status(-12345), 0);
        // The notice is shown for the ones that reached nothing, and not for a cancelled navigation.
        let notice = |e| flare_surfaces_like_notice(web_error_status(e));
        assert!(notice(-105) && notice(-102) && notice(-7) && notice(-101));
        assert!(!notice(-3) && !notice(0));
    }

    /// `flare_surfaces::web::failed_load_notice`'s "reached nothing" set.
    fn flare_surfaces_like_notice(status: i32) -> bool {
        matches!(status, 1..=8 | 10..=13 | 15 | 16)
    }

    #[test]
    fn a_zoom_factor_is_chromiums_level() {
        assert_eq!(zoom_level(1.0), 0.0);
        assert!((zoom_level(1.2) - 1.0).abs() < 1e-9);
        assert!((zoom_level(1.44) - 2.0).abs() < 1e-9);
        assert!(zoom_level(0.5) < -3.7 && zoom_level(0.5) > -3.9);
        assert_eq!(zoom_level(0.0), 0.0);
        assert_eq!(zoom_level(f64::NAN), 0.0);
    }

    #[test]
    fn device_pixels_become_css_pixels_and_never_nothing() {
        assert_eq!(css_size(1966, 1400, 2.0), (983, 700));
        assert_eq!(css_size(0, -5, 2.0), (1, 1));
        assert_eq!(css_size(100, 50, 0.0), (100, 50));
        assert_eq!(css_point(21.0, 9.0, 2.0), (10, 4));
    }

    #[test]
    fn a_download_is_named_by_its_last_part_and_never_overwrites() {
        let dir = Path::new("/d");
        let none = |_: &Path| false;
        assert_eq!(download_target(dir, "a.txt", none), dir.join("a.txt"));
        assert_eq!(
            download_target(dir, "../../etc/passwd", none),
            dir.join("passwd")
        );
        assert_eq!(download_target(dir, ".bashrc", none), dir.join("bashrc"));
        assert_eq!(download_target(dir, "", none), dir.join("download"));
        assert_eq!(download_target(dir, "a\u{7}b", none), dir.join("ab"));
        let taken = |p: &Path| p == dir.join("a.txt") || p == dir.join("a (2).txt");
        assert_eq!(download_target(dir, "a.txt", taken), dir.join("a (3).txt"));
        let taken = |p: &Path| p == dir.join("notes");
        assert_eq!(download_target(dir, "notes", taken), dir.join("notes (2)"));
    }

    #[test]
    fn an_agents_editing_chord_is_chromiums_command_on_a_mac_with_control_or_cmd() {
        use cdp_mods::{ALT, CTRL, META, SHIFT};
        for m in [CTRL, META] {
            assert_eq!(mac_edit_command("a", m).as_deref(), Some("selectAll"));
            assert_eq!(mac_edit_command("A", m).as_deref(), Some("selectAll"));
            assert_eq!(mac_edit_command("z", m).as_deref(), Some("undo"));
            assert_eq!(mac_edit_command("Z", m | SHIFT).as_deref(), Some("redo"));
            assert_eq!(mac_edit_command("y", m).as_deref(), Some("redo"));
            assert_eq!(mac_edit_command("c", m).as_deref(), Some("copy"));
            assert_eq!(mac_edit_command("x", m).as_deref(), Some("cut"));
            assert_eq!(mac_edit_command("v", m).as_deref(), Some("paste"));
        }
        // Windows' Control moves and deletions; Cmd's own meanings are left to Chromium.
        assert_eq!(
            mac_edit_command("ArrowLeft", CTRL).as_deref(),
            Some("moveWordLeft")
        );
        assert_eq!(
            mac_edit_command("ArrowRight", CTRL | SHIFT).as_deref(),
            Some("moveWordRightAndModifySelection")
        );
        assert_eq!(
            mac_edit_command("Home", CTRL).as_deref(),
            Some("moveToBeginningOfDocument")
        );
        assert_eq!(
            mac_edit_command("End", CTRL | SHIFT).as_deref(),
            Some("moveToEndOfDocumentAndModifySelection")
        );
        assert_eq!(
            mac_edit_command("Backspace", CTRL).as_deref(),
            Some("deleteWordBackward")
        );
        assert_eq!(
            mac_edit_command("Delete", CTRL).as_deref(),
            Some("deleteWordForward")
        );
        assert_eq!(mac_edit_command("ArrowLeft", META), None);
        // Plain keys, Alt chords, both modifiers, and keys with no editing meaning are the page's.
        assert_eq!(mac_edit_command("a", 0), None);
        assert_eq!(mac_edit_command("Backspace", 0), None);
        assert_eq!(mac_edit_command("a", CTRL | ALT), None);
        assert_eq!(mac_edit_command("a", CTRL | META), None);
        assert_eq!(mac_edit_command("t", META), None);
        assert_eq!(mac_edit_command("a", CTRL | SHIFT), None);
    }

    #[test]
    fn only_a_key_down_of_an_editing_chord_gets_a_command() {
        use serde_json::json;
        let down = json!({"type": "rawKeyDown", "key": "a", "code": "KeyA", "modifiers": 2});
        assert_eq!(
            mac_devtools_params("Input.dispatchKeyEvent", &down)["commands"],
            json!(["selectAll"])
        );
        let up = json!({"type": "keyUp", "key": "a", "modifiers": 2});
        assert_eq!(mac_devtools_params("Input.dispatchKeyEvent", &up), up);
        let plain = json!({"type": "keyDown", "key": "a", "text": "a", "modifiers": 0});
        assert_eq!(mac_devtools_params("Input.dispatchKeyEvent", &plain), plain);
        let own = json!({"type": "rawKeyDown", "key": "a", "modifiers": 4, "commands": []});
        assert_eq!(mac_devtools_params("Input.dispatchKeyEvent", &own), own);
        assert_eq!(mac_devtools_params("Runtime.evaluate", &down), down);
    }
}
