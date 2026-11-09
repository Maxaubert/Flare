//! The go/no-go probe for pictures in the grid (issue #35): what the real bundled ConPTY does with
//! the graphics sequences. Findings and the verdict: `docs/graphics.md`.
//!
//! The program under ConPTY is this test binary run again with `SCENE_ENV` set and `stand_in_writer`
//! as its only test. It prints fixed bytes, makes a few console API calls and reads single bytes
//! from the test. No shell runs. Flare has no picture layer, so `replay` feeds the bytes ConPTY
//! delivered to a terminal that blots the picture's cells where a layer would have put them.

mod common;

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use common::{NamedDir, Rig, temp_dir_named};
use flare_term::emulator::Emulator;

const SCENE_ENV: &str = "FLARE_APC_CONPTY_SCENE";

const APC_PIC: &[u8] = b"\x1b_Ga=T,f=100,s=8,v=4,c=8,r=4;QUFB\x1b\\";
const OSC_PIC: &[u8] = b"\x1b]1337;File=inline=1;size=3;width=8;height=4:QUFB\x07";
const CELLS_PIC: &[u8] = b"\x1b_Ga=T,U=1,i=7,f=100,s=8,v=4,c=8,r=4;QUFB\x1b\\";
const PIC_ROWS: usize = 4;
const BLOT: &str = "########";
const CELL: char = '\u{10EEEE}';

/// A 2 000 000 byte OSC 1337 body: the header, then a fixed filler.
fn big_osc_1337() -> Vec<u8> {
    let mut seq = b"\x1b]1337;File=inline=1;size=1500000;width=4;height=4:".to_vec();
    seq.extend(std::iter::repeat_n(b'Q', 2_000_000));
    seq.push(0x07);
    seq
}

enum Step {
    /// Fixed bytes to stdout.
    Out(Vec<u8>),
    /// Wait for one byte from the test.
    Wait,
    /// Ask the console where the cursor is, set it there through the API (what a line editor
    /// does), and print `text`.
    Prompt(&'static [u8]),
    /// Print the console's own idea of the window size.
    Report,
    /// Show a second console screen buffer for a moment, then come back.
    SwapBuffers,
}

/// Four rows of cells the program prints itself (kitty's Unicode placeholders).
fn cell_rows() -> Vec<u8> {
    let cell = format!("{CELL}\u{305}\u{305}");
    let mut out = Vec::new();
    for _ in 0..PIC_ROWS {
        out.extend_from_slice(cell.repeat(8).as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Eight lines of text, so there are rows above the cursor.
fn earlier_lines() -> Vec<u8> {
    let mut b = b"\x1b[2J\x1b[H".to_vec();
    for i in 0..8 {
        b.extend_from_slice(format!("EARLIER {i}\r\n").as_bytes());
    }
    b
}

/// A picture sequence with its rows reserved first; the cursor ends just under them.
fn reserved(pic: &[u8]) -> Vec<u8> {
    let mut b = b"\r\n\r\n\r\n\r\n\x1b[4A".to_vec();
    b.extend_from_slice(pic);
    b.extend_from_slice(b"\x1b[4B");
    b
}

/// What the hold test runs after the picture: a prompt, then a flood of lines.
fn hold_steps(first: Vec<u8>) -> Vec<Step> {
    let mut lines = Vec::new();
    for i in 0..40 {
        lines.extend_from_slice(format!("LINE {i:02}\r\n").as_bytes());
    }
    vec![
        Step::Out(first),
        Step::Wait,
        Step::Prompt(b"PROMPT>"),
        Step::Wait,
        Step::Out(lines),
        Step::Report,
        Step::Wait,
    ]
}

/// What the repaint test runs after the picture: a VT alternate screen, then a buffer swap.
fn repaint_steps(first: Vec<u8>) -> Vec<Step> {
    vec![
        Step::Out(first),
        Step::Wait,
        Step::Out(b"\x1b[?1049hALT-TEXT\x1b[?1049lVT-BACK\r\n".to_vec()),
        Step::Wait,
        Step::SwapBuffers,
        Step::Wait,
    ]
}

fn scene(name: &str) -> Vec<Step> {
    let with = |head: Vec<u8>, pic: &[u8]| {
        let mut b = head;
        b.extend_from_slice(pic);
        b.extend_from_slice(b"BELOW\r\n");
        b
    };
    let top = b"\x1b[2J\x1b[HTOP\r\n".to_vec();
    match name {
        "arrival" => {
            let mut b = b"\x1b[2J\x1b[H".to_vec();
            b.extend_from_slice(b"A-BEFORE\x1b_Ga=T,f=100,s=1,v=1;QUFB\x1b\\A-AFTER\r\n");
            b.extend_from_slice(b"D-BEFORE\x1bPq#0;2;0;0;0#0~~\x1b\\D-AFTER\r\n");
            b.extend_from_slice(b"Z-BEFORE\x1bPzprobe\x1b\\Z-AFTER\r\n");
            b.extend_from_slice(b"O-BEFORE");
            b.extend_from_slice(OSC_PIC);
            b.extend_from_slice(b"O-AFTER\r\nL-BEFORE");
            b.extend_from_slice(&big_osc_1337());
            b.extend_from_slice(b"L-AFTER\r\nDONE-ARRIVAL\r\n");
            vec![Step::Out(b), Step::Wait]
        }
        "naive" => hold_steps(with(top, APC_PIC)),
        "above" => hold_steps(with(earlier_lines(), APC_PIC)),
        "reserved-apc" => hold_steps(with(top, &reserved(APC_PIC))),
        "reserved-osc" => hold_steps(with(top, &reserved(OSC_PIC))),
        "cells" => {
            let mut pic = CELLS_PIC.to_vec();
            pic.extend_from_slice(&cell_rows());
            hold_steps(with(top, &pic))
        }
        "repaint-above" => repaint_steps(with(earlier_lines(), APC_PIC)),
        "repaint-reserved" => repaint_steps(with(earlier_lines(), &reserved(APC_PIC))),
        "repaint-cells" => {
            let mut pic = CELLS_PIC.to_vec();
            pic.extend_from_slice(&cell_rows());
            repaint_steps(with(earlier_lines(), &pic))
        }
        other => panic!("no scene {other}"),
    }
}

/// VT output on, as a graphics program has it, and single-byte input.
fn console_modes() {
    use windows_sys::Win32::System::Console::{
        ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, ENABLE_WRAP_AT_EOL_OUTPUT,
        GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
    };
    // SAFETY: GetStdHandle takes no pointers; SetConsoleMode only reads the handle and flags.
    unsafe {
        SetConsoleMode(
            GetStdHandle(STD_OUTPUT_HANDLE),
            ENABLE_PROCESSED_OUTPUT
                | ENABLE_WRAP_AT_EOL_OUTPUT
                | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        );
        SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), 0);
    }
}

fn buffer_info() -> windows_sys::Win32::System::Console::CONSOLE_SCREEN_BUFFER_INFO {
    use windows_sys::Win32::System::Console::{
        GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    // SAFETY: the info struct is plain data, written by the call; the handle is this process's.
    unsafe {
        let mut info = std::mem::zeroed();
        GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info);
        info
    }
}

fn reassert_cursor() {
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_OUTPUT_HANDLE, SetConsoleCursorPosition,
    };
    let at = buffer_info().dwCursorPosition;
    // SAFETY: the handle is this process's and the position came from the console.
    unsafe {
        SetConsoleCursorPosition(GetStdHandle(STD_OUTPUT_HANDLE), at);
    }
}

fn swap_buffers() {
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
    use windows_sys::Win32::System::Console::{
        CONSOLE_TEXTMODE_BUFFER, CreateConsoleScreenBuffer, GetStdHandle, STD_OUTPUT_HANDLE,
        SetConsoleActiveScreenBuffer,
    };
    // SAFETY: the handles are this process's console handles; the API allows the null pointers.
    unsafe {
        let old = GetStdHandle(STD_OUTPUT_HANDLE);
        let new = CreateConsoleScreenBuffer(
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            CONSOLE_TEXTMODE_BUFFER,
            std::ptr::null(),
        );
        SetConsoleActiveScreenBuffer(new);
        std::thread::sleep(Duration::from_millis(300));
        SetConsoleActiveScreenBuffer(old);
        CloseHandle(new);
    }
}

/// Not a test of its own: the program ConPTY runs. Does nothing unless `SCENE_ENV` is set.
#[test]
fn stand_in_writer() {
    let Ok(name) = std::env::var(SCENE_ENV) else {
        return;
    };
    console_modes();
    let mut out = std::io::stdout();
    let mut input = std::io::stdin();
    for step in scene(&name) {
        let bytes = match step {
            Step::Out(b) => b,
            Step::Wait => {
                let mut one = [0u8; 1];
                if input.read(&mut one).unwrap_or(0) == 0 {
                    return;
                }
                continue;
            }
            Step::Prompt(text) => {
                reassert_cursor();
                text.to_vec()
            }
            Step::Report => {
                let w = buffer_info().srWindow;
                let (cols, rows) = (w.Right - w.Left + 1, w.Bottom - w.Top + 1);
                format!("REPORT {cols}x{rows}\r\n").into_bytes()
            }
            Step::SwapBuffers => {
                swap_buffers();
                b"SWAPPED-BACK\r\n".to_vec()
            }
        };
        out.write_all(&bytes).expect("stdout takes the bytes");
        out.flush().expect("stdout flushes");
    }
    std::thread::sleep(Duration::from_secs(60));
}

fn start(name: &str, cols: u16, rows: u16) -> (Rig, NamedDir) {
    let dir = temp_dir_named("flare-apc-probe");
    let exe = std::env::current_exe().expect("the test knows its own path");
    let mut env: Vec<(String, String)> = std::env::vars().collect();
    env.push((SCENE_ENV.to_string(), name.to_string()));
    let rig = Rig::spawn_tapped(
        &exe.to_string_lossy(),
        &[
            "--exact",
            "stand_in_writer",
            "--nocapture",
            "--test-threads=1",
        ],
        dir.path(),
        Some(env),
        cols,
        rows,
    );
    (rig, dir)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Bytes shown as text with control bytes visible, for failure messages.
fn show(raw: &[u8]) -> String {
    raw.iter()
        .map(|&b| match b {
            0x20..=0x7e => char::from(b).to_string(),
            _ => format!("<{b:02x}>"),
        })
        .collect()
}

/// Waits for `needle` in what ConPTY delivered, then for the output to go quiet.
fn settle(rig: &Rig, needle: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while find(&rig.raw(), needle).is_none() {
        assert!(
            Instant::now() < deadline,
            "no {:?} in {}",
            String::from_utf8_lossy(needle),
            show(&rig.raw())
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    rig.wait_quiet(400);
}

/// Wakes the writer and waits for what it prints next.
fn step(rig: &Rig, expect: &[u8]) {
    rig.write(b"n");
    settle(rig, expect);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// Cells drawn at the cursor and the cursor ends under them (kitty's default).
    Advance,
    /// Cells drawn at the cursor and the cursor stays (kitty's C=1 after the program made room).
    Pinned,
    /// The rows above the cursor scroll up to make room, so the cursor keeps agreeing with ConPTY.
    Above,
    /// The program printed the cells as text.
    OwnText,
}

/// The terminal a picture layer would have given: the bytes ConPTY delivered, the picture's cells
/// blotted in at the sequence, and each resize at the byte offset it happened.
fn replay(raw: &[u8], pic: &[u8], place: Place, marks: &[(usize, u16, u16)]) -> Emulator {
    let mut e = Emulator::new(60, 24);
    let at = find(raw, pic).expect("the picture is in the bytes");
    e.feed(&raw[..at]);
    let blot_row = format!("{BLOT}\r\n");
    match place {
        Place::Advance => {
            e.feed(blot_row.repeat(PIC_ROWS).as_bytes());
        }
        Place::Pinned => {
            e.feed(b"\x1b7");
            e.feed(blot_row.repeat(PIC_ROWS - 1).as_bytes());
            e.feed(BLOT.as_bytes());
            e.feed(b"\x1b8");
        }
        Place::Above => {
            let (line, col) = e.cursor();
            let line = usize::try_from(line).expect("the cursor is on the screen");
            assert!(line >= PIC_ROWS, "the scene leaves rows above the cursor");
            e.feed(format!("\x1b[1;{line}r\x1b[{PIC_ROWS}S\x1b[r").as_bytes());
            for row in 0..PIC_ROWS {
                let target = line - PIC_ROWS + row + 1;
                e.feed(format!("\x1b[{target};1H{BLOT}").as_bytes());
            }
            e.feed(format!("\x1b[{};{}H", line + 1, col + 1).as_bytes());
        }
        Place::OwnText => {}
    }
    let mut from = at + pic.len();
    for &(offset, cols, rows) in marks {
        let offset = offset.clamp(from, raw.len());
        e.feed(&raw[from..offset]);
        e.resize(usize::from(cols), usize::from(rows));
        from = offset;
    }
    e.feed(&raw[from..]);
    e
}

/// The numbers of the rows, history included, whose text contains `needle`.
fn rows_with(e: &Emulator, needle: &str) -> Vec<i32> {
    let history = i32::try_from(e.history_size()).expect("the history fits");
    let rows = i32::try_from(e.rows()).expect("the screen fits");
    (-history..rows)
        .filter(|&l| e.row_text(l).contains(needle))
        .collect()
}

fn dump(e: &Emulator) -> Vec<String> {
    let history = i32::try_from(e.history_size()).expect("the history fits");
    let rows = i32::try_from(e.rows()).expect("the screen fits");
    (-history..rows).map(|l| e.row_text(l)).collect()
}

/// Four rows together, and the first line under them is `BELOW`. Returns that line's row.
fn assert_in_place(e: &Emulator, needle: &str, when: &str) -> i32 {
    let picture = rows_with(e, needle);
    assert_eq!(
        picture.len(),
        PIC_ROWS,
        "{when}: picture rows; {:?}",
        dump(e)
    );
    assert_eq!(
        picture[PIC_ROWS - 1] - picture[0],
        3,
        "{when}: the rows are together; {:?}",
        dump(e)
    );
    let below = rows_with(e, "BELOW");
    assert_eq!(
        below,
        [picture[PIC_ROWS - 1] + 1],
        "{when}: BELOW is under the picture; {:?}",
        dump(e)
    );
    below[0]
}

#[test]
fn every_graphics_sequence_arrives_byte_exact_and_in_order() {
    let (rig, _dir) = start("arrival", 100, 30);
    settle(&rig, b"DONE-ARRIVAL");
    let raw = rig.raw();
    let big = big_osc_1337();
    let cases: [(&[u8], &[u8], &[u8]); 5] = [
        (
            b"A-BEFORE",
            b"\x1b_Ga=T,f=100,s=1,v=1;QUFB\x1b\\",
            b"A-AFTER",
        ),
        (b"D-BEFORE", b"\x1bPq#0;2;0;0;0#0~~\x1b\\", b"D-AFTER"),
        (b"Z-BEFORE", b"\x1bPzprobe\x1b\\", b"Z-AFTER"),
        (b"O-BEFORE", OSC_PIC, b"O-AFTER"),
        (b"L-BEFORE", &big, b"L-AFTER"),
    ];
    for (before, sequence, after) in cases {
        let b = find(&raw, before).expect("the text before it arrives");
        let s = find(&raw, sequence).expect("the sequence arrives whole");
        let a = find(&raw, after).expect("the text after it arrives");
        assert_eq!(b + before.len(), s, "nothing comes between");
        assert_eq!(s + sequence.len(), a, "nothing comes between");
    }
}

#[test]
fn a_prompt_the_console_places_lands_on_a_picture_that_moved_the_cursor() {
    let (rig, _dir) = start("naive", 60, 24);
    settle(&rig, b"BELOW");
    assert_in_place(
        &replay(&rig.raw(), APC_PIC, Place::Advance, &[]),
        BLOT,
        "placed",
    );
    step(&rig, b"PROMPT>");
    let e = replay(&rig.raw(), APC_PIC, Place::Advance, &[]);
    assert_eq!(
        rows_with(&e, BLOT).len(),
        PIC_ROWS - 1,
        "ConPTY never knew the picture moved the cursor; {:?}",
        dump(&e)
    );
    assert_eq!(rows_with(&e, "PROMPT>"), [2], "{:?}", dump(&e));
}

/// A picture with its rows reserved: `BELOW` under it, a console-placed prompt under that, every
/// resize silent, and the picture scrolling away whole.
fn assert_holds(name: &str, pic: &[u8], place: Place, needle: &str) {
    let (mut rig, _dir) = start(name, 60, 24);
    settle(&rig, b"BELOW");
    let replayed = |rig: &Rig, marks: &[(usize, u16, u16)]| replay(&rig.raw(), pic, place, marks);
    assert_in_place(&replayed(&rig, &[]), needle, "placed");

    step(&rig, b"PROMPT>");
    let e = replayed(&rig, &[]);
    let below = assert_in_place(&e, needle, "after the prompt");
    assert_eq!(rows_with(&e, "PROMPT>"), [below + 1], "{:?}", dump(&e));

    let mut marks = Vec::new();
    for (cols, rows) in [(48u16, 24u16), (72, 24), (72, 16), (72, 30)] {
        let at = rig.raw().len();
        marks.push((at, cols, rows));
        rig.resize(cols, rows);
        rig.wait_quiet(400);
        assert_eq!(rig.raw().len(), at, "{cols}x{rows}: ConPTY sends nothing");
        assert_in_place(&replayed(&rig, &marks), needle, "after a resize");
    }

    step(&rig, b"REPORT 72x30");
    let e = replayed(&rig, &marks);
    assert_in_place(&e, needle, "after the flood");
    assert!(
        rows_with(&e, needle)[0] < 0,
        "the picture scrolled into the history; {:?}",
        dump(&e)
    );
}

#[test]
fn a_picture_made_room_for_above_the_cursor_holds_its_place() {
    assert_holds("above", APC_PIC, Place::Above, BLOT);
}

#[test]
fn an_apc_picture_in_rows_the_program_reserved_holds_its_place() {
    assert_holds("reserved-apc", APC_PIC, Place::Pinned, BLOT);
}

#[test]
fn an_osc_1337_picture_in_rows_the_program_reserved_holds_its_place() {
    assert_holds("reserved-osc", OSC_PIC, Place::Pinned, BLOT);
}

#[test]
fn cells_the_program_prints_hold_their_place() {
    assert_holds("cells", CELLS_PIC, Place::OwnText, &CELL.to_string());
}

/// The VT alternate screen leaves the picture alone; a console buffer swap makes ConPTY repaint the
/// screen from its own buffer and `repainted` says what became of the picture.
fn repaint_case(name: &str, pic: &[u8], place: Place, repainted: impl Fn(&Emulator)) {
    let (rig, _dir) = start(name, 60, 24);
    settle(&rig, b"BELOW");
    step(&rig, b"VT-BACK");
    let e = replay(&rig.raw(), pic, place, &[]);
    let needle = if place == Place::OwnText {
        CELL.to_string()
    } else {
        BLOT.to_string()
    };
    assert_in_place(&e, &needle, "after the VT alternate screen");
    step(&rig, b"SWAPPED-BACK");
    repainted(&replay(&rig.raw(), pic, place, &[]));
}

#[test]
fn a_console_buffer_swap_paints_over_a_picture_above_the_cursor() {
    repaint_case("repaint-above", APC_PIC, Place::Above, |e| {
        assert!(rows_with(e, BLOT).is_empty(), "{:?}", dump(e));
        assert_eq!(
            rows_with(e, "EARLIER 3").len(),
            2,
            "the repaint brings back rows the scroll had moved; {:?}",
            dump(e)
        );
    });
}

#[test]
fn a_console_buffer_swap_paints_over_a_picture_in_reserved_rows() {
    repaint_case("repaint-reserved", APC_PIC, Place::Pinned, |e| {
        assert!(rows_with(e, BLOT).is_empty(), "{:?}", dump(e));
    });
}

#[test]
fn a_console_buffer_swap_turns_printed_cells_into_replacement_characters() {
    repaint_case("repaint-cells", CELLS_PIC, Place::OwnText, |e| {
        assert!(rows_with(e, &CELL.to_string()).is_empty(), "{:?}", dump(e));
        assert_eq!(rows_with(e, "\u{FFFD}").len(), PIC_ROWS, "{:?}", dump(e));
    });
}

/// The bytes the `repaint-above` scene produces in ConPTY created with `flags`: everything up to
/// the picture's `BELOW`, then the repaint from the buffer swap. Spawns directly, because
/// portable-pty cannot pass `PSEUDOCONSOLE_PASSTHROUGH_MODE`.
fn conpty_with_flags(flags: u32) -> (Vec<u8>, Vec<u8>) {
    use std::sync::{Arc, Mutex};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, S_OK};
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows_sys::Win32::System::Console::COORD;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
        STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
    };

    flare_term::conpty::load_bundled().expect("the bundle loads");
    type Create = unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut isize) -> i32;
    type Close = unsafe extern "system" fn(isize);
    let dll: Vec<u16> = "conpty.dll\0".encode_utf16().collect();
    // SAFETY: the name is NUL terminated; `load_bundled` preloaded the module.
    let module = unsafe { GetModuleHandleW(dll.as_ptr()) };
    assert!(!module.is_null(), "conpty.dll is loaded");
    // SAFETY: the names are NUL terminated and these are ConPTY's documented signatures.
    let (create, close): (Create, Close) = unsafe {
        (
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Create>(
                GetProcAddress(module, c"CreatePseudoConsole".as_ptr().cast()).expect("export"),
            ),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Close>(
                GetProcAddress(module, c"ClosePseudoConsole".as_ptr().cast()).expect("export"),
            ),
        )
    };

    let null = std::ptr::null_mut;
    let (mut in_r, mut in_w, mut out_r, mut out_w): (HANDLE, HANDLE, HANDLE, HANDLE) =
        (null(), null(), null(), null());
    let mut hpc: isize = 0;
    // SAFETY: every out-pointer is a live local; these ends now belong to the console.
    unsafe {
        assert_ne!(CreatePipe(&mut in_r, &mut in_w, std::ptr::null(), 0), 0);
        assert_ne!(CreatePipe(&mut out_r, &mut out_w, std::ptr::null(), 0), 0);
        let size = COORD { X: 60, Y: 24 };
        assert_eq!(create(size, in_r, out_w, flags, &mut hpc), S_OK);
        CloseHandle(in_r);
        CloseHandle(out_w);
    }

    let raw = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&raw);
    let out_r_addr = out_r as usize;
    let reader = std::thread::spawn(move || {
        let handle = out_r_addr as HANDLE;
        let mut buf = vec![0u8; 65536];
        loop {
            let mut n = 0u32;
            // SAFETY: the buffer outlives the call and the length given is its length.
            let ok = unsafe {
                ReadFile(
                    handle,
                    buf.as_mut_ptr(),
                    65536,
                    &mut n,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || n == 0 {
                break;
            }
            sink.lock()
                .expect("the sink")
                .extend_from_slice(&buf[..n as usize]);
        }
    });

    let exe = std::env::current_exe().expect("the test knows its own path");
    let line = format!(
        "\"{}\" --exact stand_in_writer --nocapture --test-threads=1",
        exe.display()
    );
    let mut wide_line: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
    let mut vars: Vec<String> = std::env::vars()
        .map(|(k, v)| format!("{k}={v}"))
        .chain(Some(format!("{SCENE_ENV}=repaint-above")))
        .collect();
    vars.sort();
    let mut env_block: Vec<u16> = vars
        .iter()
        .flat_map(|v| v.encode_utf16().chain(Some(0)))
        .collect();
    env_block.push(0);

    let mut size = 0usize;
    // SAFETY: the documented size query (null list, out size); its failure return is expected.
    unsafe { InitializeProcThreadAttributeList(null(), 1, 0, &mut size) };
    let mut list = vec![0u8; size];
    let attrs = list.as_mut_ptr().cast();
    // SAFETY: STARTUPINFOEXW and PROCESS_INFORMATION are plain data; all-zero is a valid start.
    let (mut si, mut pi): (STARTUPINFOEXW, PROCESS_INFORMATION) =
        unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    si.StartupInfo.cb = u32::try_from(std::mem::size_of::<STARTUPINFOEXW>()).expect("it fits");
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.lpAttributeList = attrs;
    // SAFETY: `list` holds `size` bytes for one attribute; `hpc`, the command line and the
    // environment block all outlive the calls.
    unsafe {
        assert_ne!(InitializeProcThreadAttributeList(attrs, 1, 0, &mut size), 0);
        assert_ne!(
            UpdateProcThreadAttribute(
                attrs,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpc as *const core::ffi::c_void,
                std::mem::size_of::<isize>(),
                null(),
                std::ptr::null(),
            ),
            0
        );
        assert_ne!(
            CreateProcessW(
                std::ptr::null(),
                wide_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                env_block.as_ptr().cast(),
                std::ptr::null(),
                &si.StartupInfo,
                &mut pi,
            ),
            0,
            "the writer starts"
        );
    }

    let wait = |needle: &[u8]| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while find(&raw.lock().expect("the bytes"), needle).is_none() {
            assert!(Instant::now() < deadline, "flags {flags:#x}: no reply");
            std::thread::sleep(Duration::from_millis(30));
        }
        std::thread::sleep(Duration::from_millis(400));
    };
    let wake = || {
        let mut n = 0u32;
        // SAFETY: writes one byte from a static to the write end of a live pipe.
        unsafe { WriteFile(in_w, b"n".as_ptr(), 1, &mut n, std::ptr::null_mut()) };
    };
    wait(b"BELOW");
    let placed = raw.lock().expect("the bytes").clone();
    wake();
    wait(b"VT-BACK");
    let before_swap = raw.lock().expect("the bytes").len();
    wake();
    wait(b"SWAPPED-BACK");
    let swap = raw.lock().expect("the bytes")[before_swap..].to_vec();
    // SAFETY: the handles are ours and closed exactly once; closing the console ends the read.
    unsafe {
        TerminateProcess(pi.hProcess, 0);
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
        close(hpc);
        CloseHandle(in_w);
        DeleteProcThreadAttributeList(attrs);
    }
    reader.join().expect("the reader ends");
    (placed, swap)
}

/// `PSEUDOCONSOLE_PASSTHROUGH_MODE` is measured here and nowhere else (issue #35's spec).
#[test]
fn passthrough_mode_neither_helps_arrival_nor_stops_the_repaint() {
    const RESIZE_QUIRK: u32 = 0x2;
    const WIN32_INPUT: u32 = 0x4;
    const PASSTHROUGH: u32 = 0x8;
    let plain = conpty_with_flags(RESIZE_QUIRK | WIN32_INPUT);
    let passthrough = conpty_with_flags(RESIZE_QUIRK | WIN32_INPUT | PASSTHROUGH);
    for (label, (placed, swap)) in [("plain", &plain), ("passthrough", &passthrough)] {
        assert!(find(placed, APC_PIC).is_some(), "{label}: the APC arrives");
        assert!(
            find(swap, b"EARLIER 0").is_some(),
            "{label}: the swap repaints from ConPTY's own buffer: {}",
            show(swap)
        );
    }
}
