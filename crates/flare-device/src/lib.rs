//! The device panel (issue #248): a live iPhone simulator beside a tab, seen and used by the person and the agent.
//!
//! This crate holds everything about the panel that is not drawing and not the private simulator API: which device
//! a panel shows ([`target`]), what a source of devices can do ([`source`]) and a fake one for tests and `--e2e`
//! ([`fake`]), the 60 fps cap and the ownership of the frame ring ([`cap`]), coordinates ([`geom`]), the US keymap
//! ([`keymap`]), the UI tree filter ([`tree`]), the agent's touch indicators ([`indicator`]), the agent's job
//! ([`job`]), the sim host's wire protocol ([`wire`]) and the tested versions ([`versions`]).
//!
//! Coordinates outside the sim host are always device points in the current orientation (what the UI tree reports
//! and what the agent sees in `device_look`); only the sim host converts them to the portrait ratios the digitizer
//! takes ([`geom::to_portrait_ratio`]).
//!
//! PR 1 of the plan (`docs/superpowers/plans/2026-11-19-device-panel.md`): pure code, nothing wired, so the app does
//! not change. Rules held here: 1316 (the `sim/<udid>` target and the kind's shape check).
#![cfg_attr(not(test), allow(dead_code))]

pub mod cap;
pub mod fake;
pub mod geom;
pub mod indicator;
pub mod job;
pub mod keymap;
pub mod source;
pub mod target;
pub mod tree;
pub mod versions;
pub mod wire;

pub use source::{DeviceInfo, DeviceSource, DeviceState, SourceEvent};
pub use target::Target;
