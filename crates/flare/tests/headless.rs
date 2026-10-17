//! Runs the built binary without a window: backend probes only (Task 17 adds the screenshot test).

use std::process::{Command, Output};

fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flare"));
    cmd.args(args);
    cmd.env_remove("FLARE_FORCE_GPU_FAIL");
    cmd.env_remove("FLARE_RENDERER");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run flare")
}

#[test]
fn probe_forced_gpu_failure_falls_back_to_software() {
    let out = run(
        &["--probe-backend", "--renderer", "gpu"],
        &[("FLARE_FORCE_GPU_FAIL", "1")],
    );
    assert!(out.status.success(), "exit {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("backend: software fallback:"),
        "stdout was {stdout:?}"
    );
    assert!(
        stdout.contains("FLARE_FORCE_GPU_FAIL"),
        "stdout was {stdout:?}"
    );
}

#[test]
fn probe_software_reports_software() {
    let out = run(&["--probe-backend", "--renderer", "software"], &[]);
    assert!(out.status.success(), "exit {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim_end(), "backend: software");
}
