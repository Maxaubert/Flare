//! The 39 terminal themes (data generated in presets_data.rs from PT core/renderer/lib/termTheme.ts).

use crate::presets_data::PRESETS;

/// One theme as Prism Terminal lists it. `ansi` is the curated sixteen in ANSI order, or None for a derived palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub bg: &'static str,
    pub fg: &'static str,
    pub cursor: &'static str,
    pub accent: Option<&'static str>,
    pub ansi: Option<[&'static str; 16]>,
}

/// The core's fallback theme id (PT's `DEFAULT_TERM_THEME`).
pub const DEFAULT_TERM_THEME: &str = "prism";

/// Flare's default theme (PT `src/renderer/src/termHost.ts` defaults.theme).
pub const HOST_DEFAULT_THEME: &str = "pt-default";

/// All 39 presets in Prism Terminal's order; `pt-default` leads.
pub fn presets() -> &'static [Preset] {
    &PRESETS
}

/// The preset with this id, if there is one.
pub fn find_preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::{contrast_ratio, ANSI_KEYS};
    use crate::ground::DEFAULT_GROUND;
    use crate::testutil::fixture;
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn there_are_thirty_nine_presets_with_unique_ids_and_names() {
        let all = presets();
        assert_eq!(all.len(), 39);
        let ids: HashSet<&str> = all.iter().map(|p| p.id).collect();
        let names: HashSet<&str> = all.iter().map(|p| p.name).collect();
        assert_eq!(ids.len(), 39);
        assert_eq!(names.len(), 39);
    }

    #[test]
    fn has_the_brown_pink_and_green_pairs() {
        for id in ["umber", "fawn", "rosewood", "blossom", "moss", "sage"] {
            assert!(find_preset(id).is_some(), "{id} is missing");
        }
    }

    #[test]
    fn pt_default_leads_the_list_and_prism_is_the_core_fallback() {
        assert_eq!(presets()[0].id, "pt-default");
        assert!(find_preset(DEFAULT_TERM_THEME).is_some());
        assert_eq!(HOST_DEFAULT_THEME, presets()[0].id);
    }

    #[test]
    fn presets_data_equals_the_oracle() {
        let json = fixture("presets.json");
        let rows = json.as_array().expect("presets.json is an array");
        assert_eq!(rows.len(), presets().len());
        for (p, row) in presets().iter().zip(rows) {
            let s = |k: &str| row[k].as_str().unwrap_or_else(|| panic!("{k} of {}", p.id));
            assert_eq!(p.id, s("id"));
            assert_eq!(p.name, s("name"));
            assert_eq!(p.bg, s("bg"), "{} bg", p.id);
            assert_eq!(p.fg, s("fg"), "{} fg", p.id);
            assert_eq!(p.cursor, s("cursor"), "{} cursor", p.id);
            assert_eq!(
                p.accent,
                row.get("accent").and_then(|a| a.as_str()),
                "{} accent",
                p.id
            );
            match row.get("ansi").filter(|a| a.is_object()) {
                Some(obj) => {
                    let want: Vec<&str> = ANSI_KEYS
                        .iter()
                        .map(|k| obj[k].as_str().unwrap_or_else(|| panic!("{} {k}", p.id)))
                        .collect();
                    assert_eq!(
                        p.ansi.expect("ansi present").to_vec(),
                        want,
                        "{} ansi",
                        p.id
                    );
                }
                None => assert!(p.ansi.is_none(), "{} has no ansi in the oracle", p.id),
            }
        }
    }

    #[test]
    fn every_presets_text_reads_on_its_ground() {
        for p in presets() {
            let r = contrast_ratio(p.fg, p.bg);
            assert!(r >= 4.5, "{}: text {r:.2} on {}", p.id, p.bg);
        }
    }

    #[test]
    fn every_presets_cursor_and_accent_can_be_seen_on_its_ground() {
        for p in presets() {
            let r = contrast_ratio(p.cursor, p.bg);
            assert!(r >= 3.0, "{}: cursor {r:.2} on {}", p.id, p.bg);
            if let Some(a) = p.accent {
                let r = contrast_ratio(a, p.bg);
                assert!(r >= 3.0, "{}: accent {r:.2} on {}", p.id, p.bg);
            }
        }
    }

    #[test]
    fn pitch_is_orange_on_black() {
        let p = find_preset("pitch").expect("pitch");
        assert_eq!(p.bg, "#000000");
        assert_eq!(p.cursor, "#ec9448");
        assert_eq!(p.accent, Some("#ec9448"));
    }

    #[test]
    fn cinder_is_orange_on_the_icons_dark_grey() {
        let p = find_preset("cinder").expect("cinder");
        assert_eq!(p.bg, "#383c44");
        assert_eq!(p.cursor, "#ec9448");
        assert_eq!(p.accent, Some("#ec9448"));
    }

    #[test]
    fn the_prism_preset_ground_is_the_default_ground() {
        assert_eq!(find_preset("prism").expect("prism").bg, DEFAULT_GROUND);
    }

    /// Every `.rs` file under `crates/*/src`, as (workspace-relative path, text before its test module).
    /// The test module starts at the first `#[cfg(test)]`, so only the code that ships is scanned.
    fn shipped_sources() -> Vec<(String, String)> {
        let root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let mut files = Vec::new();
        for entry in fs::read_dir(root.join("crates")).expect("crates dir") {
            let src = entry.expect("entry").path().join("src");
            if src.is_dir() {
                collect_rs(&src, &mut files);
            }
        }
        files.sort();
        files
            .into_iter()
            .map(|p| {
                let text = fs::read_to_string(&p).expect("read source");
                let shipped = match text.find("#[cfg(test)]") {
                    Some(i) => text[..i].to_string(),
                    None => text,
                };
                let rel = p
                    .strip_prefix(&root)
                    .expect("under the workspace root")
                    .to_string_lossy()
                    .replace('\\', "/");
                (rel, shipped)
            })
            .collect()
    }

    fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn no_source_file_types_the_default_ground_but_the_allowed_ones() {
        // The ground's single home, and the knob and the generated data that name it.
        const OWNERS: [&str; 3] = [
            "crates/flare-theme/src/ground.rs",
            "crates/flare-theme/src/knob.rs",
            "crates/flare-theme/src/presets_data.rs",
        ];
        // M2's own copy of the fallback ground (flare-term). Task 11c deletes it with its luminance copy.
        const ALLOWED_UNTIL_11C: [&str; 1] = ["crates/flare-term/src/replies.rs"];
        for (name, text) in shipped_sources() {
            if OWNERS.contains(&name.as_str()) || ALLOWED_UNTIL_11C.contains(&name.as_str()) {
                continue;
            }
            let lower = text.to_lowercase();
            for needle in ["#0b0b0f", "#101215"] {
                assert!(
                    !lower.contains(needle),
                    "{name} types the default ground {needle}; use flare_theme::ground::DEFAULT_GROUND"
                );
            }
        }
    }

    #[test]
    fn no_source_compares_a_luminance_with_a_typed_0_4() {
        // Everyone compares with ground::LIGHT_GROUND. Task 11c deletes the replies.rs entry.
        const ALLOWED_UNTIL_11C: [&str; 1] = ["crates/flare-term/src/replies.rs"];
        for (name, text) in shipped_sources() {
            if name == "crates/flare-theme/src/ground.rs"
                || ALLOWED_UNTIL_11C.contains(&name.as_str())
            {
                continue;
            }
            for line in text.lines() {
                let typed = line.contains("> 0.4") || line.contains("<= 0.4");
                assert!(
                    !(line.contains("luminance(") && typed),
                    "{name} compares a luminance with a typed 0.4: {}; use ground::LIGHT_GROUND",
                    line.trim()
                );
            }
        }
    }
}
