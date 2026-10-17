mod backend;
mod config;

slint::include_modules!();

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
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

    // Task 16 wires the window here.
    log::debug!(
        "options: renderer={:?} screenshot={:?} scale={} size={:?} bench={:?}",
        args.renderer,
        args.screenshot,
        args.scale,
        args.size,
        args.bench_frames
    );
}
