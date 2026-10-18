//! M9 Task 28: the release workflow and the release scripts (their shape, read as text).
//!
//! These tests read `.github/workflows/release.yml` and `tools/release/*.ps1` as text. They do not run
//! them: the workflow runs on a Windows runner (Task 37 exercises the install), and the scripts need vpk.

use std::fs;
use std::path::PathBuf;

const WF: &str = ".github/workflows/release.yml";
const PACK: &str = "tools/release/pack.ps1";
const NOTES: &str = "tools/release/notes.ps1";
const VERIFY: &str = "tools/release/verify-pack.ps1";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn at(s: &str, needle: &str) -> usize {
    s.find(needle)
        .unwrap_or_else(|| panic!("missing in the file: {needle:?}"))
}

/// The text of the `jobs:` section.
fn jobs(wf: &str) -> &str {
    wf.split("\njobs:\n").nth(1).expect("a jobs: section")
}

/// The step blocks of the one job, each starting at its `      - ` line. Index 0 is the text before the first step.
fn step_blocks(wf: &str) -> Vec<String> {
    let body = jobs(wf).split("    steps:\n").nth(1).expect("steps:");
    format!("\n{body}")
        .split("\n      - ")
        .map(str::to_string)
        .collect()
}

#[test]
fn release_yml_runs_on_windows_latest_and_on_pushes_to_main() {
    let wf = read(WF);
    assert!(wf.contains("runs-on: windows-latest"));
    assert!(wf.contains("on:\n  push:\n    branches: [main]"));
    assert!(
        !wf.contains("pull_request"),
        "a release never runs on a pull request"
    );
}

#[test]
fn it_has_one_job_and_the_release_concurrency_group_without_cancel() {
    let wf = read(WF);
    let job_names: Vec<&str> = jobs(&wf)
        .lines()
        .filter(|l| l.starts_with("  ") && !l.starts_with("   ") && l.trim_end().ends_with(':'))
        .collect();
    assert_eq!(
        job_names,
        vec!["  release:"],
        "exactly one job, named release"
    );
    assert!(wf.contains("group: release-main"));
    assert!(wf.contains("cancel-in-progress: false"));
    assert!(!wf.contains("cancel-in-progress: true"));
}

#[test]
fn every_step_after_the_version_check_is_guarded_by_fresh() {
    let wf = read(WF);
    let blocks = step_blocks(&wf);
    let ver = blocks
        .iter()
        .position(|b| b.contains("id: ver"))
        .expect("the version check step");
    assert_eq!(ver, 2, "only the checkout runs before the version check");
    for block in &blocks[ver + 1..] {
        let head = block.lines().next().unwrap_or("");
        assert!(
            block.contains("if: steps.ver.outputs.fresh == 'true'"),
            "step not guarded by fresh: {head}"
        );
    }
}

#[test]
fn the_gates_run_before_the_pack() {
    let wf = read(WF);
    let pack = at(&wf, "tools/release/pack.ps1");
    for gate in [
        "cargo fmt --check",
        "cargo clippy",
        "cargo nextest run",
        "cargo deny check",
    ] {
        assert!(at(&wf, gate) < pack, "{gate} must run before the pack");
    }
}

#[test]
fn the_pack_flags_are_the_d8_flags() {
    let pack = read(PACK);
    for flag in [
        "--packId Flare",
        "--packTitle Flare",
        "--packAuthors Max",
        "--mainExe Flare.exe",
        "--icon",
        "--shortcuts StartMenuRoot",
        "--aumid com.flare.terminal",
        "--runtime win-x64",
        "--channel win",
        "--delta BestSpeed",
        "--releaseNotes",
        "--packDir $Stage",
        "--outputDir $Out",
    ] {
        assert!(pack.contains(flag), "pack.ps1 lacks {flag}");
    }
    assert!(
        pack.contains("crates\\flare-agent\\claude-plugin"),
        "the plugin is staged from crates\\flare-agent\\claude-plugin"
    );
    assert!(
        pack.contains("'claude-plugin'"),
        "the plugin lands as claude-plugin"
    );
    assert!(pack.contains("crates\\flare-term\\bundled\\x64"));
    assert!(pack.contains("exit $LASTEXITCODE"));
}

#[test]
fn vpk_is_pinned_to_the_crate_version() {
    let cargo = read("Cargo.toml");
    let line = cargo
        .lines()
        .find(|l| l.starts_with("velopack = "))
        .expect("a velopack line in the workspace");
    let start = line.find("\"=").expect("an exact velopack pin") + 2;
    let end = line[start..].find('"').expect("the pin's closing quote") + start;
    let pin = &line[start..end];
    let wf = read(WF);
    assert!(
        wf.contains(&format!("VPK_VERSION: {pin}")),
        "the workflow pins vpk {pin}"
    );
    assert!(wf.contains("--version $env:VPK_VERSION"));
}

#[test]
fn live_updates_follow_the_repository_variable_and_nothing_else() {
    let wf = read(WF);
    assert_eq!(
        wf.matches("vars.").count(),
        1,
        "one repository variable only"
    );
    assert!(wf.contains("vars.FLARE_LIVE_UPDATES"));
    let build = at(&wf, "      - name: Build\n");
    let notes = at(&wf, "      - name: Release notes\n");
    for (i, _) in wf.match_indices("live-updates") {
        assert!(
            build < i && i < notes,
            "live-updates appears only in the Build step"
        );
    }
}

#[test]
fn live_updates_need_a_public_repository() {
    let wf = read(WF);
    let build = at(&wf, "      - name: Build\n");
    let notes = at(&wf, "      - name: Release notes\n");
    let step = &wf[build..notes];
    assert!(step.contains("gh repo view"));
    assert!(step.contains("--json visibility"));
    assert!(step.contains("$vis -ne 'PUBLIC'"));
    // Review fix: a line scan of `default = [...]` missed a multi-line list and a dependency line
    // that turns the feature on (`flare-update = { .., features = ["live-updates"] }`). The step
    // asks Cargo's own feature resolution instead.
    assert!(step.contains("cargo metadata --format-version 1"));
    assert!(step.contains("$_.features -contains 'live-updates'"));
    assert!(
        !step.contains("Select-String"),
        "no line scan of the manifests"
    );
    let throw = step
        .find("throw \"the updater is switched on")
        .expect("the refusal throws");
    let cargo_build = step.find("cargo build --release").expect("the build line");
    assert!(throw < cargo_build, "the refusal comes before cargo build");
}

#[test]
fn the_release_goes_to_this_repository_only() {
    let wf = read(WF);
    let prefix = "https://github.com/${{ github.repository }}";
    for (i, _) in wf.match_indices("--repoUrl ") {
        let after = wf[i + "--repoUrl ".len()..].trim_start_matches('"');
        assert!(
            after.starts_with(prefix),
            "--repoUrl must be this repository"
        );
    }
    for (i, _) in wf.match_indices("github.com/") {
        assert!(
            wf[i..].starts_with("github.com/${{ github.repository }}"),
            "every github.com link names this repository"
        );
    }
    for other in ["Maxaubert", "loftspace", "arkivert", "PrismTerminal"] {
        assert!(
            !wf.contains(other),
            "no other repository in the workflow: {other}"
        );
    }
    assert_eq!(wf.matches("vpk download github").count(), 1);
    assert_eq!(wf.matches("vpk upload github").count(), 1);
}

#[test]
fn no_secret_other_than_the_github_token() {
    let wf = read(WF);
    assert!(!wf.contains("secrets."), "no repository secret");
    let allowed = [
        "github.token",
        "github.repository",
        "github.sha",
        "steps.ver.outputs.version",
        "vars.FLARE_LIVE_UPDATES",
    ];
    for expr in wf.split("${{").skip(1) {
        let inner = expr.split("}}").next().unwrap_or("").trim();
        assert!(allowed.contains(&inner), "unexpected expression: {inner}");
    }
}

#[test]
fn actions_are_pinned_to_major_tags_that_exist() {
    let wf = read(WF);
    let allowed = [
        "actions/checkout@v4",
        "actions/setup-dotnet@v4",
        "Swatinem/rust-cache@v2",
        "taiki-e/install-action@v2",
        "actions/upload-artifact@v4",
    ];
    let used: Vec<&str> = wf
        .lines()
        .filter_map(|l| {
            let t = l.trim().trim_start_matches("- ");
            t.strip_prefix("uses: ")
        })
        .collect();
    assert_eq!(used.len(), allowed.len(), "every action use is listed");
    for u in used {
        assert!(allowed.contains(&u), "unexpected action: {u}");
    }
}

#[test]
fn no_em_dash_in_any_release_file() {
    let dash = char::from_u32(0x2014).expect("the em dash");
    for rel in [WF, PACK, NOTES, VERIFY] {
        assert!(!read(rel).contains(dash), "em dash in {rel}");
    }
}
