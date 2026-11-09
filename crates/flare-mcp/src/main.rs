//! `flare-mcp.exe`: the stdio MCP server. stdout carries protocol messages only; nothing is logged.
//! With `--hook-context` it is Claude Code's prompt hook instead (`flare_mcp::hook`).

use std::io::{self, BufReader};

fn main() {
    if std::env::args().nth(1).as_deref() == Some(flare_mcp::hook::HOOK_FLAG) {
        flare_mcp::hook::hook_main();
        return;
    }
    let mut channel = flare_mcp::FlareChannel::from_env();
    // Not locked: stdin is read on a thread of its own, so a cancel is seen while a call runs.
    let stdin = BufReader::new(io::stdin());
    // Not locked for the whole run: the watcher thread writes its notification through the same handle, and
    // each `writeln!` takes the lock for one whole line.
    let stdout = io::stdout();
    let notify: flare_mcp::Notify = std::sync::Arc::new(|| {
        use std::io::Write;
        let mut out = io::stdout();
        // One whole line in one write, so it cannot interleave with a reply.
        let _ = out.write_all(flare_mcp::LIST_CHANGED_LINE.as_bytes());
        let _ = out.flush();
    });
    if let Err(e) = flare_mcp::run_notifying(stdin, stdout, &mut channel, Some(notify)) {
        eprintln!("flare-mcp: {e}");
        std::process::exit(1);
    }
}
