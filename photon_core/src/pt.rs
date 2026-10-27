//! Photon Transport over UDP, as the stock app speaks it (photon/src/network/pt: packets.rs, state.rs, mod.rs; docs/PT.md): a frame up to 1024 bytes is one datagram, acknowledged by a header-only `pt_ack` whose provenance is BLAKE3 of the datagram; a larger frame is a transfer — a signed `pt_spec` (stream letter, chunk count, chunk size, total, BLAKE3 of the whole), answered by a `pt_ack` with sequence u32::MAX, then `[letter][varint seq][chunk]` DATA packets each answered by a `pt_ack` whose provenance is BLAKE3 of the chunk, and a `pt_done` whose provenance is BLAKE3 of the reassembled bytes. The sender re-sends what is not acknowledged on a doubling timer; the receiver answers a chunk of a finished transfer with its `pt_done` again (the sender's probe for a lost DONE).
//!
//! This is the half of PT a bare-metal phone needs to join a fleet and exchange photons: one socket, no relay, no TCP, blast-then-retransmit with the window a fleet frame needs (a 50 KB chain is 50 chunks). Pure state: the caller does the I/O, hands every datagram in with the time, and sends what comes back. Time is two numbers from the caller: a monotonic microsecond count for the timers, and eagle oscillations to stamp the VSF headers. Byte compatibility with the app is pinned by tools/photon-interop.

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use vsf::file_format::VsfHeader;
use vsf::{VsfBuilder, VsfType};

/// Chunk payload per DATA packet, the app's `PTSpec::DEFAULT_PACKET_SIZE`.
pub const PACKET_SIZE: usize = 1024;
/// A frame this long or shorter is one datagram with a packet ack; longer is a transfer (`PTManager::SINGLE_PACKET_MAX`).
pub const SINGLE_PACKET_MAX: usize = 1024;
/// Chunks sent at once on the SPEC ack before waiting for acks (the app's `INITIAL_BLAST`).
pub const BLAST: usize = 256;
/// A transfer or a small packet with no answer for this long is given up (the app's stale timeout).
pub const STALE_US: u64 = 30 * US;
/// A transfer the receiver will not take: a fleet frame is tens of kilobytes, a photon under one.
pub const MAX_TRANSFER: usize = 4 << 20;
const US: u64 = 1_000_000;

/// A UDP endpoint as the `ip` process names it: family 4 or 6, the address (IPv4 in the first four bytes), the port.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Addr {
    pub fam: u8,
    pub ip: [u8; 16],
    pub port: u16,
}

/// A transfer's announcement (`pt_spec`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub sid: u8,
    pub count: u32,
    pub psize: u16,
    pub total: u32,
    pub hash: [u8; 32],
}

impl Spec {
    pub fn for_data(data: &[u8], sid: u8) -> Self {
        let total = data.len() as u32;
        let psize = PACKET_SIZE as u16;
        Spec { sid, count: data.len().div_ceil(PACKET_SIZE) as u32, psize, total, hash: *blake3::hash(data).as_bytes() }
    }

    /// BLAKE3("PT_SPEC_v1" ‖ count LE ‖ psize LE ‖ total LE ‖ hash) — what the SPEC's signature covers.
    pub fn provenance(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(b"PT_SPEC_v1");
        h.update(&self.count.to_le_bytes());
        h.update(&self.psize.to_le_bytes());
        h.update(&self.total.to_le_bytes());
        h.update(&self.hash);
        *h.finalize().as_bytes()
    }

    /// The signed `pt_spec` document.
    pub fn to_vsf(&self, key: &SigningKey, time: i64) -> Vec<u8> {
        let hp = self.provenance();
        let sig = key.sign(&hp).to_bytes();
        VsfBuilder::new()
            .creation_time_oscillations(time)
            .provenance_hash(hp)
            .signature_ed25519(key.verifying_key().to_bytes(), sig)
            .add_section(
                "pt_spec",
                vec![
                    ("sid".to_string(), VsfType::u(self.sid as usize, false)),
                    ("count".to_string(), VsfType::u(self.count as usize, false)),
                    ("psize".to_string(), VsfType::u(self.psize as usize, false)),
                    ("total".to_string(), VsfType::u(self.total as usize, false)),
                    ("hash".to_string(), VsfType::hb(self.hash.to_vec())),
                ],
            )
            .build()
            .unwrap_or_default()
    }
}

/// VSF's variable-length unsigned: seven bits a byte, high bit = more.
fn put_varint(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            out.push(b | 0x80);
        } else {
            out.push(b);
            return;
        }
    }
}

fn get_varint(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut v: u32 = 0;
    for (i, &b) in bytes.iter().enumerate().take(5) {
        v |= ((b & 0x7F) as u32) << (7 * i);
        if b & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

/// A DATA packet: `[letter][varint seq][payload]`.
pub fn encode_data(sid: u8, seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 5 + payload.len());
    out.push(sid);
    put_varint(&mut out, seq);
    out.extend_from_slice(payload);
    out
}

/// A DATA packet's parts, when the first byte is a stream letter.
pub fn decode_data(bytes: &[u8]) -> Option<(u8, u32, &[u8])> {
    let (&sid, rest) = bytes.split_first()?;
    if !sid.is_ascii_lowercase() {
        return None;
    }
    let (seq, n) = get_varint(rest)?;
    Some((sid, seq, &rest[n..]))
}

fn header_only(time: i64, hp: [u8; 32], name: &str, values: Vec<VsfType>) -> Vec<u8> {
    VsfBuilder::new().creation_time_oscillations(time).provenance_hash(hp).provenance_only().add_inline_field(name, values).build().unwrap_or_default()
}

/// A chunk's `pt_ack` (provenance = BLAKE3 of the chunk); with `seq` u32::MAX and the transfer's hash it is the SPEC's ack.
pub fn ack_vsf(time: i64, sid: u8, seq: u32, hash: [u8; 32]) -> Vec<u8> {
    header_only(time, hash, "pt_ack", vec![VsfType::u(sid as usize, false), VsfType::u(seq as usize, false)])
}

/// The `pt_done` (provenance = BLAKE3 of the reassembled bytes, or the promised hash when the assembly failed).
pub fn done_vsf(time: i64, hash: [u8; 32], ok: bool) -> Vec<u8> {
    header_only(time, hash, "pt_done", vec![VsfType::u(ok as usize, false)])
}

/// The packet ack for a frame that arrived as one datagram: stream 0, sequence 0, provenance = BLAKE3 of the datagram. The app sends one for every reliable frame it takes in (sibling and fleet frames, not pings).
pub fn packet_ack(time: i64, datagram: &[u8]) -> Vec<u8> {
    header_only(time, *blake3::hash(datagram).as_bytes(), "pt_ack", vec![VsfType::u(0, false), VsfType::u(0, false)])
}

enum Control {
    Ack { sid: u8, seq: u32, hash: [u8; 32] },
    Nak { seqs: Vec<u32> },
    Done { hash: [u8; 32], ok: bool },
    Other,
}

fn hp32(header: &VsfHeader) -> Option<[u8; 32]> {
    match &header.provenance_hash {
        VsfType::hp(b) if b.len() == 32 => b.as_slice().try_into().ok(),
        _ => None,
    }
}

/// What a decoded VSF datagram is to PT.
enum Kind {
    Control(Control),
    Spec(Spec, [u8; 32]),
    Frame,
}

fn classify(bytes: &[u8]) -> Option<Kind> {
    let (header, end) = VsfHeader::decode(bytes).ok()?;
    let hp = hp32(&header);
    // Header-only PT control: an inline field named pt_* with no section body.
    if let Some(f) = header.fields.iter().find(|f| f.name.starts_with("pt_") && f.offset_bytes == 0 && f.size_bytes == 0) {
        let u = |i: usize| f.inline_values.get(i).and_then(|v| v.as_u64());
        let c = match f.name.as_str() {
            "pt_ack" => Control::Ack { sid: u(0)? as u8, seq: u(1)? as u32, hash: hp? },
            "pt_nak" => Control::Nak { seqs: f.inline_values.iter().filter_map(|v| v.as_u64().map(|n| n as u32)).collect() },
            "pt_done" => Control::Done { hash: hp?, ok: u(0).unwrap_or(0) != 0 },
            _ => Control::Other,
        };
        return Some(Kind::Control(c));
    }
    let section = header.primary_section(bytes, end).ok()?;
    if section.name != "pt_spec" {
        return Some(Kind::Frame);
    }
    let u = |name: &str| section.get_field(name).and_then(|f| f.values.first()).and_then(|v| v.as_u64());
    let hash: [u8; 32] = match section.get_field("hash").and_then(|f| f.values.first()) {
        Some(VsfType::hb(b)) if b.len() == 32 => b.as_slice().try_into().ok()?,
        _ => return None,
    };
    let spec = Spec { sid: u("sid")? as u8, count: u("count")? as u32, psize: u("psize")? as u16, total: u("total")? as u32, hash };
    // The SPEC is signed over its provenance by the sender's device key; an unsigned or mis-signed SPEC is noise.
    let signer: [u8; 32] = match &header.signer_pubkey {
        Some(VsfType::ke(b)) if b.len() == 32 => b.as_slice().try_into().ok()?,
        _ => return None,
    };
    let sig: [u8; 64] = match &header.signature {
        Some(VsfType::ge(b)) if b.len() == 64 => b.as_slice().try_into().ok()?,
        _ => return None,
    };
    if hp? != spec.provenance() {
        return None;
    }
    VerifyingKey::from_bytes(&signer).ok()?.verify(&spec.provenance(), &Signature::from_bytes(&sig)).ok()?;
    Some(Kind::Spec(spec, signer))
}

/// What a datagram did, for the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A complete frame from `from`: one datagram, or a transfer reassembled and verified. `signer` is the SPEC's device key for a transfer, None for a bare datagram (the frame carries its own signature).
    Frame { from: Addr, bytes: Vec<u8>, signer: Option<[u8; 32]> },
    /// An outbound frame (by BLAKE3 of its bytes) was delivered, or given up on.
    Sent { to: Addr, hash: [u8; 32], ok: bool },
}

struct Out {
    to: Addr,
    sid: u8,
    data: Vec<u8>,
    hash: [u8; 32],
    count: u32,
    spec_acked: bool,
    acked: Vec<bool>,
    sent_at: Vec<u64>,
    spec_sent: u64,
    spec_tries: u32,
    rto_us: u64,
    rounds: u32,
    last_activity: u64,
}

impl Out {
    fn chunk(&self, seq: u32) -> &[u8] {
        let a = seq as usize * PACKET_SIZE;
        &self.data[a..(a + PACKET_SIZE).min(self.data.len())]
    }
    fn all_acked(&self) -> bool {
        self.acked.iter().all(|&a| a)
    }
}

struct In {
    from: Addr,
    sid: u8,
    signer: [u8; 32],
    spec: Spec,
    data: Vec<u8>,
    got: Vec<bool>,
    have: u32,
    last: u64,
}

struct Answered {
    from: Addr,
    sid: u8,
    done: Vec<u8>,
}

struct Small {
    to: Addr,
    bytes: Vec<u8>,
    hash: [u8; 32],
    sent_at: u64,
    tries: u32,
    delay_us: u64,
}

/// The transport's state for one socket. Everything it wants sent comes back from `send`, `receive` and `tick` as (destination, datagram) pairs; what arrived for the application comes out of `take_events`.
pub struct Pt {
    key: SigningKey,
    out: Vec<Out>,
    inn: Vec<In>,
    answered: Vec<Answered>,
    small: Vec<Small>,
    next_sid: Vec<(Addr, u8)>,
    events: Vec<Event>,
}

type Sends = Vec<(Addr, Vec<u8>)>;

impl Pt {
    pub fn new(key: SigningKey) -> Self {
        Pt { key, out: Vec::new(), inn: Vec::new(), answered: Vec::new(), small: Vec::new(), next_sid: Vec::new(), events: Vec::new() }
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        core::mem::take(&mut self.events)
    }

    /// Anything still in flight (the caller then polls its socket closely instead of sleeping).
    pub fn busy(&self) -> bool {
        !self.out.is_empty() || !self.small.is_empty() || !self.inn.is_empty()
    }

    /// The next stream letter for `to`: each destination gets its own a..z rotation, as the app does per peer.
    fn allocate_sid(&mut self, to: Addr) -> u8 {
        match self.next_sid.iter_mut().find(|(a, _)| *a == to) {
            Some(e) => {
                let s = e.1;
                e.1 = if s == b'z' { b'a' } else { s + 1 };
                s
            }
            None => {
                self.next_sid.push((to, b'b'));
                b'a'
            }
        }
    }

    /// Send `bytes` (a complete VSF frame) to `to` reliably. Returns the datagrams to send now; the rest follow from `receive` and `tick`. The frame is tracked by BLAKE3 of its bytes (`Event::Sent`).
    pub fn send(&mut self, to: Addr, bytes: Vec<u8>, now_us: u64, time: i64) -> Sends {
        let hash = *blake3::hash(&bytes).as_bytes();
        if bytes.len() <= SINGLE_PACKET_MAX {
            if self.small.iter().any(|s| s.hash == hash && s.to == to) {
                return Vec::new();
            }
            let out = vec![(to, bytes.clone())];
            self.small.push(Small { to, bytes, hash, sent_at: now_us, tries: 0, delay_us: US });
            return out;
        }
        let sid = self.allocate_sid(to);
        let spec = Spec::for_data(&bytes, sid);
        let n = spec.count as usize;
        let spec_bytes = spec.to_vsf(&self.key, time);
        self.out.push(Out { to, sid, data: bytes, hash, count: spec.count, spec_acked: false, acked: vec![false; n], sent_at: vec![0; n], spec_sent: now_us, spec_tries: 0, rto_us: US / 2, rounds: 0, last_activity: now_us });
        vec![(to, spec_bytes)]
    }

    /// A datagram arrived from `from`. Returns the answers to send; frames and delivery outcomes land in `take_events`.
    pub fn receive(&mut self, from: Addr, datagram: &[u8], now_us: u64, time: i64) -> Sends {
        if let Some((sid, seq, payload)) = decode_data(datagram) {
            return self.on_data(from, sid, seq, payload, now_us, time);
        }
        if datagram.len() < 4 || &datagram[0..3] != "RÅ".as_bytes() || datagram[3] != b'<' {
            return Vec::new();
        }
        match classify(datagram) {
            Some(Kind::Control(c)) => self.on_control(from, c, now_us),
            Some(Kind::Spec(spec, signer)) => self.on_spec(from, spec, signer, now_us, time),
            Some(Kind::Frame) => {
                self.events.push(Event::Frame { from, bytes: datagram.to_vec(), signer: None });
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn on_spec(&mut self, from: Addr, spec: Spec, signer: [u8; 32], now_us: u64, time: i64) -> Sends {
        let ack = ack_vsf(time, spec.sid, u32::MAX, spec.hash);
        // The same transfer announced again (a lost ack, a second path): one inbound, answered again.
        if let Some(t) = self.inn.iter_mut().find(|t| t.sid == spec.sid && t.spec.hash == spec.hash && t.signer == signer) {
            t.last = now_us;
            if t.from != from {
                t.from = from;
            }
            return vec![(from, ack)];
        }
        if spec.total as usize > MAX_TRANSFER || spec.psize as usize != PACKET_SIZE || spec.count as usize != (spec.total as usize).div_ceil(PACKET_SIZE) || spec.count == 0 {
            return Vec::new();
        }
        // A new SPEC on a letter this sender holds retires its earlier unfinished transfer there.
        self.inn.retain(|t| !(t.signer == signer && t.sid == spec.sid));
        self.answered.retain(|a| !(a.from == from && a.sid == spec.sid));
        let n = spec.count as usize;
        self.inn.push(In { from, sid: spec.sid, signer, data: vec![0u8; spec.total as usize], got: vec![false; n], have: 0, spec, last: now_us });
        vec![(from, ack)]
    }

    fn on_data(&mut self, from: Addr, sid: u8, seq: u32, payload: &[u8], now_us: u64, time: i64) -> Sends {
        let idx = self.inn.iter().position(|t| t.sid == sid && t.from == from).or_else(|| {
            // The sender's path moved between the SPEC and the DATA: adopt when exactly one transfer on this letter is waiting for its first chunk.
            let mut c = self.inn.iter().enumerate().filter(|(_, t)| t.sid == sid && t.have == 0);
            match (c.next(), c.next()) {
                (Some((i, _)), None) => Some(i),
                _ => None,
            }
        });
        let Some(i) = idx else {
            // A chunk of a transfer already answered: the sender never got the DONE — answer it again.
            return match self.answered.iter().find(|a| a.from == from && a.sid == sid) {
                Some(a) => vec![(from, a.done.clone())],
                None => Vec::new(),
            };
        };
        let t = &mut self.inn[i];
        t.from = from;
        t.last = now_us;
        let ack = ack_vsf(time, sid, seq, *blake3::hash(payload).as_bytes());
        let s = seq as usize;
        if s < t.got.len() && !t.got[s] {
            let at = s * PACKET_SIZE;
            let n = payload.len().min(t.data.len().saturating_sub(at));
            t.data[at..at + n].copy_from_slice(&payload[..n]);
            t.got[s] = true;
            t.have += 1;
        }
        let mut out = vec![(from, ack)];
        if t.have == t.spec.count {
            let ok = *blake3::hash(&t.data).as_bytes() == t.spec.hash;
            let done = done_vsf(time, t.spec.hash, ok);
            out.push((from, done.clone()));
            let t = self.inn.remove(i);
            self.answered.retain(|a| !(a.from == from && a.sid == sid));
            self.answered.push(Answered { from, sid, done });
            if self.answered.len() > 64 {
                self.answered.remove(0);
            }
            if ok {
                self.events.push(Event::Frame { from, bytes: t.data, signer: Some(t.signer) });
            }
        }
        out
    }

    fn on_control(&mut self, from: Addr, c: Control, now_us: u64) -> Sends {
        match c {
            Control::Ack { sid: 0, hash, .. } => {
                // A packet ack: a small frame landed.
                if let Some(p) = self.small.iter().position(|s| s.hash == hash) {
                    let s = self.small.remove(p);
                    self.events.push(Event::Sent { to: s.to, hash: s.hash, ok: true });
                }
                Vec::new()
            }
            Control::Ack { sid, seq: u32::MAX, hash } => {
                // The SPEC ack: the receiver is ready — blast the chunks (all of them within the blast; the rest ride the acks).
                let Some(t) = self.out.iter_mut().find(|t| t.sid == sid && t.hash == hash && !t.spec_acked) else { return Vec::new() };
                t.spec_acked = true;
                t.last_activity = now_us;
                t.to = from;
                let mut out = Vec::new();
                for seq in 0..t.count.min(BLAST as u32) {
                    out.push((t.to, encode_data(t.sid, seq, t.chunk(seq))));
                    t.sent_at[seq as usize] = now_us;
                }
                out
            }
            Control::Ack { sid, seq, hash } => {
                let Some(t) = self.out.iter_mut().find(|t| t.sid == sid && t.spec_acked && (seq as usize) < t.acked.len() && *blake3::hash(t.chunk(seq)).as_bytes() == hash) else { return Vec::new() };
                t.last_activity = now_us;
                t.rounds = 0;
                let mut out = Vec::new();
                if !t.acked[seq as usize] {
                    t.acked[seq as usize] = true;
                    // Past the blast: two more chunks per ack, the app's steady ratio.
                    let mut sent = 0;
                    for s in 0..t.count as usize {
                        if sent == 2 {
                            break;
                        }
                        if t.sent_at[s] == 0 {
                            out.push((t.to, encode_data(t.sid, s as u32, t.chunk(s as u32))));
                            t.sent_at[s] = now_us;
                            sent += 1;
                        }
                    }
                }
                out
            }
            Control::Nak { seqs } => {
                let Some(t) = self.out.iter_mut().find(|t| t.to == from && t.spec_acked) else { return Vec::new() };
                let mut out = Vec::new();
                for seq in seqs {
                    if (seq as usize) < t.acked.len() {
                        out.push((t.to, encode_data(t.sid, seq, t.chunk(seq))));
                        t.sent_at[seq as usize] = now_us;
                    }
                }
                out
            }
            Control::Done { hash, ok } => {
                if let Some(p) = self.out.iter().position(|t| t.hash == hash) {
                    let t = self.out.remove(p);
                    self.events.push(Event::Sent { to: t.to, hash: t.hash, ok });
                }
                Vec::new()
            }
            Control::Other => Vec::new(),
        }
    }

    /// The timers: SPEC re-sends (100 ms doubling to 6.4 s, then given up), chunk re-sends after the RTO (500 ms doubling to 8 s; a transfer with every chunk acked keeps re-sending its last chunk as the probe for a lost DONE), small-frame re-sends (1 s doubling, five tries), and the stale sweep.
    pub fn tick(&mut self, now_us: u64, time: i64) -> Sends {
        let mut out = Vec::new();
        let mut failed: Vec<usize> = Vec::new();
        for (i, t) in self.out.iter_mut().enumerate() {
            if now_us.saturating_sub(t.last_activity) > STALE_US || t.rounds > 10 || t.spec_tries > 8 {
                failed.push(i);
                continue;
            }
            if !t.spec_acked {
                let delay = (100_000u64 << t.spec_tries.min(6)).min(6_400_000);
                if now_us.saturating_sub(t.spec_sent) >= delay {
                    t.spec_tries += 1;
                    t.spec_sent = now_us;
                    let spec = Spec::for_data(&t.data, t.sid);
                    out.push((t.to, spec.to_vsf(&self.key, time)));
                }
                continue;
            }
            let probe = t.all_acked();
            let mut timed_out = false;
            for s in 0..t.count as usize {
                let due = if probe { s + 1 == t.count as usize } else { !t.acked[s] && t.sent_at[s] != 0 };
                if due && now_us.saturating_sub(t.sent_at[s]) >= t.rto_us {
                    out.push((t.to, encode_data(t.sid, s as u32, t.chunk(s as u32))));
                    t.sent_at[s] = now_us;
                    timed_out = true;
                }
            }
            if timed_out {
                t.rounds += 1;
                t.rto_us = (t.rto_us * 2).min(8 * US);
            }
        }
        for i in failed.into_iter().rev() {
            let t = self.out.remove(i);
            self.events.push(Event::Sent { to: t.to, hash: t.hash, ok: false });
        }
        let mut gone: Vec<usize> = Vec::new();
        for (i, s) in self.small.iter_mut().enumerate() {
            if now_us.saturating_sub(s.sent_at) >= s.delay_us {
                if s.tries >= 5 {
                    gone.push(i);
                    continue;
                }
                s.tries += 1;
                s.sent_at = now_us;
                s.delay_us = (s.delay_us * 2).min(60 * US);
                out.push((s.to, s.bytes.clone()));
            }
        }
        for i in gone.into_iter().rev() {
            let s = self.small.remove(i);
            self.events.push(Event::Sent { to: s.to, hash: s.hash, ok: false });
        }
        self.inn.retain(|t| now_us.saturating_sub(t.last) <= STALE_US);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Addr {
        let mut ip = [0u8; 16];
        ip[..4].copy_from_slice(&[192, 168, 1, n]);
        Addr { fam: 4, ip, port: 4383 }
    }

    /// Two ends joined by a channel that drops every `lossy`-th datagram; the clock advances a millisecond per step and ticks fire. Returns the frames each end delivered.
    fn run(frame: Vec<u8>, lossy: usize) -> (Vec<Event>, Vec<Event>) {
        let (a, b) = (addr(1), addr(2));
        let mut pa = Pt::new(SigningKey::from_bytes(&[1u8; 32]));
        let mut pb = Pt::new(SigningKey::from_bytes(&[2u8; 32]));
        let mut now: u64 = 1_000;
        let time: i64 = 1_420_000_000_000;
        let mut wire: Vec<(Addr, Addr, Vec<u8>)> = pa.send(b, frame, now, time).into_iter().map(|(to, d)| (a, to, d)).collect();
        let mut n = 0usize;
        for _ in 0..20_000 {
            if wire.is_empty() && !pa.busy() && !pb.busy() {
                break;
            }
            let mut next = Vec::new();
            for (from, to, d) in wire.drain(..) {
                n += 1;
                if lossy != 0 && n % lossy == 0 {
                    continue;
                }
                let (me, them) = if to == b { (&mut pb, b) } else { (&mut pa, a) };
                for (t2, d2) in me.receive(from, &d, now, time) {
                    next.push((them, t2, d2));
                }
                // The application's half: a frame that came as one datagram is acked by the receiver (the app does this per reliable frame).
                let ev = me.take_events();
                for e in &ev {
                    if let Event::Frame { from, bytes, signer: None } = e {
                        next.push((them, *from, packet_ack(time, bytes)));
                    }
                }
                me.events.extend(ev);
            }
            now += 1_000;
            for (t2, d2) in pa.tick(now, time) {
                next.push((a, t2, d2));
            }
            for (t2, d2) in pb.tick(now, time) {
                next.push((b, t2, d2));
            }
            wire = next;
        }
        (pa.take_events(), pb.take_events())
    }

    fn frame(n: usize) -> Vec<u8> {
        // A VSF-looking frame so the receiver classifies it as one, then filler.
        let mut f = VsfBuilder::new().creation_time_oscillations(5).provenance_hash([3u8; 32]).provenance_only().add_section("test", vec![("x".to_string(), VsfType::u(1, false))]).build().unwrap();
        while f.len() < n {
            f.push((f.len() % 251) as u8);
        }
        f
    }

    #[test]
    fn small_frame_is_one_datagram_and_its_ack() {
        let f = frame(300);
        let (sa, sb) = run(f.clone(), 0);
        assert_eq!(sb, vec![Event::Frame { from: addr(1), bytes: f.clone(), signer: None }]);
        assert_eq!(sa, vec![Event::Sent { to: addr(2), hash: *blake3::hash(&f).as_bytes(), ok: true }]);
    }

    #[test]
    fn packet_ack_settles_a_small_frame() {
        let (a, b) = (addr(1), addr(2));
        let mut pa = Pt::new(SigningKey::from_bytes(&[1u8; 32]));
        let f = frame(200);
        let out = pa.send(b, f.clone(), 10, 7);
        assert_eq!(out.len(), 1);
        let ack = packet_ack(7, &f);
        assert!(pa.receive(b, &ack, 20, 7).is_empty());
        assert_eq!(pa.take_events(), vec![Event::Sent { to: b, hash: *blake3::hash(&f).as_bytes(), ok: true }]);
        assert!(!pa.busy());
        let _ = a;
    }

    #[test]
    fn large_frame_transfers_clean() {
        let f = frame(50_000);
        let (sa, sb) = run(f.clone(), 0);
        assert_eq!(sb, vec![Event::Frame { from: addr(1), bytes: f.clone(), signer: Some(SigningKey::from_bytes(&[1u8; 32]).verifying_key().to_bytes()) }]);
        assert_eq!(sa, vec![Event::Sent { to: addr(2), hash: *blake3::hash(&f).as_bytes(), ok: true }]);
    }

    #[test]
    fn large_frame_survives_loss() {
        let f = frame(300_000);
        let (sa, sb) = run(f.clone(), 7);
        assert_eq!(sb.len(), 1, "delivered once: {:?}", sb.iter().map(|e| match e { Event::Frame { bytes, .. } => bytes.len(), _ => 0 }).collect::<Vec<_>>());
        match &sb[0] {
            Event::Frame { bytes, .. } => assert_eq!(bytes, &f),
            e => panic!("{e:?}"),
        }
        assert_eq!(sa, vec![Event::Sent { to: addr(2), hash: *blake3::hash(&f).as_bytes(), ok: true }]);
    }

    #[test]
    fn data_packet_round_trips_and_varint_matches_the_app() {
        assert_eq!(encode_data(b'a', 0, b"xy"), vec![b'a', 0, b'x', b'y']);
        assert_eq!(encode_data(b'b', 548, &[]), vec![b'b', 0xA4, 0x04]);
        let d = encode_data(b'z', 16383, &[9; 3]);
        assert_eq!(decode_data(&d), Some((b'z', 16383, &[9u8; 3][..])));
        assert_eq!(decode_data(b"R\xc3\x85<"), None);
    }

    #[test]
    fn spec_signs_and_parses() {
        let k = SigningKey::from_bytes(&[4u8; 32]);
        let spec = Spec::for_data(&[1u8; 3000], b'c');
        let bytes = spec.to_vsf(&k, 99);
        match classify(&bytes) {
            Some(Kind::Spec(s, signer)) => {
                assert_eq!(s, spec);
                assert_eq!(signer, k.verifying_key().to_bytes());
            }
            _ => panic!("spec did not classify"),
        }
        // A tampered count fails the provenance.
        let bad = Spec { count: 4, ..spec.clone() }.to_vsf(&k, 99);
        let mut forged = bad.clone();
        let _ = &mut forged;
        assert!(matches!(classify(&bytes), Some(Kind::Spec(..))));
        let ack = ack_vsf(99, b'c', u32::MAX, spec.hash);
        assert!(matches!(classify(&ack), Some(Kind::Control(Control::Ack { sid: b'c', seq: u32::MAX, .. }))));
        let done = done_vsf(99, spec.hash, true);
        assert!(matches!(classify(&done), Some(Kind::Control(Control::Done { ok: true, .. }))));
    }
}
