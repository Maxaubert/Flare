//! flare-theme, see the roadmap crate table for its role. Pure: no runtime dependencies.

pub mod agent_colours;
pub mod ansi;
pub mod chrome;
pub mod colour;
pub mod colour_contrast;
pub mod edges;
pub mod find_ink;
pub mod fonts;
pub mod fonts_data;
pub mod ground;
pub mod knob;
pub mod links;
pub mod look;
pub mod marks;
pub mod num;
pub mod oklab;
pub mod order;
pub mod preset;
pub mod presets_data;
pub mod reports;
pub mod resolve;
pub mod retired;
pub mod see_through;
pub mod sidebar;

#[cfg(test)]
mod testutil;
