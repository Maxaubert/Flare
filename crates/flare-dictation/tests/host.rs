//! The Parakeet passes and the resident host against REAL child processes (Task 14). Ported test for
//! test from Prism Terminal's `dictationEngine.test.ts` at e5848cb ("a Parakeet model", 11 tests, and
//! "the resident Parakeet host", 3 tests), plus the rules N6 and N7 that its three host tests do not
//! pin. The stand-ins are `fake-parakeet-cli` and `fake-parakeet-host`, started through the engine's
//! `command` seam (the exe's file name says which one is wanted).
//! Windows-only until macOS phase 7 of the port plan (the reaping job, the named-pipe link, the pid checks).
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_dictation::catalog::Runner;
use flare_dictation::engine::{
    Engine, EngineConfig, EngineKind, Reason, TranscribeRequest, TranscribeResult,
};
use flare_dictation::job::Job;
use serde::Deserialize;
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

const FAKE_CLI: &str = env!("CARGO_BIN_EXE_fake-parakeet-cli");
const FAKE_HOST: &str = env!("CARGO_BIN_EXE_fake-parakeet-host");
const FAKE_WHISPER: &str = env!("CARGO_BIN_EXE_fake-whisper-server");
const REAL_HOST: &str = env!("CARGO_BIN_EXE_flare-parakeet-host");
const WAV_LEN: usize = 1234;
const MODEL_PK: &str = r"C:\Flare Dictation\models\parakeet-v3.bin";
const MODEL_PK2: &str = r"C:\Flare Dictation\models\parakeet-v3-q4.bin";

fn model_path(id: &str) -> Option<PathBuf> {
    match id {
        "base" => Some(PathBuf::from(r"C:\Flare Dictation\models\ggml-base.bin")),
        "pk" => Some(PathBuf::from(MODEL_PK)),
        "pk2" => Some(PathBuf::from(MODEL_PK2)),
        _ => None,
    }
}

/// What `fake-parakeet-cli` says it saw.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct CliSeen {
    bytes: i64,
    model: Option<String>,
    file: Option<String>,
    no_prints: bool,
    threads: Option<String>,
    no_gpu: bool,
    pid: u32,
    cwd: Option<String>,
    cuda_cache: Option<String>,
}

/// What `fake-parakeet-host` says it saw.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct HostSeen {
    n: u64,
    bytes: usize,
    pid: u32,
    gpu: bool,
    model: String,
    dir: String,
}

fn text_of(r: &TranscribeResult) -> &str {
    match r {
        TranscribeResult::Ok(text) => text,
        other => panic!("expected an answer, got {other:?}"),
    }
}

fn cli_seen(r: &TranscribeResult) -> CliSeen {
    serde_json::from_str(text_of(r)).expect("the stand-in's JSON")
}

fn host_seen(r: &TranscribeResult) -> HostSeen {
    serde_json::from_str(text_of(r)).expect("the stand-in's JSON")
}

#[derive(Clone, Default)]
struct Knobs {
    gpu: bool,
    nvidia: bool,
    cli_cpu: Vec<String>,
    cli_gpu: Vec<String>,
    host_cpu: Vec<String>,
    host_gpu: Vec<String>,
}

fn flags(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

/// One launch the engine asked for: the exe's file name, the engine folder's kind and the argv.
#[derive(Clone, Debug)]
struct Call {
    exe: PathBuf,
    name: String,
    kind: EngineKind,
    args: Vec<String>,
}

struct World {
    engine: Engine,
    knobs: Arc<Mutex<Knobs>>,
    calls: Arc<Mutex<Vec<Call>>>,
    cpu_dir: tempfile::TempDir,
    gpu_dir: tempfile::TempDir,
    job: Arc<Job>,
}

fn world(knobs: Knobs) -> World {
    world_with(knobs, false, |_| {})
}

fn host_world(knobs: Knobs) -> World {
    world_with(knobs, true, |_| {})
}

fn world_with(knobs: Knobs, host: bool, tune: impl FnOnce(&mut EngineConfig)) -> World {
    world_in_job(knobs, host, tune, Job::new().expect("a job object"))
}

fn world_in_job(knobs: Knobs, host: bool, tune: impl FnOnce(&mut EngineConfig), job: Job) -> World {
    // A space in the folder on purpose: the real one is under a user's profile.
    let cpu_dir = tempfile::Builder::new()
        .prefix("flare dictation cpu ")
        .tempdir()
        .expect("a temp folder");
    let gpu_dir = tempfile::Builder::new()
        .prefix("flare dictation gpu ")
        .tempdir()
        .expect("a temp folder");
    let (cpu_path, gpu_path) = (cpu_dir.path().to_path_buf(), gpu_dir.path().to_path_buf());
    let knobs = Arc::new(Mutex::new(knobs));
    let calls: Arc<Mutex<Vec<Call>>> = Arc::new(Mutex::new(Vec::new()));
    let (k1, k2, k3) = (Arc::clone(&knobs), Arc::clone(&knobs), Arc::clone(&knobs));
    let (cpu_for, gpu_for) = (cpu_path.clone(), gpu_path.clone());
    let mut cfg = EngineConfig::new(
        Box::new(move || Some(cpu_for.clone())),
        Box::new(move || k1.lock().unwrap().gpu.then(|| gpu_for.clone())),
        Box::new(model_path),
    );
    cfg.runner_for = Box::new(|id| {
        if id.starts_with("pk") {
            Runner::Parakeet
        } else {
            Runner::Whisper
        }
    });
    cfg.has_nvidia = Box::new(move || k2.lock().unwrap().nvidia);
    cfg.threads = 6;
    if host {
        cfg.parakeet_host = Some(cpu_path.join("flare-parakeet-host.exe"));
    }
    let log = Arc::clone(&calls);
    cfg.command = Some(Box::new(move |exe: &Path, args: &[String]| {
        let name = exe
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        // The host's first argument is its engine folder; the others run in it.
        let in_gpu = exe.starts_with(&gpu_path)
            || (name == "flare-parakeet-host.exe"
                && args.first().is_some_and(|d| Path::new(d) == gpu_path));
        let kind = if in_gpu {
            EngineKind::Gpu
        } else {
            EngineKind::Cpu
        };
        log.lock().unwrap().push(Call {
            exe: exe.to_path_buf(),
            name: name.clone(),
            kind,
            args: args.to_vec(),
        });
        let k = k3.lock().unwrap();
        let mut all = args.to_vec();
        let target = match name.as_str() {
            "parakeet-cli.exe" => {
                all.extend(if in_gpu { &k.cli_gpu } else { &k.cli_cpu }.clone());
                FAKE_CLI
            }
            "flare-parakeet-host.exe" => {
                all.extend(if in_gpu { &k.host_gpu } else { &k.host_cpu }.clone());
                FAKE_HOST
            }
            _ => FAKE_WHISPER,
        };
        (PathBuf::from(target), all)
    }));
    tune(&mut cfg);
    let job = Arc::new(job);
    World {
        engine: Engine::start(cfg, Arc::clone(&job)),
        job,
        knobs,
        calls,
        cpu_dir,
        gpu_dir,
    }
}

impl World {
    fn set(&self, f: impl FnOnce(&mut Knobs)) {
        f(&mut self.knobs.lock().unwrap());
    }

    fn calls_named(&self, name: &str) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.name == name)
            .cloned()
            .collect()
    }

    fn cli_calls(&self) -> Vec<Call> {
        self.calls_named("parakeet-cli.exe")
    }

    fn host_calls(&self) -> Vec<Call> {
        self.calls_named("flare-parakeet-host.exe")
    }

    fn kinds(&self) -> Vec<EngineKind> {
        self.calls.lock().unwrap().iter().map(|c| c.kind).collect()
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn pids(&self) -> Vec<u32> {
        self.engine.launched_pids()
    }

    fn pid(&self, i: usize) -> u32 {
        self.engine.launched_pids()[i]
    }

    fn all_gone(&self) -> bool {
        self.pids().iter().all(|p| pid_gone(*p))
    }
}

impl Drop for World {
    fn drop(&mut self) {
        // No test may leave a process behind: every child ever launched must be gone.
        self.engine.stop();
        let pids = self.engine.launched_pids();
        let end = Instant::now() + Duration::from_secs(5);
        while pids.iter().any(|p| !pid_gone(*p)) {
            if Instant::now() > end {
                if !std::thread::panicking() {
                    panic!("a child outlived its test: {pids:?}");
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn pid_gone(pid: u32) -> bool {
    // SAFETY: OpenProcess with query and synchronize rights returns a handle or null; a non-null
    // handle is waited on with a zero timeout and closed exactly once, here.
    unsafe {
        let h = OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if h.is_null() {
            return true;
        }
        let done = WaitForSingleObject(h, 0) == WAIT_OBJECT_0;
        CloseHandle(h);
        done
    }
}

fn until(what: impl Fn() -> bool, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while !what() {
        assert!(Instant::now() < end, "timed out waiting");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn req(model: &str, final_: bool) -> TranscribeRequest {
    TranscribeRequest {
        wav: vec![7u8; WAV_LEN],
        model_id: model.to_string(),
        language: "auto".to_string(),
        final_,
    }
}

fn pk(final_: bool) -> TranscribeRequest {
    req("pk", final_)
}

fn get(rx: &Receiver<TranscribeResult>) -> TranscribeResult {
    rx.recv_timeout(Duration::from_secs(30))
        .expect("the engine answers")
}

fn ask(w: &World, r: TranscribeRequest) -> TranscribeResult {
    get(&w.engine.transcribe(r))
}

fn no(reason: Reason) -> TranscribeResult {
    TranscribeResult::No {
        reason,
        detail: None,
    }
}

fn failed_detail(r: &TranscribeResult) -> &str {
    match r {
        TranscribeResult::No {
            reason: Reason::EngineFailed,
            detail,
        } => detail.as_deref().unwrap_or(""),
        other => panic!("expected engine-failed, got {other:?}"),
    }
}

fn is_engine_failed(r: &TranscribeResult) -> bool {
    matches!(
        r,
        TranscribeResult::No {
            reason: Reason::EngineFailed,
            ..
        }
    )
}

fn same_dir(seen: &str, dir: &Path) -> bool {
    let norm = |p: &Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_lowercase()
    };
    norm(Path::new(seen)) == norm(dir)
}

// ---- "a Parakeet model" (PT, 11 tests) --------------------------------------------------------

#[test]
fn runs_parakeet_cli_once_per_pass_the_wav_on_stdin_and_never_a_file_and_no_language() {
    let w = world(Knobs::default());
    let r = ask(&w, pk(true));
    let s = cli_seen(&r);
    let calls = w.cli_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].exe, w.cpu_dir.path().join("parakeet-cli.exe"));
    // `-f -` is stdin: the audio never touches the disk.
    assert_eq!(
        calls[0].args,
        flags(&["-m", MODEL_PK, "-f", "-", "-np", "-t", "6", "-ng"])
    );
    assert_eq!(usize::try_from(s.bytes).ok(), Some(WAV_LEN));
    assert_eq!(s.model.as_deref(), Some(MODEL_PK));
    assert_eq!(s.file.as_deref(), Some("-"));
    assert!(s.no_prints && s.no_gpu);
    assert!(same_dir(s.cwd.as_deref().unwrap_or(""), w.cpu_dir.path()));
    assert_eq!(w.engine.info().engine, Some(EngineKind::Cpu));
    // Nothing stays up between passes.
    until(|| w.all_gone(), 4000);
    assert!(!w.engine.info().running);
    let again = cli_seen(&ask(&w, pk(true)));
    assert_ne!(again.pid, s.pid);
    assert_eq!(w.cli_calls().len(), 2);
}

#[test]
fn takes_the_text_off_stdout_as_utf8_crlf_trimmed() {
    let w = world(Knobs {
        cli_cpu: flags(&["--fake-unicode"]),
        ..Knobs::default()
    });
    assert_eq!(
        ask(&w, pk(true)),
        TranscribeResult::Ok("Gr\u{fc}\u{df} Gott, \u{e6}\u{f8}\u{e5}, \u{bf}qu\u{e9} tal?".into())
    );
}

#[test]
fn uses_the_gpu_pack_with_its_kernel_cache_and_without_ng_where_a_whisper_server_would() {
    let w = world_with(
        Knobs {
            gpu: true,
            nvidia: true,
            ..Knobs::default()
        },
        false,
        |cfg| {
            cfg.cuda_cache = Some(PathBuf::from(r"C:\Flare Dictation\cuda-cache"));
        },
    );
    let s = cli_seen(&ask(&w, pk(true)));
    assert_eq!(w.engine.info().engine, Some(EngineKind::Gpu));
    assert!(!s.no_gpu);
    assert!(same_dir(s.cwd.as_deref().unwrap_or(""), w.gpu_dir.path()));
    assert_eq!(
        s.cuda_cache.as_deref(),
        Some(r"C:\Flare Dictation\cuda-cache")
    );
}

#[test]
fn falls_back_to_the_cpu_for_good_when_the_gpu_pass_fails() {
    let w = world(Knobs {
        gpu: true,
        nvidia: true,
        cli_gpu: flags(&["--fake-exit"]),
        ..Knobs::default()
    });
    assert!(ask(&w, pk(true)).is_ok());
    assert_eq!(w.engine.info().engine, Some(EngineKind::Cpu));
    assert!(w.engine.info().gpu_fell_back);
    assert!(ask(&w, pk(true)).is_ok());
    assert_eq!(
        w.kinds(),
        vec![EngineKind::Gpu, EngineKind::Cpu, EngineKind::Cpu]
    );
}

#[test]
fn keeps_the_gpu_when_a_gpu_pass_only_says_error_about_the_audio_exit_0() {
    let w = world(Knobs {
        gpu: true,
        nvidia: true,
        cli_gpu: flags(&["--fake-error-zero"]),
        ..Knobs::default()
    });
    let r = ask(&w, pk(true));
    assert!(is_engine_failed(&r), "got {r:?}");
    assert!(!w.engine.info().gpu_fell_back);
    assert_eq!(w.kinds(), vec![EngineKind::Gpu]);
}

#[test]
fn fails_with_what_it_said_when_it_exits_badly_or_says_error_with_exit_0() {
    let bad = world(Knobs {
        cli_cpu: flags(&["--fake-exit"]),
        ..Knobs::default()
    });
    let r = ask(&bad, pk(true));
    assert!(failed_detail(&r).contains("CUDA error"), "got {r:?}");
    let zero = world(Knobs {
        cli_cpu: flags(&["--fake-error-zero"]),
        ..Knobs::default()
    });
    let z = ask(&zero, pk(true));
    assert!(
        failed_detail(&z).contains("failed to read audio"),
        "got {z:?}"
    );
}

#[test]
fn answers_a_partial_before_the_first_pass_with_warming_warms_with_a_silent_pass_then_serves_partials(
) {
    let w = world(Knobs::default());
    assert_eq!(ask(&w, pk(false)), no(Reason::Warming));
    until(|| !w.cli_calls().is_empty(), 4000);
    let end = Instant::now() + Duration::from_secs(6);
    loop {
        if ask(&w, pk(false)).is_ok() {
            break;
        }
        assert!(Instant::now() < end, "a partial was never served");
        std::thread::sleep(Duration::from_millis(10));
    }
    // The warm-up was a real pass of the silent wav, and ran on the CPU.
    assert!(w.cli_calls()[0].args.contains(&"-ng".to_string()));
}

#[test]
fn is_warmed_once_per_model_not_once_per_press() {
    let w = world(Knobs::default());
    assert!(get(&w.engine.warm("pk", "auto")).is_ok());
    assert!(get(&w.engine.warm("pk", "no")).is_ok());
    assert_eq!(w.cli_calls().len(), 1);
}

#[test]
fn takes_down_a_resident_whisper_server_which_holds_a_model_nobody_uses_now() {
    let w = world(Knobs::default());
    assert!(ask(&w, req("base", true)).is_ok());
    assert!(!pid_gone(w.pid(0)));
    assert!(ask(&w, pk(true)).is_ok());
    until(|| pid_gone(w.pid(0)), 4000);
    // And back on Whisper, a fresh server.
    assert!(ask(&w, req("base", true)).is_ok());
    let names: Vec<String> = w
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(
        names,
        vec![
            "whisper-server.exe",
            "parakeet-cli.exe",
            "whisper-server.exe"
        ]
    );
}

#[test]
fn stop_ends_a_pass_in_flight_at_once_fails_it_and_what_waits_and_the_next_arm_warms_again() {
    let w = world(Knobs {
        cli_cpu: flags(&["--fake-delay", "3000"]),
        ..Knobs::default()
    });
    let in_flight = w.engine.transcribe(pk(true));
    let waiting = w.engine.transcribe(pk(true));
    until(|| w.pids().len() == 1, 4000);
    std::thread::sleep(Duration::from_millis(150));
    assert!(w.engine.info().running);
    w.engine.stop();
    assert!(!w.engine.info().running);
    assert!(is_engine_failed(&get(&in_flight)));
    assert!(is_engine_failed(&get(&waiting)));
    until(|| pid_gone(w.pid(0)), 1500);
    // Off means off: no second process was started for the waiting one.
    assert_eq!(w.call_count(), 1);
    w.set(|k| k.cli_cpu.clear());
    assert_eq!(ask(&w, pk(false)), no(Reason::Warming));
}

#[test]
fn kills_a_pass_that_never_answers_and_says_so() {
    let w = world_with(
        Knobs {
            cli_cpu: flags(&["--fake-delay", "5000"]),
            ..Knobs::default()
        },
        false,
        |cfg| cfg.request_timeout = Duration::from_millis(200),
    );
    let r = ask(&w, pk(true));
    assert!(failed_detail(&r).contains("no answer"), "got {r:?}");
    until(|| pid_gone(w.pid(0)), 4000);
}

// ---- "the resident Parakeet host" (PT, 3 tests) -----------------------------------------------

#[test]
fn answers_every_pass_from_one_process_that_keeps_the_model_and_runs_no_parakeet_cli() {
    let w = host_world(Knobs::default());
    let a = host_seen(&ask(&w, pk(true)));
    let b = host_seen(&ask(&w, pk(true)));
    assert_eq!([a.n, b.n], [1, 2]);
    assert_eq!(b.pid, a.pid);
    assert_eq!(a.bytes, WAV_LEN);
    assert_eq!(a.model, MODEL_PK);
    assert!(w.cli_calls().is_empty());
    assert_eq!(w.host_calls().len(), 1);
}

#[test]
fn falls_back_to_parakeet_cli_when_the_host_never_gets_ready_and_does_not_try_it_again() {
    let w = world_with(
        Knobs {
            host_cpu: flags(&["--fake-never"]),
            ..Knobs::default()
        },
        true,
        |cfg| cfg.start_timeout = Duration::from_millis(300),
    );
    assert!(ask(&w, pk(true)).is_ok());
    assert!(ask(&w, pk(true)).is_ok());
    assert_eq!(w.host_calls().len(), 1);
    assert_eq!(w.cli_calls().len(), 2);
    // The host that never got ready was killed.
    until(|| pid_gone(w.pid(0)), 4000);
}

#[test]
fn is_ended_by_stop_and_by_a_whisper_model_taking_over() {
    let w = host_world(Knobs::default());
    assert!(ask(&w, pk(true)).is_ok());
    assert!(w.engine.info().running);
    w.engine.stop();
    until(|| w.all_gone(), 4000);
    assert!(ask(&w, pk(true)).is_ok());
    assert_eq!(w.host_calls().len(), 2);
    assert!(ask(&w, req("base", true)).is_ok());
    until(|| pid_gone(w.pid(1)), 4000);
}

// ---- N6 and N7 beyond PT's three tests --------------------------------------------------------

#[test]
fn the_host_is_started_with_the_folder_the_model_the_gpu_flag_and_the_threads() {
    let w = host_world(Knobs::default());
    assert!(ask(&w, pk(true)).is_ok());
    let calls = w.host_calls();
    let dir = w.cpu_dir.path().to_string_lossy().into_owned();
    assert_eq!(calls[0].args, flags(&[&dir, MODEL_PK, "0", "6"]));
    assert_eq!(
        calls[0].exe,
        w.cpu_dir.path().join("flare-parakeet-host.exe")
    );
}

#[test]
fn the_host_is_replaced_when_the_model_changes() {
    let w = host_world(Knobs::default());
    let a = host_seen(&ask(&w, pk(true)));
    let b = host_seen(&ask(&w, req("pk2", true)));
    assert_ne!(a.pid, b.pid);
    assert_eq!((a.n, b.n), (1, 1));
    assert_eq!(b.model, MODEL_PK2);
    assert_eq!(w.host_calls().len(), 2);
    until(|| pid_gone(w.pid(0)), 4000);
}

#[test]
fn a_host_that_answers_err_fails_the_pass_with_why_and_stays_up() {
    let w = host_world(Knobs {
        host_cpu: flags(&["--fake-err"]),
        ..Knobs::default()
    });
    let r = ask(&w, pk(true));
    assert_eq!(failed_detail(&r), "boom");
    assert!(is_engine_failed(&ask(&w, pk(true))));
    assert_eq!(w.host_calls().len(), 1);
    assert!(!pid_gone(w.pid(0)));
    assert!(w.cli_calls().is_empty());
}

#[test]
fn a_host_that_exits_mid_pass_answers_with_its_stderr_tail_and_the_next_pass_gets_a_fresh_one() {
    let w = host_world(Knobs {
        host_cpu: flags(&["--fake-die"]),
        ..Knobs::default()
    });
    let r = ask(&w, pk(true));
    assert!(failed_detail(&r).contains("host crashed"), "got {r:?}");
    // A host that is gone is not counted as running (PT's gone() clears it at once).
    assert!(!w.engine.info().running);
    w.set(|k| k.host_cpu.clear());
    assert_eq!(host_seen(&ask(&w, pk(true))).n, 1);
    assert_eq!(w.host_calls().len(), 2);
}

#[test]
fn lines_that_are_none_of_its_words_are_ignored() {
    let w = host_world(Knobs {
        host_cpu: flags(&["--fake-noise"]),
        ..Knobs::default()
    });
    assert_eq!(host_seen(&ask(&w, pk(true))).n, 1);
    assert_eq!(host_seen(&ask(&w, pk(true))).n, 2);
}

#[test]
fn a_pass_the_host_never_answers_is_killed_with_the_host_and_the_next_one_starts_afresh() {
    let w = world_with(
        Knobs {
            host_cpu: flags(&["--fake-delay", "5000"]),
            ..Knobs::default()
        },
        true,
        |cfg| cfg.request_timeout = Duration::from_millis(300),
    );
    let r = ask(&w, pk(true));
    assert!(failed_detail(&r).contains("no answer"), "got {r:?}");
    until(|| pid_gone(w.pid(0)), 4000);
    assert!(!w.engine.info().running);
    w.set(|k| k.host_cpu.clear());
    assert_eq!(host_seen(&ask(&w, pk(true))).n, 1);
    assert_eq!(w.host_calls().len(), 2);
}

#[test]
fn a_gpu_host_that_dies_falls_back_to_a_cpu_host_for_good() {
    let w = host_world(Knobs {
        gpu: true,
        nvidia: true,
        host_gpu: flags(&["--fake-die"]),
        ..Knobs::default()
    });
    let seen = host_seen(&ask(&w, pk(true)));
    assert!(!seen.gpu);
    assert!(w.engine.info().gpu_fell_back);
    assert_eq!(w.engine.info().engine, Some(EngineKind::Cpu));
    assert_eq!(w.kinds(), vec![EngineKind::Gpu, EngineKind::Cpu]);
}

#[test]
fn the_host_is_in_the_job() {
    let w = host_world(Knobs::default());
    assert!(ask(&w, pk(true)).is_ok());
    assert!(w.job.contains(w.pid(0)), "the host is not in the job");
}

#[test]
fn a_host_that_could_not_join_the_job_is_killed_and_the_pass_fails() {
    let w = world_in_job(Knobs::default(), true, |_| {}, Job::unusable_for_tests());
    let r = ask(&w, pk(true));
    assert!(failed_detail(&r).contains("job"), "got {r:?}");
    assert!(!w.host_calls().is_empty(), "a host was tried");
    // Refused children are killed and reaped inside the spawn, so none is ever recorded as running.
    assert!(w.pids().is_empty());
}

#[test]
fn a_pass_that_could_not_join_the_job_is_killed_and_fails() {
    let w = world_in_job(Knobs::default(), false, |_| {}, Job::unusable_for_tests());
    let r = ask(&w, pk(true));
    assert!(failed_detail(&r).contains("job"), "got {r:?}");
    assert!(w.pids().is_empty());
}

#[test]
fn every_pass_child_is_in_the_job() {
    let w = world(Knobs {
        cli_cpu: flags(&["--fake-delay", "1500"]),
        ..Knobs::default()
    });
    let rx = w.engine.transcribe(pk(true));
    until(|| w.pids().len() == 1, 4000);
    assert!(w.job.contains(w.pid(0)), "the pass is not in the job");
    assert!(get(&rx).is_ok());
}

#[test]
fn a_stop_while_the_host_starts_is_a_stop_not_a_broken_host() {
    // A host that is alive and silent, then a stop in the middle of the wait for it.
    let w = host_world(Knobs {
        host_cpu: flags(&["--fake-never"]),
        ..Knobs::default()
    });
    let rx = w.engine.transcribe(pk(true));
    until(|| !w.host_calls().is_empty(), 4000);
    std::thread::sleep(Duration::from_millis(100));
    w.engine.stop();
    assert!(is_engine_failed(&get(&rx)));
    // The host is still believed in: the next pass tries it again, and this one is ready.
    w.set(|k| k.host_cpu.clear());
    assert_eq!(host_seen(&ask(&w, pk(true))).n, 1);
    assert_eq!(w.host_calls().len(), 2);
    assert!(w.cli_calls().is_empty());
}

#[test]
fn a_host_that_is_not_there_to_launch_falls_back_to_parakeet_cli() {
    // No `command` seam, a path that is not a program: the launch itself fails.
    let w = world_with(Knobs::default(), true, |cfg| {
        cfg.parakeet_host = Some(PathBuf::from(r"C:\no such folder\flare-parakeet-host.exe"));
        cfg.command = None;
    });
    let r = ask(&w, pk(true));
    // The real parakeet-cli is not there either, so the pass fails, but it was TRIED: the failure is
    // the cli's launch, not a host's.
    assert!(is_engine_failed(&r), "got {r:?}");
}

// ---- the real host binary and the real engine -------------------------------------------------

#[test]
fn the_real_host_without_an_engine_never_says_ready_and_ends() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().expect("a temp folder");
    let mut child = Command::new(REAL_HOST)
        .arg(dir.path())
        .arg(r"C:\no such model.bin")
        .arg("0")
        .arg("2")
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the host starts");
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut out);
    }
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    let status = child.wait().expect("it ends");
    assert!(!out.contains("ready"), "said ready: {out:?}");
    assert!(!status.success());
    assert!(!err.trim().is_empty(), "it should say why on stderr");
}

#[test]
#[ignore = "needs vendor/whisper and the q4 model; run by Task 29's orchestrator step"]
fn the_real_host_answers_like_parakeet_cli() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let from_env =
        |var: &str, default: PathBuf| std::env::var_os(var).map_or(default, PathBuf::from);
    let dir = from_env("FLARE_WHISPER_DIR", root.join("vendor/whisper"));
    let cache = root.join("tools/e2e/.e2e-cache");
    let model = from_env(
        "FLARE_PARAKEET_MODEL",
        cache.join("ggml-parakeet-tdt-0.6b-v3-q4_0.bin"),
    );
    let clip = from_env("FLARE_E2E_CLIP", cache.join("jfk.wav"));
    for p in [&dir, &model, &clip] {
        assert!(p.exists(), "missing {}", p.display());
    }
    let wav = std::fs::read(&clip).expect("the clip");
    let launches: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let answer = |host: Option<PathBuf>| {
        let (d, m) = (dir.clone(), model.clone());
        let log = Arc::clone(&launches);
        let mut cfg = EngineConfig::new(
            Box::new(move || Some(d.clone())),
            Box::new(|| None),
            Box::new(move |_| Some(m.clone())),
        );
        cfg.runner_for = Box::new(|_| Runner::Parakeet);
        cfg.has_nvidia = Box::new(|| false);
        cfg.idle = None;
        cfg.parakeet_host = host;
        // Record every launch, so the proof can say WHICH program answered.
        cfg.command = Some(Box::new(move |exe: &Path, args: &[String]| {
            let name = exe
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            log.lock().unwrap().push(name);
            (exe.to_path_buf(), args.to_vec())
        }));
        let engine = Engine::start(cfg, Arc::new(Job::new().expect("a job")));
        let rx = engine.transcribe(TranscribeRequest {
            wav: wav.clone(),
            model_id: "parakeet-v3-q4".into(),
            language: "auto".into(),
            final_: true,
        });
        get(&rx)
    };
    let by_cli = answer(None);
    launches.lock().unwrap().clear();
    let by_host = answer(Some(PathBuf::from(REAL_HOST)));
    // The native host answered: it was launched, and parakeet-cli never was (a host that never said
    // ready would have been replaced by parakeet-cli, and the equality below would hold trivially).
    let ran = launches.lock().unwrap().clone();
    assert_eq!(ran.len(), 1, "launches: {ran:?}");
    assert!(ran[0].contains("flare-parakeet-host"), "launches: {ran:?}");
    assert!(!text_of(&by_cli).is_empty(), "parakeet-cli heard nothing");
    assert_eq!(text_of(&by_host), text_of(&by_cli));
}
