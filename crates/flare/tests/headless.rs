//! Runs the built binary without a window: backend probes, the headless screenshots and the live
//! tab's start-up check.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

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

/// The arguments of a 600x300 live screenshot of `cmd` to `png`.
fn live_args(png: &Path) -> Vec<String> {
    [
        "--live-screenshot",
        png.to_str().expect("utf-8 path"),
        "--live-shell",
        "cmd",
        "--renderer",
        "software",
        "--size",
        "600x300",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[test]
fn live_screenshot_shows_the_typed_command() {
    let dir = TempDir::new("live");
    let png = dir.png();
    let args = live_args(&png);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run(&refs, &[]);
    assert!(
        out.status.success(),
        "exit {:?}, stderr {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let img = decode(&png);
    assert_eq!((img.0, img.1), (600, 300));
    let mut ink = 0;
    for y in 37..img.1 {
        for x in 0..img.0 {
            if pixel(&img, x, y) != GROUND {
                ink += 1;
            }
        }
    }
    assert!(ink > 100, "only {ink} pixels differ from the ground");
}

/// Process ids of every process whose parent is `parent`, from a process snapshot.
fn children_of(parent: u32) -> Vec<u32> {
    let mut found = Vec::new();
    // SAFETY: the snapshot handle is checked against INVALID_HANDLE_VALUE and closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    assert_ne!(snap, INVALID_HANDLE_VALUE, "process snapshot");
    // SAFETY: an all-zero PROCESSENTRY32W is a valid starting value; dwSize is set before use.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is a live snapshot and `entry` is a writable, correctly sized struct.
    let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while more {
        if entry.th32ParentProcessID == parent {
            found.push(entry.th32ProcessID);
        }
        // SAFETY: as for Process32FirstW above.
        more = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }
    // SAFETY: `snap` came from CreateToolhelp32Snapshot and is closed exactly once.
    unsafe { CloseHandle(snap) };
    found
}

#[test]
fn live_screenshot_leaves_no_shell_behind() {
    let dir = TempDir::new("leak");
    let png = dir.png();
    let mut child = Command::new(env!("CARGO_BIN_EXE_flare"))
        .args(live_args(&png))
        .env_remove("FLARE_RENDERER")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn flare");
    let pid = child.id();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll flare") {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let status = status.expect("flare did not exit within 30 s (shutdown hung)");
    assert!(status.success(), "exit {:?}", status.code());
    assert_eq!(
        children_of(pid),
        Vec::<u32>::new(),
        "a shell or OpenConsole started by flare is still running"
    );
}

#[test]
fn a_missing_bundle_fails_loudly_and_exits_three() {
    let dir = TempDir::new("nobundle");
    let empty = dir.0.join("empty");
    std::fs::create_dir_all(&empty).expect("create empty folder");
    let png = dir.png();
    let args = live_args(&png);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run(
        &refs,
        &[("FLARE_CONPTY_DIR", empty.to_str().expect("utf-8 path"))],
    );
    assert_eq!(out.status.code(), Some(3), "exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("conpty.dll"), "stderr was {stderr:?}");
    assert!(
        !png.exists(),
        "no screenshot is written after a failed check"
    );
}
