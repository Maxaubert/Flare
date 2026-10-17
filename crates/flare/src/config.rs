//! Command-line options and the renderer choice (GPU or software).

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererChoice {
    Gpu,
    Software,
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
    /// `--scene <name>`: a headless scene for the screenshots.
    pub scene: Option<String>,
    /// `--live-tabs <n>`: how many live tabs a scene shows.
    pub live_tabs: Option<u32>,
    /// `--instance-probe`: answers whether this process was the first instance.
    pub instance_probe: bool,
    /// Bare arguments (folders or files), in order.
    pub positional: Vec<String>,
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
        scene: None,
        live_tabs: None,
        instance_probe: false,
        positional: Vec::new(),
    };
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
            "--instance-probe" => out.instance_probe = true,
            "--scene" => out.scene = Some(value(args, &mut i, flag)?.to_string()),
            "--live-tabs" => {
                let n: u32 = value(args, &mut i, flag)?
                    .parse()
                    .map_err(|_| "--live-tabs needs a whole number".to_string())?;
                out.live_tabs = Some(n);
            }
            other if other.starts_with("--user-data-dir=") => {
                let path = &other["--user-data-dir=".len()..];
                if path.is_empty() {
                    return Err("--user-data-dir= needs a path".to_string());
                }
                out.user_data_dir = Some(PathBuf::from(path));
            }
            other if !other.starts_with("--") => out.positional.push(other.to_string()),
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }
    if out.live_shell.is_some() && out.live_screenshot.is_none() {
        return Err("--live-shell needs --live-screenshot <path>".to_string());
    }
    out.renderer = match (renderer_flag, env_renderer) {
        (Some(choice), _) => choice,
        (None, Some(env)) => parse_renderer(env)?,
        (None, None) => RendererChoice::Gpu,
    };
    Ok(out)
}

/// The value after `flag`; advances `i` past it.
fn value<'a>(args: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, String> {
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} needs a value"))
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
    fn user_data_dir_equals_form_and_e2e_are_parsed() {
        let args = parse_args(&a(&["--user-data-dir=C:\\p", "--e2e"]), None).unwrap();
        assert_eq!(args.user_data_dir, Some(PathBuf::from("C:\\p")));
        assert!(args.e2e);
        assert!(parse_args(&a(&["--user-data-dir", "C:\\p"]), None).is_err());
        let args = parse_args(&a(&["C:\\x"]), None).unwrap();
        assert_eq!(args.positional, vec!["C:\\x".to_string()]);
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
}
