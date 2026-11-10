//! The theme-sample scene through the built binary (Task 28). The per-pixel checks of every theme at
//! every scale run in process (`theme_scene::tests`, which can reach the window); this file holds what
//! only the executable shows: the exit codes, the picture it writes and the sample file's bytes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flare"));
    cmd.args(args);
    cmd.env_remove("FLARE_FORCE_GPU_FAIL");
    cmd.env_remove("FLARE_RENDERER");
    cmd.output().expect("run flare")
}

/// A scratch folder under the system temp dir, removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("flare-pixels-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn decode(path: &Path) -> (u32, u32, Vec<u8>) {
    let file = std::fs::File::open(path).expect("the screenshot exists");
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .expect("png header");
    let mut buf = vec![0; reader.output_buffer_size().expect("png size")];
    let info = reader.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn has_pixel(img: &(u32, u32, Vec<u8>), rgb: [u8; 3]) -> bool {
    img.2
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[..3] == rgb && p[3] == 255)
}

fn sample_file() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/theme-sample.txt");
    std::fs::read(path).expect("docs/theme-sample.txt exists")
}

#[test]
fn an_unknown_scene_or_theme_is_an_error() {
    let dir = TempDir::new("unknown");
    let png = dir.0.join("shot.png");
    let png = png.to_str().expect("utf-8 path");

    let out = run(&[
        "--scene",
        "theme-sample",
        "--theme",
        "nope",
        "--renderer",
        "software",
        "--screenshot",
        png,
    ]);
    assert_eq!(out.status.code(), Some(1), "an unknown theme exits 1");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("nope"), "{err}");
    for id in ["pt-default", "paper", "volt", "nord"] {
        assert!(err.contains(id), "the error names the valid id {id}: {err}");
    }
    assert!(!Path::new(png).exists(), "no picture is written");

    let out = run(&["--scene", "no-such-scene", "--screenshot", png]);
    assert_eq!(out.status.code(), Some(1), "an unknown scene exits 1");
    assert!(String::from_utf8_lossy(&out.stderr).contains("theme-sample"));
}

#[cfg_attr(
    not(windows),
    ignore = "Windows fonts and text metrics; macOS phase 2 of the port plan (fonts)"
)]
#[test]
fn the_theme_sample_scene_writes_a_picture_in_the_themes_colours() {
    let dir = TempDir::new("scene");
    for (theme, red, ground) in [
        ("pt-default", [0xff, 0x61, 0x5a], [0x17, 0x17, 0x17]),
        ("paper", [0, 0, 0], [0xf6, 0xf4, 0xee]),
    ] {
        let png = dir.0.join(format!("{theme}.png"));
        let out = run(&[
            "--scene",
            "theme-sample",
            "--theme",
            theme,
            "--renderer",
            "software",
            "--size",
            "1000x640",
            "--screenshot",
            png.to_str().expect("utf-8 path"),
        ]);
        assert!(
            out.status.success(),
            "{theme}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let img = decode(&png);
        assert_eq!((img.0, img.1), (1000, 640));
        assert!(
            has_pixel(&img, ground),
            "{theme}: the ground {ground:02x?} is drawn"
        );
        if red != [0, 0, 0] {
            assert!(
                has_pixel(&img, red),
                "{theme}: the red block {red:02x?} is drawn"
            );
        }
    }
}

#[test]
fn the_sample_file_is_the_stream_the_capture_tool_types() {
    let bytes = sample_file();
    let text = String::from_utf8(bytes.clone()).expect("the sample is utf-8");
    assert!(!text.contains('\u{2014}'), "no em-dash");
    assert!(text.starts_with("PS C:\\Users\\Admin\\Documents> Get-ChildItem -Recurse\r\n"));
    assert!(
        text.ends_with("Documents> "),
        "the cursor stays after the prompt"
    );
    assert_eq!(bytes.iter().filter(|b| **b == b'\x1b').count(), 84);
    assert!(
        !bytes.windows(2).any(|w| w[1] == b'\n' && w[0] != b'\r'),
        "every line end is CRLF"
    );
    assert_eq!(text.matches("\r\n").count(), 9);
    assert!(text.contains("Flare theme sample"));
}

/// Prism Terminal's TitleMark is `pl-3.5 pr-3` around a 17px mark (a 43px box) with its line drawn
/// INSIDE the box (`after:absolute after:right-0 after:w-px`), so the line's first device pixel is
/// where the 42nd css pixel ends. Measured on PT 0.58.0's own captures (Task 31): 42 at 100, 63 at
/// 150 and 95 at 225. Flare drew the line one hairline later (43, 65, 97).
#[cfg_attr(
    not(windows),
    ignore = "Windows fonts and text metrics; macOS phase 2 of the port plan (fonts)"
)]
#[test]
fn the_mark_line_sits_where_prism_terminal_draws_it() {
    let dir = TempDir::new("mark-line");
    for (scale, want) in [("1.0", 42usize), ("1.5", 63), ("2.25", 95)] {
        let png = dir.0.join(format!("mark-{scale}.png"));
        let out = run(&[
            "--scene",
            "theme-sample",
            "--theme",
            "pt-default",
            "--renderer",
            "software",
            "--scale",
            scale,
            "--size",
            "1000x640",
            "--screenshot",
            png.to_str().expect("utf-8 path"),
        ]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (w, _, px) = decode(&png);
        let s: f32 = scale.parse().expect("scale");
        let y = (6.0 * s) as usize;
        let at = |x: usize| {
            let i = (y * w as usize + x) * 4;
            [px[i], px[i + 1], px[i + 2]]
        };
        let ground = at((600.0 * s) as usize);
        let first = ((35.0 * s) as usize..w as usize)
            .find(|&x| at(x) != ground)
            .expect("a line after the mark");
        assert_eq!(
            (first, at(first)),
            (want, [0x27, 0x27, 0x27]),
            "the mark line at scale {scale}"
        );
    }
}
