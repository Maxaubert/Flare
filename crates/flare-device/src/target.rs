//! Which device a panel shows: its target, stored in `panels.json` and named on the control channel (rules 1316 and
//! 1360).
//!
//! A simulator is `sim/<udid>`, the udid a 36-character UUID (`8-4-4-4-12` hex digits), always written lower-cased.
//! `simctl` prints udids in upper case; both read, the name is lower-cased so one device is one target.
//!
//! An Android emulator is `emu/<avd>`, the name of its AVD (Android Virtual Device). The AVD name is stable; the adb
//! serial changes with each boot, so it is never part of the target. A valid AVD name is 1 to 64 of `A-Z a-z 0-9 . _
//! -`, not only dots, not starting with a dash, and keeps its case (the emulator's `-avd` flag and its files are
//! case-sensitive on macOS).
//!
//! The target obeys the same limits as every panel target (`flare_surfaces::state::clean_target`). Only the shape is
//! checked here: whether the device still exists is a question for its source, asked on a worker (rule 34).

use flare_surfaces::state::clean_target;

use crate::source::SourceKind;

/// The prefix of a simulator's target.
pub const SIM_PREFIX: &str = "sim/";
/// The prefix of an Android emulator's target.
pub const EMU_PREFIX: &str = "emu/";
/// The length of a udid.
pub const UDID_LEN: usize = 36;
/// The longest AVD name a target takes.
pub const AVD_MAX: usize = 64;

/// A device a panel can show.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    /// An iOS simulator, by its udid (lower-cased).
    Sim { udid: String },
    /// An Android emulator, by its AVD name (case kept).
    Emu { avd: String },
}

impl Target {
    /// A simulator by its udid as `simctl` prints it, or None for anything that is not a udid.
    pub fn sim(udid: &str) -> Option<Target> {
        valid_udid(udid).then(|| Target::Sim {
            udid: udid.to_ascii_lowercase(),
        })
    }

    /// An Android emulator by its AVD name, or None for a name the emulator would not take (rule 1360).
    pub fn emu(avd: &str) -> Option<Target> {
        valid_avd(avd).then(|| Target::Emu {
            avd: avd.to_string(),
        })
    }

    /// The target's stored name: `sim/<udid>` or `emu/<avd>`.
    pub fn name(&self) -> String {
        match self {
            Target::Sim { udid } => format!("{SIM_PREFIX}{udid}"),
            Target::Emu { avd } => format!("{EMU_PREFIX}{avd}"),
        }
    }

    /// The id the device's own tools take: the udid for `simctl`, the AVD name for `emulator -avd`.
    pub fn id(&self) -> &str {
        match self {
            Target::Sim { udid } => udid,
            Target::Emu { avd } => avd,
        }
    }

    /// Which source shows this device.
    pub fn source(&self) -> SourceKind {
        match self {
            Target::Sim { .. } => SourceKind::Sim,
            Target::Emu { .. } => SourceKind::Emu,
        }
    }

    /// A name for the person when the source has not named the device yet (for example "No virtual device named
    /// Pixel 8 on this computer."): an AVD's name with its underscores as spaces, as Android Studio shows it. None for
    /// a simulator, whose udid says nothing; its name comes from `simctl`.
    pub fn display_name(&self) -> Option<String> {
        match self {
            Target::Sim { .. } => None,
            Target::Emu { avd } => Some(avd.replace('_', " ")),
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name())
    }
}

impl serde::Serialize for Target {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.name())
    }
}

impl<'de> serde::Deserialize<'de> for Target {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Target, D::Error> {
        let s = String::deserialize(d)?;
        parse(&s).ok_or_else(|| serde::de::Error::custom(format!("not a device target: {s:?}")))
    }
}

/// The target a stored or sent name means, or None for anything not shaped `sim/<udid>` or `emu/<avd>` (rule 40:
/// dropped, never guessed at). The limits of every panel target apply first.
pub fn parse(s: &str) -> Option<Target> {
    let s = clean_target(s)?;
    if let Some(udid) = s.strip_prefix(SIM_PREFIX) {
        return Target::sim(udid);
    }
    Target::emu(s.strip_prefix(EMU_PREFIX)?)
}

/// Whether `s` is a target this build reads, by its shape alone (`target_ok` for the device kind, rule 1316): no
/// file is read and no device is asked.
pub fn target_ok(s: &str) -> bool {
    parse(s).is_some_and(|t| t.name() == s)
}

/// `8-4-4-4-12` ASCII hex digits.
fn valid_udid(s: &str) -> bool {
    s.len() == UDID_LEN
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// 1 to [`AVD_MAX`] of `A-Z a-z 0-9 . _ -`, not only dots (so never `.` or `..`, which would name a folder), and not
/// starting with a dash (which `emulator -avd` would read as a flag).
fn valid_avd(s: &str) -> bool {
    (1..=AVD_MAX).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !s.bytes().all(|b| b == b'.')
        && !s.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const UDID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

    #[test]
    fn a_sim_target_round_trips_and_bad_udids_are_refused() {
        let t = parse(&format!("sim/{UDID}")).unwrap();
        assert_eq!(t, Target::Sim { udid: UDID.into() });
        assert_eq!(t.name(), format!("sim/{UDID}"));
        assert_eq!(parse(&t.name()), Some(t.clone()));
        assert_eq!(t.id(), UDID);
        assert_eq!(t.source(), SourceKind::Sim);
        assert_eq!(t.display_name(), None);
        assert_eq!(t.to_string(), t.name());
        assert!(target_ok(&t.name()));
        // simctl's upper case reads as the same device; only the lower-cased name is a stored target.
        let upper = UDID.to_ascii_uppercase();
        assert_eq!(Target::sim(&upper), Some(t.clone()));
        assert_eq!(parse(&format!("sim/{upper}")), Some(t.clone()));
        assert!(
            !target_ok(&format!("sim/{upper}")),
            "stored lower-cased only"
        );
        for bad in [
            String::new(),
            UDID.to_string(),
            format!("sim:{UDID}"),
            format!("SIM/{UDID}"),
            format!("sim/{UDID}/"),
            format!("sim/ {UDID}"),
            format!("sim/{}", &UDID[..35]),
            format!("sim/{UDID}0"),
            format!("sim/{}", UDID.replace('-', "")),
            format!("sim/{}", UDID.replacen('0', "g", 1)),
            format!("sim/{}", UDID.replacen('-', "_", 1)),
            format!("sim/{}", "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f\u{e9}"),
            format!("sim/{UDID}\0"),
            "sim/../../etc/passwd".to_string(),
            "C:\\a\\b.png".to_string(),
            "https://example.com".to_string(),
        ] {
            assert_eq!(parse(&bad), None, "{bad:?}");
            assert!(!target_ok(&bad), "{bad:?}");
        }
        assert_eq!(Target::sim("not-a-udid"), None);
    }

    /// Rule 1360: an Android emulator is `emu/<avd>`, the AVD name as the emulator takes it, case kept.
    #[test]
    fn an_emu_target_round_trips_and_bad_avd_names_are_refused() {
        let t = parse("emu/Pixel_8").unwrap();
        assert_eq!(
            t,
            Target::Emu {
                avd: "Pixel_8".into()
            }
        );
        assert_eq!(t.name(), "emu/Pixel_8");
        assert_eq!(t.id(), "Pixel_8");
        assert_eq!(t.source(), SourceKind::Emu);
        assert_eq!(t.display_name().as_deref(), Some("Pixel 8"));
        assert!(target_ok("emu/Pixel_8"));
        assert_eq!(t.to_string(), "emu/Pixel_8");
        // Case is kept: two names, two devices.
        assert_ne!(parse("emu/pixel_8"), Some(t.clone()));
        assert!(target_ok("emu/pixel_8"));
        // Every allowed character, and the longest name.
        for good in [
            "a",
            "Flare_Pixel",
            "flare-test-1",
            "Medium_Phone_API_36.1",
            "uc-api37",
            ".hidden",
            &"x".repeat(AVD_MAX),
        ] {
            let t = Target::emu(good).unwrap_or_else(|| panic!("{good:?}"));
            assert!(target_ok(&t.name()), "{good:?}");
            assert_eq!(parse(&t.name()), Some(t));
        }
        // It serialises as its name, both ways.
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#""emu/Pixel_8""#);
        assert_eq!(serde_json::from_str::<Target>(&json).unwrap(), t);
        for bad in [
            "emu/".to_string(),
            "emu".to_string(),
            "Pixel_8".to_string(),
            "EMU/Pixel_8".to_string(),
            "emu:Pixel_8".to_string(),
            "emu/Pixel 8".to_string(),
            "emu/Pixel_8/".to_string(),
            "emu/a/b".to_string(),
            "emu/a\\b".to_string(),
            "emu/.".to_string(),
            "emu/..".to_string(),
            "emu/...".to_string(),
            "emu/../avd".to_string(),
            "emu/P\u{ef}xel".to_string(),
            "emu/Pixel\0".to_string(),
            "emu/Pixel;rm".to_string(),
            // A leading dash would read as a flag on the emulator's command line.
            "emu/-no-window".to_string(),
            "emu/\u{2212}avd".to_string(),
            format!("emu/{}", "x".repeat(AVD_MAX + 1)),
        ] {
            assert_eq!(parse(&bad), None, "{bad:?}");
            assert!(!target_ok(&bad), "{bad:?}");
        }
        assert_eq!(Target::emu(""), None);
        // A udid is a valid AVD name too, but under `emu/` it is an emulator, never a simulator.
        assert_eq!(
            parse(&format!("emu/{UDID}")).unwrap().source(),
            SourceKind::Emu
        );
    }
}
