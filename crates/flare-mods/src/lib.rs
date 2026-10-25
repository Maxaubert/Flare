//! Mods, Surfaces stage 8, the library part: no UI, no Win32 window, no WebView2.
//!
//! A mod is a folder under `%APPDATA%\Flare\mods\<id>\` with a `mod.json` manifest that declares what
//! it may do (`docs/surfaces/mods.md` is the contract). A plugin (protocol 2) is the same with a `plugin.json`,
//! usually under `%APPDATA%\Flare\plugins\<id>\`; the two folders share one id space. This crate holds
//! everything about a mod that is not drawing it:
//!
//! - `manifest`: the strict parse and validation of `mod.json` and `plugin.json` (every rule, with a
//!   human-readable reason).
//! - `paths`: the syntax of a path inside a mod folder, and the canonical containment check (no escape
//!   by `..`, a drive, a UNC path or a link).
//! - `discover`: the scan of the mods and plugins folders (bounded, per-folder errors, worker-callable).
//! - `enabled`: which mods the user turned on (`mods.json`, atomic, unknown entries dropped), with what a
//!   plugin said it was when it was approved.
//! - `hash`: the content hash of a plugin folder kept with its approval.
//! - `tokens`: the in-memory token of each enabled mod (256 bits from the system RNG, never stored),
//!   and the pure map from a token to the mod's `Grant` (rule S5).
//! - `tools`: the names the agent sees (`mod_<id>_<tool>`) and the table that routes a call to its mod,
//!   with collisions detected and both sides dropped.
//! - `serve`: the answer to one request for a file of the mod's page (checked on every request, with the
//!   header policy).
//! - `bridge`: the `window.flare` script the host injects, and the messages between the page and the host.
//! - `hub`: everything the app knows about mods behind one lock (discovery, switches, tokens, the tool table
//!   and the calls waiting on a mod's page), still with no window and no browser.
//!
//! Every disk call here runs on the caller's thread, which is a worker in the app (rule 34). The
//! store and the scan never touch the UI thread.

pub mod bridge;
pub mod discover;
pub mod enabled;
pub mod hash;
pub mod hub;
pub mod manifest;
pub mod paths;
pub mod serve;
pub mod tokens;
pub mod tools;

pub use discover::{Discovery, FolderError, Found, Root, discover, discover_all};
pub use enabled::EnabledStore;
pub use hub::Hub;
pub use manifest::{
    Kind, Manifest, PROTOCOL_V1, PROTOCOL_V2, parse as parse_manifest, parse_plugin,
};
pub use tokens::{Tokens, mod_grant};
pub use tools::{Collision, Route, TooLong, ToolTable, tool_name};
