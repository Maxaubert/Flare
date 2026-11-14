//! Settings > Machines > This computer: the Computer sharing switch (machine pairing PR 2, rules 1252 to 1255).
//!
//! Spec `docs/superpowers/specs/2026-11-14-machine-pairing-design.md` ("Setup per OS", revised flow) and the approved
//! mockup (`2026-11-14-machine-pairing-mockup.html`, "Computer sharing" and "System prompts"). Turning the switch on
//! makes this computer reachable over SSH from the user's other computers:
//!
//! - **Windows** (rule 1253): one elevated PowerShell script (one UAC prompt, `Start-Process -Verb RunAs`) installs
//!   the OpenSSH Server capability when it is missing, starts `sshd` and sets it to Automatic, makes sure the sshd
//!   firewall rule exists for the Private profile only (a server Flare starts has its rule set to Private before it
//!   starts), puts the server back as it was when a step fails, and adds Flare's own two rules (its program, Private
//!   profile, local subnet only: TCP for the pairing port, UDP 5353 for the network announcement). A server Flare
//!   starts (installed now, or installed but stopped) takes keys only: `PasswordAuthentication no` and
//!   `ChallengeResponseAuthentication no` go at the top of `sshd_config` before it starts, with a backup, and the file
//!   is put back when a step fails. A server that already ran keeps its config. The script travels inline
//!   (`-EncodedCommand`), so no script file exists; it reports through a result file in a folder Flare made for this
//!   run, and Flare reads that file as untrusted text ([`parse_report`]).
//! - **macOS** (rule 1254): port 22 answering means Remote Login is on already. Otherwise `systemsetup
//!   -setremotelogin on` runs through osascript's administrator prompt; when macOS refuses (Flare has no Full Disk
//!   Access), the Flare dialog "Turn on Remote Login" offers Open System Settings at Sharing, and Flare watches port 22
//!   and carries on by itself the moment it answers.
//!
//! Turning it off (rule 1255) stops the announcement, removes the keys Flare added (their comment starts
//! `flare-pair:`), removes Flare's firewall rules (`flare.machines.firewallRule`), and offers to stop the SSH server
//! only when Flare turned it on. The switch stays on across restarts (`flare.machines.sharing`).
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
/// Flare's own inbound firewall rule for the pairing port: its program, TCP, the Private profile and the local subnet
/// only. The pairing port is picked at random (machine pairing PR 3), so the rule names the program, not a port.
pub const FLARE_RULE: &str = "Flare-Pairing-In";
/// Flare's own inbound firewall rule for the network announcement: its program, UDP 5353 (mDNS), Private, local subnet.
pub const FLARE_MDNS_RULE: &str = "Flare-Pairing-mDNS-In";
/// The mDNS port the announcement uses.
pub const MDNS_PORT: u16 = 5353;
/// The longest command line Windows takes (`CreateProcess`, in UTF-16 units, the final NUL included).
pub const COMMAND_LINE_MAX: usize = 32_767;
/// The most of a result file Flare reads: a real one is a few short lines.
pub const REPORT_BYTES_MAX: usize = 4096;
/// The most of Windows' error words Flare keeps from a result file (they go to the log only).
pub const REPORT_ERROR_MAX: usize = 300;
/// The line that starts Flare's lines at the top of `sshd_config`, so a second run replaces them instead of adding more.
pub const SSHD_MARK: &str = "# Flare Computer sharing: keys only";
/// The rights sshd gives `%ProgramData%\ssh` when it makes it: SYSTEM and Administrators full, signed-in users read.
pub const SSHD_DIR_SDDL: &str = "O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;AU)";
/// The settings a server Flare starts gets, at the top of `sshd_config` (sshd keeps the first value it reads, so they
/// win over any later line and any Match block). `ChallengeResponseAuthentication` is the name every Windows OpenSSH
/// knows; on OpenSSH 8.7 and later it is the same setting as `KbdInteractiveAuthentication`, which an older sshd
/// would refuse to start with.
pub const SSHD_KEYS_ONLY: [&str; 2] = [
    "PasswordAuthentication no",
    "ChallengeResponseAuthentication no",
];
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
    /// Windows: runs the script `build` makes for a result file elevated (one UAC prompt), inline with no script
    /// file, and waits. The result file's text (untrusted: [`parse_report`] reads it), or Err when none came back (the
    /// prompt was declined, or PowerShell did not start).
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

/// The end of every elevated script: it writes its lines into `$result` only when the folder Flare made for this run
/// is a plain folder (not a link to somewhere else) and holds no file of that name yet.
const WRITE_RESULT: &str = r#"$folder = [System.IO.Path]::GetDirectoryName($result)
$item = Get-Item -LiteralPath $folder -Force -ErrorAction SilentlyContinue
if ($null -ne $item -and $item.PSIsContainer -and -not ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -and -not (Test-Path -LiteralPath $result)) {
    Set-Content -LiteralPath $result -Value $out -Encoding UTF8
}
"#;

/// The elevated script that turns sharing on (rule 1253), reporting into `result`. `flare_exe` is the program
/// Flare's firewall rules let in.
pub fn windows_on_script(result: &Path, flare_exe: &Path) -> String {
    let result = ps_quote(&result.to_string_lossy());
    let exe = ps_quote(&flare_exe.to_string_lossy());
    let mark = ps_quote(SSHD_MARK);
    let keys_only = SSHD_KEYS_ONLY
        .iter()
        .map(|l| ps_quote(l))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"# Flare: turn on Computer sharing (rule 1253). Runs elevated, once per switch.
$ErrorActionPreference = 'Stop'
$result = {result}
$out = New-Object System.Collections.Generic.List[string]
$step = 'install'
$installed = $false
$started = $false
$rule = $false
$startType = $null
$cfg = $null
$backup = $null
$hadCfg = $true
try {{
    $cap = Get-WindowsCapability -Online | Where-Object {{ $_.Name -like 'OpenSSH.Server*' }} | Select-Object -First 1
    if ($null -eq $cap -or $cap.State -ne 'Installed') {{
        Add-WindowsCapability -Online -Name 'OpenSSH.Server~~~~0.0.1.0' | Out-Null
        $installed = $true
    }}
    $step = 'start'
    $svc = Get-Service -Name 'sshd'
    $startType = [string]$svc.StartType
    $sshdRule = Get-NetFirewallRule -Name '{SSHD_RULE}' -ErrorAction SilentlyContinue
    if ($svc.Status -ne 'Running') {{
        # A server Flare starts (installed now, or one the person had but left stopped) opens on the Private profile
        # only, before it starts: its rule may name every profile.
        $step = 'firewall'
        if ($null -ne $sshdRule) {{ Set-NetFirewallRule -Name '{SSHD_RULE}' -Profile Private -Enabled True }}
        # A server Flare starts takes keys only, from its first start. Flare's lines go at the top of sshd_config,
        # before any Match block (sshd keeps the first value it reads); a backup is kept, and the other lines stay.
        $step = 'passwords'
        $dir = Join-Path $env:ProgramData 'ssh'
        $cfg = Join-Path $dir 'sshd_config'
        if (-not (Test-Path -LiteralPath $cfg)) {{
            # A server never started has no config yet: it starts from Windows' default, as sshd itself would.
            $hadCfg = $false
            if (-not (Test-Path -LiteralPath $dir)) {{
                # The folder gets the rights sshd gives it: SYSTEM and Administrators write, signed-in users read.
                $acl = New-Object System.Security.AccessControl.DirectorySecurity
                $acl.SetSecurityDescriptorSddlForm('{SSHD_DIR_SDDL}')
                [System.IO.Directory]::CreateDirectory($dir, $acl) | Out-Null
            }}
            Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\OpenSSH\sshd_config_default') -Destination $cfg
        }}
        $backup = $cfg + '.flare-backup'
        Copy-Item -LiteralPath $cfg -Destination $backup -Force
        $flare = @({mark}, {keys_only})
        $keep = New-Object System.Collections.Generic.List[string]
        $old = [System.IO.File]::ReadAllLines($cfg)
        for ($i = 0; $i -lt $old.Count; $i++) {{
            if ($old[$i] -eq $flare[0]) {{
                # Flare's lines from an earlier run go; the person's own lines after them stay.
                $n = 1
                while ($n -lt $flare.Count -and $i + 1 -lt $old.Count -and $old[$i + 1] -eq $flare[$n]) {{ $i++; $n++ }}
                continue
            }}
            $keep.Add($old[$i])
        }}
        [System.IO.File]::WriteAllLines($cfg, [string[]]($flare + $keep))
        $step = 'start'
        Start-Service -Name 'sshd'
        $started = $true
    }}
    Set-Service -Name 'sshd' -StartupType Automatic
    $step = 'firewall'
    if ($null -eq $sshdRule) {{
        New-NetFirewallRule -Name '{SSHD_RULE}' -DisplayName 'OpenSSH Server (sshd)' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort {SSH_PORT} -Profile Private | Out-Null
    }}
    Remove-NetFirewallRule -Name '{FLARE_RULE}' -ErrorAction SilentlyContinue
    Remove-NetFirewallRule -Name '{FLARE_MDNS_RULE}' -ErrorAction SilentlyContinue
    $rule = $true
    New-NetFirewallRule -Name '{FLARE_RULE}' -DisplayName 'Flare pairing' -Enabled True -Direction Inbound -Program {exe} -Protocol TCP -RemoteAddress LocalSubnet -Action Allow -Profile Private | Out-Null
    New-NetFirewallRule -Name '{FLARE_MDNS_RULE}' -DisplayName 'Flare pairing (mDNS)' -Enabled True -Direction Inbound -Program {exe} -Protocol UDP -LocalPort {MDNS_PORT} -RemoteAddress LocalSubnet -Action Allow -Profile Private | Out-Null
    $out.Add('ok=1')
}} catch {{
    # A failed setup leaves the SSH server as it found it: a server Flare started is stopped again, its config and
    # its start type are put back, and Flare's own rules go. The switch goes back off.
    $out.Add('step=' + $step)
    $out.Add('error=' + ($_.Exception.Message -replace '[\r\n]+', ' '))
    if ($started) {{ Stop-Service -Name 'sshd' -ErrorAction SilentlyContinue }}
    if ($null -ne $backup -and (Test-Path -LiteralPath $backup)) {{ Copy-Item -LiteralPath $backup -Destination $cfg -Force -ErrorAction SilentlyContinue }}
    if (-not $hadCfg -and $null -ne $cfg) {{ Remove-Item -LiteralPath $cfg -Force -ErrorAction SilentlyContinue }}
    if ($null -ne $startType -and $startType -ne 'Automatic') {{ Set-Service -Name 'sshd' -StartupType $startType -ErrorAction SilentlyContinue }}
    if ($rule) {{
        Remove-NetFirewallRule -Name '{FLARE_RULE}' -ErrorAction SilentlyContinue
        Remove-NetFirewallRule -Name '{FLARE_MDNS_RULE}' -ErrorAction SilentlyContinue
        $rule = $false
    }}
}}
$out.Add('installed=' + [int]$installed)
$out.Add('started=' + [int]$started)
$out.Add('rule=' + [int]$rule)
{WRITE_RESULT}"#
    )
}

/// The elevated script that cleans up when sharing turns off (rule 1255), reporting into `result`: the keys Flare
/// added to `administrators_authorized_keys` (`admin_keys`), the SSH server (`stop_server`), and always Flare's own
/// firewall rules.
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
        r#"# Flare: turn off Computer sharing (rule 1255). Runs elevated.
$ErrorActionPreference = 'Stop'
$result = {result}
$out = New-Object System.Collections.Generic.List[string]
$step = 'keys'
try {{
{keys}    $step = 'stop'
{stop}    $step = 'firewall'
    Remove-NetFirewallRule -Name '{FLARE_RULE}' -ErrorAction SilentlyContinue
    Remove-NetFirewallRule -Name '{FLARE_MDNS_RULE}' -ErrorAction SilentlyContinue
    $out.Add('ok=1')
}} catch {{
    $out.Add('step=' + $step)
    $out.Add('error=' + ($_.Exception.Message -replace '[\r\n]+', ' '))
}}
{WRITE_RESULT}"#
    )
}

/// Pure: a script as it travels: no comment lines, no blank lines, no indent (none of Flare's scripts has a
/// here-string, so the lines mean the same), to keep the command line short.
pub fn ps_compact(script: &str) -> String {
    script
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pure: a PowerShell `-EncodedCommand` value: the text in UTF-16LE, in base64.
pub fn ps_encode(text: &str) -> String {
    use base64::Engine as _;
    let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

/// Pure: how long Windows' command line for `argv` is, in UTF-16 units with the final NUL: each item quoted, with a
/// space between (an upper bound; none of Flare's items holds a `"` or a `\` before a quote).
pub fn command_line_len(argv: &[String]) -> usize {
    argv.iter()
        .map(|a| a.encode_utf16().count() + 3)
        .sum::<usize>()
}

/// The elevation call (rule 1253): a hidden PowerShell that starts an elevated PowerShell with `-Verb RunAs`
/// (Windows' UAC prompt) and waits for it. The script itself travels inline, as the elevated PowerShell's
/// `-EncodedCommand` (UTF-16LE in base64), so no script file exists that another program could swap before the prompt
/// is answered. The outer command holds only fixed words and base64, so it travels as plain `-Command`. Err when the
/// command line would be longer than Windows takes (the tests hold every script well under it).
pub fn elevation_argv(script: &str) -> Result<Vec<String>, String> {
    let inner = ps_encode(&ps_compact(script));
    let command = format!(
        "$p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList @('-NoProfile','-NonInteractive','-WindowStyle','Hidden','-EncodedCommand','{inner}'); \
         exit $p.ExitCode"
    );
    let argv: Vec<String> = vec![
        "powershell.exe".into(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-WindowStyle".into(),
        "Hidden".into(),
        "-Command".into(),
        command,
    ];
    let len = command_line_len(&argv);
    if len > COMMAND_LINE_MAX {
        return Err(format!(
            "the elevated command is too long ({len} characters)"
        ));
    }
    Ok(argv)
}

/// osascript's argv that runs `command` as root behind macOS' administrator prompt (rule 1254).
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
    /// Flare's own firewall rules are in place (the on script made them).
    pub rule: bool,
    /// The step that failed, and Windows' words.
    pub step: String,
    pub error: String,
}

/// The steps an elevated script names when it fails.
pub const STEPS: [&str; 6] = ["install", "start", "passwords", "firewall", "keys", "stop"];

/// Pure: reads a result file strictly, as untrusted text (another program of the same user could have written it):
/// at most [`REPORT_BYTES_MAX`] bytes, only `key=value` lines with known keys, each once, flags `0` or `1`, a known
/// step, and either `ok=1` or a step that failed. A BOM, `\r` and blank lines are allowed. Windows' error words are
/// kept to [`REPORT_ERROR_MAX`] characters without control characters. Err says what was wrong.
pub fn parse_report(text: &str) -> Result<Report, String> {
    if text.len() > REPORT_BYTES_MAX {
        return Err(format!("the result is too long ({} bytes)", text.len()));
    }
    let mut r = Report::default();
    let mut seen: Vec<&str> = Vec::new();
    for line in text.strip_prefix('\u{feff}').unwrap_or(text).lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err("a line with no value".into());
        };
        if seen.contains(&k) {
            return Err(format!("{k} twice"));
        }
        let flag = |v: &str| match v {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(format!("{k} is not 0 or 1")),
        };
        match k {
            "ok" => r.ok = flag(v)?,
            "installed" => r.installed = flag(v)?,
            "started" => r.started = flag(v)?,
            "rule" => r.rule = flag(v)?,
            "step" if STEPS.contains(&v) => r.step = v.to_string(),
            "step" => return Err("an unknown step".into()),
            "error" => {
                r.error = v
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(REPORT_ERROR_MAX)
                    .collect()
            }
            _ => return Err("an unknown key".into()),
        }
        seen.push(k);
    }
    match (r.ok, r.step.is_empty()) {
        (true, true) | (false, false) => Ok(r),
        (true, false) => Err("ok and a failed step".into()),
        (false, true) => Err("neither ok nor a failed step".into()),
    }
}

/// The one line that says which step of the Windows setup failed, in plain words (Windows' own words go to the log).
pub fn step_line(step: &str) -> &'static str {
    match step {
        "install" => {
            "Windows could not add its SSH server. Check the internet connection and try again."
        }
        "start" => "Windows' SSH server would not start.",
        "passwords" => "Flare could not turn off password logins, so the SSH server stays off.",
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
    /// Sharing is on; `by_flare`: Flare installed or started the SSH server; `firewall`: Flare's own firewall rules
    /// are in place (Windows), so turning off removes them.
    On { by_flare: bool, firewall: bool },
    /// The person said no at the system's prompt: the switch goes back off, with nothing to say.
    Declined,
    /// macOS would not let Flare turn Remote Login on: the person turns it on in System Settings.
    NeedsSettings,
    /// It failed: the one line that says why.
    Failed(String),
}

/// BLOCKING: turns sharing on for this system (rules 1253 and 1254).
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
            let r = match parse_report(&text) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("computer sharing: the setup's result could not be read: {e}");
                    return OnResult::Failed(step_line("").to_string());
                }
            };
            if r.ok {
                OnResult::On {
                    by_flare: r.installed || r.started,
                    firewall: r.rule,
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
                return OnResult::On {
                    by_flare: false,
                    firewall: false,
                };
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
                    return OnResult::On {
                        by_flare: true,
                        firewall: false,
                    };
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

/// What turning off cleans besides the user's own keys (rule 1255).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OffWork {
    /// Stop the SSH server Flare turned on.
    pub stop_server: bool,
    /// Remove the keys Flare added to `administrators_authorized_keys`.
    pub admin_keys: bool,
    /// Remove Flare's own firewall rules.
    pub firewall: bool,
}

/// BLOCKING: cleans up when sharing turns off (rule 1255): the user's own keys, then (elevated, only when there is
/// something to do) the admin keys, the SSH server and Flare's firewall rules. Ok, or the one line that says what is
/// left.
pub fn turn_off(os: &dyn SharingOs, work: OffWork) -> Result<(), String> {
    let OffWork {
        stop_server,
        admin_keys,
        firewall,
    } = work;
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
        Platform::Windows if stop_server || admin_keys || firewall => {
            match os.run_elevated(&|result| windows_off_script(result, stop_server, admin_keys)) {
                Ok(text) => match parse_report(&text) {
                    Ok(r) if r.ok => {}
                    Ok(r) => {
                        log::warn!(
                            "computer sharing: the clean up failed at {}: {}",
                            r.step,
                            r.error
                        );
                        problem = Some(step_line(&r.step).to_string());
                    }
                    Err(e) => {
                        log::warn!(
                            "computer sharing: the clean up's result could not be read: {e}"
                        );
                        problem = Some(step_line("").to_string());
                    }
                },
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

/// Pure: what the dialog says for a question (rules 1254 and 1255).
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
    /// The clean up ended: what it was asked to clean, and how it went.
    TurnedOff {
        work: OffWork,
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
        let work = OffWork {
            stop_server: stop,
            admin_keys: prefs::admin_keys(&self.prefs),
            firewall: prefs::firewall_rule(&self.prefs),
        };
        let (os, post) = (Arc::clone(&self.os), Arc::clone(&self.post));
        worker("flare-sharing-off", move || {
            let result = turn_off(&*os, work);
            Self::say(&post, SharingMsg::TurnedOff { work, result });
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

    fn turned_on(&mut self, by_flare: bool, firewall: bool) {
        self.phase = Phase::On;
        self.ask = None;
        prefs::set_sharing_on(&self.prefs, true);
        // Flare's firewall rules stay Flare's to remove until a clean up removed them.
        if firewall {
            prefs::set_firewall_rule(&self.prefs, true);
        }
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
                OnResult::On { by_flare, firewall } => self.turned_on(by_flare, firewall),
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
                self.turned_on(false, false);
            }
            SharingMsg::TurnedOff { work, result } if self.phase == Phase::TurningOff => {
                self.phase = Phase::Off;
                match result {
                    Ok(()) => {
                        if work.stop_server {
                            prefs::set_server_by_flare(&self.prefs, false);
                        }
                        if work.admin_keys {
                            prefs::set_admin_keys(&self.prefs, false);
                        }
                        if work.firewall {
                            prefs::set_firewall_rule(&self.prefs, false);
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

/// BLOCKING: a new folder in the temp folder, made by this call (`create_dir` fails on a name that is there already, so
/// a folder planted in advance is never used).
fn fresh_folder() -> Result<PathBuf, String> {
    static RUNS: AtomicU64 = AtomicU64::new(0);
    let mut last = String::new();
    for _ in 0..8 {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let n = RUNS.fetch_add(1, Ordering::Relaxed);
        let folder =
            std::env::temp_dir().join(format!("flare-sharing-{}-{stamp}-{n}", std::process::id()));
        match std::fs::create_dir(&folder) {
            // sync-ok: SharingOs calls run on workers
            Ok(()) => return Ok(folder),
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("no folder for the result: {last}"))
}

/// BLOCKING: a file's text, at most `max` bytes (a longer file is an error, never read whole).
fn read_capped(path: &Path, max: usize) -> Result<String, String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?; // sync-ok: SharingOs calls run on workers
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes) // sync-ok: SharingOs calls run on workers
        .map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err("the result is too long".into());
    }
    String::from_utf8(bytes).map_err(|_| "the result is not UTF-8".into())
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
        // A fresh folder for this run's result (made here, never one that was there before); the script itself
        // travels inline, so no script file exists.
        let folder = fresh_folder()?;
        let result = folder.join("result.txt");
        let run = || {
            let argv = elevation_argv(&build(&result))?;
            let status = quiet(&argv[0]).args(&argv[1..]).status(); // sync-ok: waits for the UAC prompt, on a worker
            match (read_capped(&result, REPORT_BYTES_MAX), status) {
                (Ok(text), _) => Ok(text),
                (Err(_), Ok(s)) => Err(format!("no answer from the elevated script ({s})")),
                (Err(_), Err(e)) => Err(format!("PowerShell did not start: {e}")),
            }
        };
        let answer = run();
        let _ = std::fs::remove_dir_all(&folder); // sync-ok: SharingOs calls run on workers
        answer
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
