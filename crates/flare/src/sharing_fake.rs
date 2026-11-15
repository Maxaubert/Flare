//! A fake of the Computer sharing switch's OS calls, for the tests here and the ui tests: it records the scripts, the
//! admin commands and the URLs it was asked to run or open, and answers what the test set. Nothing is ever run.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{OUTCOME_KEY, Platform, Report, SSH_PORT, SharingOs, report_code};

/// The line the outer PowerShell prints for an elevated script that ended with `r` (the tests' way to answer).
pub(crate) fn outcome(r: &Report) -> String {
    format!("{OUTCOME_KEY}{}\r\n", report_code(r))
}

/// The line for a setup that went well, with these flags.
pub(crate) fn ok_outcome(installed: bool, started: bool, rule: bool) -> String {
    outcome(&Report {
        ok: true,
        installed,
        started,
        rule,
        step: "",
    })
}

/// What the fake OS was asked, and what it answers.
pub(crate) struct FakeOs {
    pub platform: Platform,
    /// Port 22: the answers in turn, then the last one for good.
    pub port: Mutex<VecDeque<bool>>,
    pub port_now: AtomicBool,
    /// What the outer PowerShell prints ([`outcome`]), or Err (PowerShell did not start).
    pub elevated: Mutex<Result<String, String>>,
    pub scripts: Mutex<Vec<String>>,
    pub admin: Mutex<Result<String, String>>,
    pub admin_cmds: Mutex<Vec<String>>,
    /// What `socketfilterfw --getblockall` prints, and how often it was asked.
    pub block_all: Mutex<Result<String, String>>,
    pub block_all_asked: Mutex<u32>,
    pub urls: Mutex<Vec<String>>,
    pub stripped: Mutex<u32>,
    pub announced: Mutex<Vec<bool>>,
}

impl FakeOs {
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            port: Mutex::new(VecDeque::new()),
            port_now: AtomicBool::new(false),
            elevated: Mutex::new(Ok(ok_outcome(false, false, false))),
            scripts: Mutex::default(),
            admin: Mutex::new(Ok(String::new())),
            admin_cmds: Mutex::default(),
            block_all: Mutex::new(Ok("Firewall has block all state set to disabled.\n".into())),
            block_all_asked: Mutex::new(0),
            urls: Mutex::default(),
            stripped: Mutex::new(0),
            announced: Mutex::default(),
        }
    }

    pub fn ports(self, answers: &[bool]) -> Self {
        *self.port.lock().unwrap() = answers.iter().copied().collect();
        self
    }

    /// Port 22 answers from now on (the person turned Remote Login on).
    pub fn open_port(&self) {
        self.port.lock().unwrap().clear();
        self.port_now.store(true, Ordering::SeqCst);
    }
}

impl SharingOs for FakeOs {
    fn platform(&self) -> Platform {
        self.platform
    }
    fn port_open(&self, port: u16) -> bool {
        assert_eq!(port, SSH_PORT);
        match self.port.lock().unwrap().pop_front() {
            Some(v) => {
                self.port_now.store(v, Ordering::SeqCst);
                v
            }
            None => self.port_now.load(Ordering::SeqCst),
        }
    }
    fn run_elevated(&self, script: &str) -> Result<String, String> {
        self.scripts.lock().unwrap().push(script.to_string());
        self.elevated.lock().unwrap().clone()
    }
    fn run_as_admin(&self, command: &str) -> Result<String, String> {
        self.admin_cmds.lock().unwrap().push(command.to_string());
        self.admin.lock().unwrap().clone()
    }
    fn firewall_block_all(&self) -> Result<String, String> {
        *self.block_all_asked.lock().unwrap() += 1;
        self.block_all.lock().unwrap().clone()
    }
    fn open_url(&self, url: &str) {
        self.urls.lock().unwrap().push(url.to_string());
    }
    fn strip_user_keys(&self) -> Result<usize, String> {
        *self.stripped.lock().unwrap() += 1;
        Ok(1)
    }
    fn announce(&self, on: bool) {
        self.announced.lock().unwrap().push(on);
    }
    fn wait(&self, _ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(1)); // sync-ok: a test's fake wait
    }
}
