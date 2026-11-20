//! Is this code Apple's? (rule 1325). The sim host dlopens SimulatorKit from the chosen Xcode and CoreSimulator from
//! `/Library/Developer`; a planted `Xcode.app` must never get its code run that way. Every Xcode the pick considers
//! ([`crate::xcode::find`]) and every framework [`super::load::load`] opens is first checked with the Security
//! framework: `SecStaticCodeCreateWithPath`, `SecRequirementCreateWithString`, `SecStaticCodeCheckValidity`.
//!
//! What is checked, and how deep (measured on this Mac, Xcode 27.2, 2026-11-20):
//! - The Xcode app against [`XCODE`] with `kSecCSDoNotValidateResources`: its main executable and sealed Info.plist,
//!   not the hash of every file in the bundle: about 7 ms. A full check of Xcode's resources takes about 8 s of disk
//!   reading (`codesign -v`), far too slow for every start; the code that is loaded is checked in full below.
//! - SimulatorKit and CoreSimulator against [`APPLE`] (`anchor apple`) in full, with `kSecCSCheckNestedCode`: the
//!   binary, the resource seal and the nested frameworks. About 2 ms and 12 ms.
//!
//! The times are from `the_real_xcode_and_its_frameworks_pass` (warm disk cache). The check and the dlopen are two
//! steps; changing the files between them needs write access to `/Applications` or `/Library/Developer`, which is
//! admin already.
//!
//! BLOCKING (file reads): a worker or the sim host's start only, never the UI thread (rule 34).

use std::ffi::{CString, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Code Apple signed with its own certificates: CoreSimulator, SimulatorKit, an Xcode from developer.apple.com.
pub const APPLE: &str = "anchor apple";

/// The Xcode app: Xcode's own designated requirement, plus `anchor apple`. An Xcode from the Mac App Store is
/// signed by "Apple Mac OS Application Signing", which is `anchor apple generic` with the App Store leaf (only Apple
/// issues it, and the App Store gives `com.apple.dt.Xcode` to Apple alone), so plain `anchor apple` refuses it.
pub const XCODE: &str = concat!(
    "identifier \"com.apple.dt.Xcode\" and (anchor apple",
    " or anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.9]",
    " or anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6]",
    " and certificate leaf[field.1.2.840.113635.100.6.1.13]",
    " and certificate leaf[subject.OU] = \"59GAB85EFG\")"
);

/// How much of the bundle is hashed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// The main executable and the sealed Info.plist; resources are not hashed (`kSecCSDoNotValidateResources`).
    Executable,
    /// Everything: the executable, every sealed resource and the nested code (`kSecCSCheckNestedCode`).
    Full,
}

type OSStatus = i32;
type CFTypeRef = *const c_void;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_SEC_CS_DEFAULT_FLAGS: u32 = 0;
const K_SEC_CS_DO_NOT_VALIDATE_RESOURCES: u32 = 1 << 2;
const K_SEC_CS_CHECK_NESTED_CODE: u32 = 1 << 3;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: CFTypeRef,
        buffer: *const u8,
        len: isize,
        is_directory: u8,
    ) -> CFTypeRef;
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecStaticCodeCreateWithPath(
        path: CFTypeRef,
        flags: u32,
        static_code: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecRequirementCreateWithString(
        text: CFTypeRef,
        flags: u32,
        requirement: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecStaticCodeCheckValidity(
        static_code: CFTypeRef,
        flags: u32,
        requirement: CFTypeRef,
    ) -> OSStatus;
}

/// A Core Foundation object this module made, released when dropped.
struct Owned(CFTypeRef);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a +1 reference from a Create call, released once.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// Checks that the code at `path` (a bundle or a binary) is validly signed and satisfies `requirement`. The error
/// names the path and the Security framework's status. BLOCKING (it reads the code).
pub fn check(path: &Path, requirement: &str, depth: Depth) -> Result<(), String> {
    let fail = |what: &str, status: OSStatus| {
        Err(format!(
            "{} is not signed by Apple ({what}: OSStatus {status})",
            path.display()
        ))
    };
    let bytes = path.as_os_str().as_bytes();
    let len = isize::try_from(bytes.len()).map_err(|_| "path too long".to_string())?;
    let req_text = CString::new(requirement).map_err(|_| "bad requirement".to_string())?;
    // SAFETY: a valid buffer of `len` bytes; null allocator is the default one. Returns a +1 CFURL or null.
    let url = Owned(unsafe {
        CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), len, 0)
    });
    if url.0.is_null() {
        return fail("CFURL", -1);
    }
    let mut code: CFTypeRef = std::ptr::null();
    // SAFETY: a live CFURL and an out pointer to a local; on success `code` is a +1 SecStaticCode.
    let status = unsafe { SecStaticCodeCreateWithPath(url.0, K_SEC_CS_DEFAULT_FLAGS, &mut code) };
    let code = Owned(code);
    if status != 0 || code.0.is_null() {
        return fail("SecStaticCodeCreateWithPath", status);
    }
    // SAFETY: a NUL-terminated UTF-8 string; returns a +1 CFString or null.
    let text = Owned(unsafe {
        CFStringCreateWithCString(
            std::ptr::null(),
            req_text.as_ptr(),
            K_CF_STRING_ENCODING_UTF8,
        )
    });
    if text.0.is_null() {
        return fail("CFString", -1);
    }
    let mut req: CFTypeRef = std::ptr::null();
    // SAFETY: a live CFString and an out pointer to a local; on success `req` is a +1 SecRequirement.
    let status =
        unsafe { SecRequirementCreateWithString(text.0, K_SEC_CS_DEFAULT_FLAGS, &mut req) };
    let req = Owned(req);
    if status != 0 || req.0.is_null() {
        return fail("SecRequirementCreateWithString", status);
    }
    let flags = match depth {
        Depth::Executable => K_SEC_CS_DO_NOT_VALIDATE_RESOURCES,
        Depth::Full => K_SEC_CS_CHECK_NESTED_CODE,
    };
    // SAFETY: a live SecStaticCode and SecRequirement made above; the call only reads them and the files.
    let status = unsafe { SecStaticCodeCheckValidity(code.0, flags, req.0) };
    if status != 0 {
        return fail("SecStaticCodeCheckValidity", status);
    }
    Ok(())
}

/// The Xcode app at `app` is Apple's ([`XCODE`], executable only).
pub fn xcode_app(app: &Path) -> Result<(), String> {
    check(app, XCODE, Depth::Executable)
}

/// The framework at `framework` is Apple's ([`APPLE`], in full).
pub fn apple_framework(framework: &Path) -> Result<(), String> {
    check(framework, APPLE, Depth::Full)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bundle that looks like Xcode (Info.plist, version.plist, an executable) but is not signed is refused, and so
    /// is a framework folder with a plain file for a binary.
    #[test]
    fn a_fake_unsigned_xcode_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Xcode.app");
        let contents = app.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>CFBundleIdentifier</key><string>com.apple.dt.Xcode</string>\
             <key>CFBundleExecutable</key><string>Xcode</string>\
             <key>CFBundleShortVersionString</key><string>99.0</string></dict></plist>",
        )
        .unwrap();
        std::fs::write(
            contents.join("version.plist"),
            "<plist><dict><key>CFBundleShortVersionString</key><string>99.0</string></dict></plist>",
        )
        .unwrap();
        std::fs::write(contents.join("MacOS/Xcode"), "#!/bin/sh\nexit 0\n").unwrap();
        let e = xcode_app(&app).unwrap_err();
        assert!(e.contains("is not signed by Apple"), "{e}");
        let fw = contents.join("SharedFrameworks/SimulatorKit.framework");
        std::fs::create_dir_all(&fw).unwrap();
        std::fs::write(fw.join("SimulatorKit"), [0u8; 64]).unwrap();
        assert!(apple_framework(&fw).is_err());
        // A path that is not there is refused, never a panic.
        assert!(apple_framework(&dir.path().join("Nothing.framework")).is_err());
        // Signed by Apple but not Xcode: `/bin/ls` meets `anchor apple`, never Xcode's requirement.
        assert!(check(Path::new("/bin/ls"), APPLE, Depth::Full).is_ok());
        assert!(xcode_app(Path::new("/bin/ls")).is_err());
    }

    /// This Mac's Xcode (when it has one at `/Applications/Xcode.app`) passes, with its frameworks; the times are
    /// printed (`--nocapture`), the numbers in the module doc.
    #[test]
    fn the_real_xcode_and_its_frameworks_pass() {
        let app = Path::new("/Applications/Xcode.app");
        if !app.exists() {
            eprintln!("skipped: no /Applications/Xcode.app");
            return;
        }
        let t = std::time::Instant::now();
        xcode_app(app).unwrap();
        eprintln!("Xcode.app (executable): {:?}", t.elapsed());
        let dev = app.join("Contents/Developer");
        if let Some(simkit) = super::super::load::simkit_paths(&dev)
            .into_iter()
            .find(|p| p.exists())
        {
            let t = std::time::Instant::now();
            apple_framework(&simkit).unwrap();
            eprintln!("SimulatorKit (full): {:?}", t.elapsed());
        }
        let coresim = Path::new(super::super::load::CORESIM);
        if coresim.exists() {
            let t = std::time::Instant::now();
            apple_framework(coresim).unwrap();
            eprintln!("CoreSimulator (full): {:?}", t.elapsed());
        }
    }
}
