//! A stand-in for parakeet-cli.exe, for the Parakeet engine tests (Task 14). Tests only, never packed.
//! Started the way the real one is (`-m <model> -f - -np -t <n> [-ng]`, measured against the b5130
//! release), it reads the WAV off STDIN to the end and prints, as the "text", a description of what it
//! RECEIVED, so a test reads the argv, the byte count and the environment back instead of trusting the
//! engine. Ported from Prism Terminal's `fakeParakeetCli.mjs` (e5848cb).
//!
//! Switches (argv, appended by the test's `command`):
//!   --fake-exit          say why on stderr and exit 3 (an engine that cannot run)
//!   --fake-delay <ms>    hold the answer this long after stdin has ended
//!   --fake-error-zero    say "error: ..." on stderr and exit 0, as the real one does for audio it
//!                        cannot read (MEASURED)
//!   --fake-unicode       answer with letters outside ASCII

use std::io::{Read, Write};
use std::time::Duration;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| argv.iter().any(|a| a == name);
    let value = |name: &str| {
        let i = argv.iter().position(|a| a == name)?;
        argv.get(i + 1).cloned()
    };

    let mut audio = Vec::new();
    let _ = std::io::stdin().lock().read_to_end(&mut audio);

    if flag("--fake-exit") {
        eprintln!("fake: CUDA error: no kernel image is available for execution on the device");
        std::process::exit(3);
    }
    if flag("--fake-error-zero") {
        eprintln!("error: failed to read audio file '-'");
        std::process::exit(0);
    }
    let delay: u64 = value("--fake-delay")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    std::thread::sleep(Duration::from_millis(delay));
    let text = if flag("--fake-unicode") {
        "Gr\u{fc}\u{df} Gott, \u{e6}\u{f8}\u{e5}, \u{bf}qu\u{e9} tal?".to_string()
    } else {
        serde_json::json!({
            "bytes": audio.len(),
            "model": value("-m"),
            "file": value("-f"),
            "noPrints": flag("-np"),
            "threads": value("-t"),
            "noGpu": flag("-ng"),
            "pid": std::process::id(),
            "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()),
            "cudaCache": std::env::var("CUDA_CACHE_PATH").ok(),
        })
        .to_string()
    };
    // CRLF, as the real one prints on Windows.
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(format!("{text}\r\n").as_bytes());
    let _ = out.flush();
}
