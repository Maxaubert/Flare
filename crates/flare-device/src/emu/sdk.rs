//! Where the Android SDK is and whether its emulator is new enough (rule 1361).
//!
//! The person's own SDK comes first (`ANDROID_HOME`, `ANDROID_SDK_ROOT`, then the usual folder:
//! `%LOCALAPPDATA%\Android\Sdk` or `~/Library/Android/sdk`): Flare uses it when its emulator is at the floor, and only
//! reads it. Otherwise Flare's own folder (`android-sdk` in Flare's folder, filled by Android plan PR A2b). Flare
//! never writes, updates or deletes anything in the person's SDK.
//!
//! Floors (spec finding 10): 37.2.13 on Windows (the GLES memory leak fix; API 37 images need 37.2.11), 37.3.10 on
//! macOS (App Nap lag; MMAP was broken on the Mac before 37.2.12).

use std::path::{Path, PathBuf};

use super::Places;
use crate::versions::Version;

pub const FLOOR_WINDOWS: &str = "37.2.13";
pub const FLOOR_MACOS: &str = "37.3.10";

/// This OS's floor.
pub fn floor() -> &'static str {
    if cfg!(target_os = "macos") {
        FLOOR_MACOS
    } else {
        FLOOR_WINDOWS
    }
}

/// Whether `version` is at or above `floor` (false for a version that does not read).
pub fn at_floor(version: &str, floor: &str) -> bool {
    match (Version::parse(version), Version::parse(floor)) {
        (Some(v), Some(f)) => v >= f,
        _ => false,
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// The emulator in an SDK.
pub fn emulator_exe(root: &Path) -> PathBuf {
    root.join("emulator").join(exe("emulator"))
}

/// adb in an SDK.
pub fn adb_exe(root: &Path) -> PathBuf {
    root.join("platform-tools").join(exe("adb"))
}

/// `Pkg.Revision` of a package's `source.properties` (`37.3.10`), None when it is missing or not a version.
pub fn revision(props: &str) -> Option<String> {
    let v = super::parse_ini(props).remove("Pkg.Revision")?;
    Version::parse(&v).is_some().then_some(v)
}

/// A folder that may hold an SDK.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub root: PathBuf,
    /// Flare's own folder (Flare may write it, A2b); false for the person's.
    pub flare_own: bool,
}

/// The folders to look in, the person's first, each once.
pub fn candidates(p: &Places) -> Vec<Candidate> {
    let usual = if cfg!(target_os = "macos") {
        p.home.as_ref().map(|h| h.join("Library/Android/sdk"))
    } else {
        p.local_app_data.as_ref().map(|l| l.join("Android/Sdk"))
    };
    let mut out: Vec<Candidate> = Vec::new();
    for root in [p.android_home.clone(), p.android_sdk_root.clone(), usual]
        .into_iter()
        .flatten()
    {
        if !out.iter().any(|c| c.root == root) {
            out.push(Candidate {
                root,
                flare_own: false,
            });
        }
    }
    if let Some(root) = p.flare_sdk()
        && !out.iter().any(|c| c.root == root)
    {
        out.push(Candidate {
            root,
            flare_own: true,
        });
    }
    out
}

/// An SDK with an emulator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sdk {
    pub root: PathBuf,
    pub flare_own: bool,
    /// The emulator's `Pkg.Revision`.
    pub emulator_version: String,
}

impl Sdk {
    pub fn emulator(&self) -> PathBuf {
        emulator_exe(&self.root)
    }

    pub fn adb(&self) -> PathBuf {
        adb_exe(&self.root)
    }
}

/// What the search found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// An SDK whose emulator is at the floor.
    Ready(Sdk),
    /// Only emulators under the floor: the newest of them (the panel offers [Update Android emulator], A2b).
    TooOld(Sdk),
    /// No emulator in any candidate.
    Missing,
}

/// The first candidate whose emulator is at `floor`; else the newest one under it; else Missing. `version_of` gives
/// a candidate's emulator version (None: no emulator there).
pub fn pick(
    cands: &[Candidate],
    floor: &str,
    version_of: impl Fn(&Path) -> Option<String>,
) -> Found {
    let mut old: Option<Sdk> = None;
    for c in cands {
        let Some(v) = version_of(&c.root) else {
            continue;
        };
        let sdk = Sdk {
            root: c.root.clone(),
            flare_own: c.flare_own,
            emulator_version: v,
        };
        if at_floor(&sdk.emulator_version, floor) {
            return Found::Ready(sdk);
        }
        let newer = old.as_ref().is_none_or(|o| {
            Version::parse(&sdk.emulator_version) > Version::parse(&o.emulator_version)
        });
        if newer {
            old = Some(sdk);
        }
    }
    old.map_or(Found::Missing, Found::TooOld)
}

/// BLOCKING (reads two small files): the emulator version in `root`, None without an emulator there.
pub fn read_version(root: &Path) -> Option<String> {
    if !emulator_exe(root).is_file() {
        return None;
    }
    let props = std::fs::read_to_string(root.join("emulator/source.properties")).ok()?; // sync-ok: called on a worker or in the emu host
    revision(&props)
}

/// BLOCKING: the SDK this machine uses (see [`pick`]).
pub fn find(p: &Places) -> Found {
    pick(&candidates(p), floor(), read_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floors_are_per_os_and_compare_part_by_part() {
        assert!(at_floor("37.2.13", FLOOR_WINDOWS));
        assert!(at_floor("37.3.10", FLOOR_WINDOWS));
        assert!(at_floor("38.0", FLOOR_MACOS));
        assert!(!at_floor("37.2.12", FLOOR_WINDOWS));
        assert!(!at_floor("37.2.13", FLOOR_MACOS), "the Mac needs 37.3.10");
        assert!(!at_floor("37.3.9", FLOOR_MACOS));
        assert!(!at_floor("abc", FLOOR_WINDOWS));
        assert_eq!(
            floor(),
            if cfg!(target_os = "macos") {
                "37.3.10"
            } else {
                "37.2.13"
            }
        );
        assert_eq!(
            revision("Pkg.UserSrc=false\nPkg.Revision=37.2.13\nPkg.Path=emulator\n").as_deref(),
            Some("37.2.13")
        );
        assert_eq!(revision("Pkg.Revision=x\n"), None);
        assert_eq!(revision(""), None);
    }

    /// Rule 1361: the person's SDK is used when it is at the floor; Flare's own folder only otherwise.
    #[test]
    fn the_persons_sdk_comes_first_when_it_is_new_enough() {
        let p = Places {
            android_home: Some("/a/home".into()),
            android_sdk_root: Some("/a/home".into()),
            local_app_data: Some("/l".into()),
            home: Some("/h".into()),
            ..Places::default()
        };
        let c = candidates(&p);
        assert_eq!(c[0].root, PathBuf::from("/a/home"));
        assert!(c.iter().filter(|c| c.root == Path::new("/a/home")).count() == 1);
        assert!(!c[0].flare_own);
        assert!(c.last().unwrap().flare_own, "Flare's own folder is last");
        assert_eq!(c.iter().filter(|c| c.flare_own).count(), 1);
        let own = c.last().unwrap().root.clone();
        let usual = c[1].root.clone();
        let versions = |m: Vec<(PathBuf, &'static str)>| {
            move |r: &Path| {
                m.iter()
                    .find(|(p, _)| p == r)
                    .map(|(_, v)| (*v).to_string())
            }
        };
        let f = FLOOR_WINDOWS;
        // The person's at the floor: theirs.
        let got = pick(
            &c,
            f,
            versions(vec![(usual.clone(), "37.3.10"), (own.clone(), "37.3.10")]),
        );
        assert!(matches!(&got, Found::Ready(s) if s.root == usual && !s.flare_own));
        // The person's under the floor, Flare's at it: Flare's.
        let got = pick(
            &c,
            f,
            versions(vec![(usual.clone(), "36.1.9"), (own.clone(), "37.3.10")]),
        );
        assert!(matches!(&got, Found::Ready(s) if s.root == own && s.flare_own));
        // Both under: the newer one, as too old.
        let got = pick(
            &c,
            f,
            versions(vec![(usual.clone(), "36.1.9"), (own.clone(), "37.1.0")]),
        );
        assert!(matches!(&got, Found::TooOld(s) if s.root == own), "{got:?}");
        assert_eq!(pick(&c, f, |_: &Path| None), Found::Missing);
    }

    #[test]
    fn an_sdk_folder_is_read_for_its_emulator_version() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read_version(d.path()), None, "no emulator");
        std::fs::create_dir_all(d.path().join("emulator")).unwrap();
        std::fs::write(
            d.path().join("emulator/source.properties"),
            "Pkg.Revision=37.3.10\n",
        )
        .unwrap();
        assert_eq!(read_version(d.path()), None, "no emulator program");
        std::fs::write(emulator_exe(d.path()), b"").unwrap();
        assert_eq!(read_version(d.path()).as_deref(), Some("37.3.10"));
        assert!(adb_exe(d.path()).ends_with(if cfg!(windows) {
            "platform-tools/adb.exe"
        } else {
            "platform-tools/adb"
        }));
    }
}
