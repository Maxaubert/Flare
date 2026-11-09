//! The `codesign` that `vpk pack` runs (port phase 8, rules 270 and 273).
//!
//! `vpk pack` for osx copies its update program (`UpdateMac`) into `Contents/MacOS`, writes `sq.version` into
//! `Contents/Resources` (with a link to it in `Contents/MacOS`), then signs, notarizes and staples, and only then zips
//! the bundle into the update package and the portable zip. Its own signing gives `UpdateMac` Velopack's broad
//! entitlements (`allow-unsigned-executable-memory`, `disable-library-validation`, the dyld key), which rule 263
//! forbids, and signs the app with one entitlements file for every program. So `tools/release/mac/pack.sh` puts a
//! link named `codesign` to `flare-mac-app` first on vpk's `PATH`, and vpk's two calls land here, at exactly the
//! moment between "vpk added its files" and "vpk zips":
//!
//! 1. `codesign ... <tmp>/Flare.app/Contents/MacOS/UpdateMac`: nothing yet; the bundle's own signing signs it next.
//! 2. `codesign ... <tmp>/Flare.app`: the whole bundle signed again inside out by role (`bundle::sign_bundle`, rule
//!    265: `UpdateMac` is a tool with no entitlements), then checked.
//!
//! Only the arguments vpk 1.2.161 passes with `--signDisableDeep` are accepted (`-s`, `-f`, `-v`, `--timestamp`,
//! `--options runtime`, `--entitlements`, `--keychain`, one path last); `--deep` or anything else is refused, and
//! so is an identity other than the one the script set, so a changed vpk fails loudly instead of signing wrongly.
//! vpk's entitlements file is read by nobody: each program's keys are its role's.

use std::path::{Path, PathBuf};

/// The update program `vpk pack` adds to `Contents/MacOS`.
pub const UPDATE_PROGRAM: &str = "UpdateMac";
/// The name `flare-mac-app` answers to as vpk's `codesign` (its `argv[0]`'s file name).
pub const SHIM_NAME: &str = "codesign";
/// The identity `pack.sh` gives vpk, for the shim to compare with what vpk passes (`-` or unset: ad hoc).
pub const PACK_IDENTITY_VAR: &str = crate::macos::sign::RELEASE_IDENTITY_VAR;

/// One `codesign` call of `vpk pack`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VpkSign {
    /// vpk's update program inside the bundle: signed with the bundle in the next call.
    UpdateProgram(PathBuf),
    /// The bundle: sign it again inside out, by role.
    App {
        app: PathBuf,
        identity: String,
        keychain: Option<String>,
    },
}

/// Whether `argv0` is the shim's name (the link `pack.sh` makes).
pub fn is_shim(argv0: &str) -> bool {
    Path::new(argv0).file_name().is_some_and(|n| n == SHIM_NAME)
}

fn is_update_program(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n == UPDATE_PROGRAM)
        && p.parent().is_some_and(|d| d.ends_with("Contents/MacOS"))
        && p.parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(|a| a.extension())
            .is_some_and(|x| x == "app")
}

/// Reads one vpk `codesign` command line (without `argv[0]`). `expected` is the identity the script gave vpk.
pub fn parse(args: &[String], expected: &str) -> Result<VpkSign, String> {
    let mut identity = None;
    let mut keychain = None;
    let mut path = None;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} without a value"))
        };
        match a.as_str() {
            "-s" => identity = Some(value("-s")?),
            "-f" | "-v" | "--timestamp" => {}
            "--options" => {
                let v = value("--options")?;
                if v != "runtime" {
                    return Err(format!("--options {v}: vpk passes runtime only"));
                }
            }
            "--entitlements" => {
                value("--entitlements")?;
            }
            "--keychain" => keychain = Some(value("--keychain")?),
            "--deep" => {
                return Err(
                    "--deep: pack with --signDisableDeep, each program is signed by its role"
                        .into(),
                );
            }
            other if other.starts_with('-') => {
                return Err(format!("{other}: not an argument vpk passes"));
            }
            other => {
                if path.is_some() {
                    return Err(format!("{other}: one path only"));
                }
                if it.peek().is_some() {
                    return Err(format!("{other}: the path comes last"));
                }
                path = Some(PathBuf::from(other));
            }
        }
    }
    let identity = identity.ok_or("no -s identity")?;
    let path = path.ok_or("no path to sign")?;
    let norm = |s: &str| match s.trim() {
        "" => "-".to_string(),
        t => t.to_string(),
    };
    if norm(&identity) != norm(expected) {
        return Err(format!(
            "vpk asked for identity {identity:?}, the script set {:?}",
            norm(expected)
        ));
    }
    if is_update_program(&path) {
        Ok(VpkSign::UpdateProgram(path))
    } else if path.extension().is_some_and(|x| x == "app") {
        Ok(VpkSign::App {
            app: path,
            identity,
            keychain,
        })
    } else {
        Err(format!(
            "{}: neither the bundle nor its {UPDATE_PROGRAM}",
            path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }

    const ENT: &str = "--entitlements /x/vendor/Velopack.entitlements";

    #[test]
    fn vpks_two_calls_are_its_update_program_then_the_bundle() {
        // The exact lines vpk 1.2.161 ran with --signDisableDeep (logged 2026-11-09).
        let up = v(&format!(
            "-s - -f -v --timestamp --options runtime {ENT} /t/velopack/temp.1/Flare.app/Contents/MacOS/UpdateMac"
        ));
        assert_eq!(
            parse(&up, "-").unwrap(),
            VpkSign::UpdateProgram("/t/velopack/temp.1/Flare.app/Contents/MacOS/UpdateMac".into())
        );
        let app = v(&format!(
            "-s - -f -v --timestamp --options runtime {ENT} /t/velopack/temp.1/Flare.app"
        ));
        assert_eq!(
            parse(&app, "").unwrap(),
            VpkSign::App {
                app: "/t/velopack/temp.1/Flare.app".into(),
                identity: "-".into(),
                keychain: None
            }
        );
        let id = "Developer ID Application: X (WV7BY8CUM8)";
        let mut kc = vec!["-s".to_string(), id.to_string()];
        kc.extend(v(&format!(
            "-f -v --timestamp --options runtime {ENT} --keychain /k/ci.keychain-db /t/Flare.app"
        )));
        assert_eq!(
            parse(&kc, id).unwrap(),
            VpkSign::App {
                app: "/t/Flare.app".into(),
                identity: id.into(),
                keychain: Some("/k/ci.keychain-db".into())
            }
        );
    }

    #[test]
    fn anything_vpk_does_not_pass_with_disable_deep_is_refused() {
        for bad in [
            format!("-s - -f -v --timestamp --options runtime {ENT} --deep /t/Flare.app"),
            format!("-s - --options library {ENT} /t/Flare.app"),
            format!("-s - --preserve-metadata=entitlements {ENT} /t/Flare.app"),
            format!("-s - {ENT} /t/Flare.app /t/Other.app"),
            format!("-s - /t/Flare.app {ENT}"),
            format!("{ENT} /t/Flare.app"),
            "-s - -f".to_string(),
            "-s - /t/Flare.app/Contents/MacOS/Flare".to_string(),
            "-s - /t/Flare.app/Contents/Frameworks/UpdateMac".to_string(),
            "-s - /t/dir/Contents/MacOS/UpdateMac".to_string(),
            "-s".to_string(),
        ] {
            assert!(parse(&v(&bad), "-").is_err(), "{bad}");
        }
    }

    #[test]
    fn an_identity_other_than_the_scripts_is_refused() {
        let line = v(&format!(
            "-s - -f -v --timestamp --options runtime {ENT} /t/Flare.app"
        ));
        assert!(parse(&line, "Developer ID Application: X (WV7BY8CUM8)").is_err());
        let mut other = vec!["-s".to_string(), "Apple Development: X".to_string()];
        other.extend(v("-f /t/Flare.app"));
        assert!(parse(&other, "Developer ID Application: X (WV7BY8CUM8)").is_err());
        assert!(parse(&other, "-").is_err());
    }

    #[test]
    fn the_shim_answers_to_codesign_only() {
        assert!(is_shim("/o/vpk-shim/codesign"));
        assert!(is_shim("codesign"));
        assert!(!is_shim("/r/target/release/flare-mac-app"));
        assert!(!is_shim("/usr/bin/codesign2"));
    }
}
