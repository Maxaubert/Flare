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
    fs::read_to_string(root().join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
        // A Windows checkout may hold CRLF (core.autocrlf); the tests split on "\n".
        .replace("\r\n", "\n")
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
    // The one other repository named is the one the side-by-side test reads, in its missing-token error.
    let rest = wf.replace(NEIGHBOUR_TOKEN_ERROR, "");
    for other in ["Maxaubert", "loftspace", "arkivert", "PrismTerminal"] {
        assert!(
            !rest.contains(other),
            "no other repository in the workflow: {other}"
        );
    }
    assert_eq!(wf.matches("vpk download github").count(), 1);
    assert_eq!(wf.matches("vpk upload github").count(), 1);
}

#[test]
fn no_secret_other_than_the_github_token_and_the_neighbour_release_token() {
    let wf = read(WF);
    assert_eq!(
        wf.matches("secrets.").count(),
        1,
        "one repository secret: the neighbour's release token"
    );
    let allowed = [
        "github.token",
        "github.repository",
        "github.sha",
        "steps.ver.outputs.version",
        "vars.FLARE_LIVE_UPDATES",
        NEIGHBOUR_TOKEN_SECRET,
    ];
    for expr in wf.split("${{").skip(1) {
        let inner = expr.split("}}").next().unwrap_or("").trim();
        assert!(allowed.contains(&inner), "unexpected expression: {inner}");
    }
}

// ---- 2026-11-11: the neighbour's installer sits in a private repository (regression rule 1141) ----

/// The secret the side-by-side step reads the neighbour's private releases with.
const NEIGHBOUR_TOKEN_SECRET: &str = "secrets.FLARE_RELEASE_TOKEN";

/// The one line a run prints when that secret is missing.
const NEIGHBOUR_TOKEN_ERROR: &str = "::error::FLARE_RELEASE_TOKEN is not set: add a fine-grained read-only token for arkivert/PrismTerminal as a repository secret (docs/shipping.md)";

/// The steps of a workflow, each from its `- ` line to the next one at the same indent.
fn steps(wf: &str) -> Vec<&str> {
    wf.split("\n      - ").skip(1).collect()
}

#[test]
fn only_the_side_by_side_step_gets_the_neighbour_token_and_it_fails_without_it() {
    for path in [WF, ".github/workflows/installer-e2e.yml"] {
        let wf = read(path).replace("\r\n", "\n");
        let with_secret: Vec<&str> = steps(&wf)
            .into_iter()
            .filter(|s| s.contains("secrets."))
            .collect();
        assert_eq!(with_secret.len(), 1, "{path}: one step reads a secret");
        let step = with_secret[0];
        assert!(
            step.contains(&format!("GH_TOKEN: ${{{{ {NEIGHBOUR_TOKEN_SECRET} }}}}")),
            "{path}: the step's gh reads the neighbour token"
        );
        assert_eq!(
            step.matches("secrets.").count(),
            1,
            "{path}: and nothing else"
        );
        let test = step
            .find("cargo test -p flare-e2e --test side_by_side")
            .unwrap_or_else(|| panic!("{path}: the secret goes to the side-by-side step"));
        // A missing secret fails the step with one clear line, before the test runs; never a skip.
        let check = step
            .find("if (-not $env:GH_TOKEN)")
            .unwrap_or_else(|| panic!("{path}: the step checks the token"));
        let error = step
            .find(NEIGHBOUR_TOKEN_ERROR)
            .unwrap_or_else(|| panic!("{path}: the step names the missing secret"));
        let exit = step[error..]
            .find("exit 1")
            .map(|i| error + i)
            .unwrap_or_else(|| panic!("{path}: the step fails"));
        assert!(check < error && error < exit && exit < test, "{path}");
        assert!(
            !step.contains("continue-on-error"),
            "{path}: a failure counts"
        );
        // The token is never printed: the only use of it in the script is the empty check.
        assert_eq!(
            step.matches("GH_TOKEN").count(),
            2,
            "{path}: GH_TOKEN is set and checked, never echoed"
        );
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
    let used: Vec<String> = wf
        .lines()
        .filter_map(|l| {
            let t = l.trim().trim_start_matches("- ");
            // `owner/name@<40-hex sha> # vN` (A4 L61): the allow-list names the action and its major.
            let spec = t.strip_prefix("uses: ")?;
            let (action, rest) = spec.split_once('@')?;
            let major = rest.split_once("# ")?.1.trim();
            Some(format!("{action}@{major}"))
        })
        .collect();
    assert_eq!(used.len(), allowed.len(), "every action use is listed");
    for u in used {
        assert!(allowed.contains(&&*u), "unexpected action: {u}");
    }
}

#[test]
fn no_em_dash_in_any_release_file() {
    let dash = char::from_u32(0x2014).expect("the em dash");
    for rel in [WF, PACK, NOTES, VERIFY] {
        assert!(!read(rel).contains(dash), "em dash in {rel}");
    }
}

// M8 Task 33: the speech engine and the Parakeet host ship beside Flare.exe.

const FETCH: &str = "tools/fetch-whisper.ps1";
const LICENSE_WHISPER: &str = "LICENSE-whisper.cpp.txt";
const README: &str = "README.md";
const NOTICES: &str = "THIRD-PARTY-NOTICES.md";

/// The quoted items of a PowerShell array `$name = @( ... )`, in order. The arrays hold no parentheses.
fn ps_array(src: &str, name: &str) -> Vec<String> {
    let head = format!("${name} = @(");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("no ${name} array"))
        + head.len();
    let end = start
        + src[start..]
            .find(')')
            .expect("the array's closing parenthesis");
    src[start..end]
        .split([',', '\n'])
        .map(|s| s.trim().trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[test]
fn the_dictation_seam_ships_the_engine_folder_and_the_parakeet_host() {
    let pack = read(PACK);
    assert!(
        pack.contains("Source = 'vendor/whisper'; Name = 'whisper'"),
        "the engine folder is packed beside the app as whisper\\"
    );
    assert!(
        pack.contains("flare-parakeet-host.exe"),
        "the Parakeet host is packed beside Flare.exe"
    );
    assert!(!pack.contains("SEAM M8"), "the M8 seam is filled in");
}

#[test]
fn the_package_ships_the_mcp_server_beside_the_app_and_every_packing_workflow_builds_it() {
    // Surfaces stage 2: the app finds `flare-mcp.exe` next to itself and gives every shell its path
    // (`FLARE_MCP_EXE`); the bundled plugin names it. A package without it would give Claude a server
    // that cannot start.
    let pack = read(PACK);
    assert!(
        pack.contains("'flare-mcp.exe'"),
        "pack.ps1 copies flare-mcp.exe"
    );
    assert!(
        read(VERIFY).contains("current/flare-mcp.exe"),
        "verify-pack.ps1 requires it in the portable zip"
    );
    for wf in [
        WF,
        ".github/workflows/installer-e2e.yml",
        ".github/workflows/update-e2e.yml",
    ] {
        assert!(
            read(wf).contains("cargo build --release -p flare-mcp"),
            "{wf} builds the MCP server before it packs"
        );
    }
}

#[test]
fn no_fake_binary_is_packed() {
    let pack = read(PACK);
    for fake in [
        "fake-whisper-server",
        "fake-parakeet-cli",
        "fake-parakeet-host",
        "fake_",
    ] {
        assert!(!pack.contains(fake), "pack.ps1 names a stand-in: {fake}");
    }
}

#[test]
fn the_package_never_names_the_models_folder() {
    // O14: the models folder is %LOCALAPPDATA%\FlareDictation; the installer and the packer never touch it,
    // so an uninstall keeps the downloaded models (M8 Task 33; M9's side-by-side test holds the folder).
    for rel in [PACK, VERIFY] {
        let text = read(rel);
        assert!(
            !text.contains("FlareDictation"),
            "{rel} names the models folder"
        );
        assert!(
            !text.contains("LOCALAPPDATA"),
            "{rel} reaches into LOCALAPPDATA"
        );
    }
}

#[test]
fn the_engine_folder_holds_the_fifteen_files_the_runtime_and_the_notice() {
    let fetch = read(FETCH);
    let files = ps_array(&fetch, "Files");
    let runtime = ps_array(&fetch, "Runtime");
    assert_eq!(files.len(), 15, "the fifteen engine files");
    assert_eq!(runtime.len(), 4, "the four Microsoft runtime DLLs");
    assert!(
        fetch.contains(&format!("'{LICENSE_WHISPER}'")),
        "the notice is written into the engine folder"
    );
    let notice = read(LICENSE_WHISPER);
    assert!(notice.contains("MIT License"), "whisper.cpp is MIT");
    assert!(
        notice.contains("ggml"),
        "the notice names its copyright holders"
    );
    assert!(!notice.contains(char::from_u32(0x2014).expect("the em dash")));
}

#[test]
fn the_notices_and_the_readme_name_every_new_dependency() {
    let readme = read(README);
    for line in [
        "`libloading` 0.8.9",
        "`ureq` 3.4.2",
        "`zip` 9.0.0",
        "whisper.cpp",
        "Mermaid 11.17.2",
    ] {
        assert!(readme.contains(line), "README.md lacks {line}");
    }
    let notices = read(NOTICES);
    for entry in [
        "libloading 0.8.9",
        "ureq 3.4.2",
        "zip 9.0.0",
        "whisper.cpp engine b5130",
        "Mermaid 11.17.2",
    ] {
        assert!(
            notices.contains(entry),
            "THIRD-PARTY-NOTICES.md lacks {entry}"
        );
    }
}

// ---- Audit 2026-10-20 (A4: H4, L60, L61) ----

const EMDASH_CHECK: &str = "tools/release/check-no-emdash.sh";
const ADVISORY_GATE: &str = "tools/release/check-advisory-gate.ps1";

#[test]
fn both_workflows_run_the_one_shared_em_dash_script() {
    // H4: release.yml's own copy lacked -I and tripped on three PNGs; one script, called by both.
    let script = read(EMDASH_CHECK);
    assert!(
        script.contains("git grep -I -n"),
        "the shared script must skip binary files (-I)"
    );
    for wf in [WF, ".github/workflows/ci.yml"] {
        let text = read(wf);
        assert!(
            text.contains("bash tools/release/check-no-emdash.sh"),
            "{wf} must call the shared script"
        );
        assert!(
            !text.contains("git grep"),
            "{wf} must not carry its own copy of the check"
        );
    }
}

#[test]
fn every_third_party_action_in_the_release_job_is_pinned_to_a_full_sha() {
    // L61: the job holds contents: write, so a moved tag must not be able to run in it.
    let wf = read(WF);
    let mut seen = 0;
    for line in wf.lines().filter(|l| l.contains("uses:")) {
        let spec = line.split("uses:").nth(1).unwrap().trim();
        let (action, rest) = spec.split_once('@').expect("uses: owner/name@ref");
        let sha = rest.split_whitespace().next().unwrap();
        assert!(
            sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
            "{action} is not pinned to a full commit SHA: {line}"
        );
        assert!(
            rest.contains("# v"),
            "{action} keeps its version in a comment: {line}"
        );
        seen += 1;
    }
    assert!(seen >= 5, "expected the five actions, saw {seen}");
}

#[test]
fn the_advisory_gate_runs_in_the_release_before_the_updater_can_be_built_in() {
    // L60: the zip 8.6 ignores in deny.toml may not survive into a build with live updates.
    let wf = read(WF);
    assert!(wf.contains("tools/release/check-advisory-gate.ps1"));
    let gate = at(&wf, "tools/release/check-advisory-gate.ps1");
    let build = at(&wf, "cargo build --release -p flare @features");
    assert!(gate < build, "the gate must run before the release build");
    let script = read(ADVISORY_GATE);
    for id in [
        "RUSTSEC-2026-0367",
        "RUSTSEC-2026-0368",
        "RUSTSEC-2026-0369",
        "RUSTSEC-2026-0429",
    ] {
        assert!(script.contains(id), "the gate must name {id}");
    }
}

fn pwsh() -> Option<&'static str> {
    ["pwsh", "powershell"].into_iter().find(|p| {
        std::process::Command::new(p)
            .args(["-NoProfile", "-Command", "exit 0"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

fn run_gate(deny_toml: &str, live: bool) -> i32 {
    let dir = std::env::temp_dir().join(format!(
        "flare-gate-{}-{}",
        std::process::id(),
        deny_toml.len()
    ));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("deny.toml");
    fs::write(&file, deny_toml).unwrap();
    let mut cmd = std::process::Command::new(pwsh().expect("a PowerShell"));
    cmd.args(["-NoProfile", "-File"])
        .arg(root().join(ADVISORY_GATE))
        .arg("-Deny")
        .arg(&file);
    if live {
        cmd.arg("-Live");
    }
    let code = cmd.status().unwrap().code().unwrap_or(-1);
    let _ = fs::remove_dir_all(&dir);
    code
}

#[test]
fn the_advisory_gate_fails_only_for_a_live_build_that_still_ignores_zip_advisories() {
    if pwsh().is_none() {
        eprintln!("no PowerShell here; the text tests above still hold");
        return;
    }
    let with = "ignore = [ { id = \"RUSTSEC-2026-0367\", reason = \"x\" } ]\n";
    let without = "ignore = [ { id = \"RUSTSEC-2026-0192\", reason = \"x\" } ]\n";
    assert_ne!(run_gate(with, true), 0, "live + ignores present must fail");
    assert_eq!(
        run_gate(with, false),
        0,
        "updater off: the ignores are allowed"
    );
    assert_eq!(run_gate(without, true), 0, "live + ignores gone passes");
    // The real file: while the updater is off the release must still pass...
    assert_eq!(
        std::process::Command::new(pwsh().unwrap())
            .args(["-NoProfile", "-File"])
            .arg(root().join(ADVISORY_GATE))
            .arg("-Deny")
            .arg(root().join("deny.toml"))
            .status()
            .unwrap()
            .code(),
        Some(0)
    );
}
