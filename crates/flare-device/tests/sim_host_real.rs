//! The real path end to end (opt-in): `SimSource` starts the built `flare-sim-host`, which attaches to a simulator
//! this test makes (`simctl create`, deleted after; never one of the person's) and streams frames over the mach
//! channel. Runs only with `FLARE_DEVICE_REAL=1` on a Mac with Xcode and an iOS runtime; otherwise it says why and
//! passes. Headless: the simulator boots with no window.
#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

use flare_device::client::{SimConfig, SimSource};
use flare_device::macos::simctl::Simctl;
use flare_device::source::{Answer, DeviceSource, SourceEvent};

/// Not a test on its own: a guard started by this binary runs as this test.
#[test]
fn guard_entry() {
    flare_proc::guard::run_if_guard();
}

#[test]
fn a_real_sim_host_streams_frames_and_takes_slots_back() {
    if std::env::var("FLARE_DEVICE_REAL").as_deref() != Ok("1") {
        eprintln!("skipped: set FLARE_DEVICE_REAL=1 to run against a simulator made for the test");
        return;
    }
    let Some(xcode) = flare_device::xcode::find() else {
        eprintln!("skipped: no Xcode");
        return;
    };
    let dev = xcode.developer_dir();
    let simctl = Simctl::new(&dev);
    let ty = simctl.iphone_type().expect("an iPhone type");
    let target = simctl
        .create("Flare Test iPhone", &ty)
        .expect("simctl create");
    struct Delete<'a>(&'a Simctl, flare_device::Target);
    impl Drop for Delete<'_> {
        fn drop(&mut self) {
            let _ = self.0.shutdown(&self.1);
            let _ = self.0.delete(&self.1);
        }
    }
    let _delete = Delete(&simctl, target.clone());
    simctl.boot(&target).expect("boot");
    // The probe app paints every frame, so frames keep coming (a still home screen sends next to none).
    let build = tempfile::tempdir().unwrap();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/device-probe/build.sh");
    let out = std::process::Command::new("/bin/sh")
        .arg(&script)
        .arg(build.path())
        .env("DEVELOPER_DIR", &dev)
        .output()
        .expect("build.sh");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    simctl
        .install(&target, &build.path().join("FPSProbe.app"))
        .expect("install");
    simctl
        .launch(&target, flare_device::macos::bench::PROBE_BUNDLE, &[])
        .expect("launch");

    flare_proc::guard::use_test_entry("guard_entry").unwrap();
    let logs = tempfile::tempdir().unwrap();
    let mut src = SimSource::start(SimConfig::new(
        env!("CARGO_BIN_EXE_flare-sim-host").into(),
        dev,
        logs.path().join("sim-host.log"),
    ));
    let list = src.list();
    let attach = src.attach(&target);
    let end = Instant::now() + Duration::from_secs(60);
    let (mut listed, mut attached, mut ring, mut frames, mut changed) =
        (false, false, None, 0u32, 0u64);
    while Instant::now() < end && !(listed && attached && frames >= 30) {
        let Ok(e) = src.events().recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        match e {
            SourceEvent::Answer { id, result } if id == list => {
                let Ok(Answer::Devices(d)) = result else {
                    panic!("{result:?}")
                };
                assert!(
                    d.iter().any(|d| d.target == target),
                    "the new device is listed"
                );
                listed = true;
            }
            SourceEvent::Answer { id, result } if id == attach => {
                assert_eq!(result, Ok(Answer::Done));
                attached = true;
            }
            SourceEvent::Frames {
                target: t,
                ring: r,
                screen,
            } => {
                assert_eq!(t, target);
                assert!(
                    screen.scale >= 2.0 && screen.size_pt.w > 300.0,
                    "{screen:?}"
                );
                ring = Some(r);
            }
            SourceEvent::Frame { target: t, .. } => {
                assert_eq!(t, target);
                // The UI takes the newest and gives its slot back once drawn.
                if let Some((device, f)) = src.take_frame(&target) {
                    assert_eq!(Some(device), ring);
                    let surfaces = src
                        .ring(device)
                        .expect("the ring's surfaces came over mach");
                    assert_eq!(surfaces.len(), flare_device::cap::RING_SLOTS);
                    assert!(surfaces[f.slot as usize].width() > 0);
                    src.slot_done(device, f.slot);
                    frames += 1;
                }
            }
            SourceEvent::Changed { count, .. } => changed = changed.max(count),
            SourceEvent::Broken(why) => panic!(
                "broken: {why}\nlog: {}",
                std::fs::read_to_string(logs.path().join("sim-host.log")).unwrap_or_default()
            ),
            _ => {}
        }
    }
    let log = std::fs::read_to_string(logs.path().join("sim-host.log")).unwrap_or_default();
    assert!(
        listed && attached,
        "listed {listed}, attached {attached}\n{log}"
    );
    assert!(frames >= 30, "{frames} frames taken\n{log}");
    assert!(changed > 0);
    assert_eq!(src.starts(), 1, "no restart");
    let hello = src.hello().unwrap();
    assert!(!hello.coresim_version.is_empty());
    src.detach(&target);
    drop(src);
}
