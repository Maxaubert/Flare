//! A stand-in for flare-parakeet-host.exe, for the host tests (Task 14). Tests only, never packed. The
//! same protocol on stdin and stdout, ported from Prism Terminal's `fakeParakeetHost.mjs` (e5848cb).
//! argv: `<engine folder> <model> <gpu 1|0> <threads> [<cache>]`, then the switches below. Each answer's
//! text is JSON of what it saw: the pass number, the bytes, its pid.
//!
//! Switches (argv, or `FAKE_HOST=<word>` in the environment as in Prism Terminal's fixture):
//!   --fake-never        FAKE_HOST=never    never say "ready" (alive and silent)
//!   --fake-err          FAKE_HOST=err      answer every pass `err boom`
//!   --fake-die          FAKE_HOST=die      on the first pass say why on stderr and exit 5
//!   --fake-noise        FAKE_HOST=noise    print lines that are none of its words before every protocol line
//!   --fake-delay <ms>   FAKE_HOST_DELAY=<ms>  hold every answer this long

use std::io::{Read, Write};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let env_mode = std::env::var("FAKE_HOST").unwrap_or_default();
    let flag = |name: &str, word: &str| argv.iter().any(|a| a == name) || env_mode == word;
    let (dir, model, gpu) = (
        argv.first().cloned().unwrap_or_default(),
        argv.get(1).cloned().unwrap_or_default(),
        argv.get(2).is_some_and(|g| g == "1"),
    );
    let delay: u64 = argv
        .iter()
        .position(|a| a == "--fake-delay")
        .and_then(|i| argv.get(i + 1))
        .cloned()
        .or_else(|| std::env::var("FAKE_HOST_DELAY").ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let (never, err, die, noise) = (
        flag("--fake-never", "never"),
        flag("--fake-err", "err"),
        flag("--fake-die", "die"),
        flag("--fake-noise", "noise"),
    );
    if never {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let mut out = std::io::stdout().lock();
    let mut say = |line: &str| {
        let _ = out.write_all(format!("{line}\n").as_bytes());
        let _ = out.flush();
    };
    if noise {
        say("loading model...");
        say("readyish");
    }
    say("ready");

    let mut input = std::io::stdin().lock();
    let mut count = 0u64;
    loop {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            if input.read(&mut byte).unwrap_or(0) == 0 {
                return;
            }
            if byte[0] == b'\n' {
                break;
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).trim().to_string();
        if head == "quit" {
            return;
        }
        let need: usize = head
            .strip_prefix("pass ")
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0);
        let mut wav = vec![0u8; need];
        if input.read_exact(&mut wav).is_err() {
            return;
        }
        count += 1;
        if die {
            eprintln!("fake: host crashed: CUDA error");
            std::process::exit(5);
        }
        std::thread::sleep(Duration::from_millis(delay));
        if noise {
            say("okay then");
            say("[ggml] a library printed this");
        }
        if err {
            say("err boom");
            continue;
        }
        let text = serde_json::json!({
            "n": count,
            "bytes": need,
            "pid": std::process::id(),
            "gpu": gpu,
            "model": model,
            "dir": dir,
            "args": argv,
        })
        .to_string();
        say(&format!("ok {}", STANDARD.encode(text.as_bytes())));
    }
}
