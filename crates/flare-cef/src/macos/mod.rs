//! The CEF engine on macOS. See the crate docs.

pub mod app;
pub mod bundle;
pub mod context;
pub mod engine;
pub mod helper;
pub mod keychain;
pub mod panels;
pub mod probe;
pub mod release;
pub mod screen;
pub mod sign;
pub mod view;

/// The CEF folder cef-dll-sys built against (its `links` metadata, through `build.rs`): the framework the dev
/// bundle copies.
pub const CEF_DIR: &str = env!("FLARE_CEF_DIR");
