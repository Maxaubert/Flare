//! macOS: the speech engine inside `Flare.app` (rule 1138). Max's Mac had the model and a working microphone, and
//! every pass answered `NoEngine`: the bundle carried no engine, and the app looked for one in
//! `Contents/MacOS/whisper`, a place a signed bundle cannot hold a folder with data in it. The engine now ships in
//! `Contents/Frameworks/whisper` and is found there. These tests build that layout in a temp folder with the
//! `fake-whisper-server` stand-in as the bundled whisper-server, started for real (no `command` seam).
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use flare_dictation::catalog::Runner;
use flare_dictation::engine::{
    bundled_engine_dir, engine_in, Engine, EngineConfig, EngineKind, Reason, TranscribeRequest,
    TranscribeResult, WHISPER_SERVER,
};
use flare_dictation::job::Job;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-whisper-server");

/// `<tmp>/Flare.app/Contents/MacOS`, with the engine folder made (and the stand-in in it) when asked.
fn fake_bundle(with_engine: bool) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix("flare bundled engine ")
        .tempdir()
        .expect("a temp folder");
    let macos = tmp.path().join("Flare.app/Contents/MacOS");
    std::fs::create_dir_all(&macos).expect("MacOS");
    let engine = tmp.path().join("Flare.app/Contents/Frameworks/whisper");
    std::fs::create_dir_all(&engine).expect("the engine folder");
    if with_engine {
        std::fs::copy(FAKE, engine.join(WHISPER_SERVER)).expect("the stand-in");
    }
    (tmp, macos)
}

/// The engine as `dictation_ctl` makes it: the bundled folder when its whisper-server is there, no pack.
fn app_engine(macos: &Path) -> Engine {
    let dir = bundled_engine_dir(macos);
    let found = dir.clone();
    let mut cfg = EngineConfig::new(
        Box::new(move || engine_in(&found).then(|| found.clone())),
        Box::new(|| None),
        Box::new(|_| {
            Some(PathBuf::from(
                "/Users/x/FlareDictation/models/ggml-base.bin",
            ))
        }),
    );
    cfg.runner_for = Box::new(|_| Runner::Whisper);
    cfg.idle = None;
    cfg.bundled_at = Some(dir);
    Engine::start(cfg, Arc::new(Job::new().expect("the reaper")))
}

fn ask(engine: &Engine, language: &str) -> TranscribeResult {
    engine
        .transcribe(TranscribeRequest {
            wav: flare_dictation::wav::silent_warmup_wav(),
            model_id: "base".into(),
            language: language.into(),
            final_: true,
        })
        .recv_timeout(Duration::from_secs(60))
        .expect("an answer")
}

#[test]
fn the_engine_of_a_bundle_is_in_its_frameworks_and_a_dev_build_looks_beside_itself() {
    assert_eq!(
        bundled_engine_dir(Path::new("/Applications/Flare.app/Contents/MacOS")),
        PathBuf::from("/Applications/Flare.app/Contents/Frameworks/whisper")
    );
    assert_eq!(
        bundled_engine_dir(Path::new("/repo/target/release")),
        PathBuf::from("/repo/target/release/whisper")
    );
    // A folder alone is not an engine: its whisper-server must be there.
    let (tmp, macos) = fake_bundle(false);
    assert!(!engine_in(&bundled_engine_dir(&macos)));
    drop(tmp);
    let (_tmp, macos) = fake_bundle(true);
    assert!(engine_in(&bundled_engine_dir(&macos)));
}

#[test]
fn the_bundled_engine_is_started_from_the_frameworks_folder_as_metal_with_language_auto() {
    let (_tmp, macos) = fake_bundle(true);
    let engine = app_engine(&macos);
    let r = ask(&engine, "");
    let TranscribeResult::Ok(text) = &r else {
        panic!("no answer from the bundled engine: {r:?}");
    };
    let seen: serde_json::Value = serde_json::from_str(text).expect("the stand-in's JSON");
    // An unset language is "auto", the server's own word for detecting it.
    assert_eq!(seen["language"], "auto", "{seen}");
    assert_eq!(seen["host"], "127.0.0.1", "{seen}");
    // Metal first (rule 1082): no -ng, so the GPU engine answered and nothing fell back.
    let info = engine.info();
    assert_eq!(info.engine, Some(EngineKind::Gpu));
    assert!(!info.gpu_fell_back);
    engine.stop();
}

#[test]
fn a_bundle_without_the_engine_says_no_engine_naming_the_folder_and_starts_nothing() {
    let (_tmp, macos) = fake_bundle(false);
    let engine = app_engine(&macos);
    let r = ask(&engine, "en");
    let TranscribeResult::No { reason, detail } = &r else {
        panic!("an answer without an engine: {r:?}");
    };
    assert_eq!(*reason, Reason::NoEngine);
    let detail = detail.as_deref().expect("the diag log is told where");
    assert!(detail.contains("whisper-server"), "{detail}");
    assert!(
        detail.contains("Flare.app/Contents/Frameworks/whisper"),
        "{detail}"
    );
    assert!(engine.launched_pids().is_empty());
}
