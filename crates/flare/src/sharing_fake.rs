//! A fake of the Computer sharing switch's OS calls, for the tests here and the ui tests: it records the scripts, the
//! admin commands and the URLs it was asked to run or open, and answers what the test set. Nothing is ever run.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{Platform, SSH_PORT, SharingOs};

/// What the fake OS was asked, and what it answers.
pub(crate) struct FakeOs {
    pub platform: Platform,
    /// Port 22: the answers in turn, then the last one for good.
    pub port: Mutex<VecDeque<bool>>,
    pub port_now: AtomicBool,
    /// The elevated script's result file text, or Err (declined).
    pub elevated: Mutex<Result<String, String>>,
    pub scripts: Mutex<Vec<String>>,
    pub admin: Mutex<Result<String, String>>,
    pub admin_cmds: Mutex<Vec<String>>,
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
            elevated: Mutex::new(Ok("ok=1\ninstalled=0\nstarted=0\n".into())),
            scripts: Mutex::default(),
            admin: Mutex::new(Ok(String::new())),
            admin_cmds: Mutex::default(),
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
    fn run_elevated(&self, build: &dyn Fn(&Path) -> String) -> Result<String, String> {
        self.scripts
            .lock()
            .unwrap()
            .push(build(Path::new(r"C:\Temp\flare-sharing-1.txt")));
        self.elevated.lock().unwrap().clone()
    }
    fn run_as_admin(&self, command: &str) -> Result<String, String> {
        self.admin_cmds.lock().unwrap().push(command.to_string());
        self.admin.lock().unwrap().clone()
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
