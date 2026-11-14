//! Settings > Machines > This computer: the Computer sharing switch (machine pairing PR 2, rules 1238 to 1241).
//!
//! Spec `docs/superpowers/specs/2026-11-14-machine-pairing-design.md` ("Setup per OS", revised flow) and the approved
//! mockup (`2026-11-14-machine-pairing-mockup.html`, "Computer sharing" and "System prompts"). Turning the switch on
//! makes this computer reachable over SSH from the user's other computers:
//!
//! - **Windows** (rule 1239): one elevated PowerShell script (one UAC prompt, `Start-Process -Verb RunAs`) installs
//!   the OpenSSH Server capability when it is missing, starts `sshd` and sets it to Automatic, makes sure the sshd
//!   firewall rule exists for the Private profile only, and adds Flare's own rule (its program, Private only) for the
//!   pairing port and the network announcement. Password logins are never turned on; when Flare installed the server
//!   itself, `PasswordAuthentication no` is written into `sshd_config`. The script reports through a temp file.
//! - **macOS** (rule 1240): port 22 answering means Remote Login is on already. Otherwise `systemsetup
//!   -setremotelogin on` runs through osascript's administrator prompt; when macOS refuses (Flare has no Full Disk
//!   Access), the Flare dialog "Turn on Remote Login" offers Open System Settings at Sharing, and Flare watches port 22
//!   and carries on by itself the moment it answers.
//!
//! Turning it off (rule 1241) stops the announcement, removes the keys Flare added (their comment starts
//! `flare-pair:`), and offers to stop the SSH server only when Flare turned it on. The switch stays on across restarts
//! (`flare.machines.sharing`).
//!
//! Every OS call is behind [`SharingOs`], so the tests use a fake: the scripts are tested as text and the elevation as
//! its argv, and never run. Every call runs on a worker (rule 34) and answers with a [`SharingMsg`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use flare_settings::kv::PrefStore;
use flare_settings::machine_prefs as prefs;
use flare_settings::page::machines::{SHARING_CANCEL, SHARING_PRIMARY, SharingInfo};

use crate::appctl::{Post, UiMsg};
use crate::machines_page::MachinesMsg;
use crate::settings_ctl::SettingsMsg;

/// The SSH server's port.
pub const SSH_PORT: u16 = 22;
/// The inbound firewall rule the OpenSSH Server capability makes (Microsoft's name for it).
pub const SSHD_RULE: &str = "OpenSSH-Server-In-TCP";
/// Flare's own inbound firewall rule: its program, on the Private profile only.
pub const FLARE_RULE: &str = "Flare-Pairing-In";
/// The comment that starts every key Flare adds to an `authorized_keys` file (machine pairing PR 3 writes them).
pub const KEY_TAG: &str = "flare-pair:";
/// System Settings at General > Sharing (macOS 13 and later).
pub const SHARING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.Sharing-Settings.extension";
/// The commands osascript runs as root on a Mac.
pub const MAC_ON: &str = "/usr/sbin/systemsetup -setremotelogin on";
pub const MAC_OFF: &str = "/usr/sbin/systemsetup -f -setremotelogin off";

/// The system the switch sets up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Mac,
    /// No setup here (a scene, a test with no fake): turning on says so.
    Other,
}

/// The OS calls of the switch. BLOCKING, every call: a worker's.
pub trait SharingOs: Send + Sync {
    fn platform(&self) -> Platform;
    /// Something answers on this computer's TCP port.
    fn port_open(&self, port: u16) -> bool;
    /// Windows: writes the script `build` makes for a result file, runs it elevated (one UAC prompt) and waits. The
    /// result file's text, or Err when none came back (the prompt was declined, or PowerShell did not start).
    fn run_elevated(&self, build: &dyn Fn(&Path) -> String) -> Result<String, String>;
    /// macOS: runs `command` as root through osascript's administrator prompt. Ok or Err with what it printed.
    fn run_as_admin(&self, command: &str) -> Result<String, String>;
    /// Opens a URL (System Settings' Sharing page).
    fn open_url(&self, url: &str);
    /// Removes the keys Flare added (a `flare-pair:` comment) from the user's own `authorized_keys`. How many.
    fn strip_user_keys(&self) -> Result<usize, String>;
    /// Starts or stops announcing this computer on the network (machine pairing PR 3 announces; nothing yet).
    fn announce(&self, on: bool);
    /// Waits between two looks at the port.
    fn wait(&self, ms: u64);
}

// ---- the pure parts ----------------------------------------------------------------------------------------------

/// A PowerShell single-quoted string: nothing inside is expanded, and a `'` is doubled.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The elevated script that turns sharing on (rule 1239), reporting into `result`. `flare_exe` is the program the
/// pairing rule lets in.
pub fn windows_on_script(result: &Path, flare_exe: &Path) -> String {
    let result = ps_quote(&result.to_string_lossy());
    let exe = ps_quote(&flare_exe.to_string_lossy());
    format!(
        r#"# Flare: turn on Computer sharing (rule 1239). Runs elevated, once per switch.
$ErrorActionPreference = 'Stop'
$result = {result}
$out = New-Object System.Collections.Generic.List[string]
$step = 'install'
$installed = $false
$started = $false
try {{
    $cap = Get-WindowsCapability -Online | Where-Object {{ $_.Name -like 'OpenSSH.Server*' }} | Select-Object -First 1
    if ($null -eq $cap -or $cap.State -ne 'Installed') {{
        Add-WindowsCapability -Online -Name 'OpenSSH.Server~~~~0.0.1.0' | Out-Null
        $installed = $true
    }}
    $step = 'start'
    $svc = Get-Service -Name 'sshd'
    if ($svc.Status -ne 'Running') {{
        Start-Service -Name 'sshd'
        $started = $true
    }}
    Set-Service -Name 'sshd' -StartupType Automatic
    if ($installed) {{
        # A server Flare installed takes keys only. sshd writes its config on its first start.
        $step = 'passwords'
        $cfg = Join-Path $env:ProgramData 'ssh\sshd_config'
        $lines = New-Object System.Collections.Generic.List[string]
        $lines.AddRange([string[]]@(Get-Content -LiteralPath $cfg))
        $at = -1
        for ($i = 0; $i -lt $lines.Count; $i++) {{ if ($lines[$i] -match '^\s*#?\s*PasswordAuthentication\s') {{ $at = $i; break }} }}
        if ($at -ge 0) {{
            $lines[$at] = 'PasswordAuthentication no'
        }} else {{
            $match = -1
            for ($i = 0; $i -lt $lines.Count; $i++) {{ if ($lines[$i] -match '^\s*Match\s') {{ $match = $i; break }} }}
            if ($match -ge 0) {{ $lines.Insert($match, 'PasswordAuthentication no') }} else {{ $lines.Add('PasswordAuthentication no') }}
        }}
        Set-Content -LiteralPath $cfg -Value $lines -Encoding ascii
        Restart-Service -Name 'sshd'
    }}
    $step = 'firewall'
    $rule = Get-NetFirewallRule -Name '{SSHD_RULE}' -ErrorAction SilentlyContinue
    if ($null -eq $rule) {{
        New-NetFirewallRule -Name '{SSHD_RULE}' -DisplayName 'OpenSSH Server (sshd)' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22 -Profile Private | Out-Null
    }} elseif ($installed) {{
        Set-NetFirewallRule -Name '{SSHD_RULE}' -Profile Private -Enabled True
    }}
    Remove-NetFirewallRule -Name '{FLARE_RULE}' -ErrorAction SilentlyContinue
    New-NetFirewallRule -Name '{FLARE_RULE}' -DisplayName 'Flare pairing' -Enabled True -Direction Inbound -Program {exe} -Action Allow -Profile Private | Out-Null
    $out.Add('ok=1')
}} catch {{
    # A server Flare installed never runs with password logins on.
    if ($step -eq 'passwords') {{ Stop-Service -Name 'sshd' -ErrorAction SilentlyContinue }}
    $out.Add('step=' + $step)
    $out.Add('error=' + ($_.Exception.Message -replace '[\r\n]+', ' '))
}}
$out.Add('installed=' + [int]$installed)
$out.Add('started=' + [int]$started)
Set-Content -LiteralPath $result -Value $out -Encoding UTF8
"#
    )
}

/// The elevated script that cleans up when sharing turns off (rule 1241), reporting into `result`: the keys Flare
/// added to `administrators_authorized_keys` (`admin_keys`), the SSH server (`stop_server`), and Flare's firewall rule.
pub fn windows_off_script(result: &Path, stop_server: bool, admin_keys: bool) -> String {
    let result = ps_quote(&result.to_string_lossy());
    let keys = if admin_keys {
        format!(
            r#"    $f = Join-Path $env:ProgramData 'ssh\administrators_authorized_keys'
    if (Test-Path -LiteralPath $f) {{
        $keep = @(Get-Content -LiteralPath $f | Where-Object {{ $_ -notmatch '\s{KEY_TAG}' }})
        Set-Content -LiteralPath $f -Value $keep -Encoding ascii
    }}
"#
        )
    } else {
        String::new()
    };
    let stop = if stop_server {
        "    Stop-Service -Name 'sshd'\n    Set-Service -Name 'sshd' -StartupType Manual\n"
    } else {
        ""
    };
    format!(
        r#"# Flare: turn off Computer sharing (rule 1241). Runs elevated.
$ErrorActionPreference = 'Stop'
$result = {result}
$out = New-Object System.Collections.Generic.List[string]
$step = 'keys'
try {{
{keys}    $step = 'stop'
{stop}    $step = 'firewall'
    Remove-NetFirewallRule -Name '{FLARE_RULE}' -ErrorAction SilentlyContinue
    $out.Add('ok=1')
}} catch {{
    $out.Add('step=' + $step)
    $out.Add('error=' + ($_.Exception.Message -replace '[\r\n]+', ' '))
}}
Set-Content -LiteralPath $result -Value $out -Encoding UTF8
"#
    )
}

/// The elevation call (rule 1239): a hidden PowerShell that starts the script's own PowerShell with `-Verb RunAs`
/// (Windows' UAC prompt) and waits for it. The outer command travels as `-EncodedCommand` (UTF-16LE in base64), so no
/// quoting of the script's path can break it.
pub fn elevation_argv(script: &Path) -> Vec<String> {
    use base64::Engine as _;
    let file = ps_quote(&format!("\"{}\"", script.to_string_lossy()));
    let command = format!(
        "$p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',{file}); exit $p.ExitCode"
    );
    let utf16: Vec<u8> = command.encode_utf16().flat_map(u16::to_le_bytes).collect();
    vec![
        "powershell.exe".into(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-WindowStyle".into(),
        "Hidden".into(),
        "-EncodedCommand".into(),
        base64::engine::general_purpose::STANDARD.encode(utf16),
    ]
}

/// osascript's argv that runs `command` as root behind macOS' administrator prompt (rule 1240).
pub fn osascript_argv(command: &str) -> Vec<String> {
    let quoted = command.replace('\\', "\\\\").replace('"', "\\\"");
    vec![
        "/usr/bin/osascript".into(),
        "-e".into(),
        format!("do shell script \"{quoted}\" with administrator privileges"),
    ]
}

/// What an elevated script wrote into its result file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub ok: bool,
    pub installed: bool,
    pub started: bool,
    /// The step that failed, and Windows' words.
    pub step: String,
    pub error: String,
}

/// Pure: reads a result file's `key=value` lines (a BOM and blank lines are skipped).
pub fn parse_report(text: &str) -> Report {
    let mut r = Report::default();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let Some((k, v)) = line.trim().split_once('=') else {
            continue;
        };
        match k {
            "ok" => r.ok = v == "1",
            "installed" => r.installed = v == "1",
            "started" => r.started = v == "1",
            "step" => r.step = v.to_string(),
            "error" => r.error = v.to_string(),
            _ => {}
        }
    }
    r
}

/// The one line that says which step of the Windows setup failed, in plain words (Windows' own words go to the log).
pub fn step_line(step: &str) -> &'static str {
    match step {
        "install" => {
            "Windows could not add its SSH server. Check the internet connection and try again."
        }
        "start" => "Windows' SSH server would not start.",
        "passwords" => {
            "Flare could not turn off password logins, so it stopped the SSH server again."
        }
        "firewall" => "Windows' firewall would not take the rule for sharing.",
        "keys" => "Flare could not remove the keys it added.",
        "stop" => "Windows' SSH server would not stop.",
        _ => "Computer sharing could not be set up.",
    }
}

/// Pure: osascript's words when the person pressed Cancel at the administrator prompt (error -128).
pub fn mac_declined(out: &str) -> bool {
    let low = out.to_ascii_lowercase();
    out.contains("-128") || low.contains("user canceled") || low.contains("user cancelled")
}

/// Pure: an `authorized_keys` text without the keys Flare added (a `flare-pair:` comment after the key), and how many
/// it dropped. Every other line, comments and blank lines included, stays as it was.
pub fn strip_flare_keys(text: &str) -> (String, usize) {
    let mut kept = String::with_capacity(text.len());
    let mut dropped = 0;
    for line in text.split_inclusive('\n') {
        let flare = line
            .split_whitespace()
            .skip(1)
            .any(|w| w.starts_with(KEY_TAG));
        if flare && !line.trim_start().starts_with('#') {
            dropped += 1;
        } else {
            kept.push_str(line);
        }
    }
    (kept, dropped)
}

// ---- turning on and off, over the OS calls -----------------------------------------------------------------------

/// How turning on went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnResult {
    /// Sharing is on; `by_flare`: Flare installed or started the SSH server.
    On { by_flare: bool },
    /// The person said no at the system's prompt: the switch goes back off, with nothing to say.
    Declined,
    /// macOS would not let Flare turn Remote Login on: the person turns it on in System Settings.
    NeedsSettings,
    /// It failed: the one line that says why.
    Failed(String),
}

/// BLOCKING: turns sharing on for this system (rules 1239 and 1240).
pub fn turn_on(os: &dyn SharingOs, flare_exe: &Path) -> OnResult {
    match os.platform() {
        Platform::Windows => {
            let text = match os.run_elevated(&|result| windows_on_script(result, flare_exe)) {
                Ok(t) => t,
                Err(e) => {
                    log::info!("computer sharing: the elevated setup did not run: {e}");
                    return OnResult::Declined;
                }
            };
            let r = parse_report(&text);
            if r.ok {
                OnResult::On {
                    by_flare: r.installed || r.started,
                }
            } else {
                log::warn!(
                    "computer sharing: the setup failed at {}: {}",
                    r.step,
                    r.error
                );
                OnResult::Failed(step_line(&r.step).to_string())
            }
        }
        Platform::Mac => {
            if os.port_open(SSH_PORT) {
                return OnResult::On { by_flare: false };
            }
            match os.run_as_admin(MAC_ON) {
                Err(out) if mac_declined(&out) => return OnResult::Declined,
                Err(out) | Ok(out) if !out.trim().is_empty() => {
                    log::info!("computer sharing: systemsetup said: {}", out.trim());
                }
                _ => {}
            }
            // systemsetup may say nothing and do nothing (no Full Disk Access): the port says whether it worked.
            for _ in 0..6 {
                if os.port_open(SSH_PORT) {
                    return OnResult::On { by_flare: true };
                }
                os.wait(500); // sync-ok: turn_on runs on a worker
            }
            OnResult::NeedsSettings
        }
        Platform::Other => {
            OnResult::Failed("Computer sharing works on Windows and macOS.".to_string())
        }
    }
}

/// BLOCKING: cleans up when sharing turns off (rule 1241): the user's own keys, then (elevated, only when there is
/// something to do) the admin keys, the SSH server and the firewall rule. Ok, or the one line that says what is left.
pub fn turn_off(os: &dyn SharingOs, stop_server: bool, admin_keys: bool) -> Result<(), String> {
    let mut problem = None;
    match os.strip_user_keys() {
        Ok(n) if n > 0 => log::info!("computer sharing: removed {n} key(s) Flare added"),
        Ok(_) => {}
        Err(e) => {
            log::warn!("computer sharing: the keys stay: {e}");
            problem = Some(step_line("keys").to_string());
        }
    }
    match os.platform() {
        Platform::Windows if stop_server || admin_keys => {
            match os.run_elevated(&|result| windows_off_script(result, stop_server, admin_keys)) {
                Ok(text) => {
                    let r = parse_report(&text);
                    if !r.ok {
                        log::warn!(
                            "computer sharing: the clean up failed at {}: {}",
                            r.step,
                            r.error
                        );
                        problem = Some(step_line(&r.step).to_string());
                    }
                }
                Err(e) => {
                    log::info!("computer sharing: the elevated clean up did not run: {e}");
                    problem = Some(
                        "Windows did not allow the change, so the SSH server and the keys stay as they were."
                            .to_string(),
                    );
                }
            }
        }
        Platform::Mac if stop_server => {
            if let Err(out) = os.run_as_admin(MAC_OFF) {
                log::info!("computer sharing: systemsetup said: {}", out.trim());
            }
            let mut open = os.port_open(SSH_PORT);
            for _ in 0..5 {
                if !open {
                    break;
                }
                os.wait(500); // sync-ok: turn_off runs on a worker
                open = os.port_open(SSH_PORT);
            }
            if open {
                problem = Some(
                    "Remote Login is still on. Turn it off in System Settings, General, Sharing."
                        .to_string(),
                );
            }
        }
        _ => {}
    }
    problem.map_or(Ok(()), Err)
}

// ---- the switch on the UI thread ---------------------------------------------------------------------------------

/// Where the switch is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Off,
    /// The setup runs (the system's prompt may be up).
    TurningOn,
    /// macOS refused: the "Turn on Remote Login" dialog is up and port 22 is watched.
    WaitingForRemoteLogin,
    On,
    /// Turning off asks whether to stop the SSH server Flare turned on.
    AskingStop,
    /// The clean up runs.
    TurningOff,
}

/// The sharing dialog's question (the Dialog look, over the Machines page).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharingAsk {
    /// macOS refused: "Turn on Remote Login", Cancel and Open System Settings (mockup, "System prompts").
    RemoteLogin,
    /// Turning off, when Flare turned the SSH server on: Keep it or Stop it.
    StopServer,
    /// A setup or clean up that failed: its line and OK.
    Notice(String),
}

/// The words of the sharing dialog: its title, its line (with a spinner before it while Flare waits), and its buttons
/// ("" for no left button).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskWords {
    pub title: &'static str,
    pub line: String,
    pub spin: bool,
    pub cancel: &'static str,
    pub primary: &'static str,
}

/// Pure: what the dialog says for a question (rules 1240 and 1241).
pub fn ask_words(ask: &SharingAsk) -> AskWords {
    match ask {
        SharingAsk::RemoteLogin => AskWords {
            title: "Turn on Remote Login",
            line: "Flare continues when it is on".into(),
            spin: true,
            cancel: "Cancel",
            primary: "Open System Settings",
        },
        SharingAsk::StopServer => AskWords {
            title: "Stop the SSH server too?",
            line: "Flare turned it on for Computer sharing. Other apps may use it.".into(),
            spin: false,
            cancel: "Keep it",
            primary: "Stop it",
        },
        SharingAsk::Notice(line) => AskWords {
            title: "Computer sharing",
            line: line.clone(),
            spin: false,
            cancel: "",
            primary: "OK",
        },
    }
}

/// A worker's answer for the switch.
#[derive(Debug)]
pub enum SharingMsg {
    TurnedOn(OnResult),
    /// Port 22 answered while "Turn on Remote Login" was up (the watch's number).
    RemoteLoginSeen(u64),
    /// The clean up ended: whether it stopped the server and cleaned the admin keys, and how it went.
    TurnedOff {
        stopped: bool,
        admin: bool,
        result: Result<(), String>,
    },
}

/// The OS calls of a build with no setup (scenes, tests that never touch the switch).
pub struct NoSharingOs;

impl SharingOs for NoSharingOs {
    fn platform(&self) -> Platform {
        Platform::Other
    }
    fn port_open(&self, _port: u16) -> bool {
        false
    }
    fn run_elevated(&self, _build: &dyn Fn(&Path) -> String) -> Result<String, String> {
        Err("no setup here".into())
    }
    fn run_as_admin(&self, _command: &str) -> Result<String, String> {
        Err("no setup here".into())
    }
    fn open_url(&self, _url: &str) {}
    fn strip_user_keys(&self) -> Result<usize, String> {
        Ok(0)
    }
    fn announce(&self, _on: bool) {}
    fn wait(&self, _ms: u64) {}
}

/// The Computer sharing switch's state on the UI thread.
pub struct Sharing {
    os: Arc<dyn SharingOs>,
    prefs: PrefStore,
    post: Post,
    flare_exe: PathBuf,
    phase: Phase,
    ask: Option<SharingAsk>,
    /// The Remote Login watch running now (0: none). A cancelled watch sees another number and ends.
    watch: Arc<AtomicU64>,
    watches: u64,
}

/// Runs `f` on a named worker; a failure to start is logged.
fn worker(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        log::warn!("could not start {name}: {e}");
    }
}

impl Sharing {
    /// The switch as the settings left it: on after a restart when it was on (and the computer is announced again).
    pub fn new(os: Arc<dyn SharingOs>, prefs: PrefStore, post: Post) -> Self {
        let on = prefs::sharing_on(&prefs);
        if on {
            os.announce(true);
        }
        Self {
            os,
            prefs,
            post,
            flare_exe: std::env::current_exe().unwrap_or_default(),
            phase: if on { Phase::On } else { Phase::Off },
            ask: None,
            watch: Arc::new(AtomicU64::new(0)),
            watches: 0,
        }
    }

    /// The OS calls in use (a page that takes the settings later keeps them).
    pub fn os(&self) -> Arc<dyn SharingOs> {
        Arc::clone(&self.os)
    }

    #[cfg(test)]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn ask(&self) -> Option<&SharingAsk> {
        self.ask.as_ref()
    }

    /// What the switch shows: on from the press until a failure; dimmed while anything runs or a question is open.
    pub fn info(&self) -> SharingInfo {
        SharingInfo {
            on: matches!(
                self.phase,
                Phase::TurningOn | Phase::WaitingForRemoteLogin | Phase::On
            ),
            busy: !matches!(self.phase, Phase::Off | Phase::On) || self.ask.is_some(),
        }
    }

    fn say(post: &Post, m: SharingMsg) {
        post(UiMsg::Settings(SettingsMsg::Machines(
            MachinesMsg::Sharing(m),
        )));
    }

    /// The switch was flipped. A press while something runs does nothing (the switch is deaf then).
    pub fn switch(&mut self, on: bool) {
        if self.ask.is_some() {
            return;
        }
        match (on, self.phase) {
            (true, Phase::Off) => {
                self.phase = Phase::TurningOn;
                let (os, post, exe) = (
                    Arc::clone(&self.os),
                    Arc::clone(&self.post),
                    self.flare_exe.clone(),
                );
                worker("flare-sharing-on", move || {
                    Self::say(&post, SharingMsg::TurnedOn(turn_on(&*os, &exe)));
                });
            }
            (false, Phase::On) => {
                // Off at once: no announcement, and the switch stays off after a restart whatever the clean up says.
                self.os.announce(false);
                prefs::set_sharing_on(&self.prefs, false);
                if prefs::server_by_flare(&self.prefs) {
                    self.phase = Phase::AskingStop;
                    self.ask = Some(SharingAsk::StopServer);
                } else {
                    self.clean_up(false);
                }
            }
            _ => {}
        }
    }

    fn clean_up(&mut self, stop: bool) {
        self.phase = Phase::TurningOff;
        let admin = prefs::admin_keys(&self.prefs);
        let (os, post) = (Arc::clone(&self.os), Arc::clone(&self.post));
        worker("flare-sharing-off", move || {
            let result = turn_off(&*os, stop, admin);
            Self::say(
                &post,
                SharingMsg::TurnedOff {
                    stopped: stop,
                    admin,
                    result,
                },
            );
        });
    }

    fn watch_port(&mut self) {
        self.watches += 1;
        let n = self.watches;
        self.watch.store(n, Ordering::SeqCst);
        let (os, post, watch) = (
            Arc::clone(&self.os),
            Arc::clone(&self.post),
            Arc::clone(&self.watch),
        );
        worker("flare-sharing-watch", move || {
            while watch.load(Ordering::SeqCst) == n {
                if os.port_open(SSH_PORT) {
                    Self::say(&post, SharingMsg::RemoteLoginSeen(n));
                    return;
                }
                os.wait(1000); // sync-ok: the watch is a worker
            }
        });
    }

    fn turned_on(&mut self, by_flare: bool) {
        self.phase = Phase::On;
        self.ask = None;
        prefs::set_sharing_on(&self.prefs, true);
        // Once Flare turned the server on, it stays Flare's until turning off stops it.
        let by_flare = by_flare || prefs::server_by_flare(&self.prefs);
        prefs::set_server_by_flare(&self.prefs, by_flare);
        self.os.announce(true);
    }

    /// A button of the sharing dialog (`SHARING_CANCEL` or `SHARING_PRIMARY`).
    pub fn button(&mut self, id: &str) {
        let Some(ask) = self.ask.clone() else {
            return;
        };
        match (ask, id) {
            (SharingAsk::RemoteLogin, SHARING_PRIMARY) => {
                let os = Arc::clone(&self.os);
                worker("flare-sharing-open", move || {
                    os.open_url(SHARING_SETTINGS_URL)
                });
            }
            (SharingAsk::RemoteLogin, SHARING_CANCEL) => {
                self.watch.store(0, Ordering::SeqCst);
                self.ask = None;
                self.phase = Phase::Off;
            }
            (SharingAsk::StopServer, SHARING_PRIMARY | SHARING_CANCEL) => {
                self.ask = None;
                self.clean_up(id == SHARING_PRIMARY);
            }
            (SharingAsk::Notice(_), SHARING_PRIMARY | SHARING_CANCEL) => self.ask = None,
            _ => {}
        }
    }

    /// Escape, or the Settings tab closing: the dialog's way out (Cancel, Keep it, OK).
    pub fn dismiss(&mut self) {
        if self.ask.is_some() {
            self.button(SHARING_CANCEL);
        }
    }

    /// A worker's answer. True when the page should be drawn again.
    pub fn on_msg(&mut self, m: SharingMsg) -> bool {
        match m {
            SharingMsg::TurnedOn(r) if self.phase == Phase::TurningOn => match r {
                OnResult::On { by_flare } => self.turned_on(by_flare),
                OnResult::Declined => self.phase = Phase::Off,
                OnResult::NeedsSettings => {
                    self.phase = Phase::WaitingForRemoteLogin;
                    self.ask = Some(SharingAsk::RemoteLogin);
                    self.watch_port();
                }
                OnResult::Failed(line) => {
                    self.phase = Phase::Off;
                    self.ask = Some(SharingAsk::Notice(line));
                }
            },
            SharingMsg::RemoteLoginSeen(n)
                if self.phase == Phase::WaitingForRemoteLogin
                    && n == self.watch.load(Ordering::SeqCst) =>
            {
                self.watch.store(0, Ordering::SeqCst);
                // The person turned Remote Login on: the server is theirs, not Flare's.
                self.turned_on(false);
            }
            SharingMsg::TurnedOff {
                stopped,
                admin,
                result,
            } if self.phase == Phase::TurningOff => {
                self.phase = Phase::Off;
                match result {
                    Ok(()) => {
                        if stopped {
                            prefs::set_server_by_flare(&self.prefs, false);
                        }
                        if admin {
                            prefs::set_admin_keys(&self.prefs, false);
                        }
                    }
                    Err(line) => self.ask = Some(SharingAsk::Notice(line)),
                }
            }
            _ => return false,
        }
        true
    }
}

/// The real OS calls.
pub struct RealSharingOs {
    home: PathBuf,
}

impl RealSharingOs {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }
}

/// A command that opens no console window on Windows.
fn quiet(program: &str) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program); // sync-ok: builds the command; its callers run on workers
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

impl SharingOs for RealSharingOs {
    fn platform(&self) -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Other
        }
    }

    fn port_open(&self, port: u16) -> bool {
        let at = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        std::net::TcpStream::connect_timeout(&at, std::time::Duration::from_millis(500)).is_ok()
    }

    fn run_elevated(&self, build: &dyn Fn(&Path) -> String) -> Result<String, String> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let base =
            std::env::temp_dir().join(format!("flare-sharing-{}-{stamp}", std::process::id()));
        let script = base.with_extension("ps1");
        let result = base.with_extension("txt");
        let _ = std::fs::remove_file(&result); // sync-ok: SharingOs calls run on workers
        // A BOM, so Windows PowerShell reads a path with letters beyond ASCII as UTF-8.
        let text = format!("\u{feff}{}", build(&result));
        std::fs::write(&script, text) // sync-ok: SharingOs calls run on workers
            .map_err(|e| format!("the script could not be written: {e}"))?;
        let argv = elevation_argv(&script);
        let status = quiet(&argv[0]).args(&argv[1..]).status(); // sync-ok: waits for the UAC prompt, on a worker
        let answer = std::fs::read_to_string(&result); // sync-ok: SharingOs calls run on workers
        let _ = std::fs::remove_file(&script); // sync-ok: SharingOs calls run on workers
        let _ = std::fs::remove_file(&result); // sync-ok: SharingOs calls run on workers
        match (answer, status) {
            (Ok(text), _) => Ok(text),
            (Err(_), Ok(s)) => Err(format!("no answer from the elevated script ({s})")),
            (Err(_), Err(e)) => Err(format!("PowerShell did not start: {e}")),
        }
    }

    fn run_as_admin(&self, command: &str) -> Result<String, String> {
        let argv = osascript_argv(command);
        let out = quiet(&argv[0])
            .args(&argv[1..])
            .output() // sync-ok: waits for the password prompt, on a worker
            .map_err(|e| format!("osascript did not start: {e}"))?;
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if out.status.success() {
            Ok(said)
        } else {
            Err(said)
        }
    }

    fn open_url(&self, url: &str) {
        if cfg!(target_os = "macos")
            && let Err(e) = quiet("/usr/bin/open").arg(url).status()
        // sync-ok: on a worker
        {
            log::warn!("could not open {url}: {e}");
        }
    }

    fn strip_user_keys(&self) -> Result<usize, String> {
        let file = self.home.join(".ssh").join("authorized_keys");
        let text = match std::fs::read_to_string(&file) {
            // sync-ok: SharingOs calls run on workers
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.to_string()),
        };
        let (kept, n) = strip_flare_keys(&text);
        if n > 0 {
            // Written in place: the file keeps its owner and its mode, which sshd checks.
            std::fs::write(&file, kept).map_err(|e| e.to_string())?; // sync-ok: SharingOs calls run on workers
        }
        Ok(n)
    }

    fn announce(&self, on: bool) {
        // Machine pairing PR 3 starts and stops the mDNS announcement here.
        log::info!("computer sharing: announced {on}");
    }

    fn wait(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms)); // sync-ok: a worker waits between two looks at a port
    }
}

#[cfg(test)]
#[path = "sharing_fake.rs"]
pub(crate) mod fake;

#[cfg(test)]
#[path = "sharing_tests.rs"]
mod tests;
