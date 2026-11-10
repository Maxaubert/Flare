//! Surfaces, stage 1: panels (no Slint, no Win32).
//!
//! A tab can show content beside its terminal. This crate holds everything about a panel that is
//! not drawing: the kinds, the per-tab state, how it is stored, the layout maths of the slot, the
//! overlay (airspace) decision and the previews (what a path may open as, how an image or a text
//! file is read). The window code in `flare` draws it and feeds it.
//!
//! Rules it holds: 13 and 14 (a path is re-resolved by the backend and a runnable file is never
//! shown as anything but text or refused), 34 (every read here is for a worker thread), 36, 40 and
//! 41 (the state is stored atomically, unknown values read as the default and are never written
//! back, a failed store keeps the defaults).

pub mod agent_cursor;
pub mod agent_job;
pub mod agent_web;
pub mod app;
pub mod audio;
pub mod devpreview;
pub mod kind;
pub mod layout;
pub mod mdview;
pub mod model;
pub mod modview;
pub mod net;
pub mod netproxy;
pub mod notify;
pub mod overlay;
pub mod preview;
pub mod slot;
pub mod state;
pub mod store;
pub mod web;
pub mod wrap;

pub use kind::PanelKind;
pub use state::{TabPanel, TabPanels};
