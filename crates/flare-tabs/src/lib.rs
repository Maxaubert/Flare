//! flare-tabs: the pure tab logic of Flare. The tab list and its rules, the strip's plan and
//! device-pixel layout, the drag state machine, the chord table, the sidebar's reveal and slide,
//! the restore planner, tabs.json, the close-flow state and the badges and progress callers put on a tab. Time is passed in, never read.

pub mod close_flow;
pub mod decor;
pub mod drag_flight;
pub mod fit_hold;
pub mod folder_names;
pub mod hidden_title;
pub mod keymap;
#[cfg(not(windows))]
mod macos;
pub mod modal;
pub mod model;
pub mod places;
pub mod restore;
pub mod reveal;
pub mod sidebar_rows;
pub mod sidebar_state;
pub mod strip_layout;
pub mod strip_plan;
pub mod tab_drag;
pub mod tab_drop;
pub mod tab_mru;
pub mod tabs;
pub mod tabs_store;
pub mod tip_place;
