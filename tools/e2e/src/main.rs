//! The `flare-e2e` driver. `flare-e2e [filter...] [--list] [--exe <path>] [--packaged] [--shots <dir>]
//! [--keep] [perf [name...]]`; `--run-one <name>` is the per-scenario child the parent starts.
//! `--fake-fill-helper <record file>` is the `pluginFill` scenario's plugin helper (a copy of this exe that Flare
//! starts), never a driver run.

#[cfg(any(windows, target_os = "macos"))]
use flare_e2e::runner::{run_all, run_one, Cli};
#[cfg(any(windows, target_os = "macos"))]
use flare_e2e::scenarios::{ALL, OPT_IN};

/// The harness drives the Windows and the Mac app (rule 1104); no other system has a build.
#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    println!("flare-e2e runs on Windows and macOS only");
    std::process::exit(2);
}

#[cfg(any(windows, target_os = "macos"))]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--fake-fill-helper") {
        let file = args.get(2).map_or_else(
            || std::path::PathBuf::from("fake-fill-helper.jsonl"),
            std::path::PathBuf::from,
        );
        std::process::exit(flare_e2e::fake_fill_helper::run(&file));
    }
    let cli = match Cli::parse(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(e) => {
            println!("{e}");
            std::process::exit(2);
        }
    };
    if cli.fetch_fixtures {
        // Downloads only what the cache lacks, by the pins; runs no scenario (M8 D10).
        match flare_e2e::fixtures::fetch_all() {
            Ok(()) => println!("fixture cache is complete"),
            Err(e) => {
                println!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if cli.list {
        for s in ALL.iter().chain(OPT_IN) {
            #[cfg(target_os = "macos")]
            if let Some(why) = flare_e2e::scenarios::mac_gate::reason(s.name) {
                println!("{}  (gated on macOS: {why})", s.name);
                continue;
            }
            println!("{}", s.name);
        }
        return;
    }
    #[cfg(windows)]
    if cli.perf {
        std::process::exit(flare_e2e::perf::main(&cli));
    }
    #[cfg(target_os = "macos")]
    if cli.perf {
        // The bar compares against the installed Prism Terminal on the owner's Windows PC (docs/performance.md).
        println!("perf runs on Windows only (it measures against the owner's baseline there)");
        std::process::exit(2);
    }
    let code = match &cli.run_one {
        Some(name) => run_one(name, &cli),
        None => run_all(&cli),
    };
    std::process::exit(code);
}
