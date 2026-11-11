//! `Flare.app` for a release (port phase 8): the dev bundle's layout ([`crate::macos::bundle`]) around the real app,
//! with its icon, its other programs, its resources and the keys a shipped Mac app needs, signed with the Developer
//! ID (`tools/release/mac/make-app.sh` drives it: build, bundle, verify, notarize, staple, DMG). Built under the
//! target dir only, never the repo.
//!
//! ```text
//! Flare.app/Contents/
//!   Info.plist                                  com.flare.terminal, the workspace version, macOS 15+
//!   MacOS/Flare                                 the app (`target/release/flare`, renamed: CEF finds its helpers
//!                                               as "<executable> Helper (X).app")
//!   MacOS/flare-mcp                             the MCP server, found beside the app (`flare_agent::plugin`)
//!   MacOS/flare-parakeet-host                   the resident Parakeet host (dictation; loads the engine's libraries)
//!   Frameworks/Chromium Embedded Framework.framework
//!   Frameworks/whisper                          the speech engine, code only (rule 1138): whisper-server,
//!                                               parakeet-cli and their libraries (`tools/build-whisper-macos.sh`)
//!   Frameworks/Flare Helper*.app                five helpers running `flare-cef-helper`
//!   Resources/Flare.icns                        from build/icon-source.png (tools/make-icon.py's 1024 px render)
//!   Resources/claude-plugin                     the Claude Code plugin template (found there by flare_agent)
//!   Resources/LICENSE, THIRD-PARTY-NOTICES.md, CEF-CREDITS.html, LICENSE-whisper.cpp.txt
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::macos::bundle::{self, BundleSpec, Extras};
use crate::macos::sign::Signing;

/// The app's name: the bundle, its executable, its helpers' prefix and the DMG's volume.
pub const APP_NAME: &str = "Flare";

/// The bundle id: the id Windows already gives the app (its AppUserModelID, `flare_win::names::AUMID`), so one name
/// stands for Flare everywhere. It is permanent: the keychain items, the microphone grant and the notification
/// settings are keyed on it.
pub const BUNDLE_ID: &str = "com.flare.terminal";

/// The oldest macOS Flare runs on (the port's target, spec decision "Targets").
pub const MIN_MACOS: &str = "15.0";

/// The sentence macOS shows when Flare, or a program run in its terminal, first asks for the microphone.
pub const MIC_USAGE: &str = "Flare uses the microphone for dictation, and for the programs you run in its terminal and \
     the web pages you open in it that ask for it.";

/// The sentence macOS shows when a program run in the terminal, or a web page in the panel, asks for the camera.
pub const CAMERA_USAGE: &str = "A program you run in Flare's terminal, or a web page you open in Flare, wants to use the camera.";

/// The sentence macOS shows when a program run in the terminal (osascript) sends Apple Events to another app.
pub const APPLE_EVENTS_USAGE: &str =
    "A program you run in Flare's terminal (osascript, for example) wants to control another app.";

/// The keys `Flare.app`'s `Info.plist` adds to the bundle's own (which hold the id, the version and
/// `LSMinimumSystemVersion`). No document or URL types: Flare opens no file kind and no scheme of its own (a folder
/// reaches it through the Finder service of phase 6).
pub fn app_keys(year: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "CFBundleDisplayName",
            format!("<string>{APP_NAME}</string>"),
        ),
        (
            "LSApplicationCategoryType",
            "<string>public.app-category.developer-tools</string>".into(),
        ),
        (
            "NSMicrophoneUsageDescription",
            format!("<string>{MIC_USAGE}</string>"),
        ),
        (
            "NSCameraUsageDescription",
            format!("<string>{CAMERA_USAGE}</string>"),
        ),
        (
            "NSAppleEventsUsageDescription",
            format!("<string>{APPLE_EVENTS_USAGE}</string>"),
        ),
        (
            "NSHumanReadableCopyright",
            format!("<string>Copyright {year} Max Aubert</string>"),
        ),
    ]
}

/// The `.iconset` names `iconutil` wants and each one's pixel size.
pub const ICONSET: [(&str, u32); 10] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
];

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Makes `<out_dir>/Flare.icns` from a 1024 px PNG with `sips` and `iconutil` (both part of macOS); its path.
pub fn make_icns(source_png: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let set = out_dir.join(format!("{APP_NAME}.iconset"));
    let _ = fs::remove_dir_all(&set);
    fs::create_dir_all(&set).map_err(|e| format!("{}: {e}", set.display()))?;
    for (name, px) in ICONSET {
        run(Command::new("/usr/bin/sips")
            .args([
                "-s",
                "format",
                "png",
                "-z",
                &px.to_string(),
                &px.to_string(),
            ])
            .arg(source_png)
            .arg("--out")
            .arg(set.join(name)))?;
    }
    let icns = out_dir.join(format!("{APP_NAME}.icns"));
    run(Command::new("/usr/bin/iconutil")
        .args(["-c", "icns"])
        .arg(&set)
        .arg("-o")
        .arg(&icns))?;
    let _ = fs::remove_dir_all(&set);
    Ok(icns)
}

/// Where the parts of a release come from.
pub struct Inputs<'a> {
    /// The repository root (resources, the icon source).
    pub root: &'a Path,
    /// The folder with the release programs: `flare`, `flare-mcp`, `flare-cef-helper`.
    pub bin_dir: &'a Path,
    /// The CEF folder (the framework, `CREDITS.html`).
    pub cef_dir: &'a Path,
    /// The speech engine `tools/build-whisper-macos.sh` built (`vendor/macos/whisper`): its programs, libraries,
    /// MIT notice and `.source` stamp.
    pub engine_dir: &'a Path,
    /// Where `Flare.app` (and its icon and entitlements) are written.
    pub out_dir: &'a Path,
    pub version: &'a str,
    pub year: &'a str,
    pub signing: Signing,
}

fn need(p: PathBuf) -> Result<PathBuf, String> {
    if p.exists() {
        Ok(p)
    } else {
        Err(format!("missing: {}", p.display()))
    }
}

/// The engine's programs, each of which must be in the folder: the names `flare_dictation::engine` starts.
pub const ENGINE_PROGRAMS: [&str; 2] = ["whisper-server", "parakeet-cli"];

/// The engine folder's name in `Contents/Frameworks` (`flare_win::names::ENGINE_FOLDER`).
pub const ENGINE_FOLDER: &str = "whisper";

/// The engine's MIT notice, in the engine folder and in `Contents/Resources`.
pub const ENGINE_LICENSE: &str = "LICENSE-whisper.cpp.txt";

/// The SHA-256 of a file, by `shasum` (part of macOS).
fn sha256(p: &Path) -> Result<String, String> {
    let out = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(p)
        .output()
        .map_err(|e| format!("shasum {}: {e}", p.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    match text.split_whitespace().next() {
        Some(h) if out.status.success() && h.len() == 64 => Ok(h.to_string()),
        _ => Err(format!("shasum {} failed", p.display())),
    }
}

/// The engine's code, copied out of `engine` into `<out>/whisper` (replaced): every Mach-O the `.source` stamp lists,
/// each hashing to what it says, and nothing else. `Contents/Frameworks` holds only code, so the notice and the stamp
/// stay out of it (a data file there breaks `codesign --strict`). Fails when the stamp is missing, a listed file is
/// missing or changed, a file of the folder is not listed, or a program is missing.
pub fn engine_code(engine: &Path, out: &Path) -> Result<PathBuf, String> {
    let stamp = engine.join(".source");
    let text = fs::read_to_string(&stamp).map_err(|e| {
        format!(
            "{}: {e} (build the engine: bash tools/build-whisper-macos.sh vendor/macos/whisper)",
            stamp.display()
        )
    })?;
    let mut listed = Vec::new();
    for line in text.lines().skip(1) {
        let (hash, name) = line
            .split_once("  ")
            .ok_or_else(|| format!("{}: not a stamp line: {line:?}", stamp.display()))?;
        if name.contains('/') || name.starts_with('.') {
            return Err(format!(
                "{}: a file outside the folder: {name:?}",
                stamp.display()
            ));
        }
        listed.push((hash.to_string(), name.to_string()));
    }
    let present: Vec<String> = fs::read_dir(engine)
        .map_err(|e| format!("{}: {e}", engine.display()))?
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n != ".source")
        .collect();
    for n in &present {
        if !listed.iter().any(|(_, l)| l == n) {
            return Err(format!("{}: {n} is not in the stamp", engine.display()));
        }
    }
    for p in ENGINE_PROGRAMS {
        if !listed.iter().any(|(_, l)| l == p) {
            return Err(format!("{}: no {p}", engine.display()));
        }
    }
    let to = out.join(ENGINE_FOLDER);
    if to.exists() {
        fs::remove_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
    for (hash, name) in &listed {
        let from = engine.join(name);
        if sha256(&from)? != *hash {
            return Err(format!("{}: not the file the stamp lists", from.display()));
        }
        if crate::macos::sign::is_macho(&from) {
            fs::copy(&from, to.join(name))
                .map_err(|e| format!("{} to {}: {e}", from.display(), to.display()))?;
        }
    }
    Ok(to)
}

/// Builds and signs `Flare.app`; its path. Fails when any part the app ships is missing.
pub fn build(i: &Inputs<'_>) -> Result<PathBuf, String> {
    let main_exe = need(i.bin_dir.join("flare"))?;
    let helper = need(i.bin_dir.join("flare-cef-helper"))?;
    let mcp = need(i.bin_dir.join("flare-mcp"))?;
    let host = need(i.bin_dir.join("flare-parakeet-host"))?;
    let engine_license = need(i.engine_dir.join(ENGINE_LICENSE))?;
    let plugin = need(i.root.join("crates/flare-agent/claude-plugin"))?;
    need(plugin.join("hooks/hooks.json"))?;
    let license = need(i.root.join("LICENSE"))?;
    let notices = need(i.root.join("THIRD-PARTY-NOTICES.md"))?;
    let credits = need(i.cef_dir.join("CREDITS.html"))?;
    let icon_src = need(i.root.join("build/icon-source.png"))?;
    fs::create_dir_all(i.out_dir).map_err(|e| format!("{}: {e}", i.out_dir.display()))?;
    let icns = make_icns(&icon_src, i.out_dir)?;
    let engine = engine_code(i.engine_dir, &i.out_dir.join("engine"))?;
    let spec = BundleSpec {
        name: APP_NAME,
        identifier: BUNDLE_ID,
        main_exe: &main_exe,
        helper_exe: &helper,
        cef_dir: i.cef_dir,
        out_dir: i.out_dir,
        background_only: false,
        version: i.version,
        signing: i.signing.clone(),
        extras: Extras {
            icon: Some(&icns),
            executables: vec![&mcp, &host],
            resources: vec![
                (&plugin, "claude-plugin"),
                (&license, "LICENSE"),
                (&notices, "THIRD-PARTY-NOTICES.md"),
                (&credits, "CEF-CREDITS.html"),
                (&engine_license, ENGINE_LICENSE),
            ],
            frameworks: vec![(&engine, ENGINE_FOLDER)],
            keys: app_keys(i.year),
        },
    };
    bundle::build(&spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_plist_names_the_app_its_version_its_mac_its_category_and_why_it_wants_the_mic() {
        let keys = app_keys("2026");
        let mut with_icon = keys.clone();
        with_icon.push(("CFBundleIconFile", "<string>Flare</string>".into()));
        let p = bundle::info_plist(APP_NAME, BUNDLE_ID, "1.2.3", false, false, &with_icon);
        for want in [
            "<key>CFBundleIdentifier</key>\n  <string>com.flare.terminal</string>",
            "<key>CFBundleExecutable</key>\n  <string>Flare</string>",
            "<key>CFBundleShortVersionString</key>\n  <string>1.2.3</string>",
            "<key>CFBundleVersion</key>\n  <string>1.2.3</string>",
            "<key>LSMinimumSystemVersion</key>\n  <string>15.0</string>",
            "<key>LSApplicationCategoryType</key>\n  <string>public.app-category.developer-tools</string>",
            "<key>NSMicrophoneUsageDescription</key>\n  <string>Flare uses the microphone",
            "<key>CFBundleIconFile</key>\n  <string>Flare</string>",
            "<key>NSHighResolutionCapable</key>\n  <true/>",
        ] {
            assert!(p.contains(want), "{want}\n{p}");
        }
        // A window app: no background-only or agent key, and no file or URL types it does not handle.
        for absent in [
            "LSBackgroundOnly",
            "LSUIElement",
            "CFBundleDocumentTypes",
            "CFBundleURLTypes",
        ] {
            assert!(!p.contains(absent), "{absent}");
        }
        // Every key that lets macOS ask (the app's TCC entitlements) has its sentence.
        for e in crate::macos::sign::entitlements(crate::macos::sign::Role::App) {
            let u = e.usage_key.expect("a TCC key");
            assert!(p.contains(&format!("<key>{u}</key>\n  <string>")), "{u}");
        }
        // Each key once, and the plist is what plutil accepts.
        assert_eq!(p.matches("<key>CFBundleIdentifier</key>").count(), 1);
        let dir = std::env::temp_dir().join(format!("flare-release-plist-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("dir");
        let f = dir.join("Info.plist");
        fs::write(&f, &p).expect("write");
        let ok = Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&f)
            .output()
            .expect("plutil runs");
        let _ = fs::remove_dir_all(&dir);
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stdout)
        );
    }

    /// A stamped engine folder as tools/build-whisper-macos.sh writes it: two programs and a library (copies of a
    /// system Mach-O), the notice, and `.source` listing each file's SHA-256.
    fn stamped_engine(dir: &Path) {
        fs::create_dir_all(dir).expect("dir");
        for n in ["whisper-server", "parakeet-cli", "libggml.0.dylib"] {
            fs::copy("/usr/bin/true", dir.join(n)).expect("a Mach-O");
        }
        fs::write(dir.join(ENGINE_LICENSE), "MIT License").expect("notice");
        let mut stamp = "927cfce34f31707e17f2bff35c349632fb9e2c3a\n".to_string();
        let mut names = vec![
            ENGINE_LICENSE,
            "libggml.0.dylib",
            "parakeet-cli",
            "whisper-server",
        ];
        names.sort_unstable();
        for n in names {
            stamp.push_str(&format!("{}  {n}\n", sha256(&dir.join(n)).expect("hash")));
        }
        fs::write(dir.join(".source"), stamp).expect("stamp");
    }

    /// Rule 1138: the engine goes into `Contents/Frameworks/whisper` as code only, and only the stamped build does.
    #[test]
    fn the_engine_is_copied_as_code_only_and_only_when_every_file_matches_its_stamp() {
        let tmp = std::env::temp_dir().join(format!("flare-engine-code-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let engine = tmp.join("whisper");
        stamped_engine(&engine);
        let out = engine_code(&engine, &tmp.join("out")).expect("copied");
        let mut got: Vec<String> = fs::read_dir(&out)
            .expect("out")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        got.sort();
        assert_eq!(got, ["libggml.0.dylib", "parakeet-cli", "whisper-server"]);
        assert_eq!(
            out.file_name().and_then(|n| n.to_str()),
            Some(ENGINE_FOLDER)
        );
        // A file the stamp does not list (a library planted beside an @loader_path rpath) is refused.
        fs::copy("/usr/bin/true", engine.join("libplanted.dylib")).expect("planted");
        let e = engine_code(&engine, &tmp.join("out")).expect_err("planted");
        assert!(e.contains("libplanted.dylib"), "{e}");
        fs::remove_file(engine.join("libplanted.dylib")).expect("rm");
        // A changed program is refused.
        fs::write(engine.join("whisper-server"), b"changed").expect("tamper");
        let e = engine_code(&engine, &tmp.join("out")).expect_err("changed");
        assert!(e.contains("whisper-server"), "{e}");
        // No stamp: the engine was never built; the error says how to build it.
        fs::remove_file(engine.join(".source")).expect("rm stamp");
        let e = engine_code(&engine, &tmp.join("out")).expect_err("no stamp");
        assert!(e.contains("build-whisper-macos.sh"), "{e}");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_engine_names_are_the_ones_the_app_starts() {
        let names = include_str!("../../../flare-win/src/names.rs");
        assert!(names.contains(&format!(
            "pub const ENGINE_FOLDER: &str = \"{ENGINE_FOLDER}\";"
        )));
        let engine = include_str!("../../../flare-dictation/src/engine/mod.rs");
        for p in ENGINE_PROGRAMS {
            assert!(engine.contains(&format!("\"{p}\"")), "{p}");
        }
        assert!(engine.contains(".join(\"Frameworks\")"));
    }

    #[test]
    fn the_bundle_id_is_the_id_windows_gives_flare() {
        assert_eq!(BUNDLE_ID, "com.flare.terminal");
        let names = include_str!("../../../flare-win/src/names.rs");
        assert!(names.contains(&format!("pub const AUMID: &str = \"{BUNDLE_ID}\";")));
        assert_eq!(MIN_MACOS, "15.0");
    }

    #[test]
    fn the_iconset_has_every_size_iconutil_asks_for() {
        assert_eq!(ICONSET.len(), 10);
        for (name, px) in ICONSET {
            let base: u32 = name
                .trim_start_matches("icon_")
                .split('x')
                .next()
                .and_then(|n| n.parse().ok())
                .expect("a size");
            let want = if name.contains("@2x") { base * 2 } else { base };
            assert_eq!(px, want, "{name}");
        }
    }
}
