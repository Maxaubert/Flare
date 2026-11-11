//! M5 numbers (Task 28 of `docs/superpowers/plans/2026-10-17-flare-m5.md`): timings of the
//! input paths, printed and not gated. Run on a release build with:
//!
//! `cargo test --release -p flare-term --test m5_numbers -- --ignored --nocapture`

use std::time::Instant;

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use flare_term::emulator::{DecorReq, Emulator};
use flare_term::find::{CountJob, FindOpts, Finder};
use flare_term::sel_text::selection_text;
use flare_term::term_buf::TermBuf;

fn line_text(i: usize) -> String {
    format!(
        "{i:06} the quick brown fox jumps over the lazy dog https://example.com/item/{i} \u{27E6}C:\\w\\src\\main.rs:{i}\u{27E7}"
    )
}

fn fill(emu: &mut Emulator, lines: usize) {
    let mut s = String::new();
    for i in 0..lines {
        s.push_str(&line_text(i));
        s.push_str("\r\n");
    }
    let _ = emu.feed(s.as_bytes());
}

fn decor_once(emu: &mut Emulator, find_open: bool) -> usize {
    let mut req = DecorReq {
        cwd: None,
        ink_key: 1,
        hover: None,
        find_open,
        ink: None,
    };
    let d = emu.decor(&mut req);
    d.links.len() + d.find.len() + d.selection.len()
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn m5_decor_200x60_with_links_selection_and_find() {
    let mut emu = Emulator::new(200, 60);
    fill(&mut emu, 40);
    let mut sel = Selection::new(
        SelectionType::Simple,
        Point::new(Line(2), Column(0)),
        Side::Left,
    );
    sel.update(Point::new(Line(5), Column(40)), Side::Right);
    emu.term_mut().selection = Some(sel);
    emu.find_set("example");

    let t = Instant::now();
    let painted = decor_once(&mut emu, true);
    let first = t.elapsed().as_secs_f64() * 1000.0;

    let calls = 200;
    let t = Instant::now();
    for _ in 0..calls {
        let _ = decor_once(&mut emu, true);
    }
    let steady = t.elapsed().as_secs_f64() * 1000.0 / f64::from(calls);
    println!(
        "m5 decor 200x60 (40 link lines, selection, find bar): first call {first:.3} ms, \
         steady mean {steady:.4} ms over {calls} calls, {painted} spans"
    );
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn m5_link_scan_viewport_of_10000_lines_of_scrollback() {
    let mut emu = Emulator::new(120, 40);
    fill(&mut emu, 10_040);
    let history = emu.history_size();
    emu.scroll_view(i32::try_from(history).unwrap_or(i32::MAX));

    let t = Instant::now();
    let spans = decor_once(&mut emu, false);
    let cold = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "m5 link scan, viewport at the top of {history} lines of scrollback: first call {cold:.3} ms, \
         {spans} spans"
    );
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn m5_count_job_find_next_and_selection_text_over_10000_lines() {
    let mut emu = Emulator::new(120, 40);
    fill(&mut emu, 10_040);
    let total = emu.total_rows();
    let cols = emu.cols();
    let history = emu.history_size();
    println!("m5 buffer: {total} rows ({history} of history), {cols} columns");

    // CountJob: one whole pass in slices; the longest slice is what the UI thread waits for.
    let finder = Finder::new("fox", FindOpts::default()).expect("a non-empty query");
    let mut job = CountJob::new(finder, total);
    let clock_start = Instant::now();
    let clock = move || clock_start.elapsed().as_secs_f64() * 1000.0;
    let mut longest = 0.0_f64;
    let mut slices = 0_usize;
    let pass = Instant::now();
    loop {
        let t = Instant::now();
        let done = job.step(&TermBuf::all(emu.term()), cols, &clock);
        longest = longest.max(t.elapsed().as_secs_f64() * 1000.0);
        slices += 1;
        if done {
            break;
        }
    }
    let total_ms = pass.elapsed().as_secs_f64() * 1000.0;
    println!(
        "m5 CountJob over {total} rows: total {total_ms:.2} ms in {slices} slices, longest slice {longest:.3} ms, {} matches",
        job.results().len()
    );

    // find_next: the next match after the anchor, repeated.
    let calls = 20;
    let t = Instant::now();
    for _ in 0..calls {
        let _ = emu.find_next("fox", false);
    }
    let per = t.elapsed().as_secs_f64() * 1000.0 / f64::from(calls);
    println!("m5 find_next over {total} rows: mean {per:.3} ms per call over {calls} calls");

    // The worst find_next: a query with no match walks the whole buffer and wraps.
    let t = Instant::now();
    let none = emu.find_next("zzzz-not-there", false);
    let miss = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "m5 find_next with no match over {total} rows: {miss:.2} ms (found: {})",
        none.is_some()
    );

    // A dense query: every slice also sorts what it found so far (find_step's results()).
    emu.find_set("e");
    let t = Instant::now();
    let mut steps = 0_usize;
    let mut longest_step = 0.0_f64;
    let clock_start = Instant::now();
    let clock = move || clock_start.elapsed().as_secs_f64() * 1000.0;
    loop {
        let s = Instant::now();
        let _ = emu.find_step(&clock);
        longest_step = longest_step.max(s.elapsed().as_secs_f64() * 1000.0);
        steps += 1;
        if emu.find_view().done {
            break;
        }
    }
    println!(
        "m5 find_step for \"e\" over {total} rows: total {:.2} ms in {steps} steps, longest step {longest_step:.3} ms, {} highlighted",
        t.elapsed().as_secs_f64() * 1000.0,
        emu.find_view().count
    );

    // selection_text: every row of the buffer.
    let end_row = total.saturating_sub(1);
    let t = Instant::now();
    let text = selection_text(
        &TermBuf::all(emu.term()),
        cols,
        (0, 0),
        (end_row, cols),
        false,
    );
    let st = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "m5 selection_text over {total} rows: {st:.2} ms, {} characters",
        text.len()
    );
}
