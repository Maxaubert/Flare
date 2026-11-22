//! `device.*` on the call's thread (device panel PR 5, issue #248): the params are checked here before the window
//! hears of the call, an install's path is resolved (its symlinks too) and held to the tab's folder or Xcode's
//! DerivedData (rule 1322), and a tab a remote viewer follows here is answered `unavailable` with nothing posted
//! (rule 1323). What only the window knows (which device the tab's panel shows, who booted it, the person's takeover)
//! is decided by the device controller (`device_agent.rs`).

use std::path::{Path, PathBuf};

use flare_device::job::{At, DeviceOp, Direction, Fail, Press, SwipeTo};
use flare_device::source::Button;
use flare_device::target::Target;

use super::*;
use crate::device_ctl::{AgentReq, REMOTE_WORDS};

/// How long `device.boot` waits for its device before it answers `busy` (the boot carries on): under the channel's
/// `DEVICE_BOOT_TIMEOUT` (130 s), so the answer is Flare's.
pub const DEVICE_BOOT_WAIT: Duration = Duration::from_secs(120);
/// How long every other device call waits for the window: under the channel's 60 s. A touch may wait 20 s for a cold
/// input connection; the first UI tree after a boot takes about 3 s.
pub const DEVICE_WAIT: Duration = Duration::from_secs(50);
/// The most text `device.type` takes, in characters.
pub const TYPE_MAX: usize = 2_000;
/// The longest device name `device.boot` takes.
const NAME_MAX: usize = 100;
/// The longest bundle id `device.launch` takes.
const BUNDLE_MAX: usize = 255;
/// The longest path `device.install` takes, in bytes.
const PATH_MAX: usize = 4096;

fn bad(m: impl Into<String>) -> CtlError {
    CtlError::new(code::BAD_PARAMS, m)
}

/// A device ref of the last look: `d` and up to four digits.
fn device_ref(params: &Value, key: &str) -> Result<Option<String>, CtlError> {
    match str_param(params, key)? {
        None => Ok(None),
        Some(r)
            if r.len() >= 2
                && r.len() <= 5
                && r.starts_with('d')
                && r[1..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            Ok(Some(r.to_string()))
        }
        Some(_) => Err(bad(format!(
            "{key} is an element of the last device_look, such as d3"
        ))),
    }
}

/// A point `{x, y}` in device points.
fn point(params: &Value, key: &str) -> Result<Option<(f64, f64)>, CtlError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(p @ Value::Object(m)) => {
            only_fields(p, &["x", "y"], key)?;
            let n = |k: &str| m.get(k).and_then(Value::as_f64).filter(|v| v.is_finite());
            match (n("x"), n("y")) {
                (Some(x), Some(y)) => Ok(Some((x, y))),
                _ => Err(bad(format!("{key} is {{x, y}} in device points"))),
            }
        }
        Some(_) => Err(bad(format!("{key} is {{x, y}} in device points"))),
    }
}

/// A whole number of milliseconds, if given.
fn ms(params: &Value, key: &str) -> Result<Option<u64>, CtlError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| bad(format!("{key} is a whole number of milliseconds"))),
    }
}

fn bool_param(params: &Value, key: &str) -> Result<Option<bool>, CtlError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(format!("{key} is true or false"))),
    }
}

/// Where a tap or a swipe's start lands: a ref, or both x and y (never both kinds, never half a point).
fn at(params: &Value) -> Result<Option<At>, CtlError> {
    let r = device_ref(params, "ref")?;
    let (x, y) = (num_param(params, "x")?, num_param(params, "y")?);
    match (r, x, y) {
        (Some(r), None, None) => Ok(Some(At::Ref(r))),
        (None, Some(x), Some(y)) => Ok(Some(At::Point(x, y))),
        (None, None, None) => Ok(None),
        _ => Err(bad(
            "give either ref (from device_look) or both x and y, not both",
        )),
    }
}

/// A bundle id: letters, digits, dots, dashes and underscores.
fn bundle_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= BUNDLE_MAX
        && s.contains('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// The `device.*` call's params as the controller takes them. Every unknown key is `bad_params`.
pub fn device_op(method: Method, params: &Value) -> Result<AgentReq, CtlError> {
    let name = method.name();
    Ok(match method {
        Method::DeviceList => {
            only_fields(params, &[], name)?;
            AgentReq::List
        }
        Method::DeviceState => {
            only_fields(params, &[], name)?;
            AgentReq::State
        }
        Method::DeviceShutdown => {
            only_fields(params, &[], name)?;
            AgentReq::Shutdown
        }
        Method::DeviceUi => {
            only_fields(params, &[], name)?;
            AgentReq::Ui
        }
        Method::DeviceLook => {
            only_fields(params, &["tree"], name)?;
            AgentReq::Look {
                tree: bool_param(params, "tree")?.unwrap_or(true),
            }
        }
        Method::DeviceBoot => {
            only_fields(params, &["target", "name"], name)?;
            let target = match str_param(params, "target")? {
                None => None,
                Some(t) => Some(
                    flare_device::target::parse(t)
                        .or_else(|| Target::sim(t))
                        .ok_or_else(|| {
                            bad("target is a device of device_list, such as sim/<udid>")
                        })?,
                ),
            };
            let name = match str_param(params, "name")? {
                None => None,
                Some(n)
                    if !n.trim().is_empty()
                        && n.len() <= NAME_MAX
                        && !n.chars().any(char::is_control) =>
                {
                    Some(n.trim().to_string())
                }
                Some(_) => return Err(bad("name is a device's name, such as iPhone 18 Pro")),
            };
            if target.is_some() && name.is_some() {
                return Err(bad("give target or name, not both"));
            }
            AgentReq::Boot { target, name }
        }
        Method::DeviceTap => {
            only_fields(params, &["ref", "x", "y", "hold_ms"], name)?;
            let at =
                at(params)?.ok_or_else(|| bad("give ref (from device_look) or both x and y"))?;
            AgentReq::Act(DeviceOp::Tap {
                at,
                hold_ms: ms(params, "hold_ms")?,
            })
        }
        Method::DeviceSwipe => {
            only_fields(
                params,
                &["from", "to", "direction", "ref", "duration_ms"],
                name,
            )?;
            let from = point(params, "from")?;
            let to = point(params, "to")?;
            let r = device_ref(params, "ref")?;
            let dir = match str_param(params, "direction")? {
                None => None,
                Some("up") => Some(Direction::Up),
                Some("down") => Some(Direction::Down),
                Some("left") => Some(Direction::Left),
                Some("right") => Some(Direction::Right),
                Some(_) => return Err(bad("direction is up, down, left or right")),
            };
            let (from, to) = match (from, to, dir, r) {
                (Some(f), Some(t), None, None) => {
                    (Some(At::Point(f.0, f.1)), SwipeTo::Point(t.0, t.1))
                }
                (None, None, Some(d), r) => (r.map(At::Ref), SwipeTo::Direction(d)),
                (Some(f), None, Some(d), None) => {
                    (Some(At::Point(f.0, f.1)), SwipeTo::Direction(d))
                }
                _ => {
                    return Err(bad(
                        "give from and to, or direction (with ref or from to start somewhere else than the middle)",
                    ));
                }
            };
            AgentReq::Act(DeviceOp::Swipe {
                from,
                to,
                duration_ms: ms(params, "duration_ms")?,
            })
        }
        Method::DeviceType => {
            only_fields(params, &["text", "ref", "submit"], name)?;
            let text = str_param(params, "text")?.ok_or_else(|| bad("text is required"))?;
            if text.chars().count() > TYPE_MAX {
                return Err(bad(format!("text is over {TYPE_MAX} characters")));
            }
            let submit = bool_param(params, "submit")?.unwrap_or(false);
            if text.is_empty() && !submit {
                return Err(bad("text is required"));
            }
            AgentReq::Act(DeviceOp::Type {
                text: text.to_string(),
                at: device_ref(params, "ref")?.map(At::Ref),
                submit,
            })
        }
        Method::DevicePress => {
            only_fields(params, &["button", "key"], name)?;
            let press = match (str_param(params, "button")?, str_param(params, "key")?) {
                (Some(b), None) => Press::Button(Button::parse(b).ok_or_else(|| {
                    bad("button is home, lock, volume_up, volume_down, shake, rotate_left or rotate_right")
                })?),
                (None, Some(k)) => {
                    if flare_device::keymap::named(k).is_none() {
                        return Err(bad(
                            "key is return, delete, escape, tab, space or an arrow (up, down, left, right)",
                        ));
                    }
                    Press::Key(k.to_string())
                }
                _ => return Err(bad("give button or key, not both")),
            };
            AgentReq::Act(DeviceOp::Press(press))
        }
        Method::DeviceLaunch => {
            only_fields(params, &["app"], name)?;
            let app = str_param(params, "app")?
                .filter(|a| bundle_id(a))
                .ok_or_else(|| {
                    bad("app is an installed app's bundle id, such as com.apple.Preferences")
                })?;
            AgentReq::Launch {
                app: app.to_string(),
            }
        }
        Method::DeviceInstall => {
            only_fields(params, &["path"], name)?;
            let path = str_param(params, "path")?
                .filter(|p| !p.trim().is_empty() && p.len() <= PATH_MAX && !p.contains('\0'))
                .ok_or_else(|| bad("path is an .app folder of a simulator build"))?;
            AgentReq::Install {
                path: PathBuf::from(path),
            }
        }
        _ => {
            return Err(CtlError::new(
                code::INTERNAL,
                format!("{name} is not a device call"),
            ));
        }
    })
}

/// Xcode's DerivedData under `home` (where an agent's `xcodebuild` puts its builds).
pub fn derived_data(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Developer")
        .join("Xcode")
        .join("DerivedData")
}

/// `device.install`'s path, checked (rule 1322): resolved from the tab's folder when relative, its symlinks resolved
/// BEFORE the place is checked (so a link in the tab's folder to an app elsewhere is refused), inside the tab's folder
/// or DerivedData, an `.app` folder whose `Info.plist` names `iPhoneSimulator`. BLOCKING (the disk): the call's thread.
pub fn install_ok(path: &Path, tab_dir: &Path, derived: &Path) -> Result<PathBuf, CtlError> {
    let named = if path.is_absolute() {
        path.to_path_buf()
    } else {
        tab_dir.join(path)
    };
    // sync-ok: the control channel's worker thread, a few stats
    let real = std::fs::canonicalize(&named)
        .map_err(|_| CtlError::new(code::NOT_FOUND, "no such .app folder"))?;
    let inside = |root: &Path| {
        std::fs::canonicalize(root) // sync-ok: the call's worker
            .is_ok_and(|r| real.starts_with(&r))
    };
    if !(inside(tab_dir) || inside(derived)) {
        return Err(CtlError::new(
            Fail::Forbidden.code(),
            "device_install installs an app from this tab's folder or Xcode's DerivedData only",
        ));
    }
    let is_app = real
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"))
        && real.is_dir(); // sync-ok: the call's worker
    if !is_app {
        return Err(bad("path is an .app folder of a simulator build"));
    }
    // A binary or XML plist alike keeps the platform's name as plain ASCII.
    let plist = std::fs::read(real.join("Info.plist")).unwrap_or_default(); // sync-ok: the call's worker
    if !plist.windows(15).any(|w| w == b"iPhoneSimulator") {
        return Err(bad(
            "that app is not a simulator build (its Info.plist does not name iPhoneSimulator); build it for the simulator",
        ));
    }
    Ok(real)
}

impl AppDispatcher {
    /// The device gate the window keeps (tests set it).
    #[cfg(test)]
    pub(crate) fn with_device(self, on: bool) -> Self {
        self.device.store(on, Ordering::SeqCst);
        self
    }

    /// `device.*` (device panel PR 5): the caller's own tab only; a tab a remote viewer follows, or a run where the
    /// panel does not work, gets `unavailable` and nothing is posted (rule 1323). The params are checked first.
    pub(super) fn device_call(
        &self,
        ctx: &CallContext,
        method: Method,
        params: &Value,
    ) -> Result<Value, CtlError> {
        let tab = Self::tab_of_call(ctx)?;
        let op = device_op(method, params)?;
        if self.remote.as_ref().is_some_and(|h| h.is_followed(&tab)) {
            return Err(CtlError::new(code::UNAVAILABLE, REMOTE_WORDS));
        }
        if !self.device.load(Ordering::SeqCst) {
            return Err(CtlError::new(
                code::UNAVAILABLE,
                "the device panel is not available in this Flare: it needs a Mac with Apple silicon, Xcode and a Flare window",
            ));
        }
        let boot = matches!(op, AgentReq::Boot { .. });
        let deadline = Instant::now() + if boot { DEVICE_BOOT_WAIT } else { DEVICE_WAIT };
        let op = match op {
            AgentReq::Install { path } => {
                let cwd = self.ask_by(CtlOp::TabCwd { tab: tab.clone() }, deadline)?;
                let tab_dir = PathBuf::from(cwd["cwd"].as_str().unwrap_or_default());
                if tab_dir.as_os_str().is_empty() {
                    return Err(CtlError::new(
                        code::UNAVAILABLE,
                        "this tab's folder is not known yet",
                    ));
                }
                let real = install_ok(&path, &tab_dir, &derived_data(Path::new(&self.home)))?;
                AgentReq::Install { path: real }
            }
            other => other,
        };
        let r = self.ask_until(
            CtlOp::DeviceAgent {
                tab,
                op,
                cancel: ctx.cancel.clone(),
            },
            deadline,
            &ctx.cancel,
        );
        match r {
            // Spec "Failure modes": a boot over 120 s answers `busy`; the boot carries on.
            Err(e) if boot && e.code == code::TIMEOUT => Err(CtlError::new(
                Fail::Busy.code(),
                "the device is still starting; it carries on booting: call device_state or device_boot again in a moment",
            )),
            other => other,
        }
    }
}
