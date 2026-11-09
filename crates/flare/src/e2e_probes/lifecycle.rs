//! Lifecycle probes (Task 16 and 20 fill them; Task 1 holds the empty register), and the
//! window-safety probe the runner asserts at every launch (M10 defect D1).

use flare_win::window_park::{ParkReport, report};
use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("window.park", window_park);
}

/// The park report as the probe answers it: the rectangle, the style bits and every problem.
/// `problems` is empty exactly when the window is off every monitor, styled no-activate tool
/// window, off the taskbar and not in the foreground.
pub fn park_json(r: &ParkReport) -> Value {
    json!({
        "rect": [r.rect.0, r.rect.1, r.rect.2, r.rect.3],
        "screen": [r.screen.0, r.screen.1, r.screen.2, r.screen.3],
        "ex_style": r.ex_style,
        "on_screen": r.on_screen(),
        "foreground": r.foreground,
        "problems": r.problems(),
    })
}

fn window_park(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let hwnd = crate::window::native_view_of(ctx.app).ok_or("the window has no handle yet")?;
    Ok(park_json(&report(hwnd)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn the_probe_lists_every_problem_and_none_for_a_safe_window() {
        let safe = ParkReport {
            rect: (-32000, -32000, -31000, -31500),
            ex_style: 0x0800_0080, // WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW
            screen: (0, 0, 1920, 1080),
            foreground: false,
        };
        let v = park_json(&safe);
        assert_eq!(v["problems"].as_array().map(Vec::len), Some(0));
        assert_eq!(v["on_screen"], false);

        let bad = ParkReport {
            rect: (10, 10, 900, 600),
            ex_style: 0,
            ..safe
        };
        let v = park_json(&bad);
        assert_eq!(v["on_screen"], true);
        assert_eq!(v["problems"].as_array().map(Vec::len), Some(3));
    }

    // Rule 340: a Mac window on a screen is safe only clear and click-through, in an accessory app.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_probe_takes_a_clear_window_on_a_screen_and_nothing_else() {
        let safe = ParkReport {
            rect: (0, 25, 1000, 665),
            ex_style: flare_win::window_park::POLICY_ACCESSORY,
            screen: (0, 0, 1024, 768),
            foreground: false,
            clear: true,
        };
        let v = park_json(&safe);
        assert_eq!(v["problems"].as_array().map(Vec::len), Some(0));
        let bad = ParkReport {
            clear: false,
            ex_style: 0,
            foreground: true,
            ..safe
        };
        assert_eq!(
            park_json(&bad)["problems"].as_array().map(Vec::len),
            Some(3)
        );
    }

    #[test]
    fn window_park_is_registered() {
        let mut r = ProbeRegistry::new();
        register(&mut r);
        assert_eq!(r.names(), vec!["window.park"]);
    }
}
