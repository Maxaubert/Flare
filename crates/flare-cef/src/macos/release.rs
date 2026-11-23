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
//!   MacOS/flare-sim-host                        the device panel's sim host (issue #248, rule 1310): the only process
//!                                               that loads CoreSimulator and SimulatorKit; a tool like the two above
//!                                               (hardened runtime, no entitlements)
//!   MacOS/flare-emu-host                        the Android device panel's emu host (Android plan PR A2): the only
//!                                               process that talks to the emulator and adb; a tool like the others
//!   Frameworks/Chromium Embedded Framework.framework
//!   Frameworks/whisper                          the speech engine, code only (rule 1138): whisper-server,
//!                                               parakeet-cli and their libraries (`tools/build-whisper-macos.sh`)
//!   Frameworks/Flare Helper*.app                five helpers running `flare-cef-helper`
//!   PlugIns/FlareDockTile.docktileplugin        the Dock tile plugin: a pinned Flare keeps the picked icon while
//!                                               closed (rule 1276; `FlareDockTile.m`, built by `build_dock_tile`)
//!   Resources/Flare.icns                        the original Flare icon, from build/icon-source.png (tools/make-icon.py's
//!                                               1024 px render of assets/icon/flare-color.svg; rule 1230)
//!   Resources/claude-plugin                     the Claude Code plugin template (found there by flare_agent)
//!   Resources/flare-agent.jar                   the Android agent helper (rule 1369), checked in under
//!                                               crates/flare-device/assets; the emu host finds it in ../Resources
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

/// The device panel's sim host, `flare_win::names::SIM_HOST_EXE` (the app starts it from beside its executable).
pub const SIM_HOST_EXE: &str = "flare-sim-host";

/// The Android device panel's emu host, `flare_win::names::EMU_HOST_EXE` (Android plan PR A6; the app starts it from
/// beside its executable from PR A4 on).
pub const EMU_HOST_EXE: &str = "flare-emu-host";

/// The Android agent helper (rule 1369), `flare_win::names::AGENT_JAR`: a data file in `Contents/Resources` that the emu
/// host pushes into an emulator.
pub const AGENT_JAR: &str = "flare-agent.jar";

/// The agent helper's checked-in build, from the repository root (`tools/android-agent/build.py` writes it).
pub const AGENT_JAR_SOURCE: &str = "crates/flare-device/assets/flare-agent.jar";

/// The other programs in `Contents/MacOS`, from the release bin dir: each signed as a tool (`Role::Tool`, identifier
/// `com.flare.terminal.<name>`, hardened runtime, no entitlements). `UpdateMac` joins them when vpk packs the app.
pub const TOOLS: [&str; 4] = [
    "flare-mcp",
    "flare-parakeet-host",
    SIM_HOST_EXE,
    EMU_HOST_EXE,
];

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

/// The sentence macOS shows the first time Flare looks for your other computers on the network (machine pairing).
pub const LOCAL_NETWORK_USAGE: &str =
    "Flare finds and links your other computers on this network, so you can open tabs on them.";

/// The Bonjour service Flare announces and looks for while Computer sharing is on (machine pairing).
pub const PAIRING_SERVICE: &str = "_flare-pair._tcp";

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
        // Machine pairing (rule 1254): macOS asks before Flare looks for and announces computers on the home network,
        // and only for the Bonjour service it names here.
        (
            "NSLocalNetworkUsageDescription",
            format!("<string>{LOCAL_NETWORK_USAGE}</string>"),
        ),
        (
            "NSBonjourServices",
            format!("<array><string>{PAIRING_SERVICE}</string></array>"),
        ),
        (
            "NSHumanReadableCopyright",
            format!("<string>Copyright {year} Max Aubert</string>"),
        ),
        // Rule 1276: the Dock loads this plugin for a pinned Flare, also while Flare is closed.
        (
            "NSDockTilePlugIn",
            format!("<string>{DOCK_TILE}.docktileplugin</string>"),
        ),
    ]
}

/// The Dock tile plugin's name (rule 1276): its bundle `<name>.docktileplugin` in `Contents/PlugIns`, its executable
/// and the last part of its id.
pub const DOCK_TILE: &str = "FlareDockTile";

/// The plugin's principal class, the one `FlareDockTile.m` defines (unique, as the Dock's helper may hold several).
pub const DOCK_TILE_CLASS: &str = "FlareDockTilePlugIn_com_flare_terminal";

/// The plugin's source, from the repository root.
pub const DOCK_TILE_SOURCE: &str = "crates/flare-cef/src/macos/FlareDockTile.m";

/// The plugin bundle's `Info.plist` text.
pub fn dock_tile_plist(version: &str) -> String {
    bundle::plist_doc(&format!(
        "  <key>CFBundleDevelopmentRegion</key>\n  <string>en</string>\n\
           <key>CFBundleExecutable</key>\n  <string>{DOCK_TILE}</string>\n\
           <key>CFBundleIdentifier</key>\n  <string>{BUNDLE_ID}.{DOCK_TILE}</string>\n\
           <key>CFBundleInfoDictionaryVersion</key>\n  <string>6.0</string>\n\
           <key>CFBundleName</key>\n  <string>{DOCK_TILE}</string>\n\
           <key>CFBundlePackageType</key>\n  <string>BNDL</string>\n\
           <key>CFBundleShortVersionString</key>\n  <string>{version}</string>\n\
           <key>CFBundleVersion</key>\n  <string>{version}</string>\n\
           <key>LSMinimumSystemVersion</key>\n  <string>{MIN_MACOS}</string>\n\
           <key>NSPrincipalClass</key>\n  <string>{DOCK_TILE_CLASS}</string>\n"
    ))
}

/// Builds the Dock tile plugin bundle from `source` under `<out_dir>/dock-tile` with `xcrun clang` (arm64, macOS 15+,
/// headless); its path. Unsigned: the app's signing signs it.
pub fn build_dock_tile(source: &Path, out_dir: &Path, version: &str) -> Result<PathBuf, String> {
    let plugin = out_dir
        .join("dock-tile")
        .join(format!("{DOCK_TILE}.docktileplugin"));
    if plugin.exists() {
        fs::remove_dir_all(&plugin).map_err(|e| format!("{}: {e}", plugin.display()))?;
    }
    let macos = plugin.join("Contents/MacOS");
    fs::create_dir_all(&macos).map_err(|e| format!("{}: {e}", macos.display()))?;
    fs::write(plugin.join("Contents/Info.plist"), dock_tile_plist(version))
        .map_err(|e| format!("{}: {e}", plugin.display()))?;
    run(Command::new("/usr/bin/xcrun")
        .args(["clang", "-bundle", "-fobjc-arc", "-O2", "-arch", "arm64"])
        .arg(format!("-mmacosx-version-min={MIN_MACOS}"))
        .args(["-framework", "AppKit", "-o"])
        .arg(macos.join(DOCK_TILE))
        .arg(source))?;
    Ok(plugin)
}

/// The shipped Mac app icon's source, from the repository root: the original Flare icon, tools/make-icon.py's 1024 px
/// render of assets/icon/flare-color.svg, the same artwork as Windows' exe icon (rule 1230, 2026-11-13: the original
/// stays the default; the other designs are choices in Settings > Appearance, set at run time).
pub const APP_ICON_SOURCE: &str = "build/icon-source.png";

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

/// The owner's Icon Composer icon (Liquid Glass, PR #199), from the repository root. Not the shipped icon since rule
/// 1230: it is kept, with the `actool` path below, for an Icon Composer icon the bundle may carry again later
/// ([`compile_app_icon`], `Extras::icon_catalog`). Its flat render is the Liquid Glass choice of the app icon picker.
pub const GLASS_ICON_SOURCE: &str = "assets/icon/app-macos.icon";

/// The files `actool` writes for the app icon named [`APP_NAME`]: the asset catalog and the `.icns`.
pub fn app_icon_outputs(dir: &Path) -> (PathBuf, PathBuf) {
    (
        dir.join(bundle::ASSET_CATALOG),
        dir.join(format!("{APP_NAME}.icns")),
    )
}

/// The `actool` arguments that compile `<dir>/<APP_NAME>.icon` into `<dir>/compiled`: an `Assets.car` holding the
/// Liquid Glass icon for macOS 26 and later and a `<APP_NAME>.icns` for the older macOS Flare runs on.
pub fn actool_args(dir: &Path) -> Vec<std::ffi::OsString> {
    let mut a: Vec<std::ffi::OsString> = vec!["actool".into()];
    a.push(dir.join(format!("{APP_NAME}.icon")).into());
    a.push("--compile".into());
    a.push(dir.join("compiled").into());
    for s in [
        "--platform",
        "macosx",
        "--target-device",
        "mac",
        "--minimum-deployment-target",
        MIN_MACOS,
        "--app-icon",
        APP_NAME,
        "--output-partial-info-plist",
    ] {
        a.push(s.into());
    }
    a.push(dir.join("partial-info.plist").into());
    for s in [
        "--errors",
        "--warnings",
        "--output-format",
        "human-readable-text",
    ] {
        a.push(s.into());
    }
    a
}

/// Compiles the Icon Composer icon `source` (a `.icon` folder) with `xcrun actool` (Xcode 26 or later, headless)
/// under `<out_dir>/app-icon`; the asset catalog and the `.icns`. actool names the icon after the folder, so the
/// folder is copied in as `<APP_NAME>.icon` first.
pub fn compile_app_icon(source: &Path, out_dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    if !source.join("icon.json").exists() {
        return Err(format!(
            "{}: not an Icon Composer icon (no icon.json)",
            source.display()
        ));
    }
    let dir = out_dir.join("app-icon");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("compiled")).map_err(|e| format!("{}: {e}", dir.display()))?;
    copy_dir(source, &dir.join(format!("{APP_NAME}.icon")))?;
    run(Command::new("/usr/bin/xcrun").args(actool_args(&dir))).map_err(|e| {
        format!("{e} (the Mac app icon needs Xcode 26 or later: xcrun actool --version)")
    })?;
    let (car, icns) = app_icon_outputs(&dir.join("compiled"));
    for f in [&car, &icns] {
        if !f.exists() {
            return Err(format!("actool wrote no {}", f.display()));
        }
    }
    Ok((car, icns))
}

/// Copies a folder of plain files and folders.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for e in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let (f, t) = (e.path(), to.join(e.file_name()));
        if f.is_dir() {
            copy_dir(&f, &t)?;
        } else {
            fs::copy(&f, &t).map_err(|e| format!("{} to {}: {e}", f.display(), t.display()))?;
        }
    }
    Ok(())
}

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

/// Where the parts of a release come from.
pub struct Inputs<'a> {
    /// The repository root (resources, the app icon's source).
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
    let tools = TOOLS
        .iter()
        .map(|t| need(i.bin_dir.join(t)))
        .collect::<Result<Vec<_>, _>>()?;
    let engine_license = need(i.engine_dir.join(ENGINE_LICENSE))?;
    let plugin = need(i.root.join("crates/flare-agent/claude-plugin"))?;
    let agent_jar = need(i.root.join(AGENT_JAR_SOURCE))?;
    need(plugin.join("hooks/hooks.json"))?;
    let license = need(i.root.join("LICENSE"))?;
    let notices = need(i.root.join("THIRD-PARTY-NOTICES.md"))?;
    let credits = need(i.cef_dir.join("CREDITS.html"))?;
    let icon_src = need(i.root.join(APP_ICON_SOURCE))?;
    fs::create_dir_all(i.out_dir).map_err(|e| format!("{}: {e}", i.out_dir.display()))?;
    let icns = make_icns(&icon_src, i.out_dir)?;
    let dock_tile = build_dock_tile(&need(i.root.join(DOCK_TILE_SOURCE))?, i.out_dir, i.version)?;
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
            icon_catalog: None,
            executables: tools.iter().map(PathBuf::as_path).collect(),
            resources: vec![
                (&plugin, "claude-plugin"),
                (&license, "LICENSE"),
                (&notices, "THIRD-PARTY-NOTICES.md"),
                (&credits, "CEF-CREDITS.html"),
                (&engine_license, ENGINE_LICENSE),
                (&agent_jar, AGENT_JAR),
            ],
            frameworks: vec![(&engine, ENGINE_FOLDER)],
            plugins: vec![&dock_tile],
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
            // Machine pairing (rule 1254): the local network prompt's sentence, and the one Bonjour service.
            "<key>NSLocalNetworkUsageDescription</key>\n  <string>Flare finds and links your other computers",
            "<key>NSBonjourServices</key>\n  <array><string>_flare-pair._tcp</string></array>",
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

    /// Rule 1276: the app names the Dock tile plugin, and the plugin's source defines the class its plist names.
    #[test]
    fn the_app_names_the_dock_tile_plugin_and_its_source_defines_its_class() {
        let keys = app_keys("2026");
        let (_, v) = keys
            .iter()
            .find(|(k, _)| *k == "NSDockTilePlugIn")
            .expect("the app names its Dock tile plugin");
        assert_eq!(v, "<string>FlareDockTile.docktileplugin</string>");
        let plist = dock_tile_plist("1.2.3");
        for want in [
            "<string>BNDL</string>",
            "<string>com.flare.terminal.FlareDockTile</string>",
            "<string>FlareDockTilePlugIn_com_flare_terminal</string>",
            "<string>1.2.3</string>",
            "<string>15.0</string>",
        ] {
            assert!(plist.contains(want), "{want} in {plist}");
        }
        let source = include_str!("FlareDockTile.m");
        assert!(source.contains(&format!("@implementation {DOCK_TILE_CLASS}")));
    }

    /// Rule 1276: clang builds the plugin into a bundle macOS reads (arm64, its plist valid), and a bundle carrying it
    /// signs inside out and verifies. Skipped where Xcode's clang cannot run.
    #[test]
    fn the_dock_tile_plugin_builds_and_the_app_signs_with_it() {
        if !Command::new("/usr/bin/xcrun")
            .args(["clang", "--version"])
            .output()
            .is_ok_and(|o| o.status.success())
        {
            eprintln!("no clang here: skipped");
            return;
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let tmp = std::env::temp_dir().join(format!("flare-docktile-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let plugin = build_dock_tile(&root.join(DOCK_TILE_SOURCE), &tmp, "1.0").expect("built");
        let exe = plugin.join("Contents/MacOS").join(DOCK_TILE);
        assert!(crate::macos::sign::is_macho(&exe));
        let lint = Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(plugin.join("Contents/Info.plist"))
            .output()
            .expect("plutil");
        assert!(lint.status.success(), "{lint:?}");
        let syms = Command::new("/usr/bin/nm")
            .arg("-gU")
            .arg(&exe)
            .output()
            .expect("nm");
        let syms = String::from_utf8_lossy(&syms.stdout);
        assert!(
            syms.contains(&format!("_OBJC_CLASS_$_{DOCK_TILE_CLASS}")),
            "{syms}"
        );

        let app = tmp.join("Flare.app");
        let contents = app.join("Contents");
        fs::create_dir_all(contents.join("MacOS")).expect("dir");
        fs::write(
            contents.join("Info.plist"),
            bundle::info_plist(
                "Flare",
                "dev.flare.t",
                "1.0",
                false,
                false,
                &app_keys("2026"),
            ),
        )
        .expect("plist");
        fs::copy("/usr/bin/true", contents.join("MacOS/Flare")).expect("exe");
        fs::create_dir_all(contents.join(bundle::PLUGINS)).expect("dir");
        copy_dir(
            &plugin,
            &contents
                .join(bundle::PLUGINS)
                .join(format!("{DOCK_TILE}.docktileplugin")),
        )
        .expect("copied");
        bundle::sign_bundle(
            &app,
            "Flare",
            "dev.flare.t",
            &Signing::AdHoc,
            &tmp.join("ent"),
        )
        .expect("signs");
        bundle::verify(&app).expect("the seal holds with the plugin");
        let out = Command::new("/usr/bin/codesign")
            .arg("-dv")
            .arg(
                contents
                    .join(bundle::PLUGINS)
                    .join(format!("{DOCK_TILE}.docktileplugin")),
            )
            .output()
            .expect("codesign");
        let text = String::from_utf8_lossy(&out.stderr);
        assert!(
            text.contains("Identifier=dev.flare.t.FlareDockTile"),
            "{text}"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_bundle_id_is_the_id_windows_gives_flare() {
        assert_eq!(BUNDLE_ID, "com.flare.terminal");
        let names = include_str!("../../../flare-win/src/names.rs");
        assert!(names.contains(&format!("pub const AUMID: &str = \"{BUNDLE_ID}\";")));
        assert_eq!(MIN_MACOS, "15.0");
    }

    /// Device panel PR 6 (issue #248): the release bundle carries `flare-sim-host` in `Contents/MacOS` as a tool,
    /// under the name the app starts it by, built by make-app.sh and checked by pack.sh like the other tools.
    #[test]
    fn the_bundle_lists_the_sim_host_as_a_tool() {
        assert!(TOOLS.contains(&SIM_HOST_EXE), "{TOOLS:?}");
        assert!(TOOLS.contains(&"flare-mcp") && TOOLS.contains(&"flare-parakeet-host"));
        let names = include_str!("../../../flare-win/src/names.rs");
        assert!(
            names.contains(&format!(
                "pub const SIM_HOST_EXE: &str = \"{SIM_HOST_EXE}\";"
            )),
            "the app starts the sim host by another name"
        );
        let app = Path::new("/x/Flare.app");
        for t in TOOLS {
            assert_eq!(
                bundle::role_of(app, &app.join("Contents/MacOS").join(t)),
                crate::macos::sign::Role::Tool,
                "{t}"
            );
        }
        let make = include_str!("../../../../tools/release/mac/make-app.sh");
        assert!(
            make.contains("cargo build --release --locked -p flare-device --bin flare-sim-host"),
            "make-app.sh builds the sim host"
        );
        let pack = include_str!("../../../../tools/release/mac/pack.sh");
        let loop_line = pack
            .lines()
            .find(|l| l.trim_start().starts_with("for t in UpdateMac "))
            .expect("pack.sh checks each tool");
        for t in TOOLS {
            assert!(
                loop_line
                    .split_whitespace()
                    .any(|w| w.trim_end_matches(';') == t),
                "{t}: {loop_line}"
            );
        }
        let doc = include_str!("release.rs");
        assert!(doc.contains("//!   MacOS/flare-sim-host "));
    }

    /// Android plan PR A6: the release bundle carries `flare-emu-host` in `Contents/MacOS` as a tool and the agent helper
    /// jar in `Contents/Resources`, under the names the app and the emu host look for. make-app.sh builds the emu host
    /// and pack.sh checks both.
    #[test]
    fn the_bundle_lists_the_emu_host_as_a_tool_and_carries_the_agent_jar() {
        assert!(TOOLS.contains(&EMU_HOST_EXE), "{TOOLS:?}");
        let app = Path::new("/x/Flare.app");
        assert_eq!(
            bundle::role_of(app, &app.join("Contents/MacOS").join(EMU_HOST_EXE)),
            crate::macos::sign::Role::Tool
        );
        let names = include_str!("../../../flare-win/src/names.rs");
        for want in [
            format!("pub const EMU_HOST_EXE: &str = \"{EMU_HOST_EXE}\";"),
            format!("pub const AGENT_JAR: &str = \"{AGENT_JAR}\";"),
        ] {
            assert!(names.contains(&want), "{want}");
        }
        // From Contents/MacOS the emu host looks for the jar in ../Resources.
        let agent = include_str!("../../../flare-emu-host/src/agent.rs");
        assert!(agent.contains(&format!("dir.join(\"../Resources/{AGENT_JAR}\")")));
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(root.join(AGENT_JAR_SOURCE).is_file());
        let me = include_str!("release.rs");
        let body = me
            .split("pub fn build(i: &Inputs<'_>)")
            .nth(1)
            .expect("build");
        assert!(body.contains("need(i.root.join(AGENT_JAR_SOURCE))"));
        assert!(body.contains("(&agent_jar, AGENT_JAR),"));
        let make = include_str!("../../../../tools/release/mac/make-app.sh");
        assert!(
            make.contains("cargo build --release --locked -p flare-emu-host"),
            "make-app.sh builds the emu host"
        );
        let pack = include_str!("../../../../tools/release/mac/pack.sh");
        assert!(
            pack.contains("\"$app/Contents/Resources/flare-agent.jar\""),
            "pack.sh checks the jar"
        );
        assert!(me.contains("//!   MacOS/flare-emu-host "));
        assert!(me.contains("//!   Resources/flare-agent.jar "));
    }

    /// Rule 1230: the shipped Mac app icon is the original Flare icon, the 1024 px render tools/make-icon.py makes of
    /// assets/icon/flare-color.svg, turned into Flare.icns by sips and iconutil. The release names it by
    /// CFBundleIconFile only: no Icon Composer catalog (CFBundleIconName) takes its place on macOS 26.
    #[test]
    fn the_mac_app_icon_is_the_original_flare_icon() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert_eq!(APP_ICON_SOURCE, "build/icon-source.png");
        let png = fs::read(root.join(APP_ICON_SOURCE)).expect("build/icon-source.png");
        assert_eq!(&png[1..4], b"PNG");
        let make = fs::read_to_string(root.join("tools/make-icon.py")).expect("make-icon.py");
        assert!(make.contains("SVG = ROOT / 'assets' / 'icon' / 'flare-color.svg'"));
        let me = fs::read_to_string(root.join("crates/flare-cef/src/macos/release.rs"))
            .expect("release.rs");
        let body = me
            .split("pub fn build(i: &Inputs<'_>)")
            .nth(1)
            .expect("build");
        let body = body.split("\n}\n").next().expect("its body");
        assert!(body.contains("make_icns(&icon_src, i.out_dir)"), "{body}");
        assert!(body.contains("icon_catalog: None,"), "{body}");
        assert!(!body.contains("compile_app_icon"), "{body}");
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

    /// The Liquid Glass Icon Composer icon (#199) stays compilable by actool, for a later bundle that carries it.
    #[test]
    fn the_glass_icon_is_an_icon_composer_icon_actool_can_compile() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let src = root.join(GLASS_ICON_SOURCE);
        let json = fs::read_to_string(src.join("icon.json")).expect("icon.json");
        assert!(json.contains("\"macOS\""), "{json}");
        for layer in ["flame.svg", "golden-flame.svg"] {
            assert!(json.contains(layer), "{layer}");
            assert!(src.join("Assets").join(layer).exists(), "{layer}");
        }
        let args: Vec<String> = actool_args(Path::new("/x"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let joined = args.join(" ");
        for want in [
            "actool /x/Flare.icon --compile /x/compiled",
            "--platform macosx",
            "--minimum-deployment-target 15.0",
            "--app-icon Flare",
        ] {
            assert!(joined.contains(want), "{want}: {joined}");
        }
        let (car, icns) = app_icon_outputs(Path::new("/x"));
        assert_eq!(car, Path::new("/x/Assets.car"));
        assert_eq!(icns, Path::new("/x/Flare.icns"));
    }

    /// actool really compiles the icon (Xcode 26 on this Mac and on the macos-26 release runner): an asset catalog
    /// and an `.icns` come out. Skipped where actool cannot run.
    #[test]
    fn actool_compiles_the_app_icon_into_a_catalog_and_an_icns() {
        let ok = Command::new("/usr/bin/xcrun")
            .args(["actool", "--version"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("no actool here: skipped");
            return;
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let tmp = std::env::temp_dir().join(format!("flare-app-icon-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let (car, icns) = compile_app_icon(&root.join(GLASS_ICON_SOURCE), &tmp).expect("compiled");
        assert!(fs::metadata(&car).expect("car").len() > 1000);
        let head = fs::read(&icns).expect("icns");
        assert_eq!(&head[..4], b"icns");
        let _ = fs::remove_dir_all(&tmp);
    }
}
