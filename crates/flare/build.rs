use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const BUNDLED: [&str; 2] = ["conpty.dll", "OpenConsole.exe"];

fn main() {
    slint_build::compile("ui/app.slint").expect("Slint UI failed to compile");
    // The exe's icon and version resource (M9). A missing RC.EXE fails the build here, loudly.
    embed_resource::compile("flare.rc", embed_resource::NONE)
        .manifest_required()
        .expect("embed the exe resource (flare.rc)");
    copy_bundled_conpty();
    copy_claude_plugin();
}

/// Copies the bundled Claude Code plugin (`flare-agent/claude-plugin`) to `<profile>/claude-plugin`,
/// the folder a shell is handed as CLAUDE_CODE_PLUGIN_DIRS. Best effort, like the ConPTY copy: a
/// failure is a build warning. The source folder does not exist until M6 Task 3; then nothing is done.
fn copy_claude_plugin() {
    let src_root = Path::new("../flare-agent/claude-plugin");
    if !src_root.is_dir() {
        return;
    }
    let mut files = Vec::new();
    collect_files(src_root, &mut files);
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    let Some(profile) = profile_dir() else {
        println!("cargo:warning=cannot find the profile folder; the Claude plugin is not copied");
        return;
    };
    for file in &files {
        let Ok(rel) = file.strip_prefix(src_root) else {
            continue;
        };
        let to = profile.join("claude-plugin").join(rel);
        let result = to
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::copy(file, &to).map(|_| ()));
        if let Err(e) = result {
            println!(
                "cargo:warning=cannot copy {} next to the exe: {e}",
                file.display()
            );
        }
    }
}

/// Every file under `dir`, folders first walked depth-first.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
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
