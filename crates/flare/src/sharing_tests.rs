//! The Computer sharing switch (rules 1252 to 1255) against a fake OS: the scripts are read as text and the elevation
//! as its argv, and nothing is ever run.

use super::fake::FakeOs;
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

// ---- the scripts and the calls, as text ---------------------------------------------------------------------------

/// Rule 1253: the Windows script installs the server only when missing, starts it and sets it Automatic, scopes the
/// firewall to the Private profile, and reports through the result file.
#[test]
fn the_windows_script_installs_starts_and_opens_the_private_profile_only() {
    let s = windows_on_script(
        Path::new(r"C:\Users\Jo O'Neil\AppData\Local\Temp\flare-sharing-1\result.txt"),
        Path::new(r"C:\Users\Jo O'Neil\AppData\Local\Flare\current\Flare.exe"),
    );
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
        "$result = 'C:\\Users\\Jo O''Neil\\AppData\\Local\\Temp\\flare-sharing-1\\result.txt'",
        "$out.Add('installed=' + [int]$installed)",
        "$out.Add('started=' + [int]$started)",
        "$out.Add('rule=' + [int]$rule)",
    ] {
        assert!(s.contains(want), "{want}\n{s}");
    }
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
/// pairing port (picked at random, so the rule names the program) and UDP 5353 for the announcement. A failed setup
/// removes them again, and the report says whether they are in place.
#[test]
fn flares_own_firewall_rules_are_narrow_and_go_when_the_setup_fails() {
    let s = windows_on_script(
        Path::new(r"C:\T\f\result.txt"),
        Path::new(r"C:\Users\Jo O'Neil\AppData\Local\Flare\current\Flare.exe"),
    );
    let exe = "'C:\\Users\\Jo O''Neil\\AppData\\Local\\Flare\\current\\Flare.exe'";
    for want in [
        format!(
            "New-NetFirewallRule -Name 'Flare-Pairing-In' -DisplayName 'Flare pairing' -Enabled True -Direction Inbound \
             -Program {exe} -Protocol TCP -RemoteAddress LocalSubnet -Action Allow -Profile Private"
        ),
        format!(
            "New-NetFirewallRule -Name 'Flare-Pairing-mDNS-In' -DisplayName 'Flare pairing (mDNS)' -Enabled True \
             -Direction Inbound -Program {exe} -Protocol UDP -LocalPort 5353 -RemoteAddress LocalSubnet -Action Allow \
             -Profile Private"
        ),
    ] {
        assert!(s.contains(&want), "{want}\n{s}");
    }
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
/// keyboard-interactive logins off before it starts, at the top of `sshd_config` so they win over any later line or
/// Match block. A backup is kept, the person's other lines stay, a missing config starts from Windows' default in a
/// folder with sshd's own rights, and a failed step puts the config back. A server that already ran is left alone.
#[test]
fn a_server_flare_starts_takes_keys_only_and_a_running_one_is_left_alone() {
    let s = windows_on_script(Path::new(r"C:\T\f\result.txt"), Path::new(r"C:\F.exe"));
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
    // Flare's lines go first (sshd keeps the first value), so no Match block can come before them.
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

/// Rule 1253: the result goes only into the folder Flare made for this run, never through a link and never over a file
/// already there.
#[test]
fn the_elevated_scripts_write_the_result_only_into_a_plain_new_file() {
    let r = Path::new(r"C:\T\f\result.txt");
    for s in [
        windows_on_script(r, Path::new(r"C:\F.exe")),
        windows_off_script(r, true, true),
    ] {
        let tail = &s[s
            .find("$folder = [System.IO.Path]::GetDirectoryName($result)")
            .unwrap()..];
        for want in [
            "-not ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)",
            "-not (Test-Path -LiteralPath $result)",
            "Set-Content -LiteralPath $result -Value $out -Encoding UTF8",
        ] {
            assert!(tail.contains(want), "{want}");
        }
        assert_eq!(s.matches("Set-Content -LiteralPath $result").count(), 1);
    }
}

/// Rule 1255: the off script cleans only what it is asked to: the admin keys Flare added (`flare-pair:` lines), the
/// server, and always Flare's two firewall rules.
#[test]
fn the_windows_off_script_removes_only_what_flare_added() {
    let r = Path::new(r"C:\T\f\result.txt");
    let both = windows_off_script(r, true, true);
    for want in [
        "administrators_authorized_keys",
        "Where-Object { $_ -notmatch '\\sflare-pair:' }",
        "Stop-Service -Name 'sshd'",
        "Set-Service -Name 'sshd' -StartupType Manual",
        "$result = 'C:\\T\\f\\result.txt'",
    ] {
        assert!(both.contains(want), "{want}\n{both}");
    }
    let keys_only = windows_off_script(r, false, true);
    assert!(keys_only.contains("administrators_authorized_keys"));
    assert!(!keys_only.contains("Stop-Service"));
    let stop_only = windows_off_script(r, true, false);
    assert!(stop_only.contains("Stop-Service -Name 'sshd'"));
    assert!(!stop_only.contains("administrators_authorized_keys"));
    let rules_only = windows_off_script(r, false, false);
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
/// prompt) and waits. The script travels inline as that PowerShell's `-EncodedCommand`: no script file exists that
/// another program could swap before the prompt is answered.
#[test]
fn the_elevation_call_runs_the_script_inline_with_runas_and_waits() {
    let script = windows_on_script(
        Path::new(r"C:\Users\Jo O'Neil\AppData\Local\Temp\flare-sharing-1\result.txt"),
        Path::new(r"C:\Users\Jo O'Neil\AppData\Local\Flare\current\Flare.exe"),
    );
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
        "$p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList @('-NoProfile','-NonInteractive','-WindowStyle','Hidden',"
    );
    let (inner, tail) = rest.split_once('\'').unwrap();
    assert_eq!(tail, "); exit $p.ExitCode");
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
    assert!(ran.contains(
        "$result = 'C:\\Users\\Jo O''Neil\\AppData\\Local\\Temp\\flare-sharing-1\\result.txt'"
    ));
    assert!(
        !ran.lines()
            .any(|l| l.starts_with('#') || l.starts_with(' '))
    );
}

/// Rule 1253: Windows takes a command line of at most 32 767 characters. Every script, with long paths full of
/// letters beyond ASCII, stays well under it (the elevated PowerShell's own command line is shorter still); one
/// that would not fit is refused before anything runs.
#[test]
fn the_elevated_command_line_stays_under_windows_limit() {
    let long = format!(r"C:\Users\{}\AppData\Local\Temp", "Jø O'Néil ".repeat(20));
    let result = PathBuf::from(format!(
        r"{long}\flare-sharing-99999-1763100000000000000-7\result.txt"
    ));
    let exe = PathBuf::from(format!(r"{long}\Flare\current\Flare.exe"));
    for script in [
        windows_on_script(&result, &exe),
        windows_off_script(&result, true, true),
    ] {
        let argv = elevation_argv(&script).unwrap();
        let len = command_line_len(&argv);
        assert!(len < COMMAND_LINE_MAX / 2, "{len}");
    }
    assert!(elevation_argv(&"$a = 1\n".repeat(4000)).is_err());
    assert_eq!(command_line_len(&["ab".into(), "c".into()]), 9);
}

/// Rule 1254: the Mac's commands go through osascript's administrator prompt, quoted for AppleScript.
#[test]
fn the_mac_runs_systemsetup_through_the_administrator_prompt() {
    assert_eq!(
        osascript_argv(MAC_ON),
        [
            "/usr/bin/osascript",
            "-e",
            "do shell script \"/usr/sbin/systemsetup -setremotelogin on\" with administrator privileges"
        ]
    );
    assert_eq!(
        osascript_argv("a \"b\" \\c")[2],
        "do shell script \"a \\\"b\\\" \\\\c\" with administrator privileges"
    );
    assert_eq!(MAC_OFF, "/usr/sbin/systemsetup -f -setremotelogin off");
    assert_eq!(
        SHARING_SETTINGS_URL,
        "x-apple.systempreferences:com.apple.Sharing-Settings.extension"
    );
    assert!(mac_declined("execution error: User canceled. (-128)"));
    assert!(!mac_declined(
        "Turning Remote Login on or off requires Full Disk Access privileges."
    ));
}

#[test]
fn a_result_file_is_read_and_a_failed_step_is_one_plain_line() {
    let r = parse_report("\u{feff}ok=1\r\ninstalled=1\r\nstarted=1\r\nrule=1\r\n").unwrap();
    assert!(r.ok && r.installed && r.started && r.rule);
    let r = parse_report(
        "step=install\nerror=Add-WindowsCapability failed. 0x800f0954\ninstalled=0\nstarted=0\nrule=0\n",
    )
    .unwrap();
    assert!(!r.ok);
    assert_eq!(r.step, "install");
    assert_eq!(r.error, "Add-WindowsCapability failed. 0x800f0954");
    assert_eq!(
        step_line(&r.step),
        "Windows could not add its SSH server. Check the internet connection and try again."
    );
    for step in [
        "install",
        "start",
        "passwords",
        "firewall",
        "keys",
        "stop",
        "x",
    ] {
        assert!(!step_line(step).contains('\u{2014}'));
    }
}

/// Rule 1253: the result file is untrusted (another program of the same user could write it), so it is read strictly:
/// known keys once each, flags 0 or 1, a known step, ok or a failed step, a size cap; Windows' words are cut and
/// cleaned. Anything else is an error, never a half-read report.
#[test]
fn a_result_file_is_read_strictly() {
    for bad in [
        "",
        "\n\n",
        "installed=1\nstarted=1\n",
        "ok=1\nstep=start\n",
        "ok=yes\n",
        "ok=1\nok=1\n",
        "ok=1\nadmin=1\n",
        "ok=1\nrule=2\n",
        "ok=1\njunk\n",
        "step=format-c\n",
        " ok=1\n",
        "ok=0\n",
    ] {
        assert!(parse_report(bad).is_err(), "{bad:?}");
    }
    let huge = format!("ok=1\nerror={}\n", "x".repeat(REPORT_BYTES_MAX));
    assert!(parse_report(&huge).is_err());
    let r = parse_report(&format!(
        "step=start\nerror=a\u{1b}[31mb\u{7}{}\n",
        "y".repeat(1000)
    ))
    .unwrap();
    assert!(r.error.starts_with("a[31mb"));
    assert_eq!(r.error.chars().count(), REPORT_ERROR_MAX);
    assert!(!r.error.chars().any(char::is_control));
    // A result that cannot be read is a failed setup with the plain line, and nothing is saved.
    let os = FakeOs::new(Platform::Windows);
    *os.elevated.lock().unwrap() = Ok("ok=1\ninstalled=1\nadmin=1\n".into());
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
    *os.elevated.lock().unwrap() = Ok("ok=1\ninstalled=1\nstarted=1\nrule=1\n".into());
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
    *os.elevated.lock().unwrap() = Err("no answer".into());
    let mut r = Rig::new(os, PrefStore::memory());
    r.sharing.switch(true);
    r.settle(|s| s.phase() == Phase::Off);
    assert_eq!(on(r.sharing.info()), (false, false));
    assert_eq!(r.sharing.ask(), None);
    *r.os.elevated.lock().unwrap() =
        Ok("step=start\nerror=boom\ninstalled=0\nstarted=0\nrule=0\n".into());
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
    assert_eq!(*r.os.admin_cmds.lock().unwrap(), [MAC_ON]);
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
    assert!(!r.sharing.on_msg(SharingMsg::RemoteLoginSeen(1)));
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
    *os.elevated.lock().unwrap() = Err("declined".into());
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
    assert_eq!(*r.os.admin_cmds.lock().unwrap(), [MAC_OFF]);
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
