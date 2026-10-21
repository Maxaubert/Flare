//! PtyProcess trait, SpawnSpec, PortablePty.

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    pub exe: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

pub trait PtyProcess: Send {
    fn pid(&self) -> u32;
    /// Once.
    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>>;
    /// Once.
    fn take_writer(&mut self) -> Option<Box<dyn Write + Send>>;
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String>;
    /// Ends the process (TerminateProcess). Never blocks. Safe to call twice.
    fn kill(&mut self);
    /// None while the process runs.
    fn exit_code(&mut self) -> Option<u32>;
    /// `cb` runs once on another thread when the process has exited (or at once, on this thread, if it already has).
    fn on_exit(&mut self, cb: Box<dyn FnOnce(Option<u32>) + Send>);
}

pub trait PtySpawner: Send + Sync {
    fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn PtyProcess>, String>;
}

/// Loads the bundled ConPTY first, then spawns through portable-pty.
pub struct PortableSpawner;

impl PtySpawner for PortableSpawner {
    fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn PtyProcess>, String> {
        crate::conpty::load_bundled().map_err(|e| e.to_string())?;
        if !std::path::Path::new(&spec.cwd).is_dir() {
            return Err(format!("The folder does not exist: {}", spec.cwd));
        }
        Ok(Box::new(PortablePty::spawn(spec)?))
    }
}

type ExitCb = Box<dyn FnOnce(Option<u32>) + Send>;

#[derive(Default)]
struct ExitState {
    done: bool,
    code: Option<u32>,
    cb: Option<ExitCb>,
}

pub struct PortablePty {
    master: Option<Box<dyn MasterPty + Send>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    pid: u32,
    exit: Arc<Mutex<ExitState>>,
    waiter: Option<JoinHandle<()>>,
}

impl PortablePty {
    pub fn spawn(spec: &SpawnSpec) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: spec.rows,
                cols: spec.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let mut cmd = CommandBuilder::new(&spec.exe);
        cmd.args(&spec.args);
        // portable-pty starts from the process and registry environment; Flare passes its own exactly.
        cmd.env_clear();
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        cmd.cwd(&spec.cwd);
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        let pid = child.process_id().unwrap_or(0);
        let killer = child.clone_killer();
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let exit = Arc::new(Mutex::new(ExitState::default()));
        let exit_w = Arc::clone(&exit);
        let waiter = std::thread::Builder::new()
            .name("flare-pty-wait".into())
            .spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code()); // sync-ok: the waiter thread of a session, ended when the process exits
                let cb = {
                    let mut st = exit_w.lock().unwrap_or_else(|p| p.into_inner());
                    st.done = true;
                    st.code = code;
                    st.cb.take()
                };
                if let Some(cb) = cb {
                    cb(code);
                }
            })
            .map_err(|e| e.to_string())?;
        // The slave handle is no longer needed here; the child holds its own.
        drop(pair.slave);
        Ok(PortablePty {
            master: Some(pair.master),
            killer,
            reader: Some(reader),
            writer: Some(writer),
            pid,
            exit,
            waiter: Some(waiter),
        })
    }
}

impl PtyProcess for PortablePty {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    fn take_writer(&mut self) -> Option<Box<dyn Write + Send>> {
        self.writer.take()
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        match &self.master {
            Some(m) => m
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| e.to_string()),
            None => Err("The terminal is closed".to_string()),
        }
    }

    fn kill(&mut self) {
        // Errors mean the process is already gone.
        let _ = self.killer.kill();
    }

    fn exit_code(&mut self) -> Option<u32> {
        let st = self.exit.lock().unwrap_or_else(|p| p.into_inner());
        st.code
    }

    fn on_exit(&mut self, cb: ExitCb) {
        let mut st = self.exit.lock().unwrap_or_else(|p| p.into_inner());
        if st.done {
            let code = st.code;
            drop(st);
            cb(code);
        } else {
            st.cb = Some(cb);
        }
    }
}

impl Drop for PortablePty {
    fn drop(&mut self) {
        // End the shell so the waiter returns, then close the pseudo console, which is what finally
        // ends a blocked read (the reader gets no end-of-file when the shell exits on its own).
        let _ = self.killer.kill();
        self.writer = None;
        self.master = None;
        if let Some(w) = self.waiter.take() {
            let _ = w.join(); // sync-ok: the process was just killed; wait returns at once
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    type Shared = Arc<Mutex<Vec<u8>>>;
    type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

    /// Reads into a shared buffer on a thread and answers the cursor query ConPTY sends at startup.
    fn pump(p: &mut Box<dyn PtyProcess>) -> (Shared, SharedWriter, JoinHandle<()>) {
        let mut reader = p.take_reader().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(p.take_writer().unwrap()));
        let buf: Shared = Arc::new(Mutex::new(Vec::new()));
        let (b2, w2) = (Arc::clone(&buf), Arc::clone(&writer));
        let h = std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = reader.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                b2.lock().unwrap().extend_from_slice(&chunk[..n]);
                if chunk[..n].windows(4).any(|w| w == b"\x1b[6n") {
                    let mut w = w2.lock().unwrap();
                    let _ = w.write_all(b"\x1b[1;1R");
                    let _ = w.flush();
                }
            }
        });
        (buf, writer, h)
    }

    fn count(buf: &Shared, needle: &str) -> usize {
        String::from_utf8_lossy(&buf.lock().unwrap())
            .matches(needle)
            .count()
    }

    fn wait_for(buf: &Shared, needle: &str, times: usize) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if count(buf, needle) >= times {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    fn std_env() -> Vec<(String, String)> {
        std::env::vars().collect()
    }

    fn cmd_spec(args: &[&str], cwd: &str, env: Vec<(String, String)>) -> SpawnSpec {
        SpawnSpec {
            exe: "cmd.exe".into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: cwd.into(),
            env,
            cols: 80,
            rows: 24,
        }
    }

    fn temp_cwd() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    struct Reap(Option<Box<dyn PtyProcess>>);
    impl Drop for Reap {
        fn drop(&mut self) {
            if let Some(p) = self.0.as_mut() {
                p.kill();
            }
        }
    }

    #[test]
    fn spawns_cmd_and_reads_its_banner() {
        let p = PortableSpawner
            .spawn(&cmd_spec(&[], &temp_cwd(), std_env()))
            .unwrap();
        let mut g = Reap(Some(p));
        let (buf, _w, _h) = pump(g.0.as_mut().unwrap());
        assert!(g.0.as_ref().unwrap().pid() > 0);
        assert!(wait_for(&buf, "Microsoft Windows", 1));
    }

    #[test]
    fn writes_reach_the_shell() {
        let p = PortableSpawner
            .spawn(&cmd_spec(&[], &temp_cwd(), std_env()))
            .unwrap();
        let mut g = Reap(Some(p));
        let (buf, w, _h) = pump(g.0.as_mut().unwrap());
        assert!(wait_for(&buf, "Microsoft Windows", 1));
        w.lock().unwrap().write_all(b"echo flare-m2-ok\r").unwrap();
        assert!(wait_for(&buf, "flare-m2-ok", 2));
    }

    #[test]
    fn exit_is_reported_without_an_eof() {
        let p = PortableSpawner
            .spawn(&cmd_spec(&[], &temp_cwd(), std_env()))
            .unwrap();
        let mut g = Reap(Some(p));
        let (buf, w, h) = pump(g.0.as_mut().unwrap());
        assert!(wait_for(&buf, "Microsoft Windows", 1));
        let (tx, rx) = std::sync::mpsc::channel();
        g.0.as_mut().unwrap().on_exit(Box::new(move |c| {
            let _ = tx.send(c);
        }));
        w.lock().unwrap().write_all(b"exit\r").unwrap();
        let code = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("exit callback");
        assert_eq!(code, Some(0));
        assert!(
            !h.is_finished(),
            "the reader must still be blocked, ConPTY sends no EOF"
        );
        drop(w);
        g.0 = None;
        let end = Instant::now() + Duration::from_secs(3);
        while !h.is_finished() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(h.is_finished(), "dropping the process ends the read");
    }

    #[test]
    fn kill_ends_the_process() {
        let p = PortableSpawner
            .spawn(&cmd_spec(&[], &temp_cwd(), std_env()))
            .unwrap();
        let mut g = Reap(Some(p));
        let (_buf, _w, _h) = pump(g.0.as_mut().unwrap());
        let p = g.0.as_mut().unwrap();
        p.kill();
        p.kill();
        let end = Instant::now() + Duration::from_secs(5);
        while p.exit_code().is_none() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(p.exit_code().is_some());
    }

    #[test]
    fn a_missing_folder_is_an_error_not_home() {
        let r = PortableSpawner.spawn(&cmd_spec(&[], "C:\\no\\such\\folder\\flare", std_env()));
        let Err(msg) = r else {
            panic!("expected an error")
        };
        assert!(msg.contains("folder"), "{msg}");
    }

    #[test]
    fn the_environment_is_exactly_what_was_given() {
        let pick = |k: &str| std::env::var(k).unwrap_or_default();
        let env = vec![
            ("PATH".to_string(), pick("PATH")),
            ("SystemRoot".to_string(), pick("SystemRoot")),
            ("FLARE_T".to_string(), "1".to_string()),
        ];
        let p = PortableSpawner
            .spawn(&cmd_spec(&["/c", "set"], &temp_cwd(), env))
            .unwrap();
        let mut g = Reap(Some(p));
        let (buf, _w, _h) = pump(g.0.as_mut().unwrap());
        assert!(wait_for(&buf, "FLARE_T=1", 1));
        // Give the rest of the listing time to arrive before checking the absence.
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(count(&buf, "USERPROFILE="), 0);
    }

    #[test]
    fn resize_is_accepted() {
        let p = PortableSpawner
            .spawn(&cmd_spec(&[], &temp_cwd(), std_env()))
            .unwrap();
        let mut g = Reap(Some(p));
        let (buf, _w, _h) = pump(g.0.as_mut().unwrap());
        assert!(wait_for(&buf, "Microsoft Windows", 1));
        assert!(g.0.as_mut().unwrap().resize(100, 30).is_ok());
    }
}
