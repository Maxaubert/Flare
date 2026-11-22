//! `flare-emu-host --self-test` (CI on both OSes, and the release check): Flare's own parts work, and what this
//! machine has is reported. JSON out; exit 0 when Flare's parts pass. A missing or old SDK, a hypervisor that is off,
//! or no agent helper (PR A3) are reported, never a failure: the runner may have none of them.

use prost::Message;
use serde_json::{Value, json};

use flare_device::emu::discovery::Token;
use flare_device::emu::sdk::{self, Found};
use flare_device::emu::{Places, avds, boot, discovery};

use crate::pb;

/// The proto's hash, from the generated code's header (`proto-sha256: ...`).
pub fn proto_sha256() -> &'static str {
    include_str!("gen/android.emulation.control.rs")
        .lines()
        .take(5)
        .find_map(|l| l.strip_prefix("// proto-sha256: "))
        .unwrap_or("")
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> Value {
    json!({ "name": name, "ok": ok, "detail": detail.into() })
}

/// The checks of Flare's own parts.
pub fn own_checks() -> Vec<Value> {
    // The generated messages encode and decode (the frame request with MMAP).
    let fmt = pb::ImageFormat {
        format: pb::image_format::ImgFormat::Rgba8888 as i32,
        transport: Some(pb::ImageTransport {
            channel: pb::image_transport::TransportChannel::Mmap as i32,
            handle: "file:///C:/x/flare/1-1.rgba".to_string(),
        }),
        ..Default::default()
    };
    let back = pb::ImageFormat::decode(fmt.encode_to_vec().as_slice()).ok();
    let protos = back.as_ref() == Some(&fmt);
    // Rule 1362: the boot command line, and the token never printed.
    let line = boot::args("Flare_Pixel", 8554, 5554);
    let cmd = boot::check(&line);
    let t = Token::new("c2VsZi10ZXN0").map(|t| format!("{t:?} {t}")).unwrap_or_default();
    let hidden = !t.is_empty() && !t.contains("c2VsZi10ZXN0");
    vec![
        check("protos", protos, format!("proto-sha256 {}", proto_sha256())),
        check("boot_command", cmd.is_ok(), cmd.err().unwrap_or_else(|| line.join(" "))),
        check("token_hidden", hidden, "a token never prints"),
    ]
}

/// The self-test's JSON and whether it passed.
pub fn run() -> (Value, bool) {
    let checks = own_checks();
    let ok = checks.iter().all(|c| c["ok"] == true);
    let places = Places::from_env();
    let found = sdk::find(&places);
    let mut out = json!({
        "ok": ok,
        "protocol": flare_device::wire::PROTOCOL,
        "version": env!("CARGO_PKG_VERSION"),
        "checks": checks,
        "floor": sdk::floor(),
        "agent_jar": "none (Android plan PR A3)",
    });
    match &found {
        Found::Ready(s) | Found::TooOld(s) => {
            out["sdk"] = json!({
                "path": s.root.to_string_lossy(),
                "flare_own": s.flare_own,
                "emulator_version": s.emulator_version,
                "at_floor": matches!(found, Found::Ready(_)),
            });
            match crate::host::accel_check(s) {
                Ok(a) => out["accel"] = json!({ "usable": a.usable(), "kind": a.kind(), "line": a.line }),
                Err(e) => out["accel"] = json!({ "error": e }),
            }
        }
        Found::Missing => {
            out["skipped"] = json!("no Android emulator on this machine");
        }
    }
    if let Some(d) = places.discovery_dir() {
        out["discovery_dir"] = json!(d.to_string_lossy());
        out["running"] = json!(discovery::read_all(&d).len());
    }
    out["avds"] = json!(avds::list(&avds::homes(&places)).len());
    (out, ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flares_own_parts_pass_the_self_test() {
        for c in own_checks() {
            assert_eq!(c["ok"], true, "{c}");
        }
        assert_eq!(proto_sha256().len(), 64);
    }

    /// The generated code matches the vendored proto (rerun tools/emu-proto-gen after changing it).
    #[test]
    fn the_generated_code_is_from_the_vendored_proto() {
        use sha2::{Digest, Sha256};
        let proto = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("proto/emulator_controller.proto"),
        )
        .unwrap();
        let hash: String = Sha256::digest(proto.replace('\r', "").as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hash, proto_sha256(), "run cargo run --manifest-path tools/emu-proto-gen/Cargo.toml");
        assert!(proto.contains("Licensed under the Apache License, Version 2.0"), "the licence header stays");
    }
}
