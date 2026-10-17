mod backend;
mod config;
mod host;
mod memory;
mod window;

slint::include_modules!();

fn main() {
    let start = std::time::Instant::now();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let env_renderer = std::env::var("FLARE_RENDERER").ok();
    let args = match config::parse_args(&argv, env_renderer.as_deref()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("flare: {message}");
            std::process::exit(1);
        }
    };

    if args.probe_backend {
        match backend::select_backend(args.renderer) {
            Ok(outcome) => {
                let name = match outcome.effective {
                    config::RendererChoice::Gpu => "gpu",
                    config::RendererChoice::Software => "software",
                };
                match outcome.fallback_reason {
                    Some(reason) => println!("backend: {name} fallback: {reason}"),
                    None => println!("backend: {name}"),
                }
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("flare: backend selection failed: {e}");
                std::process::exit(1);
            }
        }
    }

    if args.screenshot.is_some() || args.bench_frames.is_some() {
        // Task 17 adds `headless::run(args, start)` here: it exits 2 when its error starts
        // with "GPU unavailable", else 1.
        eprintln!("flare: --screenshot and --bench-frames are not available yet");
        std::process::exit(1);
    }

    let outcome = match backend::select_backend(args.renderer) {
        Ok(outcome) => outcome,
        Err(e) => {
            eprintln!("flare: backend selection failed: {e}");
            std::process::exit(1);
        }
    };
    log::info!(
        "renderer: {:?} (fallback: {:?})",
        outcome.effective,
        outcome.fallback_reason
    );
    if let Err(e) = window::run_window(outcome, start) {
        eprintln!("flare: {e}");
        std::process::exit(1);
    }
}
