//! Audit 2026-10-20 (H3, L64): `pack.ps1` needs `vendor/whisper` and `flare-parakeet-host.exe`, and
//! `cargo build -p flare` builds neither. Every workflow that packs must fetch the engine and build
//! the host first, and `verify-pack.ps1` must refuse a package without them. These tests read the
//! files as text, like `fetch_whisper.rs`.

const RELEASE: &str = include_str!("../../../.github/workflows/release.yml");
const UPDATE: &str = include_str!("../../../.github/workflows/update-e2e.yml");
const VERIFY: &str = include_str!("../../../tools/release/verify-pack.ps1");
const FETCH: &str = include_str!("../../../tools/fetch-whisper.ps1");

const FETCH_STEP: &str = "tools/fetch-whisper.ps1 -Out vendor/whisper";
const HOST_BIN: &str = "--bin flare-parakeet-host";

/// Byte offset of the first line that runs `pack.ps1` (a call, not a comment).
fn first_pack(yml: &str) -> usize {
    let mut at = 0;
    for line in yml.split_inclusive('\n') {
        if line.contains("./tools/release/pack.ps1") && !line.trim_start().starts_with('#') {
            return at;
        }
        at += line.len();
    }
    panic!("the workflow never runs pack.ps1");
}

fn assert_before_pack(name: &str, yml: &str, needle: &str) {
    let pack = first_pack(yml);
    let found = yml
        .find(needle)
        .unwrap_or_else(|| panic!("{name} lacks `{needle}`"));
    assert!(found < pack, "{name}: `{needle}` must come before the pack");
}

#[test]
fn every_packing_workflow_fetches_the_engine_before_the_pack() {
    for (name, yml) in [("release.yml", RELEASE), ("update-e2e.yml", UPDATE)] {
        assert_before_pack(name, yml, FETCH_STEP);
    }
}

#[test]
fn every_packing_workflow_builds_the_parakeet_host_before_the_pack() {
    for (name, yml) in [("release.yml", RELEASE), ("update-e2e.yml", UPDATE)] {
        assert_before_pack(name, yml, HOST_BIN);
    }
}

#[test]
fn the_update_workflow_builds_the_host_into_the_same_target_folder_as_the_exe() {
    let line = UPDATE
        .lines()
        .find(|l| l.contains(HOST_BIN))
        .expect("a host build line");
    assert!(line.contains("--target-dir target-update"), "{line}");
}

#[test]
fn verify_pack_requires_the_engine_and_the_host() {
    assert!(VERIFY.contains("'current/whisper/whisper-server.exe'"));
    assert!(VERIFY.contains("'current/flare-parakeet-host.exe'"));
}

#[test]
fn fetch_whisper_copies_the_committed_licence_and_never_downloads_it() {
    assert!(FETCH.contains("LICENSE-whisper.cpp.txt"));
    assert!(
        !FETCH.contains("raw.githubusercontent.com"),
        "the notice comes from the repository, not a mutable tag"
    );
}

#[test]
fn fetch_whisper_verifies_every_engine_file_by_hash_not_by_presence() {
    // One pinned SHA-256 per engine file, and a check that runs on an existing folder too.
    assert!(FETCH.contains("$FileHashes"));
    let early = FETCH
        .find("$OutDir already holds")
        .expect("the already-complete message");
    let check = FETCH
        .find("Get-FileHash -LiteralPath (Join-Path $OutDir")
        .expect("a per-file hash check of the folder");
    assert!(check < early, "the folder is hashed before it is trusted");
}
