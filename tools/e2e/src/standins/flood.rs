//! flood_file and long_output_cmd: deterministic floods of output (Task 15).

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Every flood line is this many bytes, the newline included.
const LINE_BYTES: usize = 100;

/// Writes `mb` mebibytes of 100-byte lines (`LINE 000000001 xxx...`) into `dir` and returns the
/// file. The file is exactly `mb * 1048576` bytes; the last line is `last_line` with a run of
/// filler in front of it, so it closes the file. Deterministic: the same arguments give the same bytes.
pub fn flood_file(dir: &Path, mb: usize, last_line: &str) -> PathBuf {
    let total = mb * 1024 * 1024;
    assert!(
        total > LINE_BYTES + last_line.len() + 1,
        "the flood is too small"
    );
    let path = dir.join(format!("flood-{mb}mb.txt"));
    let file = std::fs::File::create(&path).expect("create the flood file");
    let mut out = BufWriter::new(file);
    // Full lines first; the remainder (100 to 199 bytes) becomes the closing line.
    let full = (total - LINE_BYTES) / LINE_BYTES;
    for i in 1..=full {
        let head = format!("LINE {i:09} ");
        let filler = LINE_BYTES - 1 - head.len();
        let mut line = head;
        line.extend(std::iter::repeat_n('x', filler));
        line.push('\n');
        out.write_all(line.as_bytes()).expect("write the flood");
    }
    let rest = total - full * LINE_BYTES;
    let pad = rest - 1 - last_line.len();
    let mut last = "-".repeat(pad);
    last.push_str(last_line);
    last.push('\n');
    out.write_all(last.as_bytes()).expect("write the flood");
    out.flush().expect("flush the flood");
    path
}

/// The cmd line that prints `lines` numbered lines (`line 1` ... `line N`).
pub fn long_output_cmd(lines: usize) -> String {
    format!("for /L %i in (1,1,{lines}) do @echo line %i")
}
