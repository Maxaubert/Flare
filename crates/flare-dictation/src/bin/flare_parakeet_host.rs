//! flare-parakeet-host: the resident Parakeet host (Task 14, D3, DIFF-19). It keeps ONE model loaded and
//! answers every pass, so a pass costs 24 to 170 ms (MEASURED by Prism Terminal on an RTX 5090) instead of
//! the 0.72 to 0.86 s a fresh `parakeet-cli` spends loading the model. It runs OUT of process on purpose:
//! a CUDA crash must not take the app down.
//!
//! This is `core/dictation-host/parakeetHost.ps1` of Prism Terminal at e5848cb, native instead of a
//! compiled C# class in Windows PowerShell: the same arguments, the same stdin and stdout protocol byte
//! for byte, the same `parakeet_full_params` layout passed by value.
//!
//! args:   `<engine folder> <model> <gpu 1|0> <threads> [<cache folder>]` (the cache is accepted and
//!         ignored: there is nothing to compile).
//! stdin:  `pass <n>` LF, then n bytes of a 16 kHz mono PCM16 WAV; `quit` ends it.
//! stdout: `ready` once the model is loaded; per pass `ok <base64 of the UTF-8 text>` or `err <why>`.
//!         Nothing else is ever written there by this program.

use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

/// The largest WAV a pass may carry (64 MiB, the script's bound).
const MAX_WAV: i64 = 64 * 1024 * 1024;

/// `parakeet_context_params`, by value: a one-byte bool and an int.
#[repr(C)]
#[derive(Clone, Copy)]
struct CtxParams {
    use_gpu: u8,
    gpu_device: i32,
}

/// `parakeet.h`'s `parakeet_full_params`, field for field (passed BY VALUE). The default set is read
/// from the library (`parakeet_full_default_params_by_ref`), `n_threads` is overwritten, and the rest is
/// handed back untouched.
#[repr(C)]
#[derive(Clone, Copy)]
struct FullParams {
    strategy: i32,
    n_threads: i32,
    offset_ms: i32,
    duration_ms: i32,
    no_context: u8,
    audio_ctx: i32,
    seg_cb: *mut c_void,
    seg_ud: *mut c_void,
    tok_cb: *mut c_void,
    tok_ud: *mut c_void,
    prog_cb: *mut c_void,
    prog_ud: *mut c_void,
    enc_cb: *mut c_void,
    enc_ud: *mut c_void,
    abort_cb: *mut c_void,
    abort_ud: *mut c_void,
}

type LogCb = extern "C" fn(i32, *const c_char, *mut c_void);
type LogSetFn = unsafe extern "C" fn(Option<LogCb>, *mut c_void);
type InitFn = unsafe extern "C" fn(*const c_char, CtxParams) -> *mut c_void;
type DefaultParamsFn = unsafe extern "C" fn(i32) -> *mut FullParams;
type FreeParamsFn = unsafe extern "C" fn(*mut FullParams);
type FullFn = unsafe extern "C" fn(*mut c_void, FullParams, *const f32, i32) -> i32;
type NSegmentsFn = unsafe extern "C" fn(*mut c_void) -> i32;
type SegmentTextFn = unsafe extern "C" fn(*mut c_void, i32) -> *const c_char;
type LoadAllFn = unsafe extern "C" fn();

extern "C" fn quiet(_level: i32, _text: *const c_char, _user: *mut c_void) {}

/// What the protocol loop needs from the engine: one pass over samples, to text or to the engine's code.
trait Recognizer {
    fn recognize(&mut self, samples: &[f32]) -> Result<String, i32>;
}

/// The 16-bit mono samples of the WAV's `data` chunk, as floats. The app's own WAV is walked chunk by
/// chunk (an odd chunk is padded by a byte); a negative length ends the walk; a `data` chunk cut short by
/// the end of the buffer yields what is there. No `data` chunk is no samples.
fn pcm(wav: &[u8]) -> Vec<f32> {
    let mut i = 12usize;
    while i + 8 <= wav.len() {
        let len = i64::from(i32::from_le_bytes([
            wav[i + 4],
            wav[i + 5],
            wav[i + 6],
            wav[i + 7],
        ]));
        if len < 0 {
            break;
        }
        if &wav[i..i + 4] == b"data" {
            let room = i64::try_from(wav.len() - i - 8).unwrap_or(i64::MAX);
            let n = usize::try_from(len.min(room) / 2).unwrap_or(0);
            return (0..n)
                .map(|k| {
                    let at = i + 8 + k * 2;
                    f32::from(i16::from_le_bytes([wav[at], wav[at + 1]])) / 32768.0
                })
                .collect();
        }
        i += 8 + usize::try_from(len).unwrap_or(0) + usize::try_from(len & 1).unwrap_or(0);
    }
    Vec::new()
}

/// One line off the input, without its LF and CR. `None` at the end of the input; a line longer than 64
/// characters is answered as the empty line (a malformed head), as the script does.
fn read_line(input: &mut impl Read) -> Option<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if input.read(&mut byte).ok()? == 0 {
            return None;
        }
        if byte[0] == b'\n' {
            return Some(
                String::from_utf8_lossy(&line)
                    .trim_end_matches('\r')
                    .to_string(),
            );
        }
        if line.len() > 64 {
            return Some(String::new());
        }
        line.push(byte[0]);
    }
}

fn say(output: &mut impl Write, line: &str) -> bool {
    output
        .write_all(format!("{line}\n").as_bytes())
        .and_then(|()| output.flush())
        .is_ok()
}

/// The pass loop (N6). Returns when the input ends, on `quit`, on a malformed request (after answering
/// `err a bad request`) or when the output is gone.
fn serve(input: &mut impl Read, output: &mut impl Write, engine: &mut impl Recognizer) {
    loop {
        let Some(head) = read_line(input) else {
            return;
        };
        if head == "quit" {
            return;
        }
        let wanted = head
            .strip_prefix("pass ")
            .and_then(|n| n.trim().parse::<i32>().ok())
            .map(i64::from)
            .filter(|n| (0..=MAX_WAV).contains(n));
        let Some(n) = wanted.and_then(|n| usize::try_from(n).ok()) else {
            say(output, "err a bad request");
            return;
        };
        let mut wav = vec![0u8; n];
        if input.read_exact(&mut wav).is_err() {
            return;
        }
        let samples = pcm(&wav);
        if samples.is_empty() {
            if !say(output, "ok ") {
                return;
            }
            continue;
        }
        let said = match engine.recognize(&samples) {
            Ok(text) => format!("ok {}", STANDARD.encode(text.trim().as_bytes())),
            Err(rc) => format!("err the pass failed ({rc})"),
        };
        if !say(output, &said) {
            return;
        }
    }
}

/// The loaded library and its context.
struct Parakeet {
    ctx: *mut c_void,
    params: FullParams,
    full: FullFn,
    n_segments: NSegmentsFn,
    segment_text: SegmentTextFn,
}

impl Recognizer for Parakeet {
    fn recognize(&mut self, samples: &[f32]) -> Result<String, i32> {
        let n = i32::try_from(samples.len()).map_err(|_| -1)?;
        // SAFETY: `ctx` is the live context from init; `params` is the library's own default set by
        // value; `samples` is a live slice of `n` floats for the call.
        let rc = unsafe { (self.full)(self.ctx, self.params, samples.as_ptr(), n) };
        if rc != 0 {
            return Err(rc);
        }
        // SAFETY: `ctx` is live and holds the segments of the pass that just ran.
        let segments = unsafe { (self.n_segments)(self.ctx) };
        let mut text = Vec::new();
        for s in 0..segments {
            // SAFETY: `s` is below the segment count; the library returns a NUL-terminated string that
            // lives until the next pass, copied out here, or null.
            let ptr = unsafe { (self.segment_text)(self.ctx, s) };
            if !ptr.is_null() {
                // SAFETY: non-null and NUL-terminated by the library's contract.
                text.extend_from_slice(unsafe { CStr::from_ptr(ptr) }.to_bytes());
            }
        }
        Ok(String::from_utf8_lossy(&text).trim().to_string())
    }
}

fn wide(s: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[link(name = "kernel32")]
extern "system" {
    fn WideCharToMultiByte(
        code_page: u32,
        flags: u32,
        wide: *const u16,
        wide_len: i32,
        out: *mut u8,
        out_len: i32,
        default_char: *const u8,
        used_default: *mut i32,
    ) -> i32;
}

/// The model's path as the engine opens it: the script marshalled it as ANSI (`LPStr`), the system code
/// page, so the same bytes go in here. A path the code page cannot spell is handed over as UTF-8.
fn model_c_string(model: &str) -> Option<CString> {
    // A path the code page cannot spell goes in as its 8.3 short form (audit L32/L63).
    let safe = flare_dictation::ansi_path::engine_safe_path(Path::new(model));
    let model = safe.to_string_lossy();
    let model: &str = &model;
    let w: Vec<u16> = model.encode_utf16().collect();
    let len = i32::try_from(w.len()).ok()?;
    let mut used_default = 0i32;
    // SAFETY: `w` is a live buffer of `len` UTF-16 units; the first call only measures (null out, size
    // 0), the second writes at most `size` bytes into `out`, which has that many; `used_default` is a
    // live int.
    let ansi = unsafe {
        let size = WideCharToMultiByte(
            0,
            0,
            w.as_ptr(),
            len,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
        );
        if size <= 0 {
            None
        } else {
            let mut out = vec![0u8; usize::try_from(size).ok()?];
            let wrote = WideCharToMultiByte(
                0,
                0,
                w.as_ptr(),
                len,
                out.as_mut_ptr(),
                size,
                std::ptr::null(),
                &mut used_default,
            );
            (wrote == size && used_default == 0).then_some(out)
        }
    };
    CString::new(ansi.unwrap_or_else(|| model.as_bytes().to_vec())).ok()
}

/// Loads `ggml.dll` then `parakeet.dll` from `dir`, loads the backends and the model. `Err` says why, in
/// words for stderr.
/// Why the engine did not come up.
enum LoadError {
    /// The libraries were there and the model would not load: told to the app on stdout.
    Model,
    /// Anything else: told on stderr, and no `ready`.
    Other(String),
}

type Loaded = (Parakeet, [libloading::Library; 2]);

fn load(dir: &Path, model: &str, gpu: bool, threads: i32) -> Result<Loaded, LoadError> {
    let other = |s: String| LoadError::Other(s);
    use windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW;
    let dir_wide = wide(dir);
    // SAFETY: `dir_wide` is a live NUL-terminated wide string for the call.
    unsafe { SetDllDirectoryW(dir_wide.as_ptr()) };
    // SAFETY: loading the engine's own DLLs by full path; their initialisers are the engine's.
    let ggml = unsafe { libloading::Library::new(dir.join("ggml.dll")) }
        .map_err(|e| other(format!("ggml.dll did not load: {e}")))?;
    // SAFETY: as above.
    let lib = unsafe { libloading::Library::new(dir.join("parakeet.dll")) }
        .map_err(|e| other(format!("parakeet.dll did not load: {e}")))?;
    macro_rules! sym {
        ($lib:expr, $name:literal, $ty:ty) => {
            // SAFETY: the symbol is looked up by the name the engine exports with the signature of
            // parakeet.h / ggml-backend.h; the copied function pointer is used only while the library
            // handles, returned alongside, are alive.
            *unsafe { $lib.get::<$ty>(concat!($name, "\0").as_bytes()) }
                .map_err(|e| other(format!("{} is missing: {e}", $name)))?
        };
    }
    let log_set = sym!(lib, "parakeet_log_set", LogSetFn);
    let load_all = sym!(ggml, "ggml_backend_load_all", LoadAllFn);
    let init = sym!(lib, "parakeet_init_from_file_with_params", InitFn);
    let default_params = sym!(lib, "parakeet_full_default_params_by_ref", DefaultParamsFn);
    let free_params = sym!(lib, "parakeet_free_params", FreeParamsFn);
    let full = sym!(lib, "parakeet_full", FullFn);
    let n_segments = sym!(lib, "parakeet_full_n_segments", NSegmentsFn);
    let segment_text = sym!(lib, "parakeet_full_get_segment_text", SegmentTextFn);

    // SAFETY: the log callback is an `extern "C"` function that does nothing; load_all takes nothing.
    unsafe {
        log_set(Some(quiet), std::ptr::null_mut());
        load_all();
    }
    let path =
        model_c_string(model).ok_or_else(|| other("the model path cannot be used".to_string()))?;
    // SAFETY: `path` is a live NUL-terminated string; the params are passed by value.
    let ctx = unsafe {
        init(
            path.as_ptr(),
            CtxParams {
                use_gpu: u8::from(gpu),
                gpu_device: 0,
            },
        )
    };
    if ctx.is_null() {
        return Err(LoadError::Model);
    }
    // SAFETY: strategy 0 (greedy) is valid; the returned pointer is the library's default set, read once
    // by value and then freed with the library's own free.
    let mut params = unsafe {
        let p = default_params(0);
        if p.is_null() {
            return Err(other("the engine has no default parameters".to_string()));
        }
        let copy = std::ptr::read(p);
        free_params(p);
        copy
    };
    params.n_threads = threads;
    Ok((
        Parakeet {
            ctx,
            params,
            full,
            n_segments,
            segment_text,
        },
        [ggml, lib],
    ))
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!(
            "usage: flare-parakeet-host <engine folder> <model> <gpu 1|0> <threads> [<cache>]"
        );
        return 2;
    }
    let dir = PathBuf::from(&args[0]);
    let gpu = args[2] == "1";
    let threads = args[3].parse::<i32>().unwrap_or(4);
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    // Libraries stay loaded until the process ends.
    let (mut engine, _libs) = match load(&dir, &args[1], gpu, threads) {
        Ok(loaded) => loaded,
        Err(LoadError::Model) => {
            say(&mut output, "err the model did not load");
            return 0;
        }
        Err(LoadError::Other(why)) => {
            eprintln!("flare-parakeet-host: {why}");
            return 1;
        }
    };
    if !say(&mut output, "ready") {
        return 0;
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    serve(&mut input, &mut output, &mut engine);
    0
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_model_path_outside_the_code_page_reaches_the_engine_as_a_path_that_opens() {
        use std::os::windows::ffi::OsStrExt;
        let dir = tempfile::Builder::new()
            .prefix("\u{30e2}\u{30c7}\u{30eb}\u{65e5}\u{672c}\u{8a9e}\u{d55c}")
            .tempdir()
            .unwrap();
        let file = dir.path().join("\u{30e2}\u{30c7}\u{30eb}.bin");
        std::fs::write(&file, b"weights").unwrap();
        let w: Vec<u16> = file.as_os_str().encode_wide().collect();
        if flare_dictation::ansi_path::ansi_clean(&w) {
            return; // the code page is UTF-8 here
        }
        let c = super::model_c_string(&file.to_string_lossy()).unwrap();
        // What the engine hands to fopen: ASCII here, and it opens the same file.
        assert!(c.as_bytes().is_ascii(), "{c:?}");
        let opened = std::fs::read(c.to_str().unwrap()).unwrap();
        assert_eq!(opened, b"weights");
    }

    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    /// The layout the script's C# declares for `parakeet_full_params` on x64, worked out: four ints at 0, 4,
    /// 8, 12; `no_context` is a one-byte bool (`UnmanagedType.I1`) at 16; `audio_ctx` is an int, so the
    /// compiler pads 17 to 20 and puts it at 20; ten pointers follow, each 8-aligned, so they start at 24
    /// and run at 24, 32, 40, 48, 56, 64, 72, 80, 88, 96; the struct ends at 104, a multiple of its
    /// 8-byte alignment. `parakeet_context_params` is a one-byte bool at 0 and an int at 4 (padding 1 to
    /// 4), 8 bytes in all.
    #[test]
    fn full_params_layout_matches_the_script() {
        assert_eq!(size_of::<FullParams>(), 104);
        assert_eq!(align_of::<FullParams>(), 8);
        assert_eq!(offset_of!(FullParams, strategy), 0);
        assert_eq!(offset_of!(FullParams, n_threads), 4);
        assert_eq!(offset_of!(FullParams, offset_ms), 8);
        assert_eq!(offset_of!(FullParams, duration_ms), 12);
        assert_eq!(offset_of!(FullParams, no_context), 16);
        assert_eq!(offset_of!(FullParams, audio_ctx), 20);
        assert_eq!(offset_of!(FullParams, seg_cb), 24);
        assert_eq!(offset_of!(FullParams, seg_ud), 32);
        assert_eq!(offset_of!(FullParams, tok_cb), 40);
        assert_eq!(offset_of!(FullParams, tok_ud), 48);
        assert_eq!(offset_of!(FullParams, prog_cb), 56);
        assert_eq!(offset_of!(FullParams, prog_ud), 64);
        assert_eq!(offset_of!(FullParams, enc_cb), 72);
        assert_eq!(offset_of!(FullParams, enc_ud), 80);
        assert_eq!(offset_of!(FullParams, abort_cb), 88);
        assert_eq!(offset_of!(FullParams, abort_ud), 96);
        assert_eq!(size_of::<CtxParams>(), 8);
        assert_eq!(offset_of!(CtxParams, use_gpu), 0);
        assert_eq!(offset_of!(CtxParams, gpu_device), 4);
    }

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut c = id.to_vec();
        c.extend_from_slice(&(i32::try_from(body.len()).unwrap()).to_le_bytes());
        c.extend_from_slice(body);
        if body.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    fn wav_with(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut w = b"RIFF\0\0\0\0WAVE".to_vec();
        for c in chunks {
            w.extend_from_slice(c);
        }
        w
    }

    #[test]
    fn samples_are_the_data_chunks_int16_over_32768() {
        let body: Vec<u8> = [0i16, 16384, -32768, 32767]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let p = pcm(&wav_with(&[
            chunk(b"fmt ", &[0; 16]),
            chunk(b"data", &body),
        ]));
        assert_eq!(p, vec![0.0, 0.5, -1.0, 32767.0 / 32768.0]);
    }

    #[test]
    fn an_odd_sized_chunk_before_the_data_is_padded_by_a_byte() {
        let body: Vec<u8> = 1000i16.to_le_bytes().to_vec();
        let p = pcm(&wav_with(&[
            chunk(b"LIST", &[1, 2, 3]),
            chunk(b"data", &body),
        ]));
        assert_eq!(p, vec![1000.0 / 32768.0]);
    }

    #[test]
    fn a_data_chunk_cut_short_gives_what_is_there_and_a_missing_one_gives_nothing() {
        let mut w = wav_with(&[chunk(b"data", &[0u8; 10])]);
        w.truncate(12 + 8 + 4);
        assert_eq!(pcm(&w).len(), 2);
        assert!(pcm(&wav_with(&[chunk(b"fmt ", &[0; 16])])).is_empty());
        // A negative length ends the walk.
        let mut bad = b"RIFF\0\0\0\0WAVEjunk".to_vec();
        bad.extend_from_slice(&(-5i32).to_le_bytes());
        bad.extend_from_slice(&chunk(b"data", &[0u8; 4]));
        assert!(pcm(&bad).is_empty());
        assert!(pcm(&[]).is_empty());
    }

    #[test]
    fn an_empty_data_chunk_has_no_samples() {
        assert!(pcm(&wav_with(&[chunk(b"data", &[])])).is_empty());
    }

    struct Scripted(Vec<Result<String, i32>>, Vec<usize>);

    impl Recognizer for Scripted {
        fn recognize(&mut self, samples: &[f32]) -> Result<String, i32> {
            self.1.push(samples.len());
            self.0.remove(0)
        }
    }

    fn talk(input: &[u8], engine: &mut Scripted) -> String {
        let mut out = Vec::new();
        serve(&mut &input[..], &mut out, engine);
        String::from_utf8(out).unwrap()
    }

    fn request(wav: &[u8]) -> Vec<u8> {
        let mut r = format!("pass {}\n", wav.len()).into_bytes();
        r.extend_from_slice(wav);
        r
    }

    fn one_second() -> Vec<u8> {
        wav_with(&[chunk(b"data", &[1u8; 8])])
    }

    #[test]
    fn a_pass_answers_ok_and_the_base64_of_the_trimmed_text() {
        let mut e = Scripted(vec![Ok(" Hello, \u{e6}\u{f8}\u{e5} ".into())], vec![]);
        let said = talk(&request(&one_second()), &mut e);
        assert_eq!(
            said,
            format!("ok {}\n", STANDARD.encode("Hello, \u{e6}\u{f8}\u{e5}"))
        );
        assert_eq!(e.1, vec![4]);
    }

    #[test]
    fn an_empty_data_chunk_answers_ok_with_nothing_and_asks_nobody() {
        let mut e = Scripted(vec![], vec![]);
        let said = talk(&request(&wav_with(&[chunk(b"data", &[])])), &mut e);
        assert_eq!(said, "ok \n");
        assert!(e.1.is_empty());
    }

    #[test]
    fn a_failed_pass_answers_err_with_the_code_and_the_host_goes_on() {
        let mut e = Scripted(vec![Err(-3), Ok("x".into())], vec![]);
        let mut input = request(&one_second());
        input.extend(request(&one_second()));
        input.extend(b"quit\n");
        let said = talk(&input, &mut e);
        assert_eq!(
            said,
            format!("err the pass failed (-3)\nok {}\n", STANDARD.encode("x"))
        );
    }

    #[test]
    fn a_malformed_head_or_an_oversized_pass_answers_a_bad_request_and_ends() {
        for head in [
            "pass\n".to_string(),
            "pass x\n".to_string(),
            "pass -1\n".to_string(),
            "hello\n".to_string(),
            "pass 67108865\n".to_string(),
            "pass 99999999999\n".to_string(),
            format!("{}\n", "p".repeat(100)),
        ] {
            let mut e = Scripted(vec![], vec![]);
            let mut input = head.clone().into_bytes();
            input.extend(request(&one_second()));
            assert_eq!(talk(&input, &mut e), "err a bad request\n", "{head:?}");
            assert!(e.1.is_empty());
        }
    }

    #[test]
    fn quit_the_end_of_input_and_a_short_body_end_it_quietly() {
        let mut e = Scripted(vec![], vec![]);
        assert_eq!(talk(b"quit\n", &mut e), "");
        assert_eq!(talk(b"", &mut e), "");
        assert_eq!(talk(b"pass 100\nabc", &mut e), "");
        assert_eq!(talk(b"quit\r\n", &mut e), "");
    }

    #[test]
    fn a_crlf_head_is_read_like_an_lf_one() {
        let mut e = Scripted(vec![Ok("a".into())], vec![]);
        let wav = one_second();
        let mut input = format!("pass {}\r\n", wav.len()).into_bytes();
        input.extend(&wav);
        assert_eq!(
            talk(&input, &mut e),
            format!("ok {}\n", STANDARD.encode("a"))
        );
    }
}
