//! The Whisper engine's lifecycle against a REAL child process and a REAL loopback HTTP server (Task 13).
//! Ported test for test from Prism Terminal's `dictationEngine.test.ts` at e5848cb (the Whisper and
//! queue tests; the Parakeet ones are Task 14's). Nothing is mocked, because every rule here is about
//! processes and sockets (who is alive, who was killed, what went over the wire): the stand-in is
//! `fake-whisper-server`, started through the engine's `command` seam.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_dictation::catalog::Runner;
use flare_dictation::engine::{
    Engine, EngineConfig, EngineInfo, EngineKind, Reason, TranscribeRequest, TranscribeResult,
};
use flare_dictation::job::Job;
use serde::Deserialize;
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-whisper-server");
const WAV_LEN: usize = 1234;

fn model_path(id: &str) -> Option<PathBuf> {
    match id {
        "base" => Some(PathBuf::from(r"C:\Flare Dictation\models\ggml-base.bin")),
        "small" => Some(PathBuf::from(r"C:\Flare Dictation\models\ggml-small.bin")),
        "pk" => Some(PathBuf::from(r"C:\Flare Dictation\models\parakeet-v3.bin")),
        _ => None,
    }
}

/// What the stand-in says it saw, read back out of the `text` it answered with.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct Seen {
    n: u64,
    max_active: u64,
    bytes: i64,
    filename: Option<String>,
    r#type: Option<String>,
    format: Option<String>,
    model: Option<String>,
    language: Option<String>,
    host: String,
    port: u16,
    pid: u32,
    cwd: Option<String>,
    cuda_cache: Option<String>,
    cuda_max: Option<String>,
}

fn seen(r: &TranscribeResult) -> Seen {
    match r {
        TranscribeResult::Ok(text) => serde_json::from_str(text).expect("the stand-in's JSON"),
        other => panic!("expected an answer, got {other:?}"),
    }
}

#[derive(Clone)]
struct Knobs {
    cpu: bool,
    gpu: bool,
    nvidia: bool,
    cpu_flags: Vec<String>,
    gpu_flags: Vec<String>,
}

impl Default for Knobs {
    fn default() -> Self {
        Knobs {
            cpu: true,
            gpu: false,
            nvidia: false,
            cpu_flags: Vec::new(),
            gpu_flags: Vec::new(),
        }
    }
}

type Launched = Arc<Mutex<Vec<(EngineKind, PathBuf, Vec<String>)>>>;

struct World {
    engine: Engine,
    knobs: Arc<Mutex<Knobs>>,
    launched: Launched,
    cpu_dir: tempfile::TempDir,
    gpu_dir: tempfile::TempDir,
    job: Arc<Job>,
}

fn flags(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn world(knobs: Knobs) -> World {
    world_with(knobs, |_| {})
}

fn world_with(knobs: Knobs, tune: impl FnOnce(&mut EngineConfig)) -> World {
    world_in_job(knobs, tune, Job::new().expect("a job object"))
}

fn world_in_job(knobs: Knobs, tune: impl FnOnce(&mut EngineConfig), job: Job) -> World {
    // A space in the folder on purpose: the real one is under a user's profile.
    let cpu_dir = tempfile::Builder::new()
        .prefix("flare dictation cpu ")
        .tempdir()
        .expect("a temp folder");
    let gpu_dir = tempfile::Builder::new()
        .prefix("flare dictation gpu ")
        .tempdir()
        .expect("a temp folder");
    let knobs = Arc::new(Mutex::new(knobs));
    let launched: Launched = Arc::new(Mutex::new(Vec::new()));
    let (cpu_path, gpu_path) = (cpu_dir.path().to_path_buf(), gpu_dir.path().to_path_buf());
    let (k1, k2) = (Arc::clone(&knobs), Arc::clone(&knobs));
    let cpu_for = cpu_path.clone();
    let gpu_for = gpu_path.clone();
    let mut cfg = EngineConfig::new(
        Box::new(move || k1.lock().unwrap().cpu.then(|| cpu_for.clone())),
        Box::new(move || k2.lock().unwrap().gpu.then(|| gpu_for.clone())),
        Box::new(model_path),
    );
    cfg.runner_for = Box::new(|id| {
        if id == "pk" {
            Runner::Parakeet
        } else {
            Runner::Whisper
        }
    });
    let k3 = Arc::clone(&knobs);
    cfg.has_nvidia = Box::new(move || k3.lock().unwrap().nvidia);
    cfg.threads = 6;
    let (k4, l4) = (Arc::clone(&knobs), Arc::clone(&launched));
    cfg.command = Some(Box::new(move |exe: &Path, args: &[String]| {
        let kind = if exe.starts_with(&gpu_path) {
            EngineKind::Gpu
        } else {
            EngineKind::Cpu
        };
        l4.lock()
            .unwrap()
            .push((kind, exe.to_path_buf(), args.to_vec()));
        let k = k4.lock().unwrap();
        let mut all = args.to_vec();
        all.extend(match kind {
            EngineKind::Gpu => k.gpu_flags.clone(),
            EngineKind::Cpu => k.cpu_flags.clone(),
        });
        (PathBuf::from(FAKE), all)
    }));
    tune(&mut cfg);
    let job = Arc::new(job);
    World {
        engine: Engine::start(cfg, Arc::clone(&job)),
        job,
        knobs,
        launched,
        cpu_dir,
        gpu_dir,
    }
}

impl World {
    fn set(&self, f: impl FnOnce(&mut Knobs)) {
        f(&mut self.knobs.lock().unwrap());
    }

    fn launched_kinds(&self) -> Vec<EngineKind> {
        self.launched.lock().unwrap().iter().map(|l| l.0).collect()
    }

    fn launched_count(&self) -> usize {
        self.launched.lock().unwrap().len()
    }

    fn pid(&self, i: usize) -> u32 {
        self.engine.launched_pids()[i]
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
                    panic!("a server outlived its test: {pids:?}");
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

fn req(model: &str, language: &str, final_: bool) -> TranscribeRequest {
    TranscribeRequest {
        wav: vec![7u8; WAV_LEN],
        model_id: model.to_string(),
        language: language.to_string(),
        final_,
    }
}

fn base(final_: bool) -> TranscribeRequest {
    req("base", "auto", final_)
}

fn get(rx: &Receiver<TranscribeResult>) -> TranscribeResult {
    rx.recv_timeout(Duration::from_secs(30))
        .expect("the engine answers")
}

fn ask(w: &World, r: TranscribeRequest) -> TranscribeResult {
    get(&w.engine.transcribe(r))
}

fn warm(w: &World) -> TranscribeResult {
    get(&w.engine.warm("base", "auto"))
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

fn info(running: bool, engine: Option<EngineKind>, gpu_fell_back: bool) -> EngineInfo {
    EngineInfo {
        running,
        engine,
        gpu_fell_back,
    }
}

fn same_dir(seen_cwd: &str, dir: &Path) -> bool {
    let norm = |p: &Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_lowercase()
    };
    norm(Path::new(seen_cwd)) == norm(dir)
}

// ---------------------------------------------------------------- what can answer

#[test]
fn says_no_model_for_a_model_that_is_not_installed_and_starts_nothing() {
    let w = world(Knobs::default());
    assert_eq!(ask(&w, req("large", "auto", true)), no(Reason::NoModel));
    assert_eq!(w.launched_count(), 0);
}

#[test]
fn says_no_engine_when_there_is_neither_a_bundled_engine_nor_a_usable_gpu_pack() {
    let none = world(Knobs {
        cpu: false,
        ..Knobs::default()
    });
    assert_eq!(ask(&none, base(true)), no(Reason::NoEngine));
    // A GPU pack without an NVIDIA adapter is not an engine either.
    let pack_only = world(Knobs {
        cpu: false,
        gpu: true,
        nvidia: false,
        ..Knobs::default()
    });
    assert_eq!(ask(&pack_only, base(true)), no(Reason::NoEngine));
    assert_eq!(pack_only.launched_count(), 0);
}

#[test]
fn takes_the_gpu_pack_when_it_is_installed_and_an_nvidia_adapter_is_present() {
    let w = world(Knobs {
        gpu: true,
        nvidia: true,
        ..Knobs::default()
    });
    let r = ask(&w, base(true));
    assert!(same_dir(
        seen(&r).cwd.as_deref().unwrap_or(""),
        w.gpu_dir.path()
    ));
    assert_eq!(w.engine.info(), info(true, Some(EngineKind::Gpu), false));
}

#[test]
fn stays_on_cpu_with_the_pack_installed_and_no_nvidia_adapter_or_an_adapter_nobody_could_ask_about()
{
    // The original's second case is an adapter question that THREW; here a question that cannot be
    // answered reads as no (`has_nvidia` returns false), which is the same outcome.
    let w = world(Knobs {
        gpu: true,
        nvidia: false,
        ..Knobs::default()
    });
    ask(&w, base(true));
    assert_eq!(w.engine.info(), info(true, Some(EngineKind::Cpu), false));
    assert_eq!(w.launched_kinds(), vec![EngineKind::Cpu]);
}

// ---------------------------------------------------------------- the server

#[test]
fn is_not_started_until_the_first_transcribe_and_is_started_the_way_the_real_one_is() {
    let w = world(Knobs::default());
    assert_eq!(w.engine.info(), info(false, None, false));
    assert_eq!(w.launched_count(), 0);

    let s = seen(&ask(&w, req("base", "nb", true)));
    assert_eq!(w.launched_count(), 1);
    let (_, exe, args) = w.launched.lock().unwrap()[0].clone();
    assert_eq!(exe, w.cpu_dir.path().join("whisper-server.exe"));
    assert_eq!(
        args,
        flags(&[
            "-m",
            r"C:\Flare Dictation\models\ggml-base.bin",
            "--host",
            "127.0.0.1",
            "--port",
            &s.port.to_string(),
            "-l",
            "nb"
        ])
    );
    assert!(s.port > 0);
    // Its DLLs are beside it, so it runs IN its own folder.
    assert!(same_dir(s.cwd.as_deref().unwrap_or(""), w.cpu_dir.path()));
    assert_eq!(w.engine.info(), info(true, Some(EngineKind::Cpu), false));
}

#[test]
fn is_sent_the_wav_as_multipart_form_data_and_asked_for_json() {
    let w = world(Knobs::default());
    let s = seen(&ask(&w, base(true)));
    assert_eq!(s.bytes, i64::try_from(WAV_LEN).unwrap());
    assert_eq!(s.filename.as_deref(), Some("audio.wav"));
    assert_eq!(s.r#type.as_deref(), Some("audio/wav"));
    assert_eq!(s.format.as_deref(), Some("json"));
    assert_eq!(
        s.model.as_deref(),
        Some(r"C:\Flare Dictation\models\ggml-base.bin")
    );
    assert_eq!(s.language.as_deref(), Some("auto"));
    assert_eq!(s.host, "127.0.0.1");
    assert_eq!(w.engine.info().engine, Some(EngineKind::Cpu));
}

#[test]
fn is_reused_from_one_pass_to_the_next() {
    let w = world(Knobs::default());
    assert!(warm(&w).is_ok());
    let a = seen(&ask(&w, base(false)));
    let b = seen(&ask(&w, base(true)));
    assert_eq!(b.pid, a.pid);
    assert_eq!(b.n, 3);
    assert_eq!(w.launched_count(), 1);
}

#[test]
fn is_replaced_when_the_model_the_language_or_the_engine_kind_changes() {
    let w = world(Knobs {
        nvidia: true,
        ..Knobs::default()
    });
    let first = seen(&ask(&w, base(true)));
    let model = seen(&ask(&w, req("small", "auto", true)));
    assert_ne!(model.pid, first.pid);
    assert_eq!(
        model.model.as_deref(),
        Some(r"C:\Flare Dictation\models\ggml-small.bin")
    );
    let lang = seen(&ask(&w, req("small", "en", true)));
    assert_ne!(lang.pid, model.pid);
    assert_eq!(lang.language.as_deref(), Some("en"));
    // The GPU pack finishes downloading mid-session: the next pass moves to it.
    w.set(|k| k.gpu = true);
    let kind = seen(&ask(&w, req("small", "en", true)));
    assert_ne!(kind.pid, lang.pid);
    assert_eq!(w.engine.info().engine, Some(EngineKind::Gpu));
    assert_eq!(
        w.launched_kinds(),
        vec![
            EngineKind::Cpu,
            EngineKind::Cpu,
            EngineKind::Cpu,
            EngineKind::Gpu
        ]
    );
    // One server at a time: the three it replaced are gone.
    until(|| (0..3).all(|i| pid_gone(w.pid(i))), 4000);
    assert!(!pid_gone(w.pid(3)));
}

// ---------------------------------------------------------------- a GPU engine that will not start

#[test]
fn falls_back_to_cpu_for_the_life_of_the_engine_when_it_exits_at_start_and_answers_from_there() {
    let w = world(Knobs {
        gpu: true,
        nvidia: true,
        gpu_flags: flags(&["--fake-exit"]),
        ..Knobs::default()
    });
    assert!(ask(&w, base(true)).is_ok());
    assert_eq!(w.engine.info(), info(true, Some(EngineKind::Cpu), true));
    // Never again this session: the second pass does not even try it.
    assert!(ask(&w, base(true)).is_ok());
    assert_eq!(w.launched_kinds(), vec![EngineKind::Gpu, EngineKind::Cpu]);
}

#[test]
fn falls_back_the_same_way_when_it_never_becomes_ready_and_the_stuck_one_is_killed() {
    let w = world_with(
        Knobs {
            gpu: true,
            nvidia: true,
            gpu_flags: flags(&["--fake-never-ready"]),
            ..Knobs::default()
        },
        |c| c.start_timeout = Duration::from_millis(400),
    );
    assert!(ask(&w, base(true)).is_ok());
    assert_eq!(w.engine.info().engine, Some(EngineKind::Cpu));
    assert!(w.engine.info().gpu_fell_back);
    until(|| pid_gone(w.pid(0)), 4000);
}

#[test]
fn a_stranger_answering_on_the_engines_port_is_never_trusted() {
    // A program that grabbed the port the engine picked answers GET / with 200. The real server (here
    // the stand-in that never listens) is not the owner, so the engine must not take that for ready.
    let squatter = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = squatter.local_addr().expect("addr").port();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    let posts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let posts2 = Arc::clone(&posts);
    squatter.set_nonblocking(true).expect("nonblocking");
    let t = std::thread::spawn(move || {
        use std::io::{Read, Write};
        while !stop2.load(std::sync::atomic::Ordering::SeqCst) {
            if let Ok((mut s, _)) = squatter.accept() {
                let mut b = [0u8; 512];
                let _ = s.set_nonblocking(false);
                let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
                let n = s.read(&mut b).unwrap_or(0);
                if b[..n].starts_with(b"POST") {
                    posts2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    });
    let w = world_with(
        Knobs {
            cpu_flags: flags(&["--fake-never-ready"]),
            ..Knobs::default()
        },
        |c| {
            c.start_timeout = Duration::from_millis(1500);
            c.port = Some(Box::new(move || Ok(port)));
        },
    );
    let r = ask(&w, base(true));
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    t.join().expect("squatter");
    assert_eq!(
        posts.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "audio went to a stranger"
    );
    assert!(!r.is_ok(), "{r:?}");
}
#[test]
fn is_a_failure_not_a_loop_when_there_is_no_cpu_engine_to_fall_back_to() {
    let w = world(Knobs {
        cpu: false,
        gpu: true,
        nvidia: true,
        gpu_flags: flags(&["--fake-exit"]),
        ..Knobs::default()
    });
    let r = ask(&w, base(true));
    assert!(failed_detail(&r).contains("CUDA error"), "{r:?}");
    assert_eq!(w.launched_count(), 1);
    // The GPU engine is out for the session, so now nothing is left at all.
    assert_eq!(ask(&w, base(true)), no(Reason::NoEngine));
    assert_eq!(w.launched_count(), 1);
}

// ---------------------------------------------------------------- a CPU engine that will not start

#[test]
fn fails_that_request_with_what_the_server_said_once_and_tries_afresh_on_the_next() {
    let w = world(Knobs {
        cpu_flags: flags(&["--fake-exit"]),
        ..Knobs::default()
    });
    let r = ask(&w, base(true));
    assert!(failed_detail(&r).contains("fake: CUDA error"), "{r:?}");
    assert_eq!(w.launched_count(), 1);
    assert!(!w.engine.info().running);

    w.set(|k| k.cpu_flags.clear());
    assert!(ask(&w, base(true)).is_ok());
    assert_eq!(w.launched_count(), 2);
}

#[test]
fn fails_when_the_exe_cannot_be_launched_at_all() {
    let w = world_with(Knobs::default(), |c| {
        c.command = Some(Box::new(|_, _| {
            (
                std::env::temp_dir().join("no such whisper-server.exe"),
                Vec::new(),
            )
        }));
    });
    assert!(is_engine_failed(&ask(&w, base(true))));
}

// ---------------------------------------------------------------- passes are serialised

#[test]
fn drops_a_waiting_partial_for_a_newer_one_and_never_sends_two_at_once() {
    let w = world(Knobs {
        cpu_flags: flags(&["--fake-delay", "120"]),
        ..Knobs::default()
    });
    assert!(warm(&w).is_ok());
    let a = w.engine.transcribe(base(false));
    let b = w.engine.transcribe(base(false));
    let c = w.engine.transcribe(base(false));
    // b never reaches the server: it answers as soon as c takes its place.
    assert_eq!(get(&b), no(Reason::Superseded));
    assert_eq!(seen(&get(&a)).n, 2);
    let last = seen(&get(&c));
    assert_eq!(last.n, 3);
    assert_eq!(last.max_active, 1);
}

#[test]
fn never_drops_a_final_and_lets_it_jump_ahead_of_a_waiting_partial() {
    let w = world(Knobs {
        cpu_flags: flags(&["--fake-delay", "120"]),
        ..Knobs::default()
    });
    assert!(warm(&w).is_ok());
    let in_flight = w.engine.transcribe(base(false));
    let waiting = w.engine.transcribe(base(false));
    let final1 = w.engine.transcribe(base(true));
    // The next recording has begun before the last one's final was answered.
    let next_partial = w.engine.transcribe(base(false));
    let final2 = w.engine.transcribe(base(true));
    let newest = w.engine.transcribe(base(false));

    // A partial waiting when a final arrives is for a clip the final covers whole.
    assert_eq!(get(&waiting), no(Reason::Superseded));
    assert_eq!(get(&next_partial), no(Reason::Superseded));
    assert_eq!(seen(&get(&in_flight)).n, 2);
    assert_eq!(seen(&get(&final1)).n, 3);
    assert_eq!(seen(&get(&final2)).n, 4);
    let end = seen(&get(&newest));
    assert_eq!(end.n, 5);
    assert_eq!(end.max_active, 1);
}

// ---------------------------------------------------------------- warming up

// THE ENGINE IS WARM BEFORE ANYBODY SPEAKS (MEASURED on an RTX 5090: the first pass of a cold GPU
// engine compiled kernels for 31.8 s, and the partials queued behind it showed nothing for as long).

#[test]
fn answers_a_partial_on_a_cold_engine_at_once_with_warming_and_warms_it() {
    let w = world(Knobs::default());
    assert_eq!(ask(&w, base(false)), no(Reason::Warming));
    until(|| w.launched_count() == 1, 6000);
    // The warm-up pass lands; the next partial is served by the same server.
    let end = Instant::now() + Duration::from_secs(6);
    loop {
        if ask(&w, base(false)).is_ok() {
            break;
        }
        assert!(Instant::now() < end, "never served");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(w.launched_count(), 1);
}

#[test]
fn is_asked_for_freely_one_server_and_one_silent_pass_however_often() {
    let w = world(Knobs {
        cpu_flags: flags(&["--fake-delay", "100"]),
        ..Knobs::default()
    });
    let first = w.engine.warm("base", "auto");
    assert_eq!(get(&w.engine.warm("base", "auto")), no(Reason::Warming));
    assert!(get(&first).is_ok());
    assert!(warm(&w).is_ok());
    // The final is the SECOND pass the server ever saw: the warm-up was the first.
    assert_eq!(seen(&ask(&w, base(true))).n, 2);
    assert_eq!(w.launched_count(), 1);
}

#[test]
fn a_final_is_never_refused_while_warming_it_waits_for_the_warm_server() {
    let w = world(Knobs {
        cpu_flags: flags(&["--fake-delay", "100"]),
        ..Knobs::default()
    });
    let _warming = w.engine.warm("base", "auto");
    let fin = seen(&ask(&w, base(true)));
    assert_eq!(fin.n, 2);
    assert_eq!(fin.max_active, 1);
}

#[test]
fn gives_the_gpu_engine_its_own_kernel_cache_with_room_and_the_cpu_one_nothing() {
    let cache = PathBuf::from(r"C:\Flare Dictation\cuda-cache");
    let c1 = cache.clone();
    let g = world_with(
        Knobs {
            gpu: true,
            nvidia: true,
            ..Knobs::default()
        },
        move |c| c.cuda_cache = Some(c1),
    );
    let gpu = seen(&ask(&g, base(true)));
    assert_eq!(gpu.cuda_cache.as_deref(), cache.to_str());
    assert!(
        gpu.cuda_max
            .as_deref()
            .unwrap_or("0")
            .parse::<u64>()
            .unwrap()
            >= 4 * 1024 * 1024 * 1024
    );
    let c2 = cache.clone();
    let c = world_with(Knobs::default(), move |c| c.cuda_cache = Some(c2));
    let cpu = seen(&ask(&c, base(true)));
    // Whatever this PC's environment holds, the CPU engine is never handed OUR cache.
    assert_ne!(cpu.cuda_cache.as_deref(), cache.to_str());
}

#[test]
fn after_a_failed_warm_up_a_partial_goes_the_ordinary_way_and_says_what_failed() {
    let w = world(Knobs {
        cpu_flags: flags(&["--fake-exit"]),
        ..Knobs::default()
    });
    assert!(!warm(&w).is_ok());
    assert!(is_engine_failed(&ask(&w, base(false))));
}

// Perf review 2026-10-16: each partial on a broken engine launched a server.
#[test]
fn after_a_server_would_not_start_a_held_key_starts_no_more_servers_a_final_still_tries() {
    let w = world(Knobs {
        cpu_flags: flags(&["--fake-exit"]),
        ..Knobs::default()
    });
    assert!(!warm(&w).is_ok());
    assert_eq!(w.launched_count(), 1);
    for _ in 0..4 {
        let r = ask(&w, base(false));
        assert!(failed_detail(&r).contains("fake: CUDA error"), "{r:?}");
    }
    assert_eq!(w.launched_count(), 1);
    w.set(|k| k.cpu_flags.clear());
    assert!(ask(&w, base(true)).is_ok());
    assert_eq!(w.launched_count(), 2);
}

// ---------------------------------------------------------------- standing down

#[test]
fn kills_the_server_after_idle_with_no_request_and_starts_a_fresh_one_on_the_next() {
    let w = world_with(Knobs::default(), |c| {
        c.idle = Some(Duration::from_millis(150))
    });
    let first = seen(&ask(&w, base(true)));
    until(|| pid_gone(w.pid(0)), 4000);
    assert_eq!(w.engine.info(), info(false, Some(EngineKind::Cpu), false));

    let second = seen(&ask(&w, base(true)));
    assert_ne!(second.pid, first.pid);
    assert_eq!(w.launched_count(), 2);
}

// The dictation service (N5): `None` never stands down; stop() still ends it.
#[test]
fn never_stands_down_with_no_idle() {
    let w = world_with(Knobs::default(), |c| c.idle = None);
    ask(&w, base(true));
    std::thread::sleep(Duration::from_millis(300));
    assert!(w.engine.info().running);
    w.engine.stop();
    until(|| pid_gone(w.pid(0)), 4000);
}

#[test]
fn counts_the_idle_time_from_the_last_request_not_the_first() {
    let w = world_with(Knobs::default(), |c| {
        c.idle = Some(Duration::from_millis(1500))
    });
    ask(&w, base(true));
    std::thread::sleep(Duration::from_millis(900));
    ask(&w, base(true));
    std::thread::sleep(Duration::from_millis(900));
    // 1800 ms since the first pass, 900 since the second: still up.
    assert!(w.engine.info().running);
    assert_eq!(w.launched_count(), 1);
    until(|| pid_gone(w.pid(0)), 6000);
}

#[test]
fn does_not_stand_down_under_a_pass_that_takes_longer_than_idle() {
    let w = world_with(
        Knobs {
            cpu_flags: flags(&["--fake-delay", "250"]),
            ..Knobs::default()
        },
        |c| c.idle = Some(Duration::from_millis(60)),
    );
    assert!(ask(&w, base(true)).is_ok());
}

// ---------------------------------------------------------------- stop()

#[test]
fn stop_kills_the_server_at_once_and_fails_what_was_in_flight_and_what_was_waiting() {
    let w = world(Knobs {
        cpu_flags: flags(&["--fake-delay", "2000"]),
        ..Knobs::default()
    });
    assert!(warm(&w).is_ok());
    let in_flight = w.engine.transcribe(base(true));
    let waiting_final = w.engine.transcribe(base(true));
    let waiting_partial = w.engine.transcribe(base(false));
    // Long enough for the server to be holding the first pass.
    std::thread::sleep(Duration::from_millis(250));
    w.engine.stop();
    assert!(!w.engine.info().running);
    for p in [&in_flight, &waiting_final, &waiting_partial] {
        assert!(is_engine_failed(&get(p)));
    }
    until(|| pid_gone(w.pid(0)), 4000);
}

#[test]
fn stop_leaves_the_engine_usable_switched_back_on_the_next_pass_starts_a_server() {
    let w = world(Knobs::default());
    let first = seen(&ask(&w, base(true)));
    w.engine.stop();
    let second = seen(&ask(&w, base(true)));
    assert_ne!(second.pid, first.pid);
}

#[test]
fn stop_during_a_gpu_start_is_a_stop_not_a_failed_gpu_no_fallback_no_cpu_server() {
    let w = world_with(
        Knobs {
            gpu: true,
            nvidia: true,
            gpu_flags: flags(&["--fake-never-ready"]),
            ..Knobs::default()
        },
        |c| c.start_timeout = Duration::from_millis(5000),
    );
    let p = w.engine.transcribe(base(true));
    until(|| w.engine.launched_pids().len() == 1, 6000);
    w.engine.stop();
    assert!(is_engine_failed(&get(&p)));
    assert_eq!(w.engine.info(), info(false, None, false));
    assert_eq!(w.launched_kinds(), vec![EngineKind::Gpu]);
}

#[test]
fn a_pass_that_never_answers_is_killed_and_says_so() {
    let w = world_with(
        Knobs {
            cpu_flags: flags(&["--fake-delay", "5000"]),
            ..Knobs::default()
        },
        |c| c.request_timeout = Duration::from_millis(300),
    );
    let r = ask(&w, base(true));
    assert!(
        failed_detail(&r).contains("no answer after 300 ms"),
        "{r:?}"
    );
    until(|| pid_gone(w.pid(0)), 4000);
}

// ---------------------------------------------------------------- a server that dies mid-request

#[test]
fn fails_that_request_with_the_tail_of_its_stderr_and_the_next_one_gets_a_fresh_server() {
    let w = world(Knobs {
        cpu_flags: flags(&["--fake-die-on-inference"]),
        ..Knobs::default()
    });
    let r = ask(&w, base(true));
    assert!(failed_detail(&r).contains("out of memory"), "{r:?}");
    assert!(!w.engine.info().running);

    w.set(|k| k.cpu_flags.clear());
    assert!(ask(&w, base(true)).is_ok());
    assert_eq!(w.launched_count(), 2);
}

#[test]
fn counts_against_a_gpu_engine_too_the_next_pass_goes_to_the_cpu_one() {
    let w = world(Knobs {
        gpu: true,
        nvidia: true,
        gpu_flags: flags(&["--fake-die-on-inference"]),
        ..Knobs::default()
    });
    assert!(is_engine_failed(&ask(&w, base(true))));
    assert!(w.engine.info().gpu_fell_back);
    assert!(ask(&w, base(true)).is_ok());
    assert_eq!(w.engine.info().engine, Some(EngineKind::Cpu));
    assert_eq!(w.launched_kinds(), vec![EngineKind::Gpu, EngineKind::Cpu]);
}

#[test]
fn does_not_take_the_requests_waiting_behind_it_down_with_it() {
    let w = world(Knobs {
        cpu_flags: flags(&["--fake-die-on-inference"]),
        ..Knobs::default()
    });
    let dies = w.engine.transcribe(base(true));
    let waits = w.engine.transcribe(base(true));
    until(|| w.engine.launched_pids().len() == 1, 6000);
    w.set(|k| k.cpu_flags.clear());
    assert!(!get(&dies).is_ok());
    assert!(get(&waits).is_ok());
}

// ---------------------------------------------------------------- the review's two findings

#[test]
fn warm_and_a_cold_partial_never_ask_where_the_model_is_on_the_callers_thread() {
    // `model_path` is a disk check in the app: the caller (UI) thread must never run it.
    let callers: Arc<Mutex<Vec<std::thread::ThreadId>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_by = Arc::clone(&callers);
    let w = world_with(Knobs::default(), move |cfg| {
        cfg.model_path = Box::new(move |id| {
            seen_by.lock().unwrap().push(std::thread::current().id());
            model_path(id)
        });
    });
    let me = std::thread::current().id();
    assert!(warm(&w).is_ok());
    // Warm again (the server is up and warm now) and ask a partial: both answer from state alone.
    assert!(warm(&w).is_ok());
    assert!(ask(&w, base(false)).is_ok());
    assert!(
        !callers.lock().unwrap().contains(&me),
        "model_path ran on the caller's thread"
    );
}

#[test]
fn a_server_that_could_not_join_the_job_is_killed_and_the_pass_fails() {
    let w = world_in_job(Knobs::default(), |_| {}, Job::unusable_for_tests());
    let r = ask(&w, base(true));
    assert!(is_engine_failed(&r), "got {r:?}");
    assert!(failed_detail(&r).contains("job"), "got {r:?}");
    assert!(w.launched_count() >= 1, "a server was tried");
    // The refused child is killed and reaped inside the launch, so none is ever recorded as running.
    assert!(w.engine.launched_pids().is_empty());
}

#[test]
fn every_server_is_in_the_job() {
    let w = world(Knobs::default());
    assert!(ask(&w, base(true)).is_ok());
    assert!(w.job.contains(w.pid(0)), "the server is not in the job");
}

// ---------------------------------------------------------------- audit 2026-10-20 (A18)

// L30: a partial is served inline only by a server warm for ITS model and language; any other partial
// is answered Warming at once and warms the right server, never a cold start on the caller's pass.
#[test]
fn a_partial_for_another_model_or_language_is_answered_warming_not_run_inline() {
    let w = world(Knobs::default());
    assert!(warm(&w).is_ok());
    assert_eq!(w.launched_count(), 1);
    assert_eq!(ask(&w, req("small", "auto", false)), no(Reason::Warming));
    assert_eq!(ask(&w, req("base", "en", false)), no(Reason::Warming));
    // Those answers started a warm-up, which replaces the base/auto server on the worker's own time:
    // whether it has yet is scheduling, so the test waits for that launch (the old server is let go
    // before the new one starts), then warms base/auto again (`warm` answers Warming while another
    // warm-up is queued or running, and Ok once base/auto is warm).
    until(|| w.launched_count() >= 2, 30_000);
    until(
        || {
            let r = warm(&w);
            assert!(
                r.is_ok() || r == no(Reason::Warming),
                "a warm-up failed: {r:?}"
            );
            r.is_ok()
        },
        30_000,
    );
    // The right partial now goes straight through the warm server: no launch on the caller's pass.
    let launched = w.launched_count();
    let r = ask(&w, base(false));
    assert!(r.is_ok(), "got {r:?}");
    assert_eq!(w.launched_count(), launched, "the partial started a server");
}

// L33: dropping the engine (dictation switched off, on the UI thread) never waits for the worker.
#[test]
fn dropping_the_engine_does_not_wait_for_a_worker_stuck_in_a_launch() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = tempfile::tempdir().expect("a temp folder");
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let dir_path = dir.path().to_path_buf();
    let mut cfg = EngineConfig::new(
        Box::new(move || Some(dir_path.clone())),
        Box::new(|| None),
        Box::new(model_path),
    );
    cfg.runner_for = Box::new(|_| Runner::Whisper);
    cfg.threads = 6;
    let (e, r) = (Arc::clone(&entered), Arc::clone(&release));
    cfg.command = Some(Box::new(move |_exe: &Path, args: &[String]| {
        e.store(true, Ordering::SeqCst);
        let end = Instant::now() + Duration::from_secs(4);
        while !r.load(Ordering::SeqCst) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        (PathBuf::from(FAKE), args.to_vec())
    }));
    let job = Arc::new(Job::new().expect("a job object"));
    let engine = Engine::start(cfg, job);
    let _rx = engine.transcribe(base(true));
    until(|| entered.load(Ordering::SeqCst), 6000);
    let began = Instant::now();
    drop(engine);
    let took = began.elapsed();
    release.store(true, Ordering::SeqCst);
    assert!(
        took < Duration::from_millis(1000),
        "dropping the engine waited {took:?} for the worker"
    );
}
