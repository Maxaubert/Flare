//! GDI screen capture of a window's rectangle into straight RGBA, and the PNG writer.

use crate::dib_to_rgba;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

/// A captured rectangle as straight RGBA, alpha 255.
pub struct Shot {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Shot {
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        crate::pixel(&self.rgba, self.width, x, y)
    }
    pub fn centre(&self) -> [u8; 3] {
        self.pixel(self.width / 2, self.height / 2)
    }
}

/// PNG bytes of a straight RGBA buffer.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

pub fn save_png(path: &str, shot: &Shot) -> Result<(), String> {
    let bytes = encode_png(shot.width as u32, shot.height as u32, &shot.rgba)?;
    let dir = std::path::Path::new(path).parent();
    if let Some(dir) = dir.filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

/// The window's outer rectangle in screen pixels.
pub fn window_rect(hwnd: isize) -> Option<Rect> {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `r` is a valid, writable RECT; a bad handle makes the call return 0.
    let ok = unsafe { GetWindowRect(hwnd as HWND, &mut r) };
    (ok != 0).then_some(Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    })
}

/// The primary screen's size in pixels.
pub fn screen_size() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: GetSystemMetrics has no preconditions.
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// Copies the screen rectangle (what DWM composed, layered windows included) into a 32-bit DIB.
pub fn capture(r: Rect) -> Option<Shot> {
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, SRCCOPY,
    };
    let (w, h) = (r.width(), r.height());
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut buf = vec![0u8; w as usize * h as usize * 4];
    // SAFETY: every GDI object made here is released before returning; `buf` holds exactly
    // w * h 32-bit pixels, the size GetDIBits writes for this header.
    let copied = unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        let blt = BitBlt(mem, 0, 0, w, h, screen, r.left, r.top, SRCCOPY | CAPTUREBLT);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h; // top-down
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            buf.as_mut_ptr().cast(),
            &mut bi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        blt != 0 && lines == h
    };
    copied.then(|| Shot {
        width: w as usize,
        height: h as usize,
        rgba: dib_to_rgba(&buf, w as usize, h as usize, false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trips_a_synthetic_dib() {
        // a 2 x 2 bottom-up BGRA DIB: bottom row (blue, green), top row (red, white)
        let bgra = [
            255, 0, 0, 0, 0, 255, 0, 0, // bottom row: B then G
            0, 0, 255, 0, 255, 255, 255, 0, // top row: R then W
        ];
        let rgba = dib_to_rgba(&bgra, 2, 2, true);
        let bytes = encode_png(2, 2, &rgba).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        let mut back = vec![0u8; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut back).unwrap();
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(&back[..info.buffer_size()], &rgba[..]);
        assert_eq!(&back[0..4], &[255, 0, 0, 255], "top-left is red");
        assert_eq!(&back[12..16], &[0, 255, 0, 255], "bottom-right is green");
    }

    #[test]
    fn shot_centre_and_rect_size() {
        let shot = Shot {
            width: 3,
            height: 3,
            rgba: (0..9u8).flat_map(|i| [i, 0, 0, 255]).collect(),
        };
        assert_eq!(shot.centre(), [4, 0, 0]);
        let r = Rect {
            left: 10,
            top: 20,
            right: 410,
            bottom: 320,
        };
        assert_eq!((r.width(), r.height()), (400, 300));
    }

    #[test]
    fn a_degenerate_rect_captures_nothing() {
        let r = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 5,
        };
        assert!(capture(r).is_none());
    }
}
