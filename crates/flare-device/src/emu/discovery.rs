//! The running emulators, from their discovery files (spike S5, 2026-11-22).
//!
//! Each running emulator writes `pid_<pid>.ini` (some versions `pid_<pid>_info.ini`) in the discovery folder
//! ([`super::Places::discovery_dir`]) with `grpc.port`, `grpc.token`, `port.serial`, `port.adb`, `avd.name`,
//! `avd.dir` and `emulator.version`. An emulator started with `-grpc-use-token` has a token there; one started with a
//! plain `-grpc <port>` has a port and no token (an open port, which Flare never uses: rule 1362); one started with no
//! `-grpc` at all writes no file.
//!
//! The token lives in [`Token`], whose `Debug` and `Display` never show it (rule 1362): it goes only into the gRPC
//! header, never into a log, a hello fact, an error or an answer.

use std::path::{Path, PathBuf};

/// An emulator's gRPC token. Never printed (rule 1362).
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// None for an empty token.
    pub fn new(s: &str) -> Option<Token> {
        let s = s.trim();
        (!s.is_empty()).then(|| Token(s.to_string()))
    }

    /// The `authorization` header's value.
    pub fn bearer(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(hidden)")
    }
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("(hidden)")
    }
}

/// How Flare may reach a running emulator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// gRPC on localhost with a token (`-grpc-use-token`, as Flare and Android Studio start it).
    Token { port: u16, token: Token },
    /// A gRPC port with no token: started with a plain `-grpc`. Flare does not use an open port (rule 1362).
    OpenPort,
    /// No gRPC at all.
    None,
}

/// The line the panel shows for an emulator Flare cannot reach (spec state 6).
pub const NO_ACCESS: &str =
    "This emulator was started without access for other apps. Start it from Flare.";

/// One running emulator, as its discovery file says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Running {
    /// The pid in the file's name.
    pub pid: u32,
    pub grpc_port: Option<u16>,
    pub token: Option<Token>,
    /// `port.serial`, the console port (the adb serial is `emulator-<port>`).
    pub console_port: Option<u16>,
    pub adb_port: Option<u16>,
    pub avd: Option<String>,
    pub avd_dir: Option<PathBuf>,
    pub version: Option<String>,
}

impl Running {
    pub fn access(&self) -> Access {
        match (self.grpc_port, &self.token) {
            (Some(port), Some(token)) => Access::Token {
                port,
                token: token.clone(),
            },
            (Some(_), None) => Access::OpenPort,
            _ => Access::None,
        }
    }

    /// adb's serial for this emulator.
    pub fn serial(&self) -> Option<String> {
        Some(format!("emulator-{}", self.console_port?))
    }
}

/// The pid in a discovery file's name: `pid_<pid>.ini` or `pid_<pid>_info.ini`. None for any other name.
pub fn pid_of(file_name: &str) -> Option<u32> {
    let rest = file_name.strip_prefix("pid_")?.strip_suffix(".ini")?;
    let digits = rest.strip_suffix("_info").unwrap_or(rest);
    (!digits.is_empty() && digits.len() <= 10 && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// A discovery file's contents.
pub fn parse(pid: u32, text: &str) -> Running {
    let m = super::parse_ini(text);
    let port = |k: &str| {
        m.get(k)
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|p| *p > 0)
    };
    let text = |k: &str| m.get(k).filter(|v| !v.is_empty()).cloned();
    Running {
        pid,
        grpc_port: port("grpc.port"),
        token: m.get("grpc.token").and_then(|t| Token::new(t)),
        console_port: port("port.serial"),
        adb_port: port("port.adb"),
        // `avd.id` is the AVD's name (what `-avd` takes); `avd.name` is its display name (spike, 37.2.13).
        avd: text("avd.id").or_else(|| text("avd.name")),
        avd_dir: text("avd.dir").map(PathBuf::from),
        version: text("emulator.version"),
    }
}

/// BLOCKING (reads the folder): every running emulator's facts, by pid. A file that cannot be read is skipped.
pub fn read_all(dir: &Path) -> Vec<Running> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        // sync-ok: called by the emu host and on Flare's workers
        return Vec::new();
    };
    let mut out: Vec<Running> = rd
        .flatten()
        .filter_map(|e| {
            let pid = pid_of(&e.file_name().to_string_lossy())?;
            let text = std::fs::read_to_string(e.path()).ok()?; // sync-ok: the emu host and Flare's workers
            Some(parse(pid, &text))
        })
        .collect();
    out.sort_by_key(|r| r.pid);
    out
}

/// The running emulator of `avd`, when one is.
pub fn of_avd<'a>(list: &'a [Running], avd: &str) -> Option<&'a Running> {
    list.iter().find(|r| r.avd.as_deref() == Some(avd))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What emulator 37.2.13 wrote on Windows with `-grpc-use-token` (2026-11-22, PR A2's by-hand run; the token and
    /// the user's paths replaced). `avd.id` is the AVD's name; `avd.name` is its display name.
    pub(crate) const FIXTURE: &str = "\
launcher.dir=C:\\Users\\a\\AppData\\Local\\Android\\Sdk\\emulator
emulator.build=16740729
port.serial=5554
emulator.version=37.2.13.0
avd.id=flare-a2-test
port.adb=5555
avd.name=Flare A2 Test
avd.dir=C:\\Users\\a\\avd\\flare-a2-test.avd
cmdline=\"C:\\\\Users\\\\a\\\\AppData\\\\Local\\\\Android\\\\Sdk\\\\emulator\\\\qemu\\\\windows-x86_64\\\\qemu-system-x86_64-headless.exe\" \"-avd\" \"flare-a2-test\" \"-no-window\" \"-grpc-use-token\"
grpc.token=c2VjcmV0LXRva2VuLWZvci10ZXN0cw==
grpc.port=8601
grpc.allowlist=C:\\Users\\a\\AppData\\Local\\Android\\Sdk\\emulator\\lib\\emulator_access.json
";

    #[test]
    fn a_discovery_file_gives_the_port_the_serial_and_a_hidden_token() {
        assert_eq!(pid_of("pid_4242.ini"), Some(4242));
        assert_eq!(pid_of("pid_4242_info.ini"), Some(4242));
        for bad in [
            "pid_.ini",
            "pid_x.ini",
            "pid_1.txt",
            "4242.ini",
            "pid_1_2.ini",
            "pid_99999999999.ini",
        ] {
            assert_eq!(pid_of(bad), None, "{bad}");
        }
        let r = parse(4242, FIXTURE);
        assert_eq!(r.grpc_port, Some(8601));
        assert_eq!(r.console_port, Some(5554));
        assert_eq!(r.adb_port, Some(5555));
        assert_eq!(
            r.avd.as_deref(),
            Some("flare-a2-test"),
            "the id, not the display name"
        );
        assert_eq!(r.version.as_deref(), Some("37.2.13.0"));
        assert_eq!(r.serial().as_deref(), Some("emulator-5554"));
        let Access::Token { port, token } = r.access() else {
            panic!("{:?}", r.access())
        };
        assert_eq!(port, 8601);
        assert_eq!(token.bearer(), "Bearer c2VjcmV0LXRva2VuLWZvci10ZXN0cw==");
        // Rule 1362: the token never shows in a debug print or a display.
        let shown = format!("{r:?} {token} {token:?} {:?}", r.access());
        assert!(!shown.contains("c2VjcmV0"), "{shown}");
        assert!(shown.contains("hidden"));
    }

    /// Rule 1362: an emulator with an open port (plain `-grpc`) or none is not one Flare drives.
    #[test]
    fn an_emulator_without_a_token_has_no_access() {
        let open = parse(1, "grpc.port=8554\navd.name=A\n");
        assert_eq!(open.access(), Access::OpenPort);
        let blank = parse(1, "grpc.port=8554\ngrpc.token= \navd.name=A\n");
        assert_eq!(blank.access(), Access::OpenPort, "an empty token is none");
        assert_eq!(parse(1, "avd.name=A\n").access(), Access::None);
        assert_eq!(
            parse(1, "grpc.port=0\ngrpc.token=t\n").access(),
            Access::None
        );
        assert!(NO_ACCESS.starts_with("This emulator was started without access"));
    }

    #[test]
    fn the_folder_is_read_for_every_running_emulator() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("pid_20.ini"), FIXTURE).unwrap();
        std::fs::write(d.path().join("pid_10_info.ini"), "avd.name=B\n").unwrap();
        std::fs::write(d.path().join("other.ini"), "avd.name=C\n").unwrap();
        std::fs::create_dir(d.path().join("flare")).unwrap();
        let all = read_all(d.path());
        assert_eq!(all.iter().map(|r| r.pid).collect::<Vec<_>>(), [10, 20]);
        assert_eq!(of_avd(&all, "flare-a2-test").map(|r| r.pid), Some(20));
        assert_eq!(
            of_avd(&all, "Flare A2 Test"),
            None,
            "never by the display name"
        );
        assert_eq!(
            parse(1, "avd.name=Old\n").avd.as_deref(),
            Some("Old"),
            "no id: the name"
        );
        assert_eq!(of_avd(&all, "C"), None);
        assert!(read_all(&d.path().join("missing")).is_empty());
    }
}
