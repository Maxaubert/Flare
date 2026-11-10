//! The release `Flare.app` (port phase 8; `tools/release/mac/make-app.sh` runs it, then notarizes). macOS only.
//!
//! ```text
//! flare-mac-app <out dir> <bin dir>     build and sign Flare.app from the release flare, flare-mcp, flare-cef-helper
//! flare-mac-app check-key               whether codesign can use the identity's key without a prompt (exit 1: no)
//! flare-mac-app resign <app>            sign a bundle again, inside out by role (one a packager added programs to)
//! flare-mac-app sign-file <file>        sign one file, the DMG (no entitlements)
//! flare-mac-app verify <app>            the release checks (sign::verify_release)
//!   [--identity <Developer ID>] [--keychain <path>] [--team <id>] [--identifier <bundle id>]
//!   [--version <x.y.z>]                 the bundle's version (default: the workspace's; a throwaway test pack)
//! codesign <vpk's arguments>            run through a link named `codesign`: the signing `vpk pack` asks for
//! ```
//!
//! Run as `codesign` (`tools/release/mac/pack.sh` puts such a link first on vpk's `PATH`), it is the shim of
//! `flare_cef::macos::vpk`: vpk's call for its update program does nothing, its call for the bundle signs the whole
//! bundle again inside out by role and checks it (rules 1043 and 1046).
//!
//! The identity is `--identity`, else `FLARE_MAC_SIGN_IDENTITY`, else ad hoc (`-`); the keychain `--keychain`, else
//! `FLARE_SIGNING_KEYCHAIN`, else the search list. A Developer ID signs with the hardened runtime and a secure
//! timestamp, only when the key cannot raise a prompt (rule 1040), and a built or re-signed bundle is then checked.
//! Prints the app's (or file's) path.

#[cfg(target_os = "macos")]
fn main() {
    use flare_cef::macos::{bundle, keychain, release, sign};
    use std::path::{Path, PathBuf};
    let fail = |e: String| -> ! {
        eprintln!("{e}");
        std::process::exit(1);
    };
    let argv0 = std::env::args().next().unwrap_or_default();
    let a: Vec<String> = std::env::args().skip(1).collect();
    if flare_cef::macos::vpk::is_shim(&argv0) {
        vpk_codesign(&a);
    }
    let mut pos = Vec::new();
    let mut identity = std::env::var(sign::RELEASE_IDENTITY_VAR).unwrap_or_default();
    let mut kc = std::env::var(sign::RELEASE_KEYCHAIN_VAR).unwrap_or_default();
    let mut team = sign::TEAM.to_string();
    let mut identifier = release::BUNDLE_ID.to_string();
    let mut version = env!("CARGO_PKG_VERSION").to_string();
    let mut it = a.iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--identity" => identity = it.next().cloned().unwrap_or_default(),
            "--keychain" => kc = it.next().cloned().unwrap_or_default(),
            "--team" => team = it.next().cloned().unwrap_or_default(),
            "--identifier" => identifier = it.next().cloned().unwrap_or_default(),
            "--version" => version = it.next().cloned().unwrap_or_default(),
            _ => pos.push(x.clone()),
        }
    }
    let signing = sign::Signing::release(Some(&identity), Some(&kc));
    let verify = |app: &Path| {
        if let sign::Signing::Identity { .. } = signing {
            match sign::verify_release(app, &team) {
                Ok(n) => eprintln!(
                    "signed: {n} Mach-O files, Developer ID Application of team {team}, hardened runtime, \
                     timestamped, each with its role's entitlements"
                ),
                Err(e) => fail(format!("the signature is not what a release needs:\n{e}")),
            }
        }
    };
    let scratch = |p: &Path| -> PathBuf {
        p.parent()
            .unwrap_or(Path::new("."))
            .join(format!("{}.entitlements", release::APP_NAME))
    };
    match pos.first().map(String::as_str) {
        Some("check-key") if pos.len() == 1 => match &signing {
            sign::Signing::AdHoc => println!("ad hoc: no key"),
            sign::Signing::Identity {
                identity, keychain, ..
            } => match keychain::check(identity, keychain.as_deref()) {
                Ok(()) => println!("codesign can use the key of {identity:?} without a prompt"),
                Err(e) => fail(e),
            },
        },
        Some("resign") if pos.len() == 2 => {
            let app = Path::new(&pos[1]);
            let name = app.file_stem().and_then(|n| n.to_str()).unwrap_or_default();
            bundle::sign_bundle(app, name, &identifier, &signing, &scratch(app))
                .unwrap_or_else(|e| fail(e));
            verify(app);
            println!("{}", app.display());
        }
        Some("sign-file") if pos.len() == 2 => {
            let f = Path::new(&pos[1]);
            sign::sign(f, &signing, sign::Role::Library, None, &scratch(f))
                .unwrap_or_else(|e| fail(e));
            println!("{}", f.display());
        }
        Some("verify") if pos.len() == 2 => match sign::verify_release(Path::new(&pos[1]), &team) {
            Ok(n) => println!("ok: {n} Mach-O files"),
            Err(e) => fail(e),
        },
        Some(_) if pos.len() == 2 => {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let root = root.canonicalize().unwrap_or(root);
            let year = std::env::var("FLARE_COPYRIGHT_YEAR").unwrap_or_else(|_| "2026".into());
            let inputs = release::Inputs {
                root: &root,
                bin_dir: Path::new(&pos[1]),
                cef_dir: Path::new(flare_cef::macos::CEF_DIR),
                out_dir: Path::new(&pos[0]),
                version: &version,
                year: &year,
                signing: signing.clone(),
            };
            let app = release::build(&inputs).unwrap_or_else(|e| fail(e));
            verify(&app);
            println!("{}", app.display());
        }
        _ => {
            eprintln!(
                "usage: flare-mac-app <out dir> <bin dir> | check-key | resign <app> | sign-file <file> | verify <app>\n  \
                 [--identity <Developer ID>] [--keychain <path>] [--team <id>] [--identifier <bundle id>] \
                 [--version <x.y.z>]"
            );
            std::process::exit(2);
        }
    }
}

/// `vpk pack`'s `codesign` (rule 1046): its update program is left for the bundle's signing, the bundle is signed
/// again inside out by role and checked (a Developer ID: `verify_release`; ad hoc: `codesign --verify --deep
/// --strict`). Never returns.
#[cfg(target_os = "macos")]
fn vpk_codesign(args: &[String]) -> ! {
    use flare_cef::macos::{bundle, release, sign, vpk};
    use std::path::Path;
    let expected = std::env::var(vpk::PACK_IDENTITY_VAR).unwrap_or_default();
    let call = vpk::parse(args, &expected).unwrap_or_else(|e| {
        eprintln!("flare-mac-app as vpk's codesign: {e}");
        std::process::exit(1);
    });
    match call {
        vpk::VpkSign::UpdateProgram(p) => {
            println!(
                "{}: signed with the bundle, by its role (a tool)",
                p.display()
            );
        }
        vpk::VpkSign::App {
            app,
            identity,
            keychain,
        } => {
            let signing = sign::Signing::release(Some(&identity), keychain.as_deref());
            let name = app.file_stem().and_then(|n| n.to_str()).unwrap_or_default();
            if name != release::APP_NAME {
                eprintln!(
                    "{}: vpk's bundle must be {}.app (--packTitle)",
                    app.display(),
                    release::APP_NAME
                );
                std::process::exit(1);
            }
            let scratch = app
                .parent()
                .unwrap_or(Path::new("."))
                .join(format!("{name}.entitlements"));
            let checked = bundle::sign_bundle(&app, name, release::BUNDLE_ID, &signing, &scratch)
                .and_then(|()| match &signing {
                    sign::Signing::Identity { .. } => {
                        sign::verify_release(&app, sign::TEAM).map(|n| {
                            format!(
                                "{n} Mach-O files, Developer ID Application of team {}",
                                sign::TEAM
                            )
                        })
                    }
                    sign::Signing::AdHoc => bundle::verify(&app).map(|()| "ad hoc".to_string()),
                });
            match checked {
                Ok(what) => println!("{}: signed again by role after vpk ({what})", app.display()),
                Err(e) => {
                    eprintln!("{}: {e}", app.display());
                    std::process::exit(1);
                }
            }
        }
    }
    std::process::exit(0);
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("flare-mac-app is macOS only");
    std::process::exit(1);
}
