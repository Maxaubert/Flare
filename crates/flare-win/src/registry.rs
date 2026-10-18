//! M9 Task 13: the registry wrapper (the Registry trait, WinRegistry and FakeRegistry).
//!
//! Everything here is per user: `WinRegistry` only ever opens `HKEY_CURRENT_USER`. The one
//! machine-wide touch is `LocalMachineRead`, which reads a string with `RegGetValueW` and has no
//! write path. `SandboxRoot` gives tests a throwaway subtree so the real classes hive is never
//! written by a test.

use std::collections::{BTreeMap, BTreeSet};
use std::ptr::{null, null_mut};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
    ERROR_UNSUPPORTED_TYPE,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE,
    REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteTreeW,
    RegGetValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegSetValueExW,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegError {
    pub code: u32,
    pub what: &'static str,
}

pub trait Registry: Send + Sync {
    /// `key` is relative to the hive (HKCU). `name` None is the key's default value. Creates missing parents.
    fn set_string(&self, key: &str, name: Option<&str>, value: &str) -> Result<(), RegError>;
    /// None when the key or the value is missing or the value is not REG_SZ.
    fn get_string(&self, key: &str, name: Option<&str>) -> Result<Option<String>, RegError>;
    /// Removes the key and everything under it. Ok when it was never there.
    fn delete_tree(&self, key: &str) -> Result<(), RegError>;
    /// Removes the key only when it has NO subkeys and NO values. Ok(false) when it was kept or absent.
    fn delete_if_empty(&self, key: &str) -> Result<bool, RegError>;
    fn key_exists(&self, key: &str) -> Result<bool, RegError>;
}

fn nul_error() -> RegError {
    RegError {
        code: 87,
        what: "nul in string",
    }
}

/// UTF-16 with a trailing NUL; refuses an interior NUL.
fn wide(s: &str) -> Result<Vec<u16>, RegError> {
    if s.contains('\0') {
        return Err(nul_error());
    }
    Ok(s.encode_utf16().chain(std::iter::once(0)).collect())
}

fn wide_opt(s: Option<&str>) -> Result<Option<Vec<u16>>, RegError> {
    s.map(wide).transpose()
}

fn ptr_of(v: &Option<Vec<u16>>) -> *const u16 {
    v.as_ref().map_or(null(), |w| w.as_ptr())
}

/// Closes the key on every path.
struct KeyGuard(HKEY);

impl Drop for KeyGuard {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegCreateKeyExW/RegOpenKeyExW and is closed once.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn is_missing(code: u32) -> bool {
    code == ERROR_FILE_NOT_FOUND || code == ERROR_PATH_NOT_FOUND
}

/// Reads a REG_SZ under `hive`, with the standard size loop on ERROR_MORE_DATA.
fn read_string(hive: HKEY, key: &str, name: Option<&str>) -> Result<Option<String>, RegError> {
    let key_w = wide(key)?;
    let name_w = wide_opt(name)?;
    let mut buf: Vec<u16> = vec![0; 256];
    for _ in 0..8 {
        let mut cb: u32 = (buf.len() * 2) as u32;
        // SAFETY: key_w/name_w are NUL-terminated and outlive the call; buf has cb bytes of room.
        let rc = unsafe {
            RegGetValueW(
                hive,
                key_w.as_ptr(),
                ptr_of(&name_w),
                RRF_RT_REG_SZ,
                null_mut(),
                buf.as_mut_ptr().cast(),
                &mut cb,
            )
        };
        if rc == ERROR_SUCCESS {
            let units = (cb as usize / 2).min(buf.len());
            let mut s = &buf[..units];
            while let Some((&0, rest)) = s.split_last() {
                s = rest;
            }
            return Ok(Some(String::from_utf16_lossy(s)));
        }
        if rc == ERROR_MORE_DATA {
            let need = (cb as usize).div_ceil(2) + 1;
            buf = vec![0; need.max(buf.len() * 2)];
            continue;
        }
        if is_missing(rc) || rc == ERROR_UNSUPPORTED_TYPE {
            return Ok(None);
        }
        return Err(RegError {
            code: rc,
            what: "RegGetValueW",
        });
    }
    Err(RegError {
        code: ERROR_MORE_DATA,
        what: "RegGetValueW size loop",
    })
}

/// The per-user registry. There is no constructor for another hive.
pub struct WinRegistry {
    hive: HKEY,
}

// SAFETY: HKEY_CURRENT_USER is a process-wide pseudo handle, valid on every thread.
unsafe impl Send for WinRegistry {}
// SAFETY: the pseudo handle is read-only data; the registry API is thread safe.
unsafe impl Sync for WinRegistry {}

impl WinRegistry {
    pub fn current_user() -> Self {
        Self {
            hive: HKEY_CURRENT_USER,
        }
    }
}

impl Registry for WinRegistry {
    fn set_string(&self, key: &str, name: Option<&str>, value: &str) -> Result<(), RegError> {
        let key_w = wide(key)?;
        let name_w = wide_opt(name)?;
        let value_w = wide(value)?;
        let mut h: HKEY = null_mut();
        // SAFETY: all pointers are valid for the call; h receives an owned handle on success.
        let rc = unsafe {
            RegCreateKeyExW(
                self.hive,
                key_w.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                null(),
                &mut h,
                null_mut(),
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(RegError {
                code: rc,
                what: "RegCreateKeyExW",
            });
        }
        let _guard = KeyGuard(h);
        // SAFETY: h is open for writing; value_w holds len*2 bytes including the NUL.
        let rc = unsafe {
            RegSetValueExW(
                h,
                ptr_of(&name_w),
                0,
                REG_SZ,
                value_w.as_ptr().cast(),
                (value_w.len() * 2) as u32,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(RegError {
                code: rc,
                what: "RegSetValueExW",
            });
        }
        Ok(())
    }

    fn get_string(&self, key: &str, name: Option<&str>) -> Result<Option<String>, RegError> {
        read_string(self.hive, key, name)
    }

    fn delete_tree(&self, key: &str) -> Result<(), RegError> {
        let key_w = wide(key)?;
        // SAFETY: key_w is NUL-terminated and outlives the call.
        let rc = unsafe { RegDeleteTreeW(self.hive, key_w.as_ptr()) };
        if rc == ERROR_SUCCESS || is_missing(rc) {
            Ok(())
        } else {
            Err(RegError {
                code: rc,
                what: "RegDeleteTreeW",
            })
        }
    }

    fn delete_if_empty(&self, key: &str) -> Result<bool, RegError> {
        let key_w = wide(key)?;
        let mut h: HKEY = null_mut();
        // SAFETY: key_w is NUL-terminated; h receives an owned handle on success.
        let rc = unsafe { RegOpenKeyExW(self.hive, key_w.as_ptr(), 0, KEY_READ, &mut h) };
        if is_missing(rc) {
            return Ok(false);
        }
        if rc != ERROR_SUCCESS {
            return Err(RegError {
                code: rc,
                what: "RegOpenKeyExW",
            });
        }
        let (subkeys, values) = {
            let _guard = KeyGuard(h);
            let mut subkeys: u32 = 0;
            let mut values: u32 = 0;
            // SAFETY: h is open; only the two count out-params are requested, the rest are null.
            let rc = unsafe {
                RegQueryInfoKeyW(
                    h,
                    null_mut(),
                    null_mut(),
                    null(),
                    &mut subkeys,
                    null_mut(),
                    null_mut(),
                    &mut values,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                )
            };
            if rc != ERROR_SUCCESS {
                return Err(RegError {
                    code: rc,
                    what: "RegQueryInfoKeyW",
                });
            }
            (subkeys, values)
        };
        if subkeys != 0 || values != 0 {
            return Ok(false);
        }
        // SAFETY: key_w is NUL-terminated and outlives the call.
        let rc = unsafe { RegDeleteKeyW(self.hive, key_w.as_ptr()) };
        if rc == ERROR_SUCCESS {
            Ok(true)
        } else if is_missing(rc) {
            Ok(false)
        } else {
            Err(RegError {
                code: rc,
                what: "RegDeleteKeyW",
            })
        }
    }

    fn key_exists(&self, key: &str) -> Result<bool, RegError> {
        let key_w = wide(key)?;
        let mut h: HKEY = null_mut();
        // SAFETY: key_w is NUL-terminated; h receives an owned handle on success.
        let rc = unsafe { RegOpenKeyExW(self.hive, key_w.as_ptr(), 0, KEY_READ, &mut h) };
        if rc == ERROR_SUCCESS {
            drop(KeyGuard(h));
            Ok(true)
        } else if is_missing(rc) {
            Ok(false)
        } else {
            Err(RegError {
                code: rc,
                what: "RegOpenKeyExW",
            })
        }
    }
}

/// Read-only machine-wide strings (the log's session facts only). No trait, no write path.
pub struct LocalMachineRead;

impl LocalMachineRead {
    pub fn get_string(key: &str, name: &str) -> Option<String> {
        read_string(HKEY_LOCAL_MACHINE, key, Some(name))
            .ok()
            .flatten()
    }
}

#[derive(Default)]
struct FakeState {
    values: BTreeMap<(String, String), String>,
    keys: BTreeSet<String>,
    writes: usize,
}

/// An in-memory registry, case-insensitive like Windows.
#[derive(Default)]
pub struct FakeRegistry {
    state: Mutex<FakeState>,
}

impl FakeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn keys(&self) -> Vec<String> {
        self.lock().keys.iter().cloned().collect()
    }

    pub fn writes(&self) -> usize {
        self.lock().writes
    }
}

fn norm_key(key: &str) -> String {
    key.trim_matches('\\').to_lowercase()
}

impl Registry for FakeRegistry {
    fn set_string(&self, key: &str, name: Option<&str>, value: &str) -> Result<(), RegError> {
        if key.contains('\0') || name.is_some_and(|n| n.contains('\0')) || value.contains('\0') {
            return Err(nul_error());
        }
        let k = norm_key(key);
        let mut st = self.lock();
        let mut path = String::new();
        for part in k.split('\\').filter(|p| !p.is_empty()) {
            if !path.is_empty() {
                path.push('\\');
            }
            path.push_str(part);
            st.keys.insert(path.clone());
        }
        st.values
            .insert((k, name.unwrap_or("").to_lowercase()), value.to_string());
        st.writes += 1;
        Ok(())
    }

    fn get_string(&self, key: &str, name: Option<&str>) -> Result<Option<String>, RegError> {
        if key.contains('\0') || name.is_some_and(|n| n.contains('\0')) {
            return Err(nul_error());
        }
        let st = self.lock();
        Ok(st
            .values
            .get(&(norm_key(key), name.unwrap_or("").to_lowercase()))
            .cloned())
    }

    fn delete_tree(&self, key: &str) -> Result<(), RegError> {
        if key.contains('\0') {
            return Err(nul_error());
        }
        let k = norm_key(key);
        let prefix = format!("{k}\\");
        let mut st = self.lock();
        st.keys.retain(|x| *x != k && !x.starts_with(&prefix));
        st.values
            .retain(|(x, _), _| *x != k && !x.starts_with(&prefix));
        st.writes += 1;
        Ok(())
    }

    fn delete_if_empty(&self, key: &str) -> Result<bool, RegError> {
        if key.contains('\0') {
            return Err(nul_error());
        }
        let k = norm_key(key);
        let prefix = format!("{k}\\");
        let mut st = self.lock();
        if !st.keys.contains(&k) {
            return Ok(false);
        }
        let has_child = st.keys.iter().any(|x| x.starts_with(&prefix));
        let has_value = st.values.keys().any(|(x, _)| *x == k);
        if has_child || has_value {
            return Ok(false);
        }
        st.keys.remove(&k);
        st.writes += 1;
        Ok(true)
    }

    fn key_exists(&self, key: &str) -> Result<bool, RegError> {
        if key.contains('\0') {
            return Err(nul_error());
        }
        Ok(self.lock().keys.contains(&norm_key(key)))
    }
}

/// A throwaway subtree `HKCU\Software\FlareTest\<uuid>`; Drop deletes it.
pub struct SandboxRoot {
    root: String,
}

impl SandboxRoot {
    pub fn new() -> Self {
        Self {
            root: format!("Software\\FlareTest\\{}", uuid::Uuid::new_v4()),
        }
    }

    /// The classes root inside the sandbox, standing in for the real classes hive.
    pub fn classes(&self) -> String {
        format!("{}\\Classes", self.root)
    }
}

impl Default for SandboxRoot {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SandboxRoot {
    fn drop(&mut self) {
        let reg = WinRegistry::current_user();
        let _ = reg.delete_tree(&self.root);
        // The parent key goes too when no parallel test still has a subtree under it.
        let _ = reg.delete_if_empty("Software\\FlareTest");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_default_value_round_trips() {
        let r = FakeRegistry::new();
        r.set_string("A\\B", None, "x").unwrap();
        assert_eq!(r.get_string("A\\B", None).unwrap(), Some("x".to_string()));
    }

    #[test]
    fn fake_is_case_insensitive_like_windows() {
        let r = FakeRegistry::new();
        r.set_string("Software\\Cls\\X", None, "v").unwrap();
        assert_eq!(
            r.get_string("software\\cls\\x", None).unwrap(),
            Some("v".to_string())
        );
        assert!(r.key_exists("SOFTWARE\\CLS").unwrap());
    }

    #[test]
    fn fake_delete_tree_removes_children_only_under_that_key() {
        let r = FakeRegistry::new();
        r.set_string("A\\B", None, "1").unwrap();
        r.set_string("A\\B\\command", None, "2").unwrap();
        r.set_string("A\\BC", None, "3").unwrap();
        r.delete_tree("A\\B").unwrap();
        assert!(!r.key_exists("A\\B").unwrap());
        assert!(!r.key_exists("A\\B\\command").unwrap());
        assert_eq!(r.get_string("A\\BC", None).unwrap(), Some("3".to_string()));
    }

    #[test]
    fn fake_missing_is_none_not_an_error() {
        let r = FakeRegistry::new();
        assert_eq!(r.get_string("Nope", None).unwrap(), None);
        assert!(!r.key_exists("Nope").unwrap());
        assert!(r.delete_tree("Nope").is_ok());
        assert!(!r.delete_if_empty("Nope").unwrap());
    }

    #[test]
    fn fake_counts_writes() {
        let r = FakeRegistry::new();
        r.set_string("A", None, "1").unwrap();
        assert_eq!(r.writes(), 1);
        r.get_string("A", None).unwrap();
        assert_eq!(r.writes(), 1);
        r.delete_tree("A").unwrap();
        assert_eq!(r.writes(), 2);
    }

    fn empty_rules(r: &dyn Registry) {
        r.set_string("A\\B", None, "1").unwrap();
        assert!(!r.delete_if_empty("A").unwrap());
        assert!(r.key_exists("A").unwrap());
        r.set_string("C", None, "v").unwrap();
        assert!(!r.delete_if_empty("C").unwrap());
        r.set_string("D\\E", None, "1").unwrap();
        r.delete_tree("D\\E").unwrap();
        assert!(r.delete_if_empty("D").unwrap());
        assert!(!r.key_exists("D").unwrap());
    }

    #[test]
    fn fake_delete_if_empty_keeps_a_key_with_a_child_or_a_value() {
        empty_rules(&FakeRegistry::new());
    }

    struct Prefixed<'a>(&'a WinRegistry, String);

    impl Registry for Prefixed<'_> {
        fn set_string(&self, k: &str, n: Option<&str>, v: &str) -> Result<(), RegError> {
            self.0.set_string(&format!("{}\\{k}", self.1), n, v)
        }
        fn get_string(&self, k: &str, n: Option<&str>) -> Result<Option<String>, RegError> {
            self.0.get_string(&format!("{}\\{k}", self.1), n)
        }
        fn delete_tree(&self, k: &str) -> Result<(), RegError> {
            self.0.delete_tree(&format!("{}\\{k}", self.1))
        }
        fn delete_if_empty(&self, k: &str) -> Result<bool, RegError> {
            self.0.delete_if_empty(&format!("{}\\{k}", self.1))
        }
        fn key_exists(&self, k: &str) -> Result<bool, RegError> {
            self.0.key_exists(&format!("{}\\{k}", self.1))
        }
    }

    #[test]
    fn real_delete_if_empty_under_a_sandbox_root() {
        let sb = SandboxRoot::new();
        let real = WinRegistry::current_user();
        empty_rules(&Prefixed(&real, sb.classes()));
    }

    #[test]
    fn real_round_trip_under_a_sandbox_root() {
        let sb = SandboxRoot::new();
        let root = sb.root.clone();
        let real = WinRegistry::current_user();
        let verb = format!("{}\\Directory\\shell\\Flare", sb.classes());
        real.set_string(&verb, None, "Open in Flare").unwrap();
        real.set_string(&verb, Some("Icon"), "C:\\x.exe,0").unwrap();
        assert_eq!(
            real.get_string(&verb, None).unwrap().as_deref(),
            Some("Open in Flare")
        );
        assert_eq!(
            real.get_string(&verb, Some("Icon")).unwrap().as_deref(),
            Some("C:\\x.exe,0")
        );
        assert!(real.key_exists(&verb).unwrap());
        real.delete_tree(&verb).unwrap();
        assert!(!real.key_exists(&verb).unwrap());
        drop(sb);
        assert!(!WinRegistry::current_user().key_exists(&root).unwrap());
    }

    #[test]
    fn real_value_with_unicode_and_quotes_round_trips() {
        let sb = SandboxRoot::new();
        let real = WinRegistry::current_user();
        let key = format!("{}\\U", sb.classes());
        let v = "C:\\Prograd\u{00e4}\\\"x\" & y\\Flare.exe";
        real.set_string(&key, Some("n"), v).unwrap();
        assert_eq!(
            real.get_string(&key, Some("n")).unwrap().as_deref(),
            Some(v)
        );
    }

    #[test]
    fn real_a_long_value_over_the_first_buffer_size_round_trips() {
        let sb = SandboxRoot::new();
        let real = WinRegistry::current_user();
        let key = format!("{}\\L", sb.classes());
        let v = "ab\u{00e9}".repeat(1000);
        assert_eq!(v.chars().count(), 3000);
        real.set_string(&key, None, &v).unwrap();
        assert_eq!(
            real.get_string(&key, None).unwrap().as_deref(),
            Some(v.as_str())
        );
    }

    #[test]
    fn real_a_missing_key_reads_none_and_deletes_ok() {
        let sb = SandboxRoot::new();
        let real = WinRegistry::current_user();
        let key = format!("{}\\Missing\\Deep", sb.classes());
        assert_eq!(real.get_string(&key, None).unwrap(), None);
        assert!(real.delete_tree(&key).is_ok());
        assert!(!real.key_exists(&key).unwrap());
        assert!(!real.delete_if_empty(&key).unwrap());
    }

    #[test]
    fn real_a_nul_in_a_string_is_refused_without_a_write() {
        let sb = SandboxRoot::new();
        let real = WinRegistry::current_user();
        let key = format!("{}\\N", sb.classes());
        let e = real.set_string(&key, None, "a\0b").unwrap_err();
        assert_eq!(e, nul_error());
        assert!(real.set_string(&format!("{key}\0"), None, "a").is_err());
        assert!(real.set_string(&key, Some("n\0"), "a").is_err());
        assert!(!real.key_exists(&key).unwrap());
    }

    #[test]
    fn the_real_test_never_touches_the_classes_hive() {
        let src = include_str!("registry.rs");
        let tests = &src[src.find("#[cfg(test)]").unwrap()..];
        // Built from parts so this scan does not match itself.
        let needle = ["Software", "Classes"].join("\\\\");
        assert_eq!(
            tests.matches(needle.as_str()).count(),
            0,
            "tests must build keys from SandboxRoot only"
        );
    }
}
