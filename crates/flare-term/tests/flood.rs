//! The runaway `cat`: a 100 MB flood keeps snapshots fast and arrives complete, Ctrl+C stops an
//! endless writer, and the parser alone keeps up.
//! Windows-only until macOS phase 2 (the real bundled ConPTY and the Windows shells).
#![cfg(windows)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::{Rig, temp_dir_named};
use flare_term::emulator::Emulator;

const SPEW: &str = env!("CARGO_BIN_EXE_flare_spew");
const BODY: &str = "The quick brown fox jumps over the lazy dog 0123456789 The quick brown fox jumps over the lazy dog 0123456789";

/// 500 spew lines, the same chunk `flare_spew` writes.
fn spew_chunk() -> String {
    let mut s = String::with_capacity(500 * 120);
    for i in 0..500 {
        s.push_str(&format!("{i:07} {BODY}\r\n"));
    }
    s
}

/// Starts the sampler: times `snapshot()` every 5 ms until `stop`, returns the worst in ms.
fn start_sampler(rig: &Rig, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<(f64, usize)> {
    let emu = Arc::clone(&rig.emu);
    std::thread::spawn(move || {
        let mut worst = 0.0f64;
        let mut n = 0usize;
        while !stop.load(Ordering::SeqCst) {
            let t = Instant::now();
            let grid = emu.lock().snapshot();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            drop(grid);
            worst = worst.max(ms);
            n += 1;
            std::thread::sleep(Duration::from_millis(5)); // sync-ok: test sampler thread
        }
        (worst, n)
    })
}

fn done_visible(e: &Emulator) -> bool {
    (0..e.rows()).any(|r| {
        e.row_text(i32::try_from(r).unwrap_or(0))
            .contains("SPEW-DONE")
    })
}

/// Runs a flood of `mb` MB at 120x40; returns (seconds, worst snapshot ms, samples).
fn run_flood(mb: u32, wait_ms: u64) -> (f64, f64, usize) {
    let dir = temp_dir_named("flare-m2-flood");
    let rig = Rig::spawn(SPEW, &[&mb.to_string()], dir.path(), None, 120, 40);
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = start_sampler(&rig, Arc::clone(&stop));
    let t0 = Instant::now();
    let done = rig.wait_for(wait_ms, done_visible);
    let secs = t0.elapsed().as_secs_f64();
    stop.store(true, Ordering::SeqCst);
    let (worst, samples) = sampler.join().expect("the sampler ends");
    assert!(
        done,
        "no SPEW-DONE in {wait_ms} ms; screen {:?}",
        rig.screen()
    );
    if mb == 100 {
        // The last chunk ends with line 499, then SPEW-DONE.
        let screen = rig.screen();
        let at = screen
            .iter()
            .position(|r| r.contains("SPEW-DONE"))
            .expect("SPEW-DONE is on the screen");
        assert!(at > 0, "SPEW-DONE is on the first row; screen {screen:?}");
        assert!(
            screen[at - 1].starts_with("0000499"),
            "the row above SPEW-DONE is {:?}",
            screen[at - 1]
        );
    }
    (secs, worst, samples)
}

#[test]
fn a_100_mb_flood_keeps_snapshots_fast_and_arrives_complete() {
    let (secs, worst, samples) = run_flood(100, 90_000);
    println!(
        "flood: 100 MB in {secs:.2} s = {:.1} MB/s; worst snapshot {worst:.1} ms over {samples} samples",
        100.0 / secs
    );
    let limit = if cfg!(debug_assertions) { 250.0 } else { 100.0 };
    assert!(
        worst < limit,
        "the worst snapshot took {worst:.1} ms (limit {limit} ms)"
    );
}

/// A test runner can start us with Ctrl+C ignored, and children inherit that flag. The app is a GUI
/// process with no such flag, so the test clears it to see what a real tab sees.
fn clear_inherited_ctrl_c_ignore() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    // SAFETY: a null handler with FALSE restores normal Ctrl+C processing for this process.
    unsafe { SetConsoleCtrlHandler(None, 0) };
}

#[test]
fn ctrl_c_reaches_a_flooding_program() {
    clear_inherited_ctrl_c_ignore();
    let dir = temp_dir_named("flare-m2-ctrlc");
    let mut rig = Rig::spawn(SPEW, &["0"], dir.path(), None, 120, 40);
    std::thread::sleep(Duration::from_millis(500)); // sync-ok: let the flood start
    rig.write(&[0x03]);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut code = None;
    while Instant::now() < deadline {
        code = rig.exit_code();
        if code.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50)); // sync-ok: polling the exit code
    }
    assert!(
        code.is_some(),
        "the endless writer ran on 5 s after Ctrl+C; screen {:?}",
        rig.screen().first()
    );
}

#[test]
fn the_parser_alone_keeps_up() {
    if cfg!(debug_assertions) {
        println!("skipped in debug");
        return;
    }
    let chunk = spew_chunk();
    let bytes = chunk.as_bytes();
    let mut stream = Vec::with_capacity(64 * 1024 + bytes.len());
    while stream.len() < 64 * 1024 {
        stream.extend_from_slice(bytes);
    }
    let mut emu = Emulator::new(120, 40);
    let total: usize = 64_000_000;
    let mut fed = 0usize;
    let t = Instant::now();
    while fed < total {
        let n = stream.len().min(total - fed);
        let _ = emu.feed(&stream[..n]);
        fed += n;
    }
    let secs = t.elapsed().as_secs_f64();
    println!("parser alone: 64 MB in {secs:.2} s");
    assert!(secs < 1.5, "64 MB took {secs:.2} s to parse");
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn bench_throughput() {
    let chunk = spew_chunk();
    let mut emu = Emulator::new(120, 40);
    let t = Instant::now();
    let mut fed = 0usize;
    while fed < 64_000_000 {
        let _ = emu.feed(chunk.as_bytes());
        fed += chunk.len();
    }
    let parse = t.elapsed().as_secs_f64();
    let (secs, worst, _) = run_flood(64, 90_000);
    println!(
        "bench: end-to-end {:.1} MB/s ({secs:.2} s), parse {parse:.2} s, worst snapshot {worst:.1} ms",
        64.0 / secs
    );
}
