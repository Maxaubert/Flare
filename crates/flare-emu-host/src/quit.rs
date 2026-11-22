//! `flare-emu-host --shutdown <avd>...`: Flare's quit (rule 1366). Flare starts this detached and never waits for it:
//! for each named AVD that runs with a gRPC token (only the emulators Flare booted are named), it asks for a clean
//! shutdown (`setVmState(SHUTDOWN)`; the emulator saves its Quick Boot state and ends by itself), then exits. An
//! emulator that is not running, or that runs without a token, is left alone. The token is never printed.

use std::time::Duration;

use flare_device::emu::Places;
use flare_device::emu::discovery::{self, Access};

use crate::{grpc, pb};

/// How long one emulator may take to take the request.
const ASK_S: u64 = 10;

/// Asks each running AVD in `avds` to shut down. Returns how many took the request.
pub fn run(avds: &[String]) -> usize {
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
    for avd in avds {
        let Some(r) = discovery::of_avd(&running, avd) else {
            continue;
        };
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
