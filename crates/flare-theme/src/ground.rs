//! The ground's constants, once for the whole app (PT core/shared/ground.ts, audit 2026-10-12).

/// The ground when none can be read: the `prism` preset's.
pub const DEFAULT_GROUND: &str = "#0b0b0f";

/// Relative luminance above which a ground is LIGHT (regression rule 15: measured, never read off a name).
pub const LIGHT_GROUND: f64 = 0.4;

/// The dark ink chosen where white would not read: the ground's own near-black.
pub const ON_DARK_INK: &str = DEFAULT_GROUND;

/// The ink and the cursor of a Custom theme that names neither (PT `paletteOf`'s fallbacks).
pub const DEFAULT_INK: &str = "#e7e7ee";
pub const DEFAULT_CURSOR: &str = "#5b5bd6";
