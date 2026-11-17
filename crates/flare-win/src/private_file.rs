//! A file only its owner may read: Flare's pairing key (rule 1277).
//!
//! ssh refuses a private key that anyone else may read. Off Windows that is mode 600. On Windows, OpenSSH reads the
//! file's DACL and refuses the key ("UNPROTECTED PRIVATE KEY FILE", "Bad permissions") when an ACE lets in anyone but
//! the user, SYSTEM or Administrators. A new file takes its folder's inherited ACEs, and a profile folder can carry an
//! extra group (Max's PC: `CodexSandboxUsers`), so the key is set to a protected DACL with one ACE: full control for
//! the user. [`keep_private`] checks that before ssh reads the key and sets it again when it is wrong.

use std::io;
use std::path::Path;

/// The SIDs Windows' OpenSSH lets into a private key besides the user: SYSTEM and Administrators.
pub const TRUSTED_SIDS: [&str; 2] = ["S-1-5-18", "S-1-5-32-544"];

/// Who a file's DACL lets in, as Windows says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    /// The DACL takes no ACE from its folder.
    pub protected: bool,
    /// No DACL at all: everyone is let in.
    pub everyone: bool,
    /// The SID of every ACE that lets someone in, in order (`?` for an allow ACE of a kind that is not read).
    pub allowed: Vec<String>,
}

/// Pure: whether a DACL lets in only `user`, SYSTEM or Administrators (what OpenSSH on Windows accepts).
pub fn only_trusted(access: &Access, user: &str) -> bool {
    !access.everyone
        && access
            .allowed
            .iter()
            .all(|s| s.eq_ignore_ascii_case(user) || TRUSTED_SIDS.contains(&s.as_str()))
}

/// BLOCKING: makes `path` readable by its owner only (mode 600 off Windows; on Windows a protected DACL with one ACE,
/// full control for the user, nothing inherited).
pub fn make_private(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        win::make_private(path)
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) // sync-ok: a worker's call
    }
}

/// BLOCKING: whether `path` lets in only its owner (and, on Windows, SYSTEM and Administrators).
pub fn is_private(path: &Path) -> io::Result<bool> {
    #[cfg(windows)]
    {
        let user = win::current_user_sid()?;
        Ok(only_trusted(&win::access(path)?, &user))
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path)?.permissions().mode(); // sync-ok: a worker's call
        Ok(mode & 0o077 == 0)
    }
}

/// BLOCKING (one read of the file's access, and one write when it is wrong): `path` made private again when anyone
/// else may read it. True when it was set again.
pub fn keep_private(path: &Path) -> io::Result<bool> {
    if is_private(path)? {
        return Ok(false);
    }
    make_private(path)?;
    Ok(true)
}

/// BLOCKING: who `path`'s DACL lets in. Windows only.
#[cfg(windows)]
pub fn access(path: &Path) -> io::Result<Access> {
    win::access(path)
}

/// The SID string of the user this process runs as. Windows only.
#[cfg(windows)]
pub fn current_user_sid() -> io::Result<String> {
    win::current_user_sid()
}

/// BLOCKING: `path`'s DACL set from SDDL, protected (nothing inherited from its folder). Windows only; the tests use it
/// to let another group in.
#[cfg(windows)]
pub fn set_dacl(path: &Path, sddl: &str) -> io::Result<()> {
    win::set_dacl(path, sddl)
}

#[cfg(windows)]
mod win {
    use super::Access;
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
        GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    };

    /// ACE kinds (winnt.h): the plain allow ACE, and the deny ones, which let no one in.
    const ACCESS_ALLOWED: u8 = 0;
    const DENIED: [u8; 4] = [1, 6, 10, 12];

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// A LocalAlloc'd security descriptor, freed on drop.
    struct Descriptor(PSECURITY_DESCRIPTOR);

    impl Drop for Descriptor {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the descriptor came from a call that LocalAllocs it, and is freed once.
                unsafe { LocalFree(self.0) };
            }
        }
    }

    pub fn current_user_sid() -> io::Result<String> {
        crate::single_instance::current_identity()
            .map(|i| i.sid)
            .ok_or_else(|| io::Error::other("Windows did not say who this user is"))
    }

    fn sid_string(sid: PSID) -> Option<String> {
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: `sid` points into a live ACL; `text` receives a LocalAlloc'd string.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
            return None;
        }
        let mut n = 0;
        // SAFETY: `text` is NUL-terminated, as ConvertSidToStringSidW promises.
        while unsafe { *text.add(n) } != 0 {
            n += 1;
        }
        // SAFETY: `n` units were just walked.
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, n) });
        // SAFETY: freeing the string the call allocated, once.
        unsafe { LocalFree(text.cast()) };
        Some(s)
    }

    pub fn access(path: &Path) -> io::Result<Access> {
        let name = wide(path);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `name` is NUL-terminated; the out-pointers outlive the call; `sd` is freed by `Descriptor`.
        let rc = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut sd,
            )
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let sd = Descriptor(sd);
        let mut control: u16 = 0;
        let mut revision: u32 = 0;
        // SAFETY: `sd.0` is the descriptor the call above returned.
        unsafe { GetSecurityDescriptorControl(sd.0, &mut control, &mut revision) };
        let protected = control & SE_DACL_PROTECTED != 0;
        if dacl.is_null() {
            return Ok(Access {
                protected,
                everyone: true,
                allowed: Vec::new(),
            });
        }
        // SAFETY: `dacl` points into `sd`, alive until the end of this function.
        let count = unsafe { (*dacl).AceCount };
        let mut allowed = Vec::new();
        for i in 0..u32::from(count) {
            let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
            // SAFETY: `i` is below the ACL's count; `ace` receives a pointer into the ACL.
            if unsafe { GetAce(dacl, i, &mut ace) } == 0 || ace.is_null() {
                allowed.push("?".into());
                continue;
            }
            // SAFETY: every ACE starts with its header.
            let kind = unsafe { (*ace.cast::<ACE_HEADER>()).AceType };
            if DENIED.contains(&kind) {
                continue;
            }
            if kind != ACCESS_ALLOWED {
                allowed.push("?".into());
                continue;
            }
            // SAFETY: a plain allow ACE: its SID starts at `SidStart`.
            let sid =
                unsafe { std::ptr::addr_of_mut!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart) };
            allowed.push(sid_string(sid.cast()).unwrap_or_else(|| "?".into()));
        }
        Ok(Access {
            protected,
            everyone: false,
            allowed,
        })
    }

    pub fn make_private(path: &Path) -> io::Result<()> {
        set_dacl(path, &super::owner_only_sddl(&current_user_sid()?))
    }

    /// `path`'s DACL set from SDDL, protected (nothing inherited from its folder).
    pub fn set_dacl(path: &Path, sddl: &str) -> io::Result<()> {
        let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated and outlives the call; `sd` receives the descriptor.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || sd.is_null() {
            return Err(io::Error::last_os_error());
        }
        let sd = Descriptor(sd);
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl: *mut ACL = std::ptr::null_mut();
        // SAFETY: `sd.0` is a valid descriptor; the out-pointers outlive the call.
        let ok =
            unsafe { GetSecurityDescriptorDacl(sd.0, &mut present, &mut dacl, &mut defaulted) };
        if ok == 0 || present == 0 || dacl.is_null() {
            return Err(io::Error::other("the DACL could not be built"));
        }
        let name = wide(path);
        // SAFETY: `name` is NUL-terminated; `dacl` points into `sd`, alive across the call.
        let rc = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            )
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        Ok(())
    }
}

/// The SDDL of a private file's DACL: protected (nothing inherited), one ACE, full control for `sid`.
pub fn owner_only_sddl(sid: &str) -> String {
    format!("D:P(A;;FA;;;{sid})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc(allowed: &[&str]) -> Access {
        Access {
            protected: true,
            everyone: false,
            allowed: allowed.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Rule 1277: the user, SYSTEM and Administrators may be let in; any other SID, an ACE kind not read, or no DACL
    /// at all is not private.
    #[test]
    fn only_the_user_system_and_administrators_count_as_private() {
        let me = "S-1-5-21-1-2-3-1001";
        assert!(only_trusted(&acc(&[me]), me));
        assert!(only_trusted(&acc(&[me, "S-1-5-18", "S-1-5-32-544"]), me));
        assert!(only_trusted(&acc(&[]), me));
        // Max's PC: an extra local group inherited from the Roaming folder.
        assert!(!only_trusted(&acc(&[me, "S-1-5-21-1-2-3-1008"]), me));
        assert!(!only_trusted(&acc(&[me, "S-1-5-32-545"]), me), "Users");
        assert!(!only_trusted(&acc(&[me, "S-1-1-0"]), me), "Everyone");
        assert!(!only_trusted(&acc(&[me, "?"]), me));
        let mut none = acc(&[]);
        none.everyone = true;
        assert!(!only_trusted(&none, me));
        assert_eq!(owner_only_sddl(me), format!("D:P(A;;FA;;;{me})"));
    }

    /// Rule 1277: a key written into a folder that lets an extra group in ends with its owner alone, nothing
    /// inherited; a private file is left as it is.
    #[test]
    fn a_file_that_lets_another_group_in_is_made_private() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id_flare_ed25519");
        #[cfg(windows)]
        {
            // The folder lets the user and the Users group in, and its files inherit both (the bug's shape).
            let me = current_user_sid().unwrap();
            set_dacl(
                dir.path(),
                &format!("D:P(A;OICI;FA;;;{me})(A;OICI;FR;;;BU)"),
            )
            .unwrap();
            std::fs::write(&key, "secret").unwrap();
            let before = access(&key).unwrap();
            assert!(
                before.allowed.iter().any(|s| s == "S-1-5-32-545"),
                "{before:?}"
            );
            assert!(!is_private(&key).unwrap());
            assert!(keep_private(&key).unwrap(), "set again");
            assert_eq!(
                access(&key).unwrap(),
                Access {
                    protected: true,
                    everyone: false,
                    allowed: vec![me]
                }
            );
            assert!(!keep_private(&key).unwrap(), "already private");
            assert_eq!(
                std::fs::read_to_string(&key).unwrap(),
                "secret",
                "still readable by its owner"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::write(&key, "secret").unwrap();
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(keep_private(&key).unwrap());
            let mode = std::fs::metadata(&key).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            assert!(!keep_private(&key).unwrap());
        }
    }
}
