//! Photon on ferros: the client that joins the owner's fleet over the LAN and stands as one of its devices (photon docs/pairing-v2.md "The ceremony over the mesh"; STATUS.md "Enrollment / pairing"). An EL0 process on the `ip` process's UDP: it beacons as the stock app does (`pt_disc` to the app's multicast groups on 4384), hears the owner's other devices beaconing under the same handle, hands the nearest sponsor its binding request as a `bind_req` over Photon Transport, takes the chain (`fleet_resp`) and the fan-out (`fanout_resp`) the sponsor pushes back, folds the chain against its own roots and opens the fan-out with its own ira, and from then on answers pings as a fleet member. Everything it speaks is photon_core (pt, join, wire), pinned byte for byte against the app by tools/photon-interop.
//!
//! Identity: the device key is rooted in the kernel's ira (`ira_key("photon device")` → `join::DeviceKeys::derive`), the same after a factory reset; the identity roots (`photon.identity_seed`, `photon.handle_proof`) are pushed into the vault scope `photon` at pairing by the dev box (tools/photon-interop `roots` + `ferros-bridge store put`) - the handle itself never reaches the phone. Once bound, the chain (`photon.chain`) and the fleet key (`photon.fleet_key`) live in the same scope. The 23 pairing words are printed on the console for the sponsor's human to type; the sponsor's screen also shows this device as a nearby candidate it can tap.
//!
//! Spawn: ferros-bridge spawn photon.bin --name photon --use udp --store photon --ira. Grant slots: 0 console, 1 udp (use), 2 store photon, 3 ira. Runs on its own 512 KiB thread stack (Falcon's signing key is 116 KiB by value) over a 768 KiB heap in `.data`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use ferros_sys::{cap, now, svc, ERR};
use photon_core::join::{self, DeviceKeys, Inbound};
use photon_core::pt::{self, Addr, Event, Pt};
use photon_core::wire::{self, Frame};

const HEAP_BYTES: usize = 768 * 1024;
const STACK_BYTES: usize = 512 * 1024;

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
    /// INFO's flags byte (bit 0 IPv4 bound, bit 1 IPv6 global) and whether the stack answered at all.
    fn flags(&self) -> Option<u8> {
        let mut out = [0u8; 64];
        let n = self.call(&[3], &mut out);
        (n >= 51 && n != ERR).then_some(out[50])
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
}

impl Client {
    fn store_get(&self, key: &[u8], buf: &mut [u8]) -> Option<usize> {
        ferros_sys::store_get(cap(self.log.grants, S_STORE), key, buf)
    }
    fn store_set(&self, key: &[u8], value: &[u8]) -> bool {
        ferros_sys::store_set(cap(self.log.grants, S_STORE), key, value)
    }

    fn load(&mut self) {
        let mut b = [0u8; 32];
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
        self.log.put(b"photon: device key ");
        self.log.fp(&self.me);
        self.log.put(match (self.hp.is_some(), self.bound, self.fleet_key.is_some()) {
            (false, _, _) => b" - not paired: push photon.identity_seed and photon.handle_proof into the vault (tools/photon-interop roots)",
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

    fn beacon(&mut self) {
        let Some(hp) = self.hp else { return };
        let frame = join::beacon(hp, join::PORT, self.me, eagle());
        let mut v4 = [0u8; 16];
        v4[..4].copy_from_slice(&join::BEACON_V4);
        self.udp.send(&Addr { fam: 4, ip: v4, port: join::BEACON_PORT }, join::BEACON_PORT, &frame);
        self.udp.send(&Addr { fam: 6, ip: join::BEACON_V6, port: join::BEACON_PORT }, join::BEACON_PORT, &frame);
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
        // Open both ports before anything can arrive.
        for p in [join::PORT, join::BEACON_PORT] {
            let mut out = [0u8; 32];
            let _ = self.udp.call(&[2, (p >> 8) as u8, p as u8], &mut out);
        }
        self.load();
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
    let mut c = Client { log, udp, keys, me, identity_seed: None, hp: None, bound: false, fleet_key: None, pt, sponsors: Vec::new(), request_t: 0, request_frame: Vec::new(), last_beacon_us: 0, last_ping_us: 0, last_words_us: 0 };
    c.run()
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_BYTES) };
    unsafe { GRANTS = grants };
    let stack = unsafe { &mut *STACK.0.get() };
    ferros_sys::thread_spawn(work, stack, 0);
    ferros_sys::thread_exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
