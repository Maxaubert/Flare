//! Native e2e harness: the sandbox, process control and the registry sandbox (M9 Task 36), and the
//! M10 driver. Task 1 creates the M10 modules as stubs; Tasks 12 to 15 and the port tasks fill them.

pub mod chord;
#[cfg(windows)]
pub mod ctx;
#[cfg(windows)]
pub mod dictproc;
pub mod fake_fill_helper;
pub mod fixtures;
#[cfg(windows)]
pub mod guards;
#[cfg(windows)]
pub mod perf;
pub mod pixels;
#[cfg(windows)]
pub mod proc;
#[cfg(windows)]
pub mod regsandbox;
pub mod report;
pub mod rowpage;
#[cfg(windows)]
pub mod runner;
#[cfg(windows)]
pub mod sandbox;
#[cfg(windows)]
pub mod scenarios;
pub mod session;
pub mod slide;
pub mod standins;
#[cfg(windows)]
pub mod tour;
pub mod until;
#[cfg(windows)]
pub mod winshot;
#[cfg(windows)]
pub mod world;
