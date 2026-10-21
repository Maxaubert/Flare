//! The `flare-e2e` driver. `flare-e2e [filter...] [--list] [--exe <path>] [--packaged] [--shots <dir>]
//! [--keep] [perf [name...]]`; `--run-one <name>` is the per-scenario child the parent starts.

use flare_e2e::runner::{run_all, run_one, Cli};
use flare_e2e::scenarios::ALL;

fn main() {
    let cli = match Cli::parse(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(e) => {
            println!("{e}");
            std::process::exit(2);
        }
    };
    if cli.list {
        for s in ALL {
            println!("{}", s.name);
        }
        return;
    }
    if cli.perf {
        std::process::exit(flare_e2e::perf::main(&cli));
    }
    let code = match &cli.run_one {
        Some(name) => run_one(name, &cli),
        None => run_all(&cli),
    };
    std::process::exit(code);
}
