//! Which device a panel shows: its target, stored in `panels.json` and named on the control channel (rule 1316).
//!
//! A simulator is `sim/<udid>`, the udid a 36-character UUID (`8-4-4-4-12` hex digits), always written lower-cased.
//! `simctl` prints udids in upper case; both read, the name is lower-cased so one device is one target. The target
//! obeys the same limits as every panel target (`flare_surfaces::state::clean_target`). Only the shape is checked
//! here: whether the device still exists is a `simctl` question, asked on a worker (rule 34).

use flare_surfaces::state::clean_target;

/// The prefix of a simulator's target.
pub const SIM_PREFIX: &str = "sim/";
/// The length of a udid.
pub const UDID_LEN: usize = 36;

/// A device a panel can show. Android (a second source) adds a variant later.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    /// An iOS simulator, by its udid (lower-cased).
    Sim { udid: String },
}

impl Target {
    /// A simulator by its udid as `simctl` prints it, or None for anything that is not a udid.
    pub fn sim(udid: &str) -> Option<Target> {
        valid_udid(udid).then(|| Target::Sim {
            udid: udid.to_ascii_lowercase(),
        })
    }

    /// The target's stored name: `sim/<udid>`.
    pub fn name(&self) -> String {
        match self {
            Target::Sim { udid } => format!("{SIM_PREFIX}{udid}"),
        }
    }

    /// The udid `simctl` takes.
    pub fn udid(&self) -> &str {
        match self {
            Target::Sim { udid } => udid,
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

/// The target a stored or sent name means, or None for anything not shaped `sim/<udid>` (rule 40: dropped, never
/// guessed at). The limits of every panel target apply first.
pub fn parse(s: &str) -> Option<Target> {
    let s = clean_target(s)?;
    Target::sim(s.strip_prefix(SIM_PREFIX)?)
}

/// Whether `s` is a target this build reads, by its shape alone (`target_ok` for the device kind, rule 1316): no
/// file is read and no simulator is asked.
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
        assert_eq!(t.udid(), UDID);
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
}
