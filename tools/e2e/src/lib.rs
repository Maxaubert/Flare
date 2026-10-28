//! Native e2e harness: the sandbox, process control and the registry sandbox (M9 Task 36), and the
//! M10 driver. Task 1 creates the M10 modules as stubs; Tasks 12 to 15 and the port tasks fill them.

pub mod chord;
pub mod ctx;
pub mod dictproc;
pub mod fake_fill_helper;
pub mod fixtures;
pub mod guards;
pub mod perf;
pub mod pixels;
pub mod proc;
pub mod regsandbox;
pub mod report;
pub mod rowpage;
pub mod runner;
pub mod sandbox;
pub mod scenarios;
pub mod session;
pub mod slide;
pub mod standins;
pub mod tour;
pub mod until;
pub mod winshot;
pub mod world;
