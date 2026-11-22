//! `xcrun simctl`, the public tool: list, boot, shut down, screenshot (still mode), launch, install, the simulator's
//! own pasteboard, and create and delete (tests and the bench only). Every call has a limit and `DEVELOPER_DIR` set
//! to the chosen Xcode. Every call is BLOCKING: the sim host's request threads and Flare's workers only (rule 34).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::source::{DeviceInfo, DeviceState};
use crate::target::Target;

/// The limit on a quick call (list, shutdown, launch, pbcopy, screenshot, create, delete).
pub const QUICK: Duration = Duration::from_secs(30);
/// The limit on a boot and its wait (`boot` then `bootstatus -b`): 22 s cold on the Mac mini, so room for a slow Mac.
pub const BOOT_LIMIT: Duration = Duration::from_secs(180);
/// The limit on an install (a big app copies for a while).
pub const INSTALL_LIMIT: Duration = Duration::from_secs(120);
/// The limit on a screenshot (371 ms each on the Mac mini).
pub const SHOT_LIMIT: Duration = Duration::from_secs(10);

/// A call that failed: the tool's own words (its first error line), or that it ran out of time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunError {
    pub message: String,
    pub timed_out: bool,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Runs `cmd` (stdin `input`, or nothing), at most `limit`, killing it after. Its output, or its error text.
/// BLOCKING: a worker only.
pub fn run(cmd: &mut Command, input: Option<&[u8]>, limit: Duration) -> Result<Vec<u8>, RunError> {
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| RunError {
        message: format!(
            "{} could not start: {e}",
            cmd.get_program().to_string_lossy()
        ),
        timed_out: false,
    })?; // sync-ok: a worker
    if let (Some(text), Some(mut w)) = (input, child.stdin.take()) {
        let text = text.to_vec();
        std::thread::spawn(move || {
            let _ = w.write_all(&text);
        });
    }
    let pump = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut b);
            }
            b
        })
    };
    let out = pump(
        child
            .stdout
            .take()
            .map(|o| Box::new(o) as Box<dyn Read + Send>),
    );
    let err = pump(
        child
            .stderr
            .take()
            .map(|e| Box::new(e) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if started.elapsed() < limit => {
                std::thread::sleep(Duration::from_millis(10)); // sync-ok: a worker's bounded wait
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait(); // sync-ok: reaps a killed child at once
                break None;
            }
        }
    };
    let out = out.join().unwrap_or_default(); // sync-ok: the pipe closed with the child
    let err = err.join().unwrap_or_default(); // sync-ok: the pipe closed with the child
    match status {
        Some(s) if s.success() => Ok(out),
        Some(s) => {
            let text = String::from_utf8_lossy(&err);
            let line = text
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .to_string();
            Err(RunError {
                message: if line.is_empty() {
                    format!("exited with {s}")
                } else {
                    line
                },
                timed_out: false,
            })
        }
        None => Err(RunError {
            message: format!("gave no answer in {} s", limit.as_secs()),
            timed_out: true,
        }),
    }
}

/// `simctl` of one Xcode.
#[derive(Clone, Debug)]
pub struct Simctl {
    developer_dir: PathBuf,
}

impl Simctl {
    pub fn new(developer_dir: &Path) -> Simctl {
        Simctl {
            developer_dir: developer_dir.to_path_buf(),
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new("/usr/bin/xcrun"); // sync-ok: builds the command; run on a worker
        c.arg("simctl")
            .args(args)
            .env("DEVELOPER_DIR", &self.developer_dir);
        c
    }

    fn call(&self, args: &[&str], limit: Duration) -> Result<Vec<u8>, RunError> {
        run(&mut self.cmd(args), None, limit)
    }

    /// The devices this Mac can run (`list -j devices available`).
    pub fn list(&self) -> Result<Vec<DeviceInfo>, RunError> {
        let out = self.call(&["list", "-j", "devices", "available"], QUICK)?;
        Ok(parse_list(&String::from_utf8_lossy(&out)))
    }

    /// Boots headless and waits until it is booted. A device booted already is fine.
    pub fn boot(&self, target: &Target) -> Result<(), RunError> {
        let started = Instant::now();
        let udid = udid(target);
        if let Err(e) = self.call(&["boot", &udid], BOOT_LIMIT)
            && !e.message.contains("current state: Booted")
        {
            return Err(e);
        }
        let left = BOOT_LIMIT.saturating_sub(started.elapsed());
        self.call(&["bootstatus", &udid, "-b"], left).map(|_| ())
    }

    pub fn shutdown(&self, target: &Target) -> Result<(), RunError> {
        match self.call(&["shutdown", &udid(target)], QUICK) {
            Err(e) if e.message.contains("current state: Shutdown") => Ok(()),
            r => r.map(|_| ()),
        }
    }

    /// A PNG of the screen at full size (still mode).
    pub fn screenshot(&self, target: &Target) -> Result<Vec<u8>, RunError> {
        self.call(
            &["io", &udid(target), "screenshot", "--type=png", "-"],
            SHOT_LIMIT,
        )
    }

    pub fn launch(
        &self,
        target: &Target,
        bundle_id: &str,
        args: &[String],
    ) -> Result<(), RunError> {
        let udid = udid(target);
        let mut a = vec!["launch", udid.as_str(), bundle_id];
        a.extend(args.iter().map(String::as_str));
        self.call(&a, QUICK).map(|_| ())
    }

    pub fn install(&self, target: &Target, app: &Path) -> Result<(), RunError> {
        let app = app.to_string_lossy();
        self.call(&["install", &udid(target), &app], INSTALL_LIMIT)
            .map(|_| ())
    }

    /// Writes the simulator's own pasteboard, never the Mac's (rule 1314).
    pub fn pbcopy(&self, target: &Target, text: &str) -> Result<(), RunError> {
        run(
            &mut self.cmd(&["pbcopy", &udid(target)]),
            Some(text.as_bytes()),
            QUICK,
        )
        .map(|_| ())
    }

    /// Makes a device for a test or the bench (never one of the person's): its udid.
    pub fn create(&self, name: &str, device_type: &str) -> Result<Target, RunError> {
        let out = self.call(&["create", name, device_type], QUICK)?;
        let text = String::from_utf8_lossy(&out);
        Target::sim(text.trim()).ok_or_else(|| RunError {
            message: format!("simctl create printed {:?}", text.trim()),
            timed_out: false,
        })
    }

    /// Deletes a device made by [`Simctl::create`].
    pub fn delete(&self, target: &Target) -> Result<(), RunError> {
        self.call(&["delete", &udid(target)], QUICK).map(|_| ())
    }

    /// The newest iPhone device type's identifier (for `create`), from `list -j devicetypes`.
    pub fn iphone_type(&self) -> Result<String, RunError> {
        let out = self.call(&["list", "-j", "devicetypes"], QUICK)?;
        let v: Value = serde_json::from_slice(&out).unwrap_or(Value::Null);
        let types = v["devicetypes"].as_array().cloned().unwrap_or_default();
        let ids: Vec<(String, String)> = types
            .iter()
            .filter(|t| t["productFamily"] == "iPhone")
            .filter_map(|t| {
                Some((
                    t["name"].as_str()?.to_string(),
                    t["identifier"].as_str()?.to_string(),
                ))
            })
            .collect();
        ids.iter()
            .find(|(n, _)| n.ends_with(" Pro") && !n.contains("Max"))
            .or_else(|| ids.last())
            .map(|(_, id)| id.clone())
            .ok_or_else(|| RunError {
                message: "this Xcode has no iPhone device type".into(),
                timed_out: false,
            })
    }
}

/// `simctl` takes the udid as it prints it (upper case).
fn udid(target: &Target) -> String {
    target.udid().to_ascii_uppercase()
}

/// The devices in `simctl list -j devices`: iPhones first, then iPads, newest OS first, by name. A device whose
/// state or udid this build does not know is left out.
pub fn parse_list(json: &str) -> Vec<DeviceInfo> {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let Some(runtimes) = v["devices"].as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (runtime, devices) in runtimes {
        let Some(os) = os_name(runtime) else {
            continue;
        };
        for d in devices.as_array().into_iter().flatten() {
            if d["isAvailable"] == false {
                continue;
            }
            let (Some(udid), Some(name), Some(state)) = (
                d["udid"].as_str(),
                d["name"].as_str(),
                d["state"].as_str().and_then(DeviceState::from_simctl),
            ) else {
                continue;
            };
            let Some(target) = Target::sim(udid) else {
                continue;
            };
            let kind = d["deviceTypeIdentifier"]
                .as_str()
                .and_then(|t| t.rsplit('.').next())
                .and_then(|t| t.split('-').next())
                .filter(|k| !k.is_empty())
                .unwrap_or("")
                .to_string();
            out.push(DeviceInfo {
                target,
                name: name.to_string(),
                kind,
                os: os.clone(),
                state,
                booted_by_flare: false,
            });
        }
    }
    let family = |k: &str| match k {
        "iPhone" => 0,
        "iPad" => 1,
        _ => 2,
    };
    let os_version = |os: &str| {
        os.rsplit(' ')
            .next()
            .and_then(crate::versions::Version::parse)
    };
    out.sort_by(|a, b| {
        family(&a.kind)
            .cmp(&family(&b.kind))
            .then_with(|| os_version(&b.os).cmp(&os_version(&a.os)))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// "iOS 27.2" from `com.apple.CoreSimulator.SimRuntime.iOS-27-2`; None for watchOS, tvOS and visionOS (the panel
/// shows phones and tablets).
fn os_name(runtime: &str) -> Option<String> {
    let tail = runtime.rsplit('.').next()?;
    let (os, version) = tail.split_once('-')?;
    if os != "iOS" {
        return None;
    }
    // Only the version's numbers: a runtime made for one model adds it (`iOS-27-2-iPhone19-4`).
    let parts: Vec<&str> = version
        .split('-')
        .take_while(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(format!("iOS {}", parts.join(".")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_os_is_the_runtime_version_without_a_model_suffix() {
        let rt = "com.apple.CoreSimulator.SimRuntime.";
        assert_eq!(
            os_name(&format!("{rt}iOS-27-2")).as_deref(),
            Some("iOS 27.2")
        );
        assert_eq!(
            os_name(&format!("{rt}iOS-27-2-iPhone19-4")).as_deref(),
            Some("iOS 27.2")
        );
        assert_eq!(os_name(&format!("{rt}watchOS-27-2")), None);
        assert_eq!(os_name(&format!("{rt}iOS-beta")), None);
    }

    #[test]
    fn the_device_list_is_read_phones_first_newest_first() {
        let json = r#"{"devices":{
          "com.apple.CoreSimulator.SimRuntime.iOS-26-4":[
            {"udid":"11111111-1111-4111-8111-111111111111","name":"iPhone 17","state":"Shutdown","isAvailable":true,
             "deviceTypeIdentifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17"}],
          "com.apple.CoreSimulator.SimRuntime.iOS-27-2":[
            {"udid":"D2FFFFA2-FB27-4D6A-B889-5D6BF072999C","name":"iPhone 18 Pro","state":"Booted","isAvailable":true,
             "deviceTypeIdentifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-18-Pro"},
            {"udid":"68245989-2ABB-4313-96EB-F2C7D0C45138","name":"iPad mini (A20 Pro)","state":"Shutdown",
             "isAvailable":true,"deviceTypeIdentifier":"com.apple.CoreSimulator.SimDeviceType.iPad-mini-A20-Pro"},
            {"udid":"not-a-udid","name":"Broken","state":"Shutdown","isAvailable":true},
            {"udid":"22222222-2222-4222-8222-222222222222","name":"Gone","state":"Shutdown","isAvailable":false},
            {"udid":"33333333-3333-4333-8333-333333333333","name":"New","state":"Creating","isAvailable":true}],
          "com.apple.CoreSimulator.SimRuntime.watchOS-27-2":[
            {"udid":"44444444-4444-4444-8444-444444444444","name":"Watch","state":"Shutdown","isAvailable":true}]
        }}"#;
        let list = parse_list(json);
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["iPhone 18 Pro", "iPhone 17", "iPad mini (A20 Pro)"]);
        assert_eq!(list[0].os, "iOS 27.2");
        assert_eq!(list[0].kind, "iPhone");
        assert_eq!(list[0].state, DeviceState::Booted);
        assert_eq!(
            list[0].target.name(),
            "sim/d2ffffa2-fb27-4d6a-b889-5d6bf072999c",
            "lower-cased"
        );
        assert_eq!(
            udid(&list[0].target),
            "D2FFFFA2-FB27-4D6A-B889-5D6BF072999C"
        );
        assert_eq!(list[2].kind, "iPad");
        assert_eq!(list[1].os, "iOS 26.4");
        assert!(parse_list("not json").is_empty());
        assert!(parse_list("{}").is_empty());
    }

    #[test]
    fn a_call_that_runs_too_long_is_killed_and_says_so() {
        let t = Instant::now();
        let e = run(
            Command::new("/bin/sleep").arg("30"),
            None,
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(e.timed_out);
        assert!(t.elapsed() < Duration::from_secs(5));
        let e = run(
            Command::new("/bin/sh").args(["-c", "echo 'An error was encountered' >&2; exit 3"]),
            None,
            QUICK,
        )
        .unwrap_err();
        assert_eq!(e.message, "An error was encountered");
        assert!(!e.timed_out);
        let out = run(&mut Command::new("/bin/cat"), Some(b"caf\xc3\xa9"), QUICK).unwrap();
        assert_eq!(out, "caf\u{e9}".as_bytes());
    }
}
