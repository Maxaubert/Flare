//! The build-time engine fetch (M8 Task 28): `tools/fetch-whisper.ps1` must say what `ENGINE` in
//! `catalog.rs` says. Two copies of a pin drift silently, so this test reads the script as text.

use flare_dictation::catalog::ENGINE;

const SCRIPT: &str = include_str!("../../../tools/fetch-whisper.ps1");

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
