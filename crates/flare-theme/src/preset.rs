//! The 39 terminal themes (data generated in presets_data.rs from PT core/renderer/lib/termTheme.ts).

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
