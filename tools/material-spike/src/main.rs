//! material-spike: shows a small test window and measures how the see-through material behaves on this
//! PC (plan Task 19). This file holds the pure parts (argument parsing, DIB conversion, verdicts, the
//! JSON line); the window modes are in `session`, the GDI capture and the PNG writer in `capture`,
//! the no-window `probe` in `probe`.

mod capture;
mod probe;
mod session;

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Probe,
    GlassGpu,
    Premul,
    Border,
    GlassSoftware,
    Readback,
    Cost,
    /// One measured run, spawned ten times by `cost`.
    CostOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swapchain {
    Visual,
    Hwnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    Gpu,
    Software,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    pub mode: Mode,
    pub level: u32,
    pub swapchain: Swapchain,
    pub renderer: Renderer,
    pub seconds: f64,
    pub out: Option<String>,
    /// `--acrylic off` skips the backdrop call (isolates what the swapchain alone does).
    pub acrylic: bool,
    /// `--backdrop none` shows no white helper window under the test window.
    pub backdrop: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ArgError(pub String);

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn mode_of(s: &str) -> Option<Mode> {
    Some(match s {
        "probe" => Mode::Probe,
        "glass-gpu" => Mode::GlassGpu,
        "premul" => Mode::Premul,
        "border" => Mode::Border,
        "glass-software" => Mode::GlassSoftware,
        "readback" => Mode::Readback,
        "cost" => Mode::Cost,
        "cost-once" => Mode::CostOnce,
        _ => return None,
    })
}

fn on_off(v: &str, key: &str) -> Result<bool, ArgError> {
    match v {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(ArgError(format!("{key} takes on or off, not {v}"))),
    }
}

/// Parses `--mode <name> [--level N] [--swapchain visual|hwnd] [--renderer gpu|software] [--seconds S]
/// [--out file.png]` (the program name is not in `argv`).
pub fn parse_args(argv: &[String]) -> Result<Args, ArgError> {
    let mut mode = None;
    let mut a = Args {
        mode: Mode::Probe,
        level: 40,
        swapchain: Swapchain::Visual,
        renderer: Renderer::Gpu,
        seconds: 6.0,
        out: None,
        acrylic: true,
        backdrop: true,
    };
    let mut i = 0;
    while i < argv.len() {
        let key = argv[i].as_str();
        let v = argv
            .get(i + 1)
            .map(String::as_str)
            .ok_or_else(|| ArgError(format!("{key} needs a value")))?;
        match key {
            "--mode" => {
                mode = Some(mode_of(v).ok_or_else(|| ArgError(format!("unknown mode {v}")))?)
            }
            "--level" => {
                a.level = v.parse().map_err(|_| ArgError(format!("bad level {v}")))?;
                if a.level > 100 {
                    return Err(ArgError(format!("level {v} is over 100")));
                }
            }
            "--swapchain" => {
                a.swapchain = match v {
                    "visual" => Swapchain::Visual,
                    "hwnd" => Swapchain::Hwnd,
                    _ => return Err(ArgError(format!("bad swapchain {v}"))),
                }
            }
            "--renderer" => {
                a.renderer = match v {
                    "gpu" => Renderer::Gpu,
                    "software" => Renderer::Software,
                    _ => return Err(ArgError(format!("bad renderer {v}"))),
                }
            }
            "--seconds" => {
                a.seconds = v
                    .parse()
                    .map_err(|_| ArgError(format!("bad seconds {v}")))?;
                if a.seconds.is_nan() || a.seconds <= 0.0 {
                    return Err(ArgError(format!("seconds {v} must be positive")));
                }
            }
            "--out" => a.out = Some(v.to_string()),
            "--acrylic" => a.acrylic = on_off(v, key)?,
            "--backdrop" => {
                a.backdrop = match v {
                    "white" => true,
                    "none" => false,
                    _ => return Err(ArgError(format!("bad backdrop {v}"))),
                }
            }
            _ => return Err(ArgError(format!("unknown option {key}"))),
        }
        i += 2;
    }
    a.mode = mode.ok_or_else(|| ArgError("--mode is required".into()))?;
    Ok(a)
}

/// A GDI 32-bit DIB (BGRA, alpha undefined) as straight RGBA with alpha 255.
pub fn dib_to_rgba(bgra: &[u8], width: usize, height: usize, bottom_up: bool) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    for y in 0..height {
        let src_y = if bottom_up { height - 1 - y } else { y };
        for x in 0..width {
            let s = (src_y * width + x) * 4;
            let d = (y * width + x) * 4;
            out[d] = bgra[s + 2];
            out[d + 1] = bgra[s + 1];
            out[d + 2] = bgra[s];
            out[d + 3] = 255;
        }
    }
    out
}

/// The RGB of the pixel at (x, y) in an RGBA buffer.
pub fn pixel(rgba: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
    let i = (y * width + x) * 4;
    [rgba[i], rgba[i + 1], rgba[i + 2]]
}

fn max_channel_diff(a: [u8; 3], b: [u8; 3]) -> u8 {
    (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0)
}

/// Standard deviation of the per-pixel channel-sum difference between two equally sized RGBA buffers.
pub fn diff_stddev(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len()) / 4;
    if n == 0 {
        return 0.0;
    }
    let d: Vec<f64> = (0..n)
        .map(|p| {
            (0..3)
                .map(|c| f64::from(a[p * 4 + c]) - f64::from(b[p * 4 + c]))
                .sum::<f64>()
        })
        .collect();
    let mean = d.iter().sum::<f64>() / n as f64;
    (d.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64).sqrt()
}

/// E1: the glass shows the desktop through it. The centre pixel differs from the closed capture by more
/// than 6 on some channel, and the difference image is not flat.
pub fn e1_glass_works(centre_open: [u8; 3], centre_closed: [u8; 3], stddev: f64) -> bool {
    max_channel_diff(centre_open, centre_closed) > 6 && stddev > 0.0
}

/// E2: at alpha 1.0 the centre is exactly the ground.
pub fn e2_alpha_one_is_opaque(centre: [u8; 3], ground: [u8; 3]) -> bool {
    centre == ground
}

#[derive(Debug, PartialEq, Eq)]
pub enum Premul {
    Premultiplied,
    Straight,
    Unclear,
}

/// E3: a (128, 0, 0, 128) texel over white. Read as premultiplied it gives R 255, G 127; read as straight
/// it gives R 191, G 127, so only R tells them apart. Tolerance 4.
pub fn e3_premultiplied(centre: [u8; 3]) -> Premul {
    let near = |v: u8, t: u8| v.abs_diff(t) <= 4;
    if near(centre[0], 255) && near(centre[1], 127) {
        Premul::Premultiplied
    } else if near(centre[0], 191) {
        Premul::Straight
    } else {
        Premul::Unclear
    }
}

/// E4: the left edge column is the border colour all the way down.
pub fn e4_border_visible(column: &[[u8; 3]], border: [u8; 3]) -> bool {
    !column.is_empty() && column.iter().all(|p| *p == border)
}

/// Median of a set of timings (mean of the middle two when even). None when empty.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let m = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[m]
    } else {
        (v[m - 1] + v[m]) / 2.0
    })
}

/// A string as a JSON string literal (the tool prints one JSON line and needs no serde).
pub fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `{"k":v,...}` from pre-rendered JSON values, in the order given.
pub fn json_line(fields: &[(&str, String)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{}:{}", json_str(k), v))
        .collect();
    format!("{{{}}}", body.join(","))
}

/// The number after `"key":` in a one-line JSON object.
pub fn num_field(line: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\":");
    let rest = &line[line.find(&pat)? + pat.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn main() {
    let t0 = std::time::Instant::now();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let a = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("material-spike: {e}");
            std::process::exit(2);
        }
    };
    let out = match a.mode {
        Mode::Probe => probe::run(),
        Mode::Cost => session::cost(&a),
        _ => session::run(&a, t0),
    };
    match out {
        Ok(line) => println!("{line}"),
        Err(e) => {
            println!("{}", json_line(&[("error", json_str(&e))]));
            eprintln!("material-spike: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_every_option() {
        let a = parse_args(&s(&[
            "--mode",
            "glass-gpu",
            "--level",
            "40",
            "--swapchain",
            "hwnd",
            "--renderer",
            "software",
            "--seconds",
            "2.5",
            "--out",
            "a.png",
        ]))
        .unwrap();
        assert_eq!(a.mode, Mode::GlassGpu);
        assert_eq!(a.level, 40);
        assert_eq!(a.swapchain, Swapchain::Hwnd);
        assert_eq!(a.renderer, Renderer::Software);
        assert_eq!(a.seconds, 2.5);
        assert_eq!(a.out.as_deref(), Some("a.png"));
    }

    #[test]
    fn defaults_and_errors() {
        let a = parse_args(&s(&["--mode", "probe"])).unwrap();
        assert_eq!(
            (a.level, a.seconds, a.swapchain),
            (40, 6.0, Swapchain::Visual)
        );
        assert!(a.acrylic && a.backdrop);
        let b = parse_args(&s(&[
            "--mode",
            "probe",
            "--acrylic",
            "off",
            "--backdrop",
            "none",
        ]))
        .unwrap();
        assert!(!b.acrylic && !b.backdrop);
        assert!(parse_args(&s(&["--mode", "probe", "--acrylic", "maybe"])).is_err());
        assert!(parse_args(&s(&[])).is_err());
        assert!(parse_args(&s(&["--mode", "nope"])).is_err());
        assert!(parse_args(&s(&["--mode", "probe", "--level", "101"])).is_err());
        assert!(parse_args(&s(&["--mode", "probe", "--level"])).is_err());
        assert!(parse_args(&s(&["--mode", "probe", "--x", "1"])).is_err());
        assert!(parse_args(&s(&["--mode", "probe", "--seconds", "0"])).is_err());
    }

    #[test]
    fn dib_swaps_channels_and_flips_bottom_up() {
        let bgra = [1, 2, 3, 0, 10, 20, 30, 0];
        let top_down = dib_to_rgba(&bgra, 1, 2, false);
        assert_eq!(top_down, vec![3, 2, 1, 255, 30, 20, 10, 255]);
        let flipped = dib_to_rgba(&bgra, 1, 2, true);
        assert_eq!(flipped, vec![30, 20, 10, 255, 3, 2, 1, 255]);
        assert_eq!(pixel(&flipped, 1, 0, 1), [3, 2, 1]);
    }

    #[test]
    fn stddev_is_zero_for_a_flat_difference() {
        let a = vec![10u8; 16];
        let b = vec![4u8; 16];
        assert_eq!(diff_stddev(&a, &b), 0.0);
        let mut c = b.clone();
        c[0] = 40;
        assert!(diff_stddev(&a, &c) > 0.0);
        assert_eq!(diff_stddev(&[], &[]), 0.0);
    }

    #[test]
    fn verdicts() {
        assert!(e1_glass_works([30, 30, 30], [23, 23, 23], 2.0));
        assert!(!e1_glass_works([23, 23, 23], [23, 23, 23], 2.0));
        assert!(!e1_glass_works([30, 30, 30], [23, 23, 23], 0.0));
        assert!(e2_alpha_one_is_opaque([23, 23, 23], [23, 23, 23]));
        assert!(!e2_alpha_one_is_opaque([24, 23, 23], [23, 23, 23]));
        assert_eq!(e3_premultiplied([255, 127, 127]), Premul::Premultiplied);
        assert_eq!(e3_premultiplied([191, 127, 127]), Premul::Straight);
        assert_eq!(e3_premultiplied([10, 10, 10]), Premul::Unclear);
        assert!(e4_border_visible(&[[255, 0, 255]; 5], [255, 0, 255]));
        assert!(!e4_border_visible(
            &[[255, 0, 255], [0, 0, 0]],
            [255, 0, 255]
        ));
        assert!(!e4_border_visible(&[], [255, 0, 255]));
    }

    #[test]
    fn median_odd_even_empty() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn num_field_reads_a_number_and_misses_cleanly() {
        let l = "{\"mode\":\"cost-once\",\"start_ms\":212.50,\"frame_ms\":16.7}";
        assert_eq!(num_field(l, "start_ms"), Some(212.5));
        assert_eq!(num_field(l, "frame_ms"), Some(16.7));
        assert_eq!(num_field(l, "nope"), None);
        assert_eq!(
            parse_args(&s(&["--mode", "cost-once"])).unwrap().mode,
            Mode::CostOnce
        );
    }

    #[test]
    fn json_escapes_and_orders() {
        assert_eq!(json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        let l = json_line(&[("a", "1".into()), ("b", json_str("x"))]);
        assert_eq!(l, "{\"a\":1,\"b\":\"x\"}");
    }
}
