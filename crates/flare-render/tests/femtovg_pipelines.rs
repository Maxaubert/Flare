//! Regression rule 1181: femtovg (Slint's GPU renderer) keeps its GPU pipelines from frame to frame when a
//! frame is flushed twice, the way Slint flushes with a rendering notifier installed (its clear first, the items
//! after). femtovg 0.27.0 dropped, at the end of each flush, every pipeline that flush did not use: the clear-only
//! flush left one, and every frame built all the others again (on the Mac a naga to MSL translation and a Metal
//! compile each, about 80 % of the main thread of Max's Flare, sampled 2026-11-12). Flare's patched copy
//! (`third_party/femtovg`) drops a pipeline only after 240 flushes in a row left it unused.

use femtovg::{Canvas, Color, Paint, Path, renderer::WGPURenderer};

const W: u32 = 320;
const H: u32 = 240;

struct Gpu {
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// A headless device, or None where the machine has no adapter (the test then has nothing to measure).
fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some(Gpu {
        instance,
        device,
        queue,
    })
}

/// The render pipelines alive on the device now.
fn live_pipelines(g: &Gpu) -> usize {
    let _ = g.device.poll(wgpu::PollType::wait_indefinitely());
    g.instance
        .generate_report()
        .map_or(0, |r| r.hub_report().render_pipelines.num_allocated)
}

/// What a frame of Flare's chrome asks of femtovg: a concave fill (an agent icon), a stroke (a mark's arc) and a
/// translucent rounded rectangle (a hover).
fn items(c: &mut Canvas<WGPURenderer>) {
    let mut icon = Path::new();
    icon.move_to(10.0, 10.0);
    icon.line_to(120.0, 40.0);
    icon.line_to(40.0, 120.0);
    icon.line_to(90.0, 20.0);
    icon.close();
    c.fill_path(&icon, &Paint::color(Color::rgb(217, 119, 87)));
    let mut arc = Path::new();
    arc.arc(200.0, 120.0, 30.0, 0.0, 3.0, femtovg::Solidity::Solid);
    c.stroke_path(
        &arc,
        &Paint::color(Color::rgb(40, 200, 90)).with_line_width(3.0),
    );
    let mut hover = Path::new();
    hover.rounded_rect(150.0, 20.0, 120.0, 28.0, 6.0);
    c.fill_path(&hover, &Paint::color(Color::rgba(255, 255, 255, 40)));
}

/// The live pipelines right after the FIRST flush of each of three frames, as Slint flushes them: the clear and
/// submit, then the items and submit.
fn pipelines_after_first_flush(g: &Gpu) -> Vec<usize> {
    let target = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("femtovg pipelines target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let mut canvas =
        Canvas::new(WGPURenderer::new(g.device.clone(), g.queue.clone())).expect("a canvas");
    let mut seen = Vec::new();
    for _ in 0..3 {
        canvas.set_size(W, H, 2.0);
        canvas.clear_rect(0, 0, W, H, Color::rgb(23, 23, 23));
        g.queue.submit(canvas.flush_to_output(&target));
        seen.push(live_pipelines(g));
        items(&mut canvas);
        g.queue.submit(canvas.flush_to_output(&target));
    }
    seen
}

#[test]
fn a_clear_only_flush_keeps_the_pipelines_of_the_frame_before() {
    let Some(g) = gpu() else {
        eprintln!("skipping: no wgpu adapter");
        return;
    };
    let base = live_pipelines(&g);
    let seen: Vec<usize> = pipelines_after_first_flush(&g)
        .into_iter()
        .map(|n| n - base)
        .collect();
    // Frame 1 begins with the clear's pipeline alone. Frames 2 and 3 begin with the items' pipelines of the frame
    // before still there (unpatched femtovg: one, the clear's, so each frame built the rest again).
    assert_eq!(seen[0], 1, "{seen:?}");
    assert!(
        seen[1] >= 4,
        "the clear-only flush dropped the items' pipelines: {seen:?}"
    );
    assert_eq!(
        seen[1], seen[2],
        "no pipeline is built again in a frame like the last: {seen:?}"
    );
}
