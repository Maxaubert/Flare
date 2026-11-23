//! How Flare boots an emulator (spec builder's call 11, rule 1362):
//!
//! `emulator -avd <name> -no-window -no-audio -no-boot-anim -gpu host -grpc <port> -grpc-use-token
//! -idle-grpc-timeout 300 -port <even console port>`
//!
//! - `-grpc-use-token`, never a plain `-grpc`: a plain port lets any local program drive the device. The emulator puts
//!   the token in its discovery file; it is never logged or shown (rule 1362).
//! - `-no-window` picks the headless engine: no window, no taskbar button, no Dock icon (spike A5, rule 1370).
//! - `-gpu host`: gfxstream on the host GPU, on both OSes.
//! - `-idle-grpc-timeout 300`: the emulator shuts itself down cleanly five minutes after its last gRPC call, so one
//!   Flare booted stops by itself after Flare dies (rule 1366). The emu host calls `getStatus` every minute.
//! - Quick Boot stays as the AVD has it, so a second boot is fast.

/// Seconds without a gRPC call before a Flare-booted emulator shuts itself down (rule 1366).
pub const IDLE_TIMEOUT_S: u32 = 300;
/// How often the emu host calls `getStatus` while Flare runs (well inside [`IDLE_TIMEOUT_S`]).
pub const KEEP_ALIVE_S: u64 = 60;
/// How long a boot may take before Flare says "Still starting..." (the PC's first cold boot took 139 s).
pub const BOOT_WAIT_S: u64 = 300;
/// The emulator's console ports: even numbers from 5554 to 5682 (the console on the even port, adb on the next).
pub const CONSOLE_FIRST: u16 = 5554;
pub const CONSOLE_LAST: u16 = 5682;

/// The arguments after the emulator's path.
pub fn args(avd: &str, grpc_port: u16, console_port: u16) -> Vec<String> {
    let idle = IDLE_TIMEOUT_S.to_string();
    [
        "-avd",
        avd,
        "-no-window",
        "-no-audio",
        "-no-boot-anim",
        "-gpu",
        "host",
        "-grpc",
        &grpc_port.to_string(),
        "-grpc-use-token",
        "-idle-grpc-timeout",
        &idle,
        "-port",
        &console_port.to_string(),
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// The same command line for a cold boot (`-no-snapshot-load`): the second try when an emulator ended at once. Seen on
/// 37.2.13 (2026-11-22): a Quick Boot state saved by the idle timeout's own shutdown made the next boot shut down again
/// about a second after it loaded, when no gRPC call came first.
pub fn cold(mut args: Vec<String>) -> Vec<String> {
    if !args.iter().any(|a| a == "-no-snapshot-load") {
        args.push("-no-snapshot-load".to_string());
    }
    args
}

/// An emulator that ends this soon after its start did not boot (the second try is a cold boot).
pub const ENDED_AT_ONCE_S: u64 = 30;

/// Rule 1362, checked on every command line Flare would start: the token, no window, the host GPU and the idle
/// timeout are there, and nothing opens another way in. Err names what is wrong.
pub fn check(args: &[String]) -> Result<(), String> {
    let has = |a: &str| args.iter().any(|x| x == a);
    let value = |a: &str| {
        args.iter()
            .position(|x| x == a)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    if !has("-grpc-use-token") {
        return Err("no -grpc-use-token: the gRPC port would be open to every program".into());
    }
    if !has("-no-window") {
        return Err("no -no-window: the emulator would show a window".into());
    }
    if value("-gpu") != Some("host") {
        return Err("no -gpu host".into());
    }
    if value("-idle-grpc-timeout") != Some(&IDLE_TIMEOUT_S.to_string()) {
        return Err(format!("no -idle-grpc-timeout {IDLE_TIMEOUT_S}"));
    }
    if value("-grpc").is_none_or(|p| p.parse::<u16>().is_err()) {
        return Err("no -grpc <port>".into());
    }
    for open in ["-grpc-allowlist", "-qemu", "-shell-serial", "-http-proxy"] {
        if has(open) {
            return Err(format!("{open} is not one Flare uses"));
        }
    }
    match value("-avd") {
        Some(a) if crate::target::Target::emu(a).is_some() => Ok(()),
        _ => Err("no valid -avd".into()),
    }
}

/// The lowest console port that neither a running emulator holds (`taken`, each holds its port and the next) nor
/// `busy` says is in use (the caller binds both ports to check). None when all 65 are in use.
pub fn console_port(taken: &[u16], busy: impl Fn(u16) -> bool) -> Option<u16> {
    (CONSOLE_FIRST..=CONSOLE_LAST)
        .step_by(2)
        .find(|p| !taken.contains(p) && !taken.contains(&(p + 1)) && !busy(*p) && !busy(p + 1))
}

/// Windows process flags for the emulator: no console window (rule 1370), a process group of its own, and out of
/// any job its parent is in, so nothing that ends with Flare ends it (rule 1366: it stops by its idle timeout).
pub const WIN_NO_WINDOW: u32 = 0x0800_0000;
pub const WIN_NEW_GROUP: u32 = 0x0000_0200;
pub const WIN_BREAKAWAY: u32 = 0x0100_0000;

/// adb's mDNS discovery off (rule 1370). An adb server with mDNS on binds UDP 5353 on every network (`0.0.0.0`), and
/// Windows then shows its Firewall dialog the first time an `adb.exe` from a new folder (Flare's own SDK) does so (seen
/// in A2b's real setup). With `ADB_MDNS=0` the server binds only its localhost port (checked 2026-11-22 with
/// platform-tools 37.0.1: `0.0.0.0:5353` gone from the server's sockets). The emulator starts adb itself, so this goes
/// on the emulator's command too; the emu host sets it on every adb it runs.
pub const ADB_MDNS: (&str, &str) = ("ADB_MDNS", "0");

/// The command that boots `avd` with [`args`] (checked, rule 1362), detached from whoever starts it: its own session
/// (macOS) or process group outside any job (Windows), no window, stdin closed, its output into `log` (or nowhere).
/// `ANDROID_SDK_ROOT` and `ANDROID_HOME` name the SDK and `ANDROID_AVD_HOME` the AVD's folder (Flare's own AVDs live
/// outside the person's folder); [`ADB_MDNS`] keeps the adb it starts off the network. `breakaway` false is the second
/// try on Windows when the parent's job forbids it.
pub fn command(
    emulator: &std::path::Path,
    sdk_root: &std::path::Path,
    avd_home: &std::path::Path,
    a: &[String],
    log: Option<std::fs::File>, // sync-ok: a type, not a call
    breakaway: bool,
) -> Result<std::process::Command, String> {
    use std::process::Stdio;
    check(a)?;
    let mut c = std::process::Command::new(emulator); // sync-ok: builds the command; the caller spawns it on a worker
    c.args(a)
        .env("ANDROID_SDK_ROOT", sdk_root)
        .env("ANDROID_HOME", sdk_root)
        .env("ANDROID_AVD_HOME", avd_home)
        .env(ADB_MDNS.0, ADB_MDNS.1)
        .stdin(Stdio::null());
    match log.and_then(|f| f.try_clone().ok().map(|g| (f, g))) {
        Some((out, err)) => {
            c.stdout(out).stderr(err);
        }
        None => {
            c.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let flags = WIN_NO_WINDOW | WIN_NEW_GROUP | if breakaway { WIN_BREAKAWAY } else { 0 };
        c.creation_flags(flags);
    }
    #[cfg(not(windows))]
    let _ = breakaway;
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and touches no memory of this process; it runs between fork and exec.
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    Ok(c)
}

/// A free TCP port on localhost for the emulator's gRPC (the OS picks one; it is free again when this returns).
pub fn free_port() -> Option<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .ok()?
        .local_addr()
        .ok()
        .map(|a| a.port())
}

/// Whether a local port is in use (it cannot be bound).
pub fn port_busy(p: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", p)).is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_is_checked_and_detached() {
        let a = args("Flare_Pixel", 8554, 5554);
        let p = std::path::Path::new;
        let c = command(
            p("/sdk/emulator/emulator"),
            p("/sdk"),
            p("/avds"),
            &a,
            None,
            true,
        )
        .unwrap();
        let got: Vec<String> = c
            .get_args()
            .map(|x| x.to_string_lossy().to_string())
            .collect();
        assert_eq!(got, a);
        let env: Vec<(String, String)> = c
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.map(|v| v.to_string_lossy().to_string())
                        .unwrap_or_default(),
                )
            })
            .collect();
        assert!(env.contains(&("ANDROID_AVD_HOME".into(), "/avds".into())));
        assert!(env.contains(&("ANDROID_SDK_ROOT".into(), "/sdk".into())));
        assert!(
            env.contains(&("ADB_MDNS".into(), "0".into())),
            "rule 1370: the adb the emulator starts stays off the network"
        );
        let mut bad = a.clone();
        bad.retain(|x| x != "-grpc-use-token");
        assert!(
            command(p("/e"), p("/s"), p("/a"), &bad, None, true).is_err(),
            "rule 1362"
        );
        let port = free_port().unwrap();
        assert!(port > 0);
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        assert!(port_busy(held.local_addr().unwrap().port()));
    }

    /// Rule 1362: Flare's command line always has the token and no window, and the check refuses any other.
    #[test]
    fn the_boot_command_always_uses_a_token_and_no_window() {
        let a = args("Flare_Pixel", 8554, 5554);
        assert_eq!(
            a.join(" "),
            "-avd Flare_Pixel -no-window -no-audio -no-boot-anim -gpu host -grpc 8554 -grpc-use-token \
             -idle-grpc-timeout 300 -port 5554"
        );
        assert_eq!(check(&a), Ok(()));
        let without =
            |drop: &str| -> Vec<String> { a.iter().filter(|x| *x != drop).cloned().collect() };
        assert!(
            check(&without("-grpc-use-token"))
                .unwrap_err()
                .contains("open")
        );
        assert!(
            check(&without("-no-window"))
                .unwrap_err()
                .contains("window")
        );
        let mut b = a.clone();
        b[6] = "swiftshader_indirect".into();
        assert!(check(&b).is_err());
        let mut b = a.clone();
        b[11] = "0".into();
        assert!(check(&b).is_err(), "the idle timeout is 300");
        let mut b = a.clone();
        b.push("-grpc-allowlist".into());
        assert!(check(&b).is_err());
        assert!(check(&args("-x", 1, 5554)).is_err(), "a flag is no AVD");
        assert!(
            a.iter().all(|x| !x.contains("token=")),
            "no token on the command line"
        );
    }

    #[test]
    fn the_console_port_is_the_lowest_free_even_one() {
        assert_eq!(console_port(&[], |_| false), Some(5554));
        assert_eq!(console_port(&[5554, 5555], |_| false), Some(5556));
        assert_eq!(
            console_port(&[5555], |_| false),
            Some(5556),
            "an odd one blocks its pair"
        );
        assert_eq!(console_port(&[], |p| p == 5555 || p == 5556), Some(5558));
        assert_eq!(console_port(&[], |_| true), None);
        assert_eq!((CONSOLE_LAST - CONSOLE_FIRST) / 2 + 1, 65);
    }
}
