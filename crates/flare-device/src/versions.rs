//! The tested versions and the input path (rule 1313). The sim host reads the loaded CoreSimulator's and
//! SimulatorKit's `CFBundleVersion` and Xcode's version; this table says what was tried and which input path a
//! CoreSimulator takes. A newer, untested version is tried anyway (with a log line); an Xcode under the floor goes
//! straight to still pictures.

use serde::{Deserialize, Serialize};

/// The oldest Xcode the live view runs on (Xcode 26: best effort through legacy Indigo, untested; decision 8).
pub const XCODE_FLOOR: u32 = 26;
/// CoreSimulator from this version on takes DTUHID input; below it, legacy Indigo. Never both in one boot.
pub const DTUHID_FROM: &str = "1155.4";
/// The line still mode shows for an Xcode under the floor.
pub const TOO_OLD: &str = "This Xcode is too old for the live view.";

/// A version tried on a real Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tested {
    pub xcode: &'static str,
    pub coresim: &'static str,
    pub simkit: &'static str,
    /// Where and when it was tried.
    pub note: &'static str,
}

/// The tested table.
pub const TESTED: &[Tested] = &[Tested {
    xcode: "27.2",
    coresim: "1174.21",
    simkit: "1005.14",
    note: "Mac mini M6, macOS 27.2, iPhone 18 Pro on iOS 27.2, spikes S1 to S6, 2026-11-19",
}];

/// How the sim host sends input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputPath {
    /// XPC to the CoreDevice digitizer service (CoreSimulator 1155.4 and newer).
    Dtuhid,
    /// `SimDeviceLegacyHIDClient` (older CoreSimulator).
    Indigo,
}

/// A dotted version (`1174.21`, `27.2`), compared part by part. None for anything else.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(Vec<u32>);

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let parts = s
            .trim()
            .split('.')
            .map(|p| {
                (!p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| p.parse().ok())
                    .flatten()
            })
            .collect::<Option<Vec<u32>>>()?;
        let mut parts = parts;
        // 1174.21 and 1174.21.0 are one version.
        while parts.len() > 1 && parts.last() == Some(&0) {
            parts.pop();
        }
        Some(Version(parts))
    }

    pub fn major(&self) -> u32 {
        self.0.first().copied().unwrap_or(0)
    }
}

/// The input path a CoreSimulator version takes. An unreadable version is taken as new (the current Xcode's).
pub fn input_path(coresim_version: &str) -> InputPath {
    match (Version::parse(coresim_version), Version::parse(DTUHID_FROM)) {
        (Some(v), Some(from)) if v < from => InputPath::Indigo,
        _ => InputPath::Dtuhid,
    }
}

/// What the table says of a set of versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// In the table.
    Tested,
    /// Not in the table but not under the floor: tried, with a log line.
    Untested,
    /// Xcode under the floor (or unreadable): still pictures, [`TOO_OLD`].
    TooOld,
}

/// What the table says of an Xcode and a CoreSimulator.
pub fn support(xcode_version: &str, coresim_version: &str) -> Support {
    let Some(x) = Version::parse(xcode_version) else {
        return Support::TooOld;
    };
    if x.major() < XCODE_FLOOR {
        return Support::TooOld;
    }
    let tested = TESTED.iter().any(|t| {
        Version::parse(t.xcode) == Some(x.clone())
            && Version::parse(t.coresim) == Version::parse(coresim_version)
    });
    if tested {
        Support::Tested
    } else {
        Support::Untested
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_coresimulator_uses_indigo_and_new_uses_dtuhid() {
        assert_eq!(input_path("1174.21"), InputPath::Dtuhid, "this Mac");
        assert_eq!(input_path("1155.4"), InputPath::Dtuhid, "from 1155.4 on");
        assert_eq!(input_path("1155.4.0"), InputPath::Dtuhid);
        assert_eq!(
            input_path("1155.10"),
            InputPath::Dtuhid,
            "compared as numbers, not text"
        );
        assert_eq!(input_path("1155.3.9"), InputPath::Indigo);
        assert_eq!(input_path("1155"), InputPath::Indigo);
        assert_eq!(
            input_path("1051.17.7"),
            InputPath::Indigo,
            "an Xcode 26 era build"
        );
        assert_eq!(input_path("2000"), InputPath::Dtuhid);
        assert_eq!(
            input_path(""),
            InputPath::Dtuhid,
            "unreadable: the current path"
        );
        assert_eq!(input_path("abc"), InputPath::Dtuhid);
        // The table, and the floor.
        assert_eq!(support("27.2", "1174.21"), Support::Tested);
        assert_eq!(support("27.2.0", "1174.21.0"), Support::Tested);
        assert_eq!(support("27.3", "1180.1"), Support::Untested);
        assert_eq!(
            support("26.4", "1051.17.7"),
            Support::Untested,
            "best effort"
        );
        assert_eq!(support("16.4", "1010.1"), Support::TooOld);
        assert_eq!(support("", "1174.21"), Support::TooOld);
        for bad in ["", ".", "1..2", "1.a", "-1", "1.2.", "99999999999"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        assert!(
            TESTED
                .iter()
                .all(|t| input_path(t.coresim) == InputPath::Dtuhid)
        );
    }
}
