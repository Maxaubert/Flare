//! `--screenshot` and `--bench-frames`: the window is drawn by Slint's software renderer into a
//! buffer, and no OS window is ever created or shown.

use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use flare_render::gpu::headless_device;
use flare_render::raster::{DEFAULT_FAMILIES, Rasterizer};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Image, PhysicalSize, Rgba8Pixel, SharedPixelBuffer};

use crate::AppWindow;
use crate::config::{Args, RendererChoice};
use crate::host::{GridHost, GridPainter};
use crate::memory::process_memory;

/// Hands Slint the one software window, so nothing is created on the desktop.
struct HeadlessPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

/// Runs the headless tools. Errors starting with "GPU unavailable" mean exit code 2.
pub fn run(args: Args, start: Instant) -> Result<(), String> {
    let scale = args.scale;
    let width = (args.size.0 as f32 * scale).round() as u32;
    let height = (args.size.1 as f32 * scale).round() as u32;

    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(HeadlessPlatform(win.clone())))
        .map_err(|e| format!("cannot set the headless platform: {e}"))?;

    let glyphs = Box::new(Rasterizer::new(DEFAULT_FAMILIES, 1.0));
    let mut host = match args.renderer {
        RendererChoice::Software => GridHost::new(glyphs, GridPainter::Cpu),
        RendererChoice::Gpu => {
            let (device, queue, info) =
                headless_device().map_err(|e| format!("GPU unavailable: {e}"))?;
            log::info!("headless adapter: {} ({:?})", info.name, info.backend);
            let mut host = GridHost::new(glyphs, GridPainter::Gpu);
            host.attach_gpu(&device, &queue);
            host
        }
    };

    let app = AppWindow::new().map_err(|e| format!("cannot create the window: {e}"))?;
    win.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    win.set_size(PhysicalSize::new(width, height));
    app.show()
        .map_err(|e| format!("cannot show the window: {e}"))?;
    host.resize(
        app.get_grid_phys_width().max(0) as u32,
        app.get_grid_phys_height().max(0) as u32,
        app.get_scale(),
    );

    let frame = |host: &mut GridHost| -> Result<Vec<u8>, String> {
        let grid = host
            .render_rgba()
            .ok_or_else(|| format!("the grid has no area at {width}x{height}"))?;
        let buffer =
            SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&grid.data, grid.width, grid.height);
        app.set_grid_image(Image::from_rgba8(buffer));
        win.request_redraw();
        let mut pixels = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
        win.draw_if_needed(|r| {
            r.render(&mut pixels, width as usize);
        });
        Ok(pixels.iter().flat_map(straight_rgba).collect())
    };

    let first = frame(&mut host)?;
    let startup_ms = start.elapsed().as_secs_f64() * 1000.0;

    if let Some(path) = &args.screenshot {
        write_png(path, width, height, &first)?;
        println!("screenshot: {}", path.display());
    }

    if let Some(frames) = args.bench_frames {
        for _ in 0..frames {
            // Each frame is laid out, painted and (on the GPU) read back.
            host.render_rgba()
                .ok_or_else(|| "the grid has no area to benchmark".to_string())?;
        }
        println!("frames: {}", host.stats.summary());
        let memory = process_memory().map_or_else(|| "unavailable".to_string(), |m| m.summary());
        println!("memory: {memory}");
        println!("startup: {startup_ms:.0} ms to the first rendered grid");
    }
    Ok(())
}

/// One premultiplied pixel as straight RGBA bytes.
fn straight_rgba(p: &PremultipliedRgbaColor) -> [u8; 4] {
    let unpremultiply = |c: u8| -> u8 {
        if p.alpha == 0 {
            0
        } else {
            (u32::from(c) * 255 / u32::from(p.alpha)).min(255) as u8
        }
    };
    [
        unpremultiply(p.red),
        unpremultiply(p.green),
        unpremultiply(p.blue),
        p.alpha,
    ]
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}
