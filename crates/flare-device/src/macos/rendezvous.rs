//! The mach channel between Flare and its sim host (spec "Starting the sim host and handing over the ring", spike S2).
//!
//! Flare checks in a per-pid bootstrap service ([`Server::check_in`], `com.flare.terminal.simhost.<pid>.<n>`, the way
//! Chromium's `MachPortRendezvousServer` does) and passes its name in the sim host's argv. The sim host looks it up
//! ([`Link::connect`]) and sends a hello that carries a send right to a receive port of its own (for slot-done
//! records), then one ring message per ring (the slots' IOSurface send rights) and the frame and changed records.
//! Flare sends slot-done records back on the sim host's port. Nothing else crosses: no descriptor has to pass
//! `guard_command`, which owns the child's `pre_exec`.
//!
//! Both sides check the sender's pid from the audit token of **every** message (`MACH_RCV_TRAILER_AUDIT`), and free a
//! refused message with `mach_msg_destroy`, so another process of the same user that looks the name up cannot feed
//! Flare surfaces or records, nor free the sim host's slots.

use std::ffi::{CString, c_char};
use std::io;
use std::time::Duration;

use objc2_core_foundation::CFRetained;
use objc2_io_surface::IOSurfaceRef;

use crate::wire::Record;

/// The prefix of the service name; Flare's pid and a number follow.
pub const SERVICE_PREFIX: &str = "com.flare.terminal.simhost.";
/// The most slots a ring message may carry.
pub const MAX_SLOTS: usize = 8;
/// How many messages a receive port holds before a sender waits: room for a second of frame and changed records.
const QUEUE_LIMIT: u32 = 256;

type Port = u32;

const KERN_SUCCESS: i32 = 0;
const MACH_SEND_MSG: i32 = 0x1;
const MACH_RCV_MSG: i32 = 0x2;
const MACH_SEND_TIMEOUT: i32 = 0x10;
const MACH_RCV_TIMEOUT: i32 = 0x100;
/// `MACH_RCV_TRAILER_TYPE(MACH_MSG_TRAILER_FORMAT_0) | MACH_RCV_TRAILER_ELEMENTS(MACH_RCV_TRAILER_AUDIT)`.
const MACH_RCV_TRAILER_AUDIT_OPT: i32 = 3 << 24;
const MACH_RCV_TIMED_OUT: i32 = 0x1000_4003;
const MACH_MSG_TYPE_MOVE_SEND: u8 = 17;
const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
const MACH_MSG_TYPE_MAKE_SEND: u8 = 20;
const MACH_MSG_TYPE_MAKE_SEND_U32: u32 = 20;
const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;
const MACH_MSG_PORT_DESCRIPTOR: u8 = 0;
const MACH_PORT_RIGHT_RECEIVE: u32 = 1;
const MACH_PORT_LIMITS_INFO: i32 = 1;

/// The message ids.
const ID_HELLO: i32 = 0x464c_0001;
const ID_RING: i32 = 0x464c_0002;
const ID_RECORD: i32 = 0x464c_0003;

#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
struct Header {
    bits: u32,
    size: u32,
    remote: Port,
    local: Port,
    voucher: Port,
    id: i32,
}

#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
struct PortDesc {
    name: Port,
    pad1: u32,
    pad2: u16,
    disposition: u8,
    kind: u8,
}

/// `mach_msg_audit_trailer_t`: the sender's audit token, whose sixth word is its pid.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
struct AuditTrailer {
    ttype: u32,
    tsize: u32,
    seqno: u32,
    sender: [u32; 2],
    audit: [u32; 8],
}

const HEADER_LEN: usize = std::mem::size_of::<Header>();
const DESC_LEN: usize = std::mem::size_of::<PortDesc>();

/// A receive buffer: the biggest message (a ring of [`MAX_SLOTS`]) and the trailer.
#[repr(C, align(8))]
struct RcvBuf([u8; 1024]);

unsafe extern "C" {
    static bootstrap_port: Port;
    static mach_task_self_: Port;
    fn bootstrap_check_in(bp: Port, name: *const c_char, sp: *mut Port) -> i32;
    fn bootstrap_look_up(bp: Port, name: *const c_char, sp: *mut Port) -> i32;
    fn mach_msg(
        msg: *mut Header,
        option: i32,
        send_size: u32,
        rcv_size: u32,
        rcv_name: Port,
        timeout: u32,
        notify: Port,
    ) -> i32;
    fn mach_msg_destroy(msg: *mut Header);
    fn mach_port_allocate(task: Port, right: u32, name: *mut Port) -> i32;
    fn mach_port_deallocate(task: Port, name: Port) -> i32;
    fn mach_port_mod_refs(task: Port, name: Port, right: u32, delta: i32) -> i32;
    fn mach_port_set_attributes(
        task: Port,
        name: Port,
        flavor: i32,
        info: *const u32,
        count: u32,
    ) -> i32;
}

fn task() -> Port {
    // SAFETY: a process-wide constant libSystem sets before main.
    unsafe { mach_task_self_ }
}

fn bootstrap() -> Port {
    // SAFETY: as above.
    unsafe { bootstrap_port }
}

fn kr_err(what: &str, kr: i32) -> io::Error {
    io::Error::other(format!("{what} failed: kern_return {kr:#x}"))
}

/// Makes a receive right with room for [`QUEUE_LIMIT`] messages.
fn receive_port() -> io::Result<Port> {
    let mut p: Port = 0;
    // SAFETY: an out pointer to a local.
    let kr = unsafe { mach_port_allocate(task(), MACH_PORT_RIGHT_RECEIVE, &mut p) };
    if kr != KERN_SUCCESS {
        return Err(kr_err("mach_port_allocate", kr));
    }
    let limit = QUEUE_LIMIT;
    // SAFETY: `mach_port_limits` is one u32; the port is ours.
    unsafe { mach_port_set_attributes(task(), p, MACH_PORT_LIMITS_INFO, &limit, 1) };
    Ok(p)
}

/// Destroys a receive right of ours.
fn drop_receive(p: Port) {
    // SAFETY: the port is a receive right this process holds; one reference is dropped (it has one).
    unsafe { mach_port_mod_refs(task(), p, MACH_PORT_RIGHT_RECEIVE, -1) };
}

fn drop_send(p: Port) {
    // SAFETY: a send right this process holds.
    unsafe { mach_port_deallocate(task(), p) };
}

/// Flare's pid from a service name (`com.flare.terminal.simhost.<pid>.<n>`).
pub fn pid_of(name: &str) -> Option<i32> {
    let rest = name.strip_prefix(SERVICE_PREFIX)?;
    let pid = rest.split('.').next()?;
    pid.parse().ok().filter(|p| *p > 0)
}

/// What came in, from the expected pid.
pub enum Incoming {
    /// The sim host is up: a send right to its port (for slot-done records).
    Hello {
        reply: Peer,
        protocol: u32,
    },
    /// A new ring: its slots' surfaces, in slot order.
    Ring {
        device: u32,
        ring: u32,
        width: u32,
        height: u32,
        surfaces: Vec<CFRetained<IOSurfaceRef>>,
    },
    Record(Record),
}

impl std::fmt::Debug for Incoming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Incoming::Hello { protocol, .. } => write!(f, "Hello {{ protocol: {protocol} }}"),
            Incoming::Ring {
                device,
                ring,
                width,
                height,
                surfaces,
            } => write!(
                f,
                "Ring {{ device: {device}, ring: {ring}, {width}x{height}, {} slots }}",
                surfaces.len()
            ),
            Incoming::Record(r) => write!(f, "Record({r:?})"),
        }
    }
}

/// Why a receive gave nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecvError {
    TimedOut,
    /// From another pid (or one not shaped right): freed with `mach_msg_destroy`.
    Refused {
        pid: i32,
        why: String,
    },
    Mach(i32),
}

/// One message read, with its sender's pid.
struct Got {
    pid: i32,
    id: i32,
    complex: bool,
    ports: Vec<(Port, u8)>,
    body: Vec<u8>,
}

/// Receives one message on `port` within `timeout`. A message not from `expect_pid` is destroyed and refused.
fn receive(port: Port, expect_pid: i32, timeout: Duration) -> Result<Got, RecvError> {
    let mut buf = RcvBuf([0; 1024]);
    let h = buf.0.as_mut_ptr().cast::<Header>();
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: the buffer is ours, aligned and as long as the size passed; the trailer is asked for inside it.
    let kr = unsafe {
        mach_msg(
            h,
            MACH_RCV_MSG | MACH_RCV_TIMEOUT | MACH_RCV_TRAILER_AUDIT_OPT,
            0,
            buf.0.len() as u32,
            port,
            ms,
            0,
        )
    };
    if kr == MACH_RCV_TIMED_OUT {
        return Err(RecvError::TimedOut);
    }
    if kr != KERN_SUCCESS {
        return Err(RecvError::Mach(kr));
    }
    // SAFETY: the kernel wrote a header at the start.
    let hdr = unsafe { h.read_unaligned() };
    let size = hdr.size as usize;
    let trailer_at = (size + 3) & !3;
    let refuse = |buf: &mut RcvBuf, pid: i32, why: &str| {
        // SAFETY: the message just received, with its rights; destroyed once.
        unsafe { mach_msg_destroy(buf.0.as_mut_ptr().cast()) };
        RecvError::Refused {
            pid,
            why: why.to_string(),
        }
    };
    if size < HEADER_LEN || trailer_at + std::mem::size_of::<AuditTrailer>() > buf.0.len() {
        return Err(refuse(&mut buf, 0, "a message of a wrong size"));
    }
    // SAFETY: inside the buffer (checked above); the kernel put the audit trailer there.
    let tr = unsafe {
        buf.0
            .as_ptr()
            .add(trailer_at)
            .cast::<AuditTrailer>()
            .read_unaligned()
    };
    let pid = tr.audit[5] as i32;
    if pid != expect_pid {
        return Err(refuse(&mut buf, pid, "a message from another process"));
    }
    let complex = hdr.bits & MACH_MSGH_BITS_COMPLEX != 0;
    let mut at = HEADER_LEN;
    let mut ports = Vec::new();
    if complex {
        if size < at + 4 {
            return Err(refuse(&mut buf, pid, "a complex message with no body"));
        }
        let n =
            u32::from_ne_bytes([buf.0[at], buf.0[at + 1], buf.0[at + 2], buf.0[at + 3]]) as usize;
        at += 4;
        if n > MAX_SLOTS + 1 || size < at + n * DESC_LEN {
            return Err(refuse(&mut buf, pid, "too many descriptors"));
        }
        for i in 0..n {
            // SAFETY: inside the message (checked above).
            let d = unsafe {
                buf.0
                    .as_ptr()
                    .add(at + i * DESC_LEN)
                    .cast::<PortDesc>()
                    .read_unaligned()
            };
            if d.kind != MACH_MSG_PORT_DESCRIPTOR {
                return Err(refuse(&mut buf, pid, "a descriptor that is not a port"));
            }
            ports.push((d.name, d.disposition));
        }
        at += n * DESC_LEN;
    }
    Ok(Got {
        pid,
        id: hdr.id,
        complex,
        ports,
        body: buf.0[at..size].to_vec(),
    })
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(i * 4..i * 4 + 4)?.try_into().ok()?,
    ))
}

/// Sends `body` (and `ports`, each with its disposition) to `remote`, within `timeout`.
fn send(
    remote: Port,
    id: i32,
    ports: &[(Port, u8)],
    body: &[u8],
    timeout: Duration,
) -> Result<(), i32> {
    let mut buf: Vec<u8> = Vec::with_capacity(HEADER_LEN + 4 + ports.len() * DESC_LEN + body.len());
    let complex = !ports.is_empty();
    let header = Header {
        bits: MACH_MSG_TYPE_COPY_SEND | if complex { MACH_MSGH_BITS_COMPLEX } else { 0 },
        size: 0,
        remote,
        local: 0,
        voucher: 0,
        id,
    };
    // SAFETY: Header is plain data with no padding (packed); its bytes are copied out.
    buf.extend_from_slice(unsafe {
        std::slice::from_raw_parts((&header as *const Header).cast::<u8>(), HEADER_LEN)
    });
    if complex {
        buf.extend_from_slice(&(ports.len() as u32).to_ne_bytes());
        for &(name, disposition) in ports {
            let d = PortDesc {
                name,
                pad1: 0,
                pad2: 0,
                disposition,
                kind: MACH_MSG_PORT_DESCRIPTOR,
            };
            // SAFETY: as above, plain packed data.
            buf.extend_from_slice(unsafe {
                std::slice::from_raw_parts((&d as *const PortDesc).cast::<u8>(), DESC_LEN)
            });
        }
    }
    buf.extend_from_slice(body);
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
    let size = buf.len() as u32;
    buf[4..8].copy_from_slice(&size.to_ne_bytes());
    // An aligned copy for the kernel.
    let mut words: Vec<u32> = buf
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_ne_bytes(*c))
        .collect();
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: a whole message of `size` bytes in an aligned buffer we own; send only.
    let kr = unsafe {
        mach_msg(
            words.as_mut_ptr().cast(),
            MACH_SEND_MSG | MACH_SEND_TIMEOUT,
            size,
            0,
            0,
            ms,
            0,
        )
    };
    if kr == KERN_SUCCESS { Ok(()) } else { Err(kr) }
}

/// Flare's end: the checked-in service. One reader thread receives on it.
#[derive(Debug)]
pub struct Server {
    port: Port,
    name: String,
}

impl Server {
    /// Checks in `name` (it must carry this process's pid, [`service_name`]).
    pub fn check_in(name: &str) -> io::Result<Server> {
        let c = CString::new(name).map_err(|_| io::Error::other("a NUL in the service name"))?;
        let mut port: Port = 0;
        // SAFETY: a valid C string and an out pointer to a local.
        let kr = unsafe { bootstrap_check_in(bootstrap(), c.as_ptr(), &mut port) };
        if kr != KERN_SUCCESS {
            return Err(kr_err("bootstrap_check_in", kr));
        }
        let limit = QUEUE_LIMIT;
        // SAFETY: as in `receive_port`.
        unsafe { mach_port_set_attributes(task(), port, MACH_PORT_LIMITS_INFO, &limit, 1) };
        Ok(Server {
            port,
            name: name.to_string(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The next message from `expect_pid` (the sim host's pid), within `timeout`. BLOCKING: the reader thread only.
    pub fn recv(&self, expect_pid: i32, timeout: Duration) -> Result<Incoming, RecvError> {
        let g = receive(self.port, expect_pid, timeout)?;
        let bad = |g: Got, why: &str| {
            for (p, _) in &g.ports {
                drop_send(*p);
            }
            RecvError::Refused {
                pid: g.pid,
                why: why.to_string(),
            }
        };
        match (g.id, g.complex) {
            (ID_HELLO, true) if g.ports.len() == 1 => {
                let Some(protocol) = u32_at(&g.body, 0) else {
                    return Err(bad(g, "a hello with no protocol"));
                };
                Ok(Incoming::Hello {
                    reply: Peer {
                        port: g.ports[0].0,
                        pid: g.pid,
                    },
                    protocol,
                })
            }
            (ID_RING, true) if (1..=MAX_SLOTS).contains(&g.ports.len()) => {
                let (Some(device), Some(ring), Some(width), Some(height)) = (
                    u32_at(&g.body, 0),
                    u32_at(&g.body, 1),
                    u32_at(&g.body, 2),
                    u32_at(&g.body, 3),
                ) else {
                    return Err(bad(g, "a ring with no facts"));
                };
                let mut surfaces = Vec::with_capacity(g.ports.len());
                let mut ok = true;
                for (p, _) in &g.ports {
                    match IOSurfaceRef::lookup_from_mach_port(*p) {
                        Some(s) if s.width() == width as usize && s.height() == height as usize => {
                            surfaces.push(s)
                        }
                        _ => ok = false,
                    }
                    drop_send(*p);
                }
                if !ok {
                    return Err(RecvError::Refused {
                        pid: g.pid,
                        why: "a ring whose surfaces do not match it".into(),
                    });
                }
                Ok(Incoming::Ring {
                    device,
                    ring,
                    width,
                    height,
                    surfaces,
                })
            }
            (ID_RECORD, false) => match Record::from_bytes(&g.body) {
                Some(r @ (Record::Frame(_) | Record::Changed(_))) => Ok(Incoming::Record(r)),
                _ => Err(bad(g, "a record not shaped right")),
            },
            _ => Err(bad(g, "an unknown message")),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop_receive(self.port);
    }
}

/// The service name for this process's `n`-th sim host channel.
pub fn service_name(n: u32) -> String {
    format!("{SERVICE_PREFIX}{}.{n}", std::process::id())
}

/// Flare's send right to the sim host's port: slot-done records go here.
#[derive(Debug)]
pub struct Peer {
    port: Port,
    pid: i32,
}

impl Peer {
    /// The sim host's pid (from the hello's audit token).
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// Sends a record without waiting: false when the port is full or gone (the caller tries again later).
    pub fn send(&self, r: &Record) -> bool {
        send(self.port, ID_RECORD, &[], &r.to_bytes(), Duration::ZERO).is_ok()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        drop_send(self.port);
    }
}

/// The sim host's end: a send right to Flare's service and a receive port of its own.
#[derive(Debug)]
pub struct Link {
    to_flare: Port,
    mine: Port,
    flare_pid: i32,
}

/// How long a frame or changed record may wait for room on Flare's port before it is dropped.
const RECORD_WAIT: Duration = Duration::from_millis(20);

impl Link {
    /// Looks `name` up and says hello (with a send right to this process's own port). The name's pid is Flare's: only
    /// its messages are taken.
    pub fn connect(name: &str, protocol: u32) -> io::Result<Link> {
        let flare_pid =
            pid_of(name).ok_or_else(|| io::Error::other("not a sim host service name"))?;
        let c = CString::new(name).map_err(|_| io::Error::other("a NUL in the service name"))?;
        let mut to_flare: Port = 0;
        // SAFETY: a valid C string and an out pointer to a local.
        let kr = unsafe { bootstrap_look_up(bootstrap(), c.as_ptr(), &mut to_flare) };
        if kr != KERN_SUCCESS {
            return Err(kr_err("bootstrap_look_up", kr));
        }
        let mine = receive_port()?;
        let link = Link {
            to_flare,
            mine,
            flare_pid,
        };
        send(
            to_flare,
            ID_HELLO,
            &[(mine, MACH_MSG_TYPE_MAKE_SEND)],
            &protocol.to_le_bytes(),
            Duration::from_secs(5),
        )
        .map_err(|kr| kr_err("the hello", kr))?;
        Ok(link)
    }

    /// Hands a ring's surfaces to Flare (each as a send right of its own).
    pub fn send_ring(&self, device: u32, ring: u32, surfaces: &[&IOSurfaceRef]) -> Result<(), i32> {
        let Some(first) = surfaces.first() else {
            return Ok(());
        };
        let ports: Vec<(Port, u8)> = surfaces
            .iter()
            .map(|s| (s.create_mach_port(), MACH_MSG_TYPE_MOVE_SEND))
            .collect();
        let mut body = Vec::new();
        for v in [device, ring, first.width() as u32, first.height() as u32] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        let r = send(
            self.to_flare,
            ID_RING,
            &ports,
            &body,
            Duration::from_secs(2),
        );
        if r.is_err() {
            // Not sent: the rights are still ours.
            for (p, _) in ports {
                drop_send(p);
            }
        }
        r
    }

    /// Sends a frame or changed record; false when Flare's port stayed full (the record is dropped).
    pub fn send_record(&self, r: &Record) -> bool {
        send(self.to_flare, ID_RECORD, &[], &r.to_bytes(), RECORD_WAIT).is_ok()
    }

    /// The next slot-done record from Flare, within `timeout`. BLOCKING: the slot-done thread only.
    pub fn recv(&self, timeout: Duration) -> Result<Record, RecvError> {
        let g = receive(self.mine, self.flare_pid, timeout)?;
        for (p, _) in &g.ports {
            drop_send(*p);
        }
        match (g.id, g.complex, Record::from_bytes(&g.body)) {
            (ID_RECORD, false, Some(r @ Record::SlotDone(_))) => Ok(r),
            _ => Err(RecvError::Refused {
                pid: g.pid,
                why: "not a slot-done record".into(),
            }),
        }
    }

    /// A send right to this end's own port (tests: a stranger sending to it).
    #[cfg(test)]
    fn own_send_right(&self) -> Port {
        unsafe extern "C" {
            fn mach_port_insert_right(task: Port, name: Port, poly: Port, kind: u32) -> i32;
        }
        // SAFETY: a receive right we hold; a send right is made under the same name.
        unsafe {
            mach_port_insert_right(task(), self.mine, self.mine, MACH_MSG_TYPE_MAKE_SEND_U32)
        };
        self.mine
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        drop_send(self.to_flare);
        drop_receive(self.mine);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{FrameRecord, SlotDone};
    use std::process::{Command, Stdio};

    /// A small RGBA surface whose bytes are all `mark`.
    fn surface(w: usize, h: usize, mark: u8) -> CFRetained<IOSurfaceRef> {
        let s = crate::macos::ring::rgba_surface(w, h).expect("IOSurfaceCreate");
        crate::macos::ring::fill_surface(&s, [mark; 4]);
        s
    }

    fn first_byte(s: &IOSurfaceRef) -> u8 {
        // SAFETY: read-only lock, one byte read inside the surface.
        unsafe {
            s.lock(
                objc2_io_surface::IOSurfaceLockOptions::ReadOnly,
                std::ptr::null_mut(),
            );
            let b = *s.base_address().as_ptr().cast::<u8>();
            s.unlock(
                objc2_io_surface::IOSurfaceLockOptions::ReadOnly,
                std::ptr::null_mut(),
            );
            b
        }
    }

    /// Not a test on its own: run by `ports_from_another_pid_are_refused` as the sim host (`FLARE_RDV_PEER=host`) or
    /// as a stranger (`intruder`).
    #[test]
    fn peer() {
        let (Ok(mode), Ok(name)) = (
            std::env::var("FLARE_RDV_PEER"),
            std::env::var("FLARE_RDV_NAME"),
        ) else {
            return;
        };
        let link = Link::connect(&name, crate::wire::PROTOCOL).expect("connect");
        let surfaces: Vec<_> = (0..3u8).map(|i| surface(8, 4, 40 + i)).collect();
        let refs: Vec<&IOSurfaceRef> = surfaces.iter().map(|s| &**s).collect();
        link.send_ring(if mode == "host" { 1 } else { 66 }, 1, &refs)
            .expect("ring");
        let frame = Record::Frame(FrameRecord {
            device: 1,
            slot: 2,
            seq: if mode == "host" { 7 } else { 999_999 },
            t_us: 5,
        });
        assert!(link.send_record(&frame));
        if mode == "host" {
            // Flare answers with a slot-done record from its own pid.
            let r = link.recv(Duration::from_secs(20)).expect("slot done");
            assert_eq!(r, Record::SlotDone(SlotDone { device: 1, slot: 2 }));
            println!("PEER-OK");
        }
        std::process::exit(0);
    }

    fn spawn_peer(mode: &str, name: &str) -> std::process::Child {
        Command::new(std::env::current_exe().unwrap())
            .args(["macos::rendezvous::tests::peer", "--exact", "--nocapture"])
            .env("FLARE_RDV_PEER", mode)
            .env("FLARE_RDV_NAME", name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap()
    }

    #[test]
    fn ports_from_another_pid_are_refused() {
        assert_eq!(pid_of("com.flare.terminal.simhost.4242.3"), Some(4242));
        assert_eq!(pid_of("com.flare.terminal.simhost.x.3"), None);
        assert_eq!(pid_of("com.other.4242"), None);

        let server = Server::check_in(&service_name(900)).expect("check in");
        // A stranger of the same user looks the name up first and sends a forged hello, ring and frame record.
        let mut intruder = spawn_peer("intruder", server.name());
        let ipid = intruder.id() as i32;
        let host = spawn_peer("host", server.name());
        let hpid = host.id() as i32;
        let mut refused = Vec::new();
        let mut got = Vec::new();
        let mut reply = None;
        while got.len() < 3 {
            match server.recv(hpid, Duration::from_secs(20)) {
                Ok(Incoming::Hello { reply: r, protocol }) => {
                    assert_eq!(protocol, crate::wire::PROTOCOL);
                    assert_eq!(r.pid(), hpid);
                    reply = Some(r);
                    got.push("hello");
                }
                Ok(Incoming::Ring {
                    device,
                    width,
                    height,
                    surfaces,
                    ..
                }) => {
                    assert_eq!(
                        (device, width, height),
                        (1, 8, 4),
                        "the host's ring, never the intruder's"
                    );
                    let marks: Vec<u8> = surfaces.iter().map(|s| first_byte(s)).collect();
                    assert_eq!(marks, [40, 41, 42]);
                    got.push("ring");
                }
                Ok(Incoming::Record(r)) => {
                    let Record::Frame(f) = r else { panic!("{r:?}") };
                    assert_eq!(f.seq, 7, "the host's record, never the forged one");
                    got.push("frame");
                }
                Err(RecvError::Refused { pid, .. }) => refused.push(pid),
                Err(e) => panic!("{e:?} after {got:?}"),
            }
        }
        // The intruder's three messages may still be queued behind the host's.
        while refused.len() < 3 {
            match server.recv(hpid, Duration::from_secs(10)) {
                Err(RecvError::Refused { pid, .. }) => refused.push(pid),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            refused,
            [ipid, ipid, ipid],
            "hello, ring and frame from the stranger"
        );
        assert!(intruder.wait().unwrap().success());
        // Flare's slot-done record reaches the host, which checks it came from Flare's pid.
        let reply = reply.unwrap();
        assert!(reply.send(&Record::SlotDone(SlotDone { device: 1, slot: 2 })));
        let out = host.wait_with_output().unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("PEER-OK"));

        // The sim host's end refuses a slot-done record from anyone but Flare: here this test process sends to a
        // link that expects another pid.
        let server2 = Server::check_in(&service_name(901)).unwrap();
        let mut link = Link::connect(server2.name(), 1).unwrap();
        link.flare_pid = 1; // launchd: never this test process
        let mine = link.own_send_right();
        send(
            mine,
            ID_RECORD,
            &[],
            &Record::SlotDone(SlotDone { device: 1, slot: 0 }).to_bytes(),
            Duration::from_secs(1),
        )
        .unwrap();
        match link.recv(Duration::from_secs(5)) {
            Err(RecvError::Refused { pid, .. }) => assert_eq!(pid, std::process::id() as i32),
            other => panic!("{other:?}"),
        }
        drop_send(mine);
        assert_eq!(
            link.recv(Duration::from_millis(50)),
            Err(RecvError::TimedOut)
        );
    }
}
