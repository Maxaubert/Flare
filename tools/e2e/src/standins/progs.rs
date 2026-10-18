//! Small programs the scenarios run in a shell: a mouse reporter, the alt screen, OSC 52, bell, OSC 8,
//! DA1 and colour queries (Task 15).
//!
//! Each program is a `<name>.ps1` (the body, written with `[Console]::Write`) and a `<name>.cmd`
//! that runs it with Windows PowerShell. The function returns the `.cmd`, which is what a shell runs.
//! Programs that block on the console (the mouse reporter, the alt screen, the two query programs)
//! wait for input and are checked by their text in the tests; the rest print and exit.

use std::path::{Path, PathBuf};

/// `$e + '<seq>'`: the escape character followed by a literal sequence. Quotes are doubled.
fn esc(seq: &str) -> String {
    format!("$e + {}", ps_quote(seq))
}

/// A PowerShell single-quoted literal.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Writes `<name>.ps1` with `body` and the `<name>.cmd` that runs it; returns the `.cmd`.
fn write_prog(dir: &Path, name: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("the program folder");
    let ps1 = dir.join(format!("{name}.ps1"));
    std::fs::write(&ps1, format!("$e = [string][char]27\r\n{body}\r\n")).expect("write the .ps1");
    let cmd = dir.join(format!("{name}.cmd"));
    std::fs::write(
        &cmd,
        format!("@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \"%~dp0{name}.ps1\"\r\n"),
    )
    .expect("write the .cmd");
    cmd
}

/// Reads the console replies for two seconds, then writes them back as hex-free text after `label`.
const COLLECT_REPLIES: &str = r#"$buf = ''
$end = (Get-Date).AddSeconds(2)
while ((Get-Date) -lt $end) {
  if ([Console]::KeyAvailable) { $buf += [Console]::ReadKey($true).KeyChar } else { Start-Sleep -Milliseconds 20 }
}
[Console]::Write('REPLY ' + $buf + "`r`n")"#;

/// Enables mouse tracking (`?1000h` plus SGR `?1006h`) and prints each key it is sent as a
/// report, until `q`.
pub fn mouse_reporter(dir: &Path) -> PathBuf {
    let on = format!("{} + {}", esc("[?1000h"), esc("[?1006h"));
    let off = format!("{} + {}", esc("[?1006l"), esc("[?1000l"));
    let body = format!(
        r#"[Console]::Write({on})
while ($true) {{
  $k = [Console]::ReadKey($true)
  if ($k.KeyChar -eq 'q') {{ break }}
  [Console]::Write('REPORT ' + [int]$k.KeyChar + "`r`n")
}}
[Console]::Write({off})"#,
        on = on,
        off = off,
    );
    write_prog(dir, "mouse-reporter", &body)
}

/// Enters the alternate screen, prints a marker, and waits for `q` to leave it.
pub fn alt_screen(dir: &Path) -> PathBuf {
    let body = format!(
        r#"[Console]::Write({enter} + 'ALT-SCREEN' + "`r`n")
while ($true) {{
  if ([Console]::ReadKey($true).KeyChar -eq 'q') {{ break }}
}}
[Console]::Write({leave})"#,
        enter = esc("[?1049h"),
        leave = esc("[?1049l"),
    );
    write_prog(dir, "alt-screen", &body)
}

/// Writes an OSC 52 clipboard request carrying `base64` (the program sends it and exits).
pub fn osc52_writer(dir: &Path, base64: &str) -> PathBuf {
    let body = format!(
        "[Console]::Write($e + {} + {base64} + [char]7)",
        ps_quote("]52;c;"),
        base64 = ps_quote(base64),
    );
    write_prog(dir, "osc52-writer", &body)
}

/// Rings the bell `n` times and exits.
pub fn bell(dir: &Path, n: usize) -> PathBuf {
    let body = format!("for ($i = 0; $i -lt {n}; $i++) {{ [Console]::Write([char]7) }}");
    write_prog(dir, "bell", &body)
}

/// Prints `label` as an OSC 8 hyperlink to `uri` and exits.
pub fn osc8(dir: &Path, uri: &str, label: &str) -> PathBuf {
    let body = format!(
        "[Console]::Write($e + {open} + {uri} + [char]7 + {label} + $e + {close} + [char]7)",
        open = ps_quote("]8;;"),
        uri = ps_quote(uri),
        label = ps_quote(label),
        close = ps_quote("]8;;"),
    );
    write_prog(dir, "osc8", &body)
}

/// Sends a primary device attributes query (`CSI c`) and prints the reply it gets.
pub fn da1_dump(dir: &Path) -> PathBuf {
    let body = format!("[Console]::Write({})\r\n{COLLECT_REPLIES}", esc("[c"));
    write_prog(dir, "da1-dump", &body)
}

/// Asks for the foreground and background colours (OSC 10 and 11) and prints the replies.
pub fn color_query(dir: &Path) -> PathBuf {
    let body = format!(
        "[Console]::Write($e + {fg} + [char]7 + $e + {bg} + [char]7)\r\n{COLLECT_REPLIES}",
        fg = ps_quote("]10;?"),
        bg = ps_quote("]11;?"),
    );
    write_prog(dir, "color-query", &body)
}
