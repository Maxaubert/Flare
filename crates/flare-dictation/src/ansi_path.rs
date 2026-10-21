//! A path an engine can open when it reads it as ANSI (audit 2026-10-20, L32/L63).
//!
//! whisper-server and the Parakeet library take the model path as `char*` and open it through the C
//! runtime in the system code page. A folder the code page cannot spell (a Japanese user name on a
//! Western install, say) turns into `?` and the open fails. The 8.3 short name of the same file is plain
//! ASCII, so the path is handed over in that form whenever the original is not ANSI-clean. A volume with
//! short names off keeps the original path (nothing better exists).

use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

#[link(name = "kernel32")]
extern "system" {
    fn WideCharToMultiByte(
        code_page: u32,
        flags: u32,
        wide: *const u16,
        wide_len: i32,
        out: *mut u8,
        out_len: i32,
        default_char: *const u8,
        used_default: *mut i32,
    ) -> i32;
    fn GetShortPathNameW(long: *const u16, short: *mut u16, len: u32) -> u32;
}

/// True when the system ANSI code page spells every character of `w` (nothing becomes a default `?`).
pub fn ansi_clean(w: &[u16]) -> bool {
    if w.is_empty() {
        return true;
    }
    let Ok(len) = i32::try_from(w.len()) else {
        return false;
    };
    let mut used_default = 0i32;
    // SAFETY: `w` is a live buffer of `len` UTF-16 units; with a null output of size 0 the call only
    // measures; `used_default` is a live int.
    let size = unsafe {
        WideCharToMultiByte(
            0,
            0,
            w.as_ptr(),
            len,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            &mut used_default,
        )
    };
    size > 0 && used_default == 0
}

fn short_name(path: &Path) -> Option<PathBuf> {
    let w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `w` is a live NUL-terminated wide string; a null buffer of size 0 asks for the length.
    let need = unsafe { GetShortPathNameW(w.as_ptr(), std::ptr::null_mut(), 0) };
    if need == 0 {
        return None;
    }
    let mut out = vec![0u16; need as usize];
    // SAFETY: `out` has `need` units, the size passed.
    let got = unsafe { GetShortPathNameW(w.as_ptr(), out.as_mut_ptr(), need) };
    if got == 0 || got >= need {
        return None;
    }
    out.truncate(got as usize);
    Some(PathBuf::from(std::ffi::OsString::from_wide(&out)))
}

/// `path` itself when the code page can spell it, else its 8.3 short form when that is clean, else `path`.
pub fn engine_safe_path(path: &Path) -> PathBuf {
    let w: Vec<u16> = path.as_os_str().encode_wide().collect();
    if ansi_clean(&w) {
        return path.to_path_buf();
    }
    match short_name(path) {
        Some(short) => {
            let sw: Vec<u16> = short.as_os_str().encode_wide().collect();
            if ansi_clean(&sw) {
                short
            } else {
                path.to_path_buf()
            }
        }
        None => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ascii_path_is_left_alone() {
        let p = Path::new(r"C:\models\ggml-base.bin");
        assert_eq!(engine_safe_path(p), p);
    }

    #[test]
    fn a_path_outside_the_code_page_becomes_a_clean_short_path_to_the_same_file() {
        // Characters from scripts no Western or single-byte code page spells, so the test bites on any
        // machine whose ANSI page is not UTF-8.
        let dir = tempfile::Builder::new()
            .prefix("\u{30e2}\u{30c7}\u{30eb}\u{65e5}\u{672c}\u{8a9e}\u{d55c}")
            .tempdir()
            .unwrap();
        let file = dir.path().join("\u{30e2}\u{30c7}\u{30eb}.bin");
        std::fs::write(&file, b"weights").unwrap();
        let w: Vec<u16> = file.as_os_str().encode_wide().collect();
        if ansi_clean(&w) {
            return; // the code page is UTF-8 here: the original path already works
        }
        let safe = engine_safe_path(&file);
        let sw: Vec<u16> = safe.as_os_str().encode_wide().collect();
        assert!(ansi_clean(&sw), "still not ANSI-clean: {safe:?}");
        assert_eq!(std::fs::read(&safe).unwrap(), b"weights");
    }
}
