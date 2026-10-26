//! Mods, Surfaces stage 8, the library part: no UI, no Win32 window, no WebView2.
//!
//! A mod is a folder under `%APPDATA%\Flare\plugins\<id>\` with a `mod.json` manifest that declares what
//! it may do (`docs/surfaces/mods.md` is the contract). A plugin (protocol 2) is the same with a `plugin.json`, in
//! the same folder: there is one folder for every plugin. This crate holds everything about a mod that is not
//! drawing it:
//!
//! - `manifest`: the strict parse and validation of `mod.json` and `plugin.json` (every rule, with a
//!   human-readable reason).
//! - `paths`: the syntax of a path inside a mod folder, and the canonical containment check (no escape
//!   by `..`, a drive, a UNC path or a link).
//! - `discover`: the scan of the plugins folder (bounded, per-folder errors, worker-callable).
//! - `migrate`: the one-time move of the old `mods` folder's folders into the plugins folder.
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
//! - `storage`: each plugin's own small JSON store (`storage.get`, `storage.set`), written atomically on a worker.
//! - `install`: a plugin from a zip (staged, checked, then put in place by rename), its update and its removal.
//! - `fetch`: the download of a plugin zip from an `https` address the person gave (the one network use here).
//! - `helper`: a plugin's helper program, in a job object, with its log and its restarts.
//!
//! Every disk call here runs on the caller's thread, which is a worker in the app (rule 34). The
//! store and the scan never touch the UI thread.

pub mod bridge;
pub mod discover;
pub mod enabled;
pub mod fetch;
pub mod hash;
pub mod helper;
pub mod hub;
pub mod install;
pub mod limit;
pub mod manifest;
pub mod migrate;
pub mod paths;
pub mod serve;
pub mod storage;
pub mod tokens;
pub mod tools;

pub use discover::{Discovery, FolderError, Found, discover};
pub use enabled::EnabledStore;
pub use hub::Hub;
pub use manifest::{
    Kind, Manifest, PROTOCOL_V1, PROTOCOL_V2, parse as parse_manifest, parse_plugin,
};
pub use tokens::{Tokens, mod_grant};
pub use tools::{Collision, Route, TooLong, ToolTable, tool_name};
