//! Photon on ferros: the client that joins the owner's fleet over the LAN and stands as one of its devices (photon docs/pairing-v2.md "The ceremony over the mesh"; STATUS.md "Enrollment / pairing"). An EL0 process on the `ip` process's UDP: it beacons as the stock app does (`pt_disc` to the app's multicast groups on 4384), hears the owner's other devices beaconing under the same handle, hands the nearest sponsor its binding request as a `bind_req` over Photon Transport, takes the chain (`fleet_resp`) and the fan-out (`fanout_resp`) the sponsor pushes back, folds the chain against its own roots and opens the fan-out with its own ira, and from then on answers pings as a fleet member. Everything it speaks is photon_core (pt, join, wire), pinned byte for byte against the app by tools/photon-interop.
//!
//! Identity: the device key is rooted in the kernel's ira (`ira_key("photon device")` → `join::DeviceKeys::derive`), the same after a factory reset; the identity roots (`photon.identity_seed`, `photon.handle_proof`) are pushed into the vault scope `photon` at pairing by the dev box (tools/photon-interop `roots` + `ferros-bridge store put`) - the handle itself never reaches the phone. Once bound, the chain (`photon.chain`) and the fleet key (`photon.fleet_key`) live in the same scope. The 23 pairing words are printed on the console for the sponsor's human to type; the sponsor's screen also shows this device as a nearby candidate it can tap.
//!
//! Started by init at boot (payloads/init), or from the bridge: ferros-bridge spawn photon.bin [hex: fam, address 16, port BE — a sponsor to hand the request to directly, for a network whose access point does not pass multicast between its wireless clients; the vault key `photon.sponsor` holds the same 19 bytes for the boot-started client] --name photon --use udp --store photon --ira --serve photon. The `photon` endpoint serves the map's Photon circle: op 1 → [state: 0 not paired, 1 paired, 2 bound, 3 member][fleet devices heard][the 23 pairing words, utf-8, while unbound]. Grant slots: 0 console, 1 udp (use), 2 store photon, 3 ira. Runs on its own 128 KiB thread stack (Falcon's degree-512 signing key is about 60 KiB by value) over a 160 KiB heap in `.data`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use ferros_sys::{cap, now, svc, ERR};
use photon_core::join::{self, DeviceKeys, Inbound};
use photon_core::pt::{self, Addr, Event, Pt};
use photon_core::wire::{self, Frame};

/// Sized to fit a 1 MiB arc beside 660 KB of code (the arc pool refused 2 MiB, 2026-10-26): enough for a small fleet's chain, the request and a transfer in flight; a kernel heap region for apps is the next step.
const HEAP_BYTES: usize = 160 * 1024;
const STACK_BYTES: usize = 128 * 1024;

#[repr(C, align(16))]
struct Region<const N: usize>(UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Region<N> {}
/// Both in `.data` (a non-zero initializer keeps them out of `.bss`, which the flat image does not carry).
#[unsafe(link_section = ".data.heap")]
static HEAP: Region<HEAP_BYTES> = Region(UnsafeCell::new([1; HEAP_BYTES]));
#[unsafe(link_section = ".data.stack")]
static STACK: Region<STACK_BYTES> = Region(UnsafeCell::new([1; STACK_BYTES]));

#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

const S_UDP: u64 = 1;
const S_STORE: u64 = 2;
const S_IRA: u64 = 3;
const S_SERVE: u64 = 4;
/// Grant slot 5: entropy (the kernel's KOHI stream), for fgtw's fleet-key minting (fgtw::set_entropy).
const S_ENTROPY: u64 = 5;

struct Log {
    grants: u64,
    buf: Vec<u8>,
}

impl Log {
    fn put(&mut self, s: &[u8]) {
        self.buf.extend_from_slice(s);
    }
    fn dec(&mut self, mut v: u64) {
        let mut b = [0u8; 20];
        let mut i = b.len();
        loop {
            i -= 1;
            b[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let d = b;
        self.put(&d[i..]);
    }
    /// A key's first four bytes in hex: the fingerprint the app's logs use (`fp`), never a whole key.
    fn fp(&mut self, k: &[u8]) {
        let h = b"0123456789abcdef";
        for &c in &k[..4] {
            self.put(&[h[(c >> 4) as usize], h[(c & 15) as usize]]);
        }
    }
    fn addr(&mut self, a: &Addr) {
        if a.fam == 4 {
            for k in 0..4 {
                if k > 0 {
                    self.put(b".");
                }
                self.dec(a.ip[k] as u64);
            }
        } else {
            let h = b"0123456789abcdef";
            for g in 0..8 {
                if g > 0 {
                    self.put(b":");
                }
                for k in 0..2 {
                    let b = a.ip[2 * g + k];
                    self.put(&[h[(b >> 4) as usize], h[(b & 15) as usize]]);
                }
            }
        }
        self.put(b":");
        self.dec(a.port as u64);
    }
    fn line(&mut self) {
        self.put(b"\n");
        let mut r = [0u8; 16];
        ferros_sys::call(cap(self.grants, 0), &self.buf, &mut r);
        self.buf.clear();
    }
}

/// The `ip` process's UDP endpoint (payloads/ip): 1 SEND [fam][addr 16][dport][sport][payload], 2 RECV [port] → [fam][addr 16][sport][dport][payload], 3 INFO.
struct Udp {
    grants: u64,
    base: u64,
}

impl Udp {
    fn call(&self, req: &[u8], out: &mut [u8]) -> u64 {
        svc(ferros_sys::nr::CALL, cap(self.grants, S_UDP), req.as_ptr() as u64 - self.base, req.len() as u64, out.as_mut_ptr() as u64 - self.base, out.len() as u64)
    }
    fn send(&self, to: &Addr, sport: u16, payload: &[u8]) -> u8 {
        let mut m = Vec::with_capacity(22 + payload.len());
        m.push(1);
        m.push(to.fam);
        m.extend_from_slice(&to.ip);
        m.extend_from_slice(&to.port.to_be_bytes());
        m.extend_from_slice(&sport.to_be_bytes());
        m.extend_from_slice(payload);
        let mut r = [0u8; 16];
        // A first send may only resolve the next hop (status 1): a few tries, a moment apart.
        for _ in 0..8 {
            let n = self.call(&m, &mut r);
            if n == 1 && r[0] != 1 {
                return r[0];
            }
            ferros_sys::sleep_until(now() + ferros_sys::freq() / 500);
        }
        1
    }
    /// One datagram waiting on `port`, as (from, payload).
    fn recv(&self, port: u16, out: &mut [u8]) -> Option<(Addr, usize)> {
        let n = self.call(&[2, (port >> 8) as u8, port as u8], out);
        if n == 0 || n == ERR || n < 21 {
            return None;
        }
        let n = (n as usize).min(out.len());
        let mut ip = [0u8; 16];
        ip.copy_from_slice(&out[1..17]);
        let from = Addr { fam: out[0], ip, port: u16::from_be_bytes([out[17], out[18]]) };
        out.copy_within(21..n, 0);
        Some((from, n - 21))
    }
    /// Join a multicast group (op 8): the stack reports our membership so a snooping access point forwards the group to us.
    fn join(&self, fam: u8, group: [u8; 16]) {
        let mut m = [0u8; 18];
        m[0] = 8;
        m[1] = fam;
        m[2..].copy_from_slice(&group);
        let mut r = [0u8; 16];
        let _ = self.call(&m, &mut r);
    }
    /// INFO's flags byte (bit 0 IPv4 bound, bit 1 IPv6 global) and whether the stack answered at all.
    fn flags(&self) -> Option<u8> {
        let mut out = [0u8; 64];
        let n = self.call(&[3], &mut out);
        (n >= 51 && n != ERR).then_some(out[50])
    }
    /// The A record for `host` from the DHCP-given DNS server (payloads/httptest's resolver), or None.
    fn resolve(&self, host: &[u8]) -> Option<[u8; 4]> {
        let mut info = [0u8; 64];
        let n = self.call(&[3], &mut info);
        if n < 55 || n == ERR || info[50] & 1 == 0 || info[51..55] == [0; 4] {
            return None;
        }
        let mut q = Vec::with_capacity(300);
        q.extend_from_slice(&[1, 4]);
        let mut dst = [0u8; 16];
        dst[..4].copy_from_slice(&info[51..55]);
        q.extend_from_slice(&dst);
        q.extend_from_slice(&53u16.to_be_bytes());
        q.extend_from_slice(&DNS_PORT.to_be_bytes());
        let id = (now() as u16) | 1;
        q.extend_from_slice(&id.to_be_bytes());
        q.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        for label in host.split(|&c| c == b'.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label);
        }
        q.extend_from_slice(&[0, 0, 1, 0, 1]);
        let mut r = [0u8; 16];
        let mut out = [0u8; 1024];
        for _ in 0..3 {
            for _ in 0..50 {
                if self.call(&q, &mut r) == 1 && r[0] != 1 {
                    break;
                }
                ferros_sys::sleep_until(now() + ferros_sys::freq() / 500);
            }
            let t = now();
            while now() - t < ferros_sys::freq() {
                let n = self.call(&[2, (DNS_PORT >> 8) as u8, DNS_PORT as u8], &mut out);
                if n == 0 || n == ERR || n < 33 {
                    ferros_sys::sleep_until(now() + ferros_sys::freq() / 500);
                    continue;
                }
                let m = &out[21..(n as usize).min(out.len())];
                if m[0..2] != id.to_be_bytes() {
                    continue;
                }
                let skip = |m: &[u8], mut i: usize| -> usize {
                    while i < m.len() {
                        if m[i] == 0 {
                            return i + 1;
                        }
                        if m[i] & 0xC0 == 0xC0 {
                            return i + 2;
                        }
                        i += 1 + m[i] as usize;
                    }
                    m.len()
                };
                let an = u16::from_be_bytes([m[6], m[7]]);
                let mut i = skip(m, 12) + 4;
                for _ in 0..an {
                    i = skip(m, i);
                    if i + 10 > m.len() {
                        break;
                    }
                    let (ty, len) = (u16::from_be_bytes([m[i], m[i + 1]]), u16::from_be_bytes([m[i + 8], m[i + 9]]) as usize);
                    i += 10;
                    if ty == 1 && len == 4 && i + 4 <= m.len() {
                        return Some([m[i], m[i + 1], m[i + 2], m[i + 3]]);
                    }
                    i += len;
                }
                return None;
            }
        }
        None
    }

    /// One HTTP POST of `body` to `/` on `ip:80` (the `ip` process's client TCP, ops 4-7), as (status, body); None when the connection or the exchange fails, or takes longer than 16 s.
    fn post(&self, ip: [u8; 4], host: &[u8], body: &[u8]) -> Option<(u16, Vec<u8>)> {
        let f = ferros_sys::freq();
        let deadline = now() + 16 * f;
        let mut c = [0u8; 20];
        c[0] = 4;
        c[1] = 4;
        c[2..6].copy_from_slice(&ip);
        c[18..20].copy_from_slice(&80u16.to_be_bytes());
        let mut r = [0u8; 16];
        if self.call(&c, &mut r) != 2 || r[0] != 0 {
            return None;
        }
        let h = r[1];
        let mut req = Vec::with_capacity(160 + body.len());
        req.extend_from_slice(b"POST / HTTP/1.1\r\nHost: ");
        req.extend_from_slice(host);
        req.extend_from_slice(b"\r\nUser-Agent: ferros\r\nContent-Type: application/octet-stream\r\nConnection: close\r\nContent-Length: ");
        let mut digits = [0u8; 20];
        let mut k = digits.len();
        let mut v = body.len();
        loop {
            k -= 1;
            digits[k] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        req.extend_from_slice(&digits[k..]);
        req.extend_from_slice(b"\r\n\r\n");
        req.extend_from_slice(body);
        let (mut sent, mut connected) = (0usize, false);
        let mut got = Vec::new();
        let mut out = alloc::vec![0u8; 4096];
        let mut state = 0u8;
        while now() < deadline {
            if connected && sent < req.len() {
                let n = (req.len() - sent).min(1400);
                let mut s = Vec::with_capacity(2 + n);
                s.extend_from_slice(&[5, h]);
                s.extend_from_slice(&req[sent..sent + n]);
                if self.call(&s, &mut r) == 3 && r[0] == 0 {
                    sent += u16::from_be_bytes([r[1], r[2]]) as usize;
                }
            }
            let n = self.call(&[6, h], &mut out);
            if n == 0 || n == ERR {
                break;
            }
            let n = (n as usize).min(out.len());
            state = out[0];
            connected |= state >= 2;
            got.extend_from_slice(&out[1..n]);
            if state >= 3 && n == 1 {
                break;
            }
            if n == 1 {
                ferros_sys::sleep_until(now() + f / 1000);
            }
        }
        let _ = self.call(&[7, h], &mut r);
        if state != 3 {
            return None;
        }
        // The status line's code, then everything after the blank line (the worker answers with a Content-Length and closes, as asked).
        let end = got.windows(4).position(|w| w == b"\r\n\r\n")?;
        let code = got.get(9..12).and_then(|c| core::str::from_utf8(c).ok()).and_then(|c| c.parse::<u16>().ok())?;
        Some((code, got[end + 4..].to_vec()))
    }

    /// Our IPv4 subnet's broadcast address (address | !mask), once bound.
    fn subnet_broadcast(&self) -> Option<[u8; 4]> {
        let mut out = [0u8; 64];
        let n = self.call(&[3], &mut out);
        if n < 51 || n == ERR || out[50] & 1 == 0 {
            return None;
        }
        let mut b = [0u8; 4];
        for k in 0..4 {
            b[k] = out[6 + k] | !out[10 + k];
        }
        Some(b)
    }
}

/// Where this device stands with the identity whose roots it holds (the `photon` endpoint's op 1 state byte; pairing-v2's lamp). 0 no roots, 1 joining (the identity has a fleet, this device is not in it yet: RED, words shown), 2 bound waiting for the fleet key, 3 a member with the key, 4 looking the identity up, 5 fresh (fgtw.org holds no chain and no fleet device is heard: it may be minted), 6 wrong fleet (the seed's chain for this handle has another identity's genesis), 7 unreachable (no peer heard and no seed answer: GREY, never a verdict).
const V_NONE: u8 = 0;
const V_JOIN: u8 = 1;
const V_LOOKING: u8 = 4;
const V_FRESH: u8 = 5;
const V_WRONG: u8 = 6;
const V_UNREACHABLE: u8 = 7;

/// The seed (photon docs/peers-are-fgtw.md: peers first, the seed last). Plain HTTP on port 80, which fgtw.org serves (checked 2026-10-28; Claude memories/fgtw_plain_http_for_ferros.md): every call is one POST of a signed VSF document, and every answer is verified and folded here, so the transport carries no trust.
const SEED_HOST: &[u8] = b"fgtw.org";
const DNS_PORT: u16 = 53054;

/// fgtw's transport on ferros: one plain-HTTP POST of a request to fgtw.org through the `ip` process (Udp::post), the address resolved once and again after a failure. fgtw (feature `protocol`) builds every request and reads every answer; this only carries bytes.
struct SeedHttp<'a> {
    udp: &'a Udp,
    ip: core::cell::Cell<Option<[u8; 4]>>,
}

impl fgtw::client::FgtwTransport for SeedHttp<'_> {
    fn post(&self, body: Vec<u8>) -> Result<fgtw::client::FgtwResponse, alloc::string::String> {
        let ip = match self.ip.get() {
            Some(ip) => ip,
            None => {
                let ip = self.udp.resolve(SEED_HOST).ok_or_else(|| alloc::string::String::from("fgtw.org does not resolve"))?;
                self.ip.set(Some(ip));
                ip
            }
        };
        match self.udp.post(ip, SEED_HOST, &body) {
            Some((status, body)) => Ok(fgtw::client::FgtwResponse { status, body }),
            None => {
                self.ip.set(None);
                Err("fgtw.org did not answer".into())
            }
        }
    }
}

/// fgtw's entropy (fleet-key minting): the kernel's stream, through this process's entropy grant.
fn kernel_entropy(buf: &mut [u8]) {
    for chunk in buf.chunks_mut(4096) {
        ferros_sys::entropy(cap(unsafe { GRANTS }, S_ENTROPY), chunk);
    }
}

/// Eagle oscillations now, from the kernel's UTC wall clock (0 until it has one; the app treats a zero stamp as a frame from a clockless device).
fn eagle() -> i64 {
    match ferros_sys::utc_time() {
        Some((secs, ticks, hz)) => join::eagle_from_utc(secs, ticks, hz),
        None => 0,
    }
}

fn now_us() -> u64 {
    now() / (ferros_sys::freq() / 1_000_000).max(1)
}

struct Sponsor {
    key: [u8; 32],
    addr: Addr,
    /// The request stamp this sponsor was handed last (0 = none yet).
    asked_t: i64,
    heard_us: u64,
    online: bool,
}

struct Client {
    log: Log,
    udp: Udp,
    keys: DeviceKeys,
    me: [u8; 32],
    identity_seed: Option<[u8; 32]>,
    hp: Option<[u8; 32]>,
    bound: bool,
    fleet_key: Option<[u8; 32]>,
    pt: Pt,
    sponsors: Vec<Sponsor>,
    /// This device's current binding request and its stamp (re-minted every 210 s while unbound, as the app re-posts).
    request_t: i64,
    request_frame: Vec<u8>,
    last_beacon_us: u64,
    last_ping_us: u64,
    last_words_us: u64,
    /// When the vault was last asked for the roots while there were none (the attest screen writes them once a person mints this device's identity).
    last_load_us: u64,
    /// Where this device stands with the identity (V_*), and when the seed was last asked.
    verdict: u8,
    last_lookup_us: u64,
    /// The seed's address, once resolved.
    seed_ip: Option<[u8; 4]>,
    /// A sponsor named at spawn (its address; the key is learnt from its frames), handed the request directly.
    given: Option<Addr>,
    given_asked_t: i64,
}

impl Client {
    fn store_get(&self, key: &[u8], buf: &mut [u8]) -> Option<usize> {
        ferros_sys::store_get(cap(self.log.grants, S_STORE), key, buf)
    }
    fn store_set(&self, key: &[u8], value: &[u8]) -> bool {
        ferros_sys::store_set(cap(self.log.grants, S_STORE), key, value)
    }

    fn load(&mut self) {
        self.load_saying(true);
    }

    /// `load`, saying where the client stands only when `say` (the two-second look for roots is quiet until they appear).
    fn load_saying(&mut self, say: bool) {
        let mut b = [0u8; 32];
        if self.given.is_none() {
            let mut g = [0u8; 19];
            if self.store_get(b"sponsor", &mut g) == Some(19) && (g[0] == 4 || g[0] == 6) {
                let mut ip = [0u8; 16];
                ip.copy_from_slice(&g[1..17]);
                self.given = Some(Addr { fam: g[0], ip, port: u16::from_be_bytes([g[17], g[18]]) });
            }
        }
        if self.store_get(b"identity_seed", &mut b) == Some(32) {
            self.identity_seed = Some(b);
        }
        if self.store_get(b"handle_proof", &mut b) == Some(32) {
            self.hp = Some(b);
        }
        if self.store_get(b"fleet_key", &mut b) == Some(32) {
            self.fleet_key = Some(b);
        }
        // The chain we hold, if bound before: folded again against our roots, so a bad write never binds us.
        if let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) {
            let mut chain = alloc::vec![0u8; 128 * 1024];
            if let Some(n) = self.store_get(b"chain", &mut chain) {
                if n <= chain.len() {
                    if let Ok((blob, bound)) = join::adopt_chain(&hp, &seed, &self.me, &chain[..n]) {
                        self.bound = bound;
                        self.log.put(b"photon: chain held, ");
                        self.log.dec(blob.ops.len() as u64);
                        self.log.put(if bound { b" op(s), this device is a member" } else { b" op(s), this device is not in it" });
                        self.log.line();
                    }
                }
            }
        }
        if !say && self.hp.is_none() {
            return;
        }
        self.log.put(b"photon: device key ");
        self.log.fp(&self.me);
        self.log.put(match (self.hp.is_some(), self.bound, self.fleet_key.is_some()) {
            (false, _, _) => b" - no identity on this device yet: waiting for the person to attest (the attest screen writes the roots)",
            (true, false, _) => b" - paired, waiting to be bound by a sponsor on this network",
            (true, true, false) => b" - bound, waiting for the fleet key",
            (true, true, true) => b" - a fleet member with the fleet key",
        });
        self.log.line();
    }

    fn show_words(&mut self) {
        let (Some(seed), false) = (self.identity_seed, self.bound) else { return };
        let words = join::pair_words(&self.me, &seed);
        self.log.put(b"photon: pairing words - ");
        self.log.put(words.as_bytes());
        self.log.line();
    }

    fn send_all(&mut self, out: Vec<(Addr, Vec<u8>)>) {
        for (to, d) in out {
            self.udp.send(&to, join::PORT, &d);
        }
    }

    /// The beacon to the app's two multicast groups and, as the app also does, to the subnet broadcast — many access points pass no multicast between wireless clients (this one, 2026-10-26).
    fn beacon(&mut self) {
        let Some(hp) = self.hp else { return };
        let frame = join::beacon(hp, join::PORT, self.me, eagle());
        let mut v4 = [0u8; 16];
        v4[..4].copy_from_slice(&join::BEACON_V4);
        self.udp.send(&Addr { fam: 4, ip: v4, port: join::BEACON_PORT }, join::BEACON_PORT, &frame);
        self.udp.send(&Addr { fam: 6, ip: join::BEACON_V6, port: join::BEACON_PORT }, join::BEACON_PORT, &frame);
        if let Some(b) = self.udp.subnet_broadcast() {
            let mut ip = [0u8; 16];
            ip[..4].copy_from_slice(&b);
            self.udp.send(&Addr { fam: 4, ip, port: join::BEACON_PORT }, join::BEACON_PORT, &frame);
        }
        // A sponsor named at spawn: the request goes to it directly, once per stamp.
        if let Some(to) = self.given {
            if !self.bound && !self.request_frame.is_empty() && self.given_asked_t != self.request_t {
                self.given_asked_t = self.request_t;
                self.log.put(b"photon: handing our binding request to the sponsor named at spawn, ");
                self.log.addr(&to);
                self.log.line();
                let f = self.request_frame.clone();
                let out = self.pt.send(to, f, now_us(), eagle());
                self.send_all(out);
            }
        }
    }

    /// A beacon on 4384: a device of our fleet on this network is a sponsor - remembered, and handed our binding request while we are unbound.
    fn on_beacon(&mut self, from: Addr, bytes: &[u8]) {
        let Some((hp, port, Some(key))) = join::parse_beacon(bytes) else { return };
        if Some(hp) != self.hp || key == self.me {
            return;
        }
        let addr = Addr { port, ..from };
        let t = now_us();
        let idx = match self.sponsors.iter().position(|s| s.key == key) {
            Some(i) => {
                self.sponsors[i].addr = addr;
                self.sponsors[i].heard_us = t;
                i
            }
            None => {
                self.log.put(b"photon: fleet device ");
                self.log.fp(&key);
                self.log.put(b" beaconing at ");
                self.log.addr(&addr);
                self.log.line();
                self.sponsors.push(Sponsor { key, addr, asked_t: 0, heard_us: t, online: false });
                // A device beaconing under our handle: the identity has a fleet, whatever the seed said.
                if matches!(self.verdict, V_LOOKING | V_FRESH | V_UNREACHABLE) {
                    self.verdict = V_JOIN;
                }
                self.sponsors.len() - 1
            }
        };
        if !self.bound && !self.request_frame.is_empty() && self.sponsors[idx].asked_t != self.request_t {
            self.sponsors[idx].asked_t = self.request_t;
            self.log.put(b"photon: handing our binding request to ");
            self.log.fp(&key);
            self.log.line();
            let frame = self.request_frame.clone();
            let out = self.pt.send(addr, frame, t, eagle());
            self.send_all(out);
        }
    }

    /// A chain this device should hold: kept, and the device is a member when the fold names it.
    fn hold_chain(&mut self, chain: &[u8]) -> bool {
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return false };
        match join::adopt_chain(&hp, &seed, &self.me, chain) {
            Ok((_, true)) => {
                if self.store_set(b"chain", chain) {
                    self.bound = true;
                }
                self.bound
            }
            _ => false,
        }
    }

    /// Everything held for the identity whose roots were in the vault: its roots, chain standing, key, sponsors and request.
    fn forget_identity(&mut self) {
        self.identity_seed = None;
        self.hp = None;
        self.bound = false;
        self.fleet_key = None;
        self.sponsors.clear();
        self.request_t = 0;
        self.request_frame.clear();
        self.verdict = V_NONE;
    }

    /// fgtw's transport for this call, starting from the address resolved last.
    fn seed(&self) -> SeedHttp<'_> {
        SeedHttp { udp: &self.udp, ip: core::cell::Cell::new(self.seed_ip) }
    }

    /// Look the identity up (pairing-v2: peers first, the seed last) and take fgtw's verdict on it (`fgtw::attest::attest_verdict`, the same function Photon's probe runs). A fleet device heard beaconing under our handle already said the fleet exists, so an undecided verdict becomes joining.
    fn lookup(&mut self) {
        let Some(hp) = self.hp else { return };
        if self.bound {
            return;
        }
        let heard = !self.sponsors.is_empty();
        fgtw::set_clock(eagle());
        let t = self.seed();
        let fetched = fgtw::client::fetch(&t, &hp);
        let ip = t.ip.get();
        self.seed_ip = ip;
        let answer = match &fetched {
            Ok(Some(b)) => fgtw::attest::SeedAnswer::Chain(b),
            Ok(None) => fgtw::attest::SeedAnswer::NotFound,
            Err(_) => fgtw::attest::SeedAnswer::Unreachable,
        };
        use fgtw::attest::AttestVerdict as A;
        let v = match fgtw::attest::attest_verdict(&hp, &self.me, None, &[], answer) {
            A::Member => {
                if let Ok(Some(b)) = &fetched {
                    if let Ok(bytes) = b.to_vsf_bytes() {
                        self.hold_chain(&bytes);
                    }
                }
                V_JOIN
            }
            A::JoinOurs => V_JOIN,
            A::WrongFleet => V_WRONG,
            A::Fresh => if heard { V_JOIN } else { V_FRESH },
            A::Unreachable | A::Unverifiable(_) => if heard { V_JOIN } else { V_UNREACHABLE },
        };
        if v != self.verdict {
            self.log.put(match v {
                V_JOIN => b"photon: this identity has a fleet: waiting to be added from one of its devices".as_slice(),
                V_FRESH => b"photon: fgtw.org holds no chain for this handle and no fleet device is heard: it may be minted",
                V_WRONG => b"photon: fgtw.org's chain for this handle claims another slot",
                _ => b"photon: neither a fleet device nor fgtw.org answers: no verdict (GREY)",
            });
            self.log.line();
        }
        self.verdict = v;
    }

    /// Mint this identity (the attest screen asks, after the permanence warning and the retype), only on a fresh verdict: fgtw's `ensure_member` (genesis signed by this device and the identity, published, fetched back: the seed's slot settles on one genesis, and only a chain naming this device counts), then the fleet key, founded by this device as the only member (`mint_fleet_key`, from the kernel's entropy). Returns 0 minted, 2 not fresh, 4 another genesis won the handle (it has a fleet now), 5 the seed refused or did not answer.
    fn mint(&mut self) -> u8 {
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return 1 };
        if self.verdict != V_FRESH {
            return 2;
        }
        fgtw::set_clock(eagle());
        let t = self.seed();
        let member = fgtw::client::ensure_member(&t, &self.keys, &hp, &seed);
        let (chain, key) = match &member {
            Ok(()) => {
                fgtw::set_clock(eagle());
                let chain = fgtw::client::fetch(&t, &hp).ok().flatten().and_then(|b| b.to_vsf_bytes().ok());
                fgtw::set_clock(eagle());
                (chain, fgtw::client::mint_fleet_key(&t, &hp, &self.keys, &[self.me], &seed))
            }
            Err(_) => (None, Err(alloc::string::String::new())),
        };
        self.seed_ip = t.ip.get();
        match member {
            Ok(()) => {
                if let Some(c) = chain {
                    self.hold_chain(&c);
                }
                self.log.put(b"photon: MINTED: this device is the identity's first member");
                self.log.line();
                match key {
                    Ok((_, k)) => {
                        self.fleet_key = Some(k);
                        if self.store_set(b"fleet_key", &k) {
                            self.log.put(b"photon: the fleet key is founded and ours");
                        } else {
                            self.log.put(b"photon: the fleet key is founded but the vault refused it");
                        }
                    }
                    Err(e) => {
                        self.log.put(b"photon: the fleet key was not founded: ");
                        self.log.put(e.as_bytes());
                    }
                }
                self.log.line();
                0
            }
            Err(e) if e.contains("not in the fleet") => {
                self.verdict = V_JOIN;
                4
            }
            Err(e) => {
                self.log.put(b"photon: the genesis did not land: ");
                self.log.put(e.as_bytes());
                self.log.line();
                5
            }
        }
    }

    /// Mint (or re-mint) this device's binding request.
    fn mint_request(&mut self) {
        let (Some(seed), Some(hp)) = (self.identity_seed, self.hp) else { return };
        let t = eagle();
        let req = join::make_bind_request(&self.keys, &seed, &hp, t);
        match join::bind_req_vsf(&req, &hp, &self.keys, t) {
            Ok(f) => {
                self.request_t = t;
                self.request_frame = f;
                self.log.put(b"photon: binding request minted (");
                self.log.dec(self.request_frame.len() as u64);
                self.log.put(b" B)");
                self.log.line();
            }
            Err(e) => {
                self.log.put(b"photon: binding request failed: ");
                self.log.put(e.as_bytes());
                self.log.line();
            }
        }
    }

    /// A frame on 4383 after PT: presence first (wire), then the envelope frames (join).
    fn on_frame(&mut self, from: Addr, bytes: &[u8], via_transfer: bool) {
        let t = now_us();
        match wire::parse(bytes) {
            Some(Frame::Ping { key, hp, .. }) => {
                // Answer every ping with a tail-less pong: a device that knows us takes it as presence, one that does not drops it.
                let pong = wire::pong(&self.keys_ed(), eagle(), hp, None);
                self.udp.send(&from, join::PORT, &pong);
                if let Some(s) = self.sponsors.iter_mut().find(|s| s.key == key) {
                    s.addr = from;
                    s.heard_us = t;
                }
                return;
            }
            Some(Frame::Pong { key, .. }) => {
                if let Some(s) = self.sponsors.iter_mut().find(|s| s.key == key) {
                    s.heard_us = t;
                    if !s.online {
                        s.online = true;
                        self.log.put(b"photon: fleet device ");
                        self.log.fp(&key);
                        self.log.put(b" answers our pings");
                        self.log.line();
                    }
                }
                return;
            }
            Some(Frame::PtAck { .. }) => return,
            // A beacon on the frame port (the app broadcasts its pt_disc there too).
            Some(Frame::Other(ref name)) if name == "pt_disc" => {
                self.on_beacon(from, bytes);
                return;
            }
            Some(Frame::Photon { .. }) | Some(Frame::Ack { .. }) => {
                self.log.put(b"photon: a photon arrived - not read yet (lanes follow the join)");
                self.log.line();
                return;
            }
            Some(Frame::Other(_)) | None => {}
        }
        // An envelope frame: acknowledged at the transport like every reliable frame the app takes in (not when it came as a transfer, whose DONE already said so).
        let parsed = join::parse_frame(bytes);
        if !via_transfer {
            let ack = pt::packet_ack(eagle(), bytes);
            self.udp.send(&from, join::PORT, &ack);
        }
        match parsed {
            Ok((Inbound::FleetResp { hp, chain }, signer)) => {
                let (Some(our_hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
                if hp != our_hp {
                    return;
                }
                match join::adopt_chain(&hp, &seed, &self.me, &chain) {
                    Ok((blob, bound)) => {
                        self.log.put(b"photon: our fleet's chain from ");
                        self.log.fp(&signer);
                        self.log.put(b" - ");
                        self.log.dec(blob.ops.len() as u64);
                        self.log.put(if bound { b" op(s), THIS DEVICE IS BOUND" } else { b" op(s), not bound yet" });
                        self.log.line();
                        if !self.store_set(b"chain", &chain) {
                            self.log.put(b"photon: the vault did not take the chain");
                            self.log.line();
                        }
                        if bound && !self.bound {
                            self.bound = true;
                        }
                    }
                    Err(e) => {
                        self.log.put(b"photon: a chain from ");
                        self.log.fp(&signer);
                        self.log.put(b" was refused: ");
                        self.log.put(e.as_bytes());
                        self.log.line();
                    }
                }
            }
            Ok((Inbound::FanoutResp { hp, blob }, signer)) => {
                let (Some(our_hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
                if hp != our_hp {
                    return;
                }
                match join::open_fanout(&hp, &blob, &self.keys, &seed) {
                    Ok(Some((revision, key))) => {
                        self.log.put(b"photon: fan-out revision ");
                        self.log.dec(revision);
                        self.log.put(b" from ");
                        self.log.fp(&signer);
                        self.log.put(if self.fleet_key == Some(key) { b" - the key we hold" } else { b" - THE FLEET KEY IS OURS" });
                        self.log.line();
                        if self.fleet_key != Some(key) {
                            self.fleet_key = Some(key);
                            if !self.store_set(b"fleet_key", &key) {
                                self.log.put(b"photon: the vault did not take the fleet key");
                                self.log.line();
                            }
                        }
                    }
                    Ok(None) => {
                        self.log.put(b"photon: a fan-out from ");
                        self.log.fp(&signer);
                        self.log.put(b" holds no wrap for us yet");
                        self.log.line();
                    }
                    Err(e) => {
                        self.log.put(b"photon: a fan-out was refused: ");
                        self.log.put(e.as_bytes());
                        self.log.line();
                    }
                }
            }
            Ok((Inbound::Other(name), signer)) => {
                self.log.put(b"photon: frame ");
                self.log.put(name.as_bytes());
                self.log.put(b" from ");
                self.log.fp(&signer);
                self.log.put(b" (not handled)");
                self.log.line();
            }
            Err(_) => {}
        }
    }

    fn keys_ed(&self) -> ed25519_dalek::SigningKey {
        self.keys.signing_key()
    }

    fn pings(&mut self) {
        let t = now_us();
        let ping = wire::ping(&self.keys_ed(), eagle());
        let targets: Vec<Addr> = self.sponsors.iter().filter(|s| t.saturating_sub(s.heard_us) < 120_000_000).map(|s| s.addr).collect();
        for a in targets {
            self.udp.send(&a, join::PORT, &ping);
        }
    }

    fn run(&mut self) -> ! {
        let mut buf = alloc::vec![0u8; 2048];
        // Open both ports before anything can arrive, and join the app's beacon groups.
        for p in [join::PORT, join::BEACON_PORT] {
            let mut out = [0u8; 32];
            let _ = self.udp.call(&[2, (p >> 8) as u8, p as u8], &mut out);
        }
        let mut v4 = [0u8; 16];
        v4[..4].copy_from_slice(&join::BEACON_V4);
        self.udp.join(4, v4);
        self.udp.join(6, join::BEACON_V6);
        self.load();
        if self.hp.is_some() {
            self.verdict = V_LOOKING;
        }
        self.show_words();
        if !self.bound {
            self.mint_request();
        }
        loop {
            let t = now_us();
            let mut busy = false;
            while let Some((from, n)) = self.udp.recv(join::PORT, &mut buf) {
                busy = true;
                let datagram = buf[..n].to_vec();
                let out = self.pt.receive(from, &datagram, t, eagle());
                self.send_all(out);
                for e in self.pt.take_events() {
                    match e {
                        Event::Frame { from, bytes, signer } => self.on_frame(from, &bytes, signer.is_some()),
                        Event::Sent { to, ok, .. } => {
                            if !ok {
                                self.log.put(b"photon: a frame to ");
                                self.log.addr(&to);
                                self.log.put(b" was given up on");
                                self.log.line();
                            }
                        }
                    }
                }
            }
            while let Some((from, n)) = self.udp.recv(join::BEACON_PORT, &mut buf) {
                busy = true;
                let datagram = buf[..n].to_vec();
                self.on_beacon(from, &datagram);
            }
            let out = self.pt.tick(t, eagle());
            self.send_all(out);
            // The map asking for the Photon circle's front.
            let mut req = [0u8; 16];
            let mut tok = [0u8; 32];
            let n = ferros_sys::recv(cap(self.log.grants, S_SERVE), &mut req, &mut tok, true);
            if n != ERR && n != ferros_sys::NOTHING {
                busy = true;
                let mut reply = Vec::with_capacity(400);
                // Op 2: mint (the attest screen, after the permanence warning and the retype) → [result: 0 minted, else why not], then the state as op 1 gives it.
                if n >= 1 && req[0] == 2 {
                    let r = self.mint();
                    reply.push(r);
                }
                // Op 3: the attest screen replaced the roots (a new handle on a device that was not yet bound): forget everything of the old identity and look the new one up.
                if n >= 1 && req[0] == 3 {
                    self.forget_identity();
                    self.load_saying(true);
                    if self.hp.is_some() {
                        self.verdict = V_LOOKING;
                        self.last_lookup_us = 0;
                        if !self.bound {
                            self.mint_request();
                        }
                    }
                }
                let state: u8 = match (self.hp.is_some(), self.bound, self.fleet_key.is_some()) {
                    (false, _, _) => V_NONE,
                    (true, false, _) => self.verdict,
                    (true, true, false) => 2,
                    (true, true, true) => 3,
                };
                reply.push(state);
                reply.push(self.sponsors.len().min(255) as u8);
                if let (Some(seed), false, V_JOIN) = (self.identity_seed, self.bound, self.verdict) {
                    reply.extend_from_slice(join::pair_words(&self.me, &seed).as_bytes());
                }
                svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, reply.as_ptr() as u64 - self.udp.base, reply.len() as u64, 0, 0);
            }
            // No roots yet: the person has not attested on this device. Look again every two seconds; the attest screen (payloads/ahau) writes them, and the join starts from there without a restart.
            if self.hp.is_none() && t.saturating_sub(self.last_load_us) >= 2_000_000 {
                self.last_load_us = t;
                self.load_saying(false);
                if self.hp.is_some() {
                    self.verdict = V_LOOKING;
                    self.last_lookup_us = 0;
                    self.show_words();
                    if !self.bound {
                        self.mint_request();
                    }
                }
            }
            // The verdict: looked up once the network is up, and again every 30 s while GREY or while a fresh handle waits (someone else may mint it meanwhile).
            if self.hp.is_some() && !self.bound && matches!(self.verdict, V_LOOKING | V_UNREACHABLE | V_FRESH) && t.saturating_sub(self.last_lookup_us) >= if self.verdict == V_LOOKING { 2_000_000 } else { 30_000_000 } {
                self.last_lookup_us = t;
                if self.udp.flags().is_some_and(|f| f & 1 != 0) {
                    self.lookup();
                }
            }
            if self.hp.is_some() && t.saturating_sub(self.last_beacon_us) >= 3_000_000 {
                self.last_beacon_us = t;
                self.beacon();
            }
            if self.bound && t.saturating_sub(self.last_ping_us) >= 10_000_000 {
                self.last_ping_us = t;
                self.pings();
            }
            if !self.bound && self.hp.is_some() {
                if t.saturating_sub(self.last_words_us) >= 60_000_000 {
                    self.last_words_us = t;
                    self.show_words();
                }
                // The app re-posts its request before the registry's 5-minute freshness lapses; the same stamp discipline here, so a sponsor's candidate stays fresh.
                if self.request_t != 0 && (eagle() - self.request_t) > 210 * vsf::OSCILLATIONS_PER_SECOND as i64 {
                    self.mint_request();
                    for s in self.sponsors.iter_mut() {
                        s.asked_t = 0;
                    }
                }
            }
            if !busy && !self.pt.busy() {
                ferros_sys::sleep_until(now() + ferros_sys::freq() / 50);
            } else {
                ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
            }
        }
    }
}

static mut GRANTS: u64 = 0;

extern "C" fn work(_: u64) -> ! {
    let grants = unsafe { GRANTS };
    let base = ferros_sys::arc_base();
    let mut log = Log { grants, buf: Vec::new() };
    let udp = Udp { grants, base };
    // The network first: the stack must be up for anything below to matter.
    let mut waited = 0;
    while udp.flags().map_or(true, |f| f & 1 == 0) {
        ferros_sys::sleep_until(now() + ferros_sys::freq() / 4);
        waited += 1;
        if waited == 240 {
            log.put(b"photon: no IPv4 lease after a minute - still waiting");
            log.line();
        }
    }
    // The device key, rooted in the ira.
    let Some(fingerprint) = ferros_sys::ira_key(cap(grants, S_IRA), "photon device") else {
        log.put(b"photon: no ira-derived key (no Ira grant, or the kernel has no ira this boot)");
        log.line();
        ferros_sys::exit(1)
    };
    let t0 = now();
    let keys = DeviceKeys::derive(&fingerprint);
    log.put(b"photon: device keys derived in ");
    log.dec(ferros_sys::us_since(t0) / 1000);
    log.put(b" ms");
    log.line();
    let me = keys.device_pubkey();
    let pt = Pt::new(keys.signing_key());
    let given = unsafe { GIVEN };
    let mut c = Client { log, udp, keys, me, identity_seed: None, hp: None, bound: false, fleet_key: None, pt, sponsors: Vec::new(), request_t: 0, request_frame: Vec::new(), last_beacon_us: 0, last_ping_us: 0, last_words_us: 0, last_load_us: 0, verdict: V_NONE, last_lookup_us: 0, seed_ip: None, given, given_asked_t: 0 };
    c.run()
}

static mut GIVEN: Option<Addr> = None;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_BYTES) };
    unsafe { GRANTS = grants };
    fgtw::set_entropy(kernel_entropy);
    if input_len >= 19 {
        let mut b = [0u8; 19];
        for (k, x) in b.iter_mut().enumerate() {
            *x = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + k as u64) as *const u8) };
        }
        let mut ip = [0u8; 16];
        ip.copy_from_slice(&b[1..17]);
        unsafe { GIVEN = Some(Addr { fam: b[0], ip, port: u16::from_be_bytes([b[17], b[18]]) }) };
    }
    let stack = unsafe { &mut *STACK.0.get() };
    ferros_sys::thread_spawn(work, stack, 0);
    ferros_sys::thread_exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
