//! Signing a bundle (port phase 8): ad hoc for the dev bundles, or a Developer ID with the hardened runtime, a secure
//! timestamp and each program's own entitlements for `Flare.app` (and, with [`TEST_IDENTITY_VAR`] and
//! [`TEST_KEYCHAIN_VAR`] both set, the engine tests' bundles: the way the entitlements below are proved, running every
//! probe scenario under the hardened runtime).
//!
//! The entitlements are the ones Chromium's own Mac build gives the same processes (Chromium 156.0.8078.48,
//! `chrome/app/*-entitlements.plist`, rule 1036), minus the App Sandbox keys (Flare is not sandboxed), plus what a
//! terminal needs for the programs it runs (rule 1042). Each is written with why it is there:
//!
//! - `Flare` (the browser process, the window, the terminal): `device.audio-input`, `device.camera` and
//!   `automation.apple-events`. macOS holds the app that started a program responsible for what the program asks
//!   for (TCC), so `osascript`, `ffmpeg -f avfoundation` or `sox` in Flare's terminal is refused, silently, by a
//!   hardened app without the key. Each key has its usage string in `Info.plist` (`release::app_keys`).
//! - `Flare Helper (Renderer)` and `Flare Helper (GPU)`: `cs.allow-jit` (V8; SwiftShader's software GL in the GPU
//!   process, Chromium's `helper-gpu-entitlements.plist`).
//! - `Flare Helper`, `(Plugin)`, `(Alerts)`, `flare-mcp`: none. Flare has no plugins (CEF 156 has no Pepper).
//!
//! Never `allow-unsigned-executable-memory`, `disable-library-validation` or the dyld environment key: every Mach-O
//! in the bundle is signed by the same team; a library downloaded later (the dictation engines) must be signed by the
//! team too, or run in a helper of its own (rule 1042). `verify_release` refuses any key a program is not given here.
//!
//! `codesign` with a Developer ID reads the private key from the keychain, and when the key's access list does not
//! let it in, the system puts a SecurityAgent prompt on the screen that stays there even when `codesign` is stopped.
//! So [`sign`] first asks [`crate::macos::keychain::check`] (which cannot raise a prompt) and refuses unless the key
//! can be used without one; only then does `codesign` run, one at a time across processes, stopped at once if a
//! SecurityAgent window appears anyway, and stopped after a time that grows with what it hashes (rule 1040).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// How long a Developer ID `codesign` may run at least (no timestamp), before the size allowance.
pub const SIGN_TIMEOUT: Duration = Duration::from_secs(20);

/// The team every Mach-O of a release bundle must be signed by (Max's Developer ID).
pub const TEAM: &str = "WV7BY8CUM8";

/// The release identity (`tools/release/mac/make-app.sh`, `flare-mac-app`). Never read by the tests.
pub const RELEASE_IDENTITY_VAR: &str = "FLARE_MAC_SIGN_IDENTITY";
/// The keychain the release identity is in (CI: the throwaway one); unset: the search list.
pub const RELEASE_KEYCHAIN_VAR: &str = "FLARE_SIGNING_KEYCHAIN";
/// The engine tests' identity: only with [`TEST_KEYCHAIN_VAR`] set too (else ad hoc).
pub const TEST_IDENTITY_VAR: &str = "FLARE_TEST_SIGN_IDENTITY";
/// The keychain the tests' identity is in: required, so a test signs only from a keychain chosen for it.
pub const TEST_KEYCHAIN_VAR: &str = "FLARE_TEST_SIGN_KEYCHAIN";

/// How a bundle is signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signing {
    /// Ad hoc (`--sign -`): runs on this Mac only. Each program still gets its role's identifier and entitlements,
    /// so a static check of an ad-hoc bundle sees the same roles a release has; the hardened runtime only for
    /// [`Role::Tool`] ([`adhoc_runtime`]): library validation refuses the ad-hoc framework (no team) to the rest.
    AdHoc,
    /// A Developer ID identity (its name or SHA-1, from the keychain), the hardened runtime and the entitlements.
    Identity {
        identity: String,
        /// A secure timestamp from Apple (`--timestamp`): notarization needs it. Off for the local test bundles,
        /// which are signed again on every test run.
        timestamp: bool,
        /// The keychain that holds it (`codesign --keychain`); None: the search list.
        keychain: Option<PathBuf>,
    },
}

fn set(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|v| !v.is_empty() && *v != "-")
}

impl Signing {
    /// The release signing from the identity and keychain given (trimmed; empty or `-` is ad hoc).
    pub fn release(identity: Option<&str>, keychain: Option<&str>) -> Signing {
        match set(identity) {
            Some(id) => Signing::Identity {
                identity: id.to_string(),
                timestamp: true,
                keychain: set(keychain).map(PathBuf::from),
            },
            None => Signing::AdHoc,
        }
    }

    /// The engine tests' signing from their two values: an identity only with a keychain, never timestamped.
    pub fn for_tests_from(identity: Option<&str>, keychain: Option<&str>) -> Signing {
        match (set(identity), set(keychain)) {
            (Some(id), Some(kc)) => Signing::Identity {
                identity: id.to_string(),
                timestamp: false,
                keychain: Some(PathBuf::from(kc)),
            },
            _ => Signing::AdHoc,
        }
    }

    /// The engine tests' signing: [`TEST_IDENTITY_VAR`] in [`TEST_KEYCHAIN_VAR`], else ad hoc. The release variable
    /// is never read here, so exporting it for a release does not sign every test run.
    pub fn for_tests() -> Signing {
        let id = std::env::var(TEST_IDENTITY_VAR).ok();
        let kc = std::env::var(TEST_KEYCHAIN_VAR).ok();
        Signing::for_tests_from(id.as_deref(), kc.as_deref())
    }
}

/// Which program of the bundle a signature is for: it decides the entitlements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The app's own executable (the browser process).
    App,
    /// A CEF helper app, by its suffix (`Helper (GPU)`).
    Helper(&'static str),
    /// Another program in `Contents/MacOS` (`flare-mcp`).
    Tool,
    /// A library or the framework: no entitlements can apply.
    Library,
}

/// One entitlement, why it is there, and the `Info.plist` usage string macOS shows when it asks (TCC keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entitlement {
    pub key: &'static str,
    pub why: &'static str,
    pub usage_key: Option<&'static str>,
}

const AUDIO_INPUT: Entitlement = Entitlement {
    key: "com.apple.security.device.audio-input",
    why: "the microphone: programs run in the terminal (sox, ffmpeg), web pages in the panel, and dictation once it \
          is ported (provisional until a signed dictation run, rule 1042); a hardened app is refused it without this",
    usage_key: Some("NSMicrophoneUsageDescription"),
};

const CAMERA: Entitlement = Entitlement {
    key: "com.apple.security.device.camera",
    why: "the camera for programs run in the terminal (ffmpeg -f avfoundation, imagesnap) and web pages in the \
          panel; macOS holds Flare responsible for them and refuses a hardened app without this",
    usage_key: Some("NSCameraUsageDescription"),
};

const APPLE_EVENTS: Entitlement = Entitlement {
    key: "com.apple.security.automation.apple-events",
    why: "osascript and other Apple Events senders run in the terminal; a hardened app without this has every \
          event refused with no prompt",
    usage_key: Some("NSAppleEventsUsageDescription"),
};

const ALLOW_JIT_RENDERER: Entitlement = Entitlement {
    key: "com.apple.security.cs.allow-jit",
    why: "V8 compiles scripts into MAP_JIT pages; without it every renderer dies at its first script",
    usage_key: None,
};

const ALLOW_JIT_GPU: Entitlement = Entitlement {
    key: "com.apple.security.cs.allow-jit",
    why: "SwiftShader (software GL and WebGL where the GPU cannot be used: a VM, a blocklisted GPU) compiles shaders \
          into MAP_JIT pages; Chromium 156's helper-gpu-entitlements.plist has it",
    usage_key: None,
};

/// The entitlements of a role (see the module docs).
pub fn entitlements(role: Role) -> Vec<Entitlement> {
    match role {
        Role::App => vec![AUDIO_INPUT, CAMERA, APPLE_EVENTS],
        Role::Helper("Helper (Renderer)") => vec![ALLOW_JIT_RENDERER],
        Role::Helper("Helper (GPU)") => vec![ALLOW_JIT_GPU],
        Role::Helper(_) | Role::Tool | Role::Library => Vec::new(),
    }
}

/// The entitlements file of a role (an XML plist; each key's reason as a comment), or None when it has none.
pub fn entitlements_plist(role: Role) -> Option<String> {
    let list = entitlements(role);
    if list.is_empty() {
        return None;
    }
    let body: String = list
        .iter()
        .map(|e| {
            format!(
                "  <!-- {} -->\n  <key>{}</key>\n  <true/>\n",
                e.why.replace("--", "-"),
                e.key
            )
        })
        .collect();
    Some(crate::macos::bundle::plist_doc(&body))
}

/// Whether an ad-hoc signature of `role` carries the hardened runtime: only a tool (`flare-mcp`, Velopack's
/// `UpdateMac`), which loads nothing but the system's libraries. The app, the helpers and the framework load the CEF
/// framework, and library validation refuses an ad-hoc library ("no Team ID"), so they run without it ad hoc.
pub fn adhoc_runtime(role: Role) -> bool {
    role == Role::Tool
}

/// `codesign`'s arguments for one path (without the path).
pub fn codesign_args(
    signing: &Signing,
    role: Role,
    identifier: Option<&str>,
    entitlements: Option<&Path>,
) -> Vec<String> {
    let mut a = vec!["--force".to_string(), "--sign".to_string()];
    match signing {
        Signing::AdHoc => {
            a.push("-".into());
            if adhoc_runtime(role) {
                a.push("--options".into());
                a.push("runtime".into());
            }
            a.push("--timestamp=none".into());
            if let Some(id) = identifier {
                a.push("--identifier".into());
                a.push(id.to_string());
            }
            if let Some(e) = entitlements {
                a.push("--entitlements".into());
                a.push(e.to_string_lossy().into_owned());
            }
        }
        Signing::Identity {
            identity,
            timestamp,
            keychain,
        } => {
            a.push(identity.clone());
            if let Some(kc) = keychain {
                a.push("--keychain".into());
                a.push(kc.to_string_lossy().into_owned());
            }
            a.push("--options".into());
            a.push("runtime".into());
            a.push(if *timestamp {
                "--timestamp".into()
            } else {
                "--timestamp=none".into()
            });
            if let Some(id) = identifier {
                a.push("--identifier".into());
                a.push(id.to_string());
            }
            if let Some(e) = entitlements {
                a.push("--entitlements".into());
                a.push(e.to_string_lossy().into_owned());
            }
        }
    }
    a
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!("{cmd:?} failed: {}", text.trim()))
    }
}

/// The bytes under a path (a file, or every file in a folder; links not followed).
pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    fs::read_dir(path)
        .map(|rd| rd.flatten().map(|e| size_of(&e.path())).sum())
        .unwrap_or(0)
}

/// How long a Developer ID `codesign` of `bytes` may run: [`SIGN_TIMEOUT`] plus a second for every 4 MB it hashes
/// (the 300 MB framework: 95 s), and two minutes more for a timestamp (a call to Apple). Only a backstop: a prompt
/// is caught by its window at once (`run_guarded`).
pub fn sign_timeout(bytes: u64, timestamp: bool) -> Duration {
    let hashing = Duration::from_secs(bytes / (4 * 1024 * 1024));
    SIGN_TIMEOUT
        + hashing
        + if timestamp {
            Duration::from_secs(120)
        } else {
            Duration::ZERO
        }
}

/// The on-screen windows of the keychain's prompt process, by pid.
fn prompt_windows() -> Vec<i64> {
    crate::macos::screen::on_screen()
        .into_iter()
        .filter(|w| w.owner.contains("SecurityAgent"))
        .map(|w| w.pid)
        .collect()
}

/// Runs a Developer ID `codesign`: stopped when a SecurityAgent window appears that was not there before (and that
/// prompt is closed: its process is stopped, as nobody is to answer it), or past `limit`.
fn run_guarded(cmd: &mut Command, limit: Duration) -> Result<String, String> {
    use std::process::Stdio;
    let before = prompt_windows();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{cmd:?}: {e}"))?;
    let end = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                let new: Vec<i64> = prompt_windows()
                    .into_iter()
                    .filter(|p| !before.contains(p))
                    .collect();
                if !new.is_empty() {
                    let _ = child.kill();
                    let _ = child.wait();
                    for pid in new {
                        let _ = Command::new("/bin/kill")
                            .args(["-9", &pid.to_string()])
                            .status();
                    }
                    return Err(format!(
                        "{cmd:?} raised a keychain prompt although the key's access list allowed codesign; it was \
                         stopped and the prompt closed. Check the key in Keychain Access (Access Control)"
                    ));
                }
                if std::time::Instant::now() >= end {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{cmd:?} did not finish within {}s and was stopped",
                        limit.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("{cmd:?}: {e}")),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{cmd:?}: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!("{cmd:?} failed: {}", text.trim()))
    }
}

/// Signs one path for a role. The entitlements file, when the role has one, is written into `scratch` (never into
/// the bundle); ad hoc signs carry them too. A Developer ID sign runs only when the key can be used without a
/// prompt ([`crate::macos::keychain::check`]), and one at a time across processes (a lock file in the temp dir).
pub fn sign(
    path: &Path,
    signing: &Signing,
    role: Role,
    identifier: Option<&str>,
    scratch: &Path,
) -> Result<(), String> {
    let ent = match entitlements_plist(role) {
        Some(text) => {
            fs::create_dir_all(scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
            let name = match role {
                Role::App => "app".to_string(),
                Role::Helper(s) => s
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .collect::<String>()
                    .to_ascii_lowercase(),
                Role::Tool => "tool".into(),
                Role::Library => "library".into(),
            };
            let p = scratch.join(format!("{name}.entitlements"));
            fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
            Some(p)
        }
        None => None,
    };
    let mut cmd = Command::new("/usr/bin/codesign");
    cmd.args(codesign_args(signing, role, identifier, ent.as_deref()))
        .arg(path);
    match signing {
        Signing::AdHoc => run(&mut cmd),
        Signing::Identity {
            identity,
            timestamp,
            keychain,
        } => {
            crate::macos::keychain::check(identity, keychain.as_deref())
                .map_err(|e| format!("not signing {}: {e}", path.display()))?;
            let lock_path = std::env::temp_dir().join("flare-codesign-identity.lock");
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)
                .map_err(|e| format!("{}: {e}", lock_path.display()))?;
            lock.lock()
                .map_err(|e| format!("{}: {e}", lock_path.display()))?;
            let r = run_guarded(&mut cmd, sign_timeout(size_of(path), *timestamp));
            let _ = lock.unlock();
            r
        }
    }
    .map(|_| ())
}

/// Whether the file starts with a Mach-O (thin or fat) magic number.
pub fn is_macho(path: &Path) -> bool {
    let mut b = [0u8; 4];
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    if f.read_exact(&mut b).is_err() {
        return false;
    }
    matches!(
        u32::from_be_bytes(b),
        0xfeed_face | 0xfeed_facf | 0xcefa_edfe | 0xcffa_edfe | 0xcafe_babe | 0xbeba_feca
    )
}

/// Every Mach-O file under `dir` (symbolic links are not followed), sorted.
pub fn machos_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() && is_macho(&e.path()) {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// What `codesign -dvv` says of one signed file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignatureInfo {
    pub team: Option<String>,
    pub runtime: bool,
    pub timestamp: bool,
    pub adhoc: bool,
    /// The leaf certificate (the first `Authority=` line).
    pub authority: Option<String>,
}

/// Reads `codesign -d --verbose=2` output.
pub fn parse_signature(text: &str) -> SignatureInfo {
    let mut s = SignatureInfo::default();
    for line in text.lines() {
        if let Some(t) = line.strip_prefix("TeamIdentifier=") {
            let t = t.trim();
            if t != "not set" {
                s.team = Some(t.to_string());
            }
        } else if line.starts_with("CodeDirectory ") {
            let flags = line.split_once("flags=").map_or("", |(_, f)| f);
            s.runtime = flags.contains("runtime");
            s.adhoc = flags.contains("adhoc");
        } else if line.starts_with("Timestamp=") {
            s.timestamp = true;
        } else if line.starts_with("Signature=adhoc") {
            s.adhoc = true;
        } else if let Some(a) = line.strip_prefix("Authority=") {
            s.authority.get_or_insert_with(|| a.trim().to_string());
        }
    }
    s
}

/// The entitlement keys of `codesign -d --entitlements - --xml` output, sorted.
pub fn entitlement_keys(xml: &str) -> Vec<String> {
    let mut keys: Vec<String> = xml
        .split("<key>")
        .skip(1)
        .filter_map(|k| k.split_once("</key>").map(|(k, _)| k.trim().to_string()))
        .collect();
    keys.sort();
    keys
}

/// What is wrong with one Mach-O of a release bundle (empty: nothing): a Developer ID Application signature of
/// `team`, the hardened runtime, a secure timestamp, and exactly the entitlements its role is given.
pub fn release_faults(
    info: &SignatureInfo,
    keys: &[String],
    role: Role,
    team: &str,
) -> Vec<String> {
    let mut why = Vec::new();
    if info.team.as_deref() != Some(team) {
        why.push(format!("team {:?}", info.team));
    }
    if !info
        .authority
        .as_deref()
        .is_some_and(|a| a.starts_with("Developer ID Application:"))
    {
        why.push(format!(
            "signed by {:?}, not a Developer ID Application certificate",
            info.authority
        ));
    }
    if !info.runtime {
        why.push("no hardened runtime".into());
    }
    if !info.timestamp {
        why.push("no secure timestamp".into());
    }
    let mut want: Vec<String> = entitlements(role)
        .iter()
        .map(|e| e.key.to_string())
        .collect();
    want.sort();
    if keys != want.as_slice() {
        why.push(format!("entitlements {keys:?}, {role:?} is given {want:?}"));
    }
    why
}

/// The checks a release bundle passes: `codesign --verify --deep --strict`, and every Mach-O in it passing
/// [`release_faults`] for its role (by its place in the bundle, [`crate::macos::bundle::role_of`]). The number of
/// Mach-O files checked.
pub fn verify_release(app: &Path, team: &str) -> Result<usize, String> {
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "--verbose=2"])
        .arg(app))?;
    let files = machos_in(app);
    if files.is_empty() {
        return Err(format!("no Mach-O in {}", app.display()));
    }
    let mut bad = Vec::new();
    for f in &files {
        let text = run(Command::new("/usr/bin/codesign")
            .args(["-d", "--verbose=2"])
            .arg(f))
        .unwrap_or_else(|e| e);
        let ent = run(Command::new("/usr/bin/codesign")
            .args(["-d", "--entitlements", "-", "--xml"])
            .arg(f))
        .unwrap_or_default();
        let role = crate::macos::bundle::role_of(app, f);
        let why = release_faults(&parse_signature(&text), &entitlement_keys(&ent), role, team);
        if !why.is_empty() {
            bad.push(format!("{}: {}", f.display(), why.join(", ")));
        }
    }
    if bad.is_empty() {
        Ok(files.len())
    } else {
        Err(bad.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BROAD: [&str; 5] = [
        "allow-unsigned-executable-memory",
        "disable-library-validation",
        "get-task-allow",
        "disable-executable-page-protection",
        "allow-dyld-environment-variables",
    ];

    fn keys(role: Role) -> Vec<&'static str> {
        entitlements(role).iter().map(|e| e.key).collect()
    }

    #[test]
    fn each_program_carries_chromiums_keys_for_it_plus_the_terminals_and_never_the_broad_ones() {
        assert_eq!(
            keys(Role::App),
            [
                "com.apple.security.device.audio-input",
                "com.apple.security.device.camera",
                "com.apple.security.automation.apple-events"
            ]
        );
        // Every TCC key of the app names the usage string macOS shows; the JIT keys have none.
        for e in entitlements(Role::App) {
            assert!(
                e.usage_key
                    .is_some_and(|u| u.starts_with("NS") && u.ends_with("UsageDescription")),
                "{}",
                e.key
            );
        }
        assert_eq!(
            keys(Role::Helper("Helper (Renderer)")),
            ["com.apple.security.cs.allow-jit"]
        );
        assert_eq!(
            keys(Role::Helper("Helper (GPU)")),
            ["com.apple.security.cs.allow-jit"]
        );
        for s in ["Helper", "Helper (Plugin)", "Helper (Alerts)"] {
            assert!(entitlements(Role::Helper(s)).is_empty(), "{s}");
            assert!(entitlements_plist(Role::Helper(s)).is_none(), "{s}");
        }
        assert!(entitlements(Role::Tool).is_empty());
        assert!(entitlements(Role::Library).is_empty());
        let mut every: Vec<Role> = crate::pin::HELPER_SUFFIXES
            .iter()
            .map(|s| Role::Helper(s))
            .collect();
        every.extend([Role::App, Role::Tool, Role::Library]);
        for r in every {
            for e in entitlements(r) {
                assert!(!e.why.is_empty());
                assert!(e.usage_key.is_none() || r == Role::App);
                for broad in BROAD {
                    assert!(!e.key.contains(broad), "{r:?}: {}", e.key);
                }
            }
        }
        let p = entitlements_plist(Role::Helper("Helper (GPU)")).expect("a file");
        assert!(p.contains("<key>com.apple.security.cs.allow-jit</key>\n  <true/>"));
        assert!(p.contains("<!-- SwiftShader"));
        // A reason never ends the XML comment early.
        for r in [
            Role::App,
            Role::Helper("Helper (Renderer)"),
            Role::Helper("Helper (GPU)"),
        ] {
            let p = entitlements_plist(r).expect("a file");
            assert_eq!(p.matches("<!--").count(), p.matches("-->").count());
            assert_eq!(entitlement_keys(&p).len(), entitlements(r).len());
        }
    }

    #[test]
    fn a_developer_id_signature_has_the_runtime_and_a_timestamp_and_ad_hoc_has_neither() {
        let id = Signing::release(
            Some(" Developer ID Application: X (T) "),
            Some("/k/ci.keychain-db"),
        );
        let a = codesign_args(
            &id,
            Role::App,
            Some("com.flare.terminal"),
            Some(Path::new("/e")),
        );
        assert_eq!(
            a,
            [
                "--force",
                "--sign",
                "Developer ID Application: X (T)",
                "--keychain",
                "/k/ci.keychain-db",
                "--options",
                "runtime",
                "--timestamp",
                "--identifier",
                "com.flare.terminal",
                "--entitlements",
                "/e"
            ]
        );
        assert_eq!(Signing::release(Some("-"), None), Signing::AdHoc);
        assert_eq!(Signing::release(None, Some("/k")), Signing::AdHoc);
        let no_kc = Signing::release(Some("X"), Some(" "));
        assert!(
            !codesign_args(&no_kc, Role::Library, None, None).contains(&"--keychain".to_string())
        );
        let local = Signing::for_tests_from(Some("X"), Some("/k/test.keychain-db"));
        assert!(
            codesign_args(&local, Role::Library, None, None)
                .contains(&"--timestamp=none".to_string())
        );
        // Ad hoc: the role's identifier and entitlements, no timestamp, the hardened runtime for a tool only (the
        // rest load the ad-hoc CEF framework, which library validation would refuse).
        let adhoc = codesign_args(&Signing::AdHoc, Role::App, Some("x"), Some(Path::new("/e")));
        assert_eq!(
            adhoc,
            [
                "--force",
                "--sign",
                "-",
                "--timestamp=none",
                "--identifier",
                "x",
                "--entitlements",
                "/e"
            ]
        );
        let tool = codesign_args(
            &Signing::AdHoc,
            Role::Tool,
            Some("com.flare.terminal.UpdateMac"),
            None,
        );
        assert_eq!(
            tool,
            [
                "--force",
                "--sign",
                "-",
                "--options",
                "runtime",
                "--timestamp=none",
                "--identifier",
                "com.flare.terminal.UpdateMac"
            ]
        );
        for r in [
            Role::App,
            Role::Library,
            Role::Helper("Helper (Renderer)"),
            Role::Helper("Helper"),
        ] {
            assert!(!adhoc_runtime(r), "{r:?}");
        }
        assert_eq!(
            codesign_args(&Signing::AdHoc, Role::Library, None, None),
            ["--force", "--sign", "-", "--timestamp=none"]
        );
    }

    #[test]
    fn the_tests_sign_only_with_their_own_identity_and_a_keychain_never_with_the_release_one() {
        assert_ne!(TEST_IDENTITY_VAR, RELEASE_IDENTITY_VAR);
        assert_ne!(TEST_KEYCHAIN_VAR, RELEASE_KEYCHAIN_VAR);
        // An identity alone (what exporting the release identity looked like to the tests before): ad hoc.
        assert_eq!(
            Signing::for_tests_from(Some("Developer ID Application: X"), None),
            Signing::AdHoc
        );
        assert_eq!(Signing::for_tests_from(None, Some("/k")), Signing::AdHoc);
        assert_eq!(
            Signing::for_tests_from(Some("X"), Some("/k")),
            Signing::Identity {
                identity: "X".into(),
                timestamp: false,
                keychain: Some(PathBuf::from("/k"))
            }
        );
        let src = include_str!("sign.rs");
        let body = src
            .split("pub fn for_tests() -> Signing {")
            .nth(1)
            .and_then(|b| b.split("\n    }\n").next())
            .expect("for_tests");
        assert!(!body.contains("RELEASE_"), "{body}");
    }

    #[test]
    fn the_sign_timeout_grows_with_what_codesign_hashes() {
        assert_eq!(sign_timeout(0, false), SIGN_TIMEOUT);
        let fw = sign_timeout(300 * 1024 * 1024, false);
        assert_eq!(fw, Duration::from_secs(20 + 75));
        assert_eq!(
            sign_timeout(300 * 1024 * 1024, true),
            fw + Duration::from_secs(120)
        );
        let dir = std::env::temp_dir().join(format!("flare-sign-size-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a")).expect("dir");
        fs::write(dir.join("a/x"), [0u8; 1000]).expect("write");
        fs::write(dir.join("y"), [0u8; 24]).expect("write");
        assert_eq!(size_of(&dir), 1024);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn codesigns_display_is_read_for_team_runtime_timestamp_and_authority() {
        let signed = "Executable=/A.app/Contents/MacOS/A\nIdentifier=com.flare.terminal\n\
            CodeDirectory v=20500 size=1 flags=0x10000(runtime) hashes=1+7 location=embedded\n\
            Authority=Developer ID Application: MAX AUBERT (WV7BY8CUM8)\n\
            Authority=Developer ID Certification Authority\nAuthority=Apple Root CA\n\
            Timestamp=8 Nov 2026 at 20:00:00\nTeamIdentifier=WV7BY8CUM8\n";
        assert_eq!(
            parse_signature(signed),
            SignatureInfo {
                team: Some("WV7BY8CUM8".into()),
                runtime: true,
                timestamp: true,
                adhoc: false,
                authority: Some("Developer ID Application: MAX AUBERT (WV7BY8CUM8)".into()),
            }
        );
        let adhoc = "CodeDirectory v=20400 size=1 flags=0x20002(adhoc,linker-signed) hashes=1+0\n\
            Signature=adhoc\nTeamIdentifier=not set\n";
        let a = parse_signature(adhoc);
        assert!(a.adhoc && a.team.is_none() && !a.runtime && !a.timestamp && a.authority.is_none());
    }

    #[test]
    fn a_release_mach_o_has_exactly_its_roles_keys_and_a_developer_id_application_signer() {
        let good = SignatureInfo {
            team: Some(TEAM.into()),
            runtime: true,
            timestamp: true,
            adhoc: false,
            authority: Some("Developer ID Application: MAX AUBERT (WV7BY8CUM8)".into()),
        };
        let jit = vec!["com.apple.security.cs.allow-jit".to_string()];
        assert!(release_faults(&good, &jit, Role::Helper("Helper (Renderer)"), TEAM).is_empty());
        assert!(release_faults(&good, &[], Role::Library, TEAM).is_empty());
        let mut app: Vec<String> = keys(Role::App).iter().map(|k| k.to_string()).collect();
        app.sort();
        assert!(release_faults(&good, &app, Role::App, TEAM).is_empty());
        // A broad key, or a key on the wrong program, is refused.
        for extra in BROAD {
            let mut k = jit.clone();
            k.push(format!("com.apple.security.cs.{extra}"));
            k.sort();
            let f = release_faults(&good, &k, Role::Helper("Helper (Renderer)"), TEAM);
            assert!(f.iter().any(|w| w.contains("entitlements")), "{extra}");
        }
        assert!(!release_faults(&good, &jit, Role::Tool, TEAM).is_empty());
        assert!(!release_faults(&good, &[], Role::App, TEAM).is_empty());
        // The same team's Apple Development certificate is not a release signer.
        let dev = SignatureInfo {
            authority: Some("Apple Development: MAX AUBERT (ABCDE12345)".into()),
            ..good.clone()
        };
        let f = release_faults(&dev, &[], Role::Library, TEAM);
        assert!(
            f.len() == 1 && f[0].contains("Developer ID Application"),
            "{f:?}"
        );
        let other = SignatureInfo {
            team: Some("OTHER".into()),
            timestamp: false,
            runtime: false,
            ..good
        };
        assert_eq!(release_faults(&other, &[], Role::Library, TEAM).len(), 3);
        assert_eq!(
            entitlement_keys("<dict><key>b</key><true/><key>a</key><true/></dict>"),
            ["a", "b"]
        );
    }

    #[test]
    fn a_mach_o_is_known_by_its_magic_and_nothing_else_is() {
        let dir = std::env::temp_dir().join(format!("flare-sign-magic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).expect("dir");
        fs::write(dir.join("thin"), [0xcf, 0xfa, 0xed, 0xfe, 0, 0]).expect("write");
        fs::write(dir.join("sub/fat"), [0xca, 0xfe, 0xba, 0xbe, 0]).expect("write");
        fs::write(dir.join("text.json"), b"{}").expect("write");
        fs::write(dir.join("short"), [0xcf]).expect("write");
        let found = machos_in(&dir);
        assert_eq!(found, [dir.join("sub/fat"), dir.join("thin")]);
        let _ = fs::remove_dir_all(&dir);
    }
}
