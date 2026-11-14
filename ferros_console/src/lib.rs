//! The kernel's process console (2026-11-14): one ring of `RING` bytes, allocated once at its first write and never grown.
//!
//! Until then the console was a `Vec` that grew until a reader drained it (`diag console`), and nothing drains it on a device no host reads.
//! On the Pro under a ping flood (320 a second, the `ip` process saying every echo it answered) it grew for about 8 minutes until the kernel heap could not double it, and the device rebooted (boot 869 to 870).
//! Any chatty process could take the device down that way.
//!
//! Now a write overwrites the oldest text once the ring is full, so the console costs the same at boot and after a week.
//! Two readers share it: the draining read (`take`, `diag console`), which gets what it has not had yet, and the kept view (`kept`, `diag "console kept"`), which gets the last bytes whoever read them.
//! A byte the draining read never got (overwritten first) is counted against the process that wrote it, and the next `take` begins with a line for each such process: `… A#<n> bytes dropped from <name>`.
//! For that each byte carries its writer's index in `by`, a second ring as long as the text: exact whatever the writes' sizes or interleaving, at one byte a byte.
//!
//! A write never fails, never blocks and never allocates: `put` is a copy into the ring and a count.
//! no_std; the heap only for the two rings and the owners' table, once, and for what a read hands back.
#![no_std]

extern crate alloc;

use alloc::vec::Vec;

/// The ring's length, text bytes.
/// Sized from the 8a's console on 2026-11-14 (boot 214's, read with `diag "console kept"`): about 32 KiB in its first minute and 7 KiB a minute after that (110 bytes a second: the drivers' and Photon's periodic lines).
/// 256 KiB holds the boot and about half an hour at rest, and is also the most one reply carries (`diag "console kept"`'s limit), so one drain takes all of it.
pub const RING_LOG2: u32 = 18;
pub const RING: usize = 1 << RING_LOG2;
const MASK: u64 = RING as u64 - 1;

/// The writers a ring tells apart: the kernel (index `KERNEL`), the processes by name, and `OTHERS` for any process that finds every other entry in use.
pub const OWNERS: usize = 1 << 8;
/// The kernel's own lines (spawns, exits, refusals, faults).
pub const KERNEL: u8 = 0;
/// The last entry: a process that came when all the others were taken, each still holding text in the ring or drops not yet read.
/// It takes that many distinct names writing within one ring's length; the count of a boot's names today is under A#64.
pub const OTHERS: u8 = (OWNERS - 1) as u8;
/// The most of a name an entry keeps and compares.
pub const NAME_MAX: usize = 1 << 5;

#[derive(Clone, Copy)]
struct Owner {
    name: [u8; NAME_MAX],
    /// 0: a free entry.
    len: u8,
    /// Its bytes overwritten before the draining read got them, since that read last ran.
    dropped: u64,
    /// The write position just past its last byte: once the ring has moved a full length past it, none of its bytes remain.
    last: u64,
}

const FREE: Owner = Owner { name: [0; NAME_MAX], len: 0, dropped: 0, last: 0 };

pub struct ConRing {
    /// The text, `RING` long once allocated (empty before the first write).
    text: Vec<u8>,
    /// Each text byte's writer, an index into `owners`.
    by: Vec<u8>,
    owners: Vec<Owner>,
    /// Bytes ever written.
    w: u64,
    /// The draining read's place: every byte before it was read or dropped.
    /// Invariant: `r <= w <= r + RING`.
    r: u64,
}

impl ConRing {
    pub const fn new() -> Self {
        ConRing { text: Vec::new(), by: Vec::new(), owners: Vec::new(), w: 0, r: 0 }
    }

    /// The rings and the owners' table, at the first write: the only allocation a write ever makes.
    fn room(&mut self) {
        if !self.text.is_empty() {
            return;
        }
        self.text = alloc::vec![0; RING];
        self.by = alloc::vec![0; RING];
        self.owners = alloc::vec![FREE; OWNERS];
        self.name(KERNEL as usize, b"kernel");
        self.name(OTHERS as usize, b"other processes");
    }

    fn name(&mut self, k: usize, name: &[u8]) {
        // WHY the bound: the name is the process's, any length; the entry keeps NAME_MAX bytes of it, for the drop line alone.
        let n = name.len().min(NAME_MAX);
        let e = &mut self.owners[k];
        e.name = [0; NAME_MAX];
        e.name[..n].copy_from_slice(&name[..n]);
        e.len = n as u8;
        e.dropped = 0;
        e.last = self.w;
    }

    fn is(&self, k: usize, name: &[u8]) -> bool {
        let e = &self.owners[k];
        // WHY the bound: as in `name`, only NAME_MAX bytes are kept, so only they are compared.
        let n = name.len().min(NAME_MAX);
        e.len as usize == n && e.name[..n] == name[..n]
    }

    /// The owner index for a process of this name.
    /// `hint` is the index the process was given last (kept in its record): checked first, so a writer finds itself without a search.
    /// A new name takes a free entry, or one whose bytes have all left the ring and whose drops were read; `OTHERS` when there is none.
    pub fn owner(&mut self, name: &[u8], hint: u8) -> u8 {
        self.room();
        let h = hint as usize;
        if h != KERNEL as usize && h != OTHERS as usize && self.is(h, name) {
            return hint;
        }
        for k in 1..OTHERS as usize {
            if self.owners[k].len != 0 && self.is(k, name) {
                return k as u8;
            }
        }
        for k in 1..OTHERS as usize {
            let e = &self.owners[k];
            if e.len == 0 || (e.dropped == 0 && e.last + RING as u64 <= self.w) {
                self.name(k, name);
                return k as u8;
            }
        }
        OTHERS
    }

    /// `bytes` into the ring as owner `by`'s.
    /// A byte the draining read has not had yet is overwritten only when the ring is full; it is counted against its writer and the read's place moves past it.
    pub fn put(&mut self, by: u8, bytes: &[u8]) {
        self.room();
        for &c in bytes {
            let i = (self.w & MASK) as usize;
            if self.w - self.r == RING as u64 {
                self.owners[self.by[i] as usize].dropped += 1;
                self.r += 1;
            }
            self.text[i] = c;
            self.by[i] = by;
            self.w += 1;
        }
        self.owners[by as usize].last = self.w;
    }

    /// `v` in decimal, as owner `by`'s.
    pub fn dec(&mut self, by: u8, v: u64) {
        let mut d = [0u8; 20];
        let n = digits(v, &mut d);
        self.put(by, &d[n..]);
    }

    /// The kernel's own text (the `Vec` names the console's writers used before the ring).
    pub fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.put(KERNEL, bytes);
    }
    pub fn push(&mut self, c: u8) {
        self.put(KERNEL, &[c]);
    }

    /// The last byte written, if any.
    pub fn last(&self) -> Option<u8> {
        (self.w != 0).then(|| self.text[((self.w - 1) & MASK) as usize])
    }

    /// The write position: bytes ever written.
    pub fn pos(&self) -> u64 {
        self.w
    }

    /// The bytes written since position `at` that the ring still holds, as two slices (the second nonempty when they wrap).
    pub fn since(&self, at: u64) -> (&[u8], &[u8]) {
        let from = if self.w - at > RING as u64 { self.w - RING as u64 } else { at };
        self.span(from)
    }

    /// The bytes from position `from` to the write position; `from` is at most a ring's length back.
    fn span(&self, from: u64) -> (&[u8], &[u8]) {
        if from == self.w {
            return (&[], &[]);
        }
        let a = (from & MASK) as usize;
        let b = (self.w & MASK) as usize;
        if a < b { (&self.text[a..b], &[]) } else { (&self.text[a..], &self.text[..b]) }
    }

    /// The draining read: a line for each process whose bytes were overwritten before this read could have them, then everything written since the last read.
    pub fn take(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        for e in self.owners.iter_mut().filter(|e| e.dropped != 0) {
            out.extend_from_slice("… A#".as_bytes());
            push_dec(&mut out, e.dropped);
            out.extend_from_slice(b" bytes dropped from ");
            out.extend_from_slice(&e.name[..e.len as usize]);
            out.push(b'\n');
            e.dropped = 0;
        }
        let (a, b) = self.span(self.r);
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        self.r = self.w;
        out
    }

    /// The kept view: the last `max` bytes the ring holds, read or not; the draining read's place is not moved.
    pub fn kept(&self, max: usize) -> Vec<u8> {
        let held = if self.w > RING as u64 { RING as u64 } else { self.w };
        // WHY the bound: `max` is the asker's; the ring holds `held`.
        let n = (max as u64).min(held);
        let (a, b) = self.span(self.w - n);
        let mut out = Vec::with_capacity(n as usize);
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        out
    }

    /// Bytes overwritten before the draining read had them, not yet reported, all writers.
    pub fn dropped(&self) -> u64 {
        self.owners.iter().map(|e| e.dropped).sum()
    }
}

/// `v`'s decimal digits at the end of `d` (a u64 has at most 20); returns where they start.
fn digits(v: u64, d: &mut [u8; 20]) -> usize {
    let mut n = d.len();
    let mut v = v;
    loop {
        n -= 1;
        d[n] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            return n;
        }
    }
}

fn push_dec(out: &mut Vec<u8>, v: u64) {
    let mut d = [0u8; 20];
    let n = digits(v, &mut d);
    out.extend_from_slice(&d[n..]);
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec;

    fn text(r: &ConRing) -> Vec<u8> {
        r.kept(RING)
    }

    #[test]
    fn small_writes_read_once() {
        let mut r = ConRing::new();
        let ip = r.owner(b"ip", 0);
        r.put(ip, b"hello\n");
        r.extend_from_slice(b"kernel line\n");
        assert_eq!(r.take(), b"hello\nkernel line\n".to_vec());
        assert_eq!(r.take(), Vec::<u8>::new());
        assert_eq!(text(&r), b"hello\nkernel line\n".to_vec());
        assert_eq!(r.last(), Some(b'\n'));
    }

    #[test]
    fn full_ring_overwrites_and_counts_per_writer() {
        let mut r = ConRing::new();
        let a = r.owner(b"chatty", 0);
        let b = r.owner(b"quiet", 0);
        r.put(b, b"q0\n");
        let line = [b'x'; 1 << 6];
        // Four rings' worth: everything but the last ring's length is dropped.
        let lines = 4 * RING / line.len();
        for _ in 0..lines {
            r.put(a, &line);
        }
        let total = (3 + lines * line.len()) as u64;
        assert_eq!(r.dropped(), total - RING as u64);
        let out = r.take();
        let head = std::format!("… A#{} bytes dropped from chatty\n… A#3 bytes dropped from quiet\n", 3 * RING);
        assert!(out.starts_with(head.as_bytes()), "{}", std::string::String::from_utf8_lossy(&out[..128]));
        assert_eq!(out.len(), head.len() + RING);
        assert_eq!(r.dropped(), 0);
        // The ring never grew.
        assert_eq!(r.text.len(), RING);
        assert_eq!(r.text.capacity(), RING);
    }

    #[test]
    fn kept_is_not_drained() {
        let mut r = ConRing::new();
        let a = r.owner(b"a", 0);
        r.put(a, b"one\n");
        let _ = r.take();
        r.put(a, b"two\n");
        assert_eq!(r.kept(1 << 10), b"one\ntwo\n".to_vec());
        assert_eq!(r.kept(4), b"two\n".to_vec());
        assert_eq!(r.take(), b"two\n".to_vec());
    }

    #[test]
    fn since_wraps() {
        let mut r = ConRing::new();
        let a = r.owner(b"a", 0);
        r.put(a, &vec![b'.'; RING - 2]);
        let at = r.pos();
        r.put(a, b"wrap!");
        let (x, y) = r.since(at);
        assert_eq!([x, y].concat(), b"wrap!".to_vec());
        assert!(!y.is_empty());
        // A start more than a ring back gives the ring's whole length.
        let (x, y) = r.since(0);
        assert_eq!(x.len() + y.len(), RING);
    }

    #[test]
    fn owners_reused_only_when_gone() {
        let mut r = ConRing::new();
        let first = r.owner(b"p1", 0);
        assert_eq!(r.owner(b"p1", 0), first);
        assert_eq!(r.owner(b"p1", first), first);
        // Fill every entry with a distinct name, each with text in the ring.
        let mut names = Vec::new();
        for k in 0..OWNERS {
            let n = std::format!("n{k}").into_bytes();
            let o = r.owner(&n, 0);
            r.put(o, b"x");
            names.push((n, o));
        }
        // The table ran out: the late names share OTHERS.
        assert_eq!(names.last().unwrap().1, OTHERS);
        // A ring's length later every name's bytes are gone; after a drain (no drops left to say), entries free up again.
        let k = r.owner(b"filler", 0);
        assert_eq!(k, OTHERS);
        r.put(KERNEL, &vec![b'-'; RING]);
        let _ = r.take();
        let fresh = r.owner(b"fresh", 0);
        assert_ne!(fresh, OTHERS);
        assert_ne!(fresh, KERNEL);
    }

    #[test]
    fn dec_and_last() {
        let mut r = ConRing::new();
        assert_eq!(r.last(), None);
        r.dec(KERNEL, 0);
        r.dec(KERNEL, 1234567890);
        assert_eq!(r.take(), b"01234567890".to_vec());
    }
}
