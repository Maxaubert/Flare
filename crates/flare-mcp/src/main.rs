//! `flare-mcp.exe`: the stdio MCP server. stdout carries protocol messages only; nothing is logged.

use std::io::{self, BufReader};

fn main() {
    let mut channel = flare_mcp::FlareChannel::from_env();
    let stdin = BufReader::new(io::stdin().lock());
    let stdout = io::stdout().lock();
    if let Err(e) = flare_mcp::run(stdin, stdout, &mut channel) {
        eprintln!("flare-mcp: {e}");
        std::process::exit(1);
    }
}
