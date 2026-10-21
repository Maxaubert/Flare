//! Picks Slint's renderer at start: the GPU renderer (femtovg-wgpu) with a software fallback.

use crate::config::RendererChoice;

#[derive(Clone, Debug, PartialEq)]
pub struct BackendOutcome {
    pub effective: RendererChoice,
    pub fallback_reason: Option<String>,
}

/// True only for the value "1" of FLARE_FORCE_GPU_FAIL.
pub fn gpu_forced_to_fail(v: Option<&str>) -> bool {
    v == Some("1")
}

/// Selects Slint's backend. A failed GPU start falls back to software and says why.
/// `e2e` makes the OS window start inactive and off the taskbar (`--e2e`).
pub fn select_backend(
    choice: RendererChoice,
    e2e: bool,
) -> Result<BackendOutcome, slint::PlatformError> {
    match choice {
        RendererChoice::Software => {
            select_software(e2e)?;
            Ok(BackendOutcome {
                effective: RendererChoice::Software,
                fallback_reason: None,
            })
        }
        RendererChoice::Gpu => match try_gpu(e2e) {
            Ok(()) => Ok(BackendOutcome {
                effective: RendererChoice::Gpu,
                fallback_reason: None,
            }),
            Err(e) => {
                let reason = e.to_string();
                log::warn!("GPU renderer unavailable: {reason}; using software");
                select_software(e2e)?;
                Ok(BackendOutcome {
                    effective: RendererChoice::Software,
                    fallback_reason: Some(reason),
                })
            }
        },
    }
}

/// Slint's femtovg renderer on wgpu 30, with the device's limits raised to wgpu's defaults
/// (8192 px textures, so a 4K window at 225% fits).
fn try_gpu(e2e: bool) -> Result<(), slint::PlatformError> {
    if gpu_forced_to_fail(std::env::var("FLARE_FORCE_GPU_FAIL").ok().as_deref()) {
        return Err(slint::PlatformError::Other(
            "forced by FLARE_FORCE_GPU_FAIL".into(),
        ));
    }
    with_e2e_attributes(slint::BackendSelector::new(), e2e)
        .backend_name("winit".into())
        .renderer_name("femtovg-wgpu".into())
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Automatic(gpu_settings()))
        .select()
}

/// Slint's wgpu settings with `device_required_limits` raised from its WebGL2 default (2048 px
/// textures) to wgpu's defaults (8192 px). Regression rule 130.
fn gpu_settings() -> slint::wgpu_30::WGPUSettings {
    let mut settings = slint::wgpu_30::WGPUSettings::default();
    settings.device_required_limits = slint::wgpu_30::wgpu::Limits::default();
    settings
}

/// Under `--e2e` the OS window is created inactive and off the taskbar, so a test run never takes
/// the foreground. The winit backend creates the window at `show()`, so no handle exists earlier
/// and the attributes hook is the only place to ask.
fn with_e2e_attributes(selector: slint::BackendSelector, e2e: bool) -> slint::BackendSelector {
    if !e2e {
        return selector;
    }
    selector.with_winit_window_attributes_hook(|attributes| {
        use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
        attributes.with_active(false).with_skip_taskbar(true)
    })
}

/// Slint's software renderer. Must not be combined with `require_wgpu_30`.
fn select_software(e2e: bool) -> Result<(), slint::PlatformError> {
    with_e2e_attributes(slint::BackendSelector::new(), e2e)
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_fail_only_for_one() {
        assert!(gpu_forced_to_fail(Some("1")));
        assert!(!gpu_forced_to_fail(None));
        assert!(!gpu_forced_to_fail(Some("0")));
        assert!(!gpu_forced_to_fail(Some("")));
    }

    #[test]
    fn gpu_limits_fit_a_4k_window_at_225_percent() {
        // Slint's own default is the WebGL2 set; the grid texture of a 4K window needs 3840 px and
        // more, so the raised limit must be at least 8192.
        let default_max = slint::wgpu_30::WGPUSettings::default()
            .device_required_limits
            .max_texture_dimension_2d;
        let ours = gpu_settings()
            .device_required_limits
            .max_texture_dimension_2d;
        assert!(default_max < 3840, "Slint's default changed: {default_max}");
        assert!(ours >= 8192, "raised limit is {ours}");
    }
}
