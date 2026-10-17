//! Runs the built binary without a window: backend probes and the headless screenshot.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flare"));
    cmd.args(args);
    cmd.env_remove("FLARE_FORCE_GPU_FAIL");
    cmd.env_remove("FLARE_RENDERER");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run flare")
}

/// A unique scratch folder under the system temp dir, removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("flare-headless-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    fn png(&self) -> PathBuf {
        self.0.join("shot.png")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A decoded RGBA8 image: width, height and the packed bytes.
type Decoded = (u32, u32, Vec<u8>);

/// Decodes an RGBA8 PNG.
fn decode(path: &Path) -> Decoded {
    let file = std::fs::File::open(path).expect("screenshot exists");
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .expect("png header");
    let mut buf = vec![0; reader.output_buffer_size().expect("png size")];
    let info = reader.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn pixel(img: &Decoded, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.0 + x) * 4) as usize;
    [img.2[i], img.2[i + 1], img.2[i + 2]]
}

const RED: [u8; 3] = [0xff, 0x61, 0x5a];
const GROUND: [u8; 3] = [0x17, 0x17, 0x17];

/// The properties both renderers must show in a 400x300 screenshot at 100%.
fn assert_screenshot(img: &Decoded) {
    assert_eq!((img.0, img.1), (400, 300));
    assert_eq!(pixel(img, 399, 299), GROUND, "bottom-right is the ground");
    let red_below_bar = (37..img.1).any(|y| (0..img.0).any(|x| pixel(img, x, y) == RED));
    assert!(red_below_bar, "the grid shows the red block below the bar");
    if Path::new(r"C:\Windows\Fonts\CascadiaMono.ttf").exists() {
        // Cell (5, 1) is the trailing space of the " 1 " block, so no glyph covers it.
        assert_eq!(pixel(img, 48, 64), RED, "centre of cell (5, 1)");
    }
}

/// Runs `--screenshot` at 400x300 with the given renderer. None when the GPU is unavailable
/// (exit code 2), so the caller can skip.
fn screenshot(renderer: &str, tag: &str) -> Option<(TempDir, Decoded)> {
    let dir = TempDir::new(tag);
    let png = dir.png();
    let out = run(
        &[
            "--screenshot",
            png.to_str().expect("utf-8 path"),
            "--renderer",
            renderer,
            "--size",
            "400x300",
        ],
        &[],
    );
    if renderer == "gpu" && out.status.code() == Some(2) {
        eprintln!(
            "skipping: no GPU adapter ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    assert!(
        out.status.success(),
        "exit {:?}, stderr {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(png.exists(), "the screenshot file was written");
    let img = decode(&png);
    Some((dir, img))
}

#[test]
fn probe_forced_gpu_failure_falls_back_to_software() {
    let out = run(
        &["--probe-backend", "--renderer", "gpu"],
        &[("FLARE_FORCE_GPU_FAIL", "1")],
    );
    assert!(out.status.success(), "exit {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("backend: software fallback:"),
        "stdout was {stdout:?}"
    );
    assert!(
        stdout.contains("FLARE_FORCE_GPU_FAIL"),
        "stdout was {stdout:?}"
    );
}

#[test]
fn probe_software_reports_software() {
    let out = run(&["--probe-backend", "--renderer", "software"], &[]);
    assert!(out.status.success(), "exit {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim_end(), "backend: software");
}

#[test]
fn headless_cpu_screenshot_writes_png() {
    let (_dir, img) = screenshot("software", "cpu").expect("software never skips");
    assert_screenshot(&img);
}

#[test]
fn headless_gpu_screenshot_writes_png() {
    if let Some((_dir, img)) = screenshot("gpu", "gpu") {
        assert_screenshot(&img);
    }
}
