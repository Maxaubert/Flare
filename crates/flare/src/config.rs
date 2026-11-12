//! Command-line options and the renderer choice (GPU or software).

use std::path::PathBuf;

use flare_settings::pages::PAGES;
use flare_theme::colour::{parse_colour, to_stored};
use flare_theme::edges::WindowEdges;
use flare_theme::fonts::font_id_or_default;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererChoice {
    Gpu,
    Software,
}

/// The developer look flags (decision D13): they beat the stored look for one run and write nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LookOverrides {
    pub theme: Option<String>,
    pub see_through: Option<u8>,
    pub font: Option<String>,
    pub font_pct: Option<u32>,
    pub accent: Option<String>,
    pub background: Option<String>,
    pub secondary: Option<String>,
    pub edges: Option<WindowEdges>,
}

/// What `--settings-open` leaves open in the Settings scene (Task 51).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsOpen {
    #[default]
    None,
    Select,
    Picker,
    Editor,
    Ask,
    Help,
    WallExpanded,
}

/// The words `--settings-open` takes, in the order the matrix lists them.
pub const SETTINGS_OPEN_WORDS: [&str; 7] = [
    "none",
    "select",
    "picker",
    "editor",
    "ask",
    "help",
    "wall-expanded",
];

impl SettingsOpen {
    pub fn from_word(w: &str) -> Option<SettingsOpen> {
        Some(match w {
            "none" => SettingsOpen::None,
            "select" => SettingsOpen::Select,
            "picker" => SettingsOpen::Picker,
            "editor" => SettingsOpen::Editor,
            "ask" => SettingsOpen::Ask,
            "help" => SettingsOpen::Help,
            "wall-expanded" => SettingsOpen::WallExpanded,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub renderer: RendererChoice,
    pub screenshot: Option<PathBuf>,
    pub scale: f32,
    pub size: (u32, u32),
    pub bench_frames: Option<u32>,
    pub probe_backend: bool,
    pub sample: bool,
    pub live_screenshot: Option<PathBuf>,
    pub live_shell: Option<String>,
    /// `--user-data-dir=<path>` (the equals form only).
    pub user_data_dir: Option<PathBuf>,
    /// `--e2e`: the window shows inactive, never raises, stays off the taskbar.
    pub e2e: bool,
    /// `--no-import`: the first-run import from Prism Terminal does not run this start.
    pub no_import: bool,
    /// `--no-plugins`: every plugin stays off this start (safe mode); the switches are back the next start.
    pub no_plugins: bool,
    /// `--after-pid N` (install when idle): the build that restarted into this one; start-up waits for it to end
    /// before it takes the single-instance lock, so the new window comes back the moment the old one is gone.
    pub after_pid: Option<u32>,
    /// `--scene <name>`: a headless scene for the screenshots.
    pub scene: Option<String>,
    /// `--live-tabs <n>`: how many live tabs a scene shows.
    pub live_tabs: Option<u32>,
    /// `--instance-probe`: answers whether this process was the first instance.
    pub instance_probe: bool,
    /// `--preview-update`: show the update chip and window with the sample offer (exact spelling only).
    pub preview_update: bool,
    /// `--settings-page <id>`: the page the settings scene shows (default appearance).
    pub settings_page: Option<String>,
    /// `--settings-find <text>`: typed into the settings scene's Find field.
    pub settings_find: Option<String>,
    /// `--settings-open <word>`: the overlay the settings scene leaves open.
    pub settings_open: SettingsOpen,
    /// `--theme`, `--see-through`, `--font`, `--font-pct`, `--accent`, `--background`, `--secondary`, `--edges`.
    pub look: LookOverrides,
    /// `--serve-remote` (remote sessions PR 2): the host end of a remote link on this process's own stdio, run by ssh.
    /// Headless: no window, no single-instance lock (rule 283).
    pub serve_remote: bool,
    /// `--no-start` (with `--serve-remote` only, remote sessions PR 5): the serve process never starts a Flare hidden;
    /// with none running its relay is `unavailable`. The process tests use it.
    pub no_start: bool,
    /// `--start-hidden` (remote sessions PR 5): a normal launch whose window is parked off every screen with the e2e's
    /// park code and never takes the focus (rule 301). The serve process and `flare open` start one when no Flare runs.
    pub start_hidden: bool,
    /// `flare open --on <machine> <folder> [--resume <id>]` (remote sessions PR 5): ask the running Flare (started
    /// with `--start-hidden` when none runs) to open a tab that shows a session on that machine, then exit.
    pub open_on: Option<OpenOn>,
    /// Bare arguments (folders or files), in order.
    pub positional: Vec<String>,
}

/// What `flare open --on` asks for: the machine's id in `machines.json`, a folder on it, a session to resume there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenOn {
    pub machine: String,
    pub folder: String,
    pub resume: Option<String>,
}

/// Flags: --renderer gpu|software, --screenshot <path>, --scale <f32> (default 1.0, allowed
/// 0.5..=4.0), --size <W>x<H> (default 1000x640, logical), --bench-frames <N>, --probe-backend.
/// Env FLARE_RENDERER=gpu|software applies when the flag is absent. Default Gpu.
/// Unknown flag or bad value -> Err(message).
pub fn parse_args(args: &[String], env_renderer: Option<&str>) -> Result<Args, String> {
    let mut renderer_flag: Option<RendererChoice> = None;
    let mut out = Args {
        renderer: RendererChoice::Gpu,
        screenshot: None,
        scale: 1.0,
        size: (1000, 640),
        bench_frames: None,
        probe_backend: false,
        sample: false,
        live_screenshot: None,
        live_shell: None,
        user_data_dir: None,
        e2e: false,
        no_import: false,
        no_plugins: false,
        after_pid: None,
        scene: None,
        live_tabs: None,
        instance_probe: false,
        preview_update: false,
        settings_page: None,
        settings_find: None,
        settings_open: SettingsOpen::None,
        look: LookOverrides::default(),
        serve_remote: false,
        no_start: false,
        start_hidden: false,
        open_on: None,
        positional: Vec::new(),
    };
    // `--on` and `--resume` take values: read here, so a value never reaches the positional list or the hand-off's
    // path parsing (rule 300).
    let mut on: Option<String> = None;
    let mut resume: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--renderer" => renderer_flag = Some(parse_renderer(value(args, &mut i, flag)?)?),
            "--screenshot" => out.screenshot = Some(PathBuf::from(value(args, &mut i, flag)?)),
            "--scale" => {
                let scale: f32 = value(args, &mut i, flag)?
                    .parse()
                    .map_err(|_| "--scale needs a number".to_string())?;
                if !(0.5..=4.0).contains(&scale) {
                    return Err(format!("--scale {scale} is out of range 0.5..=4.0"));
                }
                out.scale = scale;
            }
            "--size" => out.size = parse_size(value(args, &mut i, flag)?)?,
            "--bench-frames" => {
                let frames: u32 = value(args, &mut i, flag)?
                    .parse()
                    .map_err(|_| "--bench-frames needs a whole number".to_string())?;
                out.bench_frames = Some(frames);
            }
            "--probe-backend" => out.probe_backend = true,
            "--sample" => out.sample = true,
            "--live-screenshot" => {
                out.live_screenshot = Some(PathBuf::from(value(args, &mut i, flag)?));
            }
            "--live-shell" => out.live_shell = Some(value(args, &mut i, flag)?.to_string()),
            "--e2e" => out.e2e = true,
            "--no-import" => out.no_import = true,
            "--no-plugins" => out.no_plugins = true,
            "--after-pid" => {
                let pid: u32 = value(args, &mut i, flag)?
                    .parse()
                    .map_err(|_| "--after-pid needs a process id".to_string())?;
                out.after_pid = Some(pid);
            }
            "--instance-probe" => out.instance_probe = true,
            "--serve-remote" => out.serve_remote = true,
            "--no-start" => out.no_start = true,
            "--start-hidden" => out.start_hidden = true,
            "--on" => on = Some(value(args, &mut i, flag)?.to_string()),
            "--resume" => resume = Some(value(args, &mut i, flag)?.to_string()),
            "--preview-update" => out.preview_update = true,
            "--scene" => out.scene = Some(value(args, &mut i, flag)?.to_string()),
            "--live-tabs" => {
                let n: u32 = value(args, &mut i, flag)?
                    .parse()
                    .map_err(|_| "--live-tabs needs a whole number".to_string())?;
                out.live_tabs = Some(n);
            }
            "--settings-page" => {
                let id = value(args, &mut i, flag)?;
                if !PAGES.iter().any(|p| p.id == id) {
                    let ids: Vec<&str> = PAGES.iter().map(|p| p.id).collect();
                    return Err(format!(
                        "unknown settings page '{id}': use one of {}",
                        ids.join(", ")
                    ));
                }
                out.settings_page = Some(id.to_string());
            }
            "--settings-find" => out.settings_find = Some(value(args, &mut i, flag)?.to_string()),
            "--settings-open" => {
                let w = value(args, &mut i, flag)?;
                out.settings_open = SettingsOpen::from_word(w).ok_or_else(|| {
                    format!(
                        "unknown --settings-open '{w}': use one of {}",
                        SETTINGS_OPEN_WORDS.join(", ")
                    )
                })?;
            }
            "--theme" => out.look.theme = Some(value(args, &mut i, flag)?.to_string()),
            "--see-through" => {
                let n: u8 = value(args, &mut i, flag)?
                    .parse()
                    .map_err(|_| "--see-through needs a whole number from 0 to 70".to_string())?;
                if n > 70 {
                    return Err(format!("--see-through {n} is out of range 0..=70"));
                }
                out.look.see_through = Some(n);
            }
            "--font" => {
                let id = value(args, &mut i, flag)?;
                if font_id_or_default(Some(id)) != id {
                    return Err(format!("--font {id} is not a terminal face"));
                }
                out.look.font = Some(id.to_string());
            }
            "--font-pct" => {
                let n: u32 = value(args, &mut i, flag)?
                    .parse()
                    .map_err(|_| "--font-pct needs a whole number from 50 to 200".to_string())?;
                if !(50..=200).contains(&n) || !n.is_multiple_of(10) {
                    return Err(format!(
                        "--font-pct {n} is not a size step (50 to 200 in tens)"
                    ));
                }
                out.look.font_pct = Some(n);
            }
            "--accent" => {
                out.look.accent = Some(parse_look_colour(flag, value(args, &mut i, flag)?)?);
            }
            "--background" => {
                out.look.background = Some(parse_look_colour(flag, value(args, &mut i, flag)?)?);
            }
            "--secondary" => {
                out.look.secondary = Some(parse_look_colour(flag, value(args, &mut i, flag)?)?);
            }
            "--edges" => {
                let w = value(args, &mut i, flag)?;
                out.look.edges = Some(WindowEdges::from_word(w).ok_or_else(|| {
                    format!("bad edges '{w}': use none, faint, hairline or solid")
                })?);
            }
            other if other.starts_with("--user-data-dir=") => {
                let path = &other["--user-data-dir=".len()..];
                if path.is_empty() {
                    return Err("--user-data-dir= needs a path".to_string());
                }
                // Absolute once, here: the lock and pipe names hash this text, and the updater
                // restarts the app from another working folder with it.
                out.user_data_dir =
                    Some(std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path)));
            }
            other if !other.starts_with("--") => out.positional.push(other.to_string()),
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }
    let other_mode = out.scene.is_some()
        || out.screenshot.is_some()
        || out.live_screenshot.is_some()
        || out.bench_frames.is_some()
        || out.instance_probe
        || out.live_tabs.is_some();
    // A serve process is a bridge and nothing else: it opens no folder and draws nothing (rule 283).
    if out.serve_remote
        && (!out.positional.is_empty() || other_mode || out.start_hidden || on.is_some())
    {
        return Err(
            "--serve-remote takes no folder and no other mode; only --user-data-dir".to_string(),
        );
    }
    if out.no_start && !out.serve_remote {
        return Err("--no-start goes with --serve-remote only".to_string());
    }
    if out.start_hidden && other_mode {
        return Err("--start-hidden is a launch of the app; it takes no other mode".to_string());
    }
    out.open_on = open_on(on, resume, &mut out.positional)?;
    if out.open_on.is_some() && (other_mode || out.start_hidden) {
        return Err("flare open --on takes no other mode; only --user-data-dir".to_string());
    }
    if out.live_shell.is_some() && out.live_screenshot.is_none() {
        return Err("--live-shell needs --live-screenshot <path>".to_string());
    }
    let settings_flag = out.settings_page.is_some()
        || out.settings_find.is_some()
        || out.settings_open != SettingsOpen::None;
    if settings_flag && out.scene.as_deref() != Some("settings") {
        return Err("the --settings-* flags need --scene settings".to_string());
    }
    out.renderer = match (renderer_flag, env_renderer) {
        (Some(choice), _) => choice,
        (None, Some(env)) => parse_renderer(env)?,
        (None, None) => RendererChoice::Gpu,
    };
    Ok(out)
}

/// `flare open --on <machine> <folder> [--resume <id>]`: with `--on`, the bare words must be `open` and one folder
/// (a path on that machine, never statted here), and they are taken out of `positional` so no launch opens them. Without
/// `--on` a word `open` is a folder like any other, and `--resume` alone is refused.
fn open_on(
    on: Option<String>,
    resume: Option<String>,
    positional: &mut Vec<String>,
) -> Result<Option<OpenOn>, String> {
    let Some(machine) = on else {
        if resume.is_some() {
            return Err("--resume goes with flare open --on <machine> <folder>".to_string());
        }
        return Ok(None);
    };
    const USAGE: &str = "use: flare open --on <machine> <folder> [--resume <session id>]";
    if !crate::machines::argv_word(&machine) {
        return Err(format!(
            "--on needs a machine's id from machines.json, not '{machine}'; {USAGE}"
        ));
    }
    let folder = match positional.as_slice() {
        [verb, folder] if verb == "open" && !folder.trim().is_empty() => folder.clone(),
        _ => return Err(USAGE.to_string()),
    };
    if let Some(r) = &resume
        && flare_agent::resume::valid_resume(Some(r)).is_none()
    {
        return Err(format!("--resume needs a session id, not '{r}'"));
    }
    positional.clear();
    Ok(Some(OpenOn {
        machine,
        folder,
        resume,
    }))
}

/// The value after `flag`; advances `i` past it.
fn value<'a>(args: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, String> {
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// A colour flag's value in the stored form (`#rrggbb` or `#rrggbbaa`).
fn parse_look_colour(flag: &str, v: &str) -> Result<String, String> {
    parse_colour(v)
        .map(|c| to_stored(&c))
        .ok_or_else(|| format!("{flag} needs a colour such as #rrggbb, not '{v}'"))
}

fn parse_renderer(v: &str) -> Result<RendererChoice, String> {
    match v {
        "gpu" => Ok(RendererChoice::Gpu),
        "software" => Ok(RendererChoice::Software),
        other => Err(format!("bad renderer '{other}': use gpu or software")),
    }
}

fn parse_size(v: &str) -> Result<(u32, u32), String> {
    let bad = || format!("bad size '{v}': use WIDTHxHEIGHT, for example 1000x640");
    let (w, h) = v.split_once(['x', 'X']).ok_or_else(bad)?;
    let w: u32 = w.parse().map_err(|_| bad())?;
    let h: u32 = h.parse().map_err(|_| bad())?;
    if w == 0 || h == 0 {
        return Err(bad());
    }
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Rule 300: `--on` and `--resume` values are read as values, never as folders: the positional list is empty, the
    /// profile is untouched, and a launch would open nothing of them.
    #[test]
    fn open_on_reads_its_values_and_leaves_no_folder_to_open() {
        // An absolute path on this system, so it is kept as it is.
        const UDD: &str = if cfg!(windows) { "C:\\p" } else { "/p" };
        let id = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";
        let args = parse_args(
            &a(&[
                "open",
                "--on",
                "mac",
                "/Users/max/proj",
                "--resume",
                id,
                &format!("--user-data-dir={UDD}"),
            ]),
            None,
        )
        .unwrap();
        assert_eq!(
            args.open_on,
            Some(OpenOn {
                machine: "mac".into(),
                folder: "/Users/max/proj".into(),
                resume: Some(id.into()),
            })
        );
        assert!(args.positional.is_empty());
        assert_eq!(args.user_data_dir, Some(PathBuf::from(UDD)));
        // The machine's value is never a folder, even one that exists here: it is a name machines.json is asked for.
        let args = parse_args(&a(&["--on", "C:\\Windows", "open", "C:\\w"]), None).unwrap();
        assert_eq!(args.open_on.unwrap().machine, "C:\\Windows");
        assert!(args.positional.is_empty());
        // Without --on, `open` is a folder like any other.
        let plain = parse_args(&a(&["open"]), None).unwrap();
        assert_eq!(plain.positional, ["open"]);
        assert_eq!(plain.open_on, None);
    }

    #[test]
    fn open_on_refuses_what_it_cannot_use_with_its_usage() {
        for bad in [
            &["--on", "mac"][..],
            &["--on", "mac", "open"],
            &["--on", "mac", "show", "/p"],
            &["--on", "mac", "open", "/p", "/q"],
            &["--on", "-x", "open", "/p"],
            &["--on", "mac", "open", "/p", "--resume", "x; rm"],
            &["--resume", "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f"],
            &["--on", "mac", "open", "/p", "--start-hidden"],
            &["--on", "mac", "open", "/p", "--scene", "strip"],
            &["--on", "mac"],
        ] {
            assert!(parse_args(&a(bad), None).is_err(), "{bad:?}");
        }
        let e = parse_args(&a(&["--on", "mac", "open"]), None).unwrap_err();
        assert!(e.contains("flare open --on"), "{e}");
    }

    #[test]
    fn start_hidden_is_a_launch_and_no_start_goes_with_serve_only() {
        let args = parse_args(&a(&["--start-hidden", "--user-data-dir=C:\\p"]), None).unwrap();
        assert!(args.start_hidden && !args.e2e);
        assert!(args.positional.is_empty());
        assert!(parse_args(&a(&["--start-hidden", "--scene", "strip"]), None).is_err());
        assert!(parse_args(&a(&["--serve-remote", "--start-hidden"]), None).is_err());
        assert!(
            parse_args(&a(&["--serve-remote", "--no-start"]), None)
                .unwrap()
                .no_start
        );
        assert!(parse_args(&a(&["--no-start"]), None).is_err());
    }

    #[test]
    fn serve_remote_is_a_mode_of_its_own() {
        let args = parse_args(&a(&["--serve-remote", "--user-data-dir=C:\\p"]), None).unwrap();
        assert!(args.serve_remote);
        assert!(args.positional.is_empty());
        assert!(!parse_args(&a(&[]), None).unwrap().serve_remote);
        for extra in ["C:\\work", "--instance-probe", "--scene"] {
            let mut v = vec!["--serve-remote", extra];
            if extra == "--scene" {
                v.push("strip");
            }
            assert!(parse_args(&a(&v), None).is_err(), "{extra}");
        }
    }

    #[test]
    fn default_is_gpu() {
        let args = parse_args(&a(&[]), None).unwrap();
        assert_eq!(args.renderer, RendererChoice::Gpu);
        assert_eq!(args.scale, 1.0);
        assert_eq!(args.size, (1000, 640));
        assert_eq!(args.screenshot, None);
        assert_eq!(args.bench_frames, None);
        assert!(!args.probe_backend);
    }

    #[test]
    fn flag_beats_env() {
        let args = parse_args(&a(&["--renderer", "gpu"]), Some("software")).unwrap();
        assert_eq!(args.renderer, RendererChoice::Gpu);
    }

    #[test]
    fn env_used_without_flag() {
        let args = parse_args(&a(&[]), Some("software")).unwrap();
        assert_eq!(args.renderer, RendererChoice::Software);
    }

    #[test]
    fn bad_renderer_is_error() {
        assert!(parse_args(&a(&["--renderer", "vulkan"]), None).is_err());
        assert!(parse_args(&a(&[]), Some("nope")).is_err());
    }

    #[test]
    fn screenshot_path_parsed() {
        let args = parse_args(&a(&["--screenshot", "out/shot.png"]), None).unwrap();
        assert_eq!(args.screenshot, Some(PathBuf::from("out/shot.png")));
    }

    #[test]
    fn size_parses_wxh() {
        let args = parse_args(&a(&["--size", "1280x720"]), None).unwrap();
        assert_eq!(args.size, (1280, 720));
        assert!(parse_args(&a(&["--size", "1280"]), None).is_err());
        assert!(parse_args(&a(&["--size", "0x720"]), None).is_err());
    }

    #[test]
    fn scale_out_of_range_is_error() {
        assert!(parse_args(&a(&["--scale", "0.4"]), None).is_err());
        assert!(parse_args(&a(&["--scale", "4.5"]), None).is_err());
        assert!(parse_args(&a(&["--scale", "nan"]), None).is_err());
        let args = parse_args(&a(&["--scale", "2.25"]), None).unwrap();
        assert_eq!(args.scale, 2.25);
    }

    #[test]
    fn unknown_flag_is_error() {
        assert!(parse_args(&a(&["--frobnicate"]), None).is_err());
    }

    #[test]
    fn flag_missing_value_is_error() {
        assert!(parse_args(&a(&["--screenshot"]), None).is_err());
    }

    #[test]
    fn bench_frames_parsed() {
        let args = parse_args(&a(&["--bench-frames", "300"]), None).unwrap();
        assert_eq!(args.bench_frames, Some(300));
    }

    #[test]
    fn probe_flag_parsed() {
        let args = parse_args(&a(&["--probe-backend"]), None).unwrap();
        assert!(args.probe_backend);
    }

    #[test]
    fn sample_flag_parsed() {
        let args = parse_args(&a(&[]), None).unwrap();
        assert!(!args.sample);
        let args = parse_args(&a(&["--sample"]), None).unwrap();
        assert!(args.sample);
    }

    #[test]
    fn live_screenshot_path_parsed() {
        let args = parse_args(&a(&["--live-screenshot", "out/live.png"]), None).unwrap();
        assert_eq!(args.live_screenshot, Some(PathBuf::from("out/live.png")));
        assert_eq!(args.live_shell, None);
        let args = parse_args(
            &a(&["--live-screenshot", "x.png", "--live-shell", "cmd"]),
            None,
        )
        .unwrap();
        assert_eq!(args.live_shell.as_deref(), Some("cmd"));
    }

    #[test]
    fn a_relative_profile_becomes_absolute_so_every_spelling_shares_one_lock() {
        let rel = parse_args(&a(&["--user-data-dir=dev"]), None).unwrap();
        let dir = rel.user_data_dir.unwrap();
        assert!(dir.is_absolute(), "{dir:?}");
        assert!(dir.ends_with("dev"));
        let abs = std::env::current_dir().unwrap().join("dev");
        let spelled = parse_args(&a(&[&format!("--user-data-dir={}", abs.display())]), None)
            .unwrap()
            .user_data_dir
            .unwrap();
        assert_eq!(dir, spelled);
    }

    #[cfg(windows)]
    #[test]
    fn user_data_dir_equals_form_and_e2e_are_parsed() {
        let args = parse_args(&a(&["--user-data-dir=C:\\p", "--e2e"]), None).unwrap();
        assert_eq!(args.user_data_dir, Some(PathBuf::from("C:\\p")));
        assert!(args.e2e);
        assert!(parse_args(&a(&["--user-data-dir", "C:\\p"]), None).is_err());
        let args = parse_args(&a(&["C:\\x"]), None).unwrap();
        assert_eq!(args.positional, vec!["C:\\x".to_string()]);
        assert!(parse_args(&a(&["--bogus"]), None).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn user_data_dir_equals_form_and_e2e_are_parsed_posix() {
        let args = parse_args(&a(&["--user-data-dir=/p", "--e2e"]), None).unwrap();
        assert_eq!(args.user_data_dir, Some(PathBuf::from("/p")));
        assert!(args.e2e);
        assert!(parse_args(&a(&["--user-data-dir", "/p"]), None).is_err());
        let args = parse_args(&a(&["/x"]), None).unwrap();
        assert_eq!(args.positional, vec!["/x".to_string()]);
        assert!(parse_args(&a(&["--bogus"]), None).is_err());
    }

    #[test]
    fn scene_live_tabs_and_probe_flags() {
        let args = parse_args(
            &a(&["--scene", "strip-3", "--live-tabs", "3", "--instance-probe"]),
            None,
        )
        .unwrap();
        assert_eq!(args.scene.as_deref(), Some("strip-3"));
        assert_eq!(args.live_tabs, Some(3));
        assert!(args.instance_probe);
        assert!(parse_args(&a(&["--live-tabs"]), None).is_err());
        assert!(parse_args(&a(&["--live-tabs", "x"]), None).is_err());
    }

    #[test]
    fn live_shell_needs_live_screenshot() {
        assert!(parse_args(&a(&["--live-shell", "cmd"]), None).is_err());
        assert!(parse_args(&a(&["--live-shell"]), None).is_err());
    }

    #[test]
    fn no_import_parses() {
        assert!(!parse_args(&a(&[]), None).unwrap().no_import);
        assert!(parse_args(&a(&["--no-import"]), None).unwrap().no_import);
        assert!(parse_args(&a(&["--no-import=yes"]), None).is_err());
        assert!(parse_args(&a(&["--no-plugins"]), None).unwrap().no_plugins);
        assert!(!parse_args(&a(&[]), None).unwrap().no_plugins);
        assert_eq!(
            parse_args(&a(&["--after-pid", "4242"]), None)
                .unwrap()
                .after_pid,
            Some(4242)
        );
        assert!(parse_args(&a(&["--after-pid", "x"]), None).is_err());
    }

    #[test]
    fn the_look_flags_parse() {
        let args = parse_args(
            &a(&[
                "--theme",
                "nord",
                "--see-through",
                "40",
                "--font",
                "hack",
                "--font-pct",
                "120",
                "--accent",
                "#e07a2f",
                "--background",
                "#101010",
                "--secondary",
                "#3a3a3a",
                "--edges",
                "solid",
            ]),
            None,
        )
        .unwrap();
        assert_eq!(
            args.look,
            LookOverrides {
                theme: Some("nord".into()),
                see_through: Some(40),
                font: Some("hack".into()),
                font_pct: Some(120),
                accent: Some("#e07a2f".into()),
                background: Some("#101010".into()),
                secondary: Some("#3a3a3a".into()),
                edges: Some(WindowEdges::Solid),
            }
        );
        assert_eq!(
            parse_args(&a(&[]), None).unwrap().look,
            LookOverrides::default()
        );
        assert!(parse_args(&a(&["--see-through", "90"]), None).is_err());
        assert!(parse_args(&a(&["--font-pct", "55"]), None).is_err());
        assert!(parse_args(&a(&["--edges", "dotted"]), None).is_err());
        assert!(parse_args(&a(&["--theme"]), None).is_err());
        assert!(parse_args(&a(&["--accent", "soup"]), None).is_err());
        assert!(parse_args(&a(&["--font", "comic"]), None).is_err());
    }

    #[test]
    fn the_scene_flags_parse_and_a_bad_page_is_an_error() {
        let args = parse_args(
            &a(&[
                "--scene",
                "settings",
                "--settings-page",
                "terminal",
                "--settings-find",
                "colour",
                "--settings-open",
                "wall-expanded",
                "--scale",
                "1.5",
                "--size",
                "800x600",
                "--screenshot",
                "x.png",
            ]),
            None,
        )
        .unwrap();
        assert_eq!(args.settings_page.as_deref(), Some("terminal"));
        assert_eq!(args.settings_find.as_deref(), Some("colour"));
        assert_eq!(args.settings_open, SettingsOpen::WallExpanded);
        assert_eq!(args.size, (800, 600));
        let plain = parse_args(&a(&["--scene", "settings"]), None).unwrap();
        assert_eq!(plain.settings_page, None);
        assert_eq!(plain.settings_open, SettingsOpen::None);
        for word in SETTINGS_OPEN_WORDS {
            let args =
                parse_args(&a(&["--scene", "settings", "--settings-open", word]), None).unwrap();
            assert_eq!(Some(args.settings_open), SettingsOpen::from_word(word));
        }
        let bad = |extra: &[&str]| {
            let mut v = vec!["--scene", "settings"];
            v.extend_from_slice(extra);
            parse_args(&a(&v), None)
        };
        let err = bad(&["--settings-page", "colours"]).unwrap_err();
        assert!(
            err.contains("appearance"),
            "the error names the pages: {err}"
        );
        assert!(bad(&["--settings-open", "menu"]).is_err());
        assert!(bad(&["--settings-page"]).is_err());
        assert!(bad(&["--settings-find"]).is_err());
        assert!(
            parse_args(&a(&["--settings-page", "about"]), None).is_err(),
            "the flags need the settings scene"
        );
        assert!(parse_args(&a(&["--scene", "tip", "--settings-open", "ask"]), None).is_err());
    }

    #[test]
    fn preview_flag_is_parsed() {
        let args = parse_args(&["--preview-update".to_string()], None).unwrap();
        assert!(args.preview_update);
        assert!(!parse_args(&[], None).unwrap().preview_update);
    }

    #[test]
    fn a_near_miss_is_an_error() {
        for bad in ["--preview-updates", "--preview_update", "--Preview-Update"] {
            assert!(parse_args(&[bad.to_string()], None).is_err(), "{bad}");
        }
    }
}
