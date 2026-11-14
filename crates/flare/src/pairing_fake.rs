//! A fake network for the pairing tests: announcements, browses and connections between pages in one process, over
//! in-memory pipes (`pair_wire::pipe`). No socket is ever opened.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Closer, Duplex, Me, Nearby, PairListener, PairNet};
use crate::pair_wire::pipe;

impl Duplex for pipe::End {
    fn closer(&self) -> Closer {
        let me = self.waker();
        Box::new(move || {
            let _ = me.send(Vec::new());
        })
    }
}

type Queue = Arc<Mutex<VecDeque<(Box<dyn Duplex>, String)>>>;

/// The network all fake pages share: who listens on which port, and what each announced.
#[derive(Default)]
pub(crate) struct Hub {
    listeners: Mutex<HashMap<u16, Queue>>,
    next_port: Mutex<u16>,
}

/// One computer on the fake network: its address, what it announced and looked for.
pub(crate) struct FakeNet {
    pub hub: Arc<Hub>,
    pub address: String,
    /// The computers its browse finds (the test sets them).
    pub nearby: Mutex<Vec<Nearby>>,
    pub announced: Mutex<Vec<Option<(String, u16)>>>,
    pub browsing: Mutex<Vec<bool>>,
}

impl FakeNet {
    pub fn new(hub: &Arc<Hub>, address: &str) -> Self {
        Self {
            hub: Arc::clone(hub),
            address: address.into(),
            nearby: Mutex::default(),
            announced: Mutex::default(),
            browsing: Mutex::default(),
        }
    }

    /// The port this computer listens on now, as its last announcement said.
    pub fn port(&self) -> Option<u16> {
        self.announced
            .lock()
            .unwrap()
            .last()
            .cloned()
            .flatten()
            .map(|(_, p)| p)
    }
}

struct FakeListener {
    port: u16,
    queue: Queue,
}

impl PairListener for FakeListener {
    fn port(&self) -> u16 {
        self.port
    }
    fn accept(&self, wait: Duration) -> Option<(Box<dyn Duplex>, String)> {
        let got = self.queue.lock().unwrap().pop_front();
        if got.is_none() {
            std::thread::sleep(wait.min(Duration::from_millis(5))); // sync-ok: the fake listener's worker
        }
        got
    }
}

impl PairNet for FakeNet {
    fn announce(&self, me: Option<(&Me, u16)>) {
        self.announced
            .lock()
            .unwrap()
            .push(me.map(|(m, p)| (m.name.clone(), p)));
    }
    fn browse(&self, on: bool, found: Arc<dyn Fn(Vec<Nearby>) + Send + Sync>) {
        self.browsing.lock().unwrap().push(on);
        if on {
            found(self.nearby.lock().unwrap().clone());
        }
    }
    fn listen(&self) -> Result<Box<dyn PairListener>, String> {
        let mut n = self.hub.next_port.lock().unwrap();
        *n += 1;
        let port = 40000 + *n;
        let queue: Queue = Arc::default();
        self.hub
            .listeners
            .lock()
            .unwrap()
            .insert(port, Arc::clone(&queue));
        Ok(Box::new(FakeListener { port, queue }))
    }
    fn connect(&self, to: &Nearby) -> Result<(Box<dyn Duplex>, String), String> {
        let queue = self
            .hub
            .listeners
            .lock()
            .unwrap()
            .get(&to.port)
            .cloned()
            .ok_or("connection refused")?;
        let (mine, theirs) = pipe::pair();
        queue
            .lock()
            .unwrap()
            .push_back((Box::new(theirs), self.address.clone()));
        let at = to
            .addrs
            .first()
            .map(ToString::to_string)
            .unwrap_or_default();
        Ok((Box::new(mine), at))
    }
}
