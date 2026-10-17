//! Retired theme ids and where their wearers go (Task 5).

use crate::presets_data::RETIRED;

/// The retired id's replacement, or None when the id is live (or unknown).
pub fn retired(id: &str) -> Option<&'static str> {
    RETIRED
        .iter()
        .find(|(old, _)| *old == id)
        .map(|(_, new)| *new)
}

/// The id a wearer of `id` ends up on: the replacement when retired, else the id itself.
pub fn live_theme_id(id: &str) -> &str {
    match retired(id) {
        Some(new) => new,
        None => id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::find_preset;
    use crate::presets_data::RETIRED;
    use crate::testutil::fixture;

    #[test]
    fn each_retired_goes_to_a_listed_theme_and_none_is_listed() {
        assert_eq!(RETIRED.len(), 31);
        for (old, new) in RETIRED {
            assert!(
                find_preset(new).is_some(),
                "{old} goes to {new}, which is not a preset"
            );
            assert!(
                find_preset(old).is_none(),
                "{old} is retired but still listed"
            );
        }
    }

    #[test]
    fn a_kept_id_is_itself() {
        assert_eq!(live_theme_id("pitch"), "pitch");
    }

    #[test]
    fn espresso_reads_as_umber_and_github_as_paper() {
        assert_eq!(live_theme_id("espresso"), "umber");
        assert_eq!(live_theme_id("github"), "paper");
    }

    #[test]
    fn retired_map_equals_the_oracle() {
        let json = fixture("retired.json");
        let map = json.as_object().expect("retired.json is an object");
        assert_eq!(map.len(), RETIRED.len());
        for (old, new) in RETIRED {
            assert_eq!(map.get(old).and_then(|v| v.as_str()), Some(new), "{old}");
        }
    }
}
