//! fgtw on ferros: the OS's identity service, part of attest, never an app (owner, 2026-11-04: "Photon is just an app. on non ferros it just happens to have the FGTW bits in it too. Just like Mahere, or Lumis"). Elsewhere these FGTW bits ship inside the Photon app; on ferros they are the system's, and Photon the app is something a person stows like any other. It looks the attesting identity up (its fleet's devices nearby first, fgtw.org last), mints a genesis when there is none, joins this device to the identity's fleet and stands as one of its devices, and leaves the fleet on request. It reads no photons: messages are the Photon app's.
//!
//! The fleet over the LAN (photon docs/pairing-v2.md "The ceremony over the mesh"; STATUS.md "Enrollment / pairing"): an EL0 process on the `ip` process's UDP, it beacons as the stock app does (`pt_disc` to the app's multicast groups on 4384), hears the owner's other devices beaconing under the same handle, hands the nearest sponsor its binding request as a `bind_req` over Photon Transport, takes the chain (`fleet_resp`) and the fan-out (`fanout_resp`) the sponsor pushes back, folds the chain against its own roots and opens the fan-out with its own ira, and from then on answers pings as a fleet member. Everything it speaks is photon_core (pt, join, wire), pinned byte for byte against the app by tools/photon-interop.
//!
//! Identity: the device key is rooted in the kernel's ira (`ira_key("fgtw device")` → `join::DeviceKeys::derive`; "photon device" until 2026-11-04, changed when no fleet held this device), the same after a factory reset; the identity roots (`identity_seed`, `handle_proof`) come from the attest screen (op 4, in RAM) or are pushed into the vault scope `fgtw.` by the dev box (tools/photon-interop `roots` + `ferros-bridge store put`) - the handle itself never reaches the device. Once bound, the chain (`fgtw.chain`) and the fleet key (`fgtw.fleet_key`) live in the same scope. A departure under way is kept there too (`fgtw.depart`, whatever unlock-at-boot says: it holds no root) until it completes, so a restart resumes it. The 23 pairing words are printed on the console for the sponsor's human to type; the sponsor's screen also shows this device as a nearby candidate it can tap.
//!
//! Started by init once a person has attested (payloads/init), or from the bridge: ferros-bridge spawn fgtw.bin [hex: fam, address 16, port BE — a sponsor to hand the binding request (and a departure request) to directly, for a network whose access point does not pass multicast between its wireless clients; the vault key `fgtw.sponsor` holds the same sponsor for the boot-started service, as a VSF document (ferros_kept::sponsor: `ferros-bridge sponsor <address> <port> <out.vsf>`, then `ferros-bridge store put fgtw.sponsor @<out.vsf>`)] --name fgtw --use udp --store fgtw --ira --serve fgtw --entropy --use nfc --use bt --unlock --store map --store kists. The `fgtw` endpoint (the attest screen's): every op replies [state][fleet devices heard][words], the state as `V_*` below (0 no roots, 1 joining, 2 bound without the key, 3 member, 4 looking, 5 fresh, 6 wrong fleet, 7 grey, 8 leaving, 10 selected, 11 joining but not yet confirmed by this device's person) and the words the 23 pairing words while joining once confirmed (none before, none while leaving); op 1 the state alone, op 2 `[2][seed]` mint, the handle typed again (a result byte first: 6 when it is not the one handed over), op 3 roots replaced on disk, op 4 `[4][seed][proof]` roots in RAM, op 5 `[5][intent]` leave the fleet (a result byte first; intent 1 to a new owner, 2 to the desk; the departure runs on fgtw's shared machine, depart.rs), op 32 `[32][1]` the join confirmed on this device (nothing of a join is sent before it: no binding request to the fleet or fgtw.org, no tap secret, no BLE or LAN beacon; owner 2026-11-22), `[32][0]` withdrawn. Grant slots: 0 console, 1 udp (use), 2 store fgtw, 3 ira, 4 serve fgtw, 5 entropy, 6 nfc (use), 7 bt (use), 8 the identity unlock right (bind when the fleet binds this device, release when a departure completes), 9 the vault scope `map.` and 10 the vault scope `kists.` (the person's identity space: their deck and their kists, joined to their fleet's, space.rs): while unbound, each binding request's tap secret is served on the NFC tag and its beacon id advertised over BLE (`radios`). Runs on its own 128 KiB thread stack (Falcon's degree-512 signing key is about 60 KiB by value) over a 160 KiB heap in `.data`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use ferros_sys::{cap, now, svc, ERR};
use fgtw::pq::FleetSigner;
use photon_core::join::{self, DeviceKeys, Inbound};
use photon_core::pt::{self, Addr, Event, Pt};
use photon_core::wire::{self, Frame};

mod add;
mod addview;
mod depart;
/// The facets (docs/FACETS.md): be you and the people on the `facets` endpoint, and the person's people added and listed.
mod facets;
mod kept;
mod route;
mod space;

/// The heap and the work thread's stack, in .bss: the entry's header (`ferrosRT`, ferros_rt's) tells the kernel how much zeroed memory follows the code, so the arc holds them (payload.ld). Was 160 KiB in .data to fit a 1 MiB arc: a re-minted binding request (an SLH-DSA signature, 7856 B) then found no free run once the client had run a while, and the client panicked (2026-10-28).
/// 8 MiB since 2026-11-17: at 2 MiB the Pro's service ended on "memory allocation of 67075 bytes failed" (b916) as the desktop's Photon, reloaded, began its sibling ceremony with this device's member key (baf3dec7 is a fleet member, so every Photon of the fleet takes it for a sibling until the alias cutover, PHOTON.md M4): its 573806-byte ClutchOffer arrives as a transfer this service takes in whole (photon_core pt, MAX_TRANSFER 4 MiB), beside the held chain (about 207 KB) and the bindings. A transfer as large as MAX_TRANSFER now fits beside the rest.
const HEAP_BYTES: usize = 8 << 20;
/// The work thread's stack. 128 KiB was not enough: minting the binding request (Falcon-512 and SLH-DSA-128s signatures) ran past its base. In .data that landed on the unused top of the heap; in .bss it landed on the allocator's lock byte, which then never cleared (2026-10-28: the client spun in `LockedHeap::alloc`). The stack is painted at start, and each minting logs how deep it went (`stack_used`).
const STACK_BYTES: usize = 1 << 20;
/// What an untouched stack byte holds.
const STACK_PAINT: u8 = 0xA5;

/// How much of the work thread's stack has been written: from the base, the first byte that is no longer the paint.
fn stack_used() -> usize {
    let s = unsafe { &*STACK.0.get() };
    STACK_BYTES - s.iter().position(|&b| b != STACK_PAINT).unwrap_or(STACK_BYTES)
}

#[repr(C, align(16))]
struct Region<const N: usize>(UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Region<N> {}
/// Both zero-initialised, so in `.bss`: the kernel zeroes the tail the header names.
static HEAP: Region<HEAP_BYTES> = Region(UnsafeCell::new([0; HEAP_BYTES]));
static STACK: Region<STACK_BYTES> = Region(UnsafeCell::new([0; STACK_BYTES]));
/// The network watcher's stack (`net_watch`): it only calls and stores, nothing allocated.
static WATCH_STACK: Region<{ 1 << 14 }> = Region(UnsafeCell::new([0; 1 << 14]));

use core::sync::atomic::{AtomicU64, Ordering};

/// The loop's doorbell for the network (`net_watch`): counted up at each answer of ip's WATCH, the idle wait on it (owner, 2026-11-10: "No polling!"). Until 2026-11-13 the loop asked ip RECV on both ports and INFO every 20 ms, 150 calls a second at rest.
static NET_BELL: AtomicU64 = AtomicU64::new(0);
/// The ports with news since the loop last looked: bit 0 `join::PORT`, bit 1 `join::BEACON_PORT` (the loop takes them with a swap).
static NET_PORTS: AtomicU64 = AtomicU64::new(0);
/// INFO's flags byte as the watch last said it, with bit 8 set once said; `NO_WATCH` while ip answers no WATCH (an ip from before op 11), and the loop asks every pass as before.
static NET_FLAGS: AtomicU64 = AtomicU64::new(0);
const NO_WATCH: u64 = u64::MAX;
/// The watch's id at ip (op 11; op 12 KICK would name it): "fgtw" in ASCII, one watcher in this process.
const WATCH_ID: [u8; 4] = *b"fgtw";

/// The network watcher thread: one WATCH held at ip on both ports and the flags, answered when a datagram comes for either or the flags change (a route, the call, the person), never on a clock; the news into `NET_PORTS` and `NET_FLAGS`, the loop rung on `NET_BELL`. The datagram it was told of goes back as seen, so a datagram the loop has not read yet never answers again.
extern "C" fn net_watch(grants: u64) -> ! {
    let ports = [join::PORT, join::BEACON_PORT];
    // 0xFF: no flags seen yet, so the first watch is answered at once with them.
    let (mut flags, mut seen) = (0xFFu8, 0u32);
    let mut req = [0u8; 23];
    let mut r = [0u8; 64];
    loop {
        req[0] = 11;
        // Level mode on the first watch (no flags seen yet): every datagram waiting counts, the seen number meaning nothing yet.
        req[1] = (flags == 0xFF) as u8;
        req[2] = flags;
        req[3..7].copy_from_slice(&WATCH_ID);
        req[7..11].copy_from_slice(&seen.to_be_bytes());
        // Deadline 0: none; the loop keeps its own timers.
        req[19..21].copy_from_slice(&ports[0].to_be_bytes());
        req[21..23].copy_from_slice(&ports[1].to_be_bytes());
        let n = ferros_sys::call(cap(grants, S_UDP), &req, &mut r);
        // An answer is 6 bytes and two for each port named. Anything else is an ip without WATCH (its [3]) or one going away: the loop asks every pass as before, and the watch is tried again 4 s on (WHY a timer: only a restarted ip can answer it, and nothing rings for that).
        if n == ERR || n < 6 {
            NET_FLAGS.store(NO_WATCH, Ordering::Release);
            NET_BELL.fetch_add(1, Ordering::AcqRel);
            ferros_sys::wake(&NET_BELL, 1);
            flags = 0xFF;
            ferros_sys::sleep_until(now() + 4 * ferros_sys::freq());
            continue;
        }
        flags = r[0];
        seen = u32::from_be_bytes([r[1], r[2], r[3], r[4]]);
        // WHY the min: the count is ip's word; only the ports the reply holds are read.
        let k = (r[5] as usize).min((n as usize - 6) >> 1).min(ports.len());
        let mut news = 0u64;
        for i in 0..k {
            let p = u16::from_be_bytes([r[6 + 2 * i], r[7 + 2 * i]]);
            news |= (p == ports[0]) as u64 | ((p == ports[1]) as u64) << 1;
        }
        NET_PORTS.fetch_or(news, Ordering::AcqRel);
        NET_FLAGS.store(1 << 8 | flags as u64, Ordering::Release);
        NET_BELL.fetch_add(1, Ordering::AcqRel);
        ferros_sys::wake(&NET_BELL, 1);
    }
}

/// The identity's lock bell (kernel SYS_BELL `BELL_IDENTITY`, 2026-11-17): the kernel writes its lock epoch here and wakes `id_watch` whenever it moves (the identity locked: the lock op, an unlock that locks what was open first, a release).
/// The loop compares it with the epoch the roots in hand were taken at (`Client::lock_epoch`) and wipes what it holds of the identity when they differ (`locked_out`).
/// Owner, 2026-11-17: "once we lock it there's ZERO way someone can get into those keys".
static ID_BELL: AtomicU64 = AtomicU64::new(0);
/// `id_watch`'s stack: it only waits and rings, nothing allocated.
static ID_STACK: Region<{ 1 << 14 }> = Region(UnsafeCell::new([0; 1 << 14]));

/// The identity's lock watcher: asleep on `ID_BELL` until the kernel moves it, then rings the loop (`NET_BELL`), which does the wipe on its own thread.
/// A doorbell, never a poll.
extern "C" fn id_watch(_: u64) -> ! {
    loop {
        let e = ID_BELL.load(Ordering::Acquire);
        ferros_sys::wait_world(&ID_BELL, e, 0);
        NET_BELL.fetch_add(1, Ordering::AcqRel);
        ferros_sys::wake(&NET_BELL, 1);
    }
}

/// Bytes of the identity's (its roots, the fleet key, the person key) overwritten in a way the compiler keeps, then a compiler fence so nothing after moves before them: ahau's `roots_thread` wipe.
/// The kernel also cleans the lines to DRAM (keys.rs `wipe_to_dram`); EL0 cannot, so these zeros may sit in a dirty cache line for a while (docs/KEY_REGISTERS.md "The identity's keys in the kernel alone").
pub(crate) fn wipe(b: &mut [u8]) {
    for x in b.iter_mut() {
        unsafe { core::ptr::write_volatile(x, 0) };
    }
    core::sync::atomic::compiler_fence(Ordering::SeqCst);
}

/// A key held as an option: its bytes overwritten where they lie, then None.
pub(crate) fn wipe_key(k: &mut Option<[u8; 32]>) {
    if let Some(b) = k.as_mut() {
        wipe(b);
    }
    *k = None;
}

/// A value that holds the roots by value (the add and departure machines keep the identity seed and the handle proof in themselves, fgtw::flow): dropped where it lies, its bytes overwritten, then None.
/// Setting the option to None or replacing it moves or drops the value and leaves its bytes in place.
pub(crate) fn wipe_slot<T>(slot: &mut Option<T>) {
    let p = slot as *mut Option<T>;
    // SAFETY: the value is dropped in place once, its bytes then overwritten and a None written over them without reading or dropping them; nothing reads the slot in between (`&mut` holds it).
    unsafe {
        core::ptr::drop_in_place(p);
        let b = p as *mut u8;
        for k in 0..core::mem::size_of::<Option<T>>() {
            core::ptr::write_volatile(b.add(k), 0);
        }
        core::ptr::write(p, None);
    }
    core::sync::atomic::compiler_fence(Ordering::SeqCst);
}

core::arch::global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 0
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad 0
    .size _entry, . - _entry
"#,
    main = sym fgtw_main
);

#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

const S_UDP: u64 = 1;
const S_STORE: u64 = 2;
const S_IRA: u64 = 3;
const S_SERVE: u64 = 4;
/// Grant slot 5: entropy (the kernel's KOHI stream), for fgtw's fleet-key minting (fgtw::set_entropy).
const S_ENTROPY: u64 = 5;
/// Grant slots 6 and 7: the NFC and Bluetooth drivers (docs/NFC.md, docs/BT.md), the joiner's two proximity carriers (pairing-v2 "Transports"): the tap serves the request's secret S, the beacon advertises its id.
const S_NFC: u64 = 6;
const S_BT: u64 = 7;
/// Grant slot 8: the identity unlock right (AUTH.md): the device is bound to the identity when its fleet binds this device, and released when a departure completes.
const S_UNLOCK: u64 = 8;

struct Log {
    grants: u64,
    buf: Vec<u8>,
}

/// One pass of the loop timed step by step (2026-11-17, the Pro: a kist's first wait for be you, facets op 2, was answered 5 to 6 s after it came, held behind the pass's other work): the counter at each named step's end, said on the console when the whole pass took longer than an eighth of a second, so the step that holds it is named.
struct Lap {
    t0: u64,
    marks: [(&'static [u8], u64); 1 << 5],
    n: usize,
}

impl Lap {
    fn new() -> Self {
        Self { t0: now(), marks: [(b"", 0); 1 << 5], n: 0 }
    }

    /// The step `name` ended now.
    fn mark(&mut self, name: &'static [u8]) {
        self.marks[self.n] = (name, now());
        self.n += 1;
    }

    /// The pass's steps in ms, each with its time, when the pass took longer than 2^-3 s.
    fn said(&self, log: &mut Log) {
        let f = ferros_sys::freq();
        let end = now();
        if end - self.t0 <= f >> 3 {
            return;
        }
        log.put(b"fgtw: a long pass, A#");
        log.dec((end - self.t0) * 1000 / f);
        log.put(b" ms:");
        let mut prev = self.t0;
        for &(name, t) in &self.marks[..self.n] {
            // Only the steps that took a millisecond or more: the rest are noise in the line.
            if (t - prev) * 1000 / f >= 1 {
                log.put(b" ");
                log.put(name);
                log.put(b" A#");
                log.dec((t - prev) * 1000 / f);
            }
            prev = t;
        }
        log.line();
    }
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
        // [2, port, wait 0, deadline 0 (at once), the watcher's id]: the datagrams this process has yet to take, Photon keeping its own copy of each (ip `pins`).
        let mut q = [0u8; 16];
        q[0] = 2;
        q[1..3].copy_from_slice(&port.to_be_bytes());
        q[12..16].copy_from_slice(&WATCH_ID);
        let n = self.call(&q, out);
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
    /// INFO's flags byte (bit 0 an IPv4 route, bit 1 an IPv6 one) and whether the stack answered at all.
    fn flags(&self) -> Option<u8> {
        let mut out = [0u8; 64];
        let n = self.call(&[3], &mut out);
        (n >= 51 && n != ERR).then_some(out[50])
    }
    /// `host`'s address from ip's resolver (op 10 RESOLVE, the device's one cache; the call held until the lookup ends, 16 s at most), or None (nobody present, no route or server, no such name, no answer).
    /// Family 0: the family the route will use, AAAA where IPv6 has a route, then A (owner, 2026-10-29: IPv6 preferred). Until 2026-11-13 this asked for IPv4 alone, and a network that gave no DHCPv4 lease but a working IPv6 left the identity service grey (the 8a: fgtw.org answered httptest over IPv6 while fgtw never saw a network).
    fn resolve(&self, host: &[u8]) -> Option<SeedAddr> {
        let mut q = Vec::with_capacity(3 + host.len());
        q.extend_from_slice(&[10, 0, 1 << 4]);
        q.extend_from_slice(host);
        let mut r = [0u8; 22];
        let n = self.call(&q, &mut r);
        // Status 0, found: the family and the address follow (an empty reply: nobody present).
        if n == ERR || n < 22 || r[0] != 0 || (r[1] != 4 && r[1] != 6) {
            return None;
        }
        let mut ip = [0u8; 16];
        ip.copy_from_slice(&r[2..18]);
        Some(SeedAddr { fam: r[1], ip })
    }

    /// One HTTP POST of `body` to `/` on port 80 of `to` (the `ip` process's client TCP, ops 4-7, either family), as (status, body); None when the connection or the exchange fails, or takes longer than 16 s.
    /// The request goes into the connection's send buffer as soon as it is asked for (ip sends it once the connection opens); each receive is held by ip until data comes or the connection's state changes, a second at most, never a poll (owner, 2026-11-10: "No polling!").
    fn post(&self, to: SeedAddr, host: &[u8], body: &[u8]) -> Option<(u16, Vec<u8>)> {
        let f = ferros_sys::freq();
        let deadline = now() + 16 * f;
        let mut c = [0u8; 20];
        c[0] = 4;
        c[1] = to.fam;
        c[2..18].copy_from_slice(&to.ip);
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
        let mut sent = 0usize;
        let mut got = Vec::new();
        // One receive's most: 4 KiB on the heap, the state byte ahead of it.
        let most: usize = 1 << 12;
        let mut out = alloc::vec![0u8; 1 + most];
        let mut state = 0u8;
        while now() < deadline {
            if sent < req.len() {
                // A call carries 4096 bytes: the op and handle, and 4094 of the request.
                let n = (req.len() - sent).min((1 << 12) - 2);
                let mut s = Vec::with_capacity(2 + n);
                s.extend_from_slice(&[5, h]);
                s.extend_from_slice(&req[sent..sent + n]);
                if self.call(&s, &mut r) == 3 && r[0] == 0 {
                    sent += u16::from_be_bytes([r[1], r[2]]) as usize;
                }
            }
            // Held a second at most once the request is all in ip's hands; at once while it is not.
            let wait = if sent < req.len() { 0 } else { 1 };
            let mut q = [6, h, wait, 0, 0, 0, 0];
            q[3..7].copy_from_slice(&(most as u32).to_be_bytes());
            let n = self.call(&q, &mut out);
            if n == 0 || n == ERR {
                break;
            }
            let n = (n as usize).min(out.len());
            state = out[0];
            got.extend_from_slice(&out[1..n]);
            if state >= 3 && n == 1 {
                break;
            }
            // The send buffer full (a body larger than its 256 KiB): a millisecond before more of it.
            if n == 1 && wait == 0 {
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

/// Where this device stands with the identity whose roots it holds (the `fgtw` endpoint's op 1 state byte; pairing-v2's lamp). 0 no roots, 1 joining (the identity has a fleet, this device is not in it yet: RED, words shown), 2 bound waiting for the fleet key, 3 a member with the key, 4 looking the identity up, 5 fresh (fgtw.org holds no chain and no fleet device is heard: it may be minted), 6 wrong fleet (the seed's chain for this handle has another identity's genesis), 7 unreachable (no peer heard and no seed answer: GREY, never a verdict), 8 leaving (a departure request out), 9 owned elsewhere (fgtw.org says this device is in another identity's fleet: no words), 10 selected, 11 joining but not confirmed (`V_CONFIRM`).
const V_NONE: u8 = 0;
const V_JOIN: u8 = 1;
const V_LOOKING: u8 = 4;
const V_FRESH: u8 = 5;
const V_WRONG: u8 = 6;
const V_UNREACHABLE: u8 = 7;
/// Leaving: a departure request is out (op 5) to the fleet's devices, each of which asks its person, Approve or Deny, until one approves and the chain no longer holds this device.
const V_LEAVING: u8 = 8;
/// Owned elsewhere: fgtw.org refused this device's binding request because the device is in another identity's fleet (`device_owned`). No words; only that fleet can let it go.
const V_OWNED: u8 = 9;
/// Selected: a member picked this joining device (the Fleet page's button or an NFC tap) and said so (`bind_select`, owner 2026-11-10: "if you tap, it shouldn't become part of the fleet, it should just get notified it was selected"). Nothing is bound: the attest screen turns green with the selecting device's name (op 1's words), and the join completes when the new chain folds this device in and a fan-out wrap opens, as before. Op 1 only; never a verdict.
const V_SELECTED: u8 = 10;
/// Joining, not confirmed: the identity has a fleet, and the person on this device has not yet pressed the join's confirmation (op 32 [1], `join_confirmed`; owner, 2026-11-22). Nothing of the join has left the device: the attest screen asks "Yes, I really would like to add this device and this is my identity and not a typo", and only then (state 1) is the request out, the device told to be held to the other one. Op 1 only; the verdict under it is still joining.
const V_CONFIRM: u8 = 11;

/// One transfer of the vault (the kernel's STORE_MAX): larger values go in pieces.
const STORE_PIECE: usize = 4096;

/// The seed (photon docs/peers-are-fgtw.md: peers first, the seed last). Plain HTTP on port 80, which fgtw.org serves (checked 2026-10-28; Claude memories/fgtw_plain_http_for_ferros.md): every call is one POST of a signed VSF document, and every answer is verified and folded here, so the transport carries no trust.
const SEED_HOST: &[u8] = b"fgtw.org";

/// The seed's address as ip's resolver gave it: the family (4 or 6) and the address (IPv4 in the first four bytes).
#[derive(Clone, Copy)]
struct SeedAddr {
    fam: u8,
    ip: [u8; 16],
}

/// fgtw's transport on ferros: one plain-HTTP POST of a request to fgtw.org through the `ip` process (Udp::post), the address resolved once and again after a failure. fgtw (feature `protocol`) builds every request and reads every answer; this only carries bytes.
struct SeedHttp<'a> {
    udp: &'a Udp,
    ip: core::cell::Cell<Option<SeedAddr>>,
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

// ferros_sys keeps Eagle time's rate without vsf (`eagle_now`, the documents' stamp): the two must be the one definition.
const _: () = assert!(ferros_sys::EAGLE_OSCILLATIONS_PER_SECOND == vsf::OSCILLATIONS_PER_SECOND);

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
    /// The kernel's lock epoch (`ID_BELL`) when the roots in hand were taken (op 4, op 3, the vault at start): once the bell moves past it the identity was locked, and what is held of it goes (`locked_out`).
    lock_epoch: u64,
    /// The fleet's members as the last chain this client adopted folds them (the Fleet page's list, op 6): device keys, this one among them.
    members: Vec<[u8; 32]>,
    /// The chain's ops said on the console so far (`note_members`): its whole history the first time this run adopts one, then each op added after.
    chain_seen: usize,
    pt: Pt,
    sponsors: Vec<Sponsor>,
    /// This device's current binding request and its stamp (re-minted every 210 s while unbound, as the app re-posts).
    request_t: i64,
    request_frame: Vec<u8>,
    /// The person on this device pressed the join's confirmation (op 32 [1]; the attest screen's "Yes, I really would like to add this device and this is my identity and not a typo"; owner, 2026-11-22, after a fleet notification reached his devices before he had confirmed on the new one).
    /// Until it is set nothing of the join leaves this device: no binding request is minted, so none is handed to a fleet device on the network, none is posted to fgtw.org's registry, no tap secret is served on the NFC tag and no beacon id is advertised over BLE; no LAN beacon goes out under the handle; the pairing words are neither said on the console nor in op 1's reply; and a select is not taken. What runs before it is the look-up alone (a signed question to the person's devices, then fgtw.org), which announces nothing: a handle typed alone rings no device. Forgotten with the roots (`forget_identity`), so a handle typed again (the same or another) asks again.
    join_confirmed: bool,
    /// Whether the tap secret and the beacon are out (`radios`).
    radios_on: bool,
    /// The secret and beacon id last handed out, a handover that missed a driver, and when it was tried (`radios`).
    radios_last: Option<([u8; 32], [u8; 16])>,
    radios_missed: bool,
    radios_at: u64,
    /// A device leaving its fleet, both halves, on fgtw's machine (depart.rs): this device's own request and a fleet device's request waiting for this device's person.
    dep: depart::Departing,
    /// Whether the network has been up, either family (the first address triggers a look-up), and the look-up ticks spent with no network while looking.
    net_up: bool,
    offline_looks: u8,
    last_beacon_us: u64,
    last_ping_us: u64,
    last_words_us: u64,
    /// Unlock at boot (device.unlock_at_boot, ferros_sys::PREF_UNLOCK_AT_BOOT): when on, the seed, proof, chain and fleet key are kept in the vault so the device unlocks without the handle being typed; when off (the default), nothing of the identity touches the disk and it is all derived again from the handle and the ira (owner, 2026-10-28: "sand on the beach with no keys anywhere"). Read at start and whenever roots are set, and followed each pass since 2026-11-18 (`follow_persist`): turned on mid-session (the Security page), the roots in hand are kept at once; until then they were kept only if it was on when they were handed in, while the kernel keeps its own seed at the next unlock that has it on, so the two keeps could part (the Pro, b970: the kernel unlocked at boot and this service had nothing).
    persist: bool,
    /// The last chain adopted, as its bytes (`keep_chain`): kept here as well as on disk, so turning unlock at boot on mid-session keeps it at once with the roots (`follow_persist`). The chain is public; it goes with the identity (`forget_identity`).
    chain_held: Vec<u8>,
    /// Where this device stands with the identity (V_*), and when the seed was last asked.
    verdict: u8,
    last_lookup_us: u64,
    /// When fgtw.org last answered a fetch (`now_us`), 0 never: the Fleet page says how long ago (2026-11-08: offline it said nothing).
    seed_heard_us: u64,
    /// When the seed's chain was last read for this device's Add while joining (`join_check`).
    join_checked_us: u64,
    /// The seed's address, once resolved.
    seed_ip: Option<SeedAddr>,
    /// A sponsor named at spawn (its address; the key is learnt from its frames), handed the request directly.
    given: Option<Addr>,
    given_asked_t: i64,
    /// The chains and fan-outs this device holds (fgtw::held, the rule Photon's held set follows): the seed's copy when it answers, this one when it does not.
    held: core::cell::RefCell<fgtw::held::HeldChains>,
    held_fan: core::cell::RefCell<fgtw::held::HeldFanouts>,
    /// Adding a device, the sponsor's side, on fgtw's machine (add.rs).
    add: add::Sponsoring,
    /// Whether the radios are listening (`sponsor_radios`): the NFC reader while the add machine wants it or a fleet device's departure waits here (a tap of the leaving device approves it), the BLE scan while the add machine wants it.
    listening: (bool, bool),
    /// Loop passes since the radios were last told to listen: told again every 64 while either listens, so a driver restarted meanwhile listens again (both answer a repeat with nothing changed). And what they last answered, so only a change is said.
    listen_pass: u32,
    listen_said: (u8, u8),
    /// The members of the latest chain for our handle handed to this device before it was bound (a sponsor pushes it ahead of its `bind_select`): what a select's sender is checked against.
    offered: Vec<[u8; 32]>,
    /// A `bind_select` for this device that came before any chain its sender is in (the frames crossed): its sender, checked again when a chain arrives.
    select_waiting: Option<[u8; 32]>,
    /// The member that selected this device, once its select checked out: the green screen until the join completes.
    selected_by: Option<[u8; 32]>,
    /// Words wanted from the person (op 15, the Fleet page): 0 none, 1 a newcomer's pairing words (the last resort when a tap and BLE fail; a departure takes no words since 2026-11-10), 2 a person's handle and 3 the name given them (the People page), 4 a WiFi network's passphrase (the WiFi page's picker, `entry_net` its name), 5 the person's own handle again (the Security page's attest at boot). The keyboard's host (the attest screen, ahau) asks for it (op 16), sends what is typed (ops 11, 14), and closes when the words match or the person cancels (op 15 [0]).
    entry: u8,
    /// With `entry` 4 (the WiFi page's picker, payloads/map, 2026-11-18): the network whose passphrase is wanted, its SSID's octets. The keyboard's host types the passphrase and hands it to the WiFi driver itself (pcie1 JOIN): the passphrase never passes thru here, nor thru the map.
    entry_net: Vec<u8>,
    /// The person's identity space (space.rs, docs/IDENTITY-SPACE.md): their kists, answers and deck, kept here and synced with their fleet.
    space: space::SpaceSync,
    /// The doorbell (op 21; owner, 2026-11-10: "No polling! ... It should be push or doorbell"): callers waiting for a change (their reply tokens), the change count they are answered with, and what they watch (`watch_state`) when it was last counted.
    watchers: Vec<[u8; 32]>,
    watch_gen: u32,
    watch_seen: Vec<u8>,
    /// When this lookup asked FGTW (the person's devices on this network and the fleet peers known) before fgtw.org (`ask_fgtw`); 0 not yet. The owner, 2026-11-10: "are we consulting the FGTW first or fgtw.org?"
    fgtw_asked_us: u64,
    /// The facets served to kists and the person's people (facets.rs).
    facets: facets::Facets,
}

impl Client {
    fn store_get(&self, key: &[u8], buf: &mut [u8]) -> Option<usize> {
        ferros_sys::store_get(cap(self.log.grants, S_STORE), key, buf)
    }
    /// Every write whole, whatever its size: one transfer takes at most 4096 bytes, and a fleet chain past that was refused and never kept (found 2026-11-07).
    fn store_set(&self, key: &[u8], value: &[u8]) -> bool {
        self.store_set_whole(key, value)
    }
    /// Write a value of any size: in one transfer when it fits, else in pieces the kernel assembles and commits with the last one.
    fn store_set_whole(&self, key: &[u8], value: &[u8]) -> bool {
        let c = cap(self.log.grants, S_STORE);
        if value.len() <= STORE_PIECE {
            return ferros_sys::store_set(c, key, value);
        }
        let mut ok = true;
        for (k, piece) in value.chunks(STORE_PIECE).enumerate() {
            let at = k * STORE_PIECE;
            ok &= ferros_sys::store_set_at(c, key, at, piece, at + piece.len() < value.len());
        }
        ok
    }
    /// Read a value of any size, piece by piece; None when it is absent.
    fn store_get_whole(&self, key: &[u8]) -> Option<Vec<u8>> {
        let c = cap(self.log.grants, S_STORE);
        let mut piece = alloc::vec![0u8; STORE_PIECE];
        let total = ferros_sys::store_get_at(c, key, 0, &mut piece)?;
        let mut v = Vec::with_capacity(total);
        let mut at = 0;
        while at < total {
            // WHY the min: the last piece is shorter than a transfer, and only what the value holds is read into the copy.
            let n = (total - at).min(STORE_PIECE);
            if at > 0 {
                ferros_sys::store_get_at(c, key, at, &mut piece[..n])?;
            }
            v.extend_from_slice(&piece[..n]);
            at += n;
        }
        Some(v)
    }

    /// Read the unlock-at-boot preference (this process's own grants page).
    fn read_persist(&mut self) {
        self.persist = ferros_sys::pref(ferros_sys::PREF_UNLOCK_AT_BOOT) == 1;
    }

    /// Store a value only when unlock-at-boot is on; otherwise make sure it is not on disk.
    fn persist_set(&self, key: &[u8], value: &[u8]) -> bool {
        if self.persist {
            self.store_set(key, value)
        } else {
            let _ = ferros_sys::store_delete(cap(self.log.grants, S_STORE), key);
            true
        }
    }

    /// A 32-byte root kept as its document (ferros_kept::secret: the identity seed `hI`, the handle proof `hP`, the fleet key `kc`), only when unlock-at-boot is on (`persist_set`); the document's bytes wiped once handed to the vault.
    fn persist_secret(&self, kind: ferros_kept::secret::Kind, key: &[u8], value: &[u8; 32]) -> bool {
        let Some(mut doc) = ferros_kept::secret::build(kind, value, ferros_sys::eagle_now()) else { return false };
        let ok = self.persist_set(key, &doc);
        wipe(&mut doc);
        ok
    }

    /// A 32-byte root from its document, None when absent, refused or not that document; the read's buffer wiped after.
    fn get_secret(&self, kind: ferros_kept::secret::Kind, key: &[u8]) -> Option<[u8; 32]> {
        let mut b = [0u8; 1 << 8];
        let n = self.store_get(key, &mut b);
        // WHY: the vault says the value's whole length and copies at most the buffer; a longer one is not a root's document, and reading past the copy would be the buffer's end.
        let v = n.filter(|&n| n <= b.len()).and_then(|n| ferros_kept::secret::parse(kind, &b[..n]));
        wipe(&mut b);
        v
    }

    /// A chain adopted: held here (`chain_held`) and kept on disk when unlock at boot is on.
    fn keep_chain(&mut self, chain: &[u8]) {
        self.chain_held.clear();
        self.chain_held.extend_from_slice(chain);
        self.persist_set(b"chain", chain);
    }

    /// Unlock at boot as the kernel has it now, followed (`ferros_kept::secret::follow`): turned on (the Security page), the roots, the fleet key and the chain in hand are kept now, each as its document, so the next boot finds them; turned off, everything of the identity goes from the disk at once (off is always safe; AUTH.md).
    /// Nothing when it has not changed.
    /// The kernel keeps its own seed at the retype gate's match (SYS_UNLOCK op 12, `retype_keep`) and at a typed attest with the setting on (a tag unlock never keeps it); that unlock hands the roots here (op 4), which keeps them as well.
    fn follow_persist(&mut self) {
        use ferros_kept::secret::{Follow, Kind};
        let on = ferros_sys::pref(ferros_sys::PREF_UNLOCK_AT_BOOT) == 1;
        match ferros_kept::secret::follow(on, self.persist) {
            Follow::Unchanged => {}
            Follow::Forget => {
                self.persist = false;
                self.forget_disk();
                self.log.put(b"fgtw: unlock at boot off: nothing of the identity on disk");
                self.log.line();
            }
            Follow::Keep => {
                self.persist = true;
                if self.hp.is_none() {
                    self.log.put(b"fgtw: unlock at boot on: no roots in hand yet; the next handed in are kept");
                    self.log.line();
                    return;
                }
                // Each from where it is held, by reference: no copy of a root on this stack.
                let mut kept = true;
                if let Some(s) = self.identity_seed.as_ref() {
                    kept &= self.persist_secret(Kind::IdentitySeed, b"identity_seed", s);
                }
                if let Some(p) = self.hp.as_ref() {
                    kept &= self.persist_secret(Kind::HandleProof, b"handle_proof", p);
                }
                if let Some(k) = self.fleet_key.as_ref() {
                    kept &= self.persist_secret(Kind::FleetKey, b"fleet_key", k);
                }
                if !self.chain_held.is_empty() {
                    kept &= self.store_set(b"chain", &self.chain_held);
                }
                self.log.put(if kept { b"fgtw: unlock at boot on: the roots, fleet key and chain in hand kept for the next boot" } else { b"fgtw: unlock at boot on, but the vault did not take all of the roots (a boot that unlocks shows the attest screen for the handle)" });
                self.log.line();
            }
        }
    }

    /// Everything of the identity gone from the vault (unlock-at-boot off, or a wipe): the seed, proof, chain and fleet key. The handle itself was never on disk.
    fn forget_disk(&self) {
        for key in [&b"identity_seed"[..], b"handle_proof", b"chain", b"fleet_key"] {
            let _ = ferros_sys::store_delete(cap(self.log.grants, S_STORE), key);
        }
    }

    fn load(&mut self) {
        self.load_saying(true);
    }

    /// `load`, saying where the client stands only when `say` (the two-second look for roots is quiet until they appear).
    fn load_saying(&mut self, say: bool) {
        use ferros_kept::secret::Kind;
        if self.given.is_none() {
            let mut g = [0u8; 1 << 8];
            // WHY the length check: the vault says the whole length and copies at most the buffer; a longer value is not a sponsor's document.
            if let Some((fam, ip, port)) = self.store_get(b"sponsor", &mut g).filter(|&n| n <= g.len()).and_then(|n| ferros_kept::sponsor::parse(&g[..n])) {
                self.given = Some(Addr { fam, ip, port });
            }
        }
        if let Some(mut b) = self.get_secret(Kind::IdentitySeed, b"identity_seed") {
            self.identity_seed = Some(b);
            wipe(&mut b);
        }
        if let Some(mut b) = self.get_secret(Kind::HandleProof, b"handle_proof") {
            self.hp = Some(b);
            wipe(&mut b);
        }
        if let Some(mut b) = self.get_secret(Kind::FleetKey, b"fleet_key") {
            self.fleet_key = Some(b);
            wipe(&mut b);
        }
        // The lock epoch they belong to (kept on disk only with unlock at boot, read while this identity is unlocked).
        self.lock_epoch = ID_BELL.load(Ordering::Acquire);
        // The chain we hold, if bound before: folded again against our roots, so a bad write never binds us.
        if let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) {
            // Read whole, piece by piece, exactly its own size (one transfer holds at most 4096 bytes, and a fleet chain grows past that; a 128 KiB buffer asked of a 160 KiB heap that has been running a while need not find one free run).
            if let Some(chain) = self.store_get_whole(b"chain") {
                if !chain.is_empty() {
                    if let Ok((blob, bound)) = join::adopt_chain(&hp, &seed, &self.me, &chain) {
                        self.bound = bound;
                        self.note_members(&blob);
                        self.chain_held = chain.clone();
                        self.log.put(b"fgtw: chain held, ");
                        self.log.dec(blob.ops.len() as u64);
                        self.log.put(if bound { b" op(s), this device is a member" } else { b" op(s), this device is not in it" });
                        self.log.line();
                    }
                }
            }
        }
        // The departure machine (depart.rs), made now the roots are in hand: a departure under way when the device last stopped is resumed, and finished at once if the chain held here (the machine's first fold) already lacks this device.
        self.depart_start();
        // The add machine (add.rs), made the same way: an add kept from before a restart is resumed once a fold is told.
        self.add_start();
        if !say && self.hp.is_none() {
            return;
        }
        self.log.put(b"fgtw: device key ");
        self.log.fp(&self.me);
        self.log.put(match (self.hp.is_some(), self.bound, self.fleet_key.is_some()) {
            (false, _, _) => b" - no identity in hand: waiting for the attest screen to hand the roots over",
            (true, false, _) => b" - paired, waiting to be bound by a sponsor on this network",
            (true, true, false) => b" - bound, waiting for the fleet key",
            (true, true, true) => b" - a fleet member with the fleet key",
        });
        self.log.line();
    }

    fn show_words(&mut self) {
        let (Some(seed), false) = (self.identity_seed, self.bound) else { return };
        // Leaving (a departure resumed before the chain is adopted again), or a join its person has not confirmed: no pairing words.
        if self.depart_asking() || !self.join_confirmed {
            return;
        }
        let words = join::pair_words(&self.me, &seed);
        self.log.put(b"fgtw: pairing words - ");
        self.log.put(words.as_bytes());
        self.log.line();
    }

    fn send_all(&mut self, out: Vec<(Addr, Vec<u8>)>) {
        for (to, d) in out {
            self.udp.send(&to, join::PORT, &d);
        }
    }

    /// A frame to every fleet device heard in the last two minutes (as the pings go), and to `extra` (a newcomer's address) once.
    fn push_to_fleet(&mut self, frame: Vec<u8>, extra: Option<Addr>) {
        let t = now_us();
        let mut to: Vec<Addr> = self.sponsors.iter().filter(|s| t - s.heard_us < 120_000_000).map(|s| s.addr).collect();
        if let Some(a) = extra {
            if !to.iter().any(|x| x.fam == a.fam && x.ip == a.ip && x.port == a.port) {
                to.push(a);
            }
        }
        for a in to {
            let out = self.pt.send(a, frame.clone(), now_us(), eagle());
            self.send_all(out);
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
                self.log.put(b"fgtw: handing our binding request to the sponsor named at spawn, ");
                self.log.addr(&to);
                self.log.line();
                let f = self.request_frame.clone();
                let out = self.pt.send(to, f, now_us(), eagle());
                self.send_all(out);
            }
        }
    }

    /// The lookup's question to FGTW: a signed `fleet_req` for this handle proof to this network (the beacon groups and the subnet broadcast, where beacons go) and to each fleet peer known by address. Answered by a member with its beacon, or its chain for a device it knows (`on_fleet_req`).
    fn ask_fgtw(&mut self, hp: &[u8; 32]) {
        // Signed with the Ed25519 key alone, as the app's fleet_req is (Photon `build_fleet_req_vsf`, `envelope_signer`): one datagram on a broadcast, where every scheme's signature (about 10 KB) would not fit.
        let frame = match fgtw::protocol::fleet_req(hp, self.keys.keypair(), eagle()) {
            Ok(f) => f,
            Err(e) => {
                self.log.put(b"fgtw: lookup: the question to FGTW did not build: ");
                self.log.put(e.as_bytes());
                self.log.line();
                return;
            }
        };
        self.log.put(b"fgtw: lookup: asking FGTW first (this network");
        let mut v4 = [0u8; 16];
        v4[..4].copy_from_slice(&join::BEACON_V4);
        self.udp.send(&Addr { fam: 4, ip: v4, port: join::BEACON_PORT }, join::BEACON_PORT, &frame);
        self.udp.send(&Addr { fam: 6, ip: join::BEACON_V6, port: join::BEACON_PORT }, join::BEACON_PORT, &frame);
        if let Some(b) = self.udp.subnet_broadcast() {
            let mut ip = [0u8; 16];
            ip[..4].copy_from_slice(&b);
            self.udp.send(&Addr { fam: 4, ip, port: join::BEACON_PORT }, join::BEACON_PORT, &frame);
        }
        let peers: Vec<Addr> = self.sponsors.iter().map(|s| s.addr).collect();
        for a in &peers {
            self.udp.send(&Addr { port: join::BEACON_PORT, ..*a }, join::BEACON_PORT, &frame);
        }
        self.log.put(b" and ");
        self.log.dec(peers.len() as u64);
        self.log.put(b" fleet peer(s) known)");
        self.log.line();
    }

    /// A device asking FGTW who holds a handle proof (`fleet_req` on 4384). Ours, and this device a member: a device the fleet holds gets the chain (`fleet_resp`), any other our beacon, unicast (a few bytes, what listening to beacons would tell it, so no amplification); a device asking for another handle, or this one not a member, gets nothing.
    fn on_fleet_req(&mut self, from: Addr, hp: [u8; 32], asker: [u8; 32]) {
        if Some(hp) != self.hp || asker == self.me || !self.bound {
            return;
        }
        let chain = self.held.borrow().get(&hp).and_then(|b| b.to_vsf_bytes().ok());
        if let (true, Some(chain)) = (self.members.contains(&asker), chain) {
            if let Ok(frame) = fgtw::protocol::fleet_resp(&hp, &chain, &self.keys, eagle()) {
                let out = self.pt.send(Addr { port: join::PORT, ..from }, frame, now_us(), eagle());
                self.send_all(out);
                self.log.put(b"fgtw: FGTW: our chain to ");
                self.log.fp(&asker);
                self.log.line();
            }
            return;
        }
        let frame = join::beacon(hp, join::PORT, self.me, eagle());
        self.udp.send(&Addr { port: join::BEACON_PORT, ..from }, join::BEACON_PORT, &frame);
        self.log.put(b"fgtw: FGTW: our beacon to ");
        self.log.fp(&asker);
        self.log.put(b", which asked who holds this identity");
        self.log.line();
    }

    /// A beacon on 4384: a device of our fleet on this network is a sponsor - remembered, and handed our binding request while we are unbound.
    fn on_beacon(&mut self, from: Addr, bytes: &[u8]) {
        let Some((hp, port, Some(key))) = join::parse_beacon(bytes) else {
            // Not a beacon: a device's question to FGTW, perhaps.
            if let Ok((hp, asker)) = fgtw::protocol::parse_fleet_req(bytes) {
                self.on_fleet_req(from, hp, asker);
            }
            return;
        };
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
                self.log.put(b"fgtw: fleet device ");
                self.log.fp(&key);
                self.log.put(b" beaconing at ");
                self.log.addr(&addr);
                self.log.line();
                self.sponsors.push(Sponsor { key, addr, asked_t: 0, heard_us: t, online: false });
                // Not bound: that device is asked for the chain at once, unicast (a member is answered with it, `on_fleet_req` on its side), so a member of the fleet binds from its own devices.
                // The Pro, b913 (2026-11-17): the lookup's broadcast went before the MacBook beaconed and fgtw.org did not answer, so the verdict went GREY; the MacBook's beacon 57 s later made it "joining", the lookup is not run again while joining, and a member of the fleet sat minting a binding request with no person key, so Photon got no alias.
                if !self.bound {
                    if let Some(hp) = self.hp {
                        if let Ok(frame) = fgtw::protocol::fleet_req(&hp, self.keys.keypair(), eagle()) {
                            self.udp.send(&Addr { port: join::BEACON_PORT, ..from }, join::BEACON_PORT, &frame);
                            self.log.put(b"fgtw: lookup: asking ");
                            self.log.fp(&key);
                            self.log.put(b" for our fleet's chain");
                            self.log.line();
                        }
                    }
                }
                // A device beaconing under our handle: the identity has a fleet, whatever the seed said.
                if matches!(self.verdict, V_LOOKING | V_FRESH | V_UNREACHABLE) {
                    self.verdict = V_JOIN;
                }
                self.sponsors.len() - 1
            }
        };
        // A fleet device heard: handed whatever departure frame or fan-out it has not taken. One not in the fold is a newcomer on this network, nearby for the add machine.
        self.reachable(key);
        if self.bound {
            self.add_lan_heard(key);
        }
        if !self.bound && !self.request_frame.is_empty() && self.sponsors[idx].asked_t != self.request_t {
            self.sponsors[idx].asked_t = self.request_t;
            self.log.put(b"fgtw: handing our binding request to ");
            self.log.fp(&key);
            self.log.line();
            let frame = self.request_frame.clone();
            let out = self.pt.send(addr, frame, t, eagle());
            self.send_all(out);
        }
    }

    /// The fleet's chain names this device: bound, and the kernel binds the device to this identity (one identity per device, AUTH.md: from now on only its handle opens the device, until a departure releases it).
    fn became_bound(&mut self) {
        if !self.bound && !ferros_sys::bind_identity(cap(unsafe { GRANTS }, S_UNLOCK)) {
            self.log.put(b"fgtw: the kernel did not bind the device to this identity");
            self.log.line();
        }
        self.bound = true;
    }

    /// A member with no key recovers it from the published fan-out, into RAM (the default; stored only if unlock-at-boot is on).
    fn recover_key(&mut self) {
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
        if !self.bound || self.fleet_key.is_some() {
            return;
        }
        fgtw::set_clock(eagle());
        let kp = self.keys.keypair();
        let (recovered, ip) = {
            let t2 = self.seed();
            let r = fgtw::client::recover_fleet_key(&t2, &hp, kp, &seed).ok().flatten();
            (r, t2.ip.get())
        };
        self.seed_ip = ip;
        if recovered.is_some() {
            self.seed_heard_us = now_us();
        }
        if let Some(k) = recovered {
            self.fleet_key = Some(k);
            self.persist_secret(ferros_kept::secret::Kind::FleetKey, b"fleet_key", &k);
            self.log.put(b"fgtw: the fleet key recovered from the published fan-out");
            self.log.line();
            self.add_key();
        }
    }

    /// Joining: fgtw.org's chain for this identity, read quietly; once it holds this device (a sponsor's Add, by the words or the tap), the chain is adopted, the device bound, the tap secret and the beacon stopped, and the fleet key recovered from the published fan-out. The LAN hand-over does the same when the sponsor knows this device's address.
    fn join_check(&mut self) {
        let Some(hp) = self.hp else { return };
        fgtw::set_clock(eagle());
        let (fetched, ip) = {
            let s = self.seed();
            let r = fgtw::client::fetch(&s, &hp);
            (r, s.ip.get())
        };
        self.seed_ip = ip;
        if fetched.is_ok() {
            self.seed_heard_us = now_us();
        }
        let Ok(Some(b)) = fetched else { return };
        let Ok(bytes) = b.to_vsf_bytes() else { return };
        if !self.hold_chain(&bytes) {
            return;
        }
        self.log.put(b"fgtw: fgtw.org's chain holds this device: THIS DEVICE IS BOUND (a sponsor's Add)");
        self.log.line();
        self.radios(None);
        self.recover_key();
    }

    /// A chain this device should hold: kept, and the device is a member when the fold names it.
    fn hold_chain(&mut self, chain: &[u8]) -> bool {
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return false };
        match join::adopt_chain(&hp, &seed, &self.me, chain) {
            Ok((blob, true)) => {
                self.note_members(&blob);
                self.keep_chain(chain);
                self.became_bound();
                self.bound
            }
            _ => false,
        }
    }


    /// Keep the members a chain folds to (the Fleet page's list), and tell the add machine its fold.
    fn note_members(&mut self, blob: &fgtw::fleet::MembershipBlob) {
        if let Ok(m) = blob.fold() {
            self.members = m;
            self.say_chain(blob);
            // The held copy too (fgtw::held): what a sponsor's Add extends when the seed does not answer.
            fgtw::held::ChainHold::hold(&self.held, blob);
            self.add_fold(blob);
        }
    }

    /// The fleet's history on the console (owner, 2026-11-09, after a device joined with nobody remembering how: "do we have logs of the event?"): each op of the chain this run has not said yet, what it did to which device, signed by which, and when. The chain itself is the record, signed op by op and held by fgtw.org and every member; this is it read out.
    fn say_chain(&mut self, blob: &fgtw::fleet::MembershipBlob) {
        if blob.ops.len() <= self.chain_seen {
            return;
        }
        let seed = self.identity_seed;
        for (i, op) in blob.ops.iter().enumerate().skip(self.chain_seen) {
            use fgtw::fleet::OpKind as K;
            self.log.put(b"fgtw: fleet op ");
            self.log.dec(i as u64);
            self.log.put(match op.kind {
                K::Genesis => b": founded by ".as_slice(),
                K::Add => b": added ",
                K::Remove => b": removed ",
                K::Checkpoint => b": checkpoint by ",
                K::Declare => b": keys declared by ",
                K::Lock => b": locked out ",
                K::Unlock => b": unlocked ",
                // fgtw 5ce096e (owner, 2026-11-18): a member locked out a device that is not a member, a Photon on the alias alone (a ferros Photon among them), lost, stolen or gone without signing out; it has no unlock, and any device list naming it is refused wherever it is checked (fgtw::alias_devices::check).
                // Nothing else here changes with it: it is never a member, so the fold, the add and the departure (members only) are as they were; the crate refuses the lists.
                K::LockListed => b": locked out for good (outside the fleet, on the alias) ",
            });
            self.say_device(&op.device_pubkey, seed);
            if op.signer_pubkey != op.device_pubkey {
                self.log.put(b", signed by ");
                self.say_device(&op.signer_pubkey, seed);
            }
            self.log.put(b", at Eagle second ");
            self.log.dec((op.eagle_time / vsf::OSCILLATIONS_PER_SECOND as i64) as u64);
            self.log.line();
        }
        self.chain_seen = blob.ops.len();
    }

    /// A device as the Fleet page names it (its two words, when the identity is in hand) and its key's fingerprint.
    fn say_device(&mut self, key: &[u8; 32], seed: Option<[u8; 32]>) {
        if let Some(s) = seed {
            let name = fgtw::pair::device_name_default(key, &s);
            self.log.put(name.as_bytes());
            self.log.put(b" ");
        }
        self.log.put(b"(");
        self.log.fp(key);
        self.log.put(b")");
        if *key == self.me {
            self.log.put(b" this device");
        }
    }

    /// Op 6, the Fleet page's list: `[count][count × (the device key's first 8 bytes, flags: bit 0 this device)]`, then each row's name `[length][UTF-8]`. Bound with no list yet (the chain only in RAM since unlock-at-boot is off, and none adopted this session), the seed's copy is read first.
    fn members_reply(&mut self) -> Vec<u8> {
        // Only with a network: offline the fetch only waited out its timeout, and the Fleet page with it, every time it asked (2026-11-08).
        if self.members.is_empty() && self.bound && self.net_up {
            if let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) {
                fgtw::set_clock(eagle());
                let (fetched, ip) = {
                    let s = self.seed();
                    let r = fgtw::client::fetch(&s, &hp);
                    (r, s.ip.get())
                };
                self.seed_ip = ip;
                if fetched.is_ok() {
                    self.seed_heard_us = now_us();
                }
                if let Ok(Some(b)) = fetched {
                    if let Ok(bytes) = b.to_vsf_bytes() {
                        if let Ok((blob, _)) = join::adopt_chain(&hp, &seed, &self.me, &bytes) {
                            self.note_members(&blob);
                        }
                    }
                }
            }
        }
        // WHY the cap: the reply's count is one byte and the Fleet page lays out at most 16 rows.
        let n = self.members.len().min(16);
        let mut r = Vec::with_capacity(1 + n * (9 + 1 + 36));
        r.push(n as u8);
        for k in &self.members[..n] {
            r.extend_from_slice(&k[..8]);
            // Bit 0 this device; bit 1 heard on this network (a fleet device beaconing, `sponsors`, online).
            let heard = self.sponsors.iter().any(|s| s.online && s.key == *k);
            r.push((*k == self.me) as u8 | (heard as u8) << 1);
        }
        // The names after the rows, one `[length][UTF-8]` per row in the same order: the two-word name every device of this fleet computes for a member (`fgtw::pair::device_name_default`, the one Photon's Fleet page and pairing screen show), length 0 when there is no identity seed to compute it with. A reader that knows only the rows stops before them.
        for k in &self.members[..n] {
            match self.identity_seed {
                Some(seed) => {
                    let name = fgtw::pair::device_name_default(k, &seed);
                    // Two voca words: at most 36 bytes (the longest word is 18), so the length fits its byte.
                    r.push(name.len() as u8);
                    r.extend_from_slice(name.as_bytes());
                }
                None => r.push(0),
            }
        }
        r
    }

    /// The proximity carriers: with a request, its secret served on the NFC tag and its beacon id advertised; without, both stopped. The calls carry CALL_LIVE: a driver that is down, or still bringing its chip up (Bluetooth gives WiFi 16 s at boot), fails the call at once instead of holding this client and, through it, the attest screen. One that missed its handover is tried again every 16 s from the loop (`radios_retry`); the words work meanwhile.
    fn radios(&mut self, on: Option<(&[u8; 32], &[u8; 16])>) {
        let (nfc, bt) = (cap(unsafe { GRANTS }, S_NFC), cap(unsafe { GRANTS }, S_BT));
        // Each driver's reply is [0 done, else refused] (payloads/nfc, payloads/bt): read, not just whether it answered. Until 2026-11-04 any answer logged "ok", so a refused advertise looked like a beacon out.
        let (mut ra, mut rb) = ([0u8; 4], [0u8; 4]);
        let (a, b) = match on {
            Some((s, id)) => {
                let mut m = [0u8; 33];
                m[0] = 1;
                m[1..].copy_from_slice(s);
                let a = ferros_sys::call_live(nfc, &m, &mut ra);
                let mut m = [0u8; 17];
                m[0] = 1;
                m[1..].copy_from_slice(id);
                (a, ferros_sys::call_live(bt, &m, &mut rb))
            }
            None => (ferros_sys::call_live(nfc, &[2], &mut ra), ferros_sys::call_live(bt, &[2], &mut rb)),
        };
        let said = |n: u64, r: &[u8; 4]| -> &'static [u8] { if n == ERR { b"not running" } else if n >= 1 && r[0] != 0 { b"refused" } else { b"ok" } };
        self.radios_on = on.is_some();
        self.radios_missed = on.is_some() && (a == ERR || b == ERR);
        self.radios_at = now();
        if let Some((s, id)) = on {
            self.radios_last = Some((*s, *id));
        }
        self.log.put(if on.is_some() { b"fgtw: tap secret and beacon out: nfc " } else { b"fgtw: tap secret and beacon stopped: nfc " });
        self.log.put(said(a, &ra));
        self.log.put(b", bt ");
        self.log.put(said(b, &rb));
        self.log.line();
    }

    /// Everything held for the identity whose roots were in the vault: its roots, chain standing, key, sponsors and request.
    /// The vault is not touched.
    /// The roots, the fleet key and the person key are overwritten where they lie (`wipe_key`), and so are the add and departure machines, which keep the roots in themselves (`wipe_slot`): until 2026-11-17 each was only set to None, its bytes left in RAM.
    fn forget_identity(&mut self) {
        wipe_slot(&mut self.dep.m);
        self.dep = depart::Departing::new();
        // The bindings made for this identity and any add under way go with it; a be you still held is refused, the waits for be you are kept (facets.rs `facets_forget`).
        self.facets_forget();
        wipe_slot(&mut self.add.m);
        self.add.reset();
        if self.radios_on {
            self.radios(None);
        }
        wipe_key(&mut self.identity_seed);
        wipe_key(&mut self.hp);
        self.bound = false;
        wipe_key(&mut self.fleet_key);
        self.members.clear();
        self.chain_held.clear();
        self.offered.clear();
        self.select_waiting = None;
        self.selected_by = None;
        self.sponsors.clear();
        self.request_t = 0;
        self.request_frame.clear();
        self.join_confirmed = false;
        self.verdict = V_NONE;
        // The person key's bytes overwritten before the space state goes (docs/FACETS.md D0: RAM only, as the roots).
        if let Some((k, _)) = self.space.person.as_mut() {
            wipe(k);
        }
        self.space = space::SpaceSync::new();
    }

    /// The kernel locked the identity (its bell moved past the epoch the roots were taken at): everything held of it wiped from RAM and this service back to no identity in hand, as at start with nothing kept, the vault untouched (it is sealed now anyway).
    /// The next roots come from the attest screen as at any attest (op 4).
    fn locked_out(&mut self, epoch: u64) {
        let held = self.identity_seed.is_some() || self.hp.is_some() || self.fleet_key.is_some() || self.space.person.is_some();
        self.lock_epoch = epoch;
        if !held {
            return;
        }
        self.forget_identity();
        self.log.put(b"fgtw: the kernel locked the identity: its roots, the fleet key and the person key wiped from RAM; no identity in hand");
        self.log.line();
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
        // FGTW first: the person's devices are asked (a signed fleet_req to this network and to the fleet peers known), and one that answers settles it (its beacon makes the verdict join, its chain is adopted, `on_beacon`, `on_frame`). fgtw.org, the bootstrap, only once a quarter second has passed with no answer: a device on the same network answers in milliseconds, and silence is the only "no" a broadcast gets, so this one wait is the deadline that makes the question finite.
        if self.fgtw_asked_us == 0 {
            self.ask_fgtw(&hp);
            self.fgtw_asked_us = now_us();
            return;
        }
        if now_us().wrapping_sub(self.fgtw_asked_us) < 1 << 18 {
            return;
        }
        self.fgtw_asked_us = 0;
        let heard = !self.sponsors.is_empty();
        self.log.put(b"fgtw: lookup: no device of this identity answered on FGTW; asking fgtw.org, the bootstrap");
        self.log.line();
        let t0 = now();
        fgtw::set_clock(eagle());
        let t = self.seed();
        let fetched = fgtw::client::fetch(&t, &hp);
        let ip = t.ip.get();
        self.seed_ip = ip;
        if fetched.is_ok() {
            self.seed_heard_us = now_us();
        }
        self.log.put(b"fgtw: lookup: fgtw.org answered in ");
        self.log.dec(ferros_sys::us_since(t0) / 1000);
        self.log.put(match &fetched {
            Ok(Some(_)) => b" ms: a chain".as_slice(),
            Ok(None) => b" ms: not_found",
            Err(_) => b" ms: an error",
        });
        if let Err(e) = &fetched {
            self.log.put(b" (");
            self.log.put(e.as_bytes());
            self.log.put(b")");
        }
        self.log.line();
        let answer = match &fetched {
            Ok(Some(b)) => fgtw::attest::SeedAnswer::Chain(b),
            Ok(None) => fgtw::attest::SeedAnswer::NotFound,
            Err(_) => fgtw::attest::SeedAnswer::Unreachable,
        };
        use fgtw::attest::AttestVerdict as A;
        let a = fgtw::attest::attest_verdict(&hp, &self.me, None, &[], answer);
        let member = matches!(a, A::Member);
        let v = match a {
            A::Member => {
                if let Ok(Some(b)) = &fetched {
                    if let Ok(bytes) = b.to_vsf_bytes() {
                        self.hold_chain(&bytes);
                    }
                }
                self.recover_key();
                V_JOIN
            }
            A::JoinOurs => V_JOIN,
            A::WrongFleet => V_WRONG,
            A::Fresh => if heard { V_JOIN } else { V_FRESH },
            A::Unreachable | A::Unverifiable(_) => if heard { V_JOIN } else { V_UNREACHABLE },
        };
        // A device that is no member here: is it another identity's? fgtw.org's registry answers (`device_owner_get`, never whose) before the screen offers to mint or shows join words. Unanswered, the verdict stands: the genesis and the binding request meet the same gate at the seed.
        let owned = if !member && matches!(v, V_FRESH | V_JOIN) {
            let t2 = self.seed();
            let r = fgtw::client::device_owned_elsewhere(&t2, &self.me, &hp);
            self.log.put(match &r {
                Ok(true) => b"fgtw: owner: fgtw.org says another identity holds this device".as_slice(),
                Ok(false) => b"fgtw: owner: fgtw.org says no other identity holds this device",
                Err(_) => b"fgtw: owner: no answer: ",
            });
            if let Err(e) = &r {
                self.log.put(e.as_bytes());
            }
            self.log.line();
            r == Ok(true)
        } else {
            false
        };
        let v = if owned { V_OWNED } else { v };
        if v != self.verdict {
            self.log.put(match v {
                V_JOIN if self.bound => b"fgtw: this device is a member of the identity's fleet".as_slice(),
                V_JOIN => b"fgtw: this identity has a fleet: waiting to be added from one of its devices",
                V_FRESH => b"fgtw: fgtw.org holds no chain for this handle and no fleet device is heard: it may be minted",
                V_WRONG => b"fgtw: fgtw.org's chain for this handle claims another slot",
                V_OWNED => b"fgtw: fgtw.org says this device belongs to another identity's fleet: no mint, no words",
                _ => b"fgtw: neither a fleet device nor fgtw.org answers: no verdict (GREY)",
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
                self.log.put(b"fgtw: MINTED: this device is the identity's first member");
                self.log.line();
                match key {
                    Ok((_, k)) => {
                        self.fleet_key = Some(k);
                        self.persist_secret(ferros_kept::secret::Kind::FleetKey, b"fleet_key", &k);
                        self.log.put(b"fgtw: the fleet key is founded and ours");
                    }
                    Err(e) => {
                        self.log.put(b"fgtw: the fleet key was not founded: ");
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
            Err(e) if e.contains("another identity's fleet") => {
                self.log.put(b"fgtw: the genesis was refused: this device belongs to another identity's fleet");
                self.log.line();
                self.verdict = V_OWNED;
                5
            }
            Err(e) => {
                self.log.put(b"fgtw: the genesis did not land: ");
                self.log.put(e.as_bytes());
                self.log.line();
                5
            }
        }
    }

    /// Op 32: the join confirmed by this device's person (`on`), or withdrawn. Nothing of a join leaves the device before the confirmation (owner, 2026-11-22: "Nothing is announced on a typed handle alone"); see `join_confirmed`.
    fn confirm_join(&mut self, on: bool) -> u8 {
        if !on {
            if self.join_confirmed && !self.bound {
                self.join_confirmed = false;
                self.request_frame.clear();
                self.request_t = 0;
                self.select_waiting = None;
                self.selected_by = None;
                self.radios(None);
                self.log.put(b"fgtw: the join withdrawn on this device: the binding request dropped");
                self.log.line();
            }
            return 0;
        }
        if self.hp.is_none() || self.bound || self.depart_asking() || self.verdict != V_JOIN {
            return 1;
        }
        if !self.join_confirmed {
            self.join_confirmed = true;
            // The words said on the console at once, as at a fresh request.
            self.last_words_us = 0;
            self.log.put(b"fgtw: the person confirmed the join on this device: the binding request goes out");
            self.log.line();
        }
        0
    }

    /// Mint (or re-mint) this device's binding request.
    fn mint_request(&mut self) {
        let (Some(seed), Some(hp)) = (self.identity_seed, self.hp) else { return };
        let t = eagle();
        let t0 = now();
        self.log.put(b"fgtw: minting the binding request");
        self.log.line();
        // The tap's secret: 32 fresh bytes per request, committed to in the request (`nh`, fgtw::pair::nfc_secret_hash over this device and stamp) so a sponsor that reads S off the tap knows which candidate it touched. Outside the consent bytes: the signature over the frame covers it.
        // On the X3, as the attest screen makes the handle proof: two SLH-DSA-128s signatures (the request's consent and its envelope, fgtw::protocol) took 15.3 s on the A715 workers (2026-11-04), while the person waited for the words. Pinned for the signing only.
        ferros_sys::pin(ferros_sys::CORES_BIG);
        ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
        let mut s = [0u8; 32];
        kernel_entropy(&mut s);
        let req = join::bind_request(&self.keys, &seed, &hp, t, &s);
        let framed = join::bind_req(&req, &hp, &self.keys, t);
        ferros_sys::pin(0);
        let ms = ferros_sys::us_since(t0) / 1000;
        match framed {
            Ok(f) => {
                self.request_t = t;
                self.request_frame = f;
                self.radios(Some((&s, &fgtw::pair::beacon_id(&hp, &self.me, t))));
                self.log.put(b"fgtw: binding request minted (");
                self.log.dec(self.request_frame.len() as u64);
                self.log.put(b" B) in ");
                self.log.dec(ms);
                self.log.put(b" ms; stack used ");
                self.log.dec(stack_used() as u64 / 1024);
                self.log.put(b" KiB of ");
                self.log.dec(STACK_BYTES as u64 / 1024);
                self.log.line();
                // The same request to fgtw.org's registry, as the app posts it: a sponsor lists its candidates from the registry and from requests handed to it on the LAN, and matches the typed words and the heard BLE beacon only against those. Handed over the LAN alone, a request never reached a sponsor this device could not hear (a WiFi that passes no multicast between its clients), and the sponsor had nothing to match (2026-11-04, owner: "does not register").
                match self.post_request(&req, &hp) {
                    Ok(()) => self.log.put(b"fgtw: binding request posted to fgtw.org"),
                    // fgtw.org's registry has this device's key in another identity's fleet (its device → owner index): a final answer, not a failed post (owner, 2026-11-09, "Ira is bound": a released 8a showed join words for a second handle). The request is dropped, so no words, no LAN copy and no tap; the device stays its owner's until that fleet lets it go (a departure to a new owner releases the claim).
                    Err(e) if e.starts_with("device_owned") => {
                        self.log.put(b"fgtw: binding request refused by fgtw.org: this device belongs to another identity's fleet");
                        self.request_frame.clear();
                        self.request_t = 0;
                        self.radios(None);
                        self.verdict = V_OWNED;
                    }
                    Err(e) => {
                        self.log.put(b"fgtw: binding request not posted to fgtw.org: ");
                        self.log.put(e.as_bytes());
                    }
                }
                self.log.line();
            }
            Err(e) => {
                self.log.put(b"fgtw: binding request failed: ");
                self.log.put(e.as_bytes());
                self.log.line();
            }
        }
    }

    /// Post an already-built binding request to fgtw.org's registry: the `bindreq_put` body field for field, as the app's `bindreq_post` builds it (photon src/network/fgtw/fleet.rs), so the registry copy and the LAN copy are one request with one stamp (the BLE beacon's too). `device_owned` is the registry's one-owner verdict; any other error is the seed not answering, and the LAN copy still goes.
    fn post_request(&self, req: &fgtw::fleet::BindRequest, hp: &[u8; 32]) -> Result<(), alloc::string::String> {
        use alloc::string::ToString;
        let mut section = vsf::VsfSection::new("bindreq_put");
        section.add_field("hp", vsf::VsfType::hP(hp.to_vec()));
        section.add_field("dk", vsf::VsfType::ke(req.device_pubkey.to_vec()));
        section.add_field("t", vsf::VsfType::e(vsf::types::EtType::e6(req.t)));
        section.add_field("ds", vsf::VsfType::ge(fgtw::pq::eggs_to_bytes(&req.device_sig)));
        section.add_field("is", vsf::VsfType::ge(req.identity_sig.clone()));
        if let Some(b) = &req.bundle {
            section.add_field("kb", vsf::VsfType::ge(b.to_bytes()));
        }
        if let Some(p) = &req.plate {
            section.add_field("pl", vsf::VsfType::hb(p.to_vec()));
        }
        if req.plate_kind != fgtw::fleet::plate::UNSTATED {
            section.add_field("pk", vsf::VsfType::u(req.plate_kind as usize, false));
        }
        section.add_field("nh", vsf::VsfType::hb(req.nfc_hash.to_vec()));
        let body = vsf::VsfBuilder::new().creation_time_oscillations(eagle()).add_section_direct(section).build().map_err(|e| alloc::format!("bindreq_put build: {e}"))?;
        let resp = fgtw::client::FgtwTransport::post(&self.seed(), body)?;
        if fgtw::client::is_error(&resp.body, "device_owned") {
            return Err("device_owned: this device is still in another identity's fleet".to_string());
        }
        if let Some((reason, detail)) = fgtw::client::error_frame(&resp.body) {
            return Err(alloc::format!("{reason}: {detail}"));
        }
        if !(200..300).contains(&resp.status) {
            return Err(alloc::format!("HTTP {}", resp.status));
        }
        Ok(())
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
                    // A fleet device pinging us is reachable: handed whatever departure frame or fan-out it has not taken.
                    self.reachable(key);
                }
                return;
            }
            Some(Frame::Pong { key, .. }) => {
                if let Some(i) = self.sponsors.iter().position(|s| s.key == key) {
                    let s = &mut self.sponsors[i];
                    s.heard_us = t;
                    if !s.online {
                        s.online = true;
                        self.log.put(b"fgtw: fleet device ");
                        self.log.fp(&key);
                        self.log.put(b" answers our pings");
                        self.log.line();
                    }
                    // A departure frame or fan-out it has not taken yet (its hand-off given up on, or never made): handed now that it answers.
                    self.reachable(key);
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
                self.log.put(b"fgtw: a photon arrived - not read here (messages are the Photon app's)");
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
        // A frame signed by a member of our fleet: its address learnt, and it is reachable (handed whatever departure frame or fan-out it has not taken).
        if let Ok((_, signer)) = &parsed {
            if self.bound && *signer != self.me && self.members.contains(signer) {
                self.learn(*signer, from);
                self.reachable(*signer);
            }
        }
        match parsed {
            Ok((Inbound::FleetResp { hp, chain }, signer)) => {
                let (Some(our_hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
                if hp != our_hp {
                    return;
                }
                match join::adopt_chain(&hp, &seed, &self.me, &chain) {
                    Ok((blob, bound)) => {
                        self.log.put(b"fgtw: our fleet's chain from ");
                        self.log.fp(&signer);
                        self.log.put(b" - ");
                        self.log.dec(blob.ops.len() as u64);
                        self.log.put(if bound { b" op(s), THIS DEVICE IS BOUND" } else { b" op(s), not bound yet" });
                        self.log.line();
                        self.keep_chain(&chain);
                        if bound && !self.bound {
                            self.became_bound();
                        }
                        // Not bound yet: the members a sponsor's select is checked against, and a select that came first checked now.
                        if !bound {
                            if let Ok(m) = blob.fold() {
                                self.offered = m;
                            }
                            if let Some(s) = self.select_waiting.take() {
                                self.select_from(s);
                            }
                        }
                        // A chain for our handle from a device of FGTW: the identity has a fleet, whatever fgtw.org would say.
                        if !bound && matches!(self.verdict, V_LOOKING | V_FRESH | V_UNREACHABLE) {
                            self.hold_chain(&chain);
                            self.verdict = V_JOIN;
                        }
                        // The departure machine's fold: a member's request settled, or ours complete once this device is not in it. The add machine's too.
                        self.depart_fold(&blob);
                        self.add_fold(&blob);
                    }
                    Err(e) => {
                        self.log.put(b"fgtw: a chain from ");
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
                        self.log.put(b"fgtw: fan-out revision ");
                        self.log.dec(revision);
                        self.log.put(b" from ");
                        self.log.fp(&signer);
                        self.log.put(if self.fleet_key == Some(key) { b" - the key we hold" } else { b" - THE FLEET KEY IS OURS" });
                        self.log.line();
                        if self.fleet_key != Some(key) {
                            self.fleet_key = Some(key);
                            self.persist_secret(ferros_kept::secret::Kind::FleetKey, b"fleet_key", &key);
                            // A key that arrives while the Fleet page shows opens the add flow there.
                            self.add_key();
                        }
                    }
                    Ok(None) => {
                        self.log.put(b"fgtw: a fan-out from ");
                        self.log.fp(&signer);
                        self.log.put(b" holds no wrap for us yet");
                        self.log.line();
                    }
                    Err(e) => {
                        self.log.put(b"fgtw: a fan-out was refused: ");
                        self.log.put(e.as_bytes());
                        self.log.line();
                    }
                }
            }
            // A member picked this device (its Fleet page's button or an NFC tap): green, nothing bound. Only for the handle this device is joining and for its own key; anything else is dropped without a word.
            Ok((Inbound::BindSelect { hp, joiner }, signer)) => {
                if self.bound || self.hp != Some(hp) || joiner != self.me {
                    return;
                }
                self.select_from(signer);
            }
            // A newcomer asking this device to add it, and a sibling asking to leave: the sponsor's and the approver's frames (fgtw::sponsor).
            // A newcomer's request reaches the add machine only on a bound device (add.rs): one not in a fleet has nobody to add it to.
            Ok((Inbound::Other(name), _)) if name == "bind_req" => {
                if self.bound {
                    self.add_frame(from, bytes);
                }
            }
            Ok((Inbound::Other(name), _)) if name == "depart_req" || name == "depart_deny" => self.depart_frame(Some(from), bytes),
            // The person's identity space from a sibling (space.rs).
            Ok((Inbound::Other(name), _)) if name == "space_push" => self.on_space_push(from, bytes),
            Ok((Inbound::Other(name), _)) if name == "space_want" => self.on_space_want(from, bytes),
            Ok((Inbound::Other(name), signer)) => {
                self.log.put(b"fgtw: frame ");
                self.log.put(name.as_bytes());
                self.log.put(b" from ");
                self.log.fp(&signer);
                self.log.put(b" (not handled)");
                self.log.line();
            }
            Err(_) => {}
        }
    }

    /// A select's sender checked: a current member of the chain offered for our handle (its genesis this identity's, folded, so a device locked out or removed is not in it), or of the chain this device holds. Until a chain naming it arrives the select waits (the sponsor pushes the chain first, but the frames may cross).
    fn select_from(&mut self, sender: [u8; 32]) {
        // A select before this device's person confirmed the join is not taken: nothing of this join was sent, so it answers a request from before (one still in fgtw.org's registry from an earlier boot), and the screen must not turn green on it.
        if self.bound || !self.join_confirmed {
            return;
        }
        if !self.offered.contains(&sender) && !self.members.contains(&sender) {
            self.select_waiting = Some(sender);
            return;
        }
        if self.selected_by != Some(sender) {
            self.selected_by = Some(sender);
            self.log.put(b"fgtw: selected by ");
            self.log.fp(&sender);
            self.log.put(b": green, waiting for the new chain and the fleet key");
            self.log.line();
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
        // The identity's lock bell before anything of the identity is read (`load` takes the epoch it belongs to): the kernel writes the epoch now and at every lock.
        if ferros_sys::bell(ferros_sys::BELL_IDENTITY, &ID_BELL) {
            // SAFETY: the watcher's stack, given to it alone, once.
            ferros_sys::thread_spawn(id_watch, unsafe { &mut *ID_STACK.0.get() }, 0);
        } else {
            self.log.put(b"fgtw: no identity lock bell (a kernel from before 2026-11-17): a lock leaves the roots, the fleet key and the person key here until this service ends");
            self.log.line();
        }
        self.read_persist();
        if !self.persist {
            self.forget_disk();
        }
        self.load();
        if self.hp.is_some() {
            self.verdict = V_LOOKING;
        }
        self.show_words();
        // The network watcher (`net_watch`): the ports and the flags rung on `NET_BELL`, not asked every pass.
        // SAFETY: the watcher's stack, given to it alone, once.
        ferros_sys::thread_spawn(net_watch, unsafe { &mut *WATCH_STACK.0.get() }, self.log.grants);
        loop {
            let t = now_us();
            let mut lap = Lap::new();
            let mut busy = false;
            // The bell's count before this pass looks: a ring while it runs ends the idle wait at once.
            let bell_seen = NET_BELL.load(Ordering::Acquire);
            // The identity locked since the roots in hand were taken: nothing of it is held past this point (`locked_out`).
            // First in the pass, so no step below uses what the lock took away.
            let epoch = ID_BELL.load(Ordering::Acquire);
            if epoch != self.lock_epoch {
                self.locked_out(epoch);
            }
            // Unlock at boot switched since the last pass (the retype gate turns it on in the kernel; the Security page turns it off): the disk follows at once.
            // A changed preference wakes every word wait in the system (kernel `write_pref_all`), the idle wait at the end of this loop among them, so the pass comes with the change.
            self.follow_persist();
            let watch = NET_FLAGS.load(Ordering::Acquire);
            // The ports to read: those the watch named since the last pass, or both every pass without a watch.
            let news = if watch == NO_WATCH { 3 } else { NET_PORTS.swap(0, Ordering::AcqRel) };
            while let Some((from, n)) = if news & 1 != 0 { self.udp.recv(join::PORT, &mut buf) } else { None } {
                busy = true;
                let datagram = buf[..n].to_vec();
                let out = self.pt.receive(from, &datagram, t, eagle());
                self.send_all(out);
                for e in self.pt.take_events() {
                    match e {
                        Event::Frame { from, bytes, signer } => self.on_frame(from, &bytes, signer.is_some()),
                        Event::Sent { to, hash, ok } => {
                            if !ok {
                                self.log.put(b"fgtw: a frame to ");
                                self.log.addr(&to);
                                self.log.put(b" was given up on");
                                self.log.line();
                            }
                            // A departure frame given up on is handed again when its device is next heard (the desktop restarted 14 s before one arrived, and with no retry it never saw it, 2026-11-10); one that arrived is not.
                            self.depart_sent(&to, &hash, ok);
                            // The add machine's sends the same way (a select, the grown fan-out).
                            self.add_sent(&to, &hash, ok);
                        }
                    }
                }
            }
            while let Some((from, n)) = if news & 2 != 0 { self.udp.recv(join::BEACON_PORT, &mut buf) } else { None } {
                busy = true;
                let datagram = buf[..n].to_vec();
                self.on_beacon(from, &datagram);
            }
            let out = self.pt.tick(t, eagle());
            self.send_all(out);
            lap.mark(b"the network");
            // The facets first (a kist's wait for be you, its be you, its people): answered before anything slow in this pass, and again between the slow steps below (`facets_now`), never held behind them.
            busy |= self.facets_now(&mut lap);
            // The attest screen: the state, or its roots (op 4, 65 bytes).
            // On the stack: a syscall's buffers must be in the arc. 512 bytes hold the longest words entry (23 voca words, at most 18 letters each, with spaces).
            let mut req = [0u8; 512];
            let mut tok = [0u8; 32];
            let n = ferros_sys::recv(cap(self.log.grants, S_SERVE), &mut req, &mut tok, true);
            if n != ERR && n != ferros_sys::NOTHING {
                busy = true;
                // WHY the min: `n` is what the caller sent, which may be more than the buffer took.
                let got = (n as usize).min(req.len());
                // The sponsor's and the approver's ops (the Fleet page's add flow and approvals, on fgtw's machines, add.rs and depart.rs): each answers alone, nothing of the state after it. 8 [on] the Fleet page shown or gone, and [8][1] on a selected device is No → [0]; 9 → the add flow (`add_reply`, addview.rs); 10 [key 8] a row touched → [result]; 11 [words] the words typed so far → the match (`add_words`); 12 Yes on a selected device → [result]; 13 → a departure waiting (`depart_ask_reply`); 14 [1 Approve, 0 Deny] → [result] (depart.rs); 15 [kind] words wanted from the person (0 none, 1 a newcomer's, which also reads fgtw.org's registry once) → [0]; 16 → the words wanted now [kind] (the keyboard's host asks).
                // The identity space (space.rs): 17 → its state for the map (`space_reply`: the generation the map reloads its circles on, this device's deck sync, the first pull done, the facts); 18 [on] this device's deck follows the fleet's or keeps its own → [0 done, 1 not yet: before the first pull].
                if n >= 1 && req[0] == 17 {
                    let r = self.space_reply();
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                // 21 [count u32 LE]: the doorbell. Answered [count u32 LE] at once when what the Fleet page and the map read (`watch_digest`) has changed since the caller's count, else held and answered the moment it does (`watch_ring`), so no client asks on a timer. At most 8 held: a ninth caller is answered at once with the count it has.
                if n >= 5 && req[0] == 21 {
                    self.watch_count();
                    let theirs = u32::from_le_bytes([req[1], req[2], req[3], req[4]]);
                    if theirs != self.watch_gen || self.watchers.len() >= 8 {
                        let r = self.watch_gen.to_le_bytes();
                        svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    } else {
                        self.watchers.push(tok);
                    }
                    continue;
                }
                // 19: this device's two-word name in the identity's fleet (`device_name_default` of its key under the identity seed), the one the approving device shows for its request; empty before the roots are in (owner, 2026-11-09: "would be nice to know what I'm adding").
                // 20: this device's departure given up (the Fleet page's back, owner 2026-11-09: "pressing back ... should stop me trying to leave"): forgotten here, in the vault and in the space, so no member is asked to approve it. [0] given up, [1] none under way. A member that countersigned it already has published the Remove; nothing here undoes that.
                if n >= 1 && req[0] == 20 {
                    let r = [self.give_up_depart()];
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                // 32 [1]: the person on this device confirmed the join (the attest screen's "Yes, I really would like to add this device and this is my identity and not a typo"; owner, 2026-11-22) → [0 taken, 1 nothing to confirm: no roots, bound, leaving, or the identity not found to have a fleet]. The binding request is minted in this same pass of the loop, and with it the join goes out (`join_confirmed`).
                // 32 [0]: the join withdrawn (the attest screen's back) → [0]: the request dropped, the tap secret and the BLE beacon stopped, the beacon under the handle silent again. A copy already handed to a fleet device or posted to fgtw.org lapses there on its own (the registry's five minutes).
                if n >= 2 && req[0] == 32 {
                    let r = [self.confirm_join(req[1] != 0)];
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                if n >= 1 && req[0] == 19 {
                    let r = self.identity_seed.map(|seed| fgtw::pair::device_name_default(&self.me, &seed).into_bytes()).unwrap_or_default();
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                if n >= 2 && req[0] == 18 {
                    let r = [self.space_switch(req[1] != 0)];
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                // 22 [automatic][the IANA id by hand]: the person's zone chosen on this device's clock page while it follows them (space.rs `space_zone`) → [0 done, 1 before the first pull, 2 not an id]. The kernel holds what the page shows (SYS_ZONE op 5).
                if n >= 2 && req[0] == 22 {
                    let r = [self.space_zone(req[1] != 0, &req[2..got])];
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                // 23 [the ISO 639-3 code]: the person's language chosen on this device's language page while it follows them (space.rs `space_lang`) → [0 done, 1 before the first pull, 2 not a code]. The kernel holds what the page shows (the preferences block).
                if n >= 1 && req[0] == 23 {
                    let r = [self.space_lang(&req[1..got])];
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                // 24 [on]: the person's do not disturb switched on this device's quick row (space.rs `space_dnd`) → [0 done, 1 before the first pull]. The kernel holds what the row shows (preference 40).
                if n >= 2 && req[0] == 24 {
                    let r = [self.space_dnd(req[1] != 0)];
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                // The person's people (facets.rs; docs/FACETS.md "People"): 25 → the People page; 26 [hp] remove; 27 [developer][option] → a kist's card's people; 28 [developer][option][all] all or chosen; 29 [developer][option][hp][on] pick; 30 [seed][proof] the handle typed on the keyboard (kind 2), the seed wiped here; 31 [name] the name typed (kind 3).
                if n >= 1 && (25..=31).contains(&req[0]) {
                    let k32 = |at: usize| -> [u8; 32] {
                        let mut o = [0u8; 32];
                        o.copy_from_slice(&req[at..at + 32]);
                        o
                    };
                    // WHY the lengths: another process's message, sliced below by fixed offsets.
                    let r: Vec<u8> = match req[0] {
                        25 => self.people_reply(),
                        26 if got >= 33 => alloc::vec![self.person_remove(&k32(1))],
                        27 if got >= 65 => self.kist_people_reply(&k32(1), &k32(33)),
                        28 if got >= 66 => alloc::vec![self.kist_people_set(&k32(1), &k32(33), Some(req[65] != 0), None)],
                        29 if got >= 98 => alloc::vec![self.kist_people_set(&k32(1), &k32(33), None, Some((k32(65), req[97] != 0)))],
                        30 if got >= 65 => {
                            let (mut seed, hp) = (k32(1), k32(33));
                            let r = self.person_roots(&mut seed, &hp);
                            req[1..33].fill(0);
                            alloc::vec![r]
                        }
                        31 => alloc::vec![self.person_name(&req[1..got])],
                        _ => alloc::vec![ERR as u8],
                    };
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                if n >= 1 && (8..=16).contains(&req[0]) {
                    let text = core::str::from_utf8(&req[1..got]).unwrap_or("");
                    let r: Vec<u8> = match req[0] {
                        8 if n >= 2 => alloc::vec![self.add_page(req[1] != 0)],
                        9 => self.add_reply(),
                        10 if n >= 9 => alloc::vec![self.add_pick(&req[1..9])],
                        11 => self.add_words(text),
                        12 => alloc::vec![self.add_yes()],
                        13 => self.depart_ask_reply(),
                        14 if n >= 2 => alloc::vec![self.answer_depart(req[1] != 0)],
                        // 2: a person's handle (the People page's add, facets.rs); 0 while their name is asked (3) ends it, the person added with no name.
                        15 if n >= 2 && req[1] == 2 => {
                            self.entry = 2;
                            alloc::vec![0]
                        }
                        // 4: a WiFi network's passphrase, for the network named after it ([15, 4, name length, name]; the WiFi page's picker). Only the name is kept here, never what is typed for it.
                        // WHY the lengths: another process's message; an SSID is 1 to 32 octets.
                        15 if got >= 3 && req[1] == 4 && (1..=32).contains(&req[2]) && 3 + req[2] as usize <= got => {
                            self.entry = 4;
                            self.entry_net = req[3..3 + req[2] as usize].to_vec();
                            alloc::vec![0]
                        }
                        // 5: the person's own handle again, to turn attest at boot on (the Security page; docs/AUTH.md "Unlock at boot"). The keyboard's host compares it thru the kernel (SYS_UNLOCK op 12) and ends it with op 15 [0]; nothing of it comes here. (Kind 4 was the retype gate's on its branch; forge had given 4 to the WiFi passphrase first, so it is 5 since the merge of 2026-11-20.)
                        15 if n >= 2 && req[1] == 5 => {
                            self.entry = 5;
                            alloc::vec![0]
                        }
                        // 6: the person's own handle again, to agree to the Android disclaimer (the Settings page's Android page; docs/BOOT.md "Android only by the identity's choice"). As 5: the keyboard's host compares it thru the kernel (SYS_UNLOCK op 14) and ends it with op 15 [0]; nothing of it comes here.
                        15 if n >= 2 && req[1] == 6 => {
                            self.entry = 6;
                            alloc::vec![0]
                        }
                        15 if n >= 2 && req[1] == 0 && self.entry == 3 => {
                            self.entry = 0;
                            self.person_no_name();
                            alloc::vec![0]
                        }
                        15 if n >= 2 && req[1] <= 1 => {
                            self.entry = req[1];
                            // "Type its words": the person's edge that reads fgtw.org's registry (owner, 2026-11-10: never on a timer).
                            if req[1] == 1 {
                                self.add_look();
                            }
                            alloc::vec![0]
                        }
                        // The words wanted; a WiFi passphrase's with the network's name after it ([4, name length, name]).
                        16 => match self.entry_wanted() {
                            4 => {
                                let mut r = alloc::vec![4, self.entry_net.len() as u8];
                                r.extend_from_slice(&self.entry_net);
                                r
                            }
                            k => alloc::vec![k],
                        },
                        _ => alloc::vec![ERR as u8],
                    };
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                let mut reply = Vec::with_capacity(400);
                // Op 6: the fleet's members, for the Fleet page (`members_reply`): its own reply, nothing of the state after it.
                // Op 7: how this device reaches its fleet, for the Fleet page: [the network up (either family) u8][seconds since fgtw.org last answered, u32 LE, all ones for never][fleet devices heard on this network u8].
                if n >= 1 && req[0] == 7 {
                    let mut r = [0u8; 6];
                    r[0] = self.net_up as u8;
                    let age = if self.seed_heard_us == 0 { u32::MAX } else { ((now_us() - self.seed_heard_us) / 1_000_000) as u32 };
                    r[1..5].copy_from_slice(&age.to_le_bytes());
                    // WHY the cap: a byte on the wire; past 255 devices heard the page has nothing different to say.
                    r[5] = self.sponsors.iter().filter(|s| s.online).count().min(255) as u8;
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                if n >= 1 && req[0] == 6 {
                    let r = self.members_reply();
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                // Op 2: mint (the attest screen, after the permanence warning and the retype), `[2][the retyped handle's seed 32]` → [result: 0 minted, 6 the retyped handle is not the one handed over (nothing minted), else why not], then the state as op 1 gives it.
                // The attest screen keeps no seed of its own past the hand-over (2026-11-17), so the two are compared here, in time that does not depend on where they differ, and the retyped seed is wiped from the message.
                if n >= 1 && req[0] == 2 {
                    // WHY the length: another process's message; the seed is sliced out of it at fixed offsets.
                    let same = got >= 33 && self.identity_seed.as_ref().is_some_and(|s| s.iter().zip(&req[1..33]).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0);
                    wipe(&mut req[1..33]);
                    let r = match (self.identity_seed.is_some(), same) {
                        (false, _) => 1,
                        (true, true) => self.mint(),
                        (true, false) => 6,
                    };
                    reply.push(r);
                }
                // Op 3: the attest screen replaced the roots on disk (a new handle on a device that was not yet bound): forget the old identity and look the new one up from the vault.
                if n >= 1 && req[0] == 3 {
                    self.forget_identity();
                    self.read_persist();
                    self.load_saying(true);
                    if self.hp.is_some() {
                        self.verdict = V_LOOKING;
                        self.last_lookup_us = 0;
                        self.fgtw_asked_us = 0;
                    }
                }
                // Op 4: the attest screen hands the roots in RAM (`[4][seed 32][proof 32]`), the default — nothing of the identity written unless unlock-at-boot is on. Forget the old, take these, persist or wipe per the setting, look up.
                // WHY the length check: another process's message, and the seed and proof are sliced out of it below (req[1..65]); a shorter one would read past what was sent. It said 69 from 2026-10-28 to 2026-11-04 while the attest screen sent 65, so the roots never arrived and the screen stayed on "Finding this identity.".
                if n >= 65 && req[0] == 4 {
                    let mut seed = [0u8; 32];
                    let mut hp = [0u8; 32];
                    seed.copy_from_slice(&req[1..33]);
                    hp.copy_from_slice(&req[33..65]);
                    // The message's copy wiped at once; the locals once they are in `self`.
                    wipe(&mut req[1..65]);
                    self.forget_identity();
                    self.read_persist();
                    if !self.persist {
                        self.forget_disk();
                    }
                    // Whether the vault took both (`persist_set` says true with the setting off, where nothing is to be kept): said below, so the log never claims a keep the vault refused.
                    let kept = self.persist_secret(ferros_kept::secret::Kind::IdentitySeed, b"identity_seed", &seed) & self.persist_secret(ferros_kept::secret::Kind::HandleProof, b"handle_proof", &hp);
                    self.identity_seed = Some(seed);
                    self.hp = Some(hp);
                    wipe(&mut seed);
                    wipe(&mut hp);
                    // The epoch these roots belong to: the attest screen unlocked (which moved the epoch and wrote it here) before it handed them over, so a bell read now is past any lock that came before them.
                    self.lock_epoch = ID_BELL.load(Ordering::Acquire);
                    self.verdict = V_LOOKING;
                    self.last_lookup_us = 0;
                    self.fgtw_asked_us = 0;
                    self.log.put(match (self.persist, kept) {
                        (true, true) => b"fgtw: roots set in RAM and kept on disk (unlock at boot on)",
                        (true, false) => b"fgtw: roots set in RAM; unlock at boot is on, but the vault did not take them (a boot that unlocks shows the attest screen for the handle)",
                        (false, _) => b"fgtw: roots set in RAM only (nothing of the identity on disk)",
                    });
                    self.log.line();
                    // The departure and add machines, made with these roots: a departure or an add kept from before a restart is taken up now (with unlock-at-boot off, nothing of the identity is on disk at `load`).
                    self.depart_start();
                    self.add_start();
                }
                // Op 5: leave the fleet, `[5][intent]` (1 to a new owner, 2 to the desk) → [result: 0 started, 4 the identity's last device, else why not (depart.rs `leave`)], then the state (8 leaving).
                if n >= 2 && req[0] == 5 {
                    let r = self.leave(req[1]);
                    reply.push(r);
                }
                let state: u8 = match (self.hp.is_some(), self.bound, self.fleet_key.is_some()) {
                    (false, _, _) => V_NONE,
                    (true, false, _) => self.verdict,
                    (true, true, false) => 2,
                    (true, true, true) => 3,
                };
                let leaving = self.depart_asking();
                let state = if leaving { V_LEAVING } else { state };
                // Selected by a member and not bound yet: the green screen, the selecting device's name after the state.
                let state = if !self.bound && !leaving && self.selected_by.is_some() { V_SELECTED } else { state };
                // Joining and not yet confirmed by this device's person: the attest screen asks first (op 32), and nothing of the join has gone out.
                let state = if state == V_JOIN && !self.join_confirmed { V_CONFIRM } else { state };
                reply.push(state);
                reply.push(self.sponsors.len().min(255) as u8);
                if let (Some(seed), V_SELECTED, Some(by)) = (self.identity_seed, state, self.selected_by) {
                    reply.extend_from_slice(fgtw::pair::device_name_default(&by, &seed).as_bytes());
                }
                // Joining's pairing words follow the state once the join is confirmed (state 1), the last resort when NFC and BLE fail: the attest screen keeps them hidden until its person asks ("Not in range?"; owner, 2026-11-22, shoulder surfing). A departure shows none (owner, 2026-11-10), even one resumed before the chain is adopted again; nor does a join not yet confirmed (state 11).
                if let (Some(seed), V_JOIN) = (self.identity_seed, state) {
                    reply.extend_from_slice(join::pair_words(&self.me, &seed).as_bytes());
                }
                svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, reply.as_ptr() as u64 - self.udp.base, reply.len() as u64, 0, 0);
            }
            lap.mark(b"the attest screen");
            // Joining: the seed's copy of the chain every 16 s, for the sponsor's Add. A sponsor hands the chain and the fan-out to a newcomer at the address its request came from on the LAN; one it knows only from the registry or the tap has no address, and the newcomer has to look (2026-11-04: Brittany's phone bound this device by the tap and the words stayed up). The clock only moves forward, so `t` is never behind the last check.
            if !self.bound && !self.depart_asking() && self.verdict == V_JOIN && t - self.join_checked_us >= 16_000_000 {
                self.join_checked_us = t;
                self.join_check();
            }
            lap.mark(b"join_check");
            // Leaving: the seed's copy of the chain every 16 s, its fold to the departure machine (depart.rs).
            self.depart_seed_check(t);
            lap.mark(b"depart_seed_check");
            busy |= self.facets_now(&mut lap);
            // The departure machine's slow work: signing a request or a deny, an approval's publish, a chain read (depart.rs), here in the loop and never inside a call another process waits on.
            self.depart_work();
            lap.mark(b"depart_work");
            busy |= self.facets_now(&mut lap);
            // Sponsoring (add.rs): the radios listening for newcomers and for a departure's tap, the add machine's deadline, and its slow work (signing, the registry's read, the bind, the grow), here in the loop and never inside a call another process waits on.
            self.sponsor_step();
            lap.mark(b"sponsor_step");
            busy |= self.facets_now(&mut lap);
            // The person's identity space: the first pull, then this device's copy joined to it and handed on (space.rs).
            self.space_step(t);
            lap.mark(b"space_step");
            // The facets (facets.rs): calls on `facets` taken in, one held binding signed, an add's chain looked up.
            busy |= self.facets_now(&mut lap);
            self.person_step();
            lap.mark(b"person_step");
            // The verdict: looked up once the network is up, and again every 30 s while GREY or while a fresh handle waits (someone else may mint it meanwhile).
            // The client serves from the start, network or not (the attest screen waits on it); the network coming up is a reason to look up at once, not 30 s later.
            // Either family will do (INFO bit 0 an IPv4 route, bit 1 an IPv6 one): fgtw.org answers on both, and the resolver picks the family the route will use. Until 2026-11-13 only IPv4 counted, and the 8a with no DHCPv4 lease but a working IPv6 stayed grey.
            // The flags as the watch last said them (0 until it has), or INFO every pass without a watch.
            let fl = if watch == NO_WATCH { self.udp.flags().unwrap_or(0) } else { watch as u8 };
            let up = fl & 3 != 0;
            if up && !self.net_up {
                self.net_up = true;
                self.last_lookup_us = 0;
                self.fgtw_asked_us = 0;
                self.log.put(match fl & 3 {
                    1 => b"fgtw: the network is up (IPv4)".as_slice(),
                    2 => b"fgtw: the network is up (IPv6)",
                    _ => b"fgtw: the network is up (IPv4 and IPv6)",
                });
                self.log.line();
                // A departure frame or a fan-out given up on is handed again on the new network.
                self.depart_event(fgtw::flow::Event::LinkUp);
                self.add_event(fgtw::flow::Event::LinkUp);
            }
            // Also the moment FGTW's answer window closes (`lookup`), so fgtw.org follows a silent network a quarter second later, not at the next 2 s.
            let window_closed = self.fgtw_asked_us != 0 && t.wrapping_sub(self.fgtw_asked_us) >= 1 << 18;
            if self.hp.is_some() && !self.bound && matches!(self.verdict, V_LOOKING | V_UNREACHABLE | V_FRESH) && (window_closed || t.saturating_sub(self.last_lookup_us) >= if self.verdict == V_LOOKING { 2_000_000 } else { 30_000_000 }) {
                self.last_lookup_us = t;
                if up {
                    self.offline_looks = 0;
                    self.lookup();
                } else if self.verdict == V_LOOKING && self.sponsors.is_empty() {
                    // No network to ask and no fleet device heard: after 16 s of that the honest answer is GREY (no verdict), which lets the person go on; the look-ups carry on at the grey cadence and from the moment the network is up.
                    self.offline_looks += 1;
                    if self.offline_looks >= 8 {
                        self.offline_looks = 0;
                        self.verdict = V_UNREACHABLE;
                        self.log.put(b"fgtw: no network and no fleet device heard: no verdict (GREY)");
                        self.log.line();
                    }
                }
            }
            lap.mark(b"lookup");
            busy |= self.facets_now(&mut lap);
            // The binding request only once there is a fleet to ask (the verdict says joining) and the person on this device has confirmed the join (op 32, owner 2026-11-22: nothing reaches their devices on a typed handle alone): minting it signs in three families, SLH-DSA-128s among them, which takes seconds; it is done here in the loop, never inside a call another process waits on.
            if !self.bound && !self.depart_asking() && self.hp.is_some() && self.verdict == V_JOIN && self.join_confirmed && self.request_frame.is_empty() {
                self.mint_request();
            }
            lap.mark(b"mint_request");
            // Bound: nothing left to prove proximity for.
            if self.bound && self.radios_on {
                self.radios(None);
            }
            // A driver that missed the handover (down, or still coming up): tried again every 16 s while unbound.
            if !self.bound && self.radios_missed && now().wrapping_sub(self.radios_at) > ferros_sys::freq() * 16 {
                if let Some((s, id)) = self.radios_last {
                    self.radios(Some((&s, &id)));
                }
            }
            lap.mark(b"radios");
            // The beacon under the handle: a member's, and a joining device's once its person has confirmed (`join_confirmed`); a device not yet in the fleet beacons nothing before then, so a handle typed alone announces nothing to the person's devices on this network.
            if self.hp.is_some() && (self.bound || self.join_confirmed) && t.saturating_sub(self.last_beacon_us) >= 3_000_000 {
                self.last_beacon_us = t;
                self.beacon();
            }
            lap.mark(b"beacon");
            if self.bound && t.saturating_sub(self.last_ping_us) >= 10_000_000 {
                self.last_ping_us = t;
                self.pings();
            }
            lap.mark(b"pings");
            if !self.bound && self.hp.is_some() {
                if t.saturating_sub(self.last_words_us) >= 60_000_000 {
                    self.last_words_us = t;
                    self.show_words();
                }
                // The app re-posts its request before the registry's 5-minute freshness lapses; the same stamp discipline here, so a sponsor's candidate stays fresh.
                if self.verdict == V_JOIN && self.request_t != 0 && (eagle() - self.request_t) > 210 * vsf::OSCILLATIONS_PER_SECOND as i64 {
                    self.mint_request();
                    for s in self.sponsors.iter_mut() {
                        s.asked_t = 0;
                    }
                }
            }
            // The doorbell: whoever waits is told of a change made by this pass.
            if !self.watchers.is_empty() {
                self.watch_ring();
            }
            // And the kists waiting for be you (facets op 2), of a change in its readiness made by this pass.
            self.facets_ring();
            lap.mark(b"words and rings");
            lap.said(&mut self.log);
            // Work in hand, or PT moving a frame (its acks and resends are paced by its own clock): the next pass a millisecond on, as before.
            // The person at the Fleet page (the add machine open, the radios listening for a tap or a beacon, a departure under way): every 20 ms as before, since the radios are asked for what they heard (sponsor_hear) and the machines' deadlines are theirs.
            // Otherwise nothing is asked on a clock: the wait ends when the network watch rings (`NET_BELL`), when a call comes on an endpoint this thread polls (`fgtw`, `facets`), or at the loop's next deadline (`idle_until`). Without a watch (an ip from before op 11), every 20 ms as before.
            let f = ferros_sys::freq();
            let until = if busy || self.pt.busy() {
                now() + f / 1000
            } else if watch == NO_WATCH || self.attending() {
                now() + f / 50
            } else {
                self.idle_until(t)
            };
            ferros_sys::wait_world_or_call(&NET_BELL, bell_seen, until);
        }
    }

    /// The person at work with the fleet: the add machine open (the Fleet page), the radios listening for a tap or a beacon (they are asked each pass for what they heard, `sponsor_hear`), or a departure under way. The loop keeps its 20 ms cadence then.
    fn attending(&self) -> bool {
        self.listening != (false, false) || self.add_phase() != fgtw::flow::add::Phase::Closed || self.depart_asking() || self.depart_asked().is_some()
    }

    /// When the loop's idle wait ends at the latest (counter ticks, 0 for none), from the pass's time `t` (microseconds): the earliest of the timers its steps keep. Each is the step's own rule, read here as it reads it: the beacon every 3 s and the pings every 10 s, the look-up (2 s while looking, 30 s while grey or fresh, FGTW's quarter-second answer window), joining's chain read every 16 s and the request re-minted at 210 s, the words every 60 s, a radio handover tried again at 16 s, a departure's seed read every 16 s, the space's join and seed (`space_due`), the add machine's deadline. Work in hand (a binding to sign, an add's look-up) is at once. None of them: no deadline, the wait ends at a ring or a call.
    fn idle_until(&self, t: u64) -> u64 {
        let mut due = u64::MAX;
        // WHY the mins: the earliest of the deadlines, not a bound on anyone's value.
        let mut at = |d: u64| due = due.min(d);
        if self.facets_pending() {
            at(t);
        }
        if self.hp.is_some() && (self.bound || self.join_confirmed) {
            at(self.last_beacon_us + 3_000_000);
        }
        if self.bound {
            at(self.last_ping_us + 10_000_000);
        }
        if !self.bound && !self.depart_asking() && self.verdict == V_JOIN {
            at(self.join_checked_us + 16_000_000);
        }
        if self.depart_asking() {
            at(self.dep.checked_us + 16_000_000);
        }
        if self.hp.is_some() && !self.bound && matches!(self.verdict, V_LOOKING | V_UNREACHABLE | V_FRESH) {
            if self.fgtw_asked_us != 0 {
                at(self.fgtw_asked_us + (1 << 18));
            }
            at(self.last_lookup_us + if self.verdict == V_LOOKING { 2_000_000 } else { 30_000_000 });
        }
        if !self.bound && self.hp.is_some() && self.join_confirmed {
            at(self.last_words_us + 60_000_000);
        }
        if let Some(d) = self.space_due() {
            at(d);
        }
        // The Eagle-time deadlines (the add machine's, the request's 210 s), as microseconds from now.
        let osc = vsf::OSCILLATIONS_PER_SECOND as i128;
        let e = eagle() as i128;
        let mut eagle_at = |when: i64| {
            let us = (when as i128 - e) * 1_000_000 / osc;
            at(if us <= 0 { t } else { t + us as u64 });
        };
        if let Some(w) = self.add_wake_at() {
            eagle_at(w);
        }
        if !self.bound && self.hp.is_some() && self.verdict == V_JOIN && self.request_t != 0 {
            eagle_at(self.request_t + 210 * vsf::OSCILLATIONS_PER_SECOND as i64 + 1);
        }
        let f = ferros_sys::freq();
        let mut ticks = if due == u64::MAX { 0 } else { due * (f / 1_000_000).max(1) };
        // The radio handover's retry is on the counter already.
        if !self.bound && self.radios_missed {
            let r = self.radios_at + 16 * f + 1;
            ticks = if ticks == 0 { r } else { ticks.min(r) };
        }
        // A deadline already past ends the wait at once (0 would mean none).
        if ticks == 0 && due != u64::MAX { 1 } else { ticks }
    }

    /// What the Fleet page and the map read, in one piece: the state (op 1's), the members (op 6), a departure waiting (op 13), the add flow (op 9), this device's own departure denied, and the space (op 17).
    fn watch_state(&mut self) -> Vec<u8> {
        let mut v = alloc::vec![self.hp.is_some() as u8, self.join_confirmed as u8, self.bound as u8, self.fleet_key.is_some() as u8, self.depart_asking() as u8, self.verdict, self.sponsors.len() as u8];
        // The select too: the attest screen turns green on it.
        v.extend_from_slice(self.selected_by.as_ref().map_or(&[0u8; 32][..], |s| &s[..]));
        v.extend_from_slice(&self.members_reply());
        v.extend_from_slice(&self.depart_ask_reply());
        v.extend_from_slice(&self.add_reply());
        v.extend_from_slice(self.dep.denied_by.as_ref().map_or(&[0u8; 32][..], |d| &d[..]));
        v.extend_from_slice(&self.space_reply());
        // The People page: an add's outcome and the people (a change made here, or merged from another device).
        v.extend_from_slice(&self.people_reply());
        v
    }

    /// The change count moved on when what is watched differs from what was last counted.
    fn watch_count(&mut self) {
        let d = self.watch_state();
        if d != self.watch_seen {
            self.watch_seen = d;
            self.watch_gen = self.watch_gen.wrapping_add(1);
        }
    }

    /// Every held doorbell answered with the new count, once there is a change.
    fn watch_ring(&mut self) {
        let was = self.watch_gen;
        self.watch_count();
        if self.watch_gen == was {
            return;
        }
        let r = self.watch_gen.to_le_bytes();
        for tok in core::mem::take(&mut self.watchers) {
            svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
        }
    }
}


/// The sponsor's and the approver's radios and the add machine's turn in the loop (add.rs carries the machine's actions out). Everything that signs or reaches the seed runs in the loop (`sponsor_step`), never inside a call another process waits on; the Fleet page's ops only tell the machine what the person did.
impl Client {
    /// Op 16: the words wanted from the person now, dropped once nothing is left to type them for (the add flow past picking). A departure takes no words (op 15 sets 0 or 1 only).
    fn entry_wanted(&mut self) -> u8 {
        if self.entry == 1 && self.add_phase() != fgtw::flow::add::Phase::Open {
            self.entry = 0;
        }
        self.entry
    }

    /// The radios listening: the NFC driver reading a tapped device's tag (op 4 on, op 5 off) while the add machine wants it or a fleet device's departure waits here, the Bluetooth driver scanning for pairing beacons (op 3 on, op 4 off) while the add machine wants it. CALL_LIVE: a driver that is down fails at once.
    fn sponsor_radios(&mut self, on: (bool, bool)) {
        let (nfc, bt) = (cap(unsafe { GRANTS }, S_NFC), cap(unsafe { GRANTS }, S_BT));
        let (mut ra, mut rb) = ([0u8; 4], [0u8; 4]);
        let a = ferros_sys::call_live(nfc, &[if on.0 { 4 } else { 5 }], &mut ra);
        let b = ferros_sys::call_live(bt, &[if on.1 { 3 } else { 4 }], &mut rb);
        let changed = on != self.listening;
        self.listening = on;
        self.listen_pass = 0;
        let code = |n: u64, r: &[u8; 4]| -> u8 { if n == ERR { 2 } else if n >= 1 && r[0] != 0 { 1 } else { 0 } };
        let now_said = (code(a, &ra), code(b, &rb));
        if !changed && now_said == self.listen_said {
            return;
        }
        self.listen_said = now_said;
        let said = |n: u64, r: &[u8; 4]| -> &'static [u8] { if n == ERR { b"not running" } else if n >= 1 && r[0] != 0 { b"refused" } else { b"ok" } };
        self.log.put(match on {
            (true, true) => b"fgtw: listening for newcomers: nfc reader ".as_slice(),
            (true, false) => b"fgtw: listening for a departure's tap: nfc reader ",
            _ => b"fgtw: no longer listening: nfc reader ",
        });
        self.log.put(said(a, &ra));
        self.log.put(b", bt scan ");
        self.log.put(said(b, &rb));
        self.log.line();
    }


    /// What the radios heard: the Bluetooth driver's pairing beacons (op 5: [count][count × 16 bytes], every id heard in its last 30 s) and the NFC driver's last tag read (op 6: [32 bytes] once, then nothing). `on` is what listens: (the NFC reader, the BLE scan).
    /// A tag whose secret is the salt of the departure asked about here approves it (the departure machine's `NfcRead`); any other tag read while the add machine listens goes to it, which adds the device whose request it opens.
    fn sponsor_hear(&mut self, on: (bool, bool)) {
        let (nfc, bt) = (cap(unsafe { GRANTS }, S_NFC), cap(unsafe { GRANTS }, S_BT));
        let mut r = [0u8; 1 + 16 * fgtw::pair::BEACON_UUID_LEN];
        let n = if on.1 { ferros_sys::call_live(bt, &[5], &mut r) } else { ERR };
        if n != ERR && n >= 1 {
            // WHY: the count is the driver's word, and the reply's length may run past this buffer (it says what was sent): only the ids both hold are read.
            let k = (r[0] as usize).min((n as usize - 1) / fgtw::pair::BEACON_UUID_LEN).min(16);
            let mut ids = Vec::with_capacity(k);
            for i in 0..k {
                let mut u = [0u8; fgtw::pair::BEACON_UUID_LEN];
                u.copy_from_slice(&r[1 + i * fgtw::pair::BEACON_UUID_LEN..1 + (i + 1) * fgtw::pair::BEACON_UUID_LEN]);
                ids.push(u);
            }
            self.add_ble(ids);
        }
        let mut s = [0u8; 32];
        if on.0 && ferros_sys::call_live(nfc, &[6], &mut s) == 32 {
            // The leaving device's tag: its salt names the request asked about here.
            if self.depart_asked().is_some_and(|(_, _, salt)| salt == s) {
                self.log.put(b"fgtw: a tapped device's tag is the departure asked about here: approving it");
                self.log.line();
                self.depart_event(fgtw::flow::Event::NfcRead { secret: s });
            } else if self.add.radios.0 {
                self.add_nfc(s);
            }
        }
    }

    /// The sponsor's turn, once a pass of the loop: the radios as the add machine and a departure asked about here want them, what they heard, the add machine's deadline, and its slow work.
    fn sponsor_step(&mut self) {
        // The NFC reader also while a departure is asked about here, so a tap of the leaving device (its tag serves the request's salt) approves it.
        let asked = self.bound && self.depart_asked().is_some();
        let on = (self.add.radios.0 || asked, self.add.radios.1);
        self.listen_pass += 1;
        if on != self.listening || (on != (false, false) && self.listen_pass >= 64) {
            self.sponsor_radios(on);
        }
        if on != (false, false) {
            self.sponsor_hear(on);
        }
        self.add_wake_check();
        self.add_work();
    }
}

static mut GRANTS: u64 = 0;

/// The fgtw crate's own account of what it did (a seed mirror that failed, a refused add), on the console as this process's lines are.
fn crate_log(s: &str) {
    let mut b = Vec::with_capacity(s.len() + 14);
    b.extend_from_slice(b"fgtw: crate: ");
    b.extend_from_slice(s.as_bytes());
    b.push(b'\n');
    let mut r = [0u8; 16];
    ferros_sys::call(cap(unsafe { GRANTS }, 0), &b, &mut r);
}

extern "C" fn work(_: u64) -> ! {
    let grants = unsafe { GRANTS };
    let base = ferros_sys::arc_base();
    let mut log = Log { grants, buf: Vec::new() };
    let udp = Udp { grants, base };
    // No wait for the network here: the `fgtw` endpoint is served from the start, so the attest screen always gets an answer (until 2026-10-31 this thread waited for an IPv4 lease first, and with no network attest froze on the roots hand-over). The UDP ports and beacon groups are taken now; the IP process keeps the groups and reports them once the link is up; look-ups wait in the loop for an address of either family.
    // The device key, rooted in the ira.
    let Some(fingerprint) = ferros_sys::ira_key(cap(grants, S_IRA), "fgtw device") else {
        log.put(b"fgtw: no ira-derived key (no Ira grant, or the kernel has no ira this boot)");
        log.line();
        ferros_sys::exit(1)
    };
    // On the X3 too (673 ms on the workers, 2026-11-04): the three families' keygen, once per start.
    ferros_sys::pin(ferros_sys::CORES_BIG);
    ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
    let t0 = now();
    let keys = DeviceKeys::derive(&fingerprint);
    ferros_sys::pin(0);
    log.put(b"fgtw: device keys derived in ");
    log.dec(ferros_sys::us_since(t0) / 1000);
    log.put(b" ms");
    log.line();
    let me = keys.device_pubkey();
    let pt = Pt::new(keys.signing_key());
    let given = unsafe { GIVEN };
    let mut c = Client { log, udp, keys, me, identity_seed: None, hp: None, bound: false, fleet_key: None, lock_epoch: 0, members: Vec::new(), chain_seen: 0, pt, sponsors: Vec::new(), request_t: 0, request_frame: Vec::new(), join_confirmed: false, radios_on: false, radios_last: None, radios_missed: false, radios_at: 0, dep: depart::Departing::new(), net_up: false, offline_looks: 0, last_beacon_us: 0, last_ping_us: 0, last_words_us: 0, persist: false, chain_held: Vec::new(), verdict: V_NONE, last_lookup_us: 0, seed_heard_us: 0, join_checked_us: 0, seed_ip: None, given, given_asked_t: 0, held: core::cell::RefCell::new(fgtw::held::HeldChains::new()), held_fan: core::cell::RefCell::new(fgtw::held::HeldFanouts::new()), add: add::Sponsoring::new(), listening: (false, false), listen_pass: 0, listen_said: (255, 255), offered: Vec::new(), select_waiting: None, selected_by: None, entry: 0, entry_net: Vec::new(), space: space::SpaceSync::new(), watchers: Vec::new(), watch_gen: 0, watch_seen: Vec::new(), fgtw_asked_us: 0, facets: facets::Facets::new() };
    c.run()
}

static mut GIVEN: Option<Addr> = None;

#[unsafe(no_mangle)]
pub extern "C" fn fgtw_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_BYTES) };
    unsafe { GRANTS = grants };
    fgtw::set_entropy(kernel_entropy);
    fgtw::set_log(crate_log);
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
    stack.fill(STACK_PAINT);
    ferros_sys::thread_spawn(work, stack, 0);
    ferros_sys::thread_exit(0)
}

/// A panic says what and where on the console before the process ends (nothing allocated: the heap may be why).
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    struct Buf {
        b: [u8; 256],
        n: usize,
    }
    impl core::fmt::Write for Buf {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let k = s.len().min(self.b.len() - self.n);
            self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
            self.n += k;
            Ok(())
        }
    }
    let mut b = Buf { b: [0; 256], n: 0 };
    let _ = core::fmt::Write::write_fmt(&mut b, format_args!("fgtw: PANIC {}", info.message()));
    if let Some(l) = info.location() {
        let _ = core::fmt::Write::write_fmt(&mut b, format_args!(" at {}:{}", l.file(), l.line()));
    }
    let _ = core::fmt::Write::write_str(&mut b, "\n");
    let mut r = [0u8; 16];
    ferros_sys::call(cap(unsafe { GRANTS }, 0), &b.b[..b.n], &mut r);
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}
