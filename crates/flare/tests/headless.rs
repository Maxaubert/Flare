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

/// The scenes `--scene` takes, in the order the screenshot plan lists them.
const SCENE_NAMES: [&str; 12] = [
    "strip-3",
    "strip-hover",
    "strip-many",
    "strip-list",
    "sidebar-open",
    "sidebar-find",
    "sidebar-collapsed",
    "start",
    "menu-places",
    "menu-tab",
    "dialog-close",
    "tip",
];

/// Runs `--scene <name>` at `scale` (software renderer, 1000x640 logical) and decodes the PNG.
fn scene_png(name: &str, scale: &str) -> (TempDir, Decoded) {
    let dir = TempDir::new(&format!("scene-{name}-{}", scale.replace('.', "_")));
    let png = dir.png();
    let out = run(
        &[
            "--scene",
            name,
            "--screenshot",
            png.to_str().expect("utf-8 path"),
            "--scale",
            scale,
            "--size",
            "1000x640",
            "--renderer",
            "software",
        ],
        &[],
    );
    assert!(
        out.status.success(),
        "{name} at {scale}: exit {:?}, stderr {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let img = decode(&png);
    (dir, img)
}

#[test]
fn every_scene_renders_at_the_three_scales() {
    for name in SCENE_NAMES {
        for scale in ["1", "1.5", "2.25"] {
            let (_dir, img) = scene_png(name, scale);
            let s: f32 = scale.parse().expect("scale");
            let want = ((1000.0 * s).round() as u32, (640.0 * s).round() as u32);
            assert_eq!((img.0, img.1), want, "{name} at {scale}");
            let corner = pixel(&img, 0, 0);
            let mut differ = 0;
            for y in 0..img.1 {
                for x in 0..img.0 {
                    if pixel(&img, x, y) != corner {
                        differ += 1;
                    }
                }
            }
            assert!(
                differ > 500,
                "{name} at {scale}: only {differ} pixels differ from the corner"
            );
        }
    }
}

#[test]
fn an_unknown_scene_is_an_error() {
    let dir = TempDir::new("scene-unknown");
    let png = dir.png();
    let out = run(
        &[
            "--scene",
            "no-such-scene",
            "--screenshot",
            png.to_str().expect("utf-8 path"),
            "--renderer",
            "software",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1), "exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    for name in SCENE_NAMES {
        assert!(stderr.contains(name), "stderr names {name}: {stderr:?}");
    }
    assert!(!png.exists(), "no screenshot for an unknown scene");
}

/// The M5 scenes (Task 26): the terminal's selection, find, links, menus and the Copied badge.
const M5_SCENES: [&str; 12] = [
    "term-selection",
    "term-selection-wide",
    "term-find",
    "term-find-none",
    "term-find-many",
    "term-find-empty",
    "term-links",
    "term-menu-plain",
    "term-menu-selection",
    "term-menu-link",
    "term-menu-path",
    "term-copied",
];

/// Every display scaling from 100% to 300% (roadmap Global Constraint).
const MATRIX: [f32; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

#[test]
fn every_m5_scene_renders_at_every_scale() {
    for name in M5_SCENES {
        for s in MATRIX {
            let scale = format!("{s}");
            let (_dir, img) = scene_png(name, &scale);
            let want = ((1000.0 * s).round() as u32, (640.0 * s).round() as u32);
            assert_eq!((img.0, img.1), want, "{name} at {scale}");
            let corner = pixel(&img, 0, 0);
            let mut differ = 0;
            for y in 0..img.1 {
                for x in 0..img.0 {
                    if pixel(&img, x, y) != corner {
                        differ += 1;
                    }
                }
            }
            assert!(
                differ > 500,
                "{name} at {scale}: only {differ} pixels differ from the corner"
            );
        }
    }
}

/// Counts the pixels of an image equal to `colour`.
fn count_colour(img: &Decoded, colour: [u8; 3]) -> usize {
    let mut n = 0;
    for y in 0..img.1 {
        for x in 0..img.0 {
            if pixel(img, x, y) == colour {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn the_find_scene_shows_the_match_fill() {
    let (_dir, img) = scene_png("term-find", "1");
    let matches = count_colour(&img, [0x4a, 0x51, 0x60]);
    assert!(matches > 100, "only {matches} match-fill pixels");
    let active = count_colour(&img, [0xf6, 0xa2, 0x5b]);
    assert!(active > 50, "only {active} active-fill pixels");
}

#[test]
fn the_selection_scene_shows_the_selection_blend() {
    let (_dir, img) = scene_png("term-selection", "1");
    let blend = count_colour(&img, [78, 78, 78]);
    assert!(blend > 200, "only {blend} selection-blend pixels");
}

#[test]
fn an_unknown_scene_names_the_m5_scenes() {
    let dir = TempDir::new("scene-unknown-m5");
    let png = dir.png();
    let out = run(
        &[
            "--scene",
            "no-such-scene",
            "--screenshot",
            png.to_str().expect("utf-8 path"),
            "--renderer",
            "software",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1), "exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    for name in M5_SCENES {
        assert!(stderr.contains(name), "stderr names {name}: {stderr:?}");
    }
    assert!(!png.exists(), "no screenshot for an unknown scene");
}

#[test]
fn scenes_differ_from_each_other() {
    let (_a, strip) = scene_png("strip-3", "1");
    let (_b, sidebar) = scene_png("sidebar-open", "1");
    let mut differ = 0;
    for y in 0..strip.1 {
        for x in 0..strip.0 {
            if pixel(&strip, x, y) != pixel(&sidebar, x, y) {
                differ += 1;
            }
        }
    }
    assert!(differ > 5000, "only {differ} pixels differ");
}

#[test]
fn the_strip_scene_has_crisp_dividers_at_every_scale() {
    // The strip's divider colour (Theme.divider in the default theme).
    const DIVIDER: [u8; 3] = [39, 39, 39];
    for scale in ["1", "1.5", "2.25"] {
        let s: f32 = scale.parse().expect("scale");
        let hair = s.floor().max(1.0) as u32;
        let (_dir, img) = scene_png("strip-3", scale);
        // Below the tab labels, inside the strip; the mark's own line sits left of the strip.
        let y = (28.0 * s).round() as u32;
        let is_divider = |x: u32| {
            let p = pixel(&img, x, y);
            p.iter().zip(DIVIDER).all(|(a, b)| a.abs_diff(b) <= 2)
        };
        let mut runs = Vec::new();
        let mut x = (44.0 * s).round() as u32;
        while x < img.0 {
            if is_divider(x) {
                let start = x;
                while x < img.0 && is_divider(x) {
                    x += 1;
                }
                runs.push(x - start);
            } else {
                x += 1;
            }
        }
        assert!(
            runs.len() >= 2,
            "scale {scale}: only {} dividers",
            runs.len()
        );
        for run in runs {
            assert_eq!(
                run, hair,
                "scale {scale}: a divider is {run} px, want {hair}"
            );
        }
    }
}

/// The top-left of the first run of at least `min_run` pixels equal to `colour`, from row
/// `from_y` down: a panel's inner corner (a stray anti-aliased pixel never makes such a run).
fn panel_corner(img: &Decoded, colour: [u8; 3], from_y: u32, min_run: u32) -> Option<(u32, u32)> {
    for y in from_y..img.1 {
        let mut x = 0;
        while x < img.0 {
            if pixel(img, x, y) == colour {
                let start = x;
                while x < img.0 && pixel(img, x, y) == colour {
                    x += 1;
                }
                if x - start >= min_run {
                    return Some((start, y));
                }
            } else {
                x += 1;
            }
        }
    }
    None
}

/// Task 27: every row of the terminal menu wears its glyph (PT `MenuIcon` on every row); the
/// screenshots showed only `Copy path` with one.
#[test]
fn every_terminal_menu_row_has_its_glyph() {
    const PANEL: [u8; 3] = [0x1f, 0x1f, 0x1e];
    for (name, rows) in [
        ("term-menu-path", 5u32),
        ("term-menu-link", 4),
        ("term-menu-plain", 2),
    ] {
        for scale in ["1", "1.5", "2.25"] {
            let s: f32 = scale.parse().expect("scale");
            let hair = s.floor().max(1.0) as u32;
            let (_dir, img) = scene_png(name, scale);
            // Below the tab strip, which wears the same colour.
            let below_strip = (40.0 * s).round() as u32;
            let (x0, y0) = panel_corner(&img, PANEL, below_strip, (100.0 * s) as u32)
                .expect("the menu is shown");
            let row_h = (28.0 * s).round() as u32;
            // The icon column: 11 css px in from the inner edge, 13 css px wide, then the 8 px
            // gap before the label. A row without a glyph puts its label in both.
            let at = |css: f32| x0 + (css * s).round() as u32;
            let ink_in = |ry: u32, xa: u32, xb: u32| {
                let mut n = 0;
                for y in ry + 2..ry + row_h - 2 {
                    for x in xa..xb {
                        let p = pixel(&img, x, y);
                        if p.iter().zip(PANEL).any(|(a, b)| a.abs_diff(b) > 10) {
                            n += 1;
                        }
                    }
                }
                n
            };
            for i in 0..rows {
                let ry = y0 + i * (row_h + hair);
                let glyph = ink_in(ry, at(11.0), at(26.0));
                let gap = ink_in(ry, at(26.0), at(30.0));
                assert!(
                    glyph >= 4 && gap == 0,
                    "{name} at {scale}: row {i} has no glyph (glyph {glyph} px, gap {gap} px)"
                );
            }
        }
    }
}
