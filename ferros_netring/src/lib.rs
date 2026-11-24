//! A socket's rings between an application and `ip` (docs/WIFI.md "Datagrams at speed"; owner, 2026-11-24, on waves and beams: "q removal. Less q's the better and smaller the buckets without getting stupid", "Like I want this creepy fast").
//!
//! The run is a shared region ip gives the application at its RING OPEN (docs/KERNEL.md "Shared regions"): a header, a send ring the application fills and ip drains, a receive ring ip fills and the application drains.
//! Each ring has one writer and one reader, positions that only grow (bytes written, bytes taken), and records of whole datagrams.
//! - A send lands in the ring and goes out the instant ip sees it: ip looks at every ring each pass and sleeps only after telling the application it sleeps (`IP_WAITS`), so the application's one wake (SYS_WAKE on a word of its run, which rings ip's bell) comes only when ip would not look anyway.
//! - An arrival lands in the receive ring and wakes the reader only when it said it waits (`APP_WAITS`); a reader that is reading takes it with no syscall at all.
//! - The two waiting words make a Dekker pair: the waiter stores its word, fences (SeqCst), looks once more; the producer stores its position, fences, reads the word. One of them always sees the other, so no wake is lost and none is spent on a reader that is awake.
//!
//! Every word is a u64 at an 8-byte aligned offset, read and written whole (single-copy atomic on aarch64, also thru a masked handle, which is how ip reaches it: `Mem`), the ordering made by fences.
//! What the other side wrote is its word, never trusted: ip checks every send record against what the application said it wrote, and drops the ring's unread sends when one is not whole.

#![no_std]

use core::sync::atomic::{fence, Ordering};

/// The run's first word, written by ip when it lays the run out: "fnetrng" and the layout's version, 0.
pub const MAGIC: u64 = u64::from_le_bytes(*b"fnetrng\x00");

/// The header's bytes, before the send ring: four cache lines, each side's words on a line of its own so neither side's stores take the other's line.
pub const HEAD: usize = 256;

/// The layout, written once by ip: the magic, each ring's offset and bytes, the local port.
pub const W_MAGIC: usize = 0;
pub const W_TX_OFF: usize = 8;
pub const W_TX_LEN: usize = 16;
pub const W_RX_OFF: usize = 24;
pub const W_RX_LEN: usize = 32;
pub const W_PORT: usize = 40;
/// The application's line: bytes it wrote into the send ring, bytes it took from the receive ring, and what it waits for (`WAIT_RX`, `WAIT_ROOM`).
pub const W_TX_HEAD: usize = 64;
pub const W_RX_TAIL: usize = 72;
pub const W_APP_WAITS: usize = 80;
/// ip's line: bytes it took from the send ring, bytes it wrote into the receive ring, whether it sleeps (1), and its counts: datagrams that found the receive ring full, send records that were not whole, the ring's state (`OPEN`, `CLOSED`), sends it could not send (no address of that family yet, too long for the link), sends sent.
pub const W_TX_TAIL: usize = 128;
pub const W_RX_HEAD: usize = 136;
pub const W_IP_WAITS: usize = 144;
pub const W_RX_DROPPED: usize = 152;
pub const W_TX_BAD: usize = 160;
pub const W_STATE: usize = 168;
pub const W_TX_REFUSED: usize = 176;
pub const W_TX_SENT: usize = 184;
/// ip's line, the receive side: datagrams written into the ring.
pub const W_RX_COUNT: usize = 192;

/// What the application waits for (`W_APP_WAITS` bits): a datagram (it waits on `W_RX_HEAD`), room in the send ring (on `W_TX_TAIL`).
pub const WAIT_RX: u64 = 1;
pub const WAIT_ROOM: u64 = 2;

/// The ring's state (`W_STATE`): ip serves it; ip let it go (the port or the link went, or ip is ending): no more datagrams either way.
pub const OPEN: u64 = 0;
pub const CLOSED: u64 = 1;

/// A record's header: [len u16][family u8][class u8][port u16][flags u16] in one word, then the stamp, then the address (16 bytes; IPv4 in the first 4), then the payload padded to a word.
pub const REC_HEAD: usize = 32;

/// The longest payload a record carries: what a UDP datagram can be.
pub const PAYLOAD_MOST: usize = 65507;

/// A datagram's class (the record's class byte): how ip marks it (DSCP) and which of the WiFi chip's queues it rides (WMM access category).
/// Best effort; video (AF41, 34, the video queue); voice (EF, 46, the voice queue: the shortest wait for the air).
pub const CLASS_BULK: u8 = 0;
pub const CLASS_VIDEO: u8 = 1;
pub const CLASS_VOICE: u8 = 2;

/// The run's memory, a word at a time, at byte offsets from its start (8-byte aligned): a native pointer for the application (`Raw`), a masked handle for ip.
pub trait Mem {
    fn ld(&self, at: usize) -> u64;
    fn st(&self, at: usize, v: u64);
}

/// The run reached by its address (native code: the application, the host tests).
#[derive(Clone, Copy)]
pub struct Raw(pub *mut u64);

// SAFETY: the run is shared memory both sides map for the run's life; every access is a whole aligned word.
unsafe impl Send for Raw {}
unsafe impl Sync for Raw {}

impl Mem for Raw {
    #[inline(always)]
    fn ld(&self, at: usize) -> u64 {
        // SAFETY: `at` is inside the run (the layout's offsets, checked against the run's bytes at open) and 8-byte aligned; a volatile load of a whole aligned word is single-copy atomic.
        unsafe { core::ptr::read_volatile(self.0.add(at / 8)) }
    }
    #[inline(always)]
    fn st(&self, at: usize, v: u64) {
        // SAFETY: as `ld`.
        unsafe { core::ptr::write_volatile(self.0.add(at / 8), v) }
    }
}

/// Where the rings lie in a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub tx_off: usize,
    pub tx_len: usize,
    pub rx_off: usize,
    pub rx_len: usize,
}

impl Layout {
    /// The layout for a send ring of 2^`tx_log` bytes and a receive ring of 2^`rx_log` bytes (each at least a page, `RING_LEAST_LOG`), and the run that holds them (log2 of its bytes, a power of two as the kernel gives runs): the header, then the send ring, then the receive ring.
    pub fn plan(tx_log: u32, rx_log: u32) -> (Layout, u32) {
        let (tx_len, rx_len) = (1usize << tx_log, 1usize << rx_log);
        let l = Layout { tx_off: HEAD, tx_len, rx_off: HEAD + tx_len, rx_len };
        let run = (HEAD + tx_len + rx_len).next_power_of_two();
        (l, run.trailing_zeros())
    }

    /// The layout as ip wrote it, checked against the run's bytes: None when the magic is not there or a ring is not a power of two inside the run (the run is not a ring, or not yet laid out).
    pub fn read<M: Mem>(m: &M, run: usize) -> Option<Layout> {
        if m.ld(W_MAGIC) != MAGIC {
            return None;
        }
        let l = Layout { tx_off: m.ld(W_TX_OFF) as usize, tx_len: m.ld(W_TX_LEN) as usize, rx_off: m.ld(W_RX_OFF) as usize, rx_len: m.ld(W_RX_LEN) as usize };
        let fits = |off: usize, len: usize| len.is_power_of_two() && len >= 8 && off >= HEAD && off % 8 == 0 && off <= run && len <= run - off;
        (fits(l.tx_off, l.tx_len) && fits(l.rx_off, l.rx_len)).then_some(l)
    }
}

/// The least a ring is: a page.
pub const RING_LEAST_LOG: u32 = 12;

/// A record's bytes in the ring for a payload of `len` bytes.
pub const fn rec_bytes(len: usize) -> usize {
    REC_HEAD + (len + 7) / 8 * 8
}

/// One datagram's record header, as either side reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rec {
    pub len: usize,
    pub fam: u8,
    pub class: u8,
    /// Send: the destination port; receive: the sender's port.
    pub port: u16,
    pub flags: u16,
    /// Send: the application's counter when it wrote the record; receive: ip's when the frame came from the driver. The two sides share the counter (`cntvct_el0`), so either side measures the other's leg.
    pub stamp: u64,
    pub addr: [u8; 16],
}

fn head_word(r: &Rec) -> u64 {
    r.len as u64 | (r.fam as u64) << 16 | (r.class as u64) << 24 | (r.port as u64) << 32 | (r.flags as u64) << 48
}

/// A record's words into a ring at position `pos` (bytes from its start, growing), the ring `len` bytes at `off`: the header, then the payload a word at a time, the last word padded with zeros. Wraps at the ring's end a word at a time (positions and lengths are words, so no word straddles the end).
fn put<M: Mem>(m: &M, off: usize, len: usize, pos: u64, r: &Rec, payload: &[u8]) {
    let at = |k: u64| off + ((pos + k) as usize & (len - 1));
    m.st(at(0), head_word(r));
    m.st(at(8), r.stamp);
    m.st(at(16), u64::from_le_bytes(r.addr[..8].try_into().unwrap()));
    m.st(at(24), u64::from_le_bytes(r.addr[8..].try_into().unwrap()));
    let mut k = REC_HEAD as u64;
    let mut c = payload.chunks_exact(8);
    for w in &mut c {
        m.st(at(k), u64::from_le_bytes(w.try_into().unwrap()));
        k += 8;
    }
    let rest = c.remainder();
    if !rest.is_empty() {
        let mut w = [0u8; 8];
        w[..rest.len()].copy_from_slice(rest);
        m.st(at(k), u64::from_le_bytes(w));
    }
}

/// A record's header from a ring at `pos`.
fn get_head<M: Mem>(m: &M, off: usize, len: usize, pos: u64) -> Rec {
    let at = |k: u64| off + ((pos + k) as usize & (len - 1));
    let w = m.ld(at(0));
    let mut addr = [0u8; 16];
    addr[..8].copy_from_slice(&m.ld(at(16)).to_le_bytes());
    addr[8..].copy_from_slice(&m.ld(at(24)).to_le_bytes());
    Rec { len: (w & 0xFFFF) as usize, fam: (w >> 16) as u8, class: (w >> 24) as u8, port: (w >> 32) as u16, flags: (w >> 48) as u16, stamp: m.ld(at(8)), addr }
}

/// A record's payload (`out.len()` bytes) from a ring at `pos`.
fn get_payload<M: Mem>(m: &M, off: usize, len: usize, pos: u64, out: &mut [u8]) {
    let at = |k: u64| off + ((pos + k) as usize & (len - 1));
    let mut k = REC_HEAD as u64;
    let mut c = out.chunks_exact_mut(8);
    for w in &mut c {
        w.copy_from_slice(&m.ld(at(k)).to_le_bytes());
        k += 8;
    }
    let rest = c.into_remainder();
    if !rest.is_empty() {
        let n = rest.len();
        rest.copy_from_slice(&m.ld(at(k)).to_le_bytes()[..n]);
    }
}

/// Why a send did not land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendErr {
    /// No room now: ip has not taken enough yet (wait for room, `App::arm_room`).
    Full,
    /// Longer than a datagram, or than the ring could ever hold.
    TooLong,
    /// ip let the ring go.
    Closed,
}

/// Why a receive brought nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecvErr {
    /// Nothing waits (wait for it, `App::arm_rx`).
    Empty,
    /// The next datagram is longer than the buffer: its length, the datagram left in place for a buffer that holds it.
    TooLong(usize),
    /// ip let the ring go and nothing is left in it.
    Closed,
}

/// The application's side of a run.
pub struct App<M: Mem> {
    m: M,
    l: Layout,
    tx_head: u64,
    rx_tail: u64,
}

impl<M: Mem> App<M> {
    /// The run of `run` bytes as ip laid it out; None when it is not a laid-out ring.
    pub fn open(m: M, run: usize) -> Option<Self> {
        let l = Layout::read(&m, run)?;
        fence(Ordering::Acquire);
        let (tx_head, rx_tail) = (m.ld(W_TX_HEAD), m.ld(W_RX_TAIL));
        Some(App { m, l, tx_head, rx_tail })
    }

    pub fn layout(&self) -> Layout {
        self.l
    }

    /// The local port ip bound the ring to.
    pub fn port(&self) -> u16 {
        self.m.ld(W_PORT) as u16
    }

    pub fn closed(&self) -> bool {
        self.m.ld(W_STATE) != OPEN
    }

    /// One datagram into the send ring. Ok(true): ip sleeps, so the caller wakes it (SYS_WAKE on `wake_word`, which rings ip's bell); Ok(false): ip looks at the ring on its own.
    pub fn send(&mut self, fam: u8, addr: [u8; 16], port: u16, class: u8, stamp: u64, payload: &[u8]) -> Result<bool, SendErr> {
        if self.closed() {
            return Err(SendErr::Closed);
        }
        let need = rec_bytes(payload.len()) as u64;
        if payload.len() > PAYLOAD_MOST || need > self.l.tx_len as u64 {
            return Err(SendErr::TooLong);
        }
        let tail = self.m.ld(W_TX_TAIL);
        fence(Ordering::Acquire);
        // ip's word: free room is the ring less what ip has not yet taken; a tail past the head would be ip's error, and is read as no room.
        let used = self.tx_head.wrapping_sub(tail);
        if used > self.l.tx_len as u64 || self.l.tx_len as u64 - used < need {
            return Err(SendErr::Full);
        }
        let r = Rec { len: payload.len(), fam, class, port, flags: 0, stamp, addr };
        put(&self.m, self.l.tx_off, self.l.tx_len, self.tx_head, &r, payload);
        self.tx_head += need;
        fence(Ordering::Release);
        self.m.st(W_TX_HEAD, self.tx_head);
        // The Dekker pair with ip's `arm`: the head stored before ip's waiting word is read.
        fence(Ordering::SeqCst);
        Ok(self.m.ld(W_IP_WAITS) != 0)
    }

    /// The next datagram into `out`: its record (its `len` bytes of `out` filled).
    pub fn recv(&mut self, out: &mut [u8]) -> Result<Rec, RecvErr> {
        let head = self.m.ld(W_RX_HEAD);
        fence(Ordering::Acquire);
        if head == self.rx_tail {
            return Err(if self.closed() { RecvErr::Closed } else { RecvErr::Empty });
        }
        let r = get_head(&self.m, self.l.rx_off, self.l.rx_len, self.rx_tail);
        if r.len > out.len() {
            return Err(RecvErr::TooLong(r.len));
        }
        get_payload(&self.m, self.l.rx_off, self.l.rx_len, self.rx_tail, &mut out[..r.len]);
        self.rx_tail += rec_bytes(r.len) as u64;
        fence(Ordering::Release);
        self.m.st(W_RX_TAIL, self.rx_tail);
        Ok(r)
    }

    /// Before a wait for a datagram: the waiting word set, then one more look. Some((the word to wait on, its value)): nothing waits, so wait there (SYS_WAIT at the run's address + the offset) while it holds that value, then `disarm`. None: a datagram came meanwhile, read it (the word is cleared again).
    pub fn arm_rx(&self) -> Option<(usize, u64)> {
        self.m.st(W_APP_WAITS, WAIT_RX);
        fence(Ordering::SeqCst);
        let head = self.m.ld(W_RX_HEAD);
        if head != self.rx_tail || self.closed() {
            self.disarm();
            return None;
        }
        Some((W_RX_HEAD, head))
    }

    /// Before a wait for room in the send ring (a send that found it `Full`): as `arm_rx`, on ip's tail.
    pub fn arm_room(&self, need: usize) -> Option<(usize, u64)> {
        self.m.st(W_APP_WAITS, WAIT_ROOM);
        fence(Ordering::SeqCst);
        let tail = self.m.ld(W_TX_TAIL);
        let used = self.tx_head.wrapping_sub(tail);
        if used > self.l.tx_len as u64 || self.l.tx_len as u64 - used >= rec_bytes(need) as u64 || self.closed() {
            self.disarm();
            return None;
        }
        Some((W_TX_TAIL, tail))
    }

    /// After a wait: nothing waited for any more.
    pub fn disarm(&self) {
        self.m.st(W_APP_WAITS, 0);
    }

    /// The word whose wake rings ip (any word of the run does; this one is the application's own line).
    pub fn wake_word(&self) -> usize {
        W_TX_HEAD
    }
}

/// A send record ip took: its header, its payload in the caller's buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Took {
    /// A datagram (`rec.len` bytes of the buffer).
    Sent(Rec),
    /// The ring held something that is not a whole record: everything the application had written was dropped (`W_TX_BAD` counts it), and the ring goes on from its head.
    Bad,
}

/// ip's side of a run.
pub struct Server<M: Mem> {
    m: M,
    l: Layout,
    tx_tail: u64,
    rx_head: u64,
}

impl<M: Mem> Server<M> {
    /// A run laid out afresh (ip at RING OPEN, the run new from its pool or given back by an earlier ring): every word of the header zeroed, then the layout and the port, the magic last.
    pub fn lay_out(m: M, l: Layout, port: u16) -> Self {
        let mut k = 0;
        while k < HEAD {
            m.st(k, 0);
            k += 8;
        }
        m.st(W_TX_OFF, l.tx_off as u64);
        m.st(W_TX_LEN, l.tx_len as u64);
        m.st(W_RX_OFF, l.rx_off as u64);
        m.st(W_RX_LEN, l.rx_len as u64);
        m.st(W_PORT, port as u64);
        fence(Ordering::Release);
        m.st(W_MAGIC, MAGIC);
        Server { m, l, tx_tail: 0, rx_head: 0 }
    }

    pub fn layout(&self) -> Layout {
        self.l
    }

    /// Whether the application wrote sends ip has not taken.
    pub fn tx_pending(&self) -> bool {
        self.m.ld(W_TX_HEAD) != self.tx_tail
    }

    /// The next send, its payload into `out` (which holds `PAYLOAD_MOST` bytes, so every record fits). None: nothing waits. The tail is stored by `commit`, once for a batch.
    pub fn take(&mut self, out: &mut [u8]) -> Option<Took> {
        let head = self.m.ld(W_TX_HEAD);
        fence(Ordering::Acquire);
        if head == self.tx_tail {
            return None;
        }
        // WHY the checks: the head and the record are the application's words; a record past what it says it wrote, or longer than a datagram or the ring, is not a record, and reading on would take garbage as datagrams.
        let avail = head.wrapping_sub(self.tx_tail);
        let bad = avail > self.l.tx_len as u64 || avail % 8 != 0 || avail < REC_HEAD as u64;
        let r = get_head(&self.m, self.l.tx_off, self.l.tx_len, self.tx_tail);
        if bad || r.len > PAYLOAD_MOST || r.len > out.len() || rec_bytes(r.len) as u64 > avail {
            self.tx_tail = head;
            self.m.st(W_TX_BAD, self.m.ld(W_TX_BAD) + 1);
            return Some(Took::Bad);
        }
        get_payload(&self.m, self.l.tx_off, self.l.tx_len, self.tx_tail, &mut out[..r.len]);
        self.tx_tail += rec_bytes(r.len) as u64;
        Some(Took::Sent(r))
    }

    /// The sends taken so far told to the application (its room back). True: it waits for room, so wake it (SYS_WAKE on `W_TX_TAIL` of the run).
    pub fn commit(&mut self) -> bool {
        fence(Ordering::Release);
        self.m.st(W_TX_TAIL, self.tx_tail);
        fence(Ordering::SeqCst);
        self.m.ld(W_APP_WAITS) & WAIT_ROOM != 0
    }

    /// Counts a send ip could not send (no address yet, too long for the link) or sent.
    pub fn count(&self, sent: bool) {
        let w = if sent { W_TX_SENT } else { W_TX_REFUSED };
        self.m.st(w, self.m.ld(w) + 1);
    }

    /// One arrival into the receive ring. Some(true): the application waits for it, so wake it (SYS_WAKE on `W_RX_HEAD` of the run); Some(false): it reads on its own. None: no room (counted in `W_RX_DROPPED`; the datagram is lost, as a full socket buffer drops).
    pub fn deliver(&mut self, r: &Rec, payload: &[u8]) -> Option<bool> {
        let need = rec_bytes(payload.len()) as u64;
        let tail = self.m.ld(W_RX_TAIL);
        fence(Ordering::Acquire);
        // WHY the check: the tail is the application's word; one past the head is its error, read as no room.
        let used = self.rx_head.wrapping_sub(tail);
        if used > self.l.rx_len as u64 || self.l.rx_len as u64 - used < need {
            self.m.st(W_RX_DROPPED, self.m.ld(W_RX_DROPPED) + 1);
            return None;
        }
        let r = Rec { len: payload.len(), ..*r };
        put(&self.m, self.l.rx_off, self.l.rx_len, self.rx_head, &r, payload);
        self.rx_head += need;
        fence(Ordering::Release);
        self.m.st(W_RX_HEAD, self.rx_head);
        self.m.st(W_RX_COUNT, self.m.ld(W_RX_COUNT) + 1);
        fence(Ordering::SeqCst);
        Some(self.m.ld(W_APP_WAITS) & WAIT_RX != 0)
    }

    /// Before ip sleeps: its waiting word set, then one more look. False: sends came meanwhile, so ip must not sleep (the word cleared again).
    pub fn arm(&self) -> bool {
        self.m.st(W_IP_WAITS, 1);
        fence(Ordering::SeqCst);
        if self.tx_pending() {
            self.m.st(W_IP_WAITS, 0);
            return false;
        }
        true
    }

    /// ip awake: the application's sends need no wake.
    pub fn disarm(&self) {
        self.m.st(W_IP_WAITS, 0);
    }

    /// The ring let go: no more datagrams either way; a reader waiting is woken by the caller (SYS_WAKE on `W_RX_HEAD`).
    pub fn close(&self) {
        fence(Ordering::Release);
        self.m.st(W_STATE, CLOSED);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::vec;
    use std::vec::Vec;

    fn run(log: u32) -> (Vec<AtomicU64>, Raw) {
        let v: Vec<AtomicU64> = (0..(1usize << log) / 8).map(|_| AtomicU64::new(0)).collect();
        let p = v.as_ptr() as *mut u64;
        (v, Raw(p))
    }

    #[test]
    fn plan_fits() {
        let (l, log) = Layout::plan(12, 13);
        assert_eq!(l, Layout { tx_off: HEAD, tx_len: 4096, rx_off: HEAD + 4096, rx_len: 8192 });
        assert!(HEAD + 4096 + 8192 <= 1 << log);
        assert_eq!(log, 14);
    }

    #[test]
    fn open_needs_layout() {
        let (_v, m) = run(14);
        assert!(App::open(m, 1 << 14).is_none());
        let (l, _) = Layout::plan(12, 12);
        Server::lay_out(m, l, 4383);
        let a = App::open(m, 1 << 14).unwrap();
        assert_eq!(a.port(), 4383);
        assert_eq!(a.layout(), l);
        // A layout past the run is refused.
        assert!(App::open(m, 4096).is_none());
    }

    #[test]
    fn send_and_take_wrap() {
        let (_v, m) = run(14);
        let (l, _) = Layout::plan(12, 12);
        let mut s = Server::lay_out(m, l, 7);
        let mut a = App::open(m, 1 << 14).unwrap();
        let mut out = vec![0u8; PAYLOAD_MOST];
        // Many datagrams of odd lengths thru a 4 KiB ring: every one comes out whole, in order, across the ring's end.
        for n in 0..2000usize {
            let len = (n * 37) % 700 + 1;
            let p: Vec<u8> = (0..len).map(|k| (k + n) as u8).collect();
            let mut addr = [0u8; 16];
            addr[0] = n as u8;
            assert!(a.send(4, addr, 9000 + n as u16, CLASS_VOICE, n as u64, &p).is_ok());
            match s.take(&mut out) {
                Some(Took::Sent(r)) => {
                    assert_eq!((r.len, r.fam, r.port, r.class, r.stamp, r.addr[0]), (len, 4, 9000 + n as u16, CLASS_VOICE, n as u64, n as u8));
                    assert_eq!(&out[..len], &p[..]);
                }
                x => panic!("{x:?}"),
            }
            assert!(!s.commit());
        }
        assert!(s.take(&mut out).is_none());
    }

    #[test]
    fn full_then_room() {
        let (_v, m) = run(14);
        let (l, _) = Layout::plan(12, 12);
        let mut s = Server::lay_out(m, l, 7);
        let mut a = App::open(m, 1 << 14).unwrap();
        let p = [1u8; 1000];
        let fit = 4096 / rec_bytes(1000);
        for _ in 0..fit {
            assert_eq!(a.send(4, [0; 16], 1, 0, 0, &p), Ok(false));
        }
        assert_eq!(a.send(4, [0; 16], 1, 0, 0, &p), Err(SendErr::Full));
        assert_eq!(a.send(4, [0; 16], 1, 0, 0, &[0u8; 5000]), Err(SendErr::TooLong));
        // Waiting for room: armed while full, and ip's commit says to wake it.
        assert!(a.arm_room(1000).is_some());
        let mut out = vec![0u8; PAYLOAD_MOST];
        assert!(matches!(s.take(&mut out), Some(Took::Sent(_))));
        assert!(s.commit());
        a.disarm();
        assert_eq!(a.send(4, [0; 16], 1, 0, 0, &p), Ok(false));
    }

    #[test]
    fn deliver_and_recv() {
        let (_v, m) = run(14);
        let (l, _) = Layout::plan(12, 12);
        let mut s = Server::lay_out(m, l, 7);
        let mut a = App::open(m, 1 << 14).unwrap();
        let mut buf = [0u8; 2000];
        assert_eq!(a.recv(&mut buf), Err(RecvErr::Empty));
        let r = Rec { len: 0, fam: 6, class: 0, port: 4383, flags: 0, stamp: 77, addr: [9; 16] };
        for n in 0..3000usize {
            let p: Vec<u8> = (0..(n % 1500)).map(|k| (k * 3 + n) as u8).collect();
            assert_eq!(s.deliver(&r, &p), Some(false));
            let g = a.recv(&mut buf).unwrap();
            assert_eq!((g.len, g.fam, g.port, g.stamp, g.addr), (p.len(), 6, 4383, 77, [9; 16]));
            assert_eq!(&buf[..g.len], &p[..]);
        }
        // A datagram longer than the buffer stays for a buffer that holds it.
        s.deliver(&r, &[5u8; 100]);
        assert_eq!(a.recv(&mut buf[..50]), Err(RecvErr::TooLong(100)));
        assert_eq!(a.recv(&mut buf).unwrap().len, 100);
        // A full ring drops and counts.
        let big = [0u8; 1000];
        while s.deliver(&r, &big).is_some() {}
        assert_eq!(m.ld(W_RX_DROPPED), 1);
    }

    #[test]
    fn bad_records_dropped() {
        let (_v, m) = run(14);
        let (l, _) = Layout::plan(12, 12);
        let mut s = Server::lay_out(m, l, 7);
        let mut out = vec![0u8; PAYLOAD_MOST];
        // A head past the ring, a record longer than what was written: each is Bad, and the ring goes on from the head.
        m.st(W_TX_HEAD, 1 << 20);
        assert_eq!(s.take(&mut out), Some(Took::Bad));
        assert!(s.take(&mut out).is_none());
        let mut a = App::open(m, 1 << 14).unwrap();
        s.commit();
        m.st(l.tx_off + ((1usize << 20) & (l.tx_len - 1)), 4000);
        m.st(W_TX_HEAD, (1 << 20) + 40);
        assert_eq!(s.take(&mut out), Some(Took::Bad));
        assert_eq!(m.ld(W_TX_BAD), 2);
        let _ = &mut a;
    }

    #[test]
    fn waits_pair() {
        let (_v, m) = run(14);
        let (l, _) = Layout::plan(12, 12);
        let mut s = Server::lay_out(m, l, 7);
        let mut a = App::open(m, 1 << 14).unwrap();
        // ip armed and nothing pending: it may sleep, and the application's next send says to wake it.
        assert!(s.arm());
        assert_eq!(a.send(4, [0; 16], 1, 0, 0, b"x"), Ok(true));
        // A send pending: ip must not sleep.
        s.disarm();
        assert!(!s.arm());
        // The reader armed: ip's delivery says to wake it; once a datagram waits, arming says read instead.
        let mut out = vec![0u8; PAYLOAD_MOST];
        s.take(&mut out);
        s.commit();
        assert_eq!(a.arm_rx(), Some((W_RX_HEAD, 0)));
        let r = Rec { len: 0, fam: 4, class: 0, port: 1, flags: 0, stamp: 0, addr: [0; 16] };
        assert_eq!(s.deliver(&r, b"y"), Some(true));
        a.disarm();
        assert_eq!(a.arm_rx(), None);
    }

    /// Two threads, each side on its own, the waiting words spun on instead of the kernel's wait: every datagram arrives whole and in order, none is lost to a missed wake.
    #[test]
    fn threads_race() {
        let (v, m) = run(13);
        let (l, _) = Layout::plan(11, 11);
        let mut s = Server::lay_out(m, l, 7);
        let mut a = App::open(m, 1 << 13).unwrap();
        let done = std::sync::Arc::new(AtomicBool::new(false));
        const N: usize = 200_000;
        let d2 = done.clone();
        let ip = std::thread::spawn(move || {
            let mut out = vec![0u8; PAYLOAD_MOST];
            let mut got = 0usize;
            while got < N {
                match s.take(&mut out) {
                    Some(Took::Sent(r)) => {
                        assert_eq!(r.stamp as usize, got);
                        assert_eq!(out[..r.len].iter().all(|&b| b == got as u8), true);
                        got += 1;
                        if got % 7 == 0 {
                            s.commit();
                        }
                    }
                    Some(Took::Bad) => panic!("bad"),
                    None => {
                        s.commit();
                        if s.arm() {
                            while !s.tx_pending() {
                                std::hint::spin_loop();
                            }
                        }
                        s.disarm();
                    }
                }
            }
            s.commit();
            d2.store(true, Ordering::Release);
        });
        let mut n = 0usize;
        while n < N {
            let len = n % 300;
            let p = std::vec![n as u8; len];
            match a.send(4, [0; 16], 1, 0, n as u64, &p) {
                Ok(_) => n += 1,
                Err(SendErr::Full) => {
                    if let Some((w, val)) = a.arm_room(len) {
                        while m.ld(w) == val {
                            std::hint::spin_loop();
                        }
                    }
                    a.disarm();
                }
                Err(e) => panic!("{e:?}"),
            }
        }
        ip.join().unwrap();
        assert!(done.load(Ordering::Acquire));
        drop(v);
    }
}
