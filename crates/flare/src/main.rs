#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appctl;
mod backend;
mod chords;
mod config;
mod e2e;
mod headless;
mod host;
mod keymap;
mod live;
mod memory;
mod menus_ui;
mod sidebar_ui;
mod startup;
mod tab_host;
mod tabs_ui;
mod window;

#[cfg(test)]
mod ui_harness;
#[cfg(test)]
mod ui_tests;

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

    if args.probe_backend
        || args.screenshot.is_some()
        || args.bench_frames.is_some()
        || args.live_screenshot.is_some()
    {
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

    if args.live_screenshot.is_some() {
        // The bundled ConPTY is checked before any live session starts (exit code 3).
        if let Err(e) = flare_term::conpty::verify_bundled() {
            show_fatal("Flare cannot start", &e.to_string(), false);
            std::process::exit(3);
        }
        if let Err(message) = headless::run_live(args, start) {
            eprintln!("flare: {message}");
            std::process::exit(1);
        }
        std::process::exit(0);
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
    if let Err(e) = flare_term::conpty::verify_bundled() {
        show_fatal("Flare cannot start", &e.to_string(), true);
        std::process::exit(3);
    }
    if let Err(e) = window::run_window(outcome, start, args.sample) {
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

/// Says why Flare cannot start: always on stderr, and in a message box when `dialog` is set and
/// this run has no console (the release build, or a window started from Explorer). A test run
/// never passes `dialog`, so no box can block it.
fn show_fatal(title: &str, message: &str, dialog: bool) {
    eprintln!("flare: {message}");
    if dialog && (!cfg!(debug_assertions) || !console_attached()) {
        message_box(title, message);
    }
}

/// Whether this process has a console window to write to.
#[cfg(windows)]
fn console_attached() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    // SAFETY: GetConsoleWindow takes no arguments and only reads the process's console state.
    !unsafe { GetConsoleWindow() }.is_null()
}

#[cfg(not(windows))]
fn console_attached() -> bool {
    true
}

#[cfg(windows)]
fn message_box(title: &str, message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let text = wide(message);
    let caption = wide(title);
    // SAFETY: both buffers are NUL-terminated UTF-16 and outlive the call; a null owner makes
    // the box an unowned one, and the call returns once the user closes it.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_ICONERROR | MB_OK,
        );
    }
}

#[cfg(not(windows))]
fn message_box(_title: &str, _message: &str) {}
