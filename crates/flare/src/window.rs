//! The Slint window: wires the chrome, the grid host and the renderer notifier together.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use flare_render::raster::{DEFAULT_FAMILIES, Rasterizer};
use slint::{ComponentHandle, GraphicsAPI, RenderingState};

use crate::AppWindow;
use crate::backend::BackendOutcome;
use crate::config::RendererChoice;
use crate::host::{GridHost, GridPainter};
use crate::memory::process_memory;

/// The overlay text: renderer, grid painter, frame statistics and memory.
fn overlay_text(
    effective: RendererChoice,
    fallback: Option<&str>,
    painter: GridPainter,
    frame: &str,
    memory: &str,
) -> String {
    let renderer = match (effective, fallback) {
        (RendererChoice::Gpu, _) => "GPU (femtovg-wgpu)".to_string(),
        (RendererChoice::Software, Some(reason)) => format!("software (fallback: {reason})"),
        (RendererChoice::Software, None) => "software".to_string(),
    };
    let grid = match painter {
        GridPainter::Gpu => "wgpu",
        GridPainter::Cpu => "cpu",
    };
    format!("renderer: {renderer}\ngrid: {grid}\nframe: {frame}\nmemory: {memory}")
}

/// Runs the window until it closes. `start` is main's start time, for the first-frame log.
pub fn run_window(outcome: BackendOutcome, start: Instant) -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;
    let painter = match outcome.effective {
        RendererChoice::Gpu => GridPainter::Gpu,
        RendererChoice::Software => GridPainter::Cpu,
    };
    let host = Rc::new(RefCell::new(GridHost::new(
        Box::new(Rasterizer::new(DEFAULT_FAMILIES, 1.0)),
        painter,
    )));
    // Why the grid is not on the GPU even though it was asked for, set by the notifier.
    let note: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(outcome.fallback_reason.clone()));
    let effective = outcome.effective;

    let refresh_overlay = {
        let host = host.clone();
        let note = note.clone();
        let weak = app.as_weak();
        move || {
            let Some(app) = weak.upgrade() else { return };
            let host = host.borrow();
            let memory =
                process_memory().map_or_else(|| "unavailable".to_string(), |m| m.summary());
            let text = overlay_text(
                effective,
                note.borrow().as_deref(),
                host.painter(),
                &host.stats.summary(),
                &memory,
            );
            app.set_overlay_text(text.into());
        }
    };

    // Renders into the window's image now.
    let render_now = {
        let host = host.clone();
        let weak = app.as_weak();
        move || {
            let Some(app) = weak.upgrade() else { return };
            if let Some(image) = host.borrow_mut().render() {
                app.set_grid_image(image);
            }
        }
    };

    let notifier_result = {
        let host = host.clone();
        let note = note.clone();
        let weak = app.as_weak();
        let mut first_frame = true;
        app.window()
            .set_rendering_notifier(move |state, api| match state {
                RenderingState::RenderingSetup => {
                    if let GraphicsAPI::WGPU30 { device, queue, .. } = api {
                        host.borrow_mut().attach_gpu(device, queue);
                    } else {
                        host.borrow_mut().detach_gpu();
                        *note.borrow_mut() = Some("Slint renderer has no wgpu device".to_string());
                        log::warn!("renderer: grid on the CPU (Slint renderer has no wgpu device)");
                    }
                }
                RenderingState::BeforeRendering => {
                    let Some(app) = weak.upgrade() else { return };
                    if host.borrow().is_dirty() {
                        let image = host.borrow_mut().render();
                        if let Some(image) = image {
                            app.set_grid_image(image);
                            app.window().request_redraw();
                        }
                    }
                }
                RenderingState::AfterRendering => {
                    if first_frame {
                        first_frame = false;
                        log::info!("first frame after {} ms", start.elapsed().as_millis());
                    }
                }
                RenderingState::RenderingTeardown => host.borrow_mut().detach_gpu(),
                _ => {}
            })
    };
    if let Err(e) = notifier_result {
        host.borrow_mut().detach_gpu();
        *note.borrow_mut() = Some(format!("no rendering notifier: {e}"));
        log::info!("grid on the CPU: no rendering notifier ({e})");
    }

    app.on_grid_resized({
        let host = host.clone();
        let render_now = render_now.clone();
        move |w, h, scale| {
            host.borrow_mut()
                .resize(w.max(0) as u32, h.max(0) as u32, scale);
            render_now();
        }
    });
    app.on_minimize_clicked({
        let weak = app.as_weak();
        move || {
            if let Some(app) = weak.upgrade() {
                app.window().set_minimized(true);
            }
        }
    });
    app.on_maximize_clicked({
        let weak = app.as_weak();
        move || {
            if let Some(app) = weak.upgrade() {
                let window = app.window();
                window.set_maximized(!window.is_maximized());
            }
        }
    });
    app.on_close_clicked({
        let weak = app.as_weak();
        move || {
            if let Some(app) = weak.upgrade() {
                let _ = app.hide();
            }
            let _ = slint::quit_event_loop();
        }
    });
    app.on_toggle_grid_renderer({
        let host = host.clone();
        let render_now = render_now.clone();
        let refresh_overlay = refresh_overlay.clone();
        move || {
            host.borrow_mut().toggle_painter();
            render_now();
            refresh_overlay();
        }
    });

    // The overlay refreshes twice a second, and only while it is shown.
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(500), {
        let weak = app.as_weak();
        let refresh_overlay = refresh_overlay.clone();
        move || {
            if weak.upgrade().is_some_and(|app| app.get_overlay_visible()) {
                refresh_overlay();
            }
        }
    });

    app.show()?;
    // The first size, before Slint's `changed` callbacks have fired.
    host.borrow_mut().resize(
        app.get_grid_phys_width().max(0) as u32,
        app.get_grid_phys_height().max(0) as u32,
        app.get_scale(),
    );
    render_now();
    refresh_overlay();
    let result = app.run();
    drop(timer);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_names_the_fallback() {
        let text = overlay_text(
            RendererChoice::Software,
            Some("no adapter"),
            GridPainter::Cpu,
            "no frames",
            "WS 1.0 MB, private 2.0 MB",
        );
        assert!(text.starts_with("renderer: software (fallback: no adapter)\n"));
        assert!(text.contains("grid: cpu"));
        assert!(text.contains("frame: no frames"));
    }

    #[test]
    fn overlay_names_gpu() {
        let text = overlay_text(RendererChoice::Gpu, None, GridPainter::Gpu, "x", "y");
        assert!(text.starts_with("renderer: GPU (femtovg-wgpu)\ngrid: wgpu\n"));
    }
}
