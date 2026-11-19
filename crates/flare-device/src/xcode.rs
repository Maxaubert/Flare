//! Which Xcode the device panel uses (spec "Setup", state 2). Flare picks it on a worker and starts the sim host with
//! `DEVELOPER_DIR` set to it, so the sim host's `simctl` calls and its dlopen use one Xcode.
//!
//! The order: `xcode-select -p` when it points inside an `.app`; else the newest `Xcode*.app` found (in
//! `/Applications`, `~/Applications`, and by Launch Services for `com.apple.dt.Xcode`). The Command Line Tools alone
//! count as no Xcode (they have no simulator). Flare never runs `xcode-select -s` (it needs admin).
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

/// The Xcode to use. `selected` is what `xcode-select -p` printed (None when it printed nothing); `found` are the
/// Xcode apps found, each with its version (the selected app among them when it was readable).
pub fn pick(selected: Option<&Path>, found: &[Xcode]) -> Option<Xcode> {
    if let Some(app) = selected.and_then(app_of)
        && let Some(x) = found.iter().find(|x| x.app == app)
    {
        return Some(x.clone());
    }
    found
        .iter()
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

/// Asks this Mac which Xcode to use. BLOCKING (`xcode-select`, `mdfind` and file reads, each with a limit): call it
/// on a worker, never on the UI thread (rule 34).
#[cfg(target_os = "macos")]
pub fn find() -> Option<Xcode> {
    use std::time::Duration;

    use crate::macos::simctl::run;

    let limit = Duration::from_secs(5);
    let selected = run(
        // sync-ok: the Xcode pick's worker, at most 5 s
        std::process::Command::new("/usr/bin/xcode-select").arg("-p"),
        None,
        limit,
    )
    .ok()
    .map(|o| PathBuf::from(String::from_utf8_lossy(&o).trim()))
    .filter(|p| !p.as_os_str().is_empty());
    let mut apps: Vec<PathBuf> = Vec::new();
    if let Some(app) = selected.as_deref().and_then(app_of) {
        apps.push(app);
    }
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Applications"));
    }
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue; // sync-ok: the Xcode pick's worker
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("Xcode") && name.ends_with(".app") {
                apps.push(e.path());
            }
        }
    }
    // Launch Services' list of every app with Xcode's identifier (an Xcode kept elsewhere).
    if let Ok(o) = run(
        // sync-ok: the Xcode pick's worker, at most 5 s
        std::process::Command::new("/usr/bin/mdfind")
            .arg("kMDItemCFBundleIdentifier == 'com.apple.dt.Xcode'"),
        None,
        limit,
    ) {
        apps.extend(
            String::from_utf8_lossy(&o)
                .lines()
                .map(str::trim)
                .filter(|l| l.ends_with(".app"))
                .map(PathBuf::from),
        );
    }
    apps.sort();
    apps.dedup();
    let found: Vec<Xcode> = apps
        .into_iter()
        .filter_map(|app| {
            let version = version_of(&app)?;
            Some(Xcode { app, version })
        })
        .collect();
    pick(selected.as_deref(), &found)
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
}
