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
        RendererChoice::Gpu => match try_gpu(e2e, plan) {
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
/// (8192 px textures, so a 4K window at 225% fits).
fn try_gpu(e2e: bool, plan: GlassPlan) -> Result<(), slint::PlatformError> {
    if gpu_forced_to_fail(std::env::var("FLARE_FORCE_GPU_FAIL").ok().as_deref()) {
        return Err(slint::PlatformError::Other(
            "forced by FLARE_FORCE_GPU_FAIL".into(),
        ));
    }
    with_window_attributes(slint::BackendSelector::new(), e2e, plan)
        .backend_name("winit".into())
        .renderer_name("femtovg-wgpu".into())
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Automatic(gpu_settings(
            plan,
        )))
        .select()
}

/// Slint's wgpu settings with `device_required_limits` raised from its WebGL2 default (2048 px
/// textures) to wgpu's defaults (8192 px). Regression rule 130. With a glass plan the DX12
/// presentation system is DirectComposition.
fn gpu_settings(plan: GlassPlan) -> slint::wgpu_30::WGPUSettings {
    let mut settings = slint::wgpu_30::WGPUSettings::default();
    settings.device_required_limits = slint::wgpu_30::wgpu::Limits::default();
    if plan.visual_swapchain {
        settings.backend_options.dx12.presentation_system =
            slint::wgpu_30::wgpu::Dx12SwapchainKind::DxgiFromVisual;
    }
    settings
}

/// ONE attributes hook, installed whenever the window must start transparent or under `--e2e`
/// (where it is created inactive and off the taskbar, so a test run never takes the foreground).
/// The winit backend creates the window at `show()`, so the hook is the only place to ask.
fn with_window_attributes(
    selector: slint::BackendSelector,
    e2e: bool,
    plan: GlassPlan,
) -> slint::BackendSelector {
    if !e2e && !plan.transparent {
        return selector;
    }
    let transparent = plan.transparent;
    selector.with_winit_window_attributes_hook(move |attributes| {
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
}
