//! TCP for every std app: client connections thru the ip server (payloads/ip, the `udp` endpoint: ops 4 CONNECT, 5 SEND, 6 RECV, 7 CLOSE, 14 ROOM) and its resolver (op 10 RESOLVE), docs/STD-TARGET.md "TCP".
//! Photon's fgtw.org calls (signed VSF over plain HTTP/1.1 on port 80) and its relay WebSocket ride on it; fgtw's own `post` (payloads/fgtw) speaks the same ops from no_std.
//!
//! Nothing here polls (owner, 2026-11-10: "No polling!"). Every wait is a call ip holds until something changes:
//! - a connect, until the connection opens or fails (op 6 with nothing to take);
//! - a read, until data lands or the state changes (op 6);
//! - a write that finds the send buffer full, until the room it wants is free (op 14);
//! - while nobody is present (ip answers every request with nothing then), until the person bit of ip's flags comes back (op 11 WATCH naming no ports).
//!
//! The caller's deadline (`Until`) goes to ip with each held call, which answers at exactly then; `Until::NEVER` holds until something changes.
//! The deadline is a real one when a protocol has it (an HTTP exchange's 16 s, as fgtw's post has: a server gone silent mid-reply would hold the caller for ever), never a clock to look again on.
//!
//! The process reaches ip thru its grant of the `udp` endpoint (the spawn's `--use udp`), at a slot only the app knows (its spawn line's order): `Ip::at(slot)`. The grants page names no endpoint, and asking each grant would send an ip request to whatever server sits there.
//!
//! A `Conn` is `Send` and `Sync`: read and write take `&self`, so one thread reads while another writes (std's `TcpStream` shape: `Read` and `Write` for `&TcpStream`, an `Arc` for `try_clone`).
//! The two directions are separate places at ip (a connection's held receive and its held wait for room), so neither waits on the other.
//! Two reads at once on one `Conn` take turns on its read lock (and two writes on its write lock): ip keeps one held receive a connection, and a second would answer the first early, each pushing the other out.

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration;

use crate::lock::Lock;
use crate::sys::{self, ERR, nr, off};

/// ip's ops (payloads/ip's header).
const CONNECT: u8 = 4;
const SEND: u8 = 5;
const RECV: u8 = 6;
const CLOSE: u8 = 7;
const RESOLVE: u8 = 10;
const WATCH: u8 = 11;
const ROOM: u8 = 14;

/// The most of a write one SEND carries: ip's request buffer (payloads/ip `MSG_MAX`, 2^16 since docs/NO-CEILINGS.md step 5, when a call came to carry its whole message), less the op and the handle (two bytes). It was 4094, all of the 4096 bytes a call carried.
pub const SEND_MAX: usize = (1 << 16) - 3;
/// The most data one RECV reply carries: 2^18 bytes (ip's `RECV_MAX`, the kernel's largest reply), the state byte ahead of it.
pub const RECV_MAX: usize = 1 << 18;
/// The longest host name RESOLVE takes: 253 bytes (RFC 1035's 255 on the wire, less the first length byte and the root).
pub const NAME_MAX: usize = 253;

/// The state byte's low four bits (op 6 and 14 replies): 1 connecting, 2 open, 3 the peer closed, 4 failed, 5 not a connection.
const CONNECTING: u8 = 1;
const OPEN: u8 = 2;
const PEER_CLOSED: u8 = 3;
const FAILED: u8 = 4;
const GONE: u8 = 5;
/// Why a connection failed, the state byte's high four bits in the deadline forms: 1 a reset, 2 unanswered.
const WHY_RESET: u8 = 1;
/// INFO's flags (and WATCH's): bit 3, a person is present, so ip serves applications.
const PRESENT: u8 = 1 << 3;
/// WATCH's flags when none were seen yet: the first watch is answered at once with the flags as they stand.
const NO_FLAGS: u8 = 0xFF;
/// The wait byte old-form ops carry beside the deadline: the longest an ip server from before the deadline forms holds a call (seconds), so a client meeting one is held, never answered at once again and again.
const OLD_WAIT: u8 = u8::MAX;

/// Why a call came back without what it asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The kernel refused the call: no `udp` grant at that slot.
    NoServer,
    /// No route for the destination's family yet (no address of that family on any link).
    NoRoute,
    /// All of ip's connections are in use (sixteen, `TCPN`).
    Busy,
    /// ip could not grow its arc for the connection's buffers.
    NoMemory,
    /// The peer refused the connection (a reset while connecting).
    Refused,
    /// The peer reset the open connection.
    Reset,
    /// What was sent went unanswered thru every retransmission (about 30 s), so ip gave the connection up; while connecting, the SYN did.
    Unanswered,
    /// The caller's deadline passed first.
    TimedOut,
    /// ip no longer knows the connection (closed and freed, or never one).
    Gone,
    /// The name has no address of the families asked for, or does not exist.
    NoAddress,
    /// Not a host name ip takes (empty, past 253 bytes, or a label it refuses).
    BadName,
    /// The DNS servers were silent or failing (ip's four tries a second apart).
    NoAnswer,
    /// ip answered something this client cannot read: an ip server from before the op asked for.
    BadReply,
}

/// A deadline on the counter (`cntvct_el0` ticks, the clock ip holds calls to), or `NEVER`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Until(u64);

impl Until {
    /// No deadline: held until something changes.
    pub const NEVER: Until = Until(u64::MAX);

    /// `d` from now.
    pub fn after(d: Duration) -> Until {
        // WHY saturating: `d` is the caller's and reaches 2^64 s; past the counter's range it would wrap to a deadline already passed and time out at once. That far off is no deadline: NEVER.
        Until(sys::now().saturating_add(crate::time::ticks(d)))
    }

    /// At counter tick `t`.
    pub const fn at(t: u64) -> Until {
        Until(t)
    }

    /// The counter tick (u64::MAX for NEVER).
    pub const fn ticks(self) -> u64 {
        self.0
    }

    /// Whether it has passed (NEVER never does).
    pub fn passed(self) -> bool {
        self.0 != u64::MAX && sys::now() >= self.0
    }
}

/// The family a name is resolved for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// The family the route will use, in ip's order: IPv6 first on the same link, else the family on WiFi first (owner, 2026-10-29 and 2026-11-27).
    Any = 0,
    V4 = 4,
    V6 = 6,
}

/// The ip server, thru the process's grant of the `udp` endpoint at `slot` (its spawn line's order; slot 0 is the console).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ip {
    slot: u64,
}

impl Ip {
    pub const fn at(slot: u64) -> Ip {
        Ip { slot }
    }

    /// One call: `req` out, the reply into `reply`; the reply's length (the kernel never writes past `reply`, nor says more than it wrote: process.rs SYS_REPLY), 0 while nobody is present.
    fn call(self, req: &[u8], reply: &mut [u8]) -> Result<usize, Error> {
        let n = sys::svc(nr::CALL, crate::cap(self.slot), off(req.as_ptr()), req.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64);
        if n == ERR { Err(Error::NoServer) } else { Ok(n as usize) }
    }

    /// Until a person is present again (ip answers every request with nothing while nobody is), or `until`: a WATCH naming no ports, held by ip until its flags change, as often as they change while the person bit stays clear.
    /// A watch is ip's, not a connection's: eight held at once (ip's `WATCHN`), the oldest answered early by a ninth, which costs a waiter one more call, never a loop.
    fn present(self, until: Until) -> Result<(), Error> {
        let id = watch_id();
        let mut flags = NO_FLAGS;
        loop {
            if flags != NO_FLAGS && flags & PRESENT != 0 {
                return Ok(());
            }
            if until.passed() {
                return Err(Error::TimedOut);
            }
            let mut req = [0u8; 19];
            req[0] = WATCH;
            // Level mode (bit 0): no datagram seen to compare with.
            req[1] = 1;
            req[2] = flags;
            req[3..7].copy_from_slice(&id.to_be_bytes());
            // The watch's deadline: 0 is none at ip, so NEVER goes as 0; a deadline of 0 itself has passed, and the loop returned above.
            let d = if until == Until::NEVER { 0 } else { until.0 };
            req[11..19].copy_from_slice(&d.to_be_bytes());
            let mut r = [0u8; 8];
            // A watch answer is 6 bytes at least (no ports named: 6); anything shorter is a server without WATCH, said once instead of asked again.
            if self.call(&req, &mut r)? < 6 {
                return Err(Error::BadReply);
            }
            flags = r[0];
        }
    }

    /// `host`'s address from ip's resolver (the device's one cache), for `family`.
    /// Held by ip until the lookup ends (its tries run four, a second apart), `until` at most.
    pub fn resolve(self, host: &str, family: Family, until: Until) -> Result<IpAddr, Error> {
        let name = host.as_bytes();
        // WHY the check: the request buffer holds `NAME_MAX` bytes of name, the longest a DNS name has; a longer one is no host name.
        if name.is_empty() || name.len() > NAME_MAX {
            return Err(Error::BadName);
        }
        let mut req = [0u8; 3 + NAME_MAX];
        req[0] = RESOLVE;
        req[1] = family as u8;
        req[3..3 + name.len()].copy_from_slice(name);
        loop {
            if until.passed() {
                return Err(Error::TimedOut);
            }
            req[2] = wait_secs(until);
            let mut r = [0u8; 22];
            let n = self.call(&req[..3 + name.len()], &mut r)?;
            if n == 0 {
                self.present(until)?;
                continue;
            }
            if n < 22 {
                return Err(Error::BadReply);
            }
            return match r[0] {
                0 => Ok(addr_from(r[1], &r[2..18])),
                // Still asking: the hold ran out first (a wait of 255 s at most); the lookup goes on at ip and its answer is kept, so it is asked for again.
                1 => continue,
                2 => Err(Error::NoAddress),
                3 => Err(Error::BadName),
                4 => Err(Error::NoRoute),
                5 => Err(Error::NoAnswer),
                6 => Err(Error::Busy),
                _ => Err(Error::BadReply),
            };
        }
    }
}

/// RESOLVE's wait byte for `until`: the whole seconds left, rounded up, 1 to 255 (the byte's range; 0 would answer at once from the cache).
fn wait_secs(until: Until) -> u8 {
    if until == Until::NEVER {
        return u8::MAX;
    }
    let hz = sys::freq();
    // WHY saturating: the deadline may pass between the caller's look and this one; then nothing is left, and the hold is the one second below.
    let left = until.0.saturating_sub(sys::now());
    // WHY the bounds: the byte says 1 to 255 seconds. Under 1 is the caller's last fraction of a second (held a second, and its deadline is looked at after); past 255 is held 255 and asked again.
    match left.div_ceil(hz) {
        0 => 1,
        s if s > u8::MAX as u64 => u8::MAX,
        s => s as u8,
    }
}

/// A watch's id at ip, apart from every other waiter's: a held watch with the same id answers the first, so two threads sharing one would push each other out.
/// The counter and the arc base give the process's bits, a sequence number the call's.
fn watch_id() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let base = (sys::now() as u32) ^ ((sys::arc_base() >> 12) as u32).rotate_left(16);
    base ^ NEXT.fetch_add(1, Ordering::Relaxed).rotate_left(8)
}

/// ip's (family, address bytes) for `ip`. An IPv4 address in IPv6's mapped form (::ffff:a.b.c.d, how a dual-stack caller writes one) goes as IPv4: ip would take it for an IPv6 destination no host holds (Photon's datagrams from ferros, 2026-11-17).
fn family_and_bytes(ip: IpAddr) -> (u8, [u8; 16]) {
    let mut b = [0u8; 16];
    let ip = match ip {
        IpAddr::V6(a) => a.to_ipv4_mapped().map_or(IpAddr::V6(a), IpAddr::V4),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(a) => {
            b[..4].copy_from_slice(&a.octets());
            (4, b)
        }
        IpAddr::V6(a) => {
            b.copy_from_slice(&a.octets());
            (6, b)
        }
    }
}

/// An address from ip's family byte and 16 address bytes (IPv4 in the first four).
fn addr_from(family: u8, b: &[u8]) -> IpAddr {
    if family == 4 {
        IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3]))
    } else {
        let mut a = [0u8; 16];
        a.copy_from_slice(&b[..16]);
        IpAddr::V6(Ipv6Addr::from(a))
    }
}

/// The error a failed connection's state byte names (the high four bits: why), while connecting or after.
fn failed(state: u8, connecting: bool) -> Error {
    match (state >> 4, connecting) {
        (WHY_RESET, true) => Error::Refused,
        (WHY_RESET, false) => Error::Reset,
        _ => Error::Unanswered,
    }
}

/// One client connection at the ip server, closed when dropped.
pub struct Conn {
    ip: Ip,
    /// ip's handle for it, two bytes on the wire (payloads/ip ops 4-7 and 14).
    handle: u16,
    /// One read at ip at a time, one write likewise (see the module's head).
    reading: Lock<()>,
    writing: Lock<()>,
}

impl Conn {
    /// Connect to `addr` (IPv4 or IPv6), returning once ip says the connection opened, or failed (`Refused`, `Unanswered`), or at `until` (`TimedOut`, the connection given up).
    pub fn connect(ip: Ip, addr: SocketAddr, until: Until) -> Result<Conn, Error> {
        let (family, bytes) = family_and_bytes(addr.ip());
        let mut req = [0u8; 20];
        req[0] = CONNECT;
        req[1] = family;
        req[2..18].copy_from_slice(&bytes);
        req[18..20].copy_from_slice(&addr.port().to_be_bytes());
        let handle = loop {
            let mut r = [0u8; 3];
            match ip.call(&req, &mut r)? {
                0 => ip.present(until)?,
                3 if r[0] == 0 => break u16::from_be_bytes([r[1], r[2]]),
                _ => {
                    return Err(match r[0] {
                        2 => Error::NoRoute,
                        6 => Error::NoMemory,
                        _ => Error::BadReply,
                    });
                }
            }
        };
        // From here a failure closes it (the drop), so ip frees the slot.
        let conn = Conn { ip, handle, reading: Lock::new(()), writing: Lock::new(()) };
        loop {
            // A receive that takes nothing, held until the state changes from connecting.
            let mut r = [0u8; 1];
            match conn.recv(&mut r, 0, until)? {
                None => ip.present(until)?,
                Some((s, _)) => match s & 15 {
                    CONNECTING if until.passed() => return Err(Error::TimedOut),
                    CONNECTING => {}
                    OPEN | PEER_CLOSED => return Ok(conn),
                    FAILED => return Err(failed(s, true)),
                    GONE => return Err(Error::Gone),
                    _ => return Err(Error::BadReply),
                },
            }
        }
    }

    /// One op 6 in its deadline form: `most` bytes at most into `out` after the state byte, held to `until` while there is nothing to take. Some((state byte, data bytes)), or None when nobody is present.
    fn recv(&self, out: &mut [u8], most: usize, until: Until) -> Result<Option<(u8, usize)>, Error> {
        let mut req = [0u8; 16];
        req[0] = RECV;
        req[1..3].copy_from_slice(&self.handle.to_be_bytes());
        req[3] = OLD_WAIT;
        req[4..8].copy_from_slice(&(most as u32).to_be_bytes());
        req[8..16].copy_from_slice(&until.0.to_be_bytes());
        match self.ip.call(&req, out)? {
            0 => Ok(None),
            n => Ok(Some((out[0], n - 1))),
        }
    }

    /// Read what has come in, at most `buf.len()` bytes (and `RECV_MAX` a call), held by ip until data lands, the connection's state changes, or `until`.
    /// `Ok(0)`: the peer closed and everything before its close has been read (or `buf` is empty).
    pub fn read(&self, buf: &mut [u8], until: Until) -> Result<usize, Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        let _one = self.reading.lock();
        // The reply lands in `buf` itself, the state byte first and the data after it, then the data moves down one: no second buffer for a read of up to RECV_MAX. A buffer of one byte takes a reply of two.
        let mut small = [0u8; 2];
        loop {
            let got = if buf.len() >= 2 {
                // WHY the min: one reply carries RECV_MAX bytes of data at most; the rest of a larger buffer waits for the next read.
                let len = (1 + RECV_MAX).min(buf.len());
                let out = &mut buf[..len];
                let most = out.len() - 1;
                self.recv(out, most, until)?.map(|(s, n)| {
                    out.copy_within(1..1 + n, 0);
                    (s, n)
                })
            } else {
                self.recv(&mut small, 1, until)?.map(|(s, n)| {
                    buf[0] = small[1];
                    (s, n)
                })
            };
            let Some((s, n)) = got else {
                self.ip.present(until)?;
                continue;
            };
            if n > 0 {
                return Ok(n);
            }
            match s & 15 {
                // Answered with nothing while still connecting or open: the deadline, or the state moved (it opened while this read waited), or a second reader of this handle from elsewhere took the place.
                CONNECTING | OPEN if until.passed() => return Err(Error::TimedOut),
                CONNECTING | OPEN => {}
                PEER_CLOSED => return Ok(0),
                FAILED => return Err(failed(s, false)),
                GONE => return Err(Error::Gone),
                _ => return Err(Error::BadReply),
            }
        }
    }

    /// Hand ip what of `buf` its send buffer takes now (`SEND_MAX` a call at most), waiting at ip for room while it is full, `until` at most. The count taken, at least one byte unless `buf` is empty.
    /// Taken means in ip's send buffer, not yet at the peer; ip sends it as the windows allow (from the moment the connection opens, if it has not yet).
    pub fn write(&self, buf: &[u8], until: Until) -> Result<usize, Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        let _one = self.writing.lock();
        // WHY the min: a call carries SEND_MAX bytes of data; the caller writes the rest after.
        let n = buf.len().min(SEND_MAX);
        // On the stack (this crate has no allocator of its own): 64 KiB of a std thread's 1 MiB (ferros_rt thread.rs `DEFAULT_STACK`).
        let mut req = [0u8; 3 + SEND_MAX];
        req[0] = SEND;
        req[1..3].copy_from_slice(&self.handle.to_be_bytes());
        req[3..3 + n].copy_from_slice(&buf[..n]);
        loop {
            let mut r = [0u8; 3];
            match self.ip.call(&req[..3 + n], &mut r)? {
                0 => self.ip.present(until)?,
                3 if r[0] == 0 => match u16::from_be_bytes([r[1], r[2]]) {
                    // Full: held at ip until room for this call's bytes is free.
                    0 => self.room(n as u32, until)?,
                    k => return Ok(k as usize),
                },
                // Refused: failed, closed by this side, or gone; the state says which.
                _ if r[0] == 5 => return Err(self.why_refused()),
                _ => return Err(Error::BadReply),
            }
        }
    }

    /// All of `buf` into ip's send buffer, `until` at most for the whole of it.
    pub fn write_all(&self, mut buf: &[u8], until: Until) -> Result<(), Error> {
        while !buf.is_empty() {
            let n = self.write(buf, until)?;
            buf = &buf[n..];
        }
        Ok(())
    }

    /// Op 14: held at ip until `want` bytes of the send buffer are free, the state changes, or `until`. Ok when room may be there (the next SEND says), the error when the connection cannot take data.
    fn room(&self, want: u32, until: Until) -> Result<(), Error> {
        loop {
            let mut req = [0u8; 15];
            req[0] = ROOM;
            req[1..3].copy_from_slice(&self.handle.to_be_bytes());
            req[3..7].copy_from_slice(&want.to_be_bytes());
            // A deadline already passed (0 among them, which asks at once) is answered at once as things stand.
            req[7..15].copy_from_slice(&until.0.to_be_bytes());
            let mut r = [0u8; 5];
            match self.ip.call(&req, &mut r)? {
                0 => self.ip.present(until)?,
                5 => {
                    let free = u32::from_be_bytes([r[1], r[2], r[3], r[4]]);
                    return match r[0] & 15 {
                        CONNECTING | OPEN | PEER_CLOSED if free >= want => Ok(()),
                        CONNECTING | OPEN | PEER_CLOSED if until.passed() => Err(Error::TimedOut),
                        // Answered short of the room and the deadline: the state moved (it opened); the SEND is tried again, and held here again if still full.
                        CONNECTING | OPEN | PEER_CLOSED => Ok(()),
                        FAILED => Err(failed(r[0], false)),
                        _ => Err(Error::BadReply),
                    };
                }
                1 if r[0] == GONE => return Err(Error::Gone),
                _ => return Err(Error::BadReply),
            }
        }
    }

    /// Why a SEND was refused: the connection failed (and how), or is gone or closed on this side.
    fn why_refused(&self) -> Error {
        let mut req = [0u8; 15];
        req[0] = ROOM;
        req[1..3].copy_from_slice(&self.handle.to_be_bytes());
        let mut r = [0u8; 5];
        match self.ip.call(&req, &mut r) {
            Ok(5) if r[0] & 15 == FAILED => failed(r[0], false),
            _ => Error::Gone,
        }
    }

    /// Close it: ip sends a FIN after the data already taken and frees the connection once both sides have closed (or 16 s on). The same as dropping it.
    pub fn close(self) {}
}

impl Drop for Conn {
    /// Op 7 CLOSE. While nobody is present ip does nothing for it (every request is answered with nothing then); the connection, asked about by no one, is reset and freed by ip 32 s on.
    fn drop(&mut self) {
        let mut r = [0u8; 1];
        let h = self.handle.to_be_bytes();
        let _ = self.ip.call(&[CLOSE, h[0], h[1]], &mut r);
    }
}
