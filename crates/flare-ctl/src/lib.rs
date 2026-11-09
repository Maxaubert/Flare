//! The Flare control channel, protocol v1: one local, per-user, versioned channel that the UI, the
//! agent tools (`flare-mcp`) and mods all use to show content and drive panels.
//!
//! - `proto`: wire types, error codes, parsing.
//! - `method`: the method names and their timeouts.
//! - `grant`: who may call what (rule S3 and friends), a pure function.
//! - `decor`: the badge icons, tones, keys and limits of `tabs.badge.*` and `tabs.progress.*`.
//! - `framing`: JSON lines over a [`framing::Wire`], with a size cap.
//! - `session`: one connection's server side, over any wire; the [`session::Dispatcher`] the app
//!   implements.
//! - `client`: the client library, over any wire.
//! - `text`: words from outside made safe for a model (one copy for both sides).
//! - `pipe`: the endpoint, one API on both systems. Windows (`pipe.rs`): the named pipe, its
//!   owner-only descriptor, the accept loop, `connect`. macOS (`uds.rs`): a Unix socket in a 0700
//!   per-user folder, mode 0600, the peer's uid and pid checked on both ends (rule 243). Any other
//!   system: `pipe_stub.rs`, no channel.
//!
//! The contract is `docs/surfaces/protocol.md`; this crate must stay app-neutral so the same rules
//! can be ported to Prism Terminal.

pub mod client;
pub mod decor;
pub mod framing;
pub mod grant;
pub mod method;
#[cfg(windows)]
pub mod pipe;
#[cfg(target_os = "macos")]
#[path = "uds.rs"]
pub mod pipe;
#[cfg(not(any(windows, target_os = "macos")))]
#[path = "pipe_stub.rs"]
pub mod pipe;
pub mod proto;
pub mod session;
pub mod text;
#[cfg(windows)]
mod winid;

pub use client::{Client, ClientError, HelloInfo, HelloParams};
pub use grant::{Grant, Permission};
pub use method::{Method, Namespace};
pub use pipe::{
    ENV_PIPE, PipeEnd, Server, connect, connect_default, connect_to_server, default_pipe_name,
    pipe_name,
};
pub use proto::{CtlError, Event, MAX_LINE, PROTOCOL, code};
pub use session::{CallContext, Dispatcher, ServerConfig};
