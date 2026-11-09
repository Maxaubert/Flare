//! The build-time engine fetch (M8 Task 28): `tools/fetch-whisper.ps1` must say what `ENGINE` in
//! `catalog.rs` says. Two copies of a pin drift silently, so this test reads the script as text.

use flare_dictation::catalog::{ENGINE, ENGINE_MACOS};

const SCRIPT: &str = include_str!("../../../tools/fetch-whisper.ps1");
/// The macOS engine's build (port phase 7, rule 315).
const MAC_SCRIPT: &str = include_str!("../../../tools/build-whisper-macos.sh");

/// The single-quoted value of `NAME='...'` in the macOS script (the whole line, so a changed shape fails here).
fn mac_quoted(name: &str) -> String {
    let line = MAC_SCRIPT
        .lines()
        .find(|l| l.starts_with(&format!("{name}='")))
        .unwrap_or_else(|| panic!("the macOS script has no `{name}='...'` line"));
    let start = line.find('\'').expect("a quote") + 1;
    let end = line.rfind('\'').expect("a closing quote");
    line[start..end].to_string()
}

#[test]
fn the_macos_build_pins_match_the_catalogue() {
    assert_eq!(mac_quoted("REPO"), ENGINE_MACOS.repo);
    assert_eq!(mac_quoted("TAG"), ENGINE_MACOS.tag);
    assert_eq!(mac_quoted("COMMIT"), ENGINE_MACOS.commit);
    assert_eq!(mac_quoted("TREE"), ENGINE_MACOS.tree);
    assert_eq!(mac_quoted("PROGRAMS"), ENGINE_MACOS.programs.join(" "));
}

#[test]
fn the_macos_build_checks_the_commit_and_tree_before_it_builds_and_fetches_nothing_unpinned() {
    let fetch = MAC_SCRIPT
        .find("fetch -q --depth 1 \"$REPO\" \"$COMMIT\"")
        .expect("a fetch by commit");
    let tree = MAC_SCRIPT
        .find("rev-parse 'HEAD^{tree}'")
        .expect("the tree check");
    let refuse = MAC_SCRIPT.find("refusing to build").expect("a refusal");
    let build = MAC_SCRIPT.find("cmake -S").expect("the configure");
    assert!(fetch < tree && tree < refuse && refuse < build);
    for flag in [
        "-DGGML_METAL=ON",
        "-DGGML_METAL_EMBED_LIBRARY=ON",
        "-DGGML_OPENMP=OFF",
        "-DGGML_CPU_KLEIDIAI=OFF",
        "-DWHISPER_CURL=OFF",
        "-DCMAKE_INSTALL_RPATH=@loader_path",
        "-DCMAKE_OSX_DEPLOYMENT_TARGET=15.0",
    ] {
        assert!(MAC_SCRIPT.contains(flag), "{flag}");
    }
    // No branch, no tag name and no archive is ever what is fetched.
    for bad in [
        "--branch",
        "archive/",
        "codeload",
        "releases/download",
        "curl ",
        "wget ",
    ] {
        assert!(!MAC_SCRIPT.contains(bad), "{bad}");
    }
    // Only the system's and the folder's own libraries, and the stamp hashes every file and is written last.
    assert!(MAC_SCRIPT.contains("/usr/lib/* | /System/Library/*"));
    assert!(MAC_SCRIPT.contains("shasum -a 256 -c"));
    assert!(
        MAC_SCRIPT.rfind("mv \"$STAMP.tmp\" \"$STAMP\"") > MAC_SCRIPT.find("cp -p \"$STAGE\"/*")
    );
}

/// The single-quoted value of `$Name = '...'` (the whole line is read, so a changed shape fails here).
fn quoted(name: &str) -> String {
    let line = SCRIPT
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("${name} = '")))
        .unwrap_or_else(|| panic!("the script has no `${name} = '...'` line"));
    let start = line.find('\'').expect("a quote") + 1;
    let end = line.rfind('\'').expect("a closing quote");
    line[start..end].to_string()
}

/// The single-quoted strings of `$Files = @( ... )`, in order.
fn file_list() -> Vec<String> {
    let start = SCRIPT
        .find("$Files = @(")
        .expect("the script has a $Files list");
    let body = &SCRIPT[start..];
    let body = &body[..body.find(')').expect("the list closes")];
    body.split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

#[test]
fn fetch_whisper_pins_match_the_catalogue() {
    assert_eq!(quoted("Tag"), ENGINE.tag);
    assert_eq!(quoted("Asset"), ENGINE.asset);
    assert_eq!(quoted("Sha256"), ENGINE.sha256);
    assert_eq!(
        SCRIPT
            .lines()
            .find(|l| l.trim_start().starts_with("$Bytes = "))
            .and_then(|l| l.split('=').nth(1))
            .map(|v| v.trim().parse::<u64>().expect("a number")),
        Some(ENGINE.bytes)
    );
    assert_eq!(file_list(), ENGINE.files.to_vec());
    // The URL the script builds is the one the catalogue names.
    assert!(SCRIPT.contains(
        "$Url = \"https://github.com/ggml-org/whisper.cpp/releases/download/$Tag/$Asset\""
    ));
    assert_eq!(
        ENGINE.url,
        format!(
            "https://github.com/ggml-org/whisper.cpp/releases/download/{}/{}",
            ENGINE.tag, ENGINE.asset
        )
    );
}

#[test]
fn fetch_whisper_checks_size_and_sha_before_it_unpacks() {
    let check = SCRIPT.find("Get-FileHash").expect("a SHA-256 check");
    let size = SCRIPT.find("$size -ne $Bytes").expect("a byte count check");
    let unpack = SCRIPT.find("& $tar -xf").expect("the unpack call");
    assert!(
        size < unpack && check < unpack,
        "checks run before the unpack"
    );
    // Windows' own tar by full path, never a bare `tar` that Git Bash's GNU tar could shadow.
    assert!(SCRIPT.contains("System32\\tar.exe"));
}

#[test]
fn fetch_whisper_copies_the_runtime_only_when_microsoft_signed_it() {
    for dll in [
        "msvcp140.dll",
        "vcruntime140.dll",
        "vcruntime140_1.dll",
        "vcomp140.dll",
    ] {
        assert!(SCRIPT.contains(&format!("'{dll}'")), "{dll}");
    }
    assert!(SCRIPT.contains("Get-AuthenticodeSignature"));
    assert!(SCRIPT.contains("'Valid'"));
    assert!(SCRIPT.contains("O=Microsoft Corporation"));
}

#[test]
fn fetch_whisper_writes_the_notice_and_a_stamp_and_skips_a_complete_folder() {
    assert!(SCRIPT.contains("LICENSE-whisper.cpp.txt"));
    assert!(SCRIPT.contains(".source"));
    assert!(SCRIPT.contains("already holds"));
}

#[test]
fn fetch_whisper_has_no_em_dashes() {
    assert!(!SCRIPT.contains('\u{2014}'));
}

#[test]
fn the_macos_ci_builds_the_pinned_engine_and_fetches_the_catalogues_tiny_model() {
    let ci = include_str!("../../../.github/workflows/ci.yml");
    let mac = &ci[ci.find("\n  macos:").expect("the macos job")..];
    assert!(
        mac.contains("bash tools/build-whisper-macos.sh vendor/macos/whisper vendor/macos/clips")
    );
    // The cache is keyed by the commit, so a new pin never reuses an old build.
    assert!(mac.contains(&format!("whisper-macos-arm64-{}", ENGINE_MACOS.commit)));
    let tiny = flare_dictation::catalog::model("tiny").expect("tiny");
    assert!(mac.contains(tiny.url), "the CI fetches another file");
    assert!(mac.contains(tiny.sha256));
    assert!(mac.contains(&tiny.bytes.to_string()));
    assert!(mac.contains("--test metal_real --run-ignored only"));
}

/// The text of `engine_ok` and of the stamp block in the macOS script, run as they are by bash.
#[cfg(unix)]
fn mac_stamp_and_check() -> String {
    let start = MAC_SCRIPT.find("engine_ok() {").expect("engine_ok");
    let end = start + MAC_SCRIPT[start..].find("\n}\n").expect("its end") + 3;
    let stamp = MAC_SCRIPT
        .find("{\n  echo \"$COMMIT\"")
        .expect("the stamp block");
    let stamp_end = stamp
        + MAC_SCRIPT[stamp..]
            .find("mv \"$STAMP.tmp\" \"$STAMP\"\n")
            .expect("the stamp's move")
        + "mv \"$STAMP.tmp\" \"$STAMP\"\n".len();
    format!(
        "{}\nstamp() {{\n{}}}\n",
        &MAC_SCRIPT[start..end],
        &MAC_SCRIPT[stamp..stamp_end]
    )
}

#[cfg(unix)]
#[test]
fn the_macos_engine_folder_is_reused_only_when_it_holds_exactly_the_stamped_files() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("whisper");
    std::fs::create_dir(&out).unwrap();
    for p in ENGINE_MACOS.programs {
        let f = out.join(p);
        std::fs::write(&f, format!("program {p}")).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(out.join("libggml.0.dylib"), b"lib").unwrap();
    let run = |body: &str| -> String {
        let script = format!(
            "set -eu\nCOMMIT='{}'\nPROGRAMS='{}'\nOUT='{}'\nSTAMP=\"$OUT/.source\"\n{}\n{body}\n",
            ENGINE_MACOS.commit,
            ENGINE_MACOS.programs.join(" "),
            out.display(),
            mac_stamp_and_check()
        );
        let o = std::process::Command::new("/bin/bash")
            .args(["-c", &script])
            .output()
            .expect("bash runs");
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let check = "if engine_ok; then echo OK; else echo NO; fi";
    assert_eq!(
        run(&format!("stamp\n{check}")),
        "OK",
        "a fresh stamp is reused"
    );
    let listed = std::fs::read_to_string(out.join(".source")).unwrap();
    assert!(
        !listed.contains(".source"),
        "the stamp lists itself: {listed}"
    );
    // A file the stamp does not list (a planted library) makes the folder one to rebuild.
    std::fs::write(out.join("libevil.dylib"), b"x").unwrap();
    assert_eq!(run(check), "NO", "an extra file is trusted");
    std::fs::remove_file(out.join("libevil.dylib")).unwrap();
    assert_eq!(run(check), "OK");
    // A changed file, and a missing one, too.
    std::fs::write(out.join("libggml.0.dylib"), b"swapped").unwrap();
    assert_eq!(run(check), "NO", "a changed file is trusted");
    assert_eq!(run(&format!("stamp\n{check}")), "OK");
    std::fs::remove_file(out.join("libggml.0.dylib")).unwrap();
    assert_eq!(run(check), "NO", "a missing file is trusted");
}
