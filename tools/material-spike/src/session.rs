//! The window modes: show the 400 x 300 test window (inactive, always on top, off the taskbar),
//! apply the `flare_win` calls, wait, capture it with GDI, print one JSON line and close.

use crate::capture::{self, Rect, Shot};
use crate::{
    diff_stddev, e1_glass_works, e2_alpha_one_is_opaque, e3_premultiplied, e4_border_visible,
    json_line, json_str, Args, Mode, Renderer, Swapchain,
};
use flare_win::dwm::{self, Backdrop, Border, DwmSurface};
use flare_win::material::{support, Material, Support};
use flare_win::os_build::os_version;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor;
use slint::{Brush, Color, ComponentHandle, GraphicsAPI, RenderingState, Timer};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

slint::include_modules!();

const GROUND: [u8; 3] = [0x17, 0x17, 0x17];
const BORDER_HEX: &str = "#ff00ff";
const BORDER: [u8; 3] = [0xff, 0x00, 0xff];
/// The window is up this long before the first step, so the OS has created and shown it.
const SETTLE_MS: u64 = 400;
/// "waits 1.5 s" between applying the calls and the capture.
const WAIT_MS: u64 = 1500;
const REDRAWS: u32 = 300;

fn rgb(p: [u8; 3]) -> String {
    format!("[{},{},{}]", p[0], p[1], p[2])
}

fn select(a: &Args) -> Result<(), String> {
    use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
    use slint::winit_030::winit::window::WindowLevel;
    let sel = slint::BackendSelector::new()
        .with_winit_window_attributes_hook(|attrs| {
            attrs
                .with_transparent(true)
                .with_active(false)
                .with_skip_taskbar(true)
                .with_decorations(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
        })
        .backend_name("winit".into());
    let r = match a.renderer {
        Renderer::Software => sel.renderer_name("software".into()).select(),
        Renderer::Gpu => {
            let mut s = slint::wgpu_30::WGPUSettings::default();
            s.device_required_limits = slint::wgpu_30::wgpu::Limits::default();
            s.backend_options.dx12.presentation_system = match a.swapchain {
                Swapchain::Visual => slint::wgpu_30::wgpu::Dx12SwapchainKind::DxgiFromVisual,
                Swapchain::Hwnd => slint::wgpu_30::wgpu::Dx12SwapchainKind::DxgiFromHwnd,
            };
            sel.renderer_name("femtovg-wgpu".into())
                .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Automatic(s))
                .select()
        }
    };
    r.map_err(|e| format!("backend: {e}"))
}

fn hwnd_of(ui: &Spike) -> Option<isize> {
    ui.window()
        .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        })
        .flatten()
}

/// WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, so the test window can never take the foreground.
fn no_activate(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    // SAFETY: plain style calls on a window of this process; a bad handle makes them return 0.
    unsafe {
        let h = hwnd as _;
        let old = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(
            h,
            GWL_EXSTYLE,
            old | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize,
        );
    }
}

struct Ctx {
    a: Args,
    ui: slint::Weak<Spike>,
    result: RefCell<Option<Result<String, String>>>,
    hwnd: Cell<isize>,
    rect: Cell<Option<Rect>>,
    open: RefCell<Option<Shot>>,
    fields: RefCell<Vec<(&'static str, String)>>,
}

impl Ctx {
    fn put(&self, k: &'static str, v: String) {
        self.fields.borrow_mut().push((k, v));
    }

    fn finish(&self, r: Result<(), String>) {
        if self.result.borrow().is_some() {
            return;
        }
        let out = r.map(|()| {
            let mut f = vec![("mode", json_str(mode_name(self.a.mode)))];
            f.extend(self.fields.borrow().iter().cloned());
            json_line(&f)
        });
        *self.result.borrow_mut() = Some(out);
        let _ = slint::quit_event_loop();
    }
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Probe => "probe",
        Mode::GlassGpu => "glass-gpu",
        Mode::Premul => "premul",
        Mode::Border => "border",
        Mode::GlassSoftware => "glass-software",
        Mode::Readback => "readback",
        Mode::Cost => "cost",
        Mode::CostOnce => "cost-once",
    }
}

fn after(ms: u64, f: impl FnOnce() + 'static) {
    Timer::single_shot(Duration::from_millis(ms), f);
}

/// The ground's alpha: `--level N` is the see-through percentage, so 40 is alpha 0.6.
fn ground_alpha(a: &Args) -> u8 {
    match a.mode {
        Mode::GlassGpu | Mode::GlassSoftware => {
            ((100 - a.level.min(100)) as f64 * 255.0 / 100.0).round() as u8
        }
        Mode::CostOnce => 153,
        _ => 255,
    }
}

/// Step one: move the window, switch off activation, apply this mode's `flare_win` calls.
fn apply(ctx: &Ctx) -> Result<(), String> {
    let ui = ctx.ui.upgrade().ok_or("the window is gone")?;
    let hwnd = hwnd_of(&ui).ok_or("the window has no HWND")?;
    ctx.hwnd.set(hwnd);
    no_activate(hwnd);
    let size = ui.window().size();
    let (w, h) = if size.width > 0 {
        (size.width as i32, size.height as i32)
    } else {
        (400, 300)
    };
    let (sw, sh) = capture::screen_size();
    ui.window()
        .set_position(slint::PhysicalPosition::new(sw - w - 24, sh - h - 64));
    match ctx.a.mode {
        Mode::GlassGpu | Mode::GlassSoftware | Mode::CostOnce => {
            let supported = support(os_version()) == Support::Ok;
            let mut m = Material::new(DwmSurface::new(hwnd), supported);
            m.set_bg("#171717");
            let supported_now = m.set_acrylic(true);
            ctx.put("material_supported", supported_now.to_string());
            ctx.put("glass_on", m.glass_on().to_string());
        }
        Mode::Border => {
            ctx.put(
                "set_border_ok",
                dwm::set_border(hwnd, &Border::Colour(BORDER_HEX.into())).to_string(),
            );
        }
        Mode::Readback => {
            let set_backdrop = dwm::set_system_backdrop(hwnd, Backdrop::Acrylic);
            let set_border = dwm::set_border(hwnd, &Border::Colour(BORDER_HEX.into()));
            let set_dark = dwm::set_immersive_dark(hwnd, true);
            let want_border = dwm::colorref_of(BORDER_HEX) as i32;
            let r38 = dwm::read_back_i32(hwnd, 38);
            let r34 = dwm::read_back_i32(hwnd, 34);
            let r20 = dwm::read_back_i32(hwnd, 20);
            let show = |v: Option<i32>| v.map_or("null".to_string(), |x| x.to_string());
            ctx.put(
                "set_ok",
                format!("[{set_backdrop},{set_border},{set_dark}]"),
            );
            ctx.put("attr38", show(r38));
            ctx.put("attr34", show(r34));
            ctx.put("attr20", show(r20));
            ctx.put("want", format!("[3,{want_border},1]"));
            ctx.put(
                "e6_equal",
                (r38 == Some(3) && r34 == Some(want_border) && r20 == Some(1)).to_string(),
            );
        }
        _ => {}
    }
    Ok(())
}

fn save(ctx: &Ctx, shot: &Shot) {
    if let Some(p) = &ctx.a.out {
        match capture::save_png(p, shot) {
            Ok(()) => ctx.put("png", json_str(p)),
            Err(e) => ctx.put("png_error", json_str(&e)),
        }
    }
}

/// Step two: capture the window rectangle and decide this mode's verdict (glass waits for the
/// closed capture before it can).
fn measure(ctx: &Rc<Ctx>) -> Result<(), String> {
    let hwnd = ctx.hwnd.get();
    let rect = capture::window_rect(hwnd).ok_or("no window rectangle")?;
    ctx.rect.set(Some(rect));
    let shot = capture::capture(rect).ok_or("the capture failed")?;
    ctx.put(
        "rect",
        format!(
            "[{},{},{},{}]",
            rect.left,
            rect.top,
            rect.width(),
            rect.height()
        ),
    );
    save(ctx, &shot);
    match ctx.a.mode {
        Mode::Premul => {
            let c = shot.centre();
            ctx.put("centre", rgb(c));
            ctx.put("e3", json_str(&format!("{:?}", e3_premultiplied(c))));
            ctx.finish(Ok(()));
        }
        Mode::Border => {
            let (w, h) = (shot.width, shot.height);
            let col = |x: usize| -> Vec<[u8; 3]> {
                (h / 10..h - h / 10).map(|y| shot.pixel(x, y)).collect()
            };
            let (c0, c1) = (col(0), col(1));
            ctx.put("left_mid", rgb(shot.pixel(0, h / 2)));
            ctx.put("left1_mid", rgb(shot.pixel(1, h / 2)));
            ctx.put("right_mid", rgb(shot.pixel(w - 1, h / 2)));
            ctx.put("centre", rgb(shot.centre()));
            ctx.put("e4_col0", e4_border_visible(&c0, BORDER).to_string());
            ctx.put("e4_col1", e4_border_visible(&c1, BORDER).to_string());
            ctx.finish(Ok(()));
        }
        Mode::Readback => ctx.finish(Ok(())),
        Mode::GlassGpu | Mode::GlassSoftware => {
            *ctx.open.borrow_mut() = Some(shot);
            let ui = ctx.ui.upgrade().ok_or("the window is gone")?;
            let _ = ui.hide();
            let c = ctx.clone();
            after(SETTLE_MS + 100, move || {
                let r = closed(&c);
                c.finish(r);
            });
        }
        _ => ctx.finish(Ok(())),
    }
    Ok(())
}

/// Step three (glass): the same rectangle with the window closed, and the E1 and E2 verdicts.
fn closed(ctx: &Ctx) -> Result<(), String> {
    let rect = ctx.rect.get().ok_or("no rectangle")?;
    let closed = capture::capture(rect).ok_or("the closed capture failed")?;
    let open_ref = ctx.open.borrow();
    let open = open_ref.as_ref().ok_or("no open capture")?;
    let sd = diff_stddev(&open.rgba, &closed.rgba);
    let (co, cc) = (open.centre(), closed.centre());
    ctx.put("level", ctx.a.level.to_string());
    ctx.put("ground_alpha", ground_alpha(&ctx.a).to_string());
    ctx.put("centre_open", rgb(co));
    ctx.put("centre_closed", rgb(cc));
    ctx.put("diff_stddev", format!("{sd:.3}"));
    ctx.put("e1_glass_works", e1_glass_works(co, cc, sd).to_string());
    ctx.put(
        "e2_centre_is_ground",
        e2_alpha_one_is_opaque(co, GROUND).to_string(),
    );
    Ok(())
}

/// Binds the rendering notifier: the imported texture (premul) and the redraw timing (cost-once).
fn notifier(ui: &Spike, ctx: &Rc<Ctx>, t0: Instant) -> Result<(), String> {
    let weak = ui.as_weak();
    let ctx = ctx.clone();
    let mode = ctx.a.mode;
    let done = Cell::new(false);
    let first = Cell::new(None::<Instant>);
    let count = Cell::new(0u32);
    ui.window()
        .set_rendering_notifier(move |state, api| match state {
            RenderingState::BeforeRendering if mode == Mode::Premul && !done.get() => {
                done.set(true);
                let GraphicsAPI::WGPU30 { device, queue, .. } = api else {
                    ctx.finish(Err("no wgpu device".into()));
                    return;
                };
                let size = wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                };
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("premul"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let data: Vec<u8> = (0..64 * 64).flat_map(|_| [128u8, 0, 0, 128]).collect();
                queue.write_texture(
                    tex.as_image_copy(),
                    &data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(64 * 4),
                        rows_per_image: Some(64),
                    },
                    size,
                );
                match slint::Image::try_from(tex) {
                    Ok(img) => {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_tex(img);
                            ui.set_show_tex(true);
                            ui.window().request_redraw();
                        }
                    }
                    Err(e) => ctx.finish(Err(format!("texture import: {e}"))),
                }
            }
            RenderingState::AfterRendering if mode == Mode::CostOnce => {
                let Some(ui) = weak.upgrade() else { return };
                match first.get() {
                    None => {
                        ctx.put(
                            "start_ms",
                            format!("{:.2}", t0.elapsed().as_secs_f64() * 1000.0),
                        );
                        first.set(Some(Instant::now()));
                        ui.window().request_redraw();
                    }
                    Some(t) => {
                        count.set(count.get() + 1);
                        if count.get() >= REDRAWS {
                            let ms = t.elapsed().as_secs_f64() * 1000.0 / f64::from(REDRAWS);
                            ctx.put("frame_ms", format!("{ms:.3}"));
                            ctx.finish(Ok(()));
                        } else {
                            ui.window().request_redraw();
                        }
                    }
                }
            }
            _ => {}
        })
        .map_err(|e| format!("rendering notifier: {e}"))
}

pub fn run(a: &Args, t0: Instant) -> Result<String, String> {
    select(&a.clone())?;
    let ui = Spike::new().map_err(|e| e.to_string())?;
    let (g, alpha) = match a.mode {
        Mode::Premul => ([255, 255, 255], 255),
        _ => (GROUND, ground_alpha(a)),
    };
    ui.set_ground(Brush::SolidColor(Color::from_argb_u8(
        alpha, g[0], g[1], g[2],
    )));
    let ctx = Rc::new(Ctx {
        a: a.clone(),
        ui: ui.as_weak(),
        result: RefCell::new(None),
        hwnd: Cell::new(0),
        rect: Cell::new(None),
        open: RefCell::new(None),
        fields: RefCell::new(Vec::new()),
    });
    if a.renderer == Renderer::Gpu && matches!(a.mode, Mode::Premul | Mode::CostOnce) {
        notifier(&ui, &ctx, t0)?;
    }
    ui.show().map_err(|e| e.to_string())?;
    if a.mode != Mode::CostOnce {
        let c = ctx.clone();
        after(SETTLE_MS, move || {
            if let Err(e) = apply(&c) {
                c.finish(Err(e));
                return;
            }
            let c2 = c.clone();
            after(WAIT_MS, move || {
                if let Err(e) = measure(&c2) {
                    c2.finish(Err(e));
                }
            });
        });
    } else {
        let c = ctx.clone();
        after(SETTLE_MS, move || {
            if let Err(e) = apply(&c) {
                c.finish(Err(e));
            }
        });
    }
    let cap = if a.mode == Mode::CostOnce {
        30.0
    } else {
        a.seconds
    };
    let c = ctx.clone();
    after((cap * 1000.0) as u64, move || {
        c.finish(Err("timed out".into()))
    });
    slint::run_event_loop_until_quit().map_err(|e| e.to_string())?;
    let _ = ui.hide();
    let r = ctx.result.borrow_mut().take();
    r.unwrap_or_else(|| Err("the event loop ended with no result".into()))
}

/// Mode `cost`: ten fresh processes per run, medians of start-to-first-frame and of the mean of
/// 300 forced redraws.
pub fn cost(a: &Args) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let sc = match a.swapchain {
        Swapchain::Visual => "visual",
        Swapchain::Hwnd => "hwnd",
    };
    let (mut starts, mut frames) = (Vec::new(), Vec::new());
    for _ in 0..10 {
        let out = std::process::Command::new(&exe)
            .args(["--mode", "cost-once", "--swapchain", sc])
            .output()
            .map_err(|e| e.to_string())?;
        let line = String::from_utf8_lossy(&out.stdout).to_string();
        match (
            crate::num_field(&line, "start_ms"),
            crate::num_field(&line, "frame_ms"),
        ) {
            (Some(s), Some(f)) => {
                starts.push(s);
                frames.push(f);
            }
            _ => {
                return Err(format!(
                    "a cost run failed: {line} {}",
                    String::from_utf8_lossy(&out.stderr)
                ))
            }
        }
    }
    let list = |v: &[f64]| {
        format!(
            "[{}]",
            v.iter()
                .map(|x| format!("{x:.2}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    Ok(json_line(&[
        ("mode", json_str("cost")),
        ("swapchain", json_str(sc)),
        ("runs", starts.len().to_string()),
        (
            "start_ms_median",
            format!("{:.2}", crate::median(&starts).unwrap_or(0.0)),
        ),
        (
            "frame_ms_median",
            format!("{:.3}", crate::median(&frames).unwrap_or(0.0)),
        ),
        ("start_ms_all", list(&starts)),
        ("frame_ms_all", list(&frames)),
    ]))
}
