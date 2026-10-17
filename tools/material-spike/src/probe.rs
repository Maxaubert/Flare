//! Mode `probe`: no visible window. What the OS and the DX12 adapter offer for each swapchain kind.

use crate::{json_line, json_str};
use flare_win::material::{support, Support};
use flare_win::os_build::os_version;
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use wgpu::Dx12SwapchainKind;

struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs a future to completion on this thread (wgpu's adapter request needs nothing more).
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

/// A never-shown top-level window of a predefined class, only there to give wgpu a surface.
struct HiddenWindow(isize);

impl HiddenWindow {
    fn new() -> Option<Self> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, CW_USEDEFAULT, WS_POPUP,
        };
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: the STATIC class is predefined; the window is never shown and is destroyed in Drop.
        let h = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                64,
                64,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        (!h.is_null()).then_some(Self(h as isize))
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        // SAFETY: the handle was made by CreateWindowExW above and is destroyed once.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0 as _);
        }
    }
}

fn kind_name(k: Dx12SwapchainKind) -> &'static str {
    match k {
        Dx12SwapchainKind::DxgiFromVisual => "visual",
        Dx12SwapchainKind::DxgiFromHwnd => "hwnd",
    }
}

/// `{adapter, backend, alpha_modes, formats}` for one swapchain kind, or `{error}`.
fn probe_kind(kind: Dx12SwapchainKind, hwnd: isize) -> String {
    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        backend_options: wgpu::BackendOptions {
            dx12: wgpu::Dx12BackendOptions {
                presentation_system: kind,
                ..Default::default()
            },
            ..Default::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let Some(nz) = std::num::NonZeroIsize::new(hwnd) else {
        return json_line(&[("error", json_str("no hidden window"))]);
    };
    let handle = Win32WindowHandle::new(nz);
    // SAFETY: the hidden window outlives the surface (both are dropped at the end of probe).
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(RawDisplayHandle::Windows(WindowsDisplayHandle::new())),
            raw_window_handle: RawWindowHandle::Win32(handle),
        })
    };
    let surface = match surface {
        Ok(s) => s,
        Err(e) => return json_line(&[("error", json_str(&format!("surface: {e}")))]),
    };
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }));
    let adapter = match adapter {
        Ok(a) => a,
        Err(e) => return json_line(&[("error", json_str(&format!("adapter: {e}")))]),
    };
    let info = adapter.get_info();
    let caps = surface.get_capabilities(&adapter);
    let modes: Vec<String> = caps
        .alpha_modes
        .iter()
        .map(|m| json_str(&format!("{m:?}")))
        .collect();
    let formats: Vec<String> = caps
        .formats
        .iter()
        .map(|f| json_str(&format!("{f:?}")))
        .collect();
    json_line(&[
        ("adapter", json_str(&info.name)),
        ("backend", json_str(&format!("{:?}", info.backend))),
        ("alpha_modes", format!("[{}]", modes.join(","))),
        ("formats", format!("[{}]", formats.join(","))),
    ])
}

pub fn run() -> Result<String, String> {
    let v = os_version();
    let sup = match support(v) {
        Support::Ok => "Ok",
        Support::NeedsWindows11 => "NeedsWindows11",
        Support::NeedsWindows11_22H2 => "NeedsWindows11_22H2",
    };
    let win = HiddenWindow::new().ok_or("cannot make the hidden window")?;
    let mut fields = vec![
        ("mode", json_str("probe")),
        (
            "os",
            json_str(&format!("{}.{}.{}", v.major, v.minor, v.build)),
        ),
        ("support", json_str(sup)),
    ];
    for kind in [
        Dx12SwapchainKind::DxgiFromVisual,
        Dx12SwapchainKind::DxgiFromHwnd,
    ] {
        fields.push((kind_name(kind), probe_kind(kind, win.0)));
    }
    Ok(json_line(&fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_runs_a_ready_future() {
        assert_eq!(block_on(async { 7 }), 7);
    }

    #[test]
    fn kinds_have_names() {
        assert_eq!(kind_name(Dx12SwapchainKind::DxgiFromVisual), "visual");
        assert_eq!(kind_name(Dx12SwapchainKind::DxgiFromHwnd), "hwnd");
    }
}
