//! A real pass on a Mac's own engine (port phase 7, rule 315): whisper-server and the Parakeet host built from the
//! pinned source with Metal (`tools/build-whisper-macos.sh`), the pinned tiny model, and the source's own clip.
//! Ignored by default: it needs those files, which no ordinary test run downloads or builds. The macOS CI job builds
//! the engine and runs it (`--run-ignored only`); `docs/dictation-by-hand.md` has the same check by hand.
//!
//! Where the files are: `FLARE_WHISPER_DIR` (default `vendor/macos/whisper`), `FLARE_WHISPER_MODEL` (default
//! `vendor/macos/models/ggml-tiny.bin`), `FLARE_E2E_CLIP` (default `vendor/macos/clips/jfk.wav`) and
//! `FLARE_PARAKEET_TEST_MODEL` (default `vendor/macos/clips/for-tests-ggml-parakeet-tdt.bin`). With
//! `FLARE_EXPECT_METAL=1` the pass must have run on Metal: a virtual machine without a Metal device falls back to the
//! CPU once and still answers, which is all CI can promise.
#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use flare_dictation::catalog::{self, Runner};
use flare_dictation::engine::{Engine, EngineConfig, EngineKind, TranscribeRequest};
use flare_dictation::job::Job;
use sha2::{Digest, Sha256};

const REAL_HOST: &str = env!("CARGO_BIN_EXE_flare-parakeet-host");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn from_env(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).map_or_else(|| root().join(default), PathBuf::from)
}

fn sha256_of(p: &Path) -> String {
    let bytes = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
#[ignore = "needs the engine from tools/build-whisper-macos.sh, the pinned tiny model and jfk.wav"]
fn the_mac_engine_transcribes_the_pinned_tiny_model() {
    let dir = from_env("FLARE_WHISPER_DIR", "vendor/macos/whisper");
    let model = from_env("FLARE_WHISPER_MODEL", "vendor/macos/models/ggml-tiny.bin");
    let clip = from_env("FLARE_E2E_CLIP", "vendor/macos/clips/jfk.wav");
    for p in [&dir, &model, &clip] {
        assert!(p.exists(), "missing {}", p.display());
    }
    // Only the catalogue's bytes are ever run (the store's promise): the model is checked first.
    let tiny = catalog::model("tiny").expect("the tiny model");
    assert_eq!(sha256_of(&model), tiny.sha256, "not the pinned tiny model");
    let wav = std::fs::read(&clip).expect("the clip");
    let (d, m) = (dir.clone(), model.clone());
    let mut cfg = EngineConfig::new(
        Box::new(move || Some(d.clone())),
        Box::new(|| None),
        Box::new(move |_| Some(m.clone())),
    );
    cfg.runner_for = Box::new(|_| Runner::Whisper);
    cfg.idle = None;
    assert!(cfg.bundled_gpu, "a Mac's engine is the Metal build");
    let engine = Engine::start(cfg, Arc::new(Job::new().expect("the reaper")));
    let rx = engine.transcribe(TranscribeRequest {
        wav,
        model_id: "tiny".into(),
        language: "en".into(),
        final_: true,
    });
    let answer = rx
        .recv_timeout(Duration::from_secs(180))
        .expect("the engine answers");
    let text = answer
        .text()
        .unwrap_or_else(|| panic!("no text: {answer:?}"))
        .to_lowercase();
    assert!(text.contains("country"), "heard {text:?}");
    let info = engine.info();
    eprintln!(
        "the pass ran on {:?} (fell back: {})",
        info.engine, info.gpu_fell_back
    );
    if std::env::var_os("FLARE_EXPECT_METAL").is_some_and(|v| v == "1") {
        assert_eq!(info.engine, Some(EngineKind::Gpu), "Metal did not answer");
        assert!(!info.gpu_fell_back);
    }
    engine.stop();
}

#[test]
#[ignore = "needs the engine from tools/build-whisper-macos.sh and the source's Parakeet test model"]
fn the_real_host_loads_the_mac_libraries_and_answers_a_pass() {
    let dir = from_env("FLARE_WHISPER_DIR", "vendor/macos/whisper");
    let model = from_env(
        "FLARE_PARAKEET_TEST_MODEL",
        "vendor/macos/clips/for-tests-ggml-parakeet-tdt.bin",
    );
    let clip = from_env("FLARE_E2E_CLIP", "vendor/macos/clips/jfk.wav");
    for p in [&dir, &model, &clip] {
        assert!(p.exists(), "missing {}", p.display());
    }
    let wav = std::fs::read(&clip).expect("the clip");
    let mut child = Command::new(REAL_HOST)
        .arg(&dir)
        .arg(&model)
        .arg("1")
        .arg("2")
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the host starts");
    let mut out = BufReader::new(child.stdout.take().expect("stdout"));
    let mut line = String::new();
    out.read_line(&mut line).expect("a line");
    if line.trim() != "ready" {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("the host said {line:?}, stderr {err:?}");
    }
    let mut input = child.stdin.take().expect("stdin");
    input
        .write_all(format!("pass {}\n", wav.len()).as_bytes())
        .expect("the pass header");
    input.write_all(&wav).expect("the clip");
    input.flush().expect("flush");
    line.clear();
    out.read_line(&mut line).expect("the answer");
    // The test model is the source's tiny random one: what it hears is noise, but the answer is the protocol's.
    assert!(
        line.starts_with("ok ") || line.starts_with("err "),
        "not an answer: {line:?}"
    );
    input.write_all(b"quit\n").expect("quit");
    drop(input);
    let status = child.wait().expect("it ends");
    assert!(status.success(), "{status:?}");
}
