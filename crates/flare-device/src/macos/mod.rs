//! The macOS half of the device panel: the sim host's private-API code (rule 1310: only `flare-sim-host` runs it,
//! Flare's own process never loads CoreSimulator or SimulatorKit) and the mach channel both sides share.
//!
//! - [`codesign`]: is this Xcode or framework Apple's (rule 1325), checked before any dlopen.
//! - [`load`]: dlopen the frameworks, read their versions, the hello facts, the self-test.
//! - [`guarded`]: every private call checked, wrapped against exceptions and given a limit.
//! - [`sim`]: the device's screen, its framebuffer and callbacks, the frame stream.
//! - [`ring`]: the RGBA IOSurface ring and the GPU pass that fills it.
//! - [`rendezvous`]: the mach channel (Flare's checked-in service, the sim host's port), pid-checked both ways.
//! - [`simctl`]: the public `simctl` tool.
//! - [`bench`]: `flare-sim-host --bench`, the frame path against the probe app.

pub mod bench;
pub mod codesign;
pub mod guarded;
pub mod load;
pub mod rendezvous;
pub mod ring;
pub mod sim;
pub mod simctl;
