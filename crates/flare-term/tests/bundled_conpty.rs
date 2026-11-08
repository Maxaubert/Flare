//! The ConPTY serving a Flare pty is the bundled one (module path and process image path).
//! ConPTY is Windows-only.
#![cfg(windows)]

mod common;

use std::path::PathBuf;

use common::{Rig, temp_dir_named};
use flare_term::conpty;

fn norm(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().to_lowercase();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

#[test]
fn the_startup_check_passes_with_the_committed_bundle() {
    let b = conpty::verify_bundled();
    assert!(b.is_ok(), "the startup check failed: {b:?}");
}

#[test]
fn the_conpty_dll_in_the_process_is_the_bundled_one() {
    let dir = temp_dir_named("flare-m2-dll");
    let rig = Rig::spawn("cmd.exe", &[], dir.path(), None, 80, 24);
    let bundled = conpty::load_bundled().expect("the bundle loads");
    let loaded = conpty::loaded_module_path().expect("conpty.dll is loaded in this process");
    assert_eq!(
        norm(&loaded),
        norm(&bundled.dll),
        "screen: {:?}",
        rig.screen()
    );
}

/// Full image paths of every running OpenConsole.exe we may query.
fn open_console_images() -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    let mut out = Vec::new();
    // SAFETY: the snapshot call takes no pointers; the handle is checked and closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return out;
    }
    // SAFETY: PROCESSENTRY32W is plain data; all-zero is a valid starting value before dwSize is set.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is a live snapshot and `entry` a valid out-pointer with dwSize set.
    let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while more {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = std::ffi::OsString::from_wide(&entry.szExeFile[..len])
            .to_string_lossy()
            .to_lowercase();
        if name == "openconsole.exe" {
            // SAFETY: OpenProcess takes no pointers; a null handle just means we may not query it.
            let h =
                unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID) };
            if !h.is_null() {
                let mut buf = vec![0u16; 4096];
                let mut size = buf.len() as u32;
                // SAFETY: `h` is a live handle; `buf` holds `size` u16s and `size` is a valid in/out pointer.
                let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size) };
                if ok != 0 {
                    out.push(PathBuf::from(std::ffi::OsString::from_wide(
                        &buf[..size as usize],
                    )));
                }
                // SAFETY: `h` was opened above and is closed exactly once.
                unsafe { CloseHandle(h) };
            }
        }
        // SAFETY: as for Process32FirstW.
        more = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }
    // SAFETY: `snap` was created above and is closed exactly once.
    unsafe { CloseHandle(snap) };
    out
}

#[test]
fn the_open_console_serving_a_pty_is_the_bundled_exe() {
    let dir = temp_dir_named("flare-m2-oc");
    let rig = Rig::spawn("cmd.exe", &[], dir.path(), None, 80, 24);
    let bundled = conpty::load_bundled().expect("the bundle loads");
    let want = norm(&bundled.open_console);
    let seen = rig.wait_for(10_000, |_| {
        open_console_images().iter().any(|p| norm(p) == want)
    });
    assert!(
        seen,
        "no OpenConsole.exe ran from {want}; images: {:?}; screen: {:?}",
        open_console_images(),
        rig.screen()
    );
}
