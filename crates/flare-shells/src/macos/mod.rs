//! The macOS shells: zsh, bash, fish and PowerShell, each started as a login shell, with Flare's
//! folder report (OSC 7) injected without touching the user's dotfiles (rule 227: the Mac code
//! lives here, the shared modules call in through small seams).
//!
//! - `detect`: which shells are installed (`/etc/shells`, the login shell from the user record,
//!   Homebrew), the login shell first.
//! - `integration`: the per-shell hook files and where they are installed.
//! - `osc7`: reading an OSC 7 `file://host/path` report (Flare's own hooks send no host).
//! - `env`: what a Mac shell's environment gets on top of the shared `pty_env`.
//! - `cd`: the `cd` line a tab types into a POSIX shell.
//! - `paths`: which folders reach a server, and the folder-follow rule, for POSIX paths.

pub mod cd;
pub mod detect;
pub mod env;
pub mod integration;
pub mod osc7;
pub mod paths;
pub mod sys;
