//! Native e2e harness: the sandbox, process control and the registry sandbox (M9 Task 36), and the
//! M10 driver. Task 1 creates the M10 modules as stubs; Tasks 12 to 15 and the port tasks fill them.

pub mod chord;
#[cfg(any(windows, target_os = "macos"))]
pub mod ctx;
#[cfg(windows)]
pub mod dictproc;
pub mod fake_fill_helper;
pub mod fixtures;
#[cfg(any(windows, target_os = "macos"))]
pub mod guards;
#[cfg(windows)]
pub mod perf;
pub mod pixels;
#[cfg(windows)]
pub mod proc;
// macOS (rule 335): the same API over process groups, `ps` and signals.
#[cfg(target_os = "macos")]
#[path = "proc_mac.rs"]
pub mod proc;
// macOS: the window server list, the frontmost app and the pasteboard count (rule 335).
#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(windows)]
pub mod regsandbox;
pub mod report;
pub mod rowpage;
#[cfg(any(windows, target_os = "macos"))]
pub mod runner;
#[cfg(any(windows, target_os = "macos"))]
pub mod sandbox;
#[cfg(any(windows, target_os = "macos"))]
pub mod scenarios;
pub mod session;
pub mod slide;
pub mod standins;
#[cfg(windows)]
pub mod tour;
pub mod until;
#[cfg(windows)]
pub mod winshot;
#[cfg(any(windows, target_os = "macos"))]
pub mod world;
