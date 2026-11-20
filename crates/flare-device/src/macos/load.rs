//! Loading the simulator frameworks in the sim host (never in Flare, rule 1310), reading their versions, the `hello`
//! facts, and the self-test that checks every class, protocol and selector the sim host uses.
//!
//! CoreSimulator is the system-wide `/Library/Developer/PrivateFrameworks/CoreSimulator.framework` (installed by the
//! newest Xcode that ran its first launch, shared by every Xcode). SimulatorKit belongs to the chosen Xcode:
//! `Contents/SharedFrameworks` from Xcode 27, `Contents/Developer/Library/PrivateFrameworks` before.

use std::path::{Path, PathBuf};

use objc2::runtime::{AnyClass, AnyProtocol, Bool, Sel};
use objc2_foundation::{NSBundle, NSString};

use super::codesign;
use super::guarded::{Broken, class, sel};
use crate::versions::{self, InputPath};
use crate::wire::{Hello, PROTOCOL};
use crate::xcode;

/// CoreSimulator, one per Mac.
pub const CORESIM: &str = "/Library/Developer/PrivateFrameworks/CoreSimulator.framework";

/// Where SimulatorKit may be in an Xcode (its `Contents/Developer` folder given), newest layout first.
pub fn simkit_paths(developer_dir: &Path) -> [PathBuf; 2] {
    let contents = developer_dir.parent().unwrap_or(developer_dir);
    [
        contents.join("SharedFrameworks/SimulatorKit.framework"),
        developer_dir.join("Library/PrivateFrameworks/SimulatorKit.framework"),
    ]
}

/// The frameworks the sim host loaded, and their versions.
#[derive(Debug)]
pub struct Loaded {
    pub developer_dir: PathBuf,
    pub xcode_version: String,
    pub coresim_version: String,
    pub simkit_version: String,
    pub simkit_path: PathBuf,
}

/// The Xcode to use: `DEVELOPER_DIR` (Flare sets it), else what `xcode-select -p` says (run by hand). BLOCKING.
pub fn developer_dir() -> Result<PathBuf, String> {
    if let Some(d) = std::env::var_os("DEVELOPER_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    let out = super::simctl::run(
        // sync-ok: the sim host's start, at most 5 s
        std::process::Command::new("/usr/bin/xcode-select").arg("-p"),
        None,
        std::time::Duration::from_secs(5),
    )
    .map_err(|e| format!("no Xcode: xcode-select {e}"))?;
    let p = PathBuf::from(String::from_utf8_lossy(&out).trim());
    if xcode::app_of(&p).is_none() {
        return Err("The device panel needs Xcode.".into());
    }
    Ok(p)
}

/// The `CFBundleVersion` of the bundle at `path`.
fn bundle_version(path: &Path) -> Option<String> {
    let b = NSBundle::bundleWithPath(&NSString::from_str(&path.to_string_lossy()))?;
    let v = b.objectForInfoDictionaryKey(&NSString::from_str("CFBundleVersion"))?;
    let s = v.downcast_ref::<NSString>()?.to_string();
    (!s.is_empty()).then_some(s)
}

/// Opens a framework's binary and keeps it loaded for the life of the process (Objective-C images are never
/// unloaded).
fn open(framework: &Path) -> Result<(), String> {
    let name = framework
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let bin = framework.join(&name);
    // SAFETY: a framework `load` checked as Apple-signed just before (rule 1325); its initialisers only register its
    // classes.
    let lib = unsafe { libloading::Library::new(&bin) }
        .map_err(|e| format!("{name} did not load: {e}"))?;
    std::mem::forget(lib);
    Ok(())
}

/// Loads CoreSimulator and the Xcode's SimulatorKit. BLOCKING (dlopen, and the signature checks before it): the sim
/// host's start only. Nothing is opened unless the Xcode app, SimulatorKit and CoreSimulator are all Apple's
/// (rule 1325, [`super::codesign`]): `DEVELOPER_DIR` may name any folder when the sim host is run by hand.
pub fn load(developer_dir: &Path) -> Result<Loaded, String> {
    let app = xcode::app_of(developer_dir)
        .ok_or_else(|| format!("no Xcode at {}", developer_dir.display()))?;
    let xcode_version = xcode::version_of(&app)
        .ok_or_else(|| format!("no Xcode at {}", developer_dir.display()))?;
    let simkit_path = simkit_paths(developer_dir)
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| format!("SimulatorKit missing in Xcode {xcode_version}"))?;
    codesign::xcode_app(&app)?;
    codesign::apple_framework(&simkit_path)?;
    codesign::apple_framework(Path::new(CORESIM))?;
    open(Path::new(CORESIM))?;
    open(&simkit_path)?;
    let coresim_version = bundle_version(Path::new(CORESIM)).unwrap_or_default();
    let simkit_version = bundle_version(&simkit_path).unwrap_or_default();
    Ok(Loaded {
        developer_dir: developer_dir.to_path_buf(),
        xcode_version,
        coresim_version,
        simkit_version,
        simkit_path,
    })
}

impl Loaded {
    /// The sim host's first line.
    pub fn hello(&self) -> Hello {
        Hello {
            protocol: PROTOCOL,
            version: env!("CARGO_PKG_VERSION").to_string(),
            coresim_version: self.coresim_version.clone(),
            simkit_version: self.simkit_version.clone(),
            xcode_version: self.xcode_version.clone(),
            xcode_path: self.developer_dir.to_string_lossy().to_string(),
            input_path: versions::input_path(&self.coresim_version),
            // The UI tree comes with PR 3.
            ax: false,
        }
    }

    pub fn input_path(&self) -> InputPath {
        versions::input_path(&self.coresim_version)
    }
}

/// Where a selector the sim host sends is declared.
#[derive(Clone, Copy, Debug)]
pub enum Owner {
    /// A class method.
    ClassMethod(&'static str),
    /// An instance method of a class.
    Class(&'static str),
    /// A method of a protocol (the proxies answer it by forwarding).
    Protocol(&'static str),
}

/// Every class, protocol and selector the frame path uses (`sim.rs`), checked by `--self-test` and before attach.
pub const USED: &[(Owner, &str)] = &[
    (
        Owner::ClassMethod("SimServiceContext"),
        "sharedServiceContextForDeveloperDir:error:",
    ),
    (
        Owner::Class("SimServiceContext"),
        "defaultDeviceSetWithError:",
    ),
    (Owner::Class("SimDeviceSet"), "devicesByUDID"),
    (Owner::Class("SimDevice"), "io"),
    (Owner::Class("SimDevice"), "deviceType"),
    (Owner::Class("SimDevice"), "stateString"),
    (Owner::Class("SimDeviceType"), "mainScreenSize"),
    (Owner::Class("SimDeviceType"), "mainScreenScale"),
    (Owner::Protocol("SimDeviceIOProtocol"), "ioPorts"),
    (Owner::Protocol("SimDeviceIOPortInterface"), "descriptor"),
    (
        Owner::Protocol("SimScreenAdapter"),
        "enumerateScreensWithCompletionQueue:completionHandler:",
    ),
    (Owner::Protocol("SimScreen"), "screenProperties"),
    (
        Owner::Protocol("SimScreen"),
        "registerScreenCallbacksWithUUID:callbackQueue:frameCallback:surfacesChangedCallback:propertiesChangedCallback:",
    ),
    (
        Owner::Protocol("SimScreen"),
        "unregisterScreenCallbacksWithUUID:",
    ),
    (
        Owner::Protocol("SimDisplayIOSurfaceRenderable"),
        "framebufferSurface",
    ),
    (
        Owner::Protocol("SimDisplayIOSurfaceRenderable"),
        "registerCallbackWithUUID:ioSurfacesChangeCallback:",
    ),
    (
        Owner::Protocol("SimDisplayIOSurfaceRenderable"),
        "unregisterIOSurfacesChangeCallbackWithUUID:",
    ),
];

fn protocol(name: &str) -> Result<&'static AnyProtocol, Broken> {
    let c = std::ffi::CString::new(name).map_err(|_| Broken::missing(name))?;
    AnyProtocol::get(&c).ok_or_else(|| Broken::missing(name))
}

fn class_has(c: &AnyClass, s: Sel, meta: bool) -> bool {
    if meta {
        c.metaclass().instance_method(s).is_some()
    } else {
        c.instance_method(s).is_some()
    }
}

fn protocol_has(p: &AnyProtocol, s: Sel) -> bool {
    [true, false].into_iter().any(|required| {
        // SAFETY: a protocol the runtime gave us and a registered selector; the result is a plain struct.
        let d = unsafe {
            objc2::ffi::protocol_getMethodDescription(p, s, Bool::new(required), Bool::YES)
        };
        d.name.is_some()
    })
}

/// Checks one entry of [`USED`]: the symbol that is missing, if any.
pub fn check(owner: Owner, selector: &str) -> Result<(), Broken> {
    let s = sel(selector);
    let ok = match owner {
        Owner::ClassMethod(c) => class_has(class(c)?, s, true),
        Owner::Class(c) => class_has(class(c)?, s, false),
        Owner::Protocol(p) => protocol_has(protocol(p)?, s),
    };
    if ok {
        Ok(())
    } else {
        let at = match owner {
            Owner::ClassMethod(c) => format!("+[{c} {selector}]"),
            Owner::Class(c) => format!("-[{c} {selector}]"),
            Owner::Protocol(p) => format!("<{p}> {selector}"),
        };
        Err(Broken::missing(&at))
    }
}

/// Every missing symbol (empty: the frame path can run on this Xcode).
pub fn self_test() -> Vec<Broken> {
    let mut missing = Vec::new();
    for (owner, s) in USED {
        if let Err(b) = check(*owner, s)
            && !missing.contains(&b)
        {
            missing.push(b);
        }
    }
    missing
}

/// What `--self-test` reports.
#[derive(Debug, serde::Serialize)]
pub struct SelfTest {
    pub ok: bool,
    /// Why it did not run (no Xcode, or one under the floor): the CI step skips with this reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hello: Option<Hello>,
    pub checked: usize,
    pub missing: Vec<String>,
}

/// Loads the frameworks of `DEVELOPER_DIR` (or `xcode-select`) and checks every symbol. BLOCKING.
pub fn run_self_test() -> SelfTest {
    let skip = |why: String| SelfTest {
        ok: true,
        skipped: Some(why),
        hello: None,
        checked: 0,
        missing: Vec::new(),
    };
    let dev = match developer_dir() {
        Ok(d) => d,
        Err(e) => return skip(e),
    };
    let loaded = match load(&dev) {
        Ok(l) => l,
        Err(e) => {
            // An Xcode that is there but whose frameworks do not load is a failure; no Xcode at all is a skip.
            if xcode::app_of(&dev)
                .and_then(|a| xcode::version_of(&a))
                .is_none()
            {
                return skip(e);
            }
            return SelfTest {
                ok: false,
                skipped: None,
                hello: None,
                checked: 0,
                missing: vec![e],
            };
        }
    };
    let hello = loaded.hello();
    if versions::support(&hello.xcode_version, &hello.coresim_version) == versions::Support::TooOld
    {
        return skip(format!(
            "Xcode {} is under the floor (Xcode {})",
            hello.xcode_version,
            versions::XCODE_FLOOR
        ));
    }
    let missing: Vec<String> = self_test()
        .iter()
        .map(|b| b.in_coresim(&hello.coresim_version))
        .collect();
    SelfTest {
        ok: missing.is_empty(),
        skipped: None,
        hello: Some(hello),
        checked: USED.len(),
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simkit_is_looked_for_in_both_xcode_layouts() {
        let [new, old] = simkit_paths(Path::new("/Applications/Xcode.app/Contents/Developer"));
        assert_eq!(
            new,
            Path::new("/Applications/Xcode.app/Contents/SharedFrameworks/SimulatorKit.framework")
        );
        assert_eq!(
            old,
            Path::new(
                "/Applications/Xcode.app/Contents/Developer/Library/PrivateFrameworks/SimulatorKit.framework"
            )
        );
        // A missing class or selector names itself.
        let b = check(Owner::Class("FlareNoSuchClass"), "x").unwrap_err();
        assert_eq!(b.to_string(), "FlareNoSuchClass missing");
        let b = check(Owner::Class("NSObject"), "flareNoSuchSelector").unwrap_err();
        assert_eq!(b.to_string(), "-[NSObject flareNoSuchSelector] missing");
        assert!(check(Owner::Class("NSObject"), "description").is_ok());
        assert!(check(Owner::ClassMethod("NSObject"), "new").is_ok());
        assert!(check(Owner::Protocol("NSObject"), "isEqual:").is_ok());
    }

    /// Rule 1325: a `DEVELOPER_DIR` inside an unsigned `Xcode.app` is refused before anything is opened.
    #[test]
    fn an_unsigned_xcode_is_never_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("Xcode.app/Contents");
        let simkit = contents.join("SharedFrameworks/SimulatorKit.framework");
        std::fs::create_dir_all(&simkit).unwrap();
        std::fs::create_dir_all(contents.join("Developer")).unwrap();
        std::fs::write(simkit.join("SimulatorKit"), [0u8; 64]).unwrap();
        std::fs::write(
            contents.join("version.plist"),
            "<plist><dict><key>CFBundleShortVersionString</key><string>99.0</string></dict></plist>",
        )
        .unwrap();
        let e = load(&contents.join("Developer")).unwrap_err();
        assert!(e.contains("is not signed by Apple"), "{e}");
    }

    /// CI's macOS runner (and this Mac): with an Xcode, every class and selector is there; without one, skipped with
    /// the reason.
    #[test]
    fn self_test_finds_every_class_and_selector() {
        let r = run_self_test();
        if let Some(why) = &r.skipped {
            eprintln!("skipped: {why}");
            return;
        }
        assert!(r.ok, "missing: {:?}", r.missing);
        assert_eq!(r.checked, USED.len());
        let hello = r.hello.unwrap();
        assert!(!hello.coresim_version.is_empty());
        assert!(!hello.simkit_version.is_empty());
        assert!(crate::versions::Version::parse(&hello.xcode_version).is_some());
    }
}
