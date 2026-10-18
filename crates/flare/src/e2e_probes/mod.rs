//! The read-only probes the test hook answers from: one file per scenario group, plus core (Task 11).
//! Each file's `r`egister adds its probes to the registry. Nothing here runs unless --e2e is on,
//! because 2e_glue::install is the only caller and it runs only under --e2e.
pub mod agent_hooks;
pub mod agent_launch;
pub mod agent_misc;
pub mod chrome;
pub mod closeask;
pub mod core;
pub mod diag;
pub mod dpi;
pub mod help;
pub mod indicators;
pub mod input_a;
pub mod input_b;
pub mod input_c;
pub mod input_d;
pub mod lifecycle;
pub mod settings_a;
pub mod settings_b;
pub mod settings_c;
pub mod settings_d;
pub mod strip;
pub mod update;
pub mod vertical;

/// The table of named probes. Task 11 gives it its entries.
#[derive(Debug, Default)]
pub struct ProbeRegistry;

/// Adds every group's probes to `r`.
pub fn register_all(r: &mut ProbeRegistry) {
    agent_hooks::register(r);
    agent_launch::register(r);
    agent_misc::register(r);
    chrome::register(r);
    closeask::register(r);
    core::register(r);
    diag::register(r);
    dpi::register(r);
    help::register(r);
    indicators::register(r);
    input_a::register(r);
    input_b::register(r);
    input_c::register(r);
    input_d::register(r);
    lifecycle::register(r);
    settings_a::register(r);
    settings_b::register(r);
    settings_c::register(r);
    settings_d::register(r);
    strip::register(r);
    update::register(r);
    vertical::register(r);
}
