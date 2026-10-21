//! Task 57: the light presets' resolved palettes, written once for `tools/glass-light/contrast.py`.
//! The table is a golden: it must equal what the resolver gives today. `BLESS_GLASS_LIGHT=1 cargo test
//! -p flare-theme --test glass_light` rewrites it after a deliberate change to the resolver.

use flare_theme::ansi::luminance;
use flare_theme::ground::LIGHT_GROUND;
use flare_theme::preset::presets;
use flare_theme::resolve::resolve_preset;
use std::path::PathBuf;

fn table_path() -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "tools",
        "glass-light",
        "palettes.csv",
    ]
    .iter()
    .collect()
}

fn table() -> String {
    let mut out = String::from("id,bg,fg");
    for i in 0..16 {
        out.push_str(&format!(",a{i}"));
    }
    out.push('\n');
    for p in presets() {
        if luminance(p.bg) <= LIGHT_GROUND {
            continue;
        }
        let t = resolve_preset(p);
        out.push_str(&format!("{},{},{}", p.id, t.background, t.foreground));
        for i in 0..16 {
            out.push(',');
            out.push_str(t.ansi_at(i).unwrap_or(""));
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_palette_table_equals_what_the_resolver_gives() {
    let want = table();
    let path = table_path();
    if std::env::var("BLESS_GLASS_LIGHT").is_ok() {
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(
        have.replace("\r\n", "\n"),
        want,
        "run with BLESS_GLASS_LIGHT=1"
    );
}

#[test]
fn there_are_light_presets_to_measure() {
    let n = table().lines().count() - 1;
    assert!(n >= 3, "only {n} light presets");
}
