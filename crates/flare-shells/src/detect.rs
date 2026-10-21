//! Shell detection: a Probe trait (fakeable), the real child-process probe with a timeout,
//! `detect_shells_with` and the once-per-run cache.

use crate::list::{ShellDef, canonical_path, parse_wsl_list, pwsh_path};
use crate::prompt::{PS_FILE_STYLE, PS_PROMPT_HOOK};
use crate::sys::{SysTools, sys_tools};
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct ProbeResult {
    pub ok: bool,
    pub stdout: Vec<u8>,
}

pub trait Probe: Send + Sync {
    fn run(&self, exe: &str, args: &[&str]) -> ProbeResult;
    /// Whether a file exists (the preferred PowerShell 7 path).
    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).is_file() // sync-ok: runs on the detect worker
    }
    /// The canonical form of a path, None when it does not exist.
    fn canonical(&self, path: &str) -> Option<String> {
        canonical_path(path) // sync-ok: runs on the detect worker
    }
}

/// The folder a probe runs in: the folder of its executable (System32), never Flare's own current
/// folder, because `where.exe` and CreateProcess search the current folder first.
pub fn probe_dir(exe: &str) -> Option<&str> {
    exe.rfind(['\\', '/'])
        .map(|i| &exe[..i])
        .filter(|d| !d.is_empty())
}

/// Real child process: no window, stdin and stderr null, stdout piped and read on a thread.
pub struct RealProbe {
    pub timeout: Duration,
}

impl Default for RealProbe {
    fn default() -> Self {
        Self {
            timeout: PROBE_TIMEOUT,
        }
    }
}

impl Probe for RealProbe {
    fn run(&self, exe: &str, args: &[&str]) -> ProbeResult {
        let failed = ProbeResult {
            ok: false,
            stdout: Vec::new(),
        };
        let mut cmd = Command::new(exe);
        if let Some(dir) = probe_dir(exe) {
            cmd.current_dir(dir);
        }
        let Ok(mut child) = cmd
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
        else {
            return failed;
        };
        let reader = child.stdout.take().map(|mut out| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                buf
            })
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
            if start.elapsed() >= self.timeout {
                // Kill the whole tree so a grandchild cannot keep the stdout pipe open.
                let tree = Command::new(format!(
                    "{}\\taskkill.exe",
                    sys_tools().cmd.trim_end_matches("\\cmd.exe")
                ))
                .args(["/PID", &child.id().to_string(), "/T", "/F"])
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(); // sync-ok: runs on a worker, after the probe timed out
                let _ = tree;
                let _ = child.kill();
                let _ = child.wait(); // sync-ok: runs on a worker, reaps the killed probe
                break None;
            }
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: probe poll on a worker thread
        };
        let Some(status) = status else {
            // Killed: a grandchild (ping) may still hold the pipe, so the reader is left to
            // finish on its own thread rather than joined.
            return failed;
        };
        let stdout = reader
            .map(|r| r.join().unwrap_or_default()) // sync-ok: child exited, the pipe is closed
            .unwrap_or_default();
        ProbeResult {
            ok: status.success(),
            stdout,
        }
    }
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes(*p))
        .collect();
    String::from_utf16_lossy(&units).replace('\0', "")
}

/// pwsh, powershell, cmd, then one "wsl-<name>" per distro. No Git Bash probe (decision O21).
pub fn detect_shells_with(probe: &dyn Probe, sys: &SysTools, cwd: &str) -> Vec<ShellDef> {
    let mut list = Vec::new();

    // The standard install wins; only then ask where.exe (run in System32, results kept out of cwd).
    let pwsh = if probe.exists(&sys.pwsh7) {
        Some(sys.pwsh7.clone())
    } else {
        let where_result = probe.run(&sys.where_exe, &["pwsh.exe"]);
        if where_result.ok {
            pwsh_path(&String::from_utf8_lossy(&where_result.stdout), cwd, &|p| {
                probe.canonical(p)
            })
        } else {
            None
        }
    };
    if let Some(exe) = pwsh {
        list.push(ShellDef {
            id: "pwsh".into(),
            name: "PowerShell 7".into(),
            exe,
            args: vec![
                "-NoLogo".into(),
                "-NoExit".into(),
                "-Command".into(),
                format!(
                    "Set-PSReadLineOption -PredictionSource History; try {{ Set-PSReadLineOption -EnableScreenReaderMode:$false }} catch {{}}; {PS_FILE_STYLE}; {PS_PROMPT_HOOK}"
                ),
            ],
        });
    }
    list.push(ShellDef {
        id: "powershell".into(),
        name: "Windows PowerShell".into(),
        exe: sys.powershell.clone(),
        args: vec![
            "-NoLogo".into(),
            "-NoExit".into(),
            "-Command".into(),
            format!("{PS_FILE_STYLE}; {PS_PROMPT_HOOK}"),
        ],
    });
    list.push(ShellDef {
        id: "cmd".into(),
        name: "Command Prompt".into(),
        exe: sys.cmd.clone(),
        args: vec![],
    });

    let wsl = probe.run(&sys.wsl, &["-l", "-q"]);
    if wsl.ok && !wsl.stdout.is_empty() {
        for name in parse_wsl_list(&decode_utf16le(&wsl.stdout)) {
            list.push(ShellDef {
                id: format!("wsl-{name}"),
                name: format!("WSL: {name}"),
                exe: sys.wsl.clone(),
                args: vec!["-d".into(), name, "--cd".into(), ".".into()],
            });
        }
    }
    list
}

/// Detect once through `cell`: a second call returns the stored list without probing.
pub fn get_or_detect<'a>(
    cell: &'a OnceLock<Vec<ShellDef>>,
    probe: &dyn Probe,
    sys: &SysTools,
    cwd: &str,
) -> &'a [ShellDef] {
    cell.get_or_init(|| detect_shells_with(probe, sys, cwd))
}

/// Detected once per run and cached. The FIRST call blocks (up to two probes): worker threads only.
pub fn cached_shells() -> &'static [ShellDef] {
    // sync-ok: runs only on the spawn worker, cached for the run
    static CELL: OnceLock<Vec<ShellDef>> = OnceLock::new();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    get_or_detect(&CELL, &RealProbe::default(), sys_tools(), &cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::sys_tools_for;
    use std::sync::Mutex;

    struct Fake {
        where_out: ProbeResult,
        wsl_out: ProbeResult,
        calls: Mutex<Vec<(String, Vec<String>)>>,
        sys: SysTools,
        pwsh7_exists: bool,
    }

    impl Fake {
        fn new(where_ok: bool, where_out: &str, wsl_ok: bool, wsl_bytes: Vec<u8>) -> Self {
            Fake {
                where_out: ProbeResult {
                    ok: where_ok,
                    stdout: where_out.as_bytes().to_vec(),
                },
                wsl_out: ProbeResult {
                    ok: wsl_ok,
                    stdout: wsl_bytes,
                },
                calls: Mutex::new(Vec::new()),
                sys: sys_tools_for("C:\\Windows"),
                pwsh7_exists: false,
            }
        }
    }

    impl Probe for Fake {
        fn exists(&self, path: &str) -> bool {
            self.pwsh7_exists && path == self.sys.pwsh7
        }
        fn canonical(&self, path: &str) -> Option<String> {
            Some(path.replace('/', "\\").replace("DOWNLO~1", "Downloads"))
        }
        fn run(&self, exe: &str, args: &[&str]) -> ProbeResult {
            self.calls.lock().unwrap().push((
                exe.to_string(),
                args.iter().map(|s| s.to_string()).collect(),
            ));
            let r = if exe == self.sys.where_exe {
                &self.where_out
            } else {
                &self.wsl_out
            };
            ProbeResult {
                ok: r.ok,
                stdout: r.stdout.clone(),
            }
        }
    }

    const PWSH: &str = "C:\\Program Files\\PowerShell\\7\\pwsh.exe";

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    fn ids(l: &[ShellDef]) -> Vec<&str> {
        l.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn lists_pwsh_powershell_cmd_in_that_order() {
        let f = Fake::new(true, &format!("{PWSH}\r\n"), false, vec![]);
        let l = detect_shells_with(&f, &f.sys, "C:\\x");
        assert_eq!(ids(&l), ["pwsh", "powershell", "cmd"]);
        assert_eq!(l[0].exe, PWSH);
        assert_eq!(l[1].exe, f.sys.powershell);
        assert_eq!(l[2].exe, f.sys.cmd);
    }

    #[test]
    fn the_program_files_pwsh_wins_and_where_is_not_asked() {
        let mut f = Fake::new(true, "C:\\Downloads\\evil\\pwsh.exe\n", false, vec![]);
        f.pwsh7_exists = true;
        let l = detect_shells_with(&f, &f.sys, "C:\\Downloads\\evil");
        assert_eq!(l[0].exe, f.sys.pwsh7);
        assert!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .all(|(e, _)| *e != f.sys.where_exe)
        );
    }

    #[test]
    fn a_pwsh_planted_in_the_current_folder_is_never_listed() {
        let f = Fake::new(true, "C:\\DOWNLO~1\\evil\\pwsh.exe\n", false, vec![]);
        let l = detect_shells_with(&f, &f.sys, "C:\\Downloads\\evil");
        assert_eq!(ids(&l), ["powershell", "cmd"]);
    }

    #[test]
    fn probes_run_in_the_tool_folder_never_the_current_one() {
        assert_eq!(
            probe_dir("C:\\Windows\\System32\\where.exe"),
            Some("C:\\Windows\\System32")
        );
        assert_eq!(probe_dir("where.exe"), None);
    }

    #[test]
    fn no_pwsh_means_no_pwsh_entry() {
        let f = Fake::new(true, "", false, vec![]);
        let l = detect_shells_with(&f, &f.sys, "C:\\x");
        assert_eq!(ids(&l), ["powershell", "cmd"]);
    }

    #[test]
    fn wsl_distros_are_decoded_from_utf16le() {
        let f = Fake::new(false, "", true, utf16("Ubuntu\r\nDebian\r\n"));
        let l = detect_shells_with(&f, &f.sys, "C:\\x");
        let n = l.len();
        assert_eq!(ids(&l)[n - 2..], ["wsl-Ubuntu", "wsl-Debian"]);
        assert_eq!(
            l[n - 2],
            ShellDef {
                id: "wsl-Ubuntu".into(),
                name: "WSL: Ubuntu".into(),
                exe: f.sys.wsl.clone(),
                args: vec!["-d".into(), "Ubuntu".into(), "--cd".into(), ".".into()],
            }
        );
    }

    #[test]
    fn a_failed_wsl_probe_adds_nothing() {
        let f = Fake::new(false, "", false, utf16("Ubuntu\r\n"));
        let l = detect_shells_with(&f, &f.sys, "C:\\x");
        assert!(l.iter().all(|s| !s.id.starts_with("wsl-")));
    }

    #[test]
    fn bootstrap_args_are_exact() {
        let f = Fake::new(true, PWSH, false, vec![]);
        let l = detect_shells_with(&f, &f.sys, "C:\\x");
        let pwsh = &l[0];
        assert_eq!(
            pwsh.args[3],
            format!(
                "Set-PSReadLineOption -PredictionSource History; try {{ Set-PSReadLineOption -EnableScreenReaderMode:$false }} catch {{}}; {PS_FILE_STYLE}; {PS_PROMPT_HOOK}"
            )
        );
        let ps = &l[1];
        assert_eq!(ps.args[3], format!("{PS_FILE_STYLE}; {PS_PROMPT_HOOK}"));
        assert!(l[2].args.is_empty());
        for s in [pwsh, ps] {
            assert_eq!(s.args[..3], ["-NoLogo", "-NoExit", "-Command"]);
        }
    }

    #[test]
    fn probes_exactly_where_pwsh_and_wsl_list_and_never_git_bash() {
        let f = Fake::new(true, PWSH, true, utf16("Ubuntu\r\n"));
        detect_shells_with(&f, &f.sys, "C:\\x");
        let calls = f.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                (f.sys.where_exe.clone(), vec!["pwsh.exe".to_string()]),
                (f.sys.wsl.clone(), vec!["-l".to_string(), "-q".to_string()]),
            ]
        );
    }

    #[test]
    fn the_probe_commands_are_full_system32_paths() {
        let f = Fake::new(true, PWSH, true, utf16("Ubuntu\r\n"));
        detect_shells_with(&f, &f.sys, "C:\\x");
        for (exe, _) in f.calls.lock().unwrap().iter() {
            assert_eq!(exe.as_bytes()[1], b':', "{exe}");
            assert_eq!(exe.as_bytes()[2], b'\\', "{exe}");
            assert!(exe.to_lowercase().contains("\\system32\\"), "{exe}");
        }
    }

    #[test]
    fn real_probe_times_out_and_kills_the_child() {
        let sys = sys_tools();
        let p = RealProbe {
            timeout: Duration::from_millis(300),
        };
        let t = Instant::now();
        let r = p.run(&sys.cmd, &["/c", "ping -n 6 127.0.0.1 >nul"]);
        assert!(!r.ok);
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }

    #[test]
    fn real_probe_reads_stdout() {
        let sys = sys_tools();
        let r = RealProbe::default().run(&sys.cmd, &["/c", "echo flare"]);
        assert!(r.ok);
        assert!(String::from_utf8_lossy(&r.stdout).contains("flare"));
    }

    #[test]
    fn the_cache_asks_the_probes_once() {
        let f = Fake::new(true, PWSH, true, utf16("Ubuntu\r\n"));
        let cell = OnceLock::new();
        let a = get_or_detect(&cell, &f, &f.sys, "C:\\x").to_vec();
        let b = get_or_detect(&cell, &f, &f.sys, "C:\\x").to_vec();
        assert_eq!(f.calls.lock().unwrap().len(), 2);
        assert_eq!(a, b);
    }
}
