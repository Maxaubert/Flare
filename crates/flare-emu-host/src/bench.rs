//! `flare-emu-host --bench <avd> <seconds> [--boot] [--width <px>] [--rotate] [--snapshots <n>]`: the frame path
//! against a real emulator, by hand (never in CI). JSON out.
//!
//! With `--boot` an AVD that is not running is booted with Flare's command line (rule 1362) and shut down after.
//! The frames stream with MMAP into a region in the `flare` folder of the discovery folder (rule 1363), deleted after.
//! Reports frames, fps, interval p50 and p99, `seq` gaps, the frame size (with `--width`, whether MMAP scales), the
//! rotation a landscape turn reports (`--rotate`) and the snapshot time. Use it only on an AVD made for the run.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use flare_device::emu::discovery::{self, Access};
use flare_device::emu::region::{self, Region};
use flare_device::emu::sdk::{self, Found};
use flare_device::emu::{Places, avds, boot, keys};
use flare_device::geom::Orientation;

use crate::grpc::{self, Client};
use crate::pb;

pub struct Args {
    pub avd: String,
    pub seconds: f64,
    pub boot: bool,
    pub width: u32,
    pub rotate: bool,
    pub snapshots: u32,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let i = ((v.len() as f64 * p).ceil() as usize).clamp(1, v.len()) - 1;
    (v[i] * 100.0).round() / 100.0
}

pub fn run(a: &Args) -> Result<Value, String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(bench(a))
}

async fn wait_running(dir: &std::path::Path, avd: &str, limit: Duration) -> Option<discovery::Running> {
    let started = Instant::now();
    while started.elapsed() < limit {
        if let Some(r) = discovery::of_avd(&discovery::read_all(dir), avd).cloned()
            && matches!(r.access(), Access::Token { .. })
        {
            return Some(r);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
}

async fn bench(a: &Args) -> Result<Value, String> {
    let places = Places::from_env();
    let Found::Ready(sdk) = sdk::find(&places) else {
        return Err(format!("no Android emulator at {} or newer", sdk::floor()));
    };
    let dir = places.discovery_dir().ok_or("no discovery folder")?;
    let mut out = json!({ "avd": a.avd, "emulator_version": sdk.emulator_version });
    let started = Instant::now();
    let mut booted_here = None;
    let running = match discovery::of_avd(&discovery::read_all(&dir), &a.avd).cloned() {
        Some(r) => r,
        None if a.boot => {
            let avd = avds::list(&avds::homes(&places))
                .into_iter()
                .find(|x| x.name == a.avd)
                .ok_or(format!("no AVD named {}", a.avd))?;
            let taken: Vec<u16> = discovery::read_all(&dir)
                .iter()
                .flat_map(|r| [r.console_port, r.adb_port])
                .flatten()
                .collect();
            let console = boot::console_port(&taken, boot::port_busy).ok_or("no free console port")?;
            let grpc_port = boot::free_port().ok_or("no free port")?;
            let line = boot::args(&a.avd, grpc_port, console);
            let mut c = boot::command(&sdk.emulator(), &sdk.root, &avd.home, &line, None, true)?;
            let child = match c.spawn() {
                Ok(ch) => ch,
                Err(_) => boot::command(&sdk.emulator(), &sdk.root, &avd.home, &line, None, false)?
                    .spawn()
                    .map_err(|e| format!("the emulator did not start: {e}"))?,
            };
            booted_here = Some(child);
            out["boot_command"] = json!(line.join(" "));
            wait_running(&dir, &a.avd, Duration::from_secs(boot::BOOT_WAIT_S))
                .await
                .ok_or("no discovery file with a token")?
        }
        None => return Err(format!("{} is not running (add --boot)", a.avd)),
    };
    let Access::Token { port, token } = running.access() else {
        return Err(discovery::NO_ACCESS.to_string());
    };
    let client = grpc::connect(port, &token).await?;
    // Booted.
    loop {
        let s = client.clone().get_status(()).await.map_err(|e| e.message().to_string())?;
        if s.into_inner().booted {
            break;
        }
        if started.elapsed() > Duration::from_secs(boot::BOOT_WAIT_S) {
            return Err("not booted in time".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    out["boot_s"] = json!(started.elapsed().as_secs_f64().round());
    out["discovery_keys"] = json!({
        "pid": running.pid,
        "grpc_port": running.grpc_port.is_some(),
        "token": running.token.is_some(),
        "serial": running.serial(),
        "version": running.version,
    });
    let r = frames(a, &client, &dir).await;
    let snaps = snapshots(a, &client).await;
    if let Some(mut child) = booted_here {
        let _ = client
            .clone()
            .set_vm_state(pb::VmRunState {
                state: pb::vm_run_state::RunState::Shutdown as i32,
            })
            .await;
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(90) {
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        out["shut_down_s"] = json!(t.elapsed().as_secs_f64().round());
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            out["killed"] = json!(true);
        }
    }
    out["frames"] = r?;
    out["snapshot"] = snaps?;
    Ok(out)
}

async fn frames(a: &Args, client: &Client, dir: &std::path::Path) -> Result<Value, String> {
    let one = client
        .clone()
        .get_screenshot(pb::ImageFormat {
            format: pb::image_format::ImgFormat::Rgba8888 as i32,
            ..Default::default()
        })
        .await
        .map_err(|e| e.message().to_string())?
        .into_inner();
    let f = one.format.unwrap_or_default();
    let reg = Region::create(
        &region::dir(dir),
        std::process::id(),
        1,
        region::bytes_for(f.width, f.height),
    )
    .map_err(|e| format!("no region: {e}"))?;
    let fmt = pb::ImageFormat {
        format: pb::image_format::ImgFormat::Rgba8888 as i32,
        width: a.width,
        transport: Some(pb::ImageTransport {
            channel: pb::image_transport::TransportChannel::Mmap as i32,
            handle: reg.url(),
        }),
        ..Default::default()
    };
    let mut s = client
        .clone()
        .stream_screenshot(fmt)
        .await
        .map_err(|e| format!("stream refused: {} {}", e.code(), e.message()))?
        .into_inner();
    let mut times = Vec::new();
    let mut gaps = 0u64;
    let mut last: Option<u32> = None;
    let mut sizes = std::collections::BTreeSet::new();
    let mut rotations = std::collections::BTreeSet::new();
    let started = Instant::now();
    let limit = Duration::from_secs_f64(a.seconds);
    let mut turned_at: Option<Instant> = None;
    let mut turn_seen: Option<Value> = None;
    while started.elapsed() < limit {
        let left = limit.saturating_sub(started.elapsed());
        let Ok(m) = tokio::time::timeout(left, s.message()).await else {
            break;
        };
        let img = match m {
            Ok(Some(i)) => i,
            Ok(None) => break,
            Err(e) => return Err(format!("stream failed: {} {}", e.code(), e.message())),
        };
        times.push(Instant::now());
        if last.is_some_and(|l| img.seq > l + 1) {
            gaps += 1;
        }
        last = Some(img.seq);
        let f = img.format.unwrap_or_default();
        sizes.insert((f.width, f.height));
        let rot = f.rotation.map_or(0, |r| r.rotation);
        rotations.insert(rot);
        if a.rotate && turned_at.is_none() && started.elapsed() > limit / 3 {
            let [x, y, z] = keys::rotation_degrees(Orientation::Rot90);
            client
                .clone()
                .set_physical_model(pb::PhysicalModelValue {
                    target: pb::physical_model_value::PhysicalType::Rotation as i32,
                    value: Some(pb::ParameterValue { data: vec![x, y, z] }),
                    ..Default::default()
                })
                .await
                .map_err(|e| e.message().to_string())?;
            turned_at = Some(Instant::now());
        }
        if let Some(t) = turned_at
            && turn_seen.is_none()
            && rot != 0
        {
            turn_seen = Some(json!({ "rotation": rot, "w": f.width, "h": f.height, "ms": t.elapsed().as_millis() }));
        }
    }
    if turned_at.is_some() {
        let _ = client
            .clone()
            .set_physical_model(pb::PhysicalModelValue {
                target: pb::physical_model_value::PhysicalType::Rotation as i32,
                value: Some(pb::ParameterValue { data: vec![0.0, 0.0, 0.0] }),
                ..Default::default()
            })
            .await;
    }
    let mut iv: Vec<f64> = times
        .windows(2)
        .map(|w| (w[1] - w[0]).as_secs_f64() * 1000.0)
        .collect();
    let wall = times
        .last()
        .zip(times.first())
        .map_or(0.0, |(l, f)| (*l - *f).as_secs_f64());
    let path = reg.path().to_path_buf();
    drop(reg);
    Ok(json!({
        "full_size": [f.width, f.height],
        "width_asked": a.width,
        "sizes": sizes.into_iter().collect::<Vec<_>>(),
        "rotations": rotations.into_iter().collect::<Vec<_>>(),
        "turn_to_rot90_gave": turn_seen,
        "records": times.len(),
        "fps": if wall > 0.0 { (times.len() as f64 / wall * 10.0).round() / 10.0 } else { 0.0 },
        "iv_p50_ms": pct(&mut iv, 0.5),
        "iv_p99_ms": pct(&mut iv, 0.99),
        "seq_gaps": gaps,
        "region_deleted": !path.exists(),
    }))
}

async fn snapshots(a: &Args, client: &Client) -> Result<Value, String> {
    let mut ms = Vec::new();
    let mut size = (0, 0);
    for _ in 0..a.snapshots.max(1) {
        let t = Instant::now();
        let img = client
            .clone()
            .get_screenshot(pb::ImageFormat {
                format: pb::image_format::ImgFormat::Png as i32,
                width: 411,
                height: 914,
                ..Default::default()
            })
            .await
            .map_err(|e| e.message().to_string())?
            .into_inner();
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
        let f = img.format.unwrap_or_default();
        size = (f.width, f.height);
        if !img.image.starts_with(b"\x89PNG") {
            return Err("no PNG".into());
        }
    }
    Ok(json!({ "size": [size.0, size.1], "p50_ms": pct(&mut ms, 0.5) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_read_the_sorted_values() {
        let mut v = vec![5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(pct(&mut v, 0.5), 3.0);
        assert_eq!(pct(&mut v, 0.99), 5.0);
        assert_eq!(pct(&mut [], 0.5), 0.0);
    }
}
