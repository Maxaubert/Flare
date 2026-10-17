//! flare-settings: atomic file writes, the debounced file, and the preference store that holds
//! the settings of the window (tabs, sidebar, new tab, recent and pinned folders).

pub mod atomic;
pub mod custom_theme;
pub mod debounce;
pub mod kv;
pub mod look;
pub mod new_tab;
pub mod recent_roots;
pub mod sidebar;
pub mod tab_layout;
pub mod tab_switch;
pub mod term_prefs;
pub mod window_colours;
