//! `device_look`'s picture: the framebuffer itself (not a ring slot, so it is the current screen even while the
//! stream is paused), made point sized by a Metal pass (each pixel the average of a scale x scale block, Display P3
//! to sRGB), read back, turned to the interface's orientation and encoded as PNG. At point size its pixels are the
//! tap coordinates (spec, builder's call 6): a pixel at (x, y) is where a tap at (x, y) lands.
//!
//! The PNG is encoded with the `png` crate the client already uses for still pictures, not ImageIO (no new
//! framework binding for one call).

use objc2_io_surface::IOSurfaceRef;

use super::ring::{Gpu, read_row, rgba_surface};
use crate::geom::Orientation;

/// Turns a portrait RGBA picture of `w` x `h` to the interface's orientation: its size and pixels. The pixel shown
/// at (x, y) is the portrait pixel `geom::to_portrait_ratio` maps (x, y) to, so the picture and touches agree.
pub fn turn(rgba: &[u8], w: usize, h: usize, o: Orientation) -> (usize, usize, Vec<u8>) {
    let (ow, oh) = match o {
        Orientation::Rot0 | Orientation::Rot180 => (w, h),
        Orientation::Rot90 | Orientation::Rot270 => (h, w),
    };
    if o == Orientation::Rot0 {
        return (w, h, rgba.to_vec());
    }
    let mut out = vec![0u8; ow * oh * 4];
    for y in 0..oh {
        for x in 0..ow {
            let (px, py) = match o {
                Orientation::Rot0 => (x, y),
                Orientation::Rot90 => (y, h - 1 - x),
                Orientation::Rot180 => (w - 1 - x, h - 1 - y),
                Orientation::Rot270 => (w - 1 - y, x),
            };
            let from = (py * w + px) * 4;
            let to = (y * ow + x) * 4;
            out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    (ow, oh, out)
}

/// RGBA pixels as a PNG.
pub fn encode(rgba: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(
            &mut out,
            u32::try_from(w).map_err(|_| "too wide")?,
            u32::try_from(h).map_err(|_| "too tall")?,
        );
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.set_compression(png::Compression::Fast);
        let mut wr = e.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// The current screen as a PNG at point size in the interface's orientation. BLOCKING (a GPU pass and the
/// encoding, tens of ms): the sim host's workers.
pub fn png(gpu: &Gpu, fb: &IOSurfaceRef, scale: f64, o: Orientation) -> Result<Vec<u8>, String> {
    let k = (scale.round() as u32).max(1);
    let (w, h) = (fb.width() / k as usize, fb.height() / k as usize);
    if w == 0 || h == 0 {
        return Err("an empty framebuffer".into());
    }
    let dst = rgba_surface(w, h).ok_or("IOSurfaceCreate failed")?;
    gpu.shrink(fb, &dst, k)?;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = read_row(&dst, y).ok_or("the snapshot did not read back")?;
        rgba.extend_from_slice(&row[..w * 4]);
    }
    let (ow, oh, px) = turn(&rgba, w, h, o);
    encode(&px, ow, oh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::decode_png;
    use crate::geom::{Size, to_portrait_ratio};
    use crate::macos::ring::{fill_surface, surface};

    #[test]
    fn a_turned_picture_shows_each_pixel_where_its_tap_lands() {
        // A 2 x 3 portrait picture, each pixel numbered.
        let (w, h) = (2usize, 3usize);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [i as u8, 0, 0, 255]).collect();
        for o in Orientation::ALL {
            let (ow, oh, out) = turn(&rgba, w, h, o);
            assert_eq!(
                Size::new(ow as f64, oh as f64),
                o.size(Size::new(w as f64, h as f64))
            );
            for y in 0..oh {
                for x in 0..ow {
                    // The centre of the shown pixel, through the touch map, lands in this portrait pixel.
                    let (rx, ry) = to_portrait_ratio(
                        x as f64 + 0.5,
                        y as f64 + 0.5,
                        Size::new(w as f64, h as f64),
                        o,
                    );
                    let (px, py) = ((rx * w as f64) as usize, (ry * h as f64) as usize);
                    assert_eq!(
                        out[(y * ow + x) * 4],
                        (py * w + px) as u8,
                        "{o:?} at {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_framebuffer_becomes_a_point_sized_png() {
        let gpu = Gpu::new().expect("a Metal device");
        // A 6 x 9 BGRA framebuffer at 3x: a 2 x 3 point picture.
        let fb = surface(6, 9, u32::from_be_bytes(*b"BGRA")).unwrap();
        fill_surface(&fb, [255, 255, 255, 255]);
        let png = png(&gpu, &fb, 3.0, Orientation::Rot90).unwrap();
        let (size, px) = decode_png(&png).unwrap();
        assert_eq!(size, Size::new(3.0, 2.0), "point size, turned");
        assert!(
            px.chunks(4).all(|p| p == [255, 255, 255, 255]),
            "white stays white: {px:?}"
        );
    }

    /// The real simulator (opt-in, `FLARE_DEVICE_REAL=1`): with the stream paused, the screen changes and the
    /// snapshot shows the new screen, not the last frame sent.
    #[test]
    fn a_snapshot_while_paused_is_the_current_screen() {
        use crate::macos::sim::{self, AttachOptions, Sim, Sink};
        use crate::source::Screen;
        use crate::target::Target;
        use crate::wire::Record;
        use std::sync::Arc;
        use std::time::Duration;

        let Some(d) = crate::macos::simctl::TestDevice::boot("Flare Test iPhone") else {
            return;
        };
        let (simctl, target, dev) = (&d.simctl, d.target.clone(), d.developer_dir.clone());
        std::thread::sleep(Duration::from_secs(5)); // the home screen settles
        /// Flare never hears a frame: each slot is free again at once.
        struct Nobody;
        impl Sink for Nobody {
            fn ring(
                &self,
                _: &Target,
                _: u32,
                _: u32,
                _: Screen,
                _: &Arc<crate::macos::ring::GpuRing>,
            ) {
            }
            fn record(&self, _: Record) -> bool {
                false
            }
            fn log(&self, _: &str) {}
        }
        let sim_ = Sim::open(&dev).expect("the device set");
        let gpu = Arc::new(Gpu::new().expect("a Metal device"));
        let attached = sim::attach(
            &sim_,
            &target,
            AttachOptions {
                sink: Arc::new(Nobody),
                gpu: Arc::clone(&gpu),
                ids: Arc::default(),
                probe_barcode: false,
                orientation: Arc::default(),
                on_props: None,
            },
        )
        .expect("attach");
        let stream = Arc::clone(&attached.stream);
        stream.pause(true);
        let shoot = || {
            let fb = stream.framebuffer().expect("a framebuffer");
            decode_png(&png(&gpu, &fb, stream.scale(), Orientation::Rot0).expect("a snapshot"))
                .unwrap()
        };
        let (size, home) = shoot();
        let (px, scale) = sim_.device(&target).unwrap().screen_facts().unwrap();
        assert_eq!(
            size,
            Size::new((px.w / scale).floor(), (px.h / scale).floor()),
            "point size"
        );
        // The screen changes while the stream is paused: Settings opens.
        simctl
            .launch(&target, "com.apple.Preferences", &[])
            .expect("launch Settings");
        std::thread::sleep(Duration::from_secs(3));
        let (_, settings) = shoot();
        // simctl's own screenshot of the same moment, at 3x: the snapshot matches it, not the home screen.
        let shot = decode_png(&simctl.screenshot(&target).expect("simctl screenshot")).unwrap();
        let k = (shot.0.w / size.w).round() as usize;
        let (w, h) = (size.w as usize, size.h as usize);
        let diff = |a: &[u8], b: &dyn Fn(usize, usize) -> [u8; 3]| {
            let mut total = 0u64;
            let mut n = 0u64;
            for y in (0..h).step_by(7) {
                for x in (0..w).step_by(7) {
                    let p = &a[(y * w + x) * 4..(y * w + x) * 4 + 3];
                    let q = b(x, y);
                    total += p
                        .iter()
                        .zip(q)
                        .map(|(p, q)| u64::from(p.abs_diff(q)))
                        .sum::<u64>();
                    n += 3;
                }
            }
            total as f64 / n as f64
        };
        let sw = shot.0.w as usize;
        let at_shot = |x: usize, y: usize| {
            let i = ((y * k + k / 2) * sw + x * k + k / 2) * 4;
            [shot.1[i], shot.1[i + 1], shot.1[i + 2]]
        };
        let to_settings = diff(&settings, &at_shot);
        let to_home = diff(&home, &at_shot);
        eprintln!(
            "mean difference to simctl's screenshot: snapshot {to_settings:.1}, home {to_home:.1}"
        );
        assert!(
            to_settings < 12.0,
            "the paused snapshot is the current screen: {to_settings}"
        );
        assert!(
            to_home > to_settings * 2.0,
            "and not the screen from before: {to_home}"
        );
        attached.detach();
    }
}
