#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_hub;
mod app_facts;
mod appctl;
mod ask_agent;
mod backend;
mod badge;
mod chords;
mod chrome_apply;
mod clip_flow;
mod config;
mod convo_ctl;
mod ctl_app;
mod diag_ctl;
mod dictation_ctl;
mod dictation_page;
mod dictation_scenes;
mod dictation_widget;
mod download_ask;
mod drop_flow;
mod e2e;
mod e2e_glue;
mod e2e_hook;
mod e2e_probes;
mod font_ctl;
mod headless;
mod help_ctl;
mod host;
mod ime;
mod keymap;
mod live;
mod look_ctl;
mod mark_colours;
mod material_ctl;
mod memory;
mod menus_ui;
mod mods_ctl;
mod openers;
mod panel_ctl;
mod picker_ctl;
mod plugin_ask;
mod plugin_bar;
mod plugin_install;
mod plugin_look;
mod plugin_safe;
mod plugin_sessions;
mod plugin_ui;
mod scrollbar;
mod sessions;
mod settings_ctl;
mod settings_scene;
mod ship_ctl;
mod side_slide;
mod sidebar_ui;
mod startup;
mod startup_trace;
mod tab_host;
mod tabs_ui;
mod term_menu;
mod term_ui;
mod theme_scene;
mod timer_registry;
mod ui_clock;
mod update_ctl;
mod web_ctl;
mod window;

#[cfg(test)]
mod theme_tests;
#[cfg(test)]
mod ui_harness;
#[cfg(test)]
mod ui_tests;

slint::include_modules!();

fn main() {
    // Velopack's hooks may end the process here, before anything else starts.
    ship_ctl::bootstrap();
    startup_trace::mark(startup_trace::Phase::Main);
    let start = std::time::Instant::now();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let env_renderer = std::env::var("FLARE_RENDERER").ok();
    // The GPU device is built on a worker thread while the rest of start-up runs (Task 64).
    let prewarm = backend::prewarm_if_gpu(&argv, env_renderer.as_deref());
    let args = match config::parse_args(&argv, env_renderer.as_deref()) {
        Ok(args) => args,
        Err(message) => {
            // A release build has no console of its own: attach first or a bad flag fails silently.
            attach_parent_console();
            eprintln!("flare: {message}");
            std::process::exit(1);
        }
    };

    startup_trace::mark(startup_trace::Phase::Args);

    if args.probe_backend
        || args.screenshot.is_some()
        || args.bench_frames.is_some()
        || args.live_screenshot.is_some()
        || args.scene.is_some()
    {
        attach_parent_console();
    }

    // `--scene <name>` draws one fixed state of the window (Task 32) and exits, like --screenshot.
    if args.scene.is_some() {
        match headless::run_scene(&args) {
            Ok(()) => std::process::exit(0),
            Err(message) => {
                eprintln!("flare: {message}");
                let code = if message.starts_with("GPU unavailable") {
                    2
                } else {
                    1
                };
                std::process::exit(code);
            }
        }
    }

    // Hidden test modes of the process tests (Task 34); neither opens a window.
    if args.instance_probe {
        std::process::exit(startup::run_instance_probe(
            args.user_data_dir.as_deref(),
            &argv_with_exe(),
        ));
    }
    if let (Some(n), None) = (args.live_tabs, args.scene.as_ref()) {
        if let Err(e) = flare_term::conpty::verify_bundled() {
            show_fatal("Flare cannot start", &e.to_string(), false);
            std::process::exit(3);
        }
        std::process::exit(startup::run_live_tabs(args.user_data_dir.as_deref(), n));
    }

    if args.probe_backend {
        match backend::select_backend(args.renderer, false) {
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

    e2e::init(args.e2e);
    // Single instance: a second launch hands its folders to the first and exits 0. The lock and
    // the pipe are keyed by the profile folder, so an e2e run (its own profile) never reaches the
    // owner's running Flare.
    let instance = if args.sample {
        None
    } else {
        let appdata = std::env::var("APPDATA").ok();
        let profile = startup::Profile {
            dir: startup::profile_dir(args.user_data_dir.as_deref(), appdata.as_deref()),
        };
        if let Err(e) = std::fs::create_dir_all(&profile.dir) {
            // sync-ok: startup, before any window
            log::warn!(
                "cannot create the profile folder {}: {e}",
                profile.dir.display()
            );
        }
        if let Some(pid) = args.after_pid {
            // Restarted by an install: the old build is quitting. Wait for it, so the lock is free and this one
            // is the first instance at once (not after the hand-off's wait, or never if the quit is slow).
            if !flare_win::procs::wait_for_exit(pid, AFTER_PID_WAIT) {
                log::warn!("the old Flare (pid {pid}) is still running; starting anyway");
            }
        }
        match startup::launch(&profile, &argv_with_exe(), HANDOFF_WAIT) {
            Ok(startup::Launch::HandedOver) => std::process::exit(0),
            Ok(startup::Launch::First {
                guard,
                server_names,
            }) => {
                // The last-good guard of an installed copy (install when idle): counts this start of a new
                // build, and puts the old one back after two early deaths.
                appctl::staged_glue::guard_at_start(&argv, args.user_data_dir.as_deref());
                Some(window::Instance::start(profile, server_names, guard))
            }
            Err(message) => {
                attach_parent_console();
                eprintln!("flare: {message}");
                std::process::exit(1);
            }
        }
    };
    let outcome = match backend::select_backend_with(args.renderer, args.e2e, prewarm) {
        Ok(outcome) => outcome,
        Err(e) => {
            attach_parent_console();
            eprintln!("flare: backend selection failed: {e}");
            std::process::exit(1);
        }
    };
    startup_trace::mark(startup_trace::Phase::Backend);
    log::info!(
        "renderer: {:?} (fallback: {:?})",
        outcome.effective,
        outcome.fallback_reason
    );
    // The diagnostics log starts here: the profile is known, this run is the first instance, and no
    // window or tab exists yet. The session line says which renderer won.
    if let Some(inst) = &instance {
        let backend = match outcome.effective {
            config::RendererChoice::Gpu => "gpu",
            config::RendererChoice::Software => "software",
        };
        startup::start_diagnostics(&inst.profile, backend, args.e2e);
    }
    if let Err(e) = flare_term::conpty::verify_bundled() {
        show_fatal("Flare cannot start", &e.to_string(), true);
        std::process::exit(3);
    }
    if let Err(e) = window::run_window(outcome, start, &args, instance) {
        attach_parent_console();
        eprintln!("flare: {e}");
        std::process::exit(1);
    }
    end_process_clean();
}

/// Ends a quit that finished cleanly with exit code 0. By now the shells are gone, the settings are
/// flushed and the instance lock is released. On the GPU renderer the DXGI/DirectComposition
/// teardown that runs while the process exits rewrote the exit code to 2170 (the DXGI facility), so
/// the process ends itself and skips that teardown.
fn end_process_clean() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        // SAFETY: the pseudo-handle of this process is always valid and nothing is left to run.
        unsafe {
            TerminateProcess(GetCurrentProcess(), 0);
        }
    }
    std::process::exit(0);
}

/// How long a second launch waits for the first instance to take its folders. It also covers a
/// first instance that is quitting: it stops listening at once, and the lock comes free after
/// the shells are gone (up to 3 s), when the second launch takes over as the first.
const HANDOFF_WAIT: std::time::Duration = std::time::Duration::from_secs(6);
/// How long a build restarted by an install waits for the old one to end (it waits for its shells and agents).
const AFTER_PID_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// The command line with the exe first, as `folders_from_argv` expects it.
fn argv_with_exe() -> Vec<String> {
    std::env::args().collect()
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
    if fatal_dialog_shows(
        dialog,
        cfg!(debug_assertions),
        console_attached(),
        e2e::global().on(),
    ) {
        message_box(title, message);
    }
}

/// Whether a fatal error is also shown in a message box: never under --e2e (no window is ever
/// shown to the owner), and not in a debug run that has a console to print to.
fn fatal_dialog_shows(dialog: bool, debug: bool, console: bool, e2e: bool) -> bool {
    dialog && !e2e && (!debug || !console)
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

#[cfg(test)]
mod fatal_tests {
    use super::fatal_dialog_shows;

    #[test]
    fn an_e2e_run_never_shows_the_fatal_message_box() {
        // A release run with no console shows the box; an --e2e run never does (no window is
        // ever shown to the owner), and a debug run with a console prints instead.
        assert!(fatal_dialog_shows(true, false, false, false));
        assert!(!fatal_dialog_shows(true, false, false, true));
        assert!(!fatal_dialog_shows(true, true, true, false));
        assert!(fatal_dialog_shows(true, true, false, false));
        assert!(!fatal_dialog_shows(false, false, false, false));
    }
}
