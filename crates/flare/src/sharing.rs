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
//!   profile, local subnet only: TCP for the fixed pairing ports [`PAIRING_PORTS`], UDP 5353 for the network
//!   announcement). A server Flare starts (installed now, or installed but stopped) takes keys only:
//!   `PasswordAuthentication no` and `ChallengeResponseAuthentication no` go at the top of `sshd_config` before it
//!   starts, with a backup, and the file is put back when a step fails. Before it writes there, the script checks that
//!   only SYSTEM and Administrators can change `%ProgramData%\ssh` and `sshd_config`, and that neither is a link. A
//!   server that already ran keeps its config. The script travels inline (`-EncodedCommand`), so no script file
//!   exists, and it writes no file to report: its exit code says how it went ([`parse_code`]), the outer PowerShell
//!   prints it as one line, and Flare reads that line strictly ([`parse_outcome`]).
//! - **macOS** (rule 1254): port 22 answering means Remote Login is on already. Otherwise `systemsetup
//!   -setremotelogin on` runs through osascript's administrator prompt, with Flare's keys-only file
//!   ([`MAC_KEYS_FILE`]); when macOS refuses (Flare has no Full Disk Access), the Flare dialog "Turn on Remote Login"
//!   offers Open System Settings at Sharing, and Flare watches port 22 and carries on by itself the moment it answers.
//!   The Mac firewall's "Block all incoming connections" stops sharing with one plain line ([`MAC_BLOCK_ALL_LINE`]).
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
/// Flare's own inbound firewall rule for the pairing listener: its program, TCP, the pairing ports, the Private profile
/// and the local subnet only.
pub const FLARE_RULE: &str = "Flare-Pairing-In";
/// The TCP ports the pairing listener (machine pairing PR 3) may take: the first one free, in order. Fixed, so Flare's
/// firewall rule names them.
pub const PAIRING_PORTS: std::ops::RangeInclusive<u16> = 47_790..=47_794;
/// Flare's own inbound firewall rule for the network announcement: its program, UDP 5353 (mDNS), Private, local subnet.
pub const FLARE_MDNS_RULE: &str = "Flare-Pairing-mDNS-In";
/// The mDNS port the announcement uses.
pub const MDNS_PORT: u16 = 5353;
/// The longest command line Windows takes (`CreateProcess`, in UTF-16 units, the final NUL included).
pub const COMMAND_LINE_MAX: usize = 32_767;
/// The most of the outer PowerShell's output Flare reads: a real one is one short line.
pub const OUTCOME_BYTES_MAX: usize = 256;
/// The line the outer PowerShell prints: this, then the elevated script's exit code, or `declined`.
pub const OUTCOME_KEY: &str = "flare-sharing=";
/// The elevated scripts' exit codes start here, so PowerShell's own codes (0, 1) are never read as a report.
pub const EXIT_BASE: i64 = 0x4600;
/// The exit code's failed-step number for a step the script does not know (never a real step).
const STEP_UNKNOWN: i64 = 15;
/// The SIDs that may own and change the SSH server's folder and config: SYSTEM, Administrators, TrustedInstaller, and
/// CREATOR OWNER (it gives a new file to whoever made it, and only those three can make one there).
pub const SSHD_TRUSTED_SIDS: [&str; 4] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
    "S-1-3-0",
];
/// The rights that change a file or folder: write and append data (add file, add folder), write attributes and extended
/// attributes, delete a child, delete, change the rights, take ownership, and generic write and all.
pub const WRITE_RIGHTS_MASK: u32 = 0x500D_0156;
/// The line that starts Flare's lines at the top of `sshd_config`, so a second run replaces them instead of adding more.
pub const SSHD_MARK: &str = "# Flare Computer sharing: keys only";
/// The rights sshd gives `%ProgramData%\ssh` when it makes it: SYSTEM and Administrators full, signed-in users read.
pub const SSHD_DIR_SDDL: &str = "O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;AU)";
/// The settings a server Flare starts gets, at the top of `sshd_config` (sshd keeps the first value it reads, so they
/// win over any later global line). A `Match` block the person wrote that sets one of them again still wins for the
/// logins it matches (sshd's own rule); Windows' default config has none. `ChallengeResponseAuthentication` is the
/// name every Windows OpenSSH knows; on OpenSSH 8.7 and later it is the same setting as
/// `KbdInteractiveAuthentication`, which an older sshd would refuse to start with.
pub const SSHD_KEYS_ONLY: [&str; 2] = [
    "PasswordAuthentication no",
    "ChallengeResponseAuthentication no",
];
/// The comment that starts every key Flare adds to an `authorized_keys` file (machine pairing PR 3 writes them).
pub const KEY_TAG: &str = "flare-pair:";
/// System Settings at General > Sharing (macOS 13 and later).
pub const SHARING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.Sharing-Settings.extension";
/// The keys-only file a Remote Login Flare turns on gets (rule 1254). macOS' own `/etc/ssh/sshd_config` reads
/// `Include /etc/ssh/sshd_config.d/*` before any setting of its own (macOS 13 and later), and sshd keeps the first value
/// it reads, so these lines win. `000-` sorts it first in that folder.
pub const MAC_KEYS_FILE: &str = "/etc/ssh/sshd_config.d/000-flare.conf";
/// The lines of [`MAC_KEYS_FILE`] (macOS' OpenSSH is 8.7 or later, so it knows `KbdInteractiveAuthentication`).
pub const MAC_KEYS_ONLY: [&str; 2] = [
    "PasswordAuthentication no",
    "KbdInteractiveAuthentication no",
];
/// The Mac firewall's tool, asked whether it blocks all incoming connections (no root needed).
pub const SOCKETFILTERFW: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";
/// The one line when the Mac firewall blocks all incoming connections (rule 1254).
pub const MAC_BLOCK_ALL_LINE: &str = "The Mac firewall blocks all incoming connections. Turn that off in System \
     Settings, Network, Firewall, Options.";
/// The one line when the person pressed Cancel at macOS' administrator prompt while turning off (rule 1255).
pub const MAC_OFF_DECLINED_LINE: &str = "macOS did not allow the change, so Remote Login stays on.";

/// The command osascript runs as root to turn Remote Login on (rule 1254): Flare's keys-only file first, so the server
/// never takes a password, then systemsetup. When Remote Login is still off after it (macOS refused), the file goes
/// again, so a Remote Login the person turns on by hand keeps macOS' own setting. Every value is a fixed word of Flare's.
pub fn mac_on_command() -> String {
    let lines = std::iter::once(SSHD_MARK)
        .chain(MAC_KEYS_ONLY)
        .map(|l| format!("'{l}'"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "/bin/mkdir -p /etc/ssh/sshd_config.d && /usr/bin/printf '%s\\n' {lines} > {MAC_KEYS_FILE} \
         && /usr/sbin/systemsetup -setremotelogin on; /usr/sbin/systemsetup -getremotelogin \
         | /usr/bin/grep -q 'Remote Login: On' || /bin/rm -f {MAC_KEYS_FILE}"
    )
}

/// The one line when the Mac firewall is on and Flare is not let in, and Flare did not ask for the administrator
/// password to let it in (Remote Login was on already): pairing's listener and announcement need it (rule 1256).
pub const MAC_APP_FIREWALL_LINE: &str = "The Mac firewall may block pairing. Allow Flare in System Settings, \
     Network, Firewall, Options.";

/// Pure: `socketfilterfw --getglobalstate` says the Mac firewall is on ("Firewall is enabled. (State = 1)"; 2 is
/// block all, also on). Off, or words Flare does not know, is not on.
pub fn firewall_on(out: &str) -> bool {
    let low = out.to_ascii_lowercase();
    !low.contains("disabled")
        && !low.contains("state = 0")
        && (low.contains("enabled") || low.contains("state = 1") || low.contains("state = 2"))
}

/// Pure: `socketfilterfw --getappblocked <exe>` says the firewall lets Flare in ("... is permitted"). Blocked, "not
/// part of the firewall" (never asked) or words Flare does not know are not let in.
pub fn app_allowed(out: &str) -> bool {
    let low = out.to_ascii_lowercase();
    let blocked = low.contains("blocked") && !low.contains("not blocked");
    !blocked && !low.contains("not part") && (low.contains("permitted") || low.contains("allowed"))
}

/// BLOCKING: the Mac firewall is on and does not let Flare in yet, so other computers would not reach pairing's
/// listener or hear its announcement (rule 1256). False when the firewall is off, Flare is let in, or socketfilterfw
/// did not answer.
pub fn mac_app_blocked(os: &dyn SharingOs, flare_exe: &Path) -> bool {
    match os.firewall_global_state() {
        Ok(out) if firewall_on(&out) => {}
        Ok(_) => return false,
        Err(e) => {
            log::info!("computer sharing: socketfilterfw did not answer: {e}");
            return false;
        }
    }
    match os.firewall_app_blocked(flare_exe) {
        Ok(out) if app_allowed(&out) => false,
        Ok(out) => {
            log::info!(
                "computer sharing: the firewall does not let Flare in: {}",
                out.trim()
            );
            true
        }
        Err(e) => {
            log::info!("computer sharing: socketfilterfw did not answer for Flare: {e}");
            true
        }
    }
}

/// Pure: the root commands that let Flare in through the Mac firewall (rule 1256), for the same administrator prompt
/// that turns Remote Login on: `socketfilterfw --add` and `--unblockapp` with Flare's path in sh's single quotes. None
/// for a path that is not absolute or holds a control character.
pub fn mac_allow_app_command(flare_exe: &Path) -> Option<String> {
    let exe = flare_exe.to_str()?;
    if !exe.starts_with('/') || exe.chars().any(char::is_control) {
        return None;
    }
    let exe = format!("'{}'", exe.replace('\'', "'\\''"));
    Some(format!(
        "{SOCKETFILTERFW} --add {exe} >/dev/null 2>&1; {SOCKETFILTERFW} --unblockapp {exe} >/dev/null 2>&1"
    ))
}

/// The command osascript runs as root to turn Remote Login off (rule 1255): systemsetup, then Flare's keys-only file
/// goes, only once Remote Login is off (a server left on keeps keys only).
pub fn mac_off_command() -> String {
    format!(
        "/usr/sbin/systemsetup -f -setremotelogin off; /usr/sbin/systemsetup -getremotelogin \
         | /usr/bin/grep -q 'Remote Login: Off' && /bin/rm -f {MAC_KEYS_FILE}"
    )
}

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
    /// Windows: runs `script` elevated (one UAC prompt), inline with no script file, and waits. What the outer
    /// PowerShell printed (untrusted: [`parse_outcome`] reads it), or Err when PowerShell did not start.
    fn run_elevated(&self, script: &str) -> Result<String, String>;
    /// macOS: runs `command` as root through osascript's administrator prompt. Ok or Err with what it printed.
    fn run_as_admin(&self, command: &str) -> Result<String, String>;
    /// macOS: what `socketfilterfw --getblockall` printed ([`blocks_all`] reads it), or Err when it did not run.
    fn firewall_block_all(&self) -> Result<String, String>;
    /// macOS: what `socketfilterfw --getglobalstate` printed ([`firewall_on`] reads it), or Err (machine pairing PR 3).
    fn firewall_global_state(&self) -> Result<String, String>;
    /// macOS: what `socketfilterfw --getappblocked <exe>` printed ([`app_allowed`] reads it), or Err.
    fn firewall_app_blocked(&self, exe: &Path) -> Result<String, String>;
    /// Opens a URL (System Settings' Sharing page).
    fn open_url(&self, url: &str);
    /// Removes the keys Flare added (a `flare-pair:` comment) from the user's own `authorized_keys`. How many.
    fn strip_user_keys(&self) -> Result<usize, String>;
    /// Starts or stops announcing this computer on the network. Machine pairing PR 3 announces through
    /// `pairing::PairNet` (it needs the pairing port); this stays the switch's own record of it.
    fn announce(&self, on: bool);
    /// Waits between two looks at the port.
    fn wait(&self, ms: u64);
    /// Windows: this account is in the Administrators group, so sshd reads `administrators_authorized_keys` for it
    /// (machine pairing PR 3, rule 1258).
    fn windows_admin(&self) -> bool;
    /// Adds a line to the user's own `authorized_keys` (rule 1258).
    fn add_user_key(&self, line: &str) -> Result<(), String>;
    /// Removes the line holding `key` from the user's own `authorized_keys` (rule 1260). Whether one went.
    fn remove_user_key(&self, key: &str) -> Result<bool, String>;
    /// The SSH server's public host key line (ed25519), which a viewer pins (rule 1259).
    fn ssh_host_key(&self) -> Result<String, String>;
    /// The account's user name, which a viewer's ssh logs in as.
    fn user_name(&self) -> String;
}

// ---- the pure parts ----------------------------------------------------------------------------------------------

/// A PowerShell single-quoted string: nothing inside is expanded, and every quote PowerShell reads as `'` is doubled,
/// the curly ones (U+2018 to U+201B) too, so a path like `C:\Users\D’Angelo` cannot end the string (`flare-shells`).
pub fn ps_quote(s: &str) -> String {
    flare_shells::ps_quote::ps_quote(s)
}

/// The PowerShell function both scripts check the SSH server's folder and files with before they write there as an
/// administrator (rules 1253 and 1255): it throws unless `$path` is no link (no junction, no symbolic link), its owner
/// is one of [`SSHD_TRUSTED_SIDS`], and no other principal is allowed any of [`WRITE_RIGHTS_MASK`]. Deny entries are
/// not counted (they only take rights away).
fn assert_admin_only() -> String {
    let trusted = SSHD_TRUSTED_SIDS
        .iter()
        .map(|s| ps_quote(s))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"function Assert-AdminOnly([string]$path) {{
    $item = Get-Item -LiteralPath $path -Force
    if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {{ throw ($path + ' is a link') }}
    $sid = [System.Security.Principal.SecurityIdentifier]
    $acl = Get-Acl -LiteralPath $path
    $trusted = @({trusted})
    if ($trusted -notcontains $acl.GetOwner($sid).Value) {{ throw ($path + ' has another owner') }}
    foreach ($ace in $acl.GetAccessRules($true, $true, $sid)) {{
        if ($ace.AccessControlType -eq 'Allow' -and $trusted -notcontains $ace.IdentityReference.Value -and ([int64]$ace.FileSystemRights -band 0x{WRITE_RIGHTS_MASK:08X})) {{
            throw ($path + ' can be changed by ' + $ace.IdentityReference.Value)
        }}
    }}
}}
"#
    )
}

/// The end of every elevated script: it reports through its exit code only, so it writes no file anywhere
/// ([`parse_code`] reads it). A step the list does not know is [`STEP_UNKNOWN`], never `ok`.
fn exit_tail() -> String {
    let steps = STEPS
        .iter()
        .map(|s| ps_quote(s))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"if ($fail -ne 0) {{
    $fail = [array]::IndexOf(@({steps}), $step) + 1
    if ($fail -lt 1) {{ $fail = {STEP_UNKNOWN} }}
}}
exit ({EXIT_BASE} + 8 * $fail + [int]$installed + 2 * [int]$started + 4 * [int]$rule)
"#
    )
}

/// The elevated script that turns sharing on (rule 1253), reporting through its exit code. `flare_exe` is the program
/// Flare's firewall rules let in.
pub fn windows_on_script(flare_exe: &Path) -> String {
    let exe = ps_quote(&flare_exe.to_string_lossy());
    let mark = ps_quote(SSHD_MARK);
    let keys_only = SSHD_KEYS_ONLY
        .iter()
        .map(|l| ps_quote(l))
        .collect::<Vec<_>>()
        .join(", ");
    let check = assert_admin_only();
    let tail = exit_tail();
    let ports = format!("{}-{}", PAIRING_PORTS.start(), PAIRING_PORTS.end());
    format!(
        r#"# Flare: turn on Computer sharing (rule 1253). Runs elevated, once per switch.
$ErrorActionPreference = 'Stop'
{check}$step = 'install'
$fail = 0
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
        # Flare writes into the SSH server's folder as an administrator only when no other account can change the
        # folder or the config, and neither is a link.
        $step = 'config'
        $dir = Join-Path $env:ProgramData 'ssh'
        if (-not (Test-Path -LiteralPath $dir)) {{
            # The folder gets the rights sshd gives it: SYSTEM and Administrators write, signed-in users read.
            $acl = New-Object System.Security.AccessControl.DirectorySecurity
            $acl.SetSecurityDescriptorSddlForm('{SSHD_DIR_SDDL}')
            [System.IO.Directory]::CreateDirectory($dir, $acl) | Out-Null
        }}
        Assert-AdminOnly $dir
        $cfg = Join-Path $dir 'sshd_config'
        if (-not (Test-Path -LiteralPath $cfg)) {{
            # A server never started has no config yet: it starts from Windows' default, as sshd itself would.
            $hadCfg = $false
            Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\OpenSSH\sshd_config_default') -Destination $cfg
        }}
        Assert-AdminOnly $cfg
        # A server Flare starts takes keys only, from its first start. Flare's lines go at the top of sshd_config,
        # before any Match block (sshd keeps the first global value it reads); a backup is kept, the other lines stay.
        $step = 'passwords'
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
    New-NetFirewallRule -Name '{FLARE_RULE}' -DisplayName 'Flare pairing' -Enabled True -Direction Inbound -Program {exe} -Protocol TCP -LocalPort {ports} -RemoteAddress LocalSubnet -Action Allow -Profile Private | Out-Null
    New-NetFirewallRule -Name '{FLARE_MDNS_RULE}' -DisplayName 'Flare pairing (mDNS)' -Enabled True -Direction Inbound -Program {exe} -Protocol UDP -LocalPort {MDNS_PORT} -RemoteAddress LocalSubnet -Action Allow -Profile Private | Out-Null
}} catch {{
    # A failed setup leaves the SSH server as it found it: a server Flare started is stopped again, its config and
    # its start type are put back, and Flare's own rules go. The switch goes back off.
    $fail = 1
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
{tail}"#
    )
}

/// The elevated script that cleans up when sharing turns off (rule 1255), reporting through its exit code: the keys
/// Flare added to `administrators_authorized_keys` (`admin_keys`, only when no other account can change that file or
/// its folder), the SSH server (`stop_server`), and always Flare's own firewall rules.
pub fn windows_off_script(stop_server: bool, admin_keys: bool) -> String {
    let keys = if admin_keys {
        format!(
            r#"    $dir = Join-Path $env:ProgramData 'ssh'
    $f = Join-Path $dir 'administrators_authorized_keys'
    if (Test-Path -LiteralPath $f) {{
        $step = 'config'
        Assert-AdminOnly $dir
        Assert-AdminOnly $f
        $step = 'keys'
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
    let check = if admin_keys {
        assert_admin_only()
    } else {
        String::new()
    };
    let tail = exit_tail();
    format!(
        r#"# Flare: turn off Computer sharing (rule 1255). Runs elevated.
$ErrorActionPreference = 'Stop'
{check}$step = 'keys'
$fail = 0
$installed = $false
$started = $false
$rule = $false
try {{
{keys}    $step = 'stop'
{stop}    $step = 'firewall'
    Remove-NetFirewallRule -Name '{FLARE_RULE}' -ErrorAction SilentlyContinue
    Remove-NetFirewallRule -Name '{FLARE_MDNS_RULE}' -ErrorAction SilentlyContinue
}} catch {{
    $fail = 1
}}
{tail}"#
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
/// is answered. The elevated script writes no file: the outer PowerShell reads its exit code and prints one line,
/// `flare-sharing=<code>`, or `flare-sharing=declined` when the prompt was declined (Start-Process fails). The outer
/// command holds only fixed words and base64, so it travels as plain `-Command`. Err when the command line would be
/// longer than Windows takes (the tests hold every script well under it).
pub fn elevation_argv(script: &str) -> Result<Vec<String>, String> {
    let inner = ps_encode(&ps_compact(script));
    let command = format!(
        "try {{ $p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ErrorAction Stop \
         -ArgumentList @('-NoProfile','-NonInteractive','-WindowStyle','Hidden','-EncodedCommand','{inner}'); \
         '{OUTCOME_KEY}' + $p.ExitCode }} catch {{ '{OUTCOME_KEY}declined' }}"
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

/// How an elevated script went, from its exit code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub ok: bool,
    pub installed: bool,
    pub started: bool,
    /// Flare's own firewall rules are in place (the on script made them).
    pub rule: bool,
    /// The step that failed ("" when ok).
    pub step: &'static str,
}

/// The steps an elevated script names when it fails, in the order of their number in the exit code (1 is `install`).
/// `addkey` is machine pairing's (PR 3, rule 1258).
pub const STEPS: [&str; 8] = [
    "install",
    "start",
    "config",
    "passwords",
    "firewall",
    "keys",
    "stop",
    "addkey",
];

/// Pure: the exit code an elevated script ends with for `r`: [`EXIT_BASE`], plus 8 times the failed step's number (0
/// when ok, 1 for the first of [`STEPS`]), plus `installed` (1), `started` (2) and `rule` (4). The scripts' `exit`
/// line is the same sum (the tests hold them together).
#[cfg(test)]
pub fn report_code(r: &Report) -> i64 {
    let step = if r.ok {
        0
    } else {
        STEPS
            .iter()
            .position(|s| *s == r.step)
            .map_or(STEP_UNKNOWN, |i| i as i64 + 1)
    };
    EXIT_BASE + 8 * step + i64::from(r.installed) + 2 * i64::from(r.started) + 4 * i64::from(r.rule)
}

/// Pure: an elevated script's exit code read strictly: from [`EXIT_BASE`] to 127 above it, with a known step number
/// (or 0, ok). Anything else (PowerShell's own 1 when the script could not run, a code another program left) is Err.
pub fn parse_code(code: i64) -> Result<Report, String> {
    let low = code - EXIT_BASE;
    if !(0..128).contains(&low) {
        return Err(format!("the exit code {code} is not Flare's"));
    }
    let step = match low >> 3 {
        0 => "",
        n => *STEPS
            .get(n as usize - 1)
            .ok_or_else(|| format!("the exit code {code} names no step"))?,
    };
    Ok(Report {
        ok: step.is_empty(),
        installed: low & 1 != 0,
        started: low & 2 != 0,
        rule: low & 4 != 0,
        step,
    })
}

/// What the elevation call answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Elevated {
    /// The person said no at the UAC prompt (or the elevated PowerShell did not start).
    Declined,
    /// The elevated script ran and ended with this report.
    Done(Report),
}

/// Pure: reads what the outer PowerShell printed, strictly, as untrusted text: at most [`OUTCOME_BYTES_MAX`] bytes,
/// exactly one line `flare-sharing=<code>` (decimal digits, [`parse_code`]) or `flare-sharing=declined`. A BOM, `\r`
/// and blank lines around it are allowed. Err says what was wrong.
pub fn parse_outcome(text: &str) -> Result<Elevated, String> {
    if text.len() > OUTCOME_BYTES_MAX {
        return Err(format!("the answer is too long ({} bytes)", text.len()));
    }
    let mut lines = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.is_empty());
    let (Some(line), None) = (lines.next(), lines.next()) else {
        return Err("not one line".into());
    };
    let Some(value) = line.strip_prefix(OUTCOME_KEY) else {
        return Err("not Flare's line".into());
    };
    if value == "declined" {
        return Ok(Elevated::Declined);
    }
    if value.is_empty() || value.len() > 9 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err("no exit code".into());
    }
    let code: i64 = value.parse().map_err(|_| "no exit code".to_string())?;
    parse_code(code).map(Elevated::Done)
}

/// The one line that says which step of the Windows setup failed, in plain words.
pub fn step_line(step: &str) -> &'static str {
    match step {
        "install" => {
            "Windows could not add its SSH server. Check the internet connection and try again."
        }
        "start" => "Windows' SSH server would not start.",
        "config" => {
            "Other accounts can change the SSH server's settings folder, so Flare did not touch it."
        }
        "passwords" => "Flare could not turn off password logins, so the SSH server stays off.",
        "firewall" => "Windows' firewall would not take the rule for sharing.",
        "keys" => "Flare could not remove the keys it added.",
        "stop" => "Windows' SSH server would not stop.",
        "addkey" => "Windows would not take the key for this computer.",
        _ => "Computer sharing could not be set up.",
    }
}

/// Pure: osascript's words when the person pressed Cancel at the administrator prompt (error -128).
pub fn mac_declined(out: &str) -> bool {
    let low = out.to_ascii_lowercase();
    out.contains("-128") || low.contains("user canceled") || low.contains("user cancelled")
}

/// Pure: `socketfilterfw --getblockall` says the Mac firewall blocks all incoming connections. macOS has said it as
/// "Firewall has block all state set to enabled.", "Block all ENABLED!" and "Firewall is set to block all non-essential
/// incoming connections"; anything that says disabled, or nothing known, is not blocking.
pub fn blocks_all(out: &str) -> bool {
    let low = out.to_ascii_lowercase();
    !low.contains("disabled")
        && (low.contains("enabled") || low.contains("block all non-essential"))
}

/// BLOCKING: the one line when the Mac firewall blocks all incoming connections, so port 22 answering on 127.0.0.1
/// does not mean another computer reaches it (rule 1254). None when it does not block, or the tool did not answer.
pub fn mac_firewall_problem(os: &dyn SharingOs) -> Option<String> {
    match os.firewall_block_all() {
        Ok(out) if blocks_all(&out) => {
            log::info!("computer sharing: the firewall blocks all: {}", out.trim());
            Some(MAC_BLOCK_ALL_LINE.to_string())
        }
        Ok(_) => None,
        Err(e) => {
            log::info!("computer sharing: socketfilterfw did not answer: {e}");
            None
        }
    }
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

/// The elevated script that adds a paired computer's key to Windows' `administrators_authorized_keys` (machine pairing
/// PR 3, rule 1258), reporting through its exit code like the others: the same key's older line goes, the line is
/// added, and the file gets the access sshd wants (Administrators and SYSTEM only, by SID, so any Windows language
/// works). It writes only when no other account can change the folder or the file, and neither is a link
/// (`Assert-AdminOnly`, step `config`). `key` is the key's base64 field, checked by the caller.
pub fn windows_add_key_script(line: &str, key: &str) -> String {
    let line = ps_quote(line);
    let key = ps_quote(key);
    let check = assert_admin_only();
    let tail = exit_tail();
    format!(
        r#"# Flare: let a paired computer in (rule 1258). Runs elevated.
$ErrorActionPreference = 'Stop'
{check}$step = 'config'
$fail = 0
$installed = $false
$started = $false
$rule = $false
try {{
    $dir = Join-Path $env:ProgramData 'ssh'
    Assert-AdminOnly $dir
    $f = Join-Path $dir 'administrators_authorized_keys'
    $keep = New-Object System.Collections.Generic.List[string]
    if (Test-Path -LiteralPath $f) {{
        Assert-AdminOnly $f
        foreach ($l in @(Get-Content -LiteralPath $f)) {{ if ((-split $l)[1] -ne {key}) {{ $keep.Add($l) }} }}
    }}
    $step = 'addkey'
    $keep.Add({line})
    Set-Content -LiteralPath $f -Value $keep -Encoding ascii
    & icacls.exe $f /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F' | Out-Null
    if ($LASTEXITCODE -ne 0) {{ throw "icacls ended with $LASTEXITCODE" }}
}} catch {{
    $fail = 1
}}
{tail}"#
    )
}

/// The elevated script that removes one paired computer's key from `administrators_authorized_keys` (rule 1260),
/// reporting through its exit code, after the same `Assert-AdminOnly` checks.
pub fn windows_remove_key_script(key: &str) -> String {
    let key = ps_quote(key);
    let check = assert_admin_only();
    let tail = exit_tail();
    format!(
        r#"# Flare: remove a paired computer's key (rule 1260). Runs elevated.
$ErrorActionPreference = 'Stop'
{check}$step = 'keys'
$fail = 0
$installed = $false
$started = $false
$rule = $false
try {{
    $dir = Join-Path $env:ProgramData 'ssh'
    $f = Join-Path $dir 'administrators_authorized_keys'
    if (Test-Path -LiteralPath $f) {{
        $step = 'config'
        Assert-AdminOnly $dir
        Assert-AdminOnly $f
        $step = 'keys'
        $keep = @(Get-Content -LiteralPath $f | Where-Object {{ (-split $_)[1] -ne {key} }})
        Set-Content -LiteralPath $f -Value $keep -Encoding ascii
    }}
}} catch {{
    $fail = 1
}}
{tail}"#
    )
}

/// Pure: whether an elevated run went well: Ok(true) ok, Ok(false) the person declined, Err the failed step (or ""
/// when the answer could not be read).
fn elevated_ok(answer: Result<String, String>) -> Result<bool, &'static str> {
    match answer.map(|t| parse_outcome(&t)) {
        Ok(Ok(Elevated::Done(r))) if r.ok => Ok(true),
        Ok(Ok(Elevated::Done(r))) => {
            log::warn!("pairing: the elevated key step failed at {}", r.step);
            Err(r.step)
        }
        Ok(Ok(Elevated::Declined)) => Ok(false),
        Ok(Err(e)) => {
            log::warn!("pairing: the elevated key step's answer could not be read: {e}");
            Err("")
        }
        Err(e) => {
            log::info!("pairing: the elevated key step did not run: {e}");
            Ok(false)
        }
    }
}

/// Pure: a key's base64 field that may go into a script: base64's letters only.
pub fn plain_key(key: &str) -> bool {
    crate::machines::plain_base64(key)
}

/// BLOCKING: lets a paired computer in (rule 1258): on a Windows account in Administrators, an elevated script (a UAC
/// prompt) writes `administrators_authorized_keys`; any other account's own `~/.ssh/authorized_keys` takes the line
/// with no prompt. True when the admin file took it; else the one line that says why not.
pub fn install_key(os: &dyn SharingOs, line: &str) -> Result<bool, String> {
    let key = line.split_ascii_whitespace().nth(1).unwrap_or_default();
    if os.platform() == Platform::Windows && os.windows_admin() {
        if !plain_key(key) {
            return Err("The key that came is not one Flare takes.".into());
        }
        return match elevated_ok(os.run_elevated(&windows_add_key_script(line, key))) {
            Ok(true) => Ok(true),
            Ok(false) => {
                Err("Windows did not allow the change, so the computer was not added.".into())
            }
            Err("config") => Err(step_line("config").into()),
            Err(_) => Err(step_line("addkey").into()),
        };
    }
    os.add_user_key(line).map_err(|e| {
        log::warn!("pairing: the key could not be added: {e}");
        "Flare could not add the key for this computer.".to_string()
    })?;
    Ok(false)
}

/// BLOCKING: removes a paired computer's key (rule 1260): from the user's own file, and, when a key went into the
/// admin file (`admin`), from there with one elevated script.
pub fn remove_key(os: &dyn SharingOs, key: &str, admin: bool) -> Result<(), String> {
    if let Err(e) = os.remove_user_key(key) {
        log::warn!("pairing: the key stays: {e}");
        return Err(step_line("keys").to_string());
    }
    if admin && os.platform() == Platform::Windows && plain_key(key) {
        match elevated_ok(os.run_elevated(&windows_remove_key_script(key))) {
            Ok(true) => {}
            Ok(false) => {
                return Err(
                    "Windows did not allow the change, so the key stays until sharing is turned off."
                        .into(),
                );
            }
            Err("config") => return Err(step_line("config").into()),
            Err(_) => return Err(step_line("keys").to_string()),
        }
    }
    Ok(())
}

// ---- turning on and off, over the OS calls -----------------------------------------------------------------------

/// How turning on went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnResult {
    /// Sharing is on; `by_flare`: Flare installed or started the SSH server; `firewall`: Flare's own firewall rules
    /// are in place (Windows), so turning off removes them; `note`: a line to show once it is on (the Mac firewall
    /// does not let Flare in, rule 1256).
    On {
        by_flare: bool,
        firewall: bool,
        note: Option<String>,
    },
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
            let text = match os.run_elevated(&windows_on_script(flare_exe)) {
                Ok(t) => t,
                Err(e) => {
                    log::info!("computer sharing: the elevated setup did not run: {e}");
                    return OnResult::Declined;
                }
            };
            let r = match parse_outcome(&text) {
                Ok(Elevated::Done(r)) => r,
                Ok(Elevated::Declined) => {
                    log::info!("computer sharing: the UAC prompt was declined");
                    return OnResult::Declined;
                }
                Err(e) => {
                    log::warn!("computer sharing: the setup's answer could not be read: {e}");
                    return OnResult::Failed(step_line("").to_string());
                }
            };
            if r.ok {
                OnResult::On {
                    by_flare: r.installed || r.started,
                    firewall: r.rule,
                    note: None,
                }
            } else {
                log::warn!("computer sharing: the setup failed at {}", r.step);
                OnResult::Failed(step_line(r.step).to_string())
            }
        }
        Platform::Mac => {
            if os.port_open(SSH_PORT) {
                return match mac_firewall_problem(os) {
                    Some(line) => OnResult::Failed(line),
                    // No password prompt here, so a firewall that does not let Flare in is the person's to change.
                    None => OnResult::On {
                        by_flare: false,
                        firewall: false,
                        note: mac_app_blocked(os, flare_exe)
                            .then(|| MAC_APP_FIREWALL_LINE.to_string()),
                    },
                };
            }
            // The firewall is asked before the password prompt too, so Flare never turns on a server that nobody can
            // reach and leaves it on.
            if let Some(line) = mac_firewall_problem(os) {
                return OnResult::Failed(line);
            }
            // A firewall that does not let Flare in yet does so in the same administrator prompt (rule 1256).
            let mut command = mac_on_command();
            let mut note = None;
            if mac_app_blocked(os, flare_exe) {
                match mac_allow_app_command(flare_exe) {
                    Some(allow) => command = format!("{command}; {allow}"),
                    None => note = Some(MAC_APP_FIREWALL_LINE.to_string()),
                }
            }
            match os.run_as_admin(&command) {
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
                        note,
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
            let declined = "Windows did not allow the change, so the SSH server and the keys stay as they were.";
            match os
                .run_elevated(&windows_off_script(stop_server, admin_keys))
                .map(|text| parse_outcome(&text))
            {
                Ok(Ok(Elevated::Done(r))) if r.ok => {}
                Ok(Ok(Elevated::Done(r))) => {
                    log::warn!("computer sharing: the clean up failed at {}", r.step);
                    problem = Some(step_line(r.step).to_string());
                }
                Ok(Ok(Elevated::Declined)) => {
                    log::info!("computer sharing: the UAC prompt was declined");
                    problem = Some(declined.to_string());
                }
                Ok(Err(e)) => {
                    log::warn!("computer sharing: the clean up's answer could not be read: {e}");
                    problem = Some(step_line("").to_string());
                }
                Err(e) => {
                    log::info!("computer sharing: the elevated clean up did not run: {e}");
                    problem = Some(declined.to_string());
                }
            }
        }
        Platform::Mac if stop_server => {
            match os.run_as_admin(&mac_off_command()) {
                // Cancel at the password prompt: nothing changed, so there is nothing to wait for.
                Err(out) if mac_declined(&out) => {
                    log::info!("computer sharing: the password prompt was cancelled");
                    return Err(MAC_OFF_DECLINED_LINE.to_string());
                }
                Err(out) => log::info!("computer sharing: systemsetup said: {}", out.trim()),
                Ok(_) => {}
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
    /// Port 22 answered while "Turn on Remote Login" was up (the watch's number), and the Mac firewall's one line when
    /// it blocks all incoming connections.
    RemoteLoginSeen(u64, Option<String>),
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
    fn run_elevated(&self, _script: &str) -> Result<String, String> {
        Err("no setup here".into())
    }
    fn run_as_admin(&self, _command: &str) -> Result<String, String> {
        Err("no setup here".into())
    }
    fn firewall_block_all(&self) -> Result<String, String> {
        Err("no setup here".into())
    }
    fn firewall_global_state(&self) -> Result<String, String> {
        Err("no setup here".into())
    }
    fn firewall_app_blocked(&self, _exe: &Path) -> Result<String, String> {
        Err("no setup here".into())
    }
    fn open_url(&self, _url: &str) {}
    fn strip_user_keys(&self) -> Result<usize, String> {
        Ok(0)
    }
    fn announce(&self, _on: bool) {}
    fn wait(&self, _ms: u64) {}
    fn windows_admin(&self) -> bool {
        false
    }
    fn add_user_key(&self, _line: &str) -> Result<(), String> {
        Err("no setup here".into())
    }
    fn remove_user_key(&self, _key: &str) -> Result<bool, String> {
        Ok(false)
    }
    fn ssh_host_key(&self) -> Result<String, String> {
        Err("no setup here".into())
    }
    fn user_name(&self) -> String {
        String::new()
    }
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
    /// Runs on the clean up's worker before it starts: waits for pairing's host requests to end (rule 1258).
    before_off: Arc<dyn Fn() + Send + Sync>,
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
            before_off: Arc::new(|| {}),
        }
    }

    /// The OS calls in use (a page that takes the settings later keeps them).
    pub fn os(&self) -> Arc<dyn SharingOs> {
        Arc::clone(&self.os)
    }

    /// The settings store the switch keeps its state in (pairing notes an admin key there).
    pub fn prefs(&self) -> PrefStore {
        self.prefs.clone()
    }

    /// What the clean up waits for first, on its worker: pairing's host requests that still run (rule 1258), so a key
    /// one adds meanwhile is cleaned up too.
    pub fn set_before_off(&mut self, f: Arc<dyn Fn() + Send + Sync>) {
        self.before_off = f;
    }

    #[cfg(test)]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// A line for the sharing dialog with OK (a key that could not be removed, rule 1260), unless it asks something.
    pub fn notice(&mut self, line: String) {
        if self.ask.is_none() {
            self.ask = Some(SharingAsk::Notice(line));
        }
    }

    /// Sharing is on and set up (not on its way on or off): the computer is announced and takes pairing requests.
    pub fn is_on(&self) -> bool {
        self.phase == Phase::On
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
        let (os, post, prefs, before) = (
            Arc::clone(&self.os),
            Arc::clone(&self.post),
            self.prefs.clone(),
            Arc::clone(&self.before_off),
        );
        worker("flare-sharing-off", move || {
            // A pairing request that ran when the switch went off has ended first; what it added is read after it.
            before();
            let work = OffWork {
                stop_server: stop,
                admin_keys: prefs::admin_keys(&prefs),
                firewall: prefs::firewall_rule(&prefs),
            };
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
                    let problem = mac_firewall_problem(&*os);
                    Self::say(&post, SharingMsg::RemoteLoginSeen(n, problem));
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
                OnResult::On {
                    by_flare,
                    firewall,
                    note,
                } => {
                    self.turned_on(by_flare, firewall);
                    // Sharing works; the line says what still stops pairing (rule 1256).
                    if let Some(line) = note {
                        self.ask = Some(SharingAsk::Notice(line));
                    }
                }
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
            SharingMsg::RemoteLoginSeen(n, problem)
                if self.phase == Phase::WaitingForRemoteLogin
                    && n == self.watch.load(Ordering::SeqCst) =>
            {
                self.watch.store(0, Ordering::SeqCst);
                match problem {
                    // The firewall blocks everyone: sharing does not count as working.
                    Some(line) => {
                        self.phase = Phase::Off;
                        self.ask = Some(SharingAsk::Notice(line));
                    }
                    // The person turned Remote Login on: the server is theirs, not Flare's.
                    None => self.turned_on(false, false),
                }
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

/// BLOCKING: what the Mac firewall's tool printed for a question that needs no root (a worker's call).
fn socketfilterfw(args: &[&std::ffi::OsStr]) -> Result<String, String> {
    if !cfg!(target_os = "macos") {
        return Err("not a Mac".into());
    }
    let out = quiet(SOCKETFILTERFW)
        .args(args)
        .output() // sync-ok: SharingOs calls run on workers
        .map_err(|e| format!("socketfilterfw did not start: {e}"))?;
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

    fn run_elevated(&self, script: &str) -> Result<String, String> {
        // The script travels inline and writes no file; the outer PowerShell prints one line with its exit code.
        let argv = elevation_argv(script)?;
        let out = quiet(&argv[0])
            .args(&argv[1..])
            .output() // sync-ok: waits for the UAC prompt, on a worker
            .map_err(|e| format!("PowerShell did not start: {e}"))?;
        let mut stdout = out.stdout;
        stdout.truncate(OUTCOME_BYTES_MAX + 1);
        Ok(String::from_utf8_lossy(&stdout).into_owned())
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

    fn firewall_global_state(&self) -> Result<String, String> {
        socketfilterfw(&["--getglobalstate".as_ref()])
    }

    fn firewall_app_blocked(&self, exe: &Path) -> Result<String, String> {
        socketfilterfw(&["--getappblocked".as_ref(), exe.as_os_str()])
    }

    fn firewall_block_all(&self) -> Result<String, String> {
        if !cfg!(target_os = "macos") {
            return Err("not a Mac".into());
        }
        let out = quiet(SOCKETFILTERFW)
            .arg("--getblockall")
            .output() // sync-ok: SharingOs calls run on workers
            .map_err(|e| format!("socketfilterfw did not start: {e}"))?;
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
        // The announcement itself is pairing's (`pairing::RealPairNet`: it carries the pairing port).
        log::info!("computer sharing: announced {on}");
    }

    fn wait(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms)); // sync-ok: a worker waits between two looks at a port
    }

    fn windows_admin(&self) -> bool {
        if !cfg!(windows) {
            return false;
        }
        // The Administrators group's SID is listed (as "deny only" before elevation) for every account in it.
        quiet("whoami.exe")
            .args(["/groups", "/fo", "csv"])
            .output() // sync-ok: SharingOs calls run on workers
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("S-1-5-32-544"))
            .unwrap_or(false)
    }

    fn add_user_key(&self, line: &str) -> Result<(), String> {
        crate::pair_keys::add_user_key(&self.home, line)
    }

    fn remove_user_key(&self, key: &str) -> Result<bool, String> {
        crate::pair_keys::remove_user_key(&self.home, key)
    }

    fn ssh_host_key(&self) -> Result<String, String> {
        let file = if cfg!(windows) {
            PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into()))
                .join("ssh")
                .join("ssh_host_ed25519_key.pub")
        } else {
            PathBuf::from("/etc/ssh/ssh_host_ed25519_key.pub")
        };
        std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display())) // sync-ok: SharingOs calls run on workers
    }

    fn user_name(&self) -> String {
        let k = if cfg!(windows) { "USERNAME" } else { "USER" };
        std::env::var(k).unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "sharing_fake.rs"]
pub(crate) mod fake;

#[cfg(test)]
#[path = "sharing_tests.rs"]
mod tests;
