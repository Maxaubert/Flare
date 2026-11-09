//! The openers on macOS: `NSWorkspace`, the Mac counterpart of `ShellExecuteW` and `explorer.exe`.
//!
//! - A web link opens in the default browser (`openURL:`), only when `NSURL` itself reads it as
//!   http or https with a host. The caller (`shell_open::open_url`) has already refused every
//!   other scheme and percent-encoded the link.
//! - A file opens in its default app after `openers` checked it, and only when that app is no
//!   runner: a Mac file can carry its own "Open with" app, so a `notes.txt` can be set to open with
//!   Terminal, which runs it. The handler is looked up (`URLForApplicationToOpenURL:`), refused when
//!   it is a terminal, a script runner or unknown, and the file is then opened WITH that app
//!   (`openURLs:withApplicationAtURL:`), so nothing can swap it in between. A file with an execute
//!   bit never opens: a plain document does not need one (regression rule 240).
//! - A folder opens in a Finder window (`selectFile:inFileViewerRootedAtPath:` with no file):
//!   Finder only ever SHOWS what it is handed there. `openURL:` is never used for a folder, since
//!   a bundle (`.app`) is a folder and `openURL:` would launch it.
//! - Reveal selects the item in a Finder window (`activateFileViewerSelectingURLs:`).
//!
//! `NSWorkspace` may be used from any thread; the callers are the open workers. Nothing here is run
//! by the tests (each call would bring Finder or a browser forward): they cover the pure checks.

use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSBundle, NSString, NSURL};

/// The apps that run what they open (bundle ids, lower case): terminals, script editors and
/// launchers. A file whose handler is one of these never opens from Flare.
const RUNNERS: &[&str] = &[
    "com.apple.terminal",
    "com.apple.scripteditor2",
    "com.apple.automator",
    "com.apple.automator.runner",
    "com.apple.installer",
    "com.googlecode.iterm2",
    "org.python.pythonlauncher",
    "org.alacritty",
    "net.kovidgoyal.kitty",
    "com.mitchellh.ghostty",
    "com.github.wez.wezterm",
    "co.zeit.hyper",
    // Indirect runners: Finder follows .fileloc and alias files to their target (an app or a
    // script), the Jar Launcher runs Java, Shortcuts imports and runs workflows, and Settings
    // installs .prefPane bundles.
    "com.apple.finder",
    "com.apple.jarlauncher",
    "com.apple.shortcuts",
    "com.apple.systempreferences",
    "com.apple.systemsettings",
    "com.apple.archiveutility",
];

/// File types that run, install or point elsewhere whatever app opens them (lower case, no dot).
/// Refused before the handler is even looked up, so a renamed or unknown handler cannot let them
/// through: location and alias files, Terminal and shell launchers, Java, AppleScript, Automator
/// and Shortcuts workflows, installers and loadable bundles.
const RUNNABLE_TYPES: &[&str] = &[
    "fileloc",
    "inetloc",
    "webloc",
    "afploc",
    "ftploc",
    "alias",
    "terminal",
    "command",
    "tool",
    "sh",
    "zsh",
    "bash",
    "csh",
    "tcsh",
    "fish",
    "py",
    "pl",
    "rb",
    "jar",
    "workflow",
    "action",
    "scpt",
    "scptd",
    "applescript",
    "shortcut",
    "pkg",
    "mpkg",
    "app",
    "prefpane",
    "saver",
    "qlgenerator",
    "kext",
    "plugin",
    "bundle",
    "dylib",
    "osax",
    "mobileconfig",
    "dmg",
    "iso",
];

/// Pure: whether a file's name ends in a type that runs, installs or redirects (`RUNNABLE_TYPES`).
pub fn runnable_type(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => {
            RUNNABLE_TYPES.contains(&ext.to_ascii_lowercase().as_str())
        }
        _ => false,
    }
}

/// Bundle id prefixes of runners that ship under several ids (Warp's channels).
const RUNNER_PREFIXES: &[&str] = &["dev.warp."];

/// Pure: whether the app with this bundle id may open a file Flare hands it. An app without a
/// bundle id is unknown and refused.
pub fn handler_ok(bundle_id: Option<&str>) -> bool {
    let Some(id) = bundle_id.map(str::to_ascii_lowercase) else {
        return false;
    };
    !id.is_empty()
        && !RUNNERS.contains(&id.as_str())
        && !RUNNER_PREFIXES.iter().any(|p| id.starts_with(p))
}

/// Whether the file carries a per-file "Open with" choice (`com.apple.LaunchServices.OpenWith`).
/// Such a file could name any app, runner or not, so Flare only opens files that follow their
/// type's default app; the denylist above is the second wall, not the only one.
pub fn has_open_with_override(path: &str) -> bool {
    let (Ok(p), Ok(name)) = (
        std::ffi::CString::new(path),
        std::ffi::CString::new("com.apple.LaunchServices.OpenWith"),
    ) else {
        return true;
    };
    // SAFETY: both are valid NUL-terminated strings; a null buffer with size 0 only asks the size.
    let n = unsafe {
        libc::getxattr(
            p.as_ptr(),
            name.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if n >= 0 {
        return true;
    }
    // ENOATTR means no override; any other error (unreadable file) is refused too.
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOATTR)
}

/// Pure: whether a file mode has any execute bit (user, group or other).
pub fn has_exec_bit(mode: u32) -> bool {
    mode & 0o111 != 0
}

/// The app this Mac would open `url` with, when it is one that may (`handler_ok`).
fn safe_handler(url: &NSURL) -> Result<Retained<NSURL>, String> {
    let app = NSWorkspace::sharedWorkspace()
        .URLForApplicationToOpenURL(url)
        .ok_or_else(|| "no app opens this file".to_string())?;
    let id = NSBundle::bundleWithURL(&app)
        .and_then(|b| b.bundleIdentifier())
        .map(|s| s.to_string());
    if handler_ok(id.as_deref()) {
        Ok(app)
    } else {
        Err(format!(
            "this file opens with {}, which could run it",
            id.as_deref().unwrap_or("an unknown app")
        ))
    }
}

/// The bundle id of the app this Mac would open `abs` with, when it may open it; the refusal
/// otherwise. Asks LaunchServices only: nothing opens.
pub fn handler_of(abs: &str) -> Result<String, String> {
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&*checked(abs)?);
        let app = safe_handler(&url)?;
        Ok(NSBundle::bundleWithURL(&app)
            .and_then(|b| b.bundleIdentifier())
            .map(|s| s.to_string())
            .unwrap_or_default())
    })
}

/// The web link as `NSURL` reads it, when that is http or https with a host. `escaped` is the
/// already percent-encoded link; anything `NSURL` reads differently is refused.
pub fn web_url(escaped: &str) -> Result<Retained<NSURL>, String> {
    let url = NSURL::URLWithString(&NSString::from_str(escaped))
        .ok_or_else(|| "not a link the system can read".to_string())?;
    let scheme = url.scheme().map(|s| s.to_string().to_ascii_lowercase());
    if !matches!(scheme.as_deref(), Some("http" | "https")) {
        return Err("only http and https links open".to_string());
    }
    if url.host().is_none_or(|h| h.to_string().is_empty()) {
        return Err("a web link needs a host".to_string());
    }
    Ok(url)
}

/// Opens a checked web link in the default browser.
pub fn open_web_url(escaped: &str) -> Result<(), String> {
    autoreleasepool(|_| {
        let url = web_url(escaped)?;
        if NSWorkspace::sharedWorkspace().openURL(&url) {
            Ok(())
        } else {
            Err("no app took the link".to_string())
        }
    })
}

/// Pure: whether `abs` may be handed to Finder as a path: absolute, and no control character
/// (0x00 to 0x1F, 0x7F, 0x80 to 0x9F). A quote is an ordinary file name character on the Mac and
/// no command line is built, so it is allowed.
pub fn finder_path_ok(abs: &str) -> bool {
    abs.starts_with('/')
        && !abs
            .chars()
            .any(|c| (c as u32) < 0x20 || c == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&c))
}

fn checked(abs: &str) -> Result<Retained<NSString>, String> {
    if finder_path_ok(abs) {
        Ok(NSString::from_str(abs))
    } else {
        Err("path refused".to_string())
    }
}

/// Opens a file (already canonical, allowlisted and not a folder) in its default app, when that app
/// is no runner and the file has no execute bit.
pub fn open_file(canon: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    // sync-ok: one stat on the open worker (the callers run off the UI thread)
    let mode = std::fs::metadata(canon)
        .map_err(|e| format!("cannot read the file: {e}"))?
        .permissions()
        .mode();
    if has_exec_bit(mode) {
        return Err("an executable file never opens from Flare".to_string());
    }
    if has_open_with_override(canon) {
        return Err(
            "this file is set to open with its own app, so Flare does not open it".to_string(),
        );
    }
    if runnable_type(canon) {
        return Err(
            "this kind of file runs or installs something, so Flare does not open it".to_string(),
        );
    }
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&*checked(canon)?);
        let app = safe_handler(&url)?;
        let list = NSArray::from_retained_slice(&[url]);
        let config = NSWorkspaceOpenConfiguration::configuration();
        // Opened with the very app that was checked; the result is the app's to report.
        NSWorkspace::sharedWorkspace()
            .openURLs_withApplicationAtURL_configuration_completionHandler(
                &list, &app, &config, None,
            );
        Ok(())
    })
}

/// Shows a folder in a Finder window. Finder never runs what it is handed here.
pub fn show_folder(abs: &str) -> Result<(), String> {
    autoreleasepool(|_| {
        let root = checked(abs)?;
        if NSWorkspace::sharedWorkspace().selectFile_inFileViewerRootedAtPath(None, &root) {
            Ok(())
        } else {
            Err("Finder did not open the folder".to_string())
        }
    })
}

/// Shows an item selected in a Finder window.
pub fn reveal(abs: &str) -> Result<(), String> {
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&*checked(abs)?);
        let list = NSArray::from_retained_slice(&[url]);
        NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&list);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_with_its_own_open_with_choice_is_refused_and_a_plain_one_is_not() {
        let dir = std::env::temp_dir().join(format!("flare-openwith-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("notes.txt");
        std::fs::write(&f, "hi").unwrap();
        let path = f.to_str().unwrap();
        assert!(!has_open_with_override(path));
        let st = std::process::Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.LaunchServices.OpenWith", "x", path])
            .status()
            .unwrap();
        assert!(st.success());
        assert!(has_open_with_override(path));
        assert!(has_open_with_override(
            dir.join("missing.txt").to_str().unwrap()
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runnable_and_redirecting_types_are_refused_before_any_handler() {
        for bad in [
            "/u/a.fileloc",
            "/u/a.webloc",
            "/u/x.TERMINAL",
            "/u/run.command",
            "/u/a.jar",
            "/u/s.scpt",
            "/u/w.workflow",
            "/u/i.pkg",
            "/u/p.prefPane",
            "/u/a.shortcut",
            "/u/x.sh",
            "/u/m.mobileconfig",
        ] {
            assert!(runnable_type(bad), "{bad}");
        }
        for ok in [
            "/u/notes.txt",
            "/u/a.png",
            "/u/r.md",
            "/u/.fileloc",
            "/u/dir.d/plain",
            "/u/x.pdf",
        ] {
            assert!(!runnable_type(ok), "{ok}");
        }
        for runner in [
            "com.apple.finder",
            "com.apple.JarLauncher",
            "com.apple.shortcuts",
        ] {
            assert!(!handler_ok(Some(runner)), "{runner}");
        }
    }

    #[test]
    fn only_http_and_https_with_a_host_pass() {
        assert!(web_url("https://example.com/a%20b?q=1#f").is_ok());
        assert!(web_url("HTTP://example.com").is_ok());
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "x-apple.systempreferences:com.apple.preference",
            "https:///nohost",
            "ssh://host",
            "",
        ] {
            assert!(web_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn terminals_script_runners_and_unknown_apps_never_open_a_file() {
        for bad in [
            None,
            Some(""),
            Some("com.apple.Terminal"),
            Some("com.googlecode.iTerm2"),
            Some("com.apple.ScriptEditor2"),
            Some("org.python.PythonLauncher"),
            Some("dev.warp.Warp-Stable"),
        ] {
            assert!(!handler_ok(bad), "{bad:?}");
        }
        for good in [
            "com.apple.TextEdit",
            "com.apple.Preview",
            "com.microsoft.VSCode",
        ] {
            assert!(handler_ok(Some(good)), "{good}");
        }
        assert!(has_exec_bit(0o100755));
        assert!(has_exec_bit(0o100601));
        assert!(!has_exec_bit(0o100644));
    }

    #[test]
    fn the_handler_lookup_refuses_a_terminal_and_opens_nothing() {
        // LaunchServices is only asked; no app starts.
        let dir = tempfile::tempdir().expect("temp dir");
        let script = dir.path().join("x.command");
        std::fs::write(&script, "echo hi\n").expect("write");
        let r = handler_of(script.to_str().expect("utf-8"));
        assert!(r.is_err(), "a .command file opens with Terminal: {r:?}");
        // An executable text file is refused before any app is asked.
        use std::os::unix::fs::PermissionsExt;
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "hi").expect("write");
        std::fs::set_permissions(&text, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let e = open_file(text.to_str().expect("utf-8")).expect_err("refused");
        assert!(e.contains("executable"), "{e}");
        assert!(handler_of("relative").is_err());
    }

    #[test]
    fn a_finder_path_is_absolute_and_has_no_control_character() {
        assert!(finder_path_ok("/Users/me/a \"b\" c"));
        assert!(finder_path_ok("/"));
        for bad in [
            "relative/x",
            "",
            "/a\nb",
            "/a\u{1b}",
            "/a\u{9b}",
            "/a\u{7f}",
        ] {
            assert!(!finder_path_ok(bad), "{bad:?}");
        }
    }
}
