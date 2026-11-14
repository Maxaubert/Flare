//! flare-settings: atomic file writes, the debounced file, and the preference store that holds
//! the settings of the window (tabs, sidebar, new tab, recent and pinned folders).

pub mod actions;
pub mod agent_prefs;
pub mod app_icon;
pub mod atomic;
pub mod colour_format;
pub mod copy_rules;
pub mod custom_theme;
pub mod debounce;
pub mod find;
pub mod find_state;
pub mod help_prefs;
pub mod icons;
pub mod import;
pub mod kv;
pub mod layout;
pub mod look;
pub mod machine_prefs;
pub mod new_tab;
pub mod page;
pub mod pages;
pub mod picker;
pub mod recent_roots;
pub mod rows;
pub mod sections;
pub mod sidebar;
pub mod tab_layout;
pub mod tab_switch;
pub mod term_prefs;
pub mod verb;
pub mod wall;
pub mod window_colours;
