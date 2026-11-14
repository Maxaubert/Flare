//! The Computer sharing switch (rules 1252 to 1255) against a fake OS: the scripts are read as text and the elevation
//! as its argv, and nothing is ever run.

use super::fake::{FakeOs, ok_outcome, outcome};
use super::*;
use std::sync::Mutex;

/// The switch and what its workers posted.
struct Rig {
    sharing: Sharing,
    os: Arc<FakeOs>,
    prefs: PrefStore,
    got: Arc<Mutex<Vec<UiMsg>>>,
}

impl Rig {
    fn new(os: FakeOs, prefs: PrefStore) -> Self {
        let os = Arc::new(os);
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = Arc::clone(&got);
        let post: Post = Arc::new(move |m| g.lock().unwrap().push(m));
        let sharing = Sharing::new(os.clone(), prefs.clone(), post);
        Rig {
            sharing,
            os,
            prefs,
            got,
        }
    }

    /// Hands the workers' answers to the switch until `done` holds (10 s at most).
    fn settle(&mut self, done: impl Fn(&Sharing) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let msgs: Vec<UiMsg> = self.got.lock().unwrap().drain(..).collect();
            for m in msgs {
                if let UiMsg::Settings(SettingsMsg::Machines(MachinesMsg::Sharing(m))) = m {
                    self.sharing.on_msg(m);
                }
            }
            if done(&self.sharing) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "timed out");
            std::thread::sleep(std::time::Duration::from_millis(2)); // sync-ok: a test waits for its workers
        }
    }

    fn settle_phase(&mut self, phase: Phase) {
        self.settle(|s| s.phase() == phase);
    }
}

fn on(info: SharingInfo) -> (bool, bool) {
    (info.on, info.busy)
}

fn decode(b64: &str) -> String {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).unwrap()
}

/// A failed step's report, as the fake's outer PowerShell prints it.
fn failed(step: &'static str) -> String {
    outcome(&Report {
        step,
        ..Report::default()
    })
}

// ---- the scripts and the calls, as text ---------------------------------------------------------------------------

/// Rule 1253: the Windows script installs the server only when missing, starts it and sets it Automatic, and scopes
/// the firewall to the Private profile.
#[test]
fn the_windows_script_installs_starts_and_opens_the_private_profile_only() {
    let s = windows_on_script(Path::new(
        r"C:\Users\Jo O'Neil\AppData\Local\Flare\current\Flare.exe",
    ));
    for want in [
        "$ErrorActionPreference = 'Stop'",
        "Get-WindowsCapability -Online",
        "$_.Name -like 'OpenSSH.Server*'",
        "if ($null -eq $cap -or $cap.State -ne 'Installed') {",
        "Add-WindowsCapability -Online -Name 'OpenSSH.Server~~~~0.0.1.0'",
        "Start-Service -Name 'sshd'",
        "Set-Service -Name 'sshd' -StartupType Automatic",
        "-LocalPort 22 -Profile Private",
        "if ($null -ne $sshdRule) { Set-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -Profile Private -Enabled True }",
        "$startType = [string]$svc.StartType",
        "if ($started) { Stop-Service -Name 'sshd' -ErrorAction SilentlyContinue }",
        "if ($null -ne $startType -and $startType -ne 'Automatic') { Set-Service -Name 'sshd' -StartupType $startType",
    ] {
        assert!(s.contains(want), "{want}\n{s}");
    }
    // PowerShell reads a curly quote as `'` too: a path with one (D’Angelo) is doubled, so it cannot end the string.
    let curly = windows_on_script(Path::new(
        "C:\\Users\\D\u{2018}x\u{201A}y\u{201B}\u{2019}\\Flare.exe",
    ));
    assert!(curly.contains(
        "-Program 'C:\\Users\\D\u{2018}\u{2018}x\u{201A}\u{201A}y\u{201B}\u{201B}\u{2019}\u{2019}\\Flare.exe'"
    ));
    assert_eq!(ps_quote("a'b\u{2019}c"), "'a''b\u{2019}\u{2019}c'");
    // Never a profile other than Private, never Flare's rule opened to all.
    for profile in ["Public", "Domain", "Any"] {
        assert!(!s.contains(&format!("-Profile {profile}")), "{profile}");
    }
    assert_eq!(s.matches("-Profile Private").count(), 4);
    // A server Flare starts (installed now, or one the person had but left stopped) has its rule set to Private
    // before it starts: its rule may name every profile, and it was never running, so nobody relied on it.
    let restrict = s
        .find("Set-NetFirewallRule -Name 'OpenSSH-Server-In-TCP'")
        .unwrap();
    let start = s.find("Start-Service -Name 'sshd'").unwrap();
    assert!(restrict < start);
    let guard = &s[s.find("if ($svc.Status -ne 'Running') {").unwrap()..start];
    assert!(guard.contains("Set-NetFirewallRule"), "{guard}");
    assert!(!s.contains('\u{2014}'));
}

/// Rule 1253: Flare's own rules let in only its program, on the Private profile, from the local subnet: TCP for the
/// fixed pairing ports and UDP 5353 for the announcement. A failed setup removes them again, and the report says
/// whether they are in place.
#[test]
fn flares_own_firewall_rules_are_narrow_and_go_when_the_setup_fails() {
    let s = windows_on_script(Path::new(
        r"C:\Users\Jo O'Neil\AppData\Local\Flare\current\Flare.exe",
    ));
    let exe = "'C:\\Users\\Jo O''Neil\\AppData\\Local\\Flare\\current\\Flare.exe'";
    for want in [
        format!(
            "New-NetFirewallRule -Name 'Flare-Pairing-In' -DisplayName 'Flare pairing' -Enabled True -Direction Inbound \
             -Program {exe} -Protocol TCP -LocalPort 47790-47794 -RemoteAddress LocalSubnet -Action Allow \
             -Profile Private"
        ),
        format!(
            "New-NetFirewallRule -Name 'Flare-Pairing-mDNS-In' -DisplayName 'Flare pairing (mDNS)' -Enabled True \
             -Direction Inbound -Program {exe} -Protocol UDP -LocalPort 5353 -RemoteAddress LocalSubnet -Action Allow \
             -Profile Private"
        ),
    ] {
        assert!(s.contains(&want), "{want}\n{s}");
    }
    // The pairing listener (PR 3) takes the first free port of this fixed range, so the rule can name it.
    assert_eq!(PAIRING_PORTS, 47_790..=47_794);
    assert!(!PAIRING_PORTS.contains(&SSH_PORT) && !PAIRING_PORTS.contains(&MDNS_PORT));
    assert!(
        *PAIRING_PORTS.end() < 49_152,
        "below Windows' and macOS' own outgoing ports"
    );
    let catch = &s[s.find("} catch {").unwrap()..];
    for want in [
        "Remove-NetFirewallRule -Name 'Flare-Pairing-In'",
        "Remove-NetFirewallRule -Name 'Flare-Pairing-mDNS-In'",
        "$rule = $false",
    ] {
        assert!(catch.contains(want), "{want}");
    }
    let made = s.find("$rule = $true").unwrap();
    assert!(
        made < s
            .find("New-NetFirewallRule -Name 'Flare-Pairing-In'")
            .unwrap()
    );
}

/// Rule 1253 (keys only): a server Flare starts (installed now, or installed but stopped) gets password and
/// keyboard-interactive logins off before it starts, at the top of `sshd_config` so they win over any later global
/// line (a Match block the person wrote still wins for what it matches, sshd's own rule). A backup is kept, the
/// person's other lines stay, a missing config starts from Windows' default in a folder with sshd's own rights, and a
/// failed step puts the config back. A server that already ran is left alone.
#[test]
fn a_server_flare_starts_takes_keys_only_and_a_running_one_is_left_alone() {
    let s = windows_on_script(Path::new(r"C:\F.exe"));
    let guard = s.find("if ($svc.Status -ne 'Running') {").unwrap();
    let start = s.find("Start-Service -Name 'sshd'").unwrap();
    let block = &s[guard..start];
    for want in [
        "$step = 'passwords'",
        "$cfg = Join-Path $dir 'sshd_config'",
        "$dir = Join-Path $env:ProgramData 'ssh'",
        "Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\\OpenSSH\\sshd_config_default') -Destination $cfg",
        "$acl.SetSecurityDescriptorSddlForm('O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;AU)')",
        "[System.IO.Directory]::CreateDirectory($dir, $acl)",
        "$backup = $cfg + '.flare-backup'",
        "Copy-Item -LiteralPath $cfg -Destination $backup -Force",
        "$flare = @('# Flare Computer sharing: keys only', 'PasswordAuthentication no', \
         'ChallengeResponseAuthentication no')",
        "[System.IO.File]::WriteAllLines($cfg, [string[]]($flare + $keep))",
        "$keep.Add($old[$i])",
    ] {
        assert!(block.contains(want), "{want}\n{block}");
    }
    // Flare's lines go first (sshd keeps the first global value), so no Match block can come before them.
    assert!(block.find("$flare + $keep").is_some());
    // Only the keys-only lines are written; nothing turns a login method on, and the older sshd's unknown keyword
    // is never written.
    assert!(!s.contains("PasswordAuthentication yes"));
    assert!(!s.contains(" yes'"));
    assert!(!s.contains("KbdInteractiveAuthentication"));
    // The passwords step runs only for a server that was not running, before the start: never with passwords on.
    assert_eq!(s.matches("$step = 'passwords'").count(), 1);
    assert!(s.find("WriteAllLines").unwrap() < start);
    assert!(
        s.find("Restart-Service").is_none(),
        "a running server is never touched"
    );
    // A failed step puts the config back (or removes the one Flare copied in) and stops what Flare started.
    let catch = &s[s.find("} catch {").unwrap()..];
    for want in [
        "if ($null -ne $backup -and (Test-Path -LiteralPath $backup)) { Copy-Item -LiteralPath $backup -Destination $cfg \
         -Force",
        "if (-not $hadCfg -and $null -ne $cfg) { Remove-Item -LiteralPath $cfg -Force",
    ] {
        assert!(catch.contains(want), "{want}\n{catch}");
    }
    assert_eq!(SSHD_MARK, "# Flare Computer sharing: keys only");
}

/// Rule 1253: before the script writes into `%ProgramData%\ssh` (copies the default config in, keeps a backup, writes
/// Flare's lines) and before sshd starts, the folder and `sshd_config` must be no link, owned by SYSTEM or
/// Administrators, and changeable by no other account. A wrong one fails the `config` step, which puts the server back.
#[test]
fn the_ssh_folder_and_config_are_checked_before_flare_writes_there() {
    let s = windows_on_script(Path::new(r"C:\F.exe"));
    let at = |want: &str| s.find(want).unwrap_or_else(|| panic!("{want}\n{s}"));
    // The check itself.
    for want in [
        "function Assert-AdminOnly([string]$path) {",
        "$item = Get-Item -LiteralPath $path -Force",
        "if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { throw ($path + ' is a link') }",
        "$acl = Get-Acl -LiteralPath $path",
        "$trusted = @('S-1-5-18', 'S-1-5-32-544', \
         'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464', 'S-1-3-0')",
        "if ($trusted -notcontains $acl.GetOwner($sid).Value) { throw ($path + ' has another owner') }",
        "foreach ($ace in $acl.GetAccessRules($true, $true, $sid)) {",
        "if ($ace.AccessControlType -eq 'Allow' -and $trusted -notcontains $ace.IdentityReference.Value -and \
         ([int64]$ace.FileSystemRights -band 0x500D0156)) {",
    ] {
        at(want);
    }
    // Write data, append data, write attributes and extended attributes, delete a child, delete, change the rights,
    // take ownership, generic write, generic all.
    assert_eq!(
        WRITE_RIGHTS_MASK,
        0x2 | 0x4
            | 0x10
            | 0x40
            | 0x100
            | 0x1_0000
            | 0x4_0000
            | 0x8_0000
            | 0x4000_0000
            | 0x1000_0000
    );
    // The order: the folder is made (with sshd's rights) or found, then checked, before the config is touched; the
    // config is checked before the backup and the write; everything before sshd starts.
    let step = at("$step = 'config'");
    let made = at("[System.IO.Directory]::CreateDirectory($dir, $acl)");
    let dir_check = at("Assert-AdminOnly $dir");
    let cfg_set = at("$cfg = Join-Path $dir 'sshd_config'");
    let copy_in = at("System32\\OpenSSH\\sshd_config_default");
    let cfg_check = at("Assert-AdminOnly $cfg");
    let backup = at("$backup = $cfg + '.flare-backup'");
    let write = at("[System.IO.File]::WriteAllLines");
    let start = at("Start-Service -Name 'sshd'");
    assert!(step < made && made < dir_check && dir_check < cfg_set && cfg_set < copy_in);
    assert!(copy_in < cfg_check && cfg_check < backup && backup < write && write < start);
    // `$cfg` is set only once the folder passed, so a failed folder check removes nothing in it.
    assert_eq!(s.matches("$cfg = Join-Path").count(), 1);
    assert_eq!(
        step_line("config"),
        "Other accounts can change the SSH server's settings folder, so Flare did not touch it."
    );
    // The off script checks the folder and the admin keys file before it rewrites the file; with no keys to remove it
    // writes nothing there and needs no check.
    let off = windows_off_script(false, true);
    let check = off.find("Assert-AdminOnly $f").unwrap();
    assert!(off.find("Assert-AdminOnly $dir").unwrap() < check);
    assert!(check < off.find("Set-Content -LiteralPath $f").unwrap());
    assert!(off.contains("function Assert-AdminOnly"));
    assert!(!windows_off_script(true, false).contains("Assert-AdminOnly"));
}

/// Rule 1253: the elevated scripts write no file anywhere: they report only through their exit code, which the outer,
/// non-elevated PowerShell reads and prints. The `exit` line is the same sum as [`report_code`], and the step list in
/// it is [`STEPS`] in order.
#[test]
fn the_elevated_scripts_report_through_their_exit_code_and_write_no_file() {
    for s in [
        windows_on_script(Path::new(r"C:\F.exe")),
        windows_off_script(true, true),
        windows_off_script(false, false),
    ] {
        for gone in [
            "$result",
            "result.txt",
            "$out",
            "Out-File",
            "-RedirectStandard",
        ] {
            assert!(!s.contains(gone), "{gone}\n{s}");
        }
        // Set-Content appears only for the admin keys file, inside `%ProgramData%\ssh`, after its check.
        assert!(
            s.matches("Set-Content").count() <= 1
                && !s.contains("Set-Content -LiteralPath $result"),
            "{s}"
        );
        for want in [
            "$fail = 0",
            "$fail = [array]::IndexOf(@('install', 'start', 'config', 'passwords', 'firewall', 'keys', 'stop'), $step) + 1",
            "if ($fail -lt 1) { $fail = 15 }",
            "exit (17920 + 8 * $fail + [int]$installed + 2 * [int]$started + 4 * [int]$rule)",
        ] {
            assert!(s.contains(want), "{want}\n{s}");
        }
        let catch = &s[s.find("} catch {").unwrap()..];
        assert!(catch.contains("$fail = 1"), "{catch}");
        // Every step the script names is one the exit code can carry.
        for (i, _) in s.match_indices("$step = '") {
            let name = s[i + 9..].split('\'').next().unwrap();
            assert!(STEPS.contains(&name), "{name}");
        }
    }
    assert_eq!(EXIT_BASE, 17_920);
}

/// Rule 1253: the exit code is read strictly. Every report goes there and back; PowerShell's own codes, a code below or
/// above Flare's range, or an unknown step number are errors, never a half-read report.
#[test]
fn the_exit_code_carries_the_report_and_is_read_strictly() {
    for step in [""].into_iter().chain(STEPS) {
        for flags in 0..8u8 {
            let r = Report {
                ok: step.is_empty(),
                installed: flags & 1 != 0,
                started: flags & 2 != 0,
                rule: flags & 4 != 0,
                step,
            };
            assert_eq!(parse_code(report_code(&r)), Ok(r.clone()), "{r:?}");
        }
    }
    assert_eq!(
        parse_code(EXIT_BASE + 7).unwrap(),
        Report {
            ok: true,
            installed: true,
            started: true,
            rule: true,
            step: ""
        }
    );
    assert_eq!(parse_code(EXIT_BASE + 8).unwrap().step, "install");
    assert_eq!(parse_code(EXIT_BASE + 8 * 3).unwrap().step, "config");
    for bad in [
        0,
        1,
        -1,
        EXIT_BASE - 1,
        EXIT_BASE + 8 * 8,
        EXIT_BASE + 8 * STEP_UNKNOWN,
        EXIT_BASE + 128,
        i64::from(u32::MAX),
    ] {
        assert!(parse_code(bad).is_err(), "{bad}");
    }
    // A report with an unknown step is never read back as ok.
    let unknown = report_code(&Report {
        step: "format-c",
        ..Report::default()
    });
    assert!(parse_code(unknown).is_err());
}

/// Rule 1253: the outer PowerShell's line is untrusted text, read strictly: one `flare-sharing=` line with the exit
/// code in digits, or `declined`; a BOM, `\r` and blank lines around it are fine; anything else is an error.
#[test]
fn the_outer_line_is_read_strictly() {
    let ok = format!("\u{feff}\r\nflare-sharing={}\r\n\r\n", EXIT_BASE + 7);
    assert!(
        matches!(parse_outcome(&ok), Ok(Elevated::Done(r)) if r.ok && r.installed && r.started && r.rule)
    );
    assert_eq!(
        parse_outcome("flare-sharing=declined\n"),
        Ok(Elevated::Declined)
    );
    let good = EXIT_BASE.to_string();
    for bad in [
        String::new(),
        "\n\n".into(),
        format!("flare-sharing={good}\nflare-sharing={good}\n"),
        format!("flare-sharing= {good}"),
        format!(" flare-sharing={good}"),
        format!("flare-sharing={good} "),
        format!("flare-sharing=+{good}"),
        format!("flare-sharing=-{good}"),
        "flare-sharing=".into(),
        "flare-sharing=0x4600".into(),
        "flare-sharing=1".into(),
        "flare-sharing=99999999999999999999".into(),
        format!("ok=1\nflare-sharing={good}"),
        format!("sharing={good}"),
        "flare-sharing=Declined".into(),
        format!("flare-sharing={good}\n{}", "x".repeat(OUTCOME_BYTES_MAX)),
    ] {
        assert!(parse_outcome(&bad).is_err(), "{bad:?}");
    }
    // An answer that cannot be read is a failed setup with the plain line, and nothing is saved.
    let os = FakeOs::new(Platform::Windows);
    *os.elevated.lock().unwrap() = Ok("flare-sharing=1\n".into());
    let mut r = Rig::new(os, PrefStore::memory());
    r.sharing.switch(true);
    r.settle(|s| s.ask().is_some());
    assert_eq!(
        r.sharing.ask(),
        Some(&SharingAsk::Notice(
            "Computer sharing could not be set up.".into()
        ))
    );
    assert!(!prefs::sharing_on(&r.prefs) && !prefs::firewall_rule(&r.prefs));
}

/// Rule 1255: the off script cleans only what it is asked to: the admin keys Flare added (`flare-pair:` lines), the
/// server, and always Flare's two firewall rules.
#[test]
fn the_windows_off_script_removes_only_what_flare_added() {
    let both = windows_off_script(true, true);
    for want in [
        "administrators_authorized_keys",
        "Where-Object { $_ -notmatch '\\sflare-pair:' }",
        "Stop-Service -Name 'sshd'",
        "Set-Service -Name 'sshd' -StartupType Manual",
    ] {
        assert!(both.contains(want), "{want}\n{both}");
    }
    let keys_only = windows_off_script(false, true);
    assert!(keys_only.contains("administrators_authorized_keys"));
    assert!(!keys_only.contains("Stop-Service"));
    let stop_only = windows_off_script(true, false);
    assert!(stop_only.contains("Stop-Service -Name 'sshd'"));
    assert!(!stop_only.contains("administrators_authorized_keys"));
    let rules_only = windows_off_script(false, false);
    assert!(
        !rules_only.contains("Stop-Service")
            && !rules_only.contains("administrators_authorized_keys")
    );
    for s in [&both, &keys_only, &stop_only, &rules_only] {
        assert!(s.contains("Remove-NetFirewallRule -Name 'Flare-Pairing-In'"));
        assert!(s.contains("Remove-NetFirewallRule -Name 'Flare-Pairing-mDNS-In'"));
        assert!(!s.contains("Remove-WindowsCapability"), "never uninstalls");
        assert!(
            !s.contains("Remove-NetFirewallRule -Name 'OpenSSH"),
            "the server's own rule stays"
        );
    }
}

/// Rule 1253: the elevation is one hidden PowerShell that starts an elevated PowerShell with `-Verb RunAs` (the UAC
/// prompt), waits, and prints one line with its exit code (or `declined` when Start-Process fails). The script travels
/// inline as that PowerShell's `-EncodedCommand`: no script file exists that another program could swap before the
/// prompt is answered.
#[test]
fn the_elevation_call_runs_the_script_inline_with_runas_and_waits() {
    let script = windows_on_script(Path::new(
        r"C:\Users\Jo O'Neil\AppData\Local\Flare\current\Flare.exe",
    ));
    let argv = elevation_argv(&script).unwrap();
    assert_eq!(
        argv[..6],
        [
            "powershell.exe",
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command"
        ]
    );
    let command = argv.last().unwrap();
    let (head, rest) = command
        .split_once("'-EncodedCommand','")
        .expect("the script travels encoded");
    assert_eq!(
        head,
        "try { $p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ErrorAction Stop -ArgumentList @('-NoProfile','-NonInteractive','-WindowStyle','Hidden',"
    );
    let (inner, tail) = rest.split_once('\'').unwrap();
    assert_eq!(
        tail,
        "); 'flare-sharing=' + $p.ExitCode } catch { 'flare-sharing=declined' }"
    );
    // The outer command holds only fixed words and base64: nothing a path could break.
    assert!(
        inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c))
    );
    assert!(!command.contains('"') && !command.contains(".ps1") && !command.contains("'-File'"));
    // The script that runs is the script, without its comments and indent, with the person's path intact.
    let ran = decode(inner);
    assert_eq!(ran, ps_compact(&script));
    assert!(
        ran.contains("-Program 'C:\\Users\\Jo O''Neil\\AppData\\Local\\Flare\\current\\Flare.exe'")
    );
    assert!(
        !ran.lines()
            .any(|l| l.starts_with('#') || l.starts_with(' '))
    );
}

/// Rule 1253: Windows takes a command line of at most 32 767 characters. Every script, with a long path full of
/// letters beyond ASCII, stays well under it (the elevated PowerShell's own command line is shorter still); one that
/// would not fit is refused before anything runs.
#[test]
fn the_elevated_command_line_stays_under_windows_limit() {
    let long = format!(r"C:\Users\{}\AppData\Local", "Jø O'Néil ".repeat(20));
    let exe = PathBuf::from(format!(r"{long}\Flare\current\Flare.exe"));
    for script in [windows_on_script(&exe), windows_off_script(true, true)] {
        let argv = elevation_argv(&script).unwrap();
        let len = command_line_len(&argv);
        assert!(len < COMMAND_LINE_MAX / 2, "{len}");
    }
    assert!(elevation_argv(&"$a = 1\n".repeat(4000)).is_err());
    assert_eq!(command_line_len(&["ab".into(), "c".into()]), 9);
}

/// Rule 1254: the Mac's commands go through osascript's administrator prompt, quoted for AppleScript. Turning Remote
/// Login on writes Flare's keys-only file first, in the same root command, and takes it away again when macOS refused;
/// turning it off removes the file only once Remote Login is off.
#[test]
fn the_mac_runs_systemsetup_through_the_administrator_prompt() {
    let on = mac_on_command();
    let off = mac_off_command();
    assert_eq!(
        on,
        "/bin/mkdir -p /etc/ssh/sshd_config.d && /usr/bin/printf '%s\\n' '# Flare Computer sharing: keys only' \
         'PasswordAuthentication no' 'KbdInteractiveAuthentication no' > /etc/ssh/sshd_config.d/000-flare.conf \
         && /usr/sbin/systemsetup -setremotelogin on; /usr/sbin/systemsetup -getremotelogin \
         | /usr/bin/grep -q 'Remote Login: On' || /bin/rm -f /etc/ssh/sshd_config.d/000-flare.conf"
    );
    let file = on.find(&format!("> {MAC_KEYS_FILE}")).unwrap();
    assert!(
        file < on.find("-setremotelogin on").unwrap(),
        "keys only before the server starts"
    );
    assert!(on.contains(&format!("'{SSHD_MARK}'")));
    for line in MAC_KEYS_ONLY {
        assert!(on.contains(&format!("'{line}'")), "{line}");
    }
    assert!(!on.contains(" yes"));
    assert_eq!(MAC_KEYS_FILE, "/etc/ssh/sshd_config.d/000-flare.conf");
    // Through osascript: the `\n` of printf survives AppleScript's own string (`\\n` there).
    let argv = osascript_argv(&on);
    assert_eq!(argv[..2], ["/usr/bin/osascript", "-e"]);
    assert!(argv[2].starts_with("do shell script \"/bin/mkdir -p /etc/ssh/sshd_config.d && "));
    assert!(argv[2].contains("printf '%s\\\\n' '# Flare"));
    assert!(argv[2].ends_with("000-flare.conf\" with administrator privileges"));
    assert_eq!(
        osascript_argv("a \"b\" \\c")[2],
        "do shell script \"a \\\"b\\\" \\\\c\" with administrator privileges"
    );
    assert_eq!(
        off,
        "/usr/sbin/systemsetup -f -setremotelogin off; /usr/sbin/systemsetup -getremotelogin \
         | /usr/bin/grep -q 'Remote Login: Off' && /bin/rm -f /etc/ssh/sshd_config.d/000-flare.conf"
    );
    assert!(off.find("-setremotelogin off").unwrap() < off.find("/bin/rm -f").unwrap());
    assert_eq!(
        SHARING_SETTINGS_URL,
        "x-apple.systempreferences:com.apple.Sharing-Settings.extension"
    );
    assert!(mac_declined("execution error: User canceled. (-128)"));
    assert!(!mac_declined(
        "Turning Remote Login on or off requires Full Disk Access privileges."
    ));
}

/// Rule 1254: `socketfilterfw --getblockall` is read in each wording macOS has used.
#[test]
fn the_mac_firewall_block_all_answer_is_read() {
    for yes in [
        "Firewall has block all state set to enabled.\n",
        "Block all ENABLED! \n",
        "Firewall is set to block all non-essential incoming connections\n",
    ] {
        assert!(blocks_all(yes), "{yes}");
    }
    for no in [
        "Firewall has block all state set to disabled.\n",
        "Block all DISABLED! \n",
        "",
        "socketfilterfw: unknown option",
    ] {
        assert!(!blocks_all(no), "{no}");
    }
    assert_eq!(
        SOCKETFILTERFW,
        "/usr/libexec/ApplicationFirewall/socketfilterfw"
    );
    assert!(!MAC_BLOCK_ALL_LINE.contains('\u{2014}'));
}

#[test]
fn a_failed_step_is_one_plain_line() {
    let r = parse_code(EXIT_BASE + 8).unwrap();
    assert!(!r.ok);
    assert_eq!(r.step, "install");
    assert_eq!(
        step_line(r.step),
        "Windows could not add its SSH server. Check the internet connection and try again."
    );
    for step in STEPS.into_iter().chain(["x"]) {
        assert!(!step_line(step).contains('\u{2014}'));
        assert!(step == "x" || step_line(step) != step_line(""), "{step}");
    }
}

/// Rule 1255: only the lines with Flare's comment go; the person's own keys, comments and blank lines stay.
#[test]
fn only_the_keys_flare_added_are_removed() {
    let text = "ssh-ed25519 AAAAmine max@desk\n\
                # flare-pair:not-a-key\n\
                ssh-ed25519 AAAAflare flare-pair:Max's Mac mini\n\
                \n\
                restrict ssh-ed25519 AAAAtwo flare-pair:Office\n\
                ssh-rsa AAAAthree other\n";
    let (kept, n) = strip_flare_keys(text);
    assert_eq!(n, 2);
    assert_eq!(
        kept,
        "ssh-ed25519 AAAAmine max@desk\n# flare-pair:not-a-key\n\nssh-rsa AAAAthree other\n"
    );
    assert_eq!(
        strip_flare_keys("no newline at end"),
        ("no newline at end".to_string(), 0)
    );
}

// ---- the switch -----------------------------------------------------------------------------------------------------

/// Rules 1252 and 1253: on Windows the switch runs the elevated setup, stays on (dimmed) while it runs, and once it
/// finished is on, saved, announced, and remembers that Flare turned the server on.
#[test]
fn windows_turns_on_with_one_elevated_script_and_the_switch_is_saved() {
    let os = FakeOs::new(Platform::Windows);
    *os.elevated.lock().unwrap() = Ok(ok_outcome(true, true, true));
    let mut r = Rig::new(os, PrefStore::memory());
    assert_eq!(on(r.sharing.info()), (false, false));
    r.sharing.switch(true);
    assert_eq!(
        on(r.sharing.info()),
        (true, true),
        "on and dimmed while it runs"
    );
    r.settle_phase(Phase::On);
    assert_eq!(on(r.sharing.info()), (true, false));
    assert!(prefs::sharing_on(&r.prefs) && prefs::server_by_flare(&r.prefs));
    assert!(
        prefs::firewall_rule(&r.prefs),
        "Flare's rules are saved, so turning off removes them"
    );
    assert_eq!(*r.os.announced.lock().unwrap(), [true]);
    let scripts = r.os.scripts.lock().unwrap().clone();
    assert_eq!(scripts.len(), 1, "one elevated script, one prompt");
    assert!(scripts[0].contains("Add-WindowsCapability"));
    assert!(r.os.admin_cmds.lock().unwrap().is_empty());
    // A restart: the switch is on again, and announced.
    let again = Rig::new(FakeOs::new(Platform::Windows), r.prefs.clone());
    assert_eq!(on(again.sharing.info()), (true, false));
    assert_eq!(*again.os.announced.lock().unwrap(), [true]);
}

/// Rule 1253: a declined UAC prompt puts the switch back off with nothing to say; a failed step is the Notice dialog
/// with its plain line, and nothing is saved.
#[test]
fn a_declined_or_failed_windows_setup_leaves_the_switch_off() {
    let os = FakeOs::new(Platform::Windows);
    *os.elevated.lock().unwrap() = Ok("flare-sharing=declined\r\n".into());
    let mut r = Rig::new(os, PrefStore::memory());
    r.sharing.switch(true);
    r.settle(|s| s.phase() == Phase::Off);
    assert_eq!(on(r.sharing.info()), (false, false));
    assert_eq!(r.sharing.ask(), None);
    *r.os.elevated.lock().unwrap() = Ok(failed("start"));
    r.sharing.switch(true);
    r.settle(|s| s.ask().is_some());
    assert_eq!(r.sharing.phase(), Phase::Off);
    let ask = r.sharing.ask().cloned().unwrap();
    assert_eq!(
        ask,
        SharingAsk::Notice("Windows' SSH server would not start.".into())
    );
    let words = ask_words(&ask);
    assert_eq!(
        (words.title, words.cancel, words.primary),
        ("Computer sharing", "", "OK")
    );
    assert_eq!(
        on(r.sharing.info()),
        (false, true),
        "the switch is deaf while the dialog is up"
    );
    r.sharing.switch(true);
    assert_eq!(
        r.os.scripts.lock().unwrap().len(),
        2,
        "a press under the dialog does nothing"
    );
    r.sharing.button(SHARING_PRIMARY);
    assert_eq!(r.sharing.ask(), None);
    assert!(!prefs::sharing_on(&r.prefs));
    assert!(r.os.announced.lock().unwrap().is_empty());
}

/// Rule 1254: a Mac whose port 22 answers is on at once, with no prompt.
#[test]
fn a_mac_with_remote_login_on_needs_no_prompt() {
    let mut r = Rig::new(
        FakeOs::new(Platform::Mac).ports(&[true]),
        PrefStore::memory(),
    );
    r.sharing.switch(true);
    r.settle_phase(Phase::On);
    assert!(r.os.admin_cmds.lock().unwrap().is_empty());
    assert!(!prefs::server_by_flare(&r.prefs), "the person's own server");
}

/// Rule 1254: the administrator prompt turns Remote Login on; Cancel there puts the switch back off.
#[test]
fn a_mac_asks_for_the_password_and_cancel_puts_the_switch_back() {
    let mut r = Rig::new(
        FakeOs::new(Platform::Mac).ports(&[false, false, true]),
        PrefStore::memory(),
    );
    r.sharing.switch(true);
    r.settle_phase(Phase::On);
    assert_eq!(*r.os.admin_cmds.lock().unwrap(), [mac_on_command()]);
    assert!(prefs::server_by_flare(&r.prefs));
    let mut r = Rig::new(FakeOs::new(Platform::Mac), PrefStore::memory());
    *r.os.admin.lock().unwrap() = Err("execution error: User canceled. (-128)".into());
    r.sharing.switch(true);
    r.settle(|s| s.phase() == Phase::Off);
    assert_eq!(r.sharing.ask(), None);
}

/// Rule 1254: when macOS refuses (no Full Disk Access), the "Turn on Remote Login" dialog is up with its spinner line;
/// Open System Settings opens Sharing; the switch carries on by itself the moment port 22 answers.
#[test]
fn a_refused_mac_shows_turn_on_remote_login_and_continues_by_itself() {
    let os = FakeOs::new(Platform::Mac);
    *os.admin.lock().unwrap() =
        Ok("Turning Remote Login on or off requires Full Disk Access privileges.".into());
    let mut r = Rig::new(os, PrefStore::memory());
    r.sharing.switch(true);
    r.settle(|s| s.phase() == Phase::WaitingForRemoteLogin);
    assert_eq!(r.sharing.ask(), Some(&SharingAsk::RemoteLogin));
    assert_eq!(on(r.sharing.info()), (true, true));
    let w = ask_words(&SharingAsk::RemoteLogin);
    assert_eq!(
        (w.title, w.line.as_str(), w.spin, w.cancel, w.primary),
        (
            "Turn on Remote Login",
            "Flare continues when it is on",
            true,
            "Cancel",
            "Open System Settings"
        )
    );
    r.sharing.button(SHARING_PRIMARY);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while r.os.urls.lock().unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2)); // sync-ok: a test waits for its worker
    }
    assert_eq!(*r.os.urls.lock().unwrap(), [SHARING_SETTINGS_URL]);
    assert_eq!(
        r.sharing.ask(),
        Some(&SharingAsk::RemoteLogin),
        "the dialog waits"
    );
    r.os.open_port();
    r.settle_phase(Phase::On);
    assert_eq!(r.sharing.ask(), None, "the dialog closes by itself");
    assert!(prefs::sharing_on(&r.prefs));
    assert!(!prefs::server_by_flare(&r.prefs), "the person turned it on");
    assert_eq!(*r.os.announced.lock().unwrap(), [true]);
}

/// Rule 1254: Cancel in "Turn on Remote Login" ends the watch and the switch goes back off; a late answer is ignored.
#[test]
fn cancel_in_turn_on_remote_login_puts_the_switch_back_off() {
    let os = FakeOs::new(Platform::Mac);
    *os.admin.lock().unwrap() = Err("-99".into());
    let mut r = Rig::new(os, PrefStore::memory());
    r.sharing.switch(true);
    r.settle(|s| s.phase() == Phase::WaitingForRemoteLogin);
    r.sharing.dismiss();
    assert_eq!(r.sharing.phase(), Phase::Off);
    assert_eq!(on(r.sharing.info()), (false, false));
    assert!(!r.sharing.on_msg(SharingMsg::RemoteLoginSeen(1, None)));
    assert!(!prefs::sharing_on(&r.prefs));
}

/// Rule 1255: turning off stops the announcement and saves off at once, removes Flare's keys, and offers to stop the
/// server only when Flare turned it on: Stop it runs the elevated clean up, and the server is no longer Flare's.
#[test]
fn turning_off_removes_flares_keys_and_offers_to_stop_only_flares_server() {
    let prefs_on = || {
        let p = PrefStore::memory();
        prefs::set_sharing_on(&p, true);
        p
    };
    // Not Flare's server: no question, no prompt, the user's keys cleaned.
    let mut r = Rig::new(FakeOs::new(Platform::Windows), prefs_on());
    r.sharing.switch(false);
    assert_eq!(*r.os.announced.lock().unwrap(), [true, false]);
    assert!(!prefs::sharing_on(&r.prefs), "off at once");
    assert_eq!(r.sharing.ask(), None);
    r.settle_phase(Phase::Off);
    assert_eq!(*r.os.stripped.lock().unwrap(), 1);
    assert!(
        r.os.scripts.lock().unwrap().is_empty(),
        "nothing to elevate for"
    );
    // Flare's server: the question; Stop it runs the clean up with the stop.
    let p = prefs_on();
    prefs::set_server_by_flare(&p, true);
    prefs::set_admin_keys(&p, true);
    let mut r = Rig::new(FakeOs::new(Platform::Windows), p);
    r.sharing.switch(false);
    assert_eq!(r.sharing.phase(), Phase::AskingStop);
    assert_eq!(r.sharing.ask(), Some(&SharingAsk::StopServer));
    let w = ask_words(&SharingAsk::StopServer);
    assert_eq!((w.cancel, w.primary), ("Keep it", "Stop it"));
    assert_eq!(on(r.sharing.info()), (false, true));
    r.sharing.button(SHARING_PRIMARY);
    r.settle_phase(Phase::Off);
    let scripts = r.os.scripts.lock().unwrap().clone();
    assert_eq!(scripts.len(), 1);
    assert!(scripts[0].contains("Stop-Service -Name 'sshd'"));
    assert!(scripts[0].contains("administrators_authorized_keys"));
    assert!(!prefs::server_by_flare(&r.prefs) && !prefs::admin_keys(&r.prefs));
    // Keep it: the user's keys go, the server stays Flare's (the next off asks again), nothing elevated.
    let p = prefs_on();
    prefs::set_server_by_flare(&p, true);
    let mut r = Rig::new(FakeOs::new(Platform::Windows), p);
    r.sharing.switch(false);
    r.sharing.button(SHARING_CANCEL);
    r.settle_phase(Phase::Off);
    assert!(r.os.scripts.lock().unwrap().is_empty());
    assert_eq!(*r.os.stripped.lock().unwrap(), 1);
    assert!(prefs::server_by_flare(&r.prefs));
}

/// Rule 1255: Flare's firewall rules alone are reason enough for the elevated clean up: a server that was the
/// person's (nothing to stop, no admin keys) still loses Flare's rules, and the pref clears once they are gone. When
/// the clean up fails, the pref stays, so the next off tries again.
#[test]
fn turning_off_removes_flares_firewall_rules_even_when_the_server_is_not_flares() {
    let p = PrefStore::memory();
    prefs::set_sharing_on(&p, true);
    prefs::set_firewall_rule(&p, true);
    let mut r = Rig::new(FakeOs::new(Platform::Windows), p);
    r.sharing.switch(false);
    assert_eq!(
        r.sharing.ask(),
        None,
        "no question: the server is not Flare's"
    );
    r.settle_phase(Phase::Off);
    let scripts = r.os.scripts.lock().unwrap().clone();
    assert_eq!(scripts.len(), 1, "one elevated clean up");
    assert!(scripts[0].contains("Remove-NetFirewallRule -Name 'Flare-Pairing-In'"));
    assert!(!scripts[0].contains("Stop-Service"));
    assert!(!scripts[0].contains("administrators_authorized_keys"));
    assert!(!prefs::firewall_rule(&r.prefs));
    // A declined prompt keeps the pref and says what is left.
    let p = PrefStore::memory();
    prefs::set_sharing_on(&p, true);
    prefs::set_firewall_rule(&p, true);
    let os = FakeOs::new(Platform::Windows);
    *os.elevated.lock().unwrap() = Ok("flare-sharing=declined\n".into());
    let mut r = Rig::new(os, p);
    r.sharing.switch(false);
    r.settle(|s| s.ask().is_some());
    assert!(prefs::firewall_rule(&r.prefs));
    assert_eq!(
        turn_off(
            &FakeOs::new(Platform::Windows),
            OffWork {
                firewall: true,
                ..OffWork::default()
            }
        ),
        Ok(())
    );
}

/// Rule 1255: a Mac whose Remote Login Flare turned on is stopped through the administrator prompt; when it stays on,
/// the Notice says where to turn it off.
#[test]
fn a_mac_stops_remote_login_and_says_so_when_it_stays_on() {
    let p = PrefStore::memory();
    prefs::set_sharing_on(&p, true);
    prefs::set_server_by_flare(&p, true);
    let mut r = Rig::new(FakeOs::new(Platform::Mac).ports(&[false]), p.clone());
    r.sharing.switch(false);
    r.sharing.button(SHARING_PRIMARY);
    r.settle_phase(Phase::Off);
    assert_eq!(*r.os.admin_cmds.lock().unwrap(), [mac_off_command()]);
    assert_eq!(r.sharing.ask(), None);
    prefs::set_sharing_on(&p, true);
    prefs::set_server_by_flare(&p, true);
    let os = FakeOs::new(Platform::Mac);
    os.open_port();
    let mut r = Rig::new(os, p);
    r.sharing.switch(false);
    r.sharing.button(SHARING_PRIMARY);
    r.settle(|s| s.ask().is_some());
    assert_eq!(
        r.sharing.ask(),
        Some(&SharingAsk::Notice(
            "Remote Login is still on. Turn it off in System Settings, General, Sharing.".into()
        ))
    );
    assert!(!prefs::sharing_on(&r.prefs), "sharing is off all the same");
}

/// A build with no setup (scenes) says where sharing works instead of pretending.
#[test]
fn a_build_with_no_setup_says_where_sharing_works() {
    let mut r = Rig::new(FakeOs::new(Platform::Other), PrefStore::memory());
    r.sharing.switch(true);
    r.settle(|s| s.ask().is_some());
    assert_eq!(
        r.sharing.ask(),
        Some(&SharingAsk::Notice(
            "Computer sharing works on Windows and macOS.".into()
        ))
    );
}

/// Rule 1254: port 22 answering on 127.0.0.1 is not enough when the Mac firewall blocks all incoming connections:
/// sharing does not count as working, the dialog says where to turn that off, and nothing is saved. Asked with no root,
/// with Remote Login already on, and before the password prompt otherwise, so Flare never turns on a server nobody can
/// reach.
#[test]
fn a_mac_firewall_that_blocks_all_stops_sharing_with_one_line() {
    let blocked = || {
        let os = FakeOs::new(Platform::Mac);
        *os.block_all.lock().unwrap() = Ok("Firewall has block all state set to enabled.\n".into());
        os
    };
    // Remote Login on already.
    let mut r = Rig::new(blocked().ports(&[true]), PrefStore::memory());
    r.sharing.switch(true);
    r.settle(|s| s.ask().is_some());
    assert_eq!(
        r.sharing.ask(),
        Some(&SharingAsk::Notice(MAC_BLOCK_ALL_LINE.into()))
    );
    assert_eq!(r.sharing.phase(), Phase::Off);
    assert_eq!(*r.os.block_all_asked.lock().unwrap(), 1);
    assert!(r.os.admin_cmds.lock().unwrap().is_empty());
    assert!(!prefs::sharing_on(&r.prefs) && !prefs::server_by_flare(&r.prefs));
    assert!(r.os.announced.lock().unwrap().is_empty());
    assert_eq!(
        MAC_BLOCK_ALL_LINE,
        "The Mac firewall blocks all incoming connections. Turn that off in System Settings, Network, Firewall, \
         Options."
    );
    // Remote Login off: no password prompt at all.
    let mut r = Rig::new(blocked(), PrefStore::memory());
    r.sharing.switch(true);
    r.settle(|s| s.ask().is_some());
    assert_eq!(
        r.sharing.ask(),
        Some(&SharingAsk::Notice(MAC_BLOCK_ALL_LINE.into()))
    );
    assert!(
        r.os.admin_cmds.lock().unwrap().is_empty(),
        "never turned on"
    );
    // A firewall that does not block all, or a tool that does not answer, lets sharing on.
    let os = FakeOs::new(Platform::Mac).ports(&[true]);
    *os.block_all.lock().unwrap() = Err("no such file".into());
    let mut r = Rig::new(os, PrefStore::memory());
    r.sharing.switch(true);
    r.settle_phase(Phase::On);
    assert_eq!(*r.os.block_all_asked.lock().unwrap(), 1);
    // Windows never asks it.
    let mut r = Rig::new(FakeOs::new(Platform::Windows), PrefStore::memory());
    r.sharing.switch(true);
    r.settle_phase(Phase::On);
    assert_eq!(*r.os.block_all_asked.lock().unwrap(), 0);
}

/// Rule 1254: when the person turns Remote Login on by hand while "Turn on Remote Login" is up and the firewall blocks
/// all, the dialog becomes the one line instead, and the switch goes back off.
#[test]
fn remote_login_turned_on_by_hand_behind_a_block_all_firewall_is_not_sharing() {
    let os = FakeOs::new(Platform::Mac);
    *os.admin.lock().unwrap() =
        Ok("Turning Remote Login on or off requires Full Disk Access privileges.".into());
    let mut r = Rig::new(os, PrefStore::memory());
    r.sharing.switch(true);
    r.settle(|s| s.phase() == Phase::WaitingForRemoteLogin);
    *r.os.block_all.lock().unwrap() = Ok("Block all ENABLED! \n".into());
    r.os.open_port();
    r.settle(|s| s.phase() == Phase::Off);
    assert_eq!(
        r.sharing.ask(),
        Some(&SharingAsk::Notice(MAC_BLOCK_ALL_LINE.into()))
    );
    assert!(!prefs::sharing_on(&r.prefs));
    assert!(r.os.announced.lock().unwrap().is_empty());
    r.sharing.button(SHARING_PRIMARY);
    assert_eq!(on(r.sharing.info()), (false, false));
}

/// Rule 1255: Cancel at macOS' password prompt while turning off says plainly that macOS did not allow the change, at
/// once (no 2.5 s look at the port), and the server stays Flare's, so the next turn off asks again.
#[test]
fn cancel_at_the_mac_password_prompt_when_turning_off_says_so_at_once() {
    let p = PrefStore::memory();
    prefs::set_sharing_on(&p, true);
    prefs::set_server_by_flare(&p, true);
    let os = FakeOs::new(Platform::Mac).ports(&[true, true, true]);
    *os.admin.lock().unwrap() = Err("execution error: User canceled. (-128)".into());
    let mut r = Rig::new(os, p);
    r.sharing.switch(false);
    r.sharing.button(SHARING_PRIMARY);
    r.settle(|s| s.ask().is_some());
    assert_eq!(
        r.sharing.ask(),
        Some(&SharingAsk::Notice(MAC_OFF_DECLINED_LINE.into()))
    );
    assert_eq!(
        MAC_OFF_DECLINED_LINE,
        "macOS did not allow the change, so Remote Login stays on."
    );
    assert_eq!(*r.os.admin_cmds.lock().unwrap(), [mac_off_command()]);
    assert_eq!(
        r.os.port.lock().unwrap().len(),
        3,
        "the port is never looked at"
    );
    assert!(prefs::server_by_flare(&r.prefs));
    assert!(!prefs::sharing_on(&r.prefs), "sharing is off all the same");
}

/// Rule 1253: PowerShell that did not start at all is a declined setup too: the switch goes back off with nothing to
/// say.
#[test]
fn powershell_that_did_not_start_leaves_the_switch_off() {
    let os = FakeOs::new(Platform::Windows);
    *os.elevated.lock().unwrap() = Err("PowerShell did not start".into());
    let mut r = Rig::new(os, PrefStore::memory());
    r.sharing.switch(true);
    r.settle(|s| s.phase() == Phase::Off);
    assert_eq!(r.sharing.ask(), None);
    // A failed config step (the folder check) is its own plain line.
    *r.os.elevated.lock().unwrap() = Ok(failed("config"));
    r.sharing.switch(true);
    r.settle(|s| s.ask().is_some());
    assert_eq!(
        r.sharing.ask(),
        Some(&SharingAsk::Notice(step_line("config").into()))
    );
}
