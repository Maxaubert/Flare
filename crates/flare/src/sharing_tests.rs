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

fn decode(argv: &[String]) -> String {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(argv.last().unwrap())
        .unwrap();
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).unwrap()
}

// ---- the scripts and the calls, as text ---------------------------------------------------------------------------

/// Rule 1253: the Windows script installs the server only when missing, starts it and sets it Automatic, scopes the
/// firewall to the Private profile, turns password logins off only on a server it installed (never on), and reports
/// through the result file.
#[test]
fn the_windows_script_installs_starts_and_opens_the_private_profile_only() {
    let s = windows_on_script(
        Path::new(r"C:\Users\Jo O'Neil\AppData\Local\Temp\r.txt"),
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
        "if ($installed) {",
        "'^\\s*#?\\s*PasswordAuthentication\\s'",
        "$lines[$at] = 'PasswordAuthentication no'",
        "$lines.Insert($match, 'PasswordAuthentication no')",
        "Restart-Service -Name 'sshd'",
        "-LocalPort 22 -Profile Private",
        "if ($null -ne $rule) { Set-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -Profile Private -Enabled True }",
        "New-NetFirewallRule -Name 'Flare-Pairing-In' -DisplayName 'Flare pairing' -Enabled True -Direction Inbound \
         -Program 'C:\\Users\\Jo O''Neil\\AppData\\Local\\Flare\\current\\Flare.exe' -Action Allow -Profile Private",
        "$startType = [string]$svc.StartType",
        "if ($started) { Stop-Service -Name 'sshd' -ErrorAction SilentlyContinue }",
        "if ($null -ne $startType -and $startType -ne 'Automatic') { Set-Service -Name 'sshd' -StartupType $startType",
        "$result = 'C:\\Users\\Jo O''Neil\\AppData\\Local\\Temp\\r.txt'",
        "Set-Content -LiteralPath $result -Value $out -Encoding UTF8",
    ] {
        assert!(s.contains(want), "{want}\n{s}");
    }
    // Never a password login turned on, never a profile other than Private, never Flare's rule opened to all.
    assert!(!s.contains("PasswordAuthentication yes"));
    for profile in ["Public", "Domain", "Any"] {
        assert!(!s.contains(&format!("-Profile {profile}")), "{profile}");
    }
    assert_eq!(s.matches("-Profile Private").count(), 3);
    // A server Flare starts (installed now, or one the person had but left stopped) has its rule set to Private
    // before it starts: its rule may name every profile, and it was never running, so nobody relied on it.
    let restrict = s
        .find("Set-NetFirewallRule -Name 'OpenSSH-Server-In-TCP'")
        .unwrap();
    let start = s.find("Start-Service -Name 'sshd'").unwrap();
    assert!(restrict < start);
    let guard = &s[s.find("if ($svc.Status -ne 'Running') {").unwrap()..start];
    assert!(guard.contains("Set-NetFirewallRule"), "{guard}");
    // A rule Flare makes comes only after the passwords line: the server is never reachable with passwords on.
    let made = s
        .find("New-NetFirewallRule -Name 'OpenSSH-Server-In-TCP'")
        .unwrap();
    assert!(s.find("PasswordAuthentication no").unwrap() < made);
    assert!(!s.contains('\u{2014}'));
}

/// Rule 1255: the off script cleans only what it is asked to: the admin keys Flare added (`flare-pair:` lines), the
/// server, and always Flare's firewall rule.
#[test]
fn the_windows_off_script_removes_only_what_flare_added() {
    let r = Path::new(r"C:\T\r.txt");
    let both = windows_off_script(r, true, true);
    for want in [
        "administrators_authorized_keys",
        "Where-Object { $_ -notmatch '\\sflare-pair:' }",
        "Stop-Service -Name 'sshd'",
        "Set-Service -Name 'sshd' -StartupType Manual",
        "Remove-NetFirewallRule -Name 'Flare-Pairing-In'",
        "$result = 'C:\\T\\r.txt'",
    ] {
        assert!(both.contains(want), "{want}\n{both}");
    }
    let keys_only = windows_off_script(r, false, true);
    assert!(keys_only.contains("administrators_authorized_keys"));
    assert!(!keys_only.contains("Stop-Service"));
    let stop_only = windows_off_script(r, true, false);
    assert!(stop_only.contains("Stop-Service -Name 'sshd'"));
    assert!(!stop_only.contains("administrators_authorized_keys"));
    for s in [&both, &keys_only, &stop_only] {
        assert!(!s.contains("Remove-WindowsCapability"), "never uninstalls");
        assert!(
            !s.contains("Remove-NetFirewallRule -Name 'OpenSSH"),
            "the server's own rule stays"
        );
    }
}

/// Rule 1253: the elevation is one hidden PowerShell that starts the script's PowerShell with `-Verb RunAs` (the UAC
/// prompt) and waits; the command travels encoded, and the script's path is quoted whatever it holds.
#[test]
fn the_elevation_call_runs_the_script_with_runas_and_waits() {
    let argv = elevation_argv(Path::new(r"C:\Users\Jo O'Neil\Temp\flare-sharing-1.ps1"));
    assert_eq!(
        argv[..6],
        [
            "powershell.exe",
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-EncodedCommand"
        ]
    );
    let command = decode(&argv);
    assert_eq!(
        command,
        "$p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',\
         '\"C:\\Users\\Jo O''Neil\\Temp\\flare-sharing-1.ps1\"'); exit $p.ExitCode"
    );
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
    let r = parse_report("\u{feff}installed=1\r\nstarted=1\r\nok=1\r\n");
    assert!(r.ok && r.installed && r.started);
    let r = parse_report(
        "step=install\nerror=Add-WindowsCapability failed. 0x800f0954\ninstalled=0\nstarted=0\n",
    );
    assert!(!r.ok);
    assert_eq!(r.step, "install");
    assert_eq!(
        step_line(&r.step),
        "Windows could not add its SSH server. Check the internet connection and try again."
    );
    assert_eq!(parse_report(""), Report::default());
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
    *os.elevated.lock().unwrap() = Ok("installed=1\nstarted=1\nok=1\n".into());
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
    *r.os.elevated.lock().unwrap() = Ok("step=start\nerror=boom\ninstalled=0\nstarted=0\n".into());
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
