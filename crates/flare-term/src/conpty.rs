//! Bundled ConPTY load and verify.
//!
//! portable-pty loads `conpty.dll` by bare name, so Flare preloads the bundled copy by absolute path
//! before the first `openpty`; Windows then resolves the bare name to the module already loaded.

use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const CONPTY_VERSION: &str = "1.25.260303002-preview";
pub const CONPTY_DLL_SHA256: &str =
    "3319b484b80bb53d1f4d0a9eb0ea60fd0f61da69db7280ca43b84215f19245ff";
pub const OPENCONSOLE_SHA256: &str =
    "7f68c840226505004215c0b82d4e502c24b5bc3f4b93c4baaaa19bd679c0def8";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConptyError {
    Missing(PathBuf),
    WrongHash {
        file: String,
        expected: String,
        actual: String,
    },
    LoadFailed {
        path: PathBuf,
        os_error: u32,
    },
    WrongModule {
        expected: PathBuf,
        actual: PathBuf,
    },
}

impl fmt::Display for ConptyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConptyError::Missing(p) => write!(
                f,
                "The terminal file {} is missing. Reinstall Flare.",
                p.display()
            ),
            ConptyError::WrongHash { file, .. } => {
                write!(
                    f,
                    "The terminal file {file} is not the one Flare ships. Reinstall Flare."
                )
            }
            ConptyError::LoadFailed { path, os_error } => {
                write!(
                    f,
                    "The terminal file {} could not be loaded (error {os_error}). Reinstall Flare.",
                    path.display()
                )
            }
            ConptyError::WrongModule { expected, actual } => write!(
                f,
                "conpty.dll was loaded from {} instead of {}. Reinstall Flare.",
                actual.display(),
                expected.display()
            ),
        }
    }
}

impl std::error::Error for ConptyError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundled {
    pub dir: PathBuf,
    pub dll: PathBuf,
    pub open_console: PathBuf,
}

/// When `env_dir` is set and non-empty it is the only candidate. Otherwise the exe folder, then the
/// committed copy under the crate (debug builds, or a test binary running from a `deps` folder).
pub fn candidate_dirs_with(env_dir: Option<&str>, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    if let Some(d) = env_dir
        && !d.is_empty()
    {
        return vec![PathBuf::from(d)];
    }
    let mut out = Vec::new();
    if let Some(e) = exe_dir {
        out.push(e.to_path_buf());
    }
    let in_deps = exe_dir
        .and_then(|e| e.file_name())
        .is_some_and(|n| n.eq_ignore_ascii_case("deps"));
    if cfg!(debug_assertions) || in_deps {
        out.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("bundled")
                .join("x64"),
        );
    }
    out
}

pub fn candidate_dirs() -> Vec<PathBuf> {
    let env_dir = std::env::var("FLARE_CONPTY_DIR").ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    candidate_dirs_with(env_dir.as_deref(), exe_dir.as_deref())
}

/// The first candidate holding both files; else the first missing file of the first candidate.
pub fn bundled_in(dirs: &[PathBuf]) -> Result<Bundled, ConptyError> {
    for d in dirs {
        let dll = d.join("conpty.dll");
        let open_console = d.join("OpenConsole.exe");
        if dll.is_file() && open_console.is_file() {
            return Ok(Bundled {
                dir: d.clone(),
                dll,
                open_console,
            });
        }
    }
    let first = dirs.first().cloned().unwrap_or_default();
    let dll = first.join("conpty.dll");
    if dll.is_file() {
        Err(ConptyError::Missing(first.join("OpenConsole.exe")))
    } else {
        Err(ConptyError::Missing(dll))
    }
}

pub fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?; // sync-ok: startup check and tests only, never a live path
    let digest = Sha256::digest(&bytes);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn check_one(path: &Path, expected: &str) -> Result<(), ConptyError> {
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match sha256_hex(path) {
        Ok(actual) if actual == expected => Ok(()),
        Ok(actual) => Err(ConptyError::WrongHash {
            file,
            expected: expected.to_string(),
            actual,
        }),
        Err(_) => Err(ConptyError::Missing(path.to_path_buf())),
    }
}

pub fn check_hashes(b: &Bundled) -> Result<(), ConptyError> {
    check_one(&b.dll, CONPTY_DLL_SHA256)?;
    check_one(&b.open_console, OPENCONSOLE_SHA256)
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

static LOADED: OnceLock<Result<Bundled, ConptyError>> = OnceLock::new();

/// Preloads the bundled conpty.dll by absolute path. Must run before the first `openpty`.
pub fn load_bundled() -> Result<Bundled, ConptyError> {
    LOADED
        .get_or_init(|| {
            use windows_sys::Win32::Foundation::GetLastError;
            use windows_sys::Win32::System::LibraryLoader::{
                LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
            };
            let b = bundled_in(&candidate_dirs())?;
            let path = wide(&b.dll);
            // SAFETY: `path` is a NUL-terminated UTF-16 buffer that outlives the call; the file handle
            // argument must be null for this API.
            let h = unsafe {
                LoadLibraryExW(
                    path.as_ptr(),
                    std::ptr::null_mut(),
                    LOAD_WITH_ALTERED_SEARCH_PATH,
                )
            };
            if h.is_null() {
                // SAFETY: GetLastError has no preconditions.
                let os_error = unsafe { GetLastError() };
                return Err(ConptyError::LoadFailed {
                    path: b.dll.clone(),
                    os_error,
                });
            }
            Ok(b)
        })
        .clone()
}

/// The path of the module Windows calls "conpty.dll" in this process, if loaded.
pub fn loaded_module_path() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
    let name: Vec<u16> = "conpty.dll\0".encode_utf16().collect();
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let h = unsafe { GetModuleHandleW(name.as_ptr()) };
    if h.is_null() {
        return None;
    }
    let mut buf = vec![0u16; 4096];
    // SAFETY: `buf` is valid for `buf.len()` u16 writes; `h` is a live module handle.
    let n = unsafe { GetModuleFileNameW(h, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])))
}

fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().to_lowercase();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// The startup check: load, hash both files, and confirm the module in use is the bundled one.
pub fn verify_bundled() -> Result<Bundled, ConptyError> {
    let b = load_bundled()?;
    check_hashes(&b)?;
    match loaded_module_path() {
        Some(actual) if norm(&actual) == norm(&b.dll) => Ok(b),
        Some(actual) => Err(ConptyError::WrongModule {
            expected: b.dll,
            actual,
        }),
        None => Err(ConptyError::WrongModule {
            expected: b.dll,
            actual: PathBuf::new(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_bundle() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("bundled/x64")
    }

    #[test]
    fn bundled_files_match_the_pinned_hashes() {
        let dir = manifest_bundle();
        assert_eq!(
            sha256_hex(&dir.join("conpty.dll")).unwrap(),
            CONPTY_DLL_SHA256
        );
        assert_eq!(
            sha256_hex(&dir.join("OpenConsole.exe")).unwrap(),
            OPENCONSOLE_SHA256
        );
    }

    #[test]
    fn the_env_override_is_the_only_candidate() {
        assert_eq!(
            candidate_dirs_with(Some("D:\\x"), Some(Path::new("C:\\app"))),
            vec![PathBuf::from("D:\\x")]
        );
    }

    #[test]
    fn without_an_override_the_exe_folder_comes_first() {
        assert_eq!(
            candidate_dirs_with(None, Some(Path::new("C:\\app")))[0],
            PathBuf::from("C:\\app")
        );
        assert!(
            candidate_dirs_with(Some(""), None)
                .iter()
                .all(|p| !p.as_os_str().is_empty())
        );
    }

    #[test]
    fn a_test_binary_in_deps_finds_the_committed_copy() {
        assert_eq!(
            candidate_dirs_with(None, Some(Path::new("C:\\t\\target\\release\\deps"))),
            vec![
                PathBuf::from("C:\\t\\target\\release\\deps"),
                manifest_bundle()
            ]
        );
    }

    #[test]
    fn a_missing_bundle_is_a_loud_error() {
        let t = tempfile::tempdir().unwrap();
        let Err(ConptyError::Missing(p)) = bundled_in(&[t.path().to_path_buf()]) else {
            panic!("expected Missing");
        };
        assert!(p.ends_with("conpty.dll"));
        let msg = ConptyError::Missing(p).to_string();
        assert!(msg.contains("conpty.dll") && msg.contains("Reinstall Flare."));
    }

    #[test]
    fn a_changed_file_is_reported_by_name() {
        let t = tempfile::tempdir().unwrap();
        let src = manifest_bundle();
        for f in ["conpty.dll", "OpenConsole.exe"] {
            std::fs::copy(src.join(f), t.path().join(f)).unwrap();
        }
        let mut bytes = std::fs::read(t.path().join("conpty.dll")).unwrap();
        bytes[0] ^= 0xff;
        std::fs::write(t.path().join("conpty.dll"), bytes).unwrap();
        let b = bundled_in(&[t.path().to_path_buf()]).unwrap();
        match check_hashes(&b) {
            Err(ConptyError::WrongHash { file, .. }) => assert!(file.contains("conpty.dll")),
            other => panic!("expected WrongHash, got {other:?}"),
        }
    }

    #[test]
    fn verify_loads_the_bundled_dll_by_absolute_path() {
        let b = verify_bundled().expect("bundled conpty verifies");
        let got = loaded_module_path().expect("module loaded");
        assert_eq!(norm(&got), norm(&b.dll));
    }
}
