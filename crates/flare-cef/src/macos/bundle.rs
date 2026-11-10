//! The dev bundle: what macOS CEF needs around a browser process. CEF's layout
//! (<https://bitbucket.org/chromiumembedded/cef/wiki/GeneralUsage.md>, "macOS"):
//!
//! ```text
//! <Name>.app/Contents/
//!   Info.plist
//!   MacOS/<Name>                                  the browser process
//!   Frameworks/Chromium Embedded Framework.framework
//!   Frameworks/<Name> Helper.app                  utilities, network
//!   Frameworks/<Name> Helper (GPU).app
//!   Frameworks/<Name> Helper (Renderer).app
//!   Frameworks/<Name> Helper (Plugin).app
//!   Frameworks/<Name> Helper (Alerts).app
//! ```
//!
//! Every helper runs `flare-cef-helper`. The framework is an APFS clone of the cached CEF build (instant, no
//! space). Everything is signed ad hoc (`codesign -s -`), inside out: the framework's libraries, the framework,
//! the helpers, the app. That is enough to run on this Mac; a Developer ID signature, the hardened runtime, its
//! entitlements and notarization wait for the signed app (phase 8: Max has no Developer ID yet). Built under the
//! target dir only, never the repo.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::pin::{FRAMEWORK, HELPER_SUFFIXES};

/// What to bundle.
pub struct BundleSpec<'a> {
    /// The app's name (`Flare`): the bundle, its executable and the helpers' prefix.
    pub name: &'a str,
    /// The reverse-DNS id; helpers add `.helper`, `.helper.gpu` and so on.
    pub identifier: &'a str,
    /// The browser process executable.
    pub main_exe: &'a Path,
    /// `flare-cef-helper`.
    pub helper_exe: &'a Path,
    /// The CEF folder (holds the framework).
    pub cef_dir: &'a Path,
    /// Where `<name>.app` goes (replaced if there).
    pub out_dir: &'a Path,
    /// `LSBackgroundOnly`: the app never has a Dock tile, a menu bar or a window (the probe). Off for an app that
    /// will show its own window.
    pub background_only: bool,
}

/// The helper app's name for a suffix: `Flare Helper (GPU)`.
pub fn helper_name(name: &str, suffix: &str) -> String {
    format!("{name} {suffix}")
}

/// The helper's bundle id: `.helper`, `.helper.gpu`, `.helper.renderer`, `.helper.plugin`, `.helper.alerts`.
pub fn helper_id(identifier: &str, suffix: &str) -> String {
    let kind = suffix
        .trim_start_matches("Helper")
        .trim()
        .trim_matches(|c| c == '(' || c == ')')
        .to_ascii_lowercase();
    if kind.is_empty() {
        format!("{identifier}.helper")
    } else {
        format!("{identifier}.helper.{kind}")
    }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The `Info.plist` text of the app (`helper` false) or a helper.
pub fn info_plist(exe: &str, identifier: &str, helper: bool, background_only: bool) -> String {
    let mut keys = vec![
        (
            "CFBundleDevelopmentRegion",
            "<string>en</string>".to_string(),
        ),
        (
            "CFBundleExecutable",
            format!("<string>{}</string>", xml(exe)),
        ),
        (
            "CFBundleIdentifier",
            format!("<string>{}</string>", xml(identifier)),
        ),
        (
            "CFBundleInfoDictionaryVersion",
            "<string>6.0</string>".into(),
        ),
        ("CFBundleName", format!("<string>{}</string>", xml(exe))),
        ("CFBundlePackageType", "<string>APPL</string>".into()),
        (
            "CFBundleShortVersionString",
            "<string>0.1.0</string>".into(),
        ),
        ("CFBundleVersion", "<string>0.1.0</string>".into()),
        // CEF's own advice for every process of the bundle.
        (
            "LSEnvironment",
            "<dict><key>MallocNanoZone</key><string>0</string></dict>".into(),
        ),
        ("LSMinimumSystemVersion", "<string>15.0</string>".into()),
        ("NSHighResolutionCapable", "<true/>".into()),
        ("NSSupportsAutomaticGraphicsSwitching", "<true/>".into()),
    ];
    if helper {
        // CEF's helpers: no Dock tile, no menu bar.
        keys.push(("LSUIElement", "<true/>".into()));
    }
    if background_only {
        keys.push(("LSBackgroundOnly", "<true/>".into()));
    }
    let body: String = keys
        .iter()
        .map(|(k, v)| format!("  <key>{k}</key>\n  {v}\n"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n{body}</dict>\n</plist>\n"
    )
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Clones a folder (APFS `clonefile` through `cp -c`; a plain copy where the volume cannot clone).
fn clone_dir(from: &Path, to: &Path) -> Result<(), String> {
    if run(Command::new("/bin/cp").arg("-cR").arg(from).arg(to)).is_ok() {
        return Ok(());
    }
    let _ = fs::remove_dir_all(to);
    run(Command::new("/bin/cp").arg("-R").arg(from).arg(to))
}

fn app_skeleton(app: &Path, exe_name: &str, exe: &Path, plist: &str) -> Result<(), String> {
    let contents = app.join("Contents");
    for d in ["MacOS", "Frameworks", "Resources"] {
        fs::create_dir_all(contents.join(d)).map_err(|e| format!("{}: {e}", app.display()))?;
    }
    fs::write(contents.join("Info.plist"), plist).map_err(|e| e.to_string())?;
    let to = contents.join("MacOS").join(exe_name);
    fs::copy(exe, &to).map_err(|e| format!("{} to {}: {e}", exe.display(), to.display()))?;
    Ok(())
}

/// Signs `path` ad hoc (no identity: `-s -`), replacing any signature it had.
pub fn sign_adhoc(path: &Path) -> Result<(), String> {
    run(Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-", "--timestamp=none"])
        .arg(path))
}

/// Builds `<out_dir>/<name>.app`; its path.
pub fn build(spec: &BundleSpec<'_>) -> Result<PathBuf, String> {
    let src_fw = spec.cef_dir.join(FRAMEWORK);
    if !src_fw.exists() {
        return Err(format!("no {FRAMEWORK} in {}", spec.cef_dir.display()));
    }
    fs::create_dir_all(spec.out_dir).map_err(|e| e.to_string())?;
    let app = spec.out_dir.join(format!("{}.app", spec.name));
    if app.exists() {
        fs::remove_dir_all(&app).map_err(|e| format!("{}: {e}", app.display()))?;
    }
    app_skeleton(
        &app,
        spec.name,
        spec.main_exe,
        &info_plist(spec.name, spec.identifier, false, spec.background_only),
    )?;
    let frameworks = app.join("Contents/Frameworks");
    let fw = frameworks.join(FRAMEWORK);
    clone_dir(&src_fw, &fw)?;
    let mut helpers = Vec::new();
    for suffix in HELPER_SUFFIXES {
        let name = helper_name(spec.name, suffix);
        let happ = frameworks.join(format!("{name}.app"));
        app_skeleton(
            &happ,
            &name,
            spec.helper_exe,
            &info_plist(&name, &helper_id(spec.identifier, suffix), true, false),
        )?;
        helpers.push(happ);
    }
    // Inside out: the framework's libraries, the framework, the helpers, the app.
    let libs = fw.join("Libraries");
    if let Ok(rd) = fs::read_dir(&libs) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "dylib") {
                sign_adhoc(&e.path())?;
            }
        }
    }
    sign_adhoc(&fw)?;
    for h in &helpers {
        sign_adhoc(h)?;
    }
    sign_adhoc(&app)?;
    Ok(app)
}

/// `codesign --verify --deep --strict` of a built bundle.
pub fn verify(app: &Path) -> Result<(), String> {
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(app))
}

/// The browser process executable inside a bundle.
pub fn exe_in(app: &Path, name: &str) -> PathBuf {
    app.join("Contents/MacOS").join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_are_named_and_numbered_the_way_cef_looks_for_them() {
        assert_eq!(helper_name("Flare", "Helper (GPU)"), "Flare Helper (GPU)");
        assert_eq!(helper_id("dev.flare", "Helper"), "dev.flare.helper");
        assert_eq!(
            helper_id("dev.flare", "Helper (GPU)"),
            "dev.flare.helper.gpu"
        );
        assert_eq!(
            helper_id("dev.flare", "Helper (Renderer)"),
            "dev.flare.helper.renderer"
        );
        assert_eq!(
            helper_id("dev.flare", "Helper (Plugin)"),
            "dev.flare.helper.plugin"
        );
        assert_eq!(
            helper_id("dev.flare", "Helper (Alerts)"),
            "dev.flare.helper.alerts"
        );
    }

    #[test]
    fn a_helper_plist_hides_the_helper_and_the_probe_plist_is_background_only() {
        let h = info_plist("Flare Helper (GPU)", "dev.flare.helper.gpu", true, false);
        assert!(h.contains("<key>LSUIElement</key>\n  <true/>"));
        assert!(!h.contains("LSBackgroundOnly"));
        assert!(h.contains("<string>Flare Helper (GPU)</string>"));
        let p = info_plist("Flare & <Probe>", "dev.flare.probe", false, true);
        assert!(p.contains("<key>LSBackgroundOnly</key>\n  <true/>"));
        assert!(!p.contains("LSUIElement"));
        assert!(p.contains("Flare &amp; &lt;Probe&gt;"));
        assert!(p.contains("<key>LSMinimumSystemVersion</key>\n  <string>15.0</string>"));
    }
}
