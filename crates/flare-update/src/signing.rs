//! Who signed a staged build (2026-11-20, the agent's install): the answer to "is it Developer ID signed".
//!
//! macOS: `codesign --verify --deep --strict` on the staged `Flare.app` must pass, then `codesign -dv --verbose=2`
//! names the signer: a first `Authority=Developer ID Application: ...` line is Developer ID, `Signature=adhoc` is ad
//! hoc, another authority (Apple Development) is `other`. Windows has no Developer ID: a stage there is `not-checked`.
//! The parse is pure; [`inspect_bundle`] runs the two commands (a worker's job: about a quarter of a second).

use std::path::Path;

/// What signs a staged build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signing {
    /// Developer ID Application: the signer's line and its team.
    DeveloperId {
        authority: String,
        team: Option<String>,
    },
    /// Ad hoc: runs on this Mac only.
    AdHoc,
    /// Signed by another authority (an Apple Development certificate, say).
    Other { authority: String },
    /// No signature at all.
    Unsigned,
    /// The signature does not verify (a file changed after signing): why, in `codesign`'s words.
    Invalid { why: String },
    /// Not looked at: Windows, or `codesign` could not run.
    NotChecked { why: String },
}

impl Signing {
    /// The word on the wire (`signed` in `install.staged`'s answer).
    pub fn word(&self) -> &'static str {
        match self {
            Signing::DeveloperId { .. } => "developer-id",
            Signing::AdHoc => "ad-hoc",
            Signing::Other { .. } => "other",
            Signing::Unsigned => "unsigned",
            Signing::Invalid { .. } => "invalid",
            Signing::NotChecked { .. } => "not-checked",
        }
    }

    pub fn developer_id(&self) -> bool {
        matches!(self, Signing::DeveloperId { .. })
    }

    /// One line for a person: "Developer ID (WV7BY8CUM8)", "ad hoc", ...
    pub fn describe(&self) -> String {
        match self {
            Signing::DeveloperId { team: Some(t), .. } => format!("Developer ID ({t})"),
            Signing::DeveloperId { team: None, .. } => "Developer ID".to_string(),
            Signing::AdHoc => "ad hoc".to_string(),
            Signing::Other { authority } => format!("signed by {authority}, not Developer ID"),
            Signing::Unsigned => "not signed".to_string(),
            Signing::Invalid { why } => format!("a signature that does not verify ({why})"),
            Signing::NotChecked { why } => format!("not checked ({why})"),
        }
    }
}

/// Pure: what `codesign -dv --verbose=2` printed (on stderr) says, for a bundle whose `--verify` passed.
pub fn parse_display(out: &str) -> Signing {
    let field = |key: &str| {
        out.lines()
            .find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim().to_string()))
    };
    if out.contains("code object is not signed at all") {
        return Signing::Unsigned;
    }
    // The first authority is the leaf: the one that signed.
    if let Some(authority) = field("Authority=") {
        if authority.starts_with("Developer ID Application:") {
            let team = field("TeamIdentifier=").filter(|t| !t.is_empty() && t != "not set");
            return Signing::DeveloperId { authority, team };
        }
        return Signing::Other { authority };
    }
    if field("Signature=").as_deref() == Some("adhoc") || out.contains("flags=0x2(adhoc)") {
        return Signing::AdHoc;
    }
    Signing::Unsigned
}

/// Pure: a failed `codesign --verify` (its status was not 0) with what it printed.
pub fn parse_verify_failure(out: &str) -> Signing {
    if out.contains("code object is not signed at all") {
        return Signing::Unsigned;
    }
    let why = out
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("codesign --verify failed");
    let why: String = why.chars().take(200).collect();
    Signing::Invalid { why }
}

/// Runs `codesign` on `app` (macOS). Elsewhere, or when `codesign` cannot run, [`Signing::NotChecked`].
pub fn inspect_bundle(app: &Path) -> Signing {
    if !cfg!(target_os = "macos") {
        return Signing::NotChecked {
            why: "not a macOS build".to_string(),
        };
    }
    let run = |args: &[&str]| {
        std::process::Command::new("/usr/bin/codesign")
            .args(args)
            .arg(app)
            .output() // sync-ok: the control channel's call thread, never the UI thread; a fraction of a second
    };
    let verify = match run(&["--verify", "--deep", "--strict"]) {
        Ok(o) => o,
        Err(e) => {
            return Signing::NotChecked {
                why: format!("codesign did not run: {e}"),
            };
        }
    };
    if !verify.status.success() {
        return parse_verify_failure(&String::from_utf8_lossy(&verify.stderr));
    }
    match run(&["-dv", "--verbose=2"]) {
        Ok(o) => parse_display(&String::from_utf8_lossy(&o.stderr)),
        Err(e) => Signing::NotChecked {
            why: format!("codesign did not run: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `codesign -dv --verbose=2` printed for a staged Developer ID build on Max's Mac (2026-11-20).
    const DEVELOPER_ID: &str = "Executable=/x/staged/Flare.app/Contents/MacOS/Flare
Identifier=com.flare.terminal
Format=app bundle with Mach-O thin (arm64)
CodeDirectory v=20500 size=138571 flags=0x10000(runtime) hashes=4320+7 location=embedded
Signature size=9041
Authority=Developer ID Application: MAX AUBERT (WV7BY8CUM8)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=20 Nov 2026 at 08:38:41
TeamIdentifier=WV7BY8CUM8
Sealed Resources version=2 rules=13 files=28
Internal requirements count=1 size=180
";

    /// The installed ad hoc copy on the same Mac.
    const AD_HOC: &str = "Executable=/Applications/Flare.app/Contents/MacOS/Flare
Identifier=com.flare.terminal
CodeDirectory v=20400 size=138571 flags=0x2(adhoc) hashes=4320+7 location=embedded
Signature=adhoc
Info.plist entries=22
TeamIdentifier=not set
";

    #[test]
    fn a_developer_id_build_names_its_signer_and_team() {
        let s = parse_display(DEVELOPER_ID);
        assert_eq!(
            s,
            Signing::DeveloperId {
                authority: "Developer ID Application: MAX AUBERT (WV7BY8CUM8)".into(),
                team: Some("WV7BY8CUM8".into()),
            }
        );
        assert!(s.developer_id());
        assert_eq!(s.word(), "developer-id");
        assert_eq!(s.describe(), "Developer ID (WV7BY8CUM8)");
    }

    #[test]
    fn ad_hoc_and_other_signers_are_not_developer_id() {
        let s = parse_display(AD_HOC);
        assert_eq!(s, Signing::AdHoc);
        assert!(!s.developer_id());
        assert_eq!(s.word(), "ad-hoc");
        let dev = DEVELOPER_ID.replace(
            "Developer ID Application: MAX AUBERT",
            "Apple Development: MAX AUBERT",
        );
        let s = parse_display(&dev);
        assert!(matches!(s, Signing::Other { .. }), "{s:?}");
        assert!(!s.developer_id());
        // Only the leaf counts: a chain that mentions Developer ID further up is not Developer ID.
        assert_eq!(
            parse_display("Authority=Someone Else\nAuthority=Developer ID Application: X (T)\n")
                .word(),
            "other"
        );
    }

    #[test]
    fn an_unsigned_or_broken_bundle_says_so() {
        let msg = "/x/Flare.app: code object is not signed at all\n";
        assert_eq!(parse_display(msg), Signing::Unsigned);
        assert_eq!(parse_verify_failure(msg), Signing::Unsigned);
        let s = parse_verify_failure(
            "/x/Flare.app: a sealed resource is missing or invalid\nfile modified: /x/a\n",
        );
        assert_eq!(s.word(), "invalid");
        assert!(s.describe().contains("a sealed resource is missing"));
        assert_eq!(parse_verify_failure("").word(), "invalid");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn windows_is_not_checked() {
        assert_eq!(inspect_bundle(Path::new("C:\\x")).word(), "not-checked");
    }

    /// The real `codesign` on a bundle that is not there: invalid or not checked, never Developer ID.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_missing_bundle_is_never_developer_id() {
        let dir = tempfile::tempdir().unwrap();
        let s = inspect_bundle(&dir.path().join("Flare.app"));
        assert!(!s.developer_id(), "{s:?}");
    }
}
