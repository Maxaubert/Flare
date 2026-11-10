//! The Mac release's workflow and scripts, read as text (port phase 8, rules 1046, 1048 to 1050 and 1052). A pure file
//! reader that runs on every OS: it starts nothing.

use std::path::PathBuf;

/// The file with LF line ends: a Windows checkout (core.autocrlf) holds the workflow and scripts with CRLF, and the
/// checks below look for whole lines.
fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .replace("\r\n", "\n")
}

const WF: &str = ".github/workflows/macos-app.yml";
const SECRETS: [&str; 5] = [
    "MACOS_CERT_P12_BASE64",
    "MACOS_CERT_PASSWORD",
    "APPLE_API_KEY_P8_BASE64",
    "APPLE_API_KEY_ID",
    "APPLE_API_ISSUER_ID",
];

#[test]
fn the_mac_release_runs_after_the_windows_one_never_inside_it() {
    let wf = read(WF);
    assert!(wf.contains(
        "  workflow_run:\n    workflows: [release]\n    types: [completed]\n    branches: [main]\n"
    ));
    assert!(wf.contains("github.event.workflow_run.conclusion == 'success'"));
    assert!(wf.contains("ref: ${{ github.event.workflow_run.head_sha || github.sha }}"));
    assert!(
        wf.contains("runs-on: macos-26\n"),
        "a pinned runner, its SDK written down"
    );
    assert!(
        !wf.contains("pull_request"),
        "the signing job never runs for a pull request"
    );
    let windows = read(".github/workflows/release.yml");
    for word in ["macos", "osx", "tools/release/mac", "MACOS_CERT"] {
        assert!(!windows.contains(word), "release.yml mentions {word}");
    }
}

#[test]
fn credentials_come_from_the_five_secrets_only_and_without_them_nothing_is_published() {
    let wf = read(WF);
    for (i, _) in wf.match_indices("secrets.") {
        let name: String = wf[i + "secrets.".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            continue; // prose: "the secrets."
        }
        assert!(SECRETS.contains(&name.as_str()), "secret {name}");
    }
    // A run after a release builds only a version that has no Mac build yet, and only with the secrets.
    let plan = &wf[wf.find("- name: What to build").unwrap()..wf.find("\n  app:\n").unwrap()];
    let has_osx = plan
        .find("grep -qx 'releases.osx.json'")
        .expect("an existing Mac build is checked");
    let go = plan
        .find("build=true publish=true")
        .expect("the publishing branch");
    assert!(has_osx < go);
    let gate = &wf
        [wf.find("- name: Signing, and whether").unwrap()..wf.find("- name: Toolchain").unwrap()];
    assert!(
        gate.contains("if [ \"$HAS_CERT\" != true ]"),
        "the secrets are checked"
    );
    assert!(gate.contains("go=false"));
    let app = &wf[wf.find("\n  app:\n").unwrap()..];
    for step in app.split("\n      - ").skip(1) {
        if step.starts_with("uses: actions/checkout")
            || step.starts_with("name: Signing, and whether")
            || step.contains("if: always()")
        {
            continue;
        }
        assert!(
            step.contains("if: steps.gate.outputs.go == 'true'"),
            "a step runs past the gate: {}",
            step.lines().next().unwrap_or_default()
        );
    }
    assert_eq!(
        wf.matches("publish == 'true'").count(),
        1,
        "one publishing step"
    );
    let publish = &wf[wf.find("- name: Add the Mac build to the release").unwrap()..];
    assert!(publish.contains("--channel osx --merge"));
    // The keychain goes whatever happened.
    let clean = &wf[wf.find("- name: Remove the signing keychain").unwrap()..];
    assert!(clean.contains("if: always()"));
    assert!(clean.contains("security delete-keychain"));
}

#[test]
fn vpk_is_pinned_to_the_velopack_crate_on_both_oses() {
    let cargo = read("Cargo.toml");
    let line = cargo
        .lines()
        .find(|l| l.starts_with("velopack = "))
        .expect("a velopack pin");
    let pin = line.split('"').nth(1).unwrap().trim_start_matches('=');
    assert!(read(WF).contains(&format!("VPK_VERSION: {pin}")));
    assert!(read(".github/workflows/release.yml").contains(&format!("VPK_VERSION: {pin}")));
    assert!(read(WF).contains("dotnet tool install -g vpk --version \"$VPK_VERSION\"\n"));
}

#[test]
fn the_pack_signs_by_role_through_flare_mac_app_and_checks_inside_the_packages() {
    let pack = read("tools/release/mac/pack.sh");
    assert!(pack.contains("ln -s \"$BIN/flare-mac-app\" \"$WORK/shim/codesign\""));
    assert!(pack.contains("PATH=\"$WORK/shim:$PATH\" \"$VPK\""));
    assert!(pack.contains("--signDisableDeep"));
    assert!(
        !pack.contains("--signEntitlements"),
        "each program's keys are its role's"
    );
    assert!(pack.contains("--noInst"), "the installer is the DMG");
    assert!(pack.contains("check_bundle \"$WORK/portable/Flare.app\""));
    assert!(pack.contains("check_bundle \"$WORK/nupkg/Flare.app\""));
    for needed in [
        "codesign --verify --deep --strict",
        "flare-mac-app\" verify",
        "stapler validate",
        "source=Notarized Developer ID",
    ] {
        assert!(pack.contains(needed), "{needed}");
    }
    // No script signs with an identity itself (rule 1040): every Developer ID codesign goes through flare-mac-app.
    for script in [
        "pack.sh",
        "make-app.sh",
        "lib.sh",
        "update-e2e.sh",
        "ci-keychain.sh",
    ] {
        let text = read(&format!("tools/release/mac/{script}"));
        for bad in [
            "codesign -s",
            "codesign --sign",
            "codesign -f",
            "codesign --force",
        ] {
            assert!(!text.contains(bad), "{script}: {bad}");
        }
    }
}

#[test]
fn the_update_test_uses_throwaway_versions_and_never_opens_the_app() {
    let e2e = read("tools/release/mac/update-e2e.sh");
    assert!(e2e.contains("for v in 0.0.1 0.0.2; do"));
    let test = read("crates/flare-update/tests/mac_update.rs");
    assert!(test.contains(".without_restart()"));
    for text in [&e2e, &test] {
        assert!(!text.contains("/usr/bin/open"));
        assert!(!text.contains("Command::new(\"open\")"));
        assert!(!text.contains("\nopen "));
        assert!(!text.contains("\"/Applications\""));
    }
}

#[test]
fn the_mac_build_is_the_commit_the_release_was_tagged_at_one_run_per_version() {
    let wf = read(WF);
    // Not the commit that ended the release run: a merge without a version bump ends it with success too.
    assert!(wf.contains("sha=\"$(gh api \"repos/$REPO/commits/v$v\" --jq .sha"));
    let app = &wf[wf.find("\n  app:\n").unwrap()..];
    assert!(app.contains("ref: ${{ needs.plan.outputs.sha }}"));
    assert!(!app.contains("workflow_run.head_sha"));
    assert!(app.contains("group: macos-app-${{ needs.plan.outputs.version }}"));
    assert!(app.contains("cancel-in-progress: false"));
    assert!(
        !wf[..wf.find("jobs:").unwrap()].contains("concurrency:"),
        "no run-wide group keyed by commit"
    );
    // The checked-out commit is the version, and the release is read again before the upload.
    assert!(app.contains("The commit checked out is version"));
    let publish = &wf[wf.find("- name: Add the Mac build to the release").unwrap()..];
    let again = publish
        .find("grep -qx 'releases.osx.json'")
        .expect("read again");
    assert!(again < publish.find("vpk upload github").unwrap());
}

#[test]
fn signing_secrets_reach_a_run_from_main_only() {
    let wf = read(WF);
    let app = &wf[wf.find("\n  app:\n").unwrap()..];
    assert!(app.contains("environment: ${{ needs.plan.outputs.environment }}"));
    let plan = &wf[wf.find("- name: What to build").unwrap()..wf.find("\n  app:\n").unwrap()];
    assert!(plan.contains("[ \"$REF\" = refs/heads/main ] && environment=mac-release"));
    assert!(!plan.contains("secrets."), "the plan job sees no secret");
    let gate = &app
        [app.find("- name: Signing, and whether").unwrap()..app.find("- name: Toolchain").unwrap()];
    assert!(gate.contains(
        "if [ \"$EVENT\" != workflow_run ] && [ \"$REF\" != refs/heads/main ]; then signed=false; fi"
    ));
    let kc = &app[app.find("- name: Signing keychain").unwrap()..];
    assert!(kc.contains("steps.gate.outputs.signed == 'true'"));
}

#[test]
fn no_key_file_or_password_outlives_its_one_use() {
    let wf = read(WF);
    // The keychain is made after the build and removed before any later action (artifact, upload).
    let build = wf.find("- name: Build the programs").unwrap();
    let kc = wf.find("- name: Signing keychain").unwrap();
    let rm = wf.find("- name: Remove the signing keychain").unwrap();
    assert!(build < kc && kc < rm);
    assert!(rm < wf.find("- name: Keep the packages").unwrap());
    assert!(wf.contains("FLARE_BUILD_ONLY=1 bash tools/release/mac/make-app.sh"));
    let ci = read("tools/release/mac/ci-keychain.sh");
    assert!(ci.contains("trap 'rm -f \"$P12\" \"$P8\"' EXIT"));
    let stored = ci.find("notarytool store-credentials").unwrap();
    assert!(
        ci[stored..].contains("\nrm -f \"$P8\"\n"),
        "the .p8 goes after its one use"
    );
    for gone in ["FLARE_NOTARY_KEY", "FLARE_NOTARY_ISSUER", "-lut 21600"] {
        assert!(!ci.contains(gone), "{gone}");
    }
    assert!(ci.contains("set-keychain-settings -l -u -t 9000"));
    assert!(
        wf.contains("timeout-minutes: 150"),
        "the keychain's lock time is the job's limit"
    );
    // No password on a command line.
    let lib = read("tools/release/mac/lib.sh");
    assert!(!lib.contains("--password"));
    assert!(!lib.contains("--apple-id"));
    // A private key leaves Max's Mac for good, and never enters git.
    let doc = read("docs/releasing-macos.md");
    assert!(!doc.contains("move the `.p12` to the Bin"));
    assert!(doc.contains("rm -P"));
    let ignore = read(".gitignore");
    for pat in ["*.p12", "*.p8", "AuthKey_*"] {
        assert!(ignore.lines().any(|l| l.trim() == pat), "{pat}");
    }
}

#[test]
fn the_scripts_remove_only_what_they_made_and_never_lose_each_others_cleanup() {
    let lib = read("tools/release/mac/lib.sh");
    assert_eq!(
        lib.matches("\ntrap ").count(),
        1,
        "one EXIT trap, the list's"
    );
    assert!(lib.contains("trap run_cleanup EXIT"));
    let dmg = &lib[lib.find("make_dmg() {").unwrap()..];
    assert!(dmg.contains("on_exit \"hdiutil detach -quiet $(printf %q \"$mnt\")"));
    let pack = read("tools/release/mac/pack.sh");
    assert!(!pack.contains("\ntrap "), "pack.sh adds to lib.sh's list");
    assert!(pack.contains("on_exit \"rm -rf -- $(printf %q \"$WORK\")\""));
    let e2e = read("tools/release/mac/update-e2e.sh");
    let guard = e2e
        .find("WORK=\"$(cd \"$WORK\" && pwd -P)\"")
        .expect("resolved first");
    assert!(guard < e2e.find("rm -rf -- \"$WORK\"").unwrap());
    assert!(e2e.contains("*/../*|*/./*) refuse"));
    assert!(e2e.contains("\"$target\"/mac-update-e2e*)"));
    assert!(e2e.contains("\"$base\"/?*) ok=1"));
}

#[test]
fn the_pack_checks_each_programs_role_ad_hoc_too() {
    let pack = read("tools/release/mac/pack.sh");
    let check = &pack[pack.find("check_bundle() {").unwrap()..];
    for needed in [
        "for t in UpdateMac flare-mcp; do",
        "Identifier=$BUNDLE_ID.$t",
        "flags=[^ ]*runtime",
        "carries entitlements",
        "device.audio-input device.camera automation.apple-events",
    ] {
        assert!(check.contains(needed), "{needed}");
    }
    let ad_hoc_too = check.find("for t in UpdateMac").unwrap();
    assert!(ad_hoc_too < check.find("if [ \"$IDENTITY\" != - ]; then").unwrap());
}
