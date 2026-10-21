//! The Windows build number (Task 17). Read once through `RtlGetVersion`, which is not shimmed
//! by app manifests the way `GetVersionEx` is.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OsVersion {
    pub major: u32,
    pub minor: u32,
    pub build: u32,
}

static VERSION: OnceLock<OsVersion> = OnceLock::new();

/// This PC's Windows version, read once and cached; `{ 10, 0, 0 }` when the call fails.
pub fn os_version() -> OsVersion {
    *VERSION.get_or_init(read_version)
}

#[cfg(windows)]
fn read_version() -> OsVersion {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    // SAFETY: a zeroed OSVERSIONINFOW with its size set is the documented input of RtlGetVersion.
    let mut v: OSVERSIONINFOW = unsafe { core::mem::zeroed() };
    v.dwOSVersionInfoSize = size_of::<OSVERSIONINFOW>() as u32;
    // sync-ok: ntdll call, microseconds
    // SAFETY: `v` is a valid, writable OSVERSIONINFOW.
    let status = unsafe { RtlGetVersion(&mut v) };
    if status == 0 {
        OsVersion {
            major: v.dwMajorVersion,
            minor: v.dwMinorVersion,
            build: v.dwBuildNumber,
        }
    } else {
        OsVersion {
            major: 10,
            minor: 0,
            build: 0,
        }
    }
}

#[cfg(not(windows))]
fn read_version() -> OsVersion {
    OsVersion {
        major: 10,
        minor: 0,
        build: 0,
    }
}

/// "10.0.22631" -> `{ 10, 0, 22631 }`; empty or junk -> `None`.
pub fn parse_release(rel: &str) -> Option<OsVersion> {
    let mut it = rel.trim().split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let build = it.next()?.parse().ok()?;
    Some(OsVersion {
        major,
        minor,
        build,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_release_reads_dotted_builds() {
        assert_eq!(
            parse_release("10.0.22631"),
            Some(OsVersion {
                major: 10,
                minor: 0,
                build: 22631
            })
        );
        assert_eq!(parse_release(""), None);
        assert_eq!(parse_release("x"), None);
    }

    #[cfg(windows)]
    #[test]
    fn os_version_reads_this_pcs_build() {
        let v = os_version();
        assert_eq!(v.major, 10);
        assert!(v.build >= 17763, "build {}", v.build);
    }
}
