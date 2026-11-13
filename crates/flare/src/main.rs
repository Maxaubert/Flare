#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_hub;
mod app_ask;
mod app_facts;
mod app_icon;
mod appctl;
mod ask_agent;
mod backend;
mod badge;
mod chords;
mod chrome_apply;
mod cli_open;
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
mod hidden_start;
mod host;
mod ime;
mod keymap;
mod live;
mod look_ctl;
mod mac_menu;
mod machines;
mod machines_page;
mod mark_colours;
mod material_ctl;
mod memory;
mod menus_ui;
mod mods_ctl;
mod openers;
mod panel_ctl;
mod panel_sel;
mod picker_ctl;
mod plugin_ask;
mod plugin_bar;
mod plugin_install;
mod plugin_look;
mod plugin_safe;
mod plugin_sessions;
mod plugin_ui;
mod remote_convo;
mod remote_files;
mod remote_host;
mod remote_link;
mod remote_status;
mod remote_tunnel;
mod scrollbar;
mod serve_remote;
mod sessions;
mod settings_ctl;
mod settings_scene;
mod ship_ctl;
mod side_slide;
mod sidebar_ui;
mod ssh_config;
mod startup;
mod startup_trace;
mod tab_host;
mod tabs_ui;
mod term_menu;
mod term_ui;
mod theme_scene;
mod thumbs;
mod timer_registry;
mod ui_clock;
mod ui_font;
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
    let mut args = match config::parse_args(&argv, env_renderer.as_deref()) {
        Ok(args) => args,
        Err(message) => {
            // A release build has no console of its own: attach first or a bad flag fails silently.
            attach_parent_console();
            eprintln!("flare: {message}");
            std::process::exit(1);
        }
    };
    // Rule 1207: a renamed copy (`FlareStable.exe`) with no --user-data-dir runs on `%APPDATA%\<its name>`, as if the
    // flag named it, so its profile, lock, pipe and a hidden start all agree (the serve process ssh starts included).
    args.user_data_dir = startup::effective_user_data_dir(args.user_data_dir.take());

    startup_trace::mark(startup_trace::Phase::Args);

    // `--serve-remote` (remote sessions PR 2): the host end of a remote link, before anything else starts. It takes no
    // single-instance lock, opens no window and sets up no e2e (rule 283); stdout is the link's alone (rule 277).
    if args.serve_remote {
        // `--no-start` (the process tests): never a hidden start, the relay is `unavailable` with no Flare running.
        std::process::exit(serve_remote::run(
            args.user_data_dir.as_deref(),
            args.e2e,
            !args.no_start,
        ));
    }

    // `flare open --on <machine> <folder>` (remote sessions PR 5): a command, not a launch. It asks the running Flare
    // (started hidden when none runs) for the tab and exits; its values never reach the hand-off's path parsing
    // (rule 300), and it takes no lock and opens no window of its own.
    if let Some(open) = &args.open_on {
        attach_parent_console();
        std::process::exit(cli_open::run(open, args.user_data_dir.as_deref(), args.e2e));
    }

    // macOS: the shared application is CEF's kind before winit makes it, so the web panel's engine can start
    // later (port phase 5; rule 1023). Windows has WebView2 and nothing to prepare. After the serve and `open --on`
    // branches: neither is an app, so neither makes an NSApplication, registers with LaunchServices or reaches for the
    // window server (rule 1196: ssh starts the serve process in a login with no desktop).
    #[cfg(target_os = "macos")]
    if let Err(e) = flare_web::host::prepare_app() {
        log::warn!("the web engine cannot run in this process: {e}");
    }

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
        // ConPTY is Windows-only; the macOS pty needs no bundle (port plan phase 2).
        #[cfg(windows)]
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
        // ConPTY is Windows-only; the macOS pty needs no bundle (port plan phase 2).
        #[cfg(windows)]
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
    // macOS: an --e2e window and a `--start-hidden` one (remote sessions PR 5, rules 1107, 301, 302) are never pulled
    // onto a screen and are born clear and click-through, from their first order-in, and kept from App Nap, so a hidden
    // host's live shells keep running while nobody looks. A `--start-hidden` run lets go of all of it at its first raise.
    #[cfg(target_os = "macos")]
    if window::parked_start(args.e2e, args.start_hidden) {
        flare_win::window_park::keep_windows_off_screen();
        flare_win::window_park::hold_off_app_nap();
    }
    // Single instance: a second launch hands its folders to the first and exits 0. The lock and
    // the pipe are keyed by the profile folder, so an e2e run (its own profile) never reaches the
    // owner's running Flare.
    let instance = if args.sample {
        None
    } else {
        let appdata = flare_win::paths::roaming_app_data();
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
                appctl::staged_glue::guard_at_start(&argv, args.user_data_dir.as_deref(), args.e2e);
                Some(window::Instance::start(profile, server_names, guard))
            }
            Err(message) => {
                attach_parent_console();
                eprintln!("flare: {message}");
                std::process::exit(1);
            }
        }
    };
    // The window is born parked under `--e2e` and `--start-hidden` alike (rule 301).
    let born_parked = window::parked_start(args.e2e, args.start_hidden);
    let outcome = match backend::select_backend_with(args.renderer, born_parked, prewarm) {
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
    // ConPTY is Windows-only; the macOS pty needs no bundle (port plan phase 2).
    #[cfg(windows)]
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
#[cfg_attr(not(windows), allow(dead_code))] // only the ConPTY check calls it so far
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
#[cfg_attr(not(windows), allow(dead_code))]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
fn message_box(_title: &str, _message: &str) {}

#[cfg(test)]
mod serve_order_tests {
    /// Rule 283: the serve branch comes before the e2e set-up, the instance code and the window, so a serve process
    /// can never take the lock, start a window or be mistaken for a launch.
    #[test]
    fn serve_remote_branches_before_the_lock_the_e2e_and_the_window() {
        let src = include_str!("main.rs");
        let at = |s: &str| src.find(s).unwrap_or_else(|| panic!("{s} is in main.rs"));
        let serve = at("if args.serve_remote {");
        for later in [
            "e2e::init(args.e2e)",
            "startup::launch(&profile",
            "window::run_window(",
            "backend::select_backend_with(",
            "conpty::verify_bundled()",
            // Rule 1196: no NSApplication in a serve process.
            "flare_web::host::prepare_app()",
        ] {
            assert!(
                serve < at(later),
                "the serve branch must come before {later}"
            );
        }
    }

    /// Rule 300: `flare open --on` is a command that exits before the lock, the hand-off (which reads the command line
    /// for folders) and the window, so its values never reach path parsing and it never opens a window itself.
    #[test]
    fn open_on_branches_before_the_hand_off_and_the_window() {
        let src = include_str!("main.rs");
        let at = |s: &str| src.find(s).unwrap_or_else(|| panic!("{s} is in main.rs"));
        let open = at("if let Some(open) = &args.open_on {");
        for later in [
            "flare_web::host::prepare_app()",
            "e2e::init(args.e2e)",
            "startup::launch(&profile",
            "window::run_window(",
        ] {
            assert!(open < at(later), "the open branch must come before {later}");
        }
    }
}

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
