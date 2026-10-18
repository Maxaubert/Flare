//! M9 Task 32: ShipCtl, the Velopack bootstrap, the verb, the modes and the flags. Filled by that task.

/// The version the Velopack first-run hook reported, if this launch is the first after an install.
/// Declared by Task 31 (the diagnostics log writes the `first-run` crumb from it); Task 32 fills it from
/// the hook inside `bootstrap()` and keeps this declaration.
pub static FIRST_RUN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
