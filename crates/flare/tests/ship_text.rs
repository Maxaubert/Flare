//! M9 Tasks 26 and 35: the shipped text checks (the icon, the notes, the neutrality scan).
//!
//! The resource text comes from `src/rc_text.rs`, included here by path: the library and binary
//! module lists belong to the orchestrator, and the function is pure text.

#[path = "../src/rc_text.rs"]
mod rc_text;

use std::fs;
use std::path::PathBuf;

use rc_text::rc_text;

#[cfg(windows)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn the_icon_is_resource_one() {
    let text = rc_text("0.1.0", r"C:\x\icon.ico");
    assert!(text.starts_with("1 ICON"), "resources start with: {text:?}");
}

#[test]
fn the_version_numbers_come_from_the_argument() {
    let text = rc_text("0.1.0", r"C:\x\icon.ico");
    assert!(text.contains("FILEVERSION 0,1,0,0"));
    assert!(text.contains("PRODUCTVERSION 0,1,0,0"));
    assert!(text.contains(r#""FileVersion", "0.1.0.0""#));
}

#[test]
fn a_prerelease_version_keeps_its_text() {
    let text = rc_text("0.2.0-beta.1", r"C:\x\icon.ico");
    assert!(text.contains("FILEVERSION 0,2,0,0"));
    assert!(text.contains(r#""ProductVersion", "0.2.0-beta.1""#));
}

#[test]
fn backslashes_in_the_icon_path_are_doubled() {
    let text = rc_text("0.1.0", r"C:\a\b\icon.ico");
    assert!(
        text.contains(r#"1 ICON "C:\\a\\b\\icon.ico""#),
        "icon line: {text:?}"
    );
}

#[test]
fn no_prism_and_no_em_dash_in_the_resource() {
    let text = rc_text("0.1.0", r"C:\x\icon.ico");
    assert!(!text.to_lowercase().contains("prism"));
    assert!(!text.contains('\u{2014}'));
}

/// Rule 1182: the app icon has its own sources, one per platform (2026-11-12). Windows' .ico (the exe, the
/// installer and the taskbar) is rendered from assets/icon/app-windows.svg; the Mac's is the Icon Composer icon
/// assets/icon/app-macos.icon, compiled by actool (flare-cef's macos::release). The in-app mark,
/// assets/icon/flare-color.svg, is neither and stays.
#[test]
fn each_platform_app_icon_has_its_own_source_and_the_in_app_mark_stays() {
    let icon = root().join("assets").join("icon");
    for f in [
        "app-windows.svg",
        "app-macos.icon/icon.json",
        "app-macos.icon/Assets/flame.svg",
        "app-macos.icon/Assets/golden-flame.svg",
        "flare-color.svg",
    ] {
        assert!(icon.join(f).exists(), "assets/icon/{f} is missing");
    }
    let make = fs::read_to_string(root().join("tools").join("make-icon.py")).expect("make-icon.py");
    assert!(make.contains("SVG = ROOT / 'assets' / 'icon' / 'app-windows.svg'"));
    let mac = fs::read_to_string(root().join("crates/flare-cef/src/macos/release.rs"))
        .expect("release.rs");
    assert!(mac.contains("pub const APP_ICON_SOURCE: &str = \"assets/icon/app-macos.icon\";"));
    let bundle =
        fs::read_to_string(root().join("crates/flare-cef/src/macos/bundle.rs")).expect("bundle.rs");
    assert!(bundle.contains("\"CFBundleIconName\""));
    assert!(bundle.contains("\"CFBundleIconFile\""));
}

#[test]
fn the_ico_has_19_frames_in_pt_order() {
    let data = fs::read(root().join("build").join("icon.ico")).expect("build/icon.ico exists");
    let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]);
    assert_eq!(u16_at(0), 0, "reserved");
    assert_eq!(u16_at(2), 1, "type is icon");
    assert_eq!(u16_at(4), 19, "frame count");
    let widths: Vec<u32> = (0..19)
        .map(|i| match data[6 + 16 * i] {
            0 => 256,
            w => u32::from(w),
        })
        .collect();
    assert_eq!(
        widths,
        [
            16, 20, 24, 28, 30, 32, 36, 40, 42, 48, 54, 56, 60, 64, 72, 80, 96, 128, 256
        ]
    );
}

/// The file version and product name of the built exe, read from its resource.
#[cfg(windows)]
fn exe_strings() -> (String, String) {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    let exe = env!("CARGO_BIN_EXE_flare");
    let wide: Vec<u16> = std::ffi::OsStr::new(exe)
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that lives for every call below; the
    // buffer is sized by GetFileVersionInfoSizeW and handed back to the same API family.
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle);
        assert!(size > 0, "the exe has no version resource");
        let mut buf = vec![0u8; size as usize];
        let ok = GetFileVersionInfoW(wide.as_ptr(), 0, size, buf.as_mut_ptr().cast());
        assert!(ok != 0, "GetFileVersionInfoW failed");
        let query = |name: &str| -> String {
            let key: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let mut value: *mut std::ffi::c_void = null_mut();
            let mut len = 0u32;
            let found = VerQueryValueW(buf.as_ptr().cast(), key.as_ptr(), &mut value, &mut len);
            assert!(found != 0, "no {name} in the version resource");
            // SAFETY: VerQueryValueW returned a pointer to `len` UTF-16 units inside `buf`,
            // which stays alive until the end of this closure.
            let units = std::slice::from_raw_parts(value.cast::<u16>(), len as usize);
            String::from_utf16_lossy(units)
                .trim_end_matches('\0')
                .to_string()
        };
        (
            query(r"\StringFileInfo\040904b0\FileVersion"),
            query(r"\StringFileInfo\040904b0\ProductName"),
        )
    }
}

#[cfg(windows)]
#[test]
fn the_built_exe_has_the_icon_and_the_version() {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::UI::Shell::ExtractIconExW;

    let exe = env!("CARGO_BIN_EXE_flare");
    let wide: Vec<u16> = std::ffi::OsStr::new(exe)
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: a NUL-terminated path; index -1 with null icon pointers only counts the icons.
    let count = unsafe { ExtractIconExW(wide.as_ptr(), -1, null_mut(), null_mut(), 0) };
    assert!(count >= 1, "the exe carries no icon: {count}");
    let (_, product) = exe_strings();
    assert_eq!(product, "Flare");
}

#[cfg(windows)]
#[test]
fn the_resource_version_equals_cargo_toml() {
    let (file_version, _) = exe_strings();
    assert!(
        file_version.starts_with(VERSION),
        "FileVersion {file_version:?} does not start with {VERSION:?}"
    );
}
