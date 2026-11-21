//! Is this code Apple's? (rule 1325). The sim host dlopens SimulatorKit from the chosen Xcode and CoreSimulator from
//! `/Library/Developer`; a planted `Xcode.app` must never get its code run that way. Every Xcode the pick considers
//! ([`crate::xcode::find`]) and every framework [`super::load::load`] opens is first checked with the Security
//! framework: `SecStaticCodeCreateWithPath`, `SecRequirementCreateWithString`, `SecStaticCodeCheckValidity`.
//!
//! What is checked, and how deep (measured on this Mac, Xcode 27.2, 2026-11-20):
//! - The Xcode app against [`XCODE`] with `kSecCSDoNotValidateResources`: its main executable and sealed Info.plist,
//!   not the hash of every file in the bundle: about 13 ms. A full check of Xcode's resources takes about 8 s of disk
//!   reading (`codesign -v`), far too slow for every start. What Flare runs from the bundle is checked on its own:
//!   `simctl` ([`XCODE_RUN`]) byte for byte against the app's resource seal, about 200 ms more (the Security
//!   framework reads Xcode's 32 MB `CodeResources`), `libxcrun.dylib` and the frameworks in full below.
//! - SimulatorKit and CoreSimulator against [`APPLE`] (`anchor apple`) in full, with `kSecCSCheckNestedCode`: the
//!   binary, the resource seal and the nested frameworks. About 2 ms and 12 ms. CoreSimulator's own `simctl` (which
//!   Xcode's script execs) is one of its sealed resources.
//!
//! The times are from `the_real_xcode_and_its_frameworks_pass` (warm disk cache). Paths are resolved
//! (`canonicalize`) before they are checked, so a symlink cannot be swapped between the check and the use. The check
//! and the dlopen are still two steps: changing the files themselves between them needs write access to the Xcode
//! bundle or `/Library/Developer`.
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
    fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: isize) -> CFTypeRef;
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
    fn SecCodeValidateFileResource(
        static_code: CFTypeRef,
        relative_path: CFTypeRef,
        file_data: CFTypeRef,
        flags: u32,
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

/// A CFString of `text`, or None.
fn cf_string(text: &str) -> Option<Owned> {
    let c = CString::new(text).ok()?;
    // SAFETY: a NUL-terminated UTF-8 string; returns a +1 CFString or null.
    let s = Owned(unsafe {
        CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8)
    });
    (!s.0.is_null()).then_some(s)
}

/// Checks that the code at `path` (a bundle or a binary) is validly signed and satisfies `requirement`, and gives
/// back its static code. The error names the path and the Security framework's status. BLOCKING (it reads the code).
fn checked(path: &Path, requirement: &str, depth: Depth) -> Result<Owned, String> {
    let fail = |what: &str, status: OSStatus| {
        Err(format!(
            "{} is not signed by Apple ({what}: OSStatus {status})",
            path.display()
        ))
    };
    let bytes = path.as_os_str().as_bytes();
    let len = isize::try_from(bytes.len()).map_err(|_| "path too long".to_string())?;
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
    let Some(text) = cf_string(requirement) else {
        return fail("CFString", -1);
    };
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
    Ok(code)
}

/// Checks that the code at `path` (a bundle or a binary) is validly signed and satisfies `requirement`. The error
/// names the path and the Security framework's status. BLOCKING (it reads the code).
pub fn check(path: &Path, requirement: &str, depth: Depth) -> Result<(), String> {
    checked(path, requirement, depth).map(|_| ())
}

/// The files of the Xcode app that Flare runs, relative to `Contents`: `simctl`, which `xcrun simctl` runs (Xcode
/// 27's is a shell script that execs CoreSimulator's). [`crate::xcode::verify`] also checks `libxcrun.dylib` and
/// that `xcrun` resolves `simctl` to this file.
pub const XCODE_RUN: &[&str] = &["Developer/usr/bin/simctl"];

/// The Xcode app at `app` is Apple's ([`XCODE`]): its executable and Info.plist, and each file in [`XCODE_RUN`]
/// byte for byte against the app's resource seal (`SecCodeValidateFileResource`), or Apple-signed on its own, so a
/// bundle made of Xcode's signed executable and a planted `simctl` is refused. The rest of the resources are not
/// hashed (about 8 s).
pub fn xcode_app(app: &Path) -> Result<(), String> {
    let code = checked(app, XCODE, Depth::Executable)?;
    for rel in XCODE_RUN {
        let file = app.join("Contents").join(rel);
        // sync-ok: the Xcode pick's worker or the sim host's start; a small file.
        let data = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let len = isize::try_from(data.len()).map_err(|_| "file too long".to_string())?;
        // SAFETY: a valid buffer of `len` bytes, copied by CFDataCreate; returns a +1 CFData or null.
        let cf_data = Owned(unsafe { CFDataCreate(std::ptr::null(), data.as_ptr(), len) });
        let Some(cf_rel) = cf_string(rel) else {
            return Err(format!(
                "{} is not sealed by Xcode's signature",
                file.display()
            ));
        };
        if cf_data.0.is_null() {
            return Err(format!(
                "{} is not sealed by Xcode's signature",
                file.display()
            ));
        }
        // SAFETY: a live SecStaticCode, CFString and CFData made above; the call only reads them.
        let status = unsafe {
            SecCodeValidateFileResource(code.0, cf_rel.0, cf_data.0, K_SEC_CS_DEFAULT_FLAGS)
        };
        // An older Xcode's `simctl` is a signed Mach-O of its own: Apple's signature on it is enough.
        if status != 0 && check(&file, APPLE, Depth::Full).is_err() {
            return Err(format!(
                "{} is not sealed by Xcode's signature (OSStatus {status})",
                file.display()
            ));
        }
    }
    Ok(())
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

    /// Rule 1325: a bundle made of the real Xcode's signed executable, Info.plist and signature, with a planted
    /// `simctl`, meets Xcode's requirement when resources are not hashed, so [`xcode_app`] checks `simctl` against
    /// the seal and refuses it. Skipped without `/Applications/Xcode.app`.
    #[test]
    fn a_planted_simctl_is_refused() {
        let real = Path::new("/Applications/Xcode.app/Contents");
        if !real.join("Developer/usr/bin/simctl").exists() {
            eprintln!("skipped: no /Applications/Xcode.app");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Xcode-beta.app");
        let contents = app.join("Contents");
        for rel in [
            "MacOS/Xcode",
            "Info.plist",
            "PkgInfo",
            "version.plist",
            "_CodeSignature/CodeResources",
            "Developer/usr/bin/simctl",
        ] {
            let to = contents.join(rel);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(real.join(rel), &to).unwrap();
        }
        // The real simctl, copied: it matches the seal.
        let t = std::time::Instant::now();
        let shallow = check(&app, XCODE, Depth::Executable);
        if shallow.is_err() {
            eprintln!("note: the copied executable alone fails here too: {shallow:?}");
        }
        if shallow.is_ok() {
            xcode_app(&app).unwrap();
            eprintln!("Xcode.app (executable + simctl): {:?}", t.elapsed());
        }
        let simctl = contents.join("Developer/usr/bin/simctl");
        std::fs::remove_file(&simctl).unwrap();
        std::fs::write(&simctl, "#!/bin/bash\ntouch /tmp/flare-pwned\n").unwrap();
        let e = xcode_app(&app).unwrap_err();
        assert!(e.contains("simctl") || e.contains("not signed"), "{e}");
        std::fs::remove_file(&simctl).unwrap();
        assert!(xcode_app(&app).is_err(), "a missing simctl is refused");
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
        check(app, XCODE, Depth::Executable).unwrap();
        eprintln!("Xcode.app (executable): {:?}", t.elapsed());
        let t = std::time::Instant::now();
        xcode_app(app).unwrap();
        eprintln!("Xcode.app (executable + simctl seal): {:?}", t.elapsed());
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
