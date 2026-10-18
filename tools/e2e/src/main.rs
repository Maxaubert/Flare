//! The `flare-e2e` driver. Task 1 lists the scenario table (`--list`) and runs the named scenarios
//! in-process; every stub fails with "not ported yet". Task 13 replaces this with the runner.

use flare_e2e::ctx::Ctx;
use flare_e2e::scenarios::ALL;

fn main() {
    let words: Vec<String> = std::env::args().skip(1).collect();
    if words.iter().any(|w| w == "--list") {
        for s in ALL {
            println!("{}", s.name);
        }
        return;
    }
    let filters: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let picked: Vec<_> = ALL
        .iter()
        .filter(|s| {
            let name = s.name.to_lowercase();
            filters.is_empty() || filters.iter().any(|f| name.contains(f.as_str()))
        })
        .collect();
    if picked.is_empty() {
        println!("no scenario matches {}", words.join(" "));
        std::process::exit(1);
    }
    let mut failed = 0_usize;
    for s in picked {
        println!("\n{}", s.name);
        let mut ctx = Ctx::new(s.name);
        if (s.run)(&mut ctx).is_err() || !ctx.fails.is_empty() {
            failed += 1;
        }
    }
    if failed > 0 {
        println!("{failed} scenario(s) FAILED");
        std::process::exit(1);
    }
    println!("all scenarios passed");
}
