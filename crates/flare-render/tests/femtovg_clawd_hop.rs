//! Regression rules 1202 and 1203 on the GPU renderer: Clawd's working hop draws the same pixels for the same box, on the
//! renderer the Mac uses (femtovg on wgpu, Metal there) at the Mac's Retina scale (2.0) and at 1.0 (a plain
//! external display). The software-renderer test (`ui_tests::marks::every_frame_of_the_hop_draws_clawd_on_whole_
//! device_pixels`) cannot see femtovg's anti-aliasing, so this one draws the hop the way Slint's femtovg item renderer
//! (`i-slint-renderer-femtovg` 1.18.1, `draw_path`) draws `AgentIcon`'s Clawd: the path fitted to the box in logical
//! px (`fit: fill`), each point times the window's scale, the canvas moved to the box's place in device px.
//!
//! The box comes from `flare_agent::icon_motion::icon_box`, the rule `tabmark.slint` follows (`Px.snap` against the
//! icon's `absolute-position`, with `Px.scale` the window's scale factor, `1px / 1phx`). A window's place on the
//! screen never enters: femtovg draws into the window's own surface, whose origin is the content's top left, so only
//! the icon's place in the window (often a fraction of a logical px on the Mac) matters, and the origins below are
//! fractional on purpose.

use std::collections::HashMap;

use femtovg::{Canvas, Color, Paint, Path, renderer::WGPURenderer};
use flare_agent::icon::{CLAWD_FEET, CLAWD_VIEWBOX, clawd_path, clawd_shut_path};
use flare_agent::icon_motion::{HOP_MS, hop_at, icon_box};

/// 512 x 4 bytes is a whole number of wgpu's 256-byte rows.
const W: u32 = 512;
const H: u32 = 96;
const SIZE: f32 = 18.0;
const GROUND: Color = Color::rgbf(0.09, 0.09, 0.09);
const ORANGE: [u8; 3] = [0xd7, 0x77, 0x57];

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some(Gpu { device, queue })
}

/// The cells of a Clawd path (`M{x} {y} h1.02 v2.04 h-1.02 z`, `icon::clawd_cells_path`) as their top-left corners.
fn cells(path: &str) -> Vec<(f32, f32)> {
    path.split('M')
        .filter(|c| !c.trim().is_empty())
        .map(|c| {
            let mut n = c.split_whitespace();
            let x: f32 = n.next().unwrap().parse().unwrap();
            let y: f32 = n.next().unwrap().parse().unwrap();
            (x, y)
        })
        .collect()
}

struct Painter {
    g: Gpu,
    canvas: Canvas<WGPURenderer>,
    target: wgpu::Texture,
    read: wgpu::Buffer,
}

impl Painter {
    fn new(g: Gpu) -> Self {
        let target = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("clawd hop target"),
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let read = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("clawd hop readback"),
            size: u64::from(W * H * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let canvas =
            Canvas::new(WGPURenderer::new(g.device.clone(), g.queue.clone())).expect("a canvas");
        Self {
            g,
            canvas,
            target,
            read,
        }
    }

    /// Draws `path`'s cells fitted to a box of `w` x `h` device px at (`left`, `top`) device px, as Slint's femtovg
    /// item renderer does at `scale`, and reads the frame back as straight RGB rows.
    fn draw(&mut self, path: &str, scale: f32, (left, top, w, h): (f32, f32, f32, f32)) -> Frame {
        let (vx, vy, vw, vh) = CLAWD_VIEWBOX;
        // The box in logical px (what `fit: fill` fits the view box to), then each point times the scale.
        let (lw, lh) = (w / scale, h / scale);
        let pt = |x: f32, y: f32| ((x - vx) * lw / vw * scale, (y - vy) * lh / vh * scale);
        let c = &mut self.canvas;
        c.set_size(W, H, 1.0);
        c.clear_rect(0, 0, W, H, GROUND);
        c.save();
        c.translate(left, top);
        let mut p = Path::new();
        for (x, y) in cells(path) {
            let (x0, y0) = pt(x, y);
            let (x1, y1) = pt(x + 1.02, y + 2.04);
            p.move_to(x0, y0);
            p.line_to(x1, y0);
            p.line_to(x1, y1);
            p.line_to(x0, y1);
            p.close();
        }
        c.fill_path(
            &p,
            &Paint::color(Color::rgb(ORANGE[0], ORANGE[1], ORANGE[2])),
        );
        c.restore();
        self.g.queue.submit(c.flush_to_output(&self.target));
        let mut enc = self
            .g
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        enc.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.read,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(W * 4),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
        );
        self.g.queue.submit([enc.finish()]);
        let slice = self.read.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("the readback maps"));
        let _ = self.g.device.poll(wgpu::PollType::wait_indefinitely());
        let px = slice
            .get_mapped_range()
            .expect("the mapped range")
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| [b[2], b[1], b[0]])
            .collect();
        self.read.unmap();
        Frame(px)
    }
}

struct Frame(Vec<[u8; 3]>);

/// Where a box size was first drawn, and its pixels.
type Drawn = (String, Vec<[u8; 3]>);

impl Frame {
    fn px(&self, x: u32, y: u32) -> [u8; 3] {
        self.0[(y * W + x) as usize]
    }

    /// Whether anything but the ground is drawn at (x, y).
    fn ink(&self, x: u32, y: u32) -> bool {
        self.px(x, y).iter().any(|&c| c.abs_diff(23) > 2)
    }

    fn crop(&self, (left, top, w, h): (u32, u32, u32, u32)) -> Vec<[u8; 3]> {
        let mut v = Vec::new();
        for y in top - 2..top + h + 2 {
            for x in left - 2..left + w + 2 {
                v.push(self.px(x, y));
            }
        }
        v
    }
}

fn near(a: &[[u8; 3]], b: &[[u8; 3]]) -> bool {
    a.iter()
        .zip(b)
        .all(|(p, q)| p.iter().zip(q).all(|(x, y)| x.abs_diff(*y) <= 2))
}

#[test]
fn every_frame_of_the_hop_draws_clawd_alike_on_the_gpu_at_retina_and_plain_scale() {
    let Some(g) = gpu() else {
        eprintln!("skipping: no wgpu adapter");
        return;
    };
    let mut p = Painter::new(g);
    let body = clawd_path();
    for scale in [2.0f32, 1.0] {
        // Each box size, with the first frame and place that drew it and its pixels.
        let mut seen: HashMap<(u32, u32), Drawn> = HashMap::new();
        // Whole, half and quarter logical px: where a Mac strip may put the icon in the window.
        for origin in [(100.0f32, 20.0f32), (100.25, 20.5), (37.5, 27.75)] {
            for ms in (0..HOP_MS).step_by(16) {
                let (rise, sx, sy) = hop_at(ms);
                let b = icon_box(origin, SIZE, scale, (sx, sy, rise), Some(CLAWD_FEET));
                let f = p.draw(&body, scale, b);
                let bu = (b.0 as u32, b.1 as u32, b.2 as u32, b.3 as u32);
                // Nothing is drawn further out than the anti-aliased fringe, one device pixel round the box.
                for y in 0..H {
                    for x in 0..W {
                        let inside = x + 1 >= bu.0
                            && x < bu.0 + bu.2 + 1
                            && y + 1 >= bu.1
                            && y < bu.1 + bu.3 + 1;
                        assert!(
                            inside || !f.ink(x, y),
                            "{ms} ms at {scale}, origin {origin:?}: ink at ({x}, {y}) outside the box {b:?}"
                        );
                    }
                }
                let crop = f.crop(bu);
                let at = format!("{ms} ms at {origin:?}");
                match seen.get(&(bu.2, bu.3)) {
                    Some((first, want)) => assert!(
                        near(want, &crop),
                        "at {scale}: {at} draws a {} x {} box unlike {first} did",
                        bu.2,
                        bu.3
                    ),
                    None => {
                        seen.insert((bu.2, bu.3), (at, crop));
                    }
                }
            }
        }
        assert!(seen.len() > 2, "the hop squashes and stretches at {scale}");
    }
    // The check can see a smear: the rest box moved by half a device pixel draws other pixels.
    let b = icon_box((100.0, 20.0), SIZE, 2.0, (1.0, 1.0, 0.0), Some(CLAWD_FEET));
    let bu = (b.0 as u32, b.1 as u32, b.2 as u32, b.3 as u32);
    let whole = p.draw(&body, 2.0, b).crop(bu);
    let half = p.draw(&body, 2.0, (b.0 + 0.5, b.1, b.2, b.3)).crop(bu);
    assert!(!near(&whole, &half), "half a device pixel shows");
}

/// The blink is one shape on the GPU too: the shut eyes are Clawd's full colour, with no seam where an eye meets the
/// head (a lid laid over the body left one).
#[test]
fn the_shut_clawd_has_no_seam_on_the_gpu_at_retina_scale() {
    let Some(g) = gpu() else {
        eprintln!("skipping: no wgpu adapter");
        return;
    };
    let mut p = Painter::new(g);
    let scale = 2.0;
    let b = icon_box(
        (100.0, 20.0),
        SIZE,
        scale,
        (1.0, 1.0, 0.0),
        Some(CLAWD_FEET),
    );
    let open = p.draw(&clawd_path(), scale, b);
    let shut = p.draw(&clawd_shut_path(), scale, b);
    let (vx, vy, vw, vh) = CLAWD_VIEWBOX;
    let at = |x: f32, y: f32| {
        (
            (b.0 + (x - vx) / vw * b.2) as u32,
            (b.1 + (y - vy) / vh * b.3) as u32,
        )
    };
    let full = |c: [u8; 3]| c.iter().zip(ORANGE).all(|(a, o)| a.abs_diff(o) <= 2);
    // The left eye (cell 5, 1: x 5 to 6.02, y 2 to 4.04): its middle, and the edges it shares with the head.
    for (x, y) in [(5.5, 3.0), (5.02, 3.0), (6.0, 3.0), (5.5, 2.02), (5.5, 4.0)] {
        let (px, py) = at(x, y);
        assert!(
            full(shut.px(px, py)),
            "shut, ({x}, {y}) at ({px}, {py}): {:?}",
            shut.px(px, py)
        );
    }
    let (px, py) = at(5.5, 3.0);
    assert!(!full(open.px(px, py)), "the open eye is open");
}
