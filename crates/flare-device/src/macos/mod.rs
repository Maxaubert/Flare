//! The macOS half of the device panel: the sim host's private-API code (rule 1310: only `flare-sim-host` runs it,
//! Flare's own process never loads CoreSimulator or SimulatorKit) and the mach channel both sides share.
//!
//! - [`load`]: dlopen the frameworks, read their versions, the hello facts, the self-test.
//! - [`guarded`]: every private call checked, wrapped against exceptions and given a limit.
//! - [`sim`]: the device's screen, its framebuffer and callbacks, the frame stream.
//! - [`ring`]: the RGBA IOSurface ring and the GPU pass that fills it.
//! - [`rendezvous`]: the mach channel (Flare's checked-in service, the sim host's port), pid-checked both ways.
//! - [`simctl`]: the public `simctl` tool.
//! - [`xpc`]: XPC to the simulator's services, the liveness barrier.
//! - [`dtuhid`]: touches, keys and buttons as `dtuhidd` messages (CoreSimulator 1155.4 and newer).
//! - [`indigo`]: the legacy Indigo client (older CoreSimulator), never mixed with DTUHID in one boot.
//! - [`input`]: one device's input worker, everything in order.
//! - [`orient`]: rotation (Purple GSEvent 50) and the interface's orientation (CoreDevice `displayinfo`).
//! - [`ax`]: the UI tree (`AXPTranslator`), one request in flight.
//! - [`snapshot`]: the current screen as a point-sized PNG.
//! - [`bench`]: `flare-sim-host --bench` and `--input-bench`, against the probe apps.

pub mod ax;
pub mod bench;
pub mod dtuhid;
pub mod guarded;
pub mod indigo;
pub mod input;
pub mod load;
pub mod orient;
pub mod rendezvous;
pub mod ring;
pub mod sim;
pub mod simctl;
pub mod snapshot;
pub mod xpc;
