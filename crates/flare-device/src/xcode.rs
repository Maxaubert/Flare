//! Which Xcode the device panel uses (spec "Setup", state 2). Flare picks it on a worker and starts the sim host with
//! `DEVELOPER_DIR` set to it, so the sim host's `simctl` calls and its dlopen use one Xcode.
//!
//! The order: `xcode-select -p` when it points inside an `.app`; else the newest `Xcode*.app` at the top of
//! `/Applications`. Nothing else is looked at (rule 1325): not `~/Applications`, not Spotlight, so an `Xcode.app`
//! someone left in Downloads is never a candidate. Every candidate must also be signed by Apple
//! ([`crate::macos::codesign`]); one that is not is skipped with a log line, and with none left Flare has no Xcode.
//! The Command Line Tools alone count as no Xcode (they have no simulator). Flare never runs `xcode-select -s` (it
//! needs admin).
//!
//! [`pick`] is pure; [`find`] (macOS) asks the system, BLOCKING, on a worker (rule 34). Used by Flare's side in PR 4b.

use std::path::{Path, PathBuf};

use crate::versions::Version;

/// An Xcode found on this Mac.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Xcode {
    /// The app: `/Applications/Xcode.app`.
    pub app: PathBuf,
    /// Its `CFBundleShortVersionString`: "27.2".
    pub version: String,
}

impl Xcode {
    /// The folder `DEVELOPER_DIR` names: `<app>/Contents/Developer`.
    pub fn developer_dir(&self) -> PathBuf {
        self.app.join("Contents").join("Developer")
    }
}

/// The `.app` a path is inside (`/Applications/Xcode.app/Contents/Developer` is in `/Applications/Xcode.app`), or
/// None (the Command Line Tools, `/Library/Developer/CommandLineTools`, are in no app).
pub fn app_of(path: &Path) -> Option<PathBuf> {
    let mut app = PathBuf::new();
    for c in path.components() {
        app.push(c);
        if c.as_os_str()
            .to_str()
            .is_some_and(|s| s.len() > 4 && s.ends_with(".app"))
        {
            return Some(app);
        }
    }
    None
}

/// The folder whose top-level `Xcode*.app` apps are candidates (rule 1325).
pub const APPLICATIONS: &str = "/Applications";

/// True for `/Applications/Xcode*.app`, at the top of the folder (never one inside another folder).
pub fn in_applications(app: &Path) -> bool {
    app.parent() == Some(Path::new(APPLICATIONS))
        && app
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("Xcode") && n.ends_with(".app"))
}

/// The Xcode to use. `selected` is what `xcode-select -p` printed (None when it printed nothing); `found` are the
/// Xcode apps found, each with its version (the selected app among them when it was readable). Only the selected app
/// and `/Applications/Xcode*.app` are ever picked (rule 1325); any other path in `found` is ignored.
pub fn pick(selected: Option<&Path>, found: &[Xcode]) -> Option<Xcode> {
    if let Some(app) = selected.and_then(app_of)
        && let Some(x) = found.iter().find(|x| x.app == app)
    {
        return Some(x.clone());
    }
    found
        .iter()
        .filter(|x| in_applications(&x.app))
        .filter_map(|x| Version::parse(&x.version).map(|v| (v, x)))
        .max_by(|(a, xa), (b, xb)| a.cmp(b).then_with(|| xb.app.cmp(&xa.app)))
        .map(|(_, x)| x.clone())
}

/// The string under `key` in an XML property list (Xcode's `Contents/version.plist`), or None.
pub fn plist_string(xml: &str, key: &str) -> Option<String> {
    let k = format!("<key>{key}</key>");
    let rest = &xml[xml.find(&k)? + k.len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("<string>")?;
    let end = rest.find("</string>")?;
    let v = rest[..end].trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// The version of the Xcode app at `app`, from its `Contents/version.plist`. BLOCKING (a file read): a worker only.
pub fn version_of(app: &Path) -> Option<String> {
    let xml = std::fs::read_to_string(app.join("Contents").join("version.plist")).ok()?; // sync-ok: the Xcode pick's worker
    plist_string(&xml, "CFBundleShortVersionString")
}

/// Asks this Mac which Xcode to use. BLOCKING (`xcode-select`, file reads and the signature checks, each bounded):
/// call it on a worker, never on the UI thread (rule 34). Candidates are the `xcode-select` app and
/// `/Applications/Xcode*.app` only; each must pass [`crate::macos::codesign::xcode_app`] and its SimulatorKit
/// [`crate::macos::codesign::apple_framework`], or it is skipped with a warning (rule 1325).
#[cfg(target_os = "macos")]
pub fn find() -> Option<Xcode> {
    use std::time::Duration;

    use crate::macos::simctl::run;

    let selected = run(
        // sync-ok: the Xcode pick's worker, at most 5 s
        std::process::Command::new("/usr/bin/xcode-select").arg("-p"),
        None,
        Duration::from_secs(5),
    )
    .ok()
    .map(|o| PathBuf::from(String::from_utf8_lossy(&o).trim()))
    .filter(|p| !p.as_os_str().is_empty());
    let mut apps: Vec<PathBuf> = Vec::new();
    if let Some(app) = selected.as_deref().and_then(app_of) {
        apps.push(app);
    }
    if let Ok(rd) = std::fs::read_dir(APPLICATIONS) {
        // sync-ok: the Xcode pick's worker
        apps.extend(
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| in_applications(p)),
        );
    }
    apps.sort();
    apps.dedup();
    let found: Vec<Xcode> = apps
        .into_iter()
        .filter_map(|app| {
            let version = version_of(&app)?;
            if let Err(e) = verify(&app) {
                log::warn!("device panel: skipping Xcode {version}: {e}");
                return None;
            }
            Some(Xcode { app, version })
        })
        .collect();
    pick(selected.as_deref(), &found)
}

/// The Xcode app at `app` and the SimulatorKit in it are Apple's (rule 1325). BLOCKING (it reads the code).
#[cfg(target_os = "macos")]
pub fn verify(app: &Path) -> Result<(), String> {
    use crate::macos::{codesign, load};

    codesign::xcode_app(app)?;
    let dev = app.join("Contents").join("Developer");
    let simkit = load::simkit_paths(&dev)
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| format!("SimulatorKit missing in {}", app.display()))?;
    codesign::apple_framework(&simkit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(app: &str, version: &str) -> Xcode {
        Xcode {
            app: PathBuf::from(app),
            version: version.into(),
        }
    }

    #[test]
    fn command_line_tools_alone_are_no_xcode() {
        let clt = Path::new("/Library/Developer/CommandLineTools");
        assert_eq!(app_of(clt), None);
        assert_eq!(pick(Some(clt), &[]), None);
        assert_eq!(pick(None, &[]), None);
        // An app whose version cannot be read is no Xcode either.
        assert_eq!(pick(Some(clt), &[x("/Applications/Xcode.app", "")]), None);
    }

    #[test]
    fn the_newest_xcode_app_is_picked_when_xcode_select_points_elsewhere() {
        let found = [
            x("/Applications/Xcode-26.app", "26.4"),
            x("/Applications/Xcode.app", "27.2"),
            x("/Applications/Xcode-beta.app", "27.10"),
        ];
        let clt = Path::new("/Library/Developer/CommandLineTools");
        assert_eq!(
            pick(Some(clt), &found),
            Some(found[2].clone()),
            "27.10 is newer than 27.2"
        );
        assert_eq!(pick(None, &found), Some(found[2].clone()));
        // xcode-select inside an app wins, even an older one.
        let sel = Path::new("/Applications/Xcode-26.app/Contents/Developer");
        assert_eq!(
            app_of(sel),
            Some(PathBuf::from("/Applications/Xcode-26.app"))
        );
        assert_eq!(pick(Some(sel), &found), Some(found[0].clone()));
        assert_eq!(
            found[0].developer_dir(),
            PathBuf::from("/Applications/Xcode-26.app/Contents/Developer")
        );
        // Selected but gone: the newest found.
        let gone = Path::new("/Volumes/Old/Xcode.app/Contents/Developer");
        assert_eq!(pick(Some(gone), &found), Some(found[2].clone()));
        // The version is read from Xcode's version.plist.
        let plist = "<plist version=\"1.0\">\n<dict>\n\t<key>CFBundleShortVersionString</key>\n\t<string>27.2</string>\n\t<key>CFBundleVersion</key>\n\t<string>25400.63.5</string>\n</dict>\n</plist>";
        assert_eq!(
            plist_string(plist, "CFBundleShortVersionString").as_deref(),
            Some("27.2")
        );
        assert_eq!(plist_string(plist, "ProductBuildVersion"), None);
        assert_eq!(plist_string("<key>A</key><integer>1</integer>", "A"), None);
    }

    /// This Mac's Xcode (when `/Applications` has one) is found and passes the signature checks.
    #[cfg(target_os = "macos")]
    #[test]
    fn find_takes_this_macs_signed_xcode() {
        let any = std::fs::read_dir(APPLICATIONS)
            .map(|rd| rd.flatten().any(|e| in_applications(&e.path())))
            .unwrap_or(false);
        if !any {
            eprintln!("skipped: no Xcode in /Applications");
            return;
        }
        let x = find().expect("a signed Xcode in /Applications is found");
        verify(&x.app).unwrap();
    }

    /// Rule 1325: an Xcode anywhere but the top of `/Applications` is never picked by version, however new; the
    /// `xcode-select` app (chosen by an admin) still is.
    #[test]
    fn pick_ignores_xcodes_outside_applications() {
        let found = [
            x("/Applications/Xcode.app", "27.2"),
            x("/Users/max/Downloads/Xcode.app", "99.0"),
            x("/Users/max/Applications/Xcode.app", "98.0"),
            x("/Applications/Old/Xcode.app", "97.0"),
            x("/Volumes/Ext/Xcode-beta.app", "96.0"),
            x("/Applications/NotXcode.app", "95.0"),
        ];
        assert_eq!(pick(None, &found), Some(found[0].clone()));
        let clt = Path::new("/Library/Developer/CommandLineTools");
        assert_eq!(pick(Some(clt), &found), Some(found[0].clone()));
        assert_eq!(pick(None, &found[1..]), None);
        let sel = Path::new("/Volumes/Ext/Xcode-beta.app/Contents/Developer");
        assert_eq!(pick(Some(sel), &found), Some(found[4].clone()));
        assert!(in_applications(Path::new("/Applications/Xcode-26.app")));
        assert!(!in_applications(Path::new("/Applications/Old/Xcode.app")));
        assert!(!in_applications(Path::new(
            "/Applications/Xcode.app/Contents"
        )));
    }
}
