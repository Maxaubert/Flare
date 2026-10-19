//! Scan tests that hold the privacy rules (Task 16).
//!
//! No code here: the rules are held by reading the workspace's own source. Flare opens the microphone
//! and nothing else (no camera, no render endpoint for recording), and a clip never reaches the disk.
//! The patterns are assembled with `concat!` so this file never matches its own scan.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn workspace() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
    }

    fn files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if matches!(
                    name.as_str(),
                    "target" | "targets" | ".git" | ".claude" | "node_modules" | "vendor"
                ) {
                    continue;
                }
                files(&p, exts, out);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| exts.contains(&x))
            {
                out.push(p);
            }
        }
    }

    /// The source without its comment lines and without its test module (a test may write a fixture).
    fn code_of(path: &Path) -> String {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let head = text.split("#[cfg(test)]").next().unwrap_or("");
        head.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn this_file() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("privacy.rs")
    }

    #[test]
    fn only_capture_endpoints_are_opened() {
        let mut all = Vec::new();
        for d in ["crates", "tools"] {
            files(&workspace().join(d), &["rs", "toml", "slint"], &mut all);
        }
        files(&workspace(), &["toml"], &mut Vec::new());
        assert!(!all.is_empty(), "the scan found no files");
        let banned = [
            concat!("MFCreate", "Video"),
            concat!("MFCreate", "DeviceSource"),
            concat!("Windows.Media", ".Capture"),
            concat!("Media", "_Capture"),
            concat!("Media", "Capture"),
            concat!("IAudio", "RenderClient"),
            concat!("STREAMFLAGS", "_LOOPBACK"),
            concat!("KSCATEGORY", "_VIDEO_CAMERA"),
            concat!("KSCATEGORY", "_SENSOR_CAMERA"),
            concat!("e5323777", "-f976-4f5b-9b55-b94699c46e44"),
            concat!("capNew", "VideoCapture"),
            concat!("nokhwa", ""),
        ];
        let me = this_file().canonicalize().unwrap();
        for f in &all {
            if f.canonicalize().ok().as_ref() == Some(&me) {
                continue;
            }
            let text = std::fs::read_to_string(f)
                .unwrap_or_default()
                .to_lowercase();
            for b in banned {
                assert!(
                    !text.contains(&b.to_lowercase()),
                    "{} mentions {b}",
                    f.display()
                );
            }
        }
        // The one render-endpoint mention in mic.rs reads the default speaker's id as the echo reference.
        let mic = code_of(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join("mic.rs"),
        );
        let render = concat!("e", "Render");
        for line in mic.lines().filter(|l| l.contains(render)) {
            let ok = line.trim_start().starts_with("use ")
                || line.contains("GetDefaultAudioEndpoint")
                || line.trim_start().starts_with("eCapture")
                || line.trim_end().ends_with(',');
            assert!(
                ok,
                "mic.rs uses a render endpoint for something else: {line}"
            );
        }
        assert!(
            mic.lines()
                .filter(|l| l.contains(render) && l.contains("GetDefaultAudioEndpoint"))
                .count()
                == 1,
            "exactly one place reads the render endpoint's id"
        );
        assert!(mic.contains("eCapture"), "mic.rs opens capture endpoints");
        // Nothing but the id of a render endpoint is ever taken from it: no Activate on a render device.
        let render_devices = mic
            .matches(concat!("GetDefaultAudioEndpoint(", "eRender"))
            .count();
        assert_eq!(render_devices, 1);
    }

    #[test]
    fn no_audio_is_written_to_disk() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut scanned = vec![src.join("mic.rs"), src.join("controller.rs")];
        files(&src.join("engine"), &["rs"], &mut scanned);
        assert!(scanned.len() >= 3, "the engine files were not found");
        let banned = [
            concat!("fs::", "write"),
            concat!("File::", "create"),
            concat!("File::", "options"),
            concat!("Open", "Options"),
            concat!("fs::", "copy"),
            concat!("create_", "new"),
            concat!("temp", "file"),
            concat!("fs::", "rename"),
            concat!("write_", "all"),
        ];
        for f in &scanned {
            let code = code_of(f);
            for b in banned {
                // A child's stdin/stdout is not a file; `write_all` there is how audio reaches an engine.
                if b == concat!("write_", "all") {
                    continue;
                }
                assert!(!code.contains(b), "{} writes to disk ({b})", f.display());
            }
        }
    }
}
