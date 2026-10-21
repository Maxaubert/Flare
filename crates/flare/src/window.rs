//! The Slint window: wires the chrome, the grid host and the renderer notifier together.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flare_render::raster::{DEFAULT_FAMILIES, Rasterizer};
use slint::winit_030::winit::event::{ElementState, Ime, MouseScrollDelta, WindowEvent};
use slint::winit_030::winit::keyboard::ModifiersState;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{CloseRequestResponse, ComponentHandle, GraphicsAPI, RenderingState};

use flare_term::keys::{KeyState, TermKeyAction, term_key_action};

use crate::AppWindow;
use crate::backend::BackendOutcome;
use crate::config::RendererChoice;
use crate::host::{GridHost, GridPainter};
use crate::keymap::{key_event_from_parts, reserved_for_slint};
use crate::live::{Live, LiveSource, WheelAccum, cell_at, shutdown, start_live};
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

/// Fires once: the first call to `fire` answers true, every later one false.
#[derive(Debug, Default)]
struct Once(bool);

impl Once {
    fn fire(&mut self) -> bool {
        !std::mem::replace(&mut self.0, true)
    }
}

/// Without a rendering notifier, a repaint is due when the grid is dirty.
fn repaint_due(dirty: bool) -> bool {
    dirty
}

/// Runs the window until it closes. `start` is main's start time, for the first-frame log.
/// `sample` keeps M1's sample grid instead of a live shell.
pub fn run_window(
    outcome: BackendOutcome,
    start: Instant,
    sample: bool,
) -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;
    let painter = match outcome.effective {
        RendererChoice::Gpu => GridPainter::Gpu,
        RendererChoice::Software => GridPainter::Cpu,
    };
    let host = Rc::new(RefCell::new(GridHost::new(
        Box::new(Rasterizer::new(DEFAULT_FAMILIES, 1.0)),
        painter,
    )));
    let live: Option<Rc<Live>> = if sample {
        None
    } else {
        let live = Rc::new(start_live(app.as_weak()));
        host.borrow_mut()
            .set_source(Box::new(LiveSource::new(Arc::clone(&live.handle))));
        Some(live)
    };
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
    let no_notifier = Rc::new(Cell::new(false));
    let first_software_frame = Rc::new(RefCell::new(Once::default()));
    let render_now = {
        let host = host.clone();
        let weak = app.as_weak();
        let no_notifier = no_notifier.clone();
        let first_software_frame = first_software_frame.clone();
        move || {
            let Some(app) = weak.upgrade() else { return };
            let image = host.borrow_mut().render();
            if let Some(image) = image {
                app.set_grid_image(image);
                // No notifier means no AfterRendering: log the first frame from here.
                if no_notifier.get() && first_software_frame.borrow_mut().fire() {
                    log::info!("first frame after {} ms", start.elapsed().as_millis());
                }
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
                        *note.borrow_mut() = None;
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
        no_notifier.set(true);
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
        let live = live.clone();
        move || {
            if let Some(live) = &live {
                shutdown(live);
            }
            if let Some(app) = weak.upgrade() {
                let _ = app.hide();
            }
            let _ = slint::quit_event_loop();
        }
    });
    app.window().on_close_requested({
        let live = live.clone();
        move || {
            if let Some(live) = &live {
                shutdown(live);
            }
            CloseRequestResponse::HideWindow
        }
    });
    if let Some(live) = live.clone() {
        install_input_filter(&app, live, host.clone());
    }
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

    // Without a notifier nothing re-renders a dirty grid, so poll for it (M2's live output).
    let frame_timer = slint::Timer::default();
    if no_notifier.get() {
        frame_timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), {
            let host = host.clone();
            let render_now = render_now.clone();
            move || {
                if repaint_due(host.borrow().is_dirty()) {
                    render_now();
                }
            }
        });
    }

    app.show()?;
    if !sample {
        app.window().with_winit_window(|w| w.set_ime_allowed(true));
    }
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
    drop(frame_timer);
    result
}

/// One winit filter: physical keys, IME text and the wheel go to the live shell.
fn install_input_filter(app: &AppWindow, live: Rc<Live>, host: Rc<RefCell<GridHost>>) {
    let mods = Cell::new(ModifiersState::empty());
    let cursor = Cell::new((0.0_f64, 0.0_f64));
    let accum = RefCell::new(WheelAccum::new());
    let weak = app.as_weak();
    app.window().on_winit_window_event(move |_window, event| {
        match event {
            WindowEvent::ModifiersChanged(m) => {
                mods.set(m.state());
                EventResult::Propagate
            }
            WindowEvent::CursorMoved { position, .. } => {
                cursor.set((position.x, position.y));
                EventResult::Propagate
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                let ev = key_event_from_parts(
                    event.state == ElementState::Pressed,
                    &event.physical_key,
                    &event.logical_key,
                    event.text.as_deref(),
                    mods.get(),
                );
                if reserved_for_slint(&ev.code) {
                    return EventResult::Propagate;
                }
                let mut no_delete = || false;
                let mut state = KeyState {
                    has_selection: false,
                    delete_selection: &mut no_delete,
                    host_owns: &|_| false,
                    agent_here: false,
                };
                match term_key_action(&ev, &mut state) {
                    TermKeyAction::Pty => {
                        live.handle.send_key(&ev);
                        EventResult::PreventDefault
                    }
                    // The clipboard path is M5.
                    TermKeyAction::Paste | TermKeyAction::PastePlain => EventResult::PreventDefault,
                    _ => EventResult::Propagate,
                }
            }
            WindowEvent::Ime(Ime::Commit(s)) => {
                live.handle.send_text(s);
                EventResult::PreventDefault
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some(app) = weak.upgrade() else {
                    return EventResult::Propagate;
                };
                let scale = app.get_scale();
                let (x, y) = cursor.get();
                if y < (36.0 * f64::from(scale)).round() {
                    return EventResult::Propagate;
                }
                let (m, (cols, rows)) = {
                    let h = host.borrow();
                    (h.metrics(), h.dims())
                };
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y.round() as i32,
                    MouseScrollDelta::PixelDelta(p) => accum.borrow_mut().add(p.y, m.cell_h),
                };
                if notches != 0 {
                    let (col, row) = cell_at(x, y, scale, &m, cols, rows);
                    live.handle.wheel(notches, col, row);
                }
                EventResult::PreventDefault
            }
            _ => EventResult::Propagate,
        }
    });
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
    fn once_fires_a_single_time() {
        let mut once = Once::default();
        assert!(once.fire());
        assert!(!once.fire());
        assert!(!once.fire());
    }

    #[test]
    fn repaint_follows_dirty() {
        assert!(repaint_due(true));
        assert!(!repaint_due(false));
    }

    #[test]
    fn overlay_names_gpu() {
        let text = overlay_text(RendererChoice::Gpu, None, GridPainter::Gpu, "x", "y");
        assert!(text.starts_with("renderer: GPU (femtovg-wgpu)\ngrid: wgpu\n"));
    }
}
