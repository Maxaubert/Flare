use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const BUNDLED: [&str; 2] = ["conpty.dll", "OpenConsole.exe"];

fn main() {
    slint_build::compile("ui/app.slint").expect("Slint UI failed to compile");
    copy_bundled_conpty();
}

/// Copies the bundled ConPTY files next to the built exe (the profile folder), so a debug or
/// release run finds them. The installer (M9) ships them beside the exe. Best effort: a failed
/// copy is a build warning, and the start-up check reports a missing file at run time.
fn copy_bundled_conpty() {
    let src_dir = Path::new("../flare-term/bundled/x64");
    for name in BUNDLED {
        println!("cargo:rerun-if-changed={}", src_dir.join(name).display());
    }
    let Some(profile) = profile_dir() else {
        println!("cargo:warning=cannot find the profile folder; the bundled ConPTY is not copied");
        return;
    };
    for name in BUNDLED {
        let from = src_dir.join(name);
        if let Err(e) = fs::copy(&from, profile.join(name)) {
            println!(
                "cargo:warning=cannot copy {} next to the exe: {e}",
                from.display()
            );
        }
    }
}

/// OUT_DIR is `<profile>/build/<pkg>-<hash>/out`, so the profile folder is its third ancestor.
fn profile_dir() -> Option<PathBuf> {
    let out = env::var("OUT_DIR").ok()?;
    Path::new(&out).ancestors().nth(3).map(Path::to_path_buf)
}
