#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod config;
mod headless;
mod host;
mod keymap;
mod live;
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
            // A release build has no console of its own: attach first or a bad flag fails silently.
            attach_parent_console();
            eprintln!("flare: {message}");
            std::process::exit(1);
        }
    };

    if args.probe_backend || args.screenshot.is_some() || args.bench_frames.is_some() {
        attach_parent_console();
    }

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
        if let Err(message) = headless::run(args, start) {
            eprintln!("flare: {message}");
            let code = if message.starts_with("GPU unavailable") {
                2
            } else {
                1
            };
            std::process::exit(code);
        }
        std::process::exit(0);
    }

    let outcome = match backend::select_backend(args.renderer) {
        Ok(outcome) => outcome,
        Err(e) => {
            attach_parent_console();
            eprintln!("flare: backend selection failed: {e}");
            std::process::exit(1);
        }
    };
    log::info!(
        "renderer: {:?} (fallback: {:?})",
        outcome.effective,
        outcome.fallback_reason
    );
    if let Err(e) = window::run_window(outcome, start, false) {
        attach_parent_console();
        eprintln!("flare: {e}");
        std::process::exit(1);
    }
}

/// The release build has no console of its own (windows subsystem). A headless mode run from a
/// terminal attaches to the parent's console so its output shows; redirected pipes and files
/// are unaffected, and with no parent console this does nothing.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: AttachConsole takes a process id constant, touches no memory of ours and fails
    // harmlessly (returns 0) when there is no parent console.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}
