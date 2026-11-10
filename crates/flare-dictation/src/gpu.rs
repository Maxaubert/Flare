//! The NVIDIA probe (Task 17): is there an NVIDIA adapter, answered once per process.
//! `FLARE_E2E_NVIDIA` (`1` or `0`) overrides the probe, under --e2e only (`flare_win::seam`); the name is spelled through `flare_win::names`.

use std::sync::OnceLock;

#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

/// The PCI vendor id of NVIDIA.
#[cfg(windows)]
const NVIDIA_VENDOR: u32 = 0x10DE;

/// The bundled engine has a GPU of its own: Metal, on a Mac (rule 1082). Every Mac Flare runs on is Apple
/// Silicon (aarch64-apple-darwin, macOS 15 or later), and every Apple Silicon GPU has Metal, so nothing is
/// probed; the engine itself falls back to the CPU once and for good if Metal will not start. No pack exists
/// on a Mac, so the models the pack is for are fast without one and the Recommended pick follows.
pub const BUILT_IN_GPU: bool = cfg!(target_os = "macos");

/// Whether a GPU engine is there to run a model: the NVIDIA pack installed (Windows), or Metal (a Mac).
pub fn gpu_ready(pack_installed: bool) -> bool {
    pack_installed || BUILT_IN_GPU
}

/// True when an NVIDIA adapter is present (or the e2e seam says so). Answered once per process.
pub fn has_nvidia() -> bool {
    static CELL: OnceLock<bool> = OnceLock::new();
    let seam = flare_win::seam::var(flare_win::names::SEAM_NVIDIA);
    decide(&CELL, seam.as_deref(), probe)
}

/// The answer: the seam when it says `1` or `0`, else the probe, computed at most once in `cell`.
fn decide(cell: &OnceLock<bool>, seam: Option<&str>, probe: impl FnOnce() -> bool) -> bool {
    *cell.get_or_init(|| match seam {
        Some("1") => true,
        Some("0") => false,
        _ => probe(),
    })
}

/// Enumerates the DXGI adapters (no device is created) and looks for NVIDIA's vendor id.
#[cfg(windows)]
fn probe() -> bool {
    // sync-ok: a DXGI enumeration of the adapters, a few milliseconds, answered once per process
    // SAFETY: CreateDXGIFactory1 takes no pointers from us; it returns a new factory or an error.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return false;
    };
    let mut index = 0;
    // SAFETY: `factory` is a live IDXGIFactory1 owned here; EnumAdapters1 fails past the last adapter.
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
        // SAFETY: `adapter` is a live IDXGIAdapter1 owned here; GetDesc1 only writes its own descriptor.
        if let Ok(desc) = unsafe { adapter.GetDesc1() } {
            if desc.VendorId == NVIDIA_VENDOR {
                return true;
            }
        }
        index += 1;
    }
    false
}

/// No NVIDIA adapter on a Mac (Apple Silicon only, port plan targets).
#[cfg(not(windows))]
fn probe() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn the_seam_overrides_the_probe() {
        // "1" and "0" answer without the probe; anything else leaves the probe to decide.
        let cell = OnceLock::new();
        assert!(decide(&cell, Some("1"), || panic!(
            "the probe must not run"
        )));
        let cell = OnceLock::new();
        assert!(!decide(&cell, Some("0"), || panic!(
            "the probe must not run"
        )));
        let cell = OnceLock::new();
        assert!(decide(&cell, Some("yes"), || true));
        let cell = OnceLock::new();
        assert!(!decide(&cell, None, || false));
    }

    #[test]
    fn a_mac_has_a_gpu_engine_without_a_pack_and_windows_only_with_one() {
        assert_eq!(BUILT_IN_GPU, cfg!(target_os = "macos"));
        assert!(gpu_ready(true));
        assert_eq!(gpu_ready(false), cfg!(target_os = "macos"));
    }

    #[test]
    fn the_probe_is_answered_once() {
        let cell = OnceLock::new();
        let calls = Cell::new(0);
        let first = decide(&cell, None, || {
            calls.set(calls.get() + 1);
            true
        });
        let second = decide(&cell, None, || {
            calls.set(calls.get() + 1);
            false
        });
        assert!(first && second);
        assert_eq!(calls.get(), 1);
    }
}
