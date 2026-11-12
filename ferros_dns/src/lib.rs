//! The pure parts of the ip process's resolver (payloads/ip, op 10 RESOLVE): one resolver for the device, in place of the four copies its clients carried (httptest, the kist stower, fgtw, Mahere), each a little different from the others by 2026-11-12.
//! Nothing here touches the machine: ip sends the questions, takes the datagrams, keeps the lookups' deadlines and the held calls, and asks these functions what the protocol says.
//! Each piece is tested on the host (`cargo test -p ferros_dns`).
//!
//! - [`Name`]: a host name in the wire's form (RFC 1035 3.1), lowercased, every label and the whole held to the protocol's lengths.
//! - [`query`]: the question, recursion desired (RFC 1035 4.1).
//! - [`read`]: a datagram read as the answer to one question, or passed over. Every length and offset is checked (the message is the network's, any length and any content): the id, the response bit, the question section (the name, type and class asked), the records' bounds, compression pointers that only point back. CNAME chains are followed; a negative answer's TTL comes from its SOA (RFC 2308 5).
//! - [`Cache`]: the device's answers, a fixed table searched in order (a handful of hosts, no HashMap), each record a name and a type: an address until its TTL runs out and kept after as the last good one, or "no such address" for a short while.

#![no_std]

/// Record types: an IPv4 address, an IPv6 address (RFC 3596), an alias, a zone's start of authority (its negative TTL).
pub const A: u16 = 1;
pub const AAAA: u16 = 28;
const CNAME: u16 = 5;
const SOA: u16 = 6;
/// The Internet class.
const IN: u16 = 1;

/// A name's most bytes in the wire's form, its lengths and the root's zero counted (RFC 1035 3.1).
pub const NAME_MAX: usize = 255;
/// A question's most bytes: the 12-byte header, the name, its type and class.
pub const QUERY_MAX: usize = 12 + NAME_MAX + 4;
/// The most aliases followed from the name asked to its address: eight, past any chain a CDN builds (two or three); a longer one, or a loop, is a failed answer.
const CHAIN_MAX: usize = 1 << 3;
/// A negative answer's longest life: 256 s. RFC 2308 5 takes the SOA's minimum and its TTL, which zones set up to a day; a name created a minute after it was asked for would stay unknown that long. The owner's proposal: negative answers cached briefly.
pub const NEG_MAX: u32 = 1 << 8;
/// A negative answer with no SOA in it: 8 s. RFC 2308 5 says such an answer should not be cached, against two caching servers bouncing it between them for ever; a stub forwards to nobody, and a few seconds keep a client asking in a loop from sending a question each time.
pub const NEG_NO_SOA: u32 = 1 << 3;
/// Failed connections in a row to an address before its name is asked for again (Mahere's rule, 2026-11-11, now the device's): one failure is the network's, not the address's.
pub const FAILURES: u8 = 1 << 2;

/// A host name in the wire's form: each label's length and bytes, then the root's zero; lowercased, so two spellings of one name are one name (RFC 4343).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Name {
    w: [u8; NAME_MAX],
    n: u8,
}

impl Name {
    pub const EMPTY: Name = Name { w: [0; NAME_MAX], n: 0 };

    /// `host` (dotted, as a person writes it; one trailing dot allowed) in the wire's form; None when it is not a host name: an empty label, a label over 63 bytes, a name over 255 in the wire's form, or a byte that is not a letter, a digit, `-` or `_`.
    pub fn from_host(host: &[u8]) -> Option<Name> {
        let host = match host.split_last() {
            Some((b'.', rest)) => rest,
            _ => host,
        };
        if host.is_empty() {
            return None;
        }
        let mut nm = Name::EMPTY;
        let mut k = 0;
        for label in host.split(|&c| c == b'.') {
            // WHY: the host is the caller's; a label is 1 to 63 bytes (RFC 1035 2.3.4: the length byte's two high bits mark a pointer), and the name with its lengths and the root's zero at most 255 (this check keeps `k + 1 + len` in the array and the root's zero at 254 at most).
            if label.is_empty() || label.len() > 63 || k + 1 + label.len() + 1 > NAME_MAX {
                return None;
            }
            nm.w[k] = label.len() as u8;
            for (j, &c) in label.iter().enumerate() {
                let c = c.to_ascii_lowercase();
                // WHY: a host name's bytes (RFC 952, RFC 1123 2.1, and `_` for service names); anything else is not a name to put on the wire for a caller.
                if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_') {
                    return None;
                }
                nm.w[k + 1 + j] = c;
            }
            k += 1 + label.len();
        }
        nm.w[k] = 0;
        nm.n = (k + 1) as u8;
        Some(nm)
    }

    /// The name in the wire's form.
    pub fn wire(&self) -> &[u8] {
        &self.w[..self.n as usize]
    }
}

/// The question for `name`'s records of `qtype` (`A` or `AAAA`), message `id`, recursion desired, into `out`; its length.
pub fn query(id: u16, name: &Name, qtype: u16, out: &mut [u8; QUERY_MAX]) -> usize {
    out[..12].fill(0);
    out[0..2].copy_from_slice(&id.to_be_bytes());
    // Recursion desired; one question.
    out[2] = 1;
    out[5] = 1;
    let w = name.wire();
    out[12..12 + w.len()].copy_from_slice(w);
    let k = 12 + w.len();
    out[k..k + 2].copy_from_slice(&qtype.to_be_bytes());
    out[k + 2..k + 4].copy_from_slice(&IN.to_be_bytes());
    k + 4
}

/// What a server's answer to one question says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// An address (IPv4 in the first 4 bytes), and how long it holds in seconds (the least TTL along the chain of aliases to it).
    Found { addr: [u8; 16], ttl: u32 },
    /// No such address: the name does not exist (NXDOMAIN), or has no record of the type asked (NODATA); how long that holds, in seconds (RFC 2308 5, at most [`NEG_MAX`]).
    NoName { ttl: u32 },
    /// The server could not say (SERVFAIL, REFUSED, any other code), or its answer was cut short (TC) before the address, or the chain of aliases ran past [`CHAIN_MAX`]: ask again, or another server.
    Failed,
}

/// A TTL as the wire has it: one with the high bit set is 0 (RFC 2181 8).
fn ttl_of(b: [u8; 4]) -> u32 {
    let t = u32::from_be_bytes(b);
    if t >> 31 != 0 { 0 } else { t }
}

/// The name at `at` in message `m`, its pointers followed and its letters lowercased, and where it ends where it was written (past its zero, or past its first pointer). None when it runs off the message, uses a reserved label type (RFC 6891 drops the extended ones), points anywhere but back, or comes to more than 255 bytes.
/// It ends: a label adds two bytes at least to a name held to 255, and a pointer moves strictly back, so between two labels the pointers are fewer than the message's bytes.
fn name_at(m: &[u8], at: usize) -> Option<(Name, usize)> {
    let mut nm = Name::EMPTY;
    let mut k = 0;
    let mut i = at;
    let mut end = None;
    loop {
        // WHY every `get`: the offsets are the message's (the network's).
        let len = *m.get(i)? as usize;
        match len >> 6 {
            0 if len == 0 => {
                nm.w[k] = 0;
                nm.n = (k + 1) as u8;
                return Some((nm, end.unwrap_or(i + 1)));
            }
            0 => {
                let label = m.get(i + 1..i + 1 + len)?;
                // WHY: the name's 255 bytes (RFC 1035 3.1); a longer one is not a name, and the array holds 255.
                if k + 1 + len + 1 > NAME_MAX {
                    return None;
                }
                nm.w[k] = len as u8;
                for (j, &c) in label.iter().enumerate() {
                    nm.w[k + 1 + j] = c.to_ascii_lowercase();
                }
                k += 1 + len;
                i += 1 + len;
            }
            3 => {
                let p = (len & 63) << 8 | *m.get(i + 1)? as usize;
                if end.is_none() {
                    end = Some(i + 2);
                }
                // WHY: a pointer names a prior occurrence (RFC 1035 4.1.4); one that points at itself or ahead could loop for ever.
                if p >= i {
                    return None;
                }
                i = p;
            }
            _ => return None,
        }
    }
}

/// One resource record: its owner, type, class, TTL, where its data starts and how long it is, and where the next record starts.
struct Rr {
    owner: Name,
    ty: u16,
    class: u16,
    ttl: u32,
    data: usize,
    len: usize,
    next: usize,
}

/// The record at `at`; None when any of it lies past the message's end.
fn rr_at(m: &[u8], at: usize) -> Option<Rr> {
    let (owner, i) = name_at(m, at)?;
    // WHY the `get`s: the type, class, TTL and data length (10 bytes), then the data, must be inside the message.
    let h = m.get(i..i + 10)?;
    let len = u16::from_be_bytes([h[8], h[9]]) as usize;
    m.get(i + 10..i + 10 + len)?;
    Some(Rr {
        owner,
        ty: u16::from_be_bytes([h[0], h[1]]),
        class: u16::from_be_bytes([h[2], h[3]]),
        ttl: ttl_of([h[4], h[5], h[6], h[7]]),
        data: i + 10,
        len,
        next: i + 10 + len,
    })
}

/// A negative answer's TTL (RFC 2308 5): the SOA's own TTL or its minimum field, the smaller, from the authority section's `n` records at `at`; [`NEG_NO_SOA`] when it carries none; never more than [`NEG_MAX`].
fn negative_ttl(m: &[u8], mut at: usize, n: usize) -> u32 {
    for _ in 0..n {
        let Some(r) = rr_at(m, at) else { break };
        at = r.next;
        // An SOA's data: two names (a byte each at least), then five 32-bit fields, the minimum last (RFC 1035 3.3.13). WHY the length check: the fields are read from the data's end, and data shorter than 22 bytes holds no SOA.
        if r.ty == SOA && r.class == IN && r.len >= 22 {
            let e = r.data + r.len;
            let min = ttl_of([m[e - 4], m[e - 3], m[e - 2], m[e - 1]]);
            // WHY the mins: RFC 2308 5's rule (the smaller of the two), then the brief life the owner's proposal asks for (`NEG_MAX`).
            return r.ttl.min(min).min(NEG_MAX);
        }
    }
    NEG_NO_SOA
}

/// Message `m` read as the answer to question `id` for `name`'s `qtype` records: None when it is not that (another id, not a response, another opcode, another question, or records running past its end: passed over, as if lost); else what it says.
pub fn read(m: &[u8], id: u16, name: &Name, qtype: u16) -> Option<Reply> {
    // WHY each check: the message is the network's. The header's 12 bytes; the id ours; the response bit set; a standard query's opcode.
    let h = m.get(..12)?;
    if h[0..2] != id.to_be_bytes() || h[2] & 0x80 == 0 || (h[2] >> 3) & 15 != 0 {
        return None;
    }
    let count = |k: usize| u16::from_be_bytes([h[k], h[k + 1]]) as usize;
    // The question section, as asked: one question, our name, type and class (an answer that does not repeat the question is no answer to it).
    if count(4) != 1 {
        return None;
    }
    let (q, i) = name_at(m, 12)?;
    let qt = m.get(i..i + 4)?;
    if q != *name || qt[0..2] != qtype.to_be_bytes() || qt[2..4] != IN.to_be_bytes() {
        return None;
    }
    let answers = i + 4;
    // Every answer record inside the message, and where the authority section starts.
    let mut auth = answers;
    for _ in 0..count(6) {
        auth = rr_at(m, auth)?.next;
    }
    match h[3] & 15 {
        0 => {}
        3 => return Some(Reply::NoName { ttl: negative_ttl(m, auth, count(8)) }),
        _ => return Some(Reply::Failed),
    }
    let want = if qtype == AAAA { 16 } else { 4 };
    // The chain from the name asked: the record of the type asked whose owner is the name at the chain's end, the aliases followed (a pass over the answers each; servers put them in order, so one pass usually does).
    let mut target = *name;
    let mut ttl = u32::MAX;
    for _ in 0..CHAIN_MAX {
        let mut j = answers;
        let mut moved = false;
        for _ in 0..count(6) {
            // Read above: every answer is inside the message.
            let r = rr_at(m, j)?;
            j = r.next;
            if r.owner != target || r.class != IN {
                continue;
            }
            if r.ty == qtype && r.len == want {
                let mut addr = [0u8; 16];
                addr[..want].copy_from_slice(&m[r.data..r.data + want]);
                // WHY the min: the address holds only as long as every alias on the way to it.
                return Some(Reply::Found { addr, ttl: ttl.min(r.ttl) });
            }
            if r.ty == CNAME {
                let (t, _) = name_at(m, r.data)?;
                ttl = ttl.min(r.ttl);
                target = t;
                moved = true;
                break;
            }
        }
        if !moved {
            // Truncated (TC): the address may be in what was cut, so the answer says nothing (no TCP here to ask for the rest).
            if h[2] & 2 != 0 {
                return Some(Reply::Failed);
            }
            return Some(Reply::NoName { ttl: negative_ttl(m, auth, count(8)) });
        }
    }
    // The chain ran past `CHAIN_MAX`: a loop, or no chain anyone builds.
    Some(Reply::Failed)
}

/// One record of the cache: a name and a type, what the last answer for them said, and how it has done since.
#[derive(Clone, Copy)]
pub struct Rec {
    pub used: bool,
    pub name: Name,
    pub qtype: u16,
    /// What the last answer said: 0 nothing yet, 1 an address (kept as the last good one after its TTL runs out), 2 no such address.
    pub have: u8,
    pub addr: [u8; 16],
    /// The answer's TTL in seconds, and the counter's tick it holds until.
    pub ttl: u32,
    pub until: u64,
    /// Connections to the address that failed in a row since one opened ([`Cache::conn`]).
    pub fails: u8,
    /// When it was last asked for (the least recently asked gives way when the table is full).
    pub used_t: u64,
    /// A lookup for it is in flight (ip's bookkeeping): it never gives way while one is.
    pub asking: bool,
}

impl Rec {
    pub const FREE: Rec = Rec { used: false, name: Name::EMPTY, qtype: 0, have: 0, addr: [0; 16], ttl: 0, until: 0, fails: 0, used_t: 0, asking: false };
}

/// What the cache holds for a record now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// An address inside its TTL, and the whole seconds it has left.
    Fresh { addr: [u8; 16], left: u32 },
    /// No such address, inside the negative answer's TTL.
    NoName,
    /// An address whose TTL ran out (or whose connections failed [`FAILURES`] times in a row): the last good one, used while its name is asked for again.
    Stale { addr: [u8; 16] },
    /// Nothing, or a negative answer that ran out: ask.
    Unknown,
}

/// The device's answers: `N` records searched in order (a device asks for a handful of hosts; a linear search of a few dozen names beats hashing them).
pub struct Cache<const N: usize> {
    pub r: [Rec; N],
}

impl<const N: usize> Cache<N> {
    pub const fn new() -> Self {
        Cache { r: [Rec::FREE; N] }
    }

    /// The record for `name`'s `qtype` (`now` the counter's tick), made when there is none: in a free slot, else in place of the record asked for least recently that no lookup is using. None when every record is in a lookup.
    pub fn slot(&mut self, name: &Name, qtype: u16, now: u64) -> Option<usize> {
        let i = match self.r.iter().position(|r| r.used && r.qtype == qtype && r.name == *name) {
            Some(i) => i,
            None => {
                let i = match self.r.iter().position(|r| !r.used) {
                    Some(i) => i,
                    None => (0..N).filter(|&i| !self.r[i].asking).min_by_key(|&i| self.r[i].used_t)?,
                };
                self.r[i] = Rec { used: true, name: *name, qtype, ..Rec::FREE };
                i
            }
        };
        self.r[i].used_t = now;
        Some(i)
    }

    /// What record `i` holds at tick `now` (`freq` ticks a second).
    pub fn look(&self, i: usize, now: u64, freq: u64) -> Look {
        let r = &self.r[i];
        match r.have {
            1 if now < r.until => Look::Fresh { addr: r.addr, left: ((r.until - now) / freq) as u32 },
            1 => Look::Stale { addr: r.addr },
            2 if now < r.until => Look::NoName,
            _ => Look::Unknown,
        }
    }

    /// Record `i`'s lookup found `addr`, holding `ttl` seconds.
    pub fn found(&mut self, i: usize, addr: [u8; 16], ttl: u32, now: u64, freq: u64) {
        let r = &mut self.r[i];
        r.have = 1;
        r.addr = addr;
        r.ttl = ttl;
        r.until = now + ttl as u64 * freq;
        r.fails = 0;
    }

    /// Record `i`'s lookup says there is no such address, for `ttl` seconds: the server's word, so a last good address goes with it.
    pub fn no_name(&mut self, i: usize, ttl: u32, now: u64, freq: u64) {
        let r = &mut self.r[i];
        r.have = 2;
        r.addr = [0; 16];
        r.ttl = ttl;
        r.until = now + ttl as u64 * freq;
        r.fails = 0;
    }

    /// Record `i`'s lookup brought no answer (the servers silent, or failing): a last good address is kept for another of its TTL (it worked before; a lookup that failed says nothing against it) and returned; None when it has none.
    pub fn lost(&mut self, i: usize, now: u64, freq: u64) -> Option<([u8; 16], u32)> {
        let r = &mut self.r[i];
        if r.have != 1 {
            return None;
        }
        r.until = now + r.ttl as u64 * freq;
        r.fails = 0;
        Some((r.addr, r.ttl))
    }

    /// A connection to `addr` (family `fam`, IPv4 in the first 4 bytes) opened (`ok`) or failed: an opening clears its records' failures; the [`FAILURES`]th failure in a row makes them stale now, so the next ask for the name asks the server again (the address still used meanwhile).
    pub fn conn(&mut self, fam: u8, addr: &[u8; 16], ok: bool, now: u64) {
        let qtype = if fam == 4 { A } else { AAAA };
        for r in self.r.iter_mut().filter(|r| r.used && r.have == 1 && r.qtype == qtype && r.addr == *addr) {
            if ok {
                r.fails = 0;
                continue;
            }
            r.fails += 1;
            if r.fails == FAILURES {
                r.fails = 0;
                // Stale from now, unless its TTL already ran out.
                if r.until > now {
                    r.until = now;
                }
            }
        }
    }
}

impl<const N: usize> Default for Cache<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
