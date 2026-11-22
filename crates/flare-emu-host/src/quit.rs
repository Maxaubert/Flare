//! `flare-emu-host --shutdown <avd>[@<ms>]...`: Flare's quit (rule 1366). Flare starts this detached and never waits
//! for it: for each named AVD that runs with a gRPC token (only the emulators Flare booted are named), it asks for a
//! clean shutdown (`setVmState(SHUTDOWN)`; the emulator saves its Quick Boot state and ends by itself), then exits. An
//! emulator that is not running, or that runs without a token, is left alone. The token is never printed.
//!
//! `@<ms>` is when Flare's boot of it ended (Unix ms, `booted.json`). An emulator writes its discovery file when it
//! starts, so one Flare booted has a file made before that time. A file made later belongs to an emulator started
//! after the one Flare booted had gone (it idled out, then the person started the same AVD from Android Studio): that
//! one is not Flare's and is left running.

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use flare_device::emu::Places;
use flare_device::emu::discovery::{self, Access};

use crate::{grpc, pb};

/// How long one emulator may take to take the request.
const ASK_S: u64 = 10;

/// Slack for the clock between Flare's boot time and the discovery file's time.
const SLACK_MS: u64 = 60_000;

/// One `--shutdown` argument: an AVD name, checked as a target is, and the boot time after `@` if any. None for
/// anything else.
pub fn arg(a: &str) -> Option<(String, Option<u64>)> {
    let (avd, at) = match a.split_once('@') {
        Some((avd, ms)) => (avd, Some(ms.parse::<u64>().ok()?)),
        None => (a, None),
    };
    flare_device::target::Target::emu(avd).map(|_| (avd.to_string(), at))
}

/// Whether an emulator whose discovery file was made at `made_ms` is the one Flare booted by `booted_ms`. Unknown
/// times (an old entry, a file system without creation times) keep the old rule: it is Flare's.
pub fn flares(booted_ms: Option<u64>, made_ms: Option<u64>) -> bool {
    match (booted_ms, made_ms) {
        (Some(b), Some(m)) => m <= b.saturating_add(SLACK_MS),
        _ => true,
    }
}

/// When the discovery file of `pid` in `dir` was made (Unix ms), if the file system says.
fn made_ms(dir: &Path, pid: u32) -> Option<u64> {
    [format!("pid_{pid}.ini"), format!("pid_{pid}_info.ini")]
        .iter()
        .find_map(|n| std::fs::metadata(dir.join(n)).ok()?.created().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| u64::try_from(d.as_millis()).ok())
}

/// Asks each running AVD in `avds` (with when Flare booted it) to shut down. Returns how many took the request.
pub fn run(avds: &[(String, Option<u64>)]) -> usize {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return 0;
    };
    let places = Places::from_env();
    let Some(dir) = places.discovery_dir() else {
        return 0;
    };
    let running = discovery::read_all(&dir);
    let mut n = 0;
    for (avd, booted) in avds {
        let Some(r) = discovery::of_avd(&running, avd) else {
            continue;
        };
        if !flares(*booted, made_ms(&dir, r.pid)) {
            eprintln!(
                "flare-emu-host: shutdown: {avd} was started after Flare's boot of it; left running"
            );
            continue;
        }
        let Access::Token { port, token } = r.access() else {
            continue;
        };
        let asked = rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(ASK_S), async {
                let c = grpc::connect(port, &token).await.ok()?;
                c.clone()
                    .set_vm_state(pb::VmRunState {
                        state: pb::vm_run_state::RunState::Shutdown as i32,
                    })
                    .await
                    .ok()
            })
            .await
            .ok()
            .flatten()
        });
        if asked.is_some() {
            n += 1;
        } else {
            eprintln!("flare-emu-host: shutdown: {avd} did not take the request");
        }
    }
    n
}

/// Now in Unix ms (tests).
#[cfg(test)]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shutdown_argument_is_an_avd_name_and_maybe_its_boot_time() {
        assert_eq!(arg("Flare_Pixel"), Some(("Flare_Pixel".into(), None)));
        assert_eq!(
            arg("Flare_Pixel@1732280000000"),
            Some(("Flare_Pixel".into(), Some(1_732_280_000_000)))
        );
        for bad in ["-x", "Flare_Pixel@", "Flare_Pixel@soon", "a b", "@5", ""] {
            assert_eq!(arg(bad), None, "{bad}");
        }
    }

    /// Rule 1366: an emulator started after Flare's boot of it ended (the one Flare booted idled out, the person
    /// started the same AVD in Android Studio) is not Flare's, so quit leaves it running.
    #[test]
    fn only_the_emulator_flare_booted_is_shut_down() {
        let booted = 1_000_000;
        assert!(
            flares(Some(booted), Some(booted - 30_000)),
            "started by Flare's boot"
        );
        assert!(
            flares(Some(booted), Some(booted + 5_000)),
            "within the clock's slack"
        );
        assert!(
            !flares(Some(booted), Some(booted + 10 * 60_000)),
            "started later"
        );
        assert!(flares(None, Some(booted)), "an old entry without a time");
        assert!(flares(Some(booted), None), "no creation time");
        // The discovery file's time is read from the file system.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("pid_42.ini"), "avd.id=Flare_Pixel\n").unwrap();
        let made = made_ms(d.path(), 42);
        if let Some(m) = made {
            assert!(m.abs_diff(now_ms()) < 60_000, "{m}");
        }
        assert_eq!(made_ms(d.path(), 43), None);
    }
}
