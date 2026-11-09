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
//! space). Everything is signed inside out ([`crate::macos::sign`]): the framework's libraries, the framework, the
//! helpers, the app's other programs, the app. Ad hoc (`codesign -s -`) for the dev bundles, or a Developer ID with
//! the hardened runtime and each program's entitlements (`Flare.app`, phase 8, `tools/release/mac/make-app.sh`).
//! Built under the target dir only, never the repo.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::macos::sign::{self, Role, Signing};
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
    /// `CFBundleShortVersionString` and `CFBundleVersion` of the app and its helpers.
    pub version: &'a str,
    /// Ad hoc, or a Developer ID with the hardened runtime.
    pub signing: Signing,
    /// What a release app adds (icon, more programs, resources, Info.plist keys); empty for a dev bundle.
    pub extras: Extras<'a>,
}

/// What `Flare.app` holds beyond a dev bundle.
#[derive(Default)]
pub struct Extras<'a> {
    /// The `.icns`, copied to `Contents/Resources/<name>.icns` and named by `CFBundleIconFile`.
    pub icon: Option<&'a Path>,
    /// More programs for `Contents/MacOS` (by their file name), signed as [`Role::Tool`] with `<identifier>.<name>`.
    pub executables: Vec<&'a Path>,
    /// Files and folders copied into `Contents/Resources` under the given name (never into `Contents/MacOS`, which
    /// holds only code: a folder there breaks the seal).
    pub resources: Vec<(&'a Path, &'a str)>,
    /// More keys for the app's `Info.plist` (key, its XML value).
    pub keys: Vec<(&'static str, String)>,
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

/// The `Info.plist` text of the app (`helper` false) or a helper, with `extra` keys after the common ones.
pub fn info_plist(
    exe: &str,
    identifier: &str,
    version: &str,
    helper: bool,
    background_only: bool,
    extra: &[(&'static str, String)],
) -> String {
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
            format!("<string>{}</string>", xml(version)),
        ),
        (
            "CFBundleVersion",
            format!("<string>{}</string>", xml(version)),
        ),
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
    for (k, v) in extra {
        keys.retain(|(have, _)| have != k);
        keys.push((k, v.clone()));
    }
    let body: String = keys
        .iter()
        .map(|(k, v)| format!("  <key>{k}</key>\n  {v}\n"))
        .collect();
    plist_doc(&body)
}

/// A property list document around a dictionary's body (Apple's DTD names its address; nothing is fetched).
pub fn plist_doc(body: &str) -> String {
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
        .args(sign::codesign_args(
            &Signing::AdHoc,
            Role::Library,
            None,
            None,
        ))
        .arg(path))
}

/// Copies a file or a folder (keeping symbolic links as links).
fn copy_any(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_dir() {
        clone_dir(from, to)
    } else {
        fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("{} to {}: {e}", from.display(), to.display()))
    }
}

fn file_name(p: &Path) -> Result<&str, String> {
    p.file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has no file name", p.display()))
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
    // The entitlements files: beside the bundle, never in it.
    let scratch = spec.out_dir.join(format!("{}.entitlements", spec.name));
    let mut keys = spec.extras.keys.clone();
    if spec.extras.icon.is_some() {
        keys.push((
            "CFBundleIconFile",
            format!("<string>{}</string>", xml(spec.name)),
        ));
    }
    app_skeleton(
        &app,
        spec.name,
        spec.main_exe,
        &info_plist(
            spec.name,
            spec.identifier,
            spec.version,
            false,
            spec.background_only,
            &keys,
        ),
    )?;
    let contents = app.join("Contents");
    if let Some(icon) = spec.extras.icon {
        copy_any(
            icon,
            &contents
                .join("Resources")
                .join(format!("{}.icns", spec.name)),
        )?;
    }
    for exe in &spec.extras.executables {
        copy_any(exe, &contents.join("MacOS").join(file_name(exe)?))?;
    }
    for (res, name) in &spec.extras.resources {
        copy_any(res, &contents.join("Resources").join(name))?;
    }
    let frameworks = contents.join("Frameworks");
    let fw = frameworks.join(FRAMEWORK);
    clone_dir(&src_fw, &fw)?;
    for suffix in HELPER_SUFFIXES {
        let name = helper_name(spec.name, suffix);
        let id = helper_id(spec.identifier, suffix);
        app_skeleton(
            &frameworks.join(format!("{name}.app")),
            &name,
            spec.helper_exe,
            &info_plist(&name, &id, spec.version, true, false, &[]),
        )?;
    }
    sign_bundle(&app, spec.name, spec.identifier, &spec.signing, &scratch)?;
    Ok(app)
}

/// Whether `p` is under `dir` (or is it).
fn under(p: &Path, dir: &Path) -> bool {
    p.starts_with(dir)
}

/// Signs a bundle of this layout inside out, whoever made its parts (a Velopack package adds its update program
/// to `Contents/MacOS`, rule 270): any Mach-O outside the frameworks, the helpers and `Contents/MacOS`; each
/// framework's own Mach-O files, then the framework; the five helpers; the other programs in `Contents/MacOS`
/// (identifier `<identifier>.<file>`); the app. A helper app CEF does not know is refused.
pub fn sign_bundle(
    app: &Path,
    name: &str,
    identifier: &str,
    s: &Signing,
    scratch: &Path,
) -> Result<(), String> {
    let contents = app.join("Contents");
    let frameworks_dir = contents.join("Frameworks");
    let macos = contents.join("MacOS");
    let main = macos.join(name);
    if !main.is_file() {
        return Err(format!("no {} in {}", main.display(), app.display()));
    }
    let mut frameworks = Vec::new();
    let mut helper_apps = Vec::new();
    if let Ok(rd) = fs::read_dir(&frameworks_dir) {
        for e in rd.flatten() {
            let p = e.path();
            match p.extension().and_then(|x| x.to_str()) {
                Some("framework") => frameworks.push(p),
                Some("app") => helper_apps.push(p),
                _ => {}
            }
        }
    }
    frameworks.sort();
    let mut helpers = Vec::new();
    for h in &helper_apps {
        let suffix = HELPER_SUFFIXES
            .iter()
            .find(|sfx| {
                h.file_name()
                    .is_some_and(|n| n == format!("{}.app", helper_name(name, sfx)).as_str())
            })
            .ok_or_else(|| format!("{}: not a helper CEF starts", h.display()))?;
        helpers.push((h.clone(), *suffix));
    }
    helpers.sort_by_key(|(_, sfx)| HELPER_SUFFIXES.iter().position(|x| x == sfx));
    let all = sign::machos_in(app);
    for stray in all.iter().filter(|p| {
        !under(p, &macos)
            && !frameworks.iter().any(|f| under(p, f))
            && !helper_apps.iter().any(|h| under(p, h))
    }) {
        sign::sign(stray, s, Role::Library, None, scratch)?;
    }
    for fw in &frameworks {
        let stem = fw.file_stem().and_then(|n| n.to_str()).unwrap_or_default();
        for inner in sign::machos_in(fw) {
            let own = inner.file_name().is_some_and(|n| n == stem)
                && (inner.parent() == Some(fw.as_path())
                    || inner
                        .parent()
                        .and_then(Path::parent)
                        .is_some_and(|v| v.ends_with("Versions")));
            if !own {
                sign::sign(&inner, s, Role::Library, None, scratch)?;
            }
        }
        sign::sign(fw, s, Role::Library, None, scratch)?;
    }
    for (h, suffix) in &helpers {
        let id = helper_id(identifier, suffix);
        sign::sign(h, s, Role::Helper(suffix), Some(id.as_str()), scratch)?;
    }
    for t in all
        .iter()
        .filter(|p| p.parent() == Some(macos.as_path()) && **p != main)
    {
        let id = format!("{identifier}.{}", file_name(t)?);
        sign::sign(t, s, Role::Tool, Some(&id), scratch)?;
    }
    sign::sign(app, s, Role::App, Some(identifier), scratch)
}

/// The role of a Mach-O by its place in `app` (the app's name is its folder's, `Flare.app`): the app's executable,
/// a helper's executable, another program in `Contents/MacOS`, or a library (anything else).
pub fn role_of(app: &Path, file: &Path) -> Role {
    let name = app.file_stem().and_then(|n| n.to_str()).unwrap_or_default();
    let macos = app.join("Contents/MacOS");
    if file.parent() == Some(macos.as_path()) {
        return if file.file_name().is_some_and(|n| n == name) {
            Role::App
        } else {
            Role::Tool
        };
    }
    for sfx in HELPER_SUFFIXES {
        let h = app
            .join("Contents/Frameworks")
            .join(format!("{}.app", helper_name(name, sfx)))
            .join("Contents/MacOS");
        if file.parent() == Some(h.as_path()) {
            return Role::Helper(sfx);
        }
    }
    Role::Library
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
        let h = info_plist(
            "Flare Helper (GPU)",
            "dev.flare.helper.gpu",
            "0.1.0",
            true,
            false,
            &[],
        );
        assert!(h.contains("<key>LSUIElement</key>\n  <true/>"));
        assert!(!h.contains("LSBackgroundOnly"));
        assert!(h.contains("<string>Flare Helper (GPU)</string>"));
        let p = info_plist(
            "Flare & <Probe>",
            "dev.flare.probe",
            "0.1.0",
            false,
            true,
            &[],
        );
        assert!(p.contains("<key>LSBackgroundOnly</key>\n  <true/>"));
        assert!(!p.contains("LSUIElement"));
        assert!(p.contains("Flare &amp; &lt;Probe&gt;"));
        assert!(p.contains("<key>LSMinimumSystemVersion</key>\n  <string>15.0</string>"));
    }

    #[test]
    fn a_mach_os_role_is_read_from_its_place_in_the_bundle() {
        let app = Path::new("/x/Flare.app");
        assert_eq!(role_of(app, &app.join("Contents/MacOS/Flare")), Role::App);
        assert_eq!(
            role_of(app, &app.join("Contents/MacOS/flare-mcp")),
            Role::Tool
        );
        assert_eq!(
            role_of(app, &app.join("Contents/MacOS/UpdateMac")),
            Role::Tool
        );
        assert_eq!(
            role_of(
                app,
                &app.join(
                    "Contents/Frameworks/Flare Helper (GPU).app/Contents/MacOS/Flare Helper (GPU)"
                )
            ),
            Role::Helper("Helper (GPU)")
        );
        assert_eq!(
            role_of(
                app,
                &app.join("Contents/Frameworks/Flare Helper.app/Contents/MacOS/Flare Helper")
            ),
            Role::Helper("Helper")
        );
        assert_eq!(
            role_of(
                app,
                &app.join("Contents/Frameworks/Chromium Embedded Framework.framework/Libraries/libEGL.dylib")
            ),
            Role::Library
        );
    }

    #[test]
    fn any_bundle_of_this_layout_is_signed_inside_out_with_programs_it_did_not_make() {
        let dir = std::env::temp_dir().join(format!("flare-signbundle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let app = dir.join("Flare.app");
        let exe = Path::new("/usr/bin/true");
        app_skeleton(
            &app,
            "Flare",
            exe,
            &info_plist("Flare", "dev.flare.t", "1.0", false, true, &[]),
        )
        .expect("app");
        let h = app.join("Contents/Frameworks/Flare Helper (GPU).app");
        app_skeleton(
            &h,
            "Flare Helper (GPU)",
            exe,
            &info_plist(
                "Flare Helper (GPU)",
                "dev.flare.t.helper.gpu",
                "1.0",
                true,
                false,
                &[],
            ),
        )
        .expect("helper");
        // What a packager adds after the build: an update program beside the app.
        fs::copy(exe, app.join("Contents/MacOS/UpdateMac")).expect("tool");
        let scratch = dir.join("ent");
        sign_bundle(&app, "Flare", "dev.flare.t", &Signing::AdHoc, &scratch).expect("signs");
        verify(&app).expect("the seal holds with the added program");
        for f in [
            app.join("Contents/MacOS/UpdateMac"),
            h.join("Contents/MacOS/Flare Helper (GPU)"),
        ] {
            let out = Command::new("/usr/bin/codesign")
                .args(["-dv"])
                .arg(&f)
                .output()
                .expect("codesign");
            let text = String::from_utf8_lossy(&out.stderr).into_owned();
            assert!(text.contains("Signature=adhoc"), "{text}");
        }
        // A helper app CEF does not start is refused, not signed blind.
        fs::create_dir_all(app.join("Contents/Frameworks/Other.app/Contents/MacOS")).expect("dir");
        let e = sign_bundle(&app, "Flare", "dev.flare.t", &Signing::AdHoc, &scratch)
            .expect_err("unknown helper");
        assert!(e.contains("Other.app"), "{e}");
        let _ = fs::remove_dir_all(&dir);
    }
}
