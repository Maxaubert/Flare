//! Picks Slint's renderer at start: the GPU renderer (femtovg-wgpu) with a software fallback.
//! On Windows 11 22H2 and later with the GPU renderer the window is created transparent on a
//! DirectComposition swapchain, so the See-through slider works without a restart (decision D8).

use crate::config::RendererChoice;
use flare_win::material::system_backdrop_ok;
use flare_win::os_build::{OsVersion, os_version};

#[derive(Clone, Debug, PartialEq)]
pub struct BackendOutcome {
    pub effective: RendererChoice,
    pub fallback_reason: Option<String>,
    /// The window was created transparent on the GPU renderer, so the See-through slider can work.
    pub glass_possible: bool,
}

/// What the window is created with so that glass can ever be used. A transparent window cannot be
/// made later, and only the DirectComposition swapchain offers a non-opaque alpha mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlassPlan {
    pub transparent: bool,
    pub visual_swapchain: bool,
}

impl GlassPlan {
    pub const NONE: GlassPlan = GlassPlan {
        transparent: false,
        visual_swapchain: false,
    };
}

/// Both true iff the GPU renderer is chosen, the build has the backdrop call and glass is not off.
pub fn glass_plan(choice: RendererChoice, v: OsVersion, disabled: bool) -> GlassPlan {
    let on = choice == RendererChoice::Gpu && system_backdrop_ok(v) && !disabled;
    GlassPlan {
        transparent: on,
        visual_swapchain: on,
    }
}

/// True only for the value "1" of FLARE_NO_GLASS.
pub fn glass_disabled(env: Option<&str>) -> bool {
    env == Some("1")
}

/// True only for the value "1" of FLARE_FORCE_GPU_FAIL.
pub fn gpu_forced_to_fail(v: Option<&str>) -> bool {
    v == Some("1")
}

fn outcome(
    effective: RendererChoice,
    plan: GlassPlan,
    fallback_reason: Option<String>,
) -> BackendOutcome {
    BackendOutcome {
        effective,
        fallback_reason,
        glass_possible: effective == RendererChoice::Gpu && plan.transparent,
    }
}

/// Selects Slint's backend. A failed GPU start falls back to software and says why.
/// `e2e` makes the OS window start inactive and off the taskbar (`--e2e`).
pub fn select_backend(
    choice: RendererChoice,
    e2e: bool,
) -> Result<BackendOutcome, slint::PlatformError> {
    select_backend_with(choice, e2e, None)
}

/// `select_backend` with the GPU device `prewarm_if_gpu` started at the top of `main` (if any).
pub fn select_backend_with(
    choice: RendererChoice,
    e2e: bool,
    prewarm: Option<Prewarm>,
) -> Result<BackendOutcome, slint::PlatformError> {
    let plan = glass_plan(
        choice,
        os_version(),
        glass_disabled(std::env::var("FLARE_NO_GLASS").ok().as_deref()),
    );
    match choice {
        RendererChoice::Software => {
            select_software(e2e, GlassPlan::NONE)?;
            Ok(outcome(RendererChoice::Software, GlassPlan::NONE, None))
        }
        RendererChoice::Gpu => match try_gpu(e2e, plan, prewarm) {
            Ok(()) => Ok(outcome(RendererChoice::Gpu, plan, None)),
            Err(e) => {
                let reason = e.to_string();
                log::warn!("GPU renderer unavailable: {reason}; using software");
                select_software(e2e, GlassPlan::NONE)?;
                Ok(outcome(
                    RendererChoice::Software,
                    GlassPlan::NONE,
                    Some(reason),
                ))
            }
        },
    }
}

/// Slint's femtovg renderer on wgpu 30, with the device's limits raised to wgpu's defaults
/// (8192 px textures, so a 4K window at 225% fits). With a prewarm the instance, adapter and
/// device were built on another thread while the app started and Slint is handed them.
fn try_gpu(
    e2e: bool,
    plan: GlassPlan,
    prewarm: Option<Prewarm>,
) -> Result<(), slint::PlatformError> {
    if gpu_forced_to_fail(std::env::var("FLARE_FORCE_GPU_FAIL").ok().as_deref()) {
        return Err(slint::PlatformError::Other(
            "forced by FLARE_FORCE_GPU_FAIL".into(),
        ));
    }
    let configuration = gpu_configuration(prewarm, plan)?;
    with_window_attributes(slint::BackendSelector::new(), e2e, plan)
        .backend_name("winit".into())
        .renderer_name("femtovg-wgpu".into())
        .require_wgpu_30(configuration)
        .select()
}

/// The wgpu configuration for Slint: the prewarmed device when there is one (a failed prewarm is
/// the software fallback with its own reason), otherwise Slint's own creation.
fn gpu_configuration(
    prewarm: Option<Prewarm>,
    plan: GlassPlan,
) -> Result<slint::wgpu_30::WGPUConfiguration, slint::PlatformError> {
    match prewarm {
        Some(p) => {
            let Prepared {
                instance,
                adapter,
                device,
                queue,
            } = take(p).map_err(slint::PlatformError::Other)?;
            Ok(slint::wgpu_30::WGPUConfiguration::Manual {
                instance,
                adapter,
                device,
                queue,
            })
        }
        None => Ok(slint::wgpu_30::WGPUConfiguration::Automatic(gpu_settings(
            plan,
        ))),
    }
}

/// The DX12 options of the instance. With a glass plan the presentation system is DirectComposition.
/// Slint's `Automatic` path and the prewarm both build their instance from this.
pub fn instance_options(plan: GlassPlan) -> slint::wgpu_30::wgpu::BackendOptions {
    let mut options = slint::wgpu_30::wgpu::BackendOptions::from_env_or_default();
    if plan.visual_swapchain {
        options.dx12.presentation_system = slint::wgpu_30::wgpu::Dx12SwapchainKind::DxgiFromVisual;
    }
    options
}

/// Slint's wgpu settings with `device_required_limits` raised from its WebGL2 default (2048 px
/// textures) to wgpu's defaults (8192 px). Regression rule 130.
fn gpu_settings(plan: GlassPlan) -> slint::wgpu_30::WGPUSettings {
    let mut settings = slint::wgpu_30::WGPUSettings::default();
    settings.device_required_limits = slint::wgpu_30::wgpu::Limits::default();
    settings.backend_options = instance_options(plan);
    settings
}

/// The GPU objects built on the prewarm thread, ready for Slint's `Manual` configuration.
pub struct Prepared {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// A GPU device being built on a worker thread while the app parses its arguments and starts.
pub struct Prewarm {
    handle: std::thread::JoinHandle<Result<Prepared, String>>,
}

impl Prewarm {
    fn spawn(build: impl FnOnce() -> Result<Prepared, String> + Send + 'static) -> Option<Prewarm> {
        std::thread::Builder::new()
            .name("flare-gpu-prewarm".into())
            .spawn(build)
            .ok()
            .map(|handle| Prewarm { handle })
    }
}

/// The glass plan of a run that will open a window on the GPU renderer, or None when no prewarm is
/// wanted: software (flag or env), a forced GPU failure, or an argument list that never reaches the
/// window (a bad flag, or a mode that makes its own device or none).
fn prewarm_plan(
    argv: &[String],
    env_renderer: Option<&str>,
    force_fail: Option<&str>,
    no_glass: Option<&str>,
    os: OsVersion,
) -> Option<GlassPlan> {
    if gpu_forced_to_fail(force_fail) {
        return None;
    }
    let args = crate::config::parse_args(argv, env_renderer).ok()?;
    let opens_window = !(args.probe_backend
        || args.screenshot.is_some()
        || args.bench_frames.is_some()
        || args.live_screenshot.is_some()
        || args.scene.is_some()
        || args.instance_probe
        || args.live_tabs.is_some());
    if args.renderer != RendererChoice::Gpu || !opens_window {
        return None;
    }
    Some(glass_plan(args.renderer, os, glass_disabled(no_glass)))
}

/// Starts building the GPU device on a worker thread, straight after `ship_ctl::bootstrap()` and
/// before the arguments are parsed. None for the software renderer.
pub fn prewarm_if_gpu(argv: &[String], env_renderer: Option<&str>) -> Option<Prewarm> {
    let plan = prewarm_plan(
        argv,
        env_renderer,
        std::env::var("FLARE_FORCE_GPU_FAIL").ok().as_deref(),
        std::env::var("FLARE_NO_GLASS").ok().as_deref(),
        os_version(),
    )?;
    Prewarm::spawn(move || build_prepared(plan))
}

/// Waits for the prewarm thread. A failure carries its reason (the software fallback reports it).
pub fn take(p: Prewarm) -> Result<Prepared, String> {
    p.handle
        .join() // sync-ok: start-up, the window needs the device and the thread is nearly done by now
        .unwrap_or_else(|_| Err("the GPU prewarm thread panicked".into()))
}

/// The backends the prewarm instance starts. Without `WGPU_BACKEND` that is DX12 alone: the glass
/// swapchain (`DxgiFromVisual`) is DX12-only, and an instance that also starts Vulkan loads the
/// Vulkan and OpenGL driver stacks of every GPU in the PC (about 32 MB of private memory, M10 Task
/// 66). A named choice is kept, without OpenGL as Slint's own `Automatic` path leaves it out
/// (`mask_backends`); a choice that would leave nothing is kept whole.
fn prewarm_backends(named: Option<wgpu::Backends>) -> wgpu::Backends {
    match named {
        None => wgpu::Backends::DX12,
        Some(b) => {
            let masked = b & !wgpu::Backends::GL;
            if masked.is_empty() { b } else { masked }
        }
    }
}

/// Builds the instance, the high-performance adapter and the device the way Slint's `Automatic`
/// path does, except that no window surface exists yet to ask the adapter about.
fn build_prepared(plan: GlassPlan) -> Result<Prepared, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: prewarm_backends(wgpu::Backends::from_env()),
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        backend_options: instance_options(plan),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        display: None,
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .map_err(|e| format!("Failed to find an appropriate adapter: {e}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("flare"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        trace: wgpu::Trace::default(),
    }))
    .map_err(|e| format!("Failed to create device: {e}"))?;
    Ok(Prepared {
        instance,
        adapter,
        device,
        queue,
    })
}
/// ONE attributes hook, installed whenever the window must start transparent or under `--e2e`
/// (where it is created inactive and off the taskbar, so a test run never takes the foreground).
/// The winit backend creates the window at `show()`, so the hook is the only place to ask.
fn with_window_attributes(
    selector: slint::BackendSelector,
    e2e: bool,
    plan: GlassPlan,
) -> slint::BackendSelector {
    // The dictation widget (M8 Task 26) is a second window made by the same backend: it asks whether its surface has alpha
    // and, while it is being made, gets its own attributes from this one hook.
    crate::dictation_widget::set_alpha_surface(plan.transparent);
    let transparent = plan.transparent;
    selector.with_winit_window_attributes_hook(move |attributes| {
        if crate::dictation_widget::creating() {
            return crate::dictation_widget::attributes(attributes);
        }
        if !e2e && !transparent {
            return attributes;
        }
        let attributes = attributes.with_transparent(transparent);
        if e2e {
            e2e_attributes(attributes)
        } else {
            attributes
        }
    })
}

/// The --e2e window is BORN inactive, off the taskbar and parked off every monitor, so it never
/// shows at its normal place before the hook parks it.
pub fn e2e_attributes(
    attributes: slint::winit_030::winit::window::WindowAttributes,
) -> slint::winit_030::winit::window::WindowAttributes {
    use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
    let (x, y) = flare_win::window_park::parked_origin();
    attributes
        .with_active(false)
        .with_skip_taskbar(true)
        .with_position(slint::winit_030::winit::dpi::PhysicalPosition::new(x, y))
}

/// Slint's software renderer. Must not be combined with `require_wgpu_30`.
fn select_software(e2e: bool, plan: GlassPlan) -> Result<(), slint::PlatformError> {
    with_window_attributes(slint::BackendSelector::new(), e2e, plan)
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_e2e_window_is_created_at_the_parking_spot() {
        use slint::winit_030::winit::dpi::Position;
        let a = e2e_attributes(slint::winit_030::winit::window::WindowAttributes::default());
        let Some(Position::Physical(p)) = a.position else {
            panic!("no physical position: {:?}", a.position);
        };
        assert_eq!((p.x, p.y), (-32000, -32000));
        assert!(!a.active);
    }

    fn v(build: u32) -> OsVersion {
        OsVersion {
            major: 10,
            minor: 0,
            build,
        }
    }

    #[test]
    fn force_fail_only_for_one() {
        assert!(gpu_forced_to_fail(Some("1")));
        assert!(!gpu_forced_to_fail(None));
        assert!(!gpu_forced_to_fail(Some("0")));
        assert!(!gpu_forced_to_fail(Some("")));
    }

    #[test]
    fn glass_plan_needs_the_gpu_and_the_22h2_backdrop() {
        let on = GlassPlan {
            transparent: true,
            visual_swapchain: true,
        };
        assert_eq!(glass_plan(RendererChoice::Gpu, v(22621), false), on);
        assert_eq!(
            glass_plan(RendererChoice::Gpu, v(22620), false),
            GlassPlan::NONE
        );
        assert_eq!(
            glass_plan(RendererChoice::Gpu, v(19045), false),
            GlassPlan::NONE
        );
        assert_eq!(
            glass_plan(RendererChoice::Software, v(26300), false),
            GlassPlan::NONE
        );
        assert_eq!(
            glass_plan(RendererChoice::Gpu, v(26300), true),
            GlassPlan::NONE
        );
    }

    #[test]
    fn glass_disabled_only_for_one() {
        assert!(glass_disabled(Some("1")));
        assert!(!glass_disabled(None));
        assert!(!glass_disabled(Some("0")));
    }

    #[test]
    fn the_outcome_says_glass_is_possible_only_on_the_gpu_with_a_transparent_window() {
        let on = glass_plan(RendererChoice::Gpu, v(26300), false);
        assert!(outcome(RendererChoice::Gpu, on, None).glass_possible);
        // A forced GPU failure builds the software selection with the empty plan.
        let o = outcome(RendererChoice::Software, GlassPlan::NONE, Some("x".into()));
        assert!(!o.glass_possible);
        // Even a transparent plan is no glass when the effective renderer is software.
        assert!(!outcome(RendererChoice::Software, on, None).glass_possible);
    }

    #[test]
    fn the_visual_swapchain_is_asked_for_only_with_a_plan() {
        use slint::wgpu_30::wgpu::Dx12SwapchainKind;
        let on = glass_plan(RendererChoice::Gpu, v(26300), false);
        assert!(matches!(
            gpu_settings(on).backend_options.dx12.presentation_system,
            Dx12SwapchainKind::DxgiFromVisual
        ));
        assert!(!matches!(
            gpu_settings(GlassPlan::NONE)
                .backend_options
                .dx12
                .presentation_system,
            Dx12SwapchainKind::DxgiFromVisual
        ));
    }

    #[test]
    fn gpu_limits_fit_a_4k_window_at_225_percent() {
        // Slint's own default is the WebGL2 set; the grid texture of a 4K window needs 3840 px and
        // more, so the raised limit must be at least 8192.
        let default_max = slint::wgpu_30::WGPUSettings::default()
            .device_required_limits
            .max_texture_dimension_2d;
        let ours = gpu_settings(GlassPlan::NONE)
            .device_required_limits
            .max_texture_dimension_2d;
        assert!(default_max < 3840, "Slint's default changed: {default_max}");
        assert!(ours >= 8192, "raised limit is {ours}");
    }

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn software_choice_never_starts_a_prewarm() {
        let os = v(26300);
        assert!(prewarm_plan(&argv(&["--renderer", "software"]), None, None, None, os).is_none());
        assert!(prewarm_plan(&argv(&[]), Some("software"), None, None, os).is_none());
        assert!(prewarm_plan(&argv(&[]), None, None, None, os).is_some());
        assert!(prewarm_plan(&argv(&["--renderer", "gpu"]), None, None, None, os).is_some());
        // A forced failure, a bad flag and the modes that make their own device get none.
        assert!(prewarm_plan(&argv(&[]), None, Some("1"), None, os).is_none());
        assert!(prewarm_plan(&argv(&["--renderer", "vulkan"]), None, None, None, os).is_none());
        assert!(prewarm_plan(&argv(&["--probe-backend"]), None, None, None, os).is_none());
        assert!(prewarm_plan(&argv(&["--screenshot", "a.png"]), None, None, None, os).is_none());
    }

    #[test]
    fn the_prewarm_instance_options_match_the_glass_plan() {
        use slint::wgpu_30::wgpu::Dx12SwapchainKind;
        let on = glass_plan(RendererChoice::Gpu, v(26300), false);
        let plan = prewarm_plan(&argv(&[]), None, None, None, v(26300)).unwrap();
        assert_eq!(plan, on);
        assert!(matches!(
            instance_options(plan).dx12.presentation_system,
            Dx12SwapchainKind::DxgiFromVisual
        ));
        let no_glass = prewarm_plan(&argv(&[]), None, None, Some("1"), v(26300)).unwrap();
        assert_eq!(no_glass, GlassPlan::NONE);
        assert!(!matches!(
            instance_options(no_glass).dx12.presentation_system,
            Dx12SwapchainKind::DxgiFromVisual
        ));
    }

    #[test]
    fn a_failed_prewarm_is_the_software_fallback_with_its_reason() {
        let p = Prewarm::spawn(|| Err("no adapter on the fake".to_string())).unwrap();
        let e = gpu_configuration(Some(p), GlassPlan::NONE).unwrap_err();
        assert!(e.to_string().contains("no adapter on the fake"), "{e}");
        let panicking = Prewarm::spawn(|| panic!("boom")).unwrap();
        let e = gpu_configuration(Some(panicking), GlassPlan::NONE).unwrap_err();
        assert!(e.to_string().contains("panicked"), "{e}");
        // No prewarm: Slint creates its own.
        assert!(matches!(
            gpu_configuration(None, GlassPlan::NONE),
            Ok(slint::wgpu_30::WGPUConfiguration::Automatic(_))
        ));
    }

    #[test]
    fn the_prewarm_instance_starts_dx12_alone_unless_a_backend_is_named() {
        // Slint's own device path avoids GL, and the glass swapchain is DX12-only: an instance
        // that also starts Vulkan and OpenGL loads every GPU driver stack for nothing.
        assert_eq!(prewarm_backends(None), wgpu::Backends::DX12);
        // A named choice is kept, minus OpenGL; an OpenGL-only choice is kept whole.
        let named = wgpu::Backends::VULKAN | wgpu::Backends::GL;
        assert_eq!(prewarm_backends(Some(named)), wgpu::Backends::VULKAN);
        assert_eq!(
            prewarm_backends(Some(wgpu::Backends::GL)),
            wgpu::Backends::GL
        );
    }
    #[test]
    fn the_device_asks_for_the_memory_usage_hint() {
        // The prewarmed device uses the same hint as Slint's own (`device_memory_hints` defaults to
        // MemoryUsage): smaller heaps over speed, which a terminal never needs.
        let src = include_str!("backend.rs");
        let from = src.find("fn build_prepared").expect("build_prepared");
        let body = &src[from..];
        assert!(body.contains("memory_hints: wgpu::MemoryHints::MemoryUsage"));
    }
    #[test]
    fn the_prewarmed_device_has_the_raised_limits() {
        match build_prepared(GlassPlan::NONE) {
            Ok(p) => assert!(
                p.device.limits().max_texture_dimension_2d >= 8192,
                "{}",
                p.device.limits().max_texture_dimension_2d
            ),
            Err(e) => assert!(e.contains("adapter"), "unexpected failure: {e}"),
        }
    }

    #[test]
    fn main_starts_the_prewarm_before_parsing_arguments() {
        let src = include_str!("main.rs");
        let at = |needle: &str| {
            src.find(needle)
                .unwrap_or_else(|| panic!("main.rs lacks {needle}"))
        };
        let boot = at("ship_ctl::bootstrap();");
        let warm = at("backend::prewarm_if_gpu(");
        let parse = at("config::parse_args(");
        assert!(boot < warm && warm < parse, "order: {boot} {warm} {parse}");
    }
}
