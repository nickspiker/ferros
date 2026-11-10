//! fgtw on ferros: the OS's identity service, part of attest, never an app (owner, 2026-11-04: "Photon is just an app. on non ferros it just happens to have the FGTW bits in it too. Just like Mahere, or Lumis"). Elsewhere these FGTW bits ship inside the Photon app; on ferros they are the system's, and Photon the app is something a person installs like any other. It looks the attesting identity up (its fleet's devices nearby first, fgtw.org last), mints a genesis when there is none, joins this device to the identity's fleet and stands as one of its devices, and leaves the fleet on request. It reads no photons: messages are the Photon app's.
//!
//! The fleet over the LAN (photon docs/pairing-v2.md "The ceremony over the mesh"; STATUS.md "Enrollment / pairing"): an EL0 process on the `ip` process's UDP, it beacons as the stock app does (`pt_disc` to the app's multicast groups on 4384), hears the owner's other devices beaconing under the same handle, hands the nearest sponsor its binding request as a `bind_req` over Photon Transport, takes the chain (`fleet_resp`) and the fan-out (`fanout_resp`) the sponsor pushes back, folds the chain against its own roots and opens the fan-out with its own ira, and from then on answers pings as a fleet member. Everything it speaks is photon_core (pt, join, wire), pinned byte for byte against the app by tools/photon-interop.
//!
//! Identity: the device key is rooted in the kernel's ira (`ira_key("fgtw device")` → `join::DeviceKeys::derive`; "photon device" until 2026-11-04, changed when no fleet held this device), the same after a factory reset; the identity roots (`identity_seed`, `handle_proof`) come from the attest screen (op 4, in RAM) or are pushed into the vault scope `fgtw.` by the dev box (tools/photon-interop `roots` + `ferros-bridge store put`) - the handle itself never reaches the device. Once bound, the chain (`fgtw.chain`) and the fleet key (`fgtw.fleet_key`) live in the same scope. A departure under way is kept there too (`fgtw.depart`, whatever unlock-at-boot says: it holds no root) until it completes, so a restart resumes it. The 23 pairing words are printed on the console for the sponsor's human to type; the sponsor's screen also shows this device as a nearby candidate it can tap.
//!
//! Started by init once a person has attested (payloads/init), or from the bridge: ferros-bridge spawn fgtw.bin [hex: fam, address 16, port BE — a sponsor to hand the binding request (and a departure request) to directly, for a network whose access point does not pass multicast between its wireless clients; the vault key `fgtw.sponsor` holds the same 19 bytes for the boot-started service] --name fgtw --use udp --store fgtw --ira --serve fgtw --entropy --use nfc --use bt --unlock --store map --store kists. The `fgtw` endpoint (the attest screen's): every op replies [state][fleet devices heard][words], the state as `V_*` below (0 no roots, 1 joining, 2 bound without the key, 3 member, 4 looking, 5 fresh, 6 wrong fleet, 7 grey, 8 leaving) and the words the 23 pairing words while joining or the 3 approval words while leaving; op 1 the state alone, op 2 mint (a result byte first), op 3 roots replaced on disk, op 4 `[4][seed][proof]` roots in RAM, op 5 `[5][intent]` leave the fleet (a result byte first; intent 1 to a new owner, 2 to the desk). Grant slots: 0 console, 1 udp (use), 2 store fgtw, 3 ira, 4 serve fgtw, 5 entropy, 6 nfc (use), 7 bt (use), 8 the identity unlock right (bind when the fleet binds this device, release when a departure completes), 9 the vault scope `map.` and 10 the vault scope `kists.` (the person's identity space: their home screen and their kists, joined to their fleet's, space.rs): while unbound, each binding request's tap secret is served on the NFC tag and its beacon id advertised over BLE (`radios`). Runs on its own 128 KiB thread stack (Falcon's degree-512 signing key is about 60 KiB by value) over a 160 KiB heap in `.data`.

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

mod space;

/// The heap and the work thread's stack, in .bss: the entry's header (`ferrosRT`, ferros_rt's) tells the kernel how much zeroed memory follows the code, so the arc holds them (payload.ld). Was 160 KiB in .data to fit a 1 MiB arc: a re-minted binding request (an SLH-DSA signature, 7856 B) then found no free run once the client had run a while, and the client panicked (2026-10-28).
const HEAP_BYTES: usize = 2 << 20;
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

/// A departure from the fleet in progress (pairing-v2 "Chain changes": removal is bilateral, the leaver's consent countersigned by a surviving member).
/// Kept in the vault as `fgtw.depart` (`DEPART_KEY`) from the moment it starts until it completes, so a reboot resumes it rather than dropping it (`keep_depart`, `resume_depart`).
struct Depart {
    t: i64,
    /// The request's salt (fgtw::protocol::DepartReq): a denied request is never asked again, so a new one takes a new salt.
    salt: [u8; 32],
    frame: Vec<u8>,
    checked_us: u64,
}

/// Where this device stands with the identity whose roots it holds (the `fgtw` endpoint's op 1 state byte; pairing-v2's lamp). 0 no roots, 1 joining (the identity has a fleet, this device is not in it yet: RED, words shown), 2 bound waiting for the fleet key, 3 a member with the key, 4 looking the identity up, 5 fresh (fgtw.org holds no chain and no fleet device is heard: it may be minted), 6 wrong fleet (the seed's chain for this handle has another identity's genesis), 7 unreachable (no peer heard and no seed answer: GREY, never a verdict), 8 leaving (a departure request out, its approval words shown), 9 owned elsewhere (fgtw.org says this device is in another identity's fleet: no words).
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

/// The kept departure's name in the `fgtw.` scope: one VSF document, section `depart`, with `rq` the signed `depart_req` frame as a binary blob (its stamp, consent and salt inside it).
/// Nothing of the identity is in it: the frame is this device's signature over its consent, and which identity it leaves is proven again at resume by checking the consent's Ed25519 egg against the roots then in hand (`resume_depart`).
const DEPART_KEY: &[u8] = b"depart";
/// The departure requests denied (`Client::denied`): one VSF document, section `denied`, a `request` field each (the leaving device `ke`, the salt `hb`, the consent stamp `e`).
const DENIED_KEY: &[u8] = b"denied";
/// One transfer of the vault (the kernel's STORE_MAX): larger values go in pieces.
const STORE_PIECE: usize = 4096;

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
    /// The departure stamp this sibling was handed last (0 = none yet).
    depart_t: i64,
    /// The request stamp this sponsor was handed last (0 = none yet).
    asked_t: i64,
    heard_us: u64,
    online: bool,
}

/// The add flow's phases, as the Fleet page reads them (op 9).
const ADD_CLOSED: u8 = 0;
/// Open: candidates listed from the registry and the LAN, nothing picked.
const ADD_OPEN: u8 = 1;
/// A candidate picked (by its words, its tag, or nearby): the Add being signed and published.
const ADD_BINDING: u8 = 2;
/// Selected (a nearby row touched on the screen): nothing bound; the device was told (`bind_select`) and should be green, and Add binds and grows it as one act. Back returns to the list with nothing to undo (owner, 2026-11-10, after a pick 50 ft away stayed bound when Back was pressed).
const ADD_SELECTED: u8 = 3;
/// The fan-out being grown to the newcomer.
const ADD_GROWING: u8 = 4;
/// Added: the newcomer is in the chain and holds the fleet key.
const ADD_DONE: u8 = 5;
/// The last step failed; `note` says why.
const ADD_FAILED: u8 = 6;

/// How long a binding request heard over the LAN, or a beacon over BLE, counts as "nearby" (Photon's LAN_HEARD_FRESH).
const NEARBY_US: u64 = 30_000_000;

/// The sponsor's side of adding a device to this fleet (fgtw::sponsor; photon docs/pairing-v2.md), while the Fleet page's add flow is open. The newcomer's binding request reaches this device from the seed's registry (listed every 8 s while open) and over the LAN (`bind_req`, kept whether or not the flow is open, as Photon keeps it); its BLE beacon and NFC tag are read by the radios' drivers.
struct Adding {
    phase: u8,
    /// The registry's last list, and when it was asked.
    registry: Vec<fgtw::fleet::BindRequest>,
    listed_us: u64,
    /// The candidate rows (fgtw::sponsor::candidates), rebuilt whenever a source changes.
    cands: Vec<fgtw::sponsor::Candidate>,
    /// The request picked and how (its words matched, Add pressed on a selected device, or an NFC tap), and the device bound. Either way the key follows at once.
    picked: Option<(fgtw::fleet::BindRequest, &'static str)>,
    bound: Option<[u8; 32]>,
    /// The device selected and told so, waiting for the person's Add (`select`).
    selected: Option<fgtw::fleet::BindRequest>,
    /// The person's confirm, waiting for the loop to grow the fan-out.
    confirm: bool,
    /// The words typed so far (op 11), matched live.
    words: alloc::string::String,
    /// What the Fleet page shows under the phase.
    note: alloc::string::String,
}

impl Adding {
    const fn new() -> Self {
        Adding { phase: ADD_CLOSED, registry: Vec::new(), listed_us: 0, cands: Vec::new(), picked: None, bound: None, selected: None, confirm: false, words: alloc::string::String::new(), note: alloc::string::String::new() }
    }
}

/// A sibling's departure waiting for this device's person to approve it (`depart_req`, checked on receipt): the leaving device, its consent stamp and eggs, its intent, its words' commitment, and an approval asked for (the words matched; the loop countersigns).
struct DepartAsk {
    leaver: [u8; 32],
    t: i64,
    sig: Vec<u8>,
    intent: u8,
    /// The request's salt, which a Deny names.
    salt: [u8; 32],
    approve: bool,
    /// What happened to the approval (0 waiting, 1 approving, 2 approved, 3 failed).
    result: u8,
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
    /// The fleet's members as the last chain this client adopted folds them (the Fleet page's list, op 6): device keys, this one among them.
    members: Vec<[u8; 32]>,
    /// The chain's ops said on the console so far (`note_members`): its whole history the first time this run adopts one, then each op added after.
    chain_seen: usize,
    pt: Pt,
    sponsors: Vec<Sponsor>,
    /// This device's current binding request and its stamp (re-minted every 210 s while unbound, as the app re-posts).
    request_t: i64,
    request_frame: Vec<u8>,
    /// Whether the tap secret and the beacon are out (`radios`).
    radios_on: bool,
    /// The secret and beacon id last handed out, a handover that missed a driver, and when it was tried (`radios`).
    radios_last: Option<([u8; 32], [u8; 16])>,
    radios_missed: bool,
    radios_at: u64,
    /// A departure in progress (op 5): its stamp, the approval words this screen shows, the signed `depart_req`, and when the chain was last checked for our absence.
    depart: Option<Depart>,
    /// Whether IPv4 has been up (the first lease triggers a look-up), and the look-up ticks spent with no network while looking.
    net_up: bool,
    offline_looks: u8,
    last_beacon_us: u64,
    last_ping_us: u64,
    last_words_us: u64,
    /// Unlock at boot (device.unlock_at_boot, ferros_sys::PREF_UNLOCK_AT_BOOT): when on, the seed, proof, chain and fleet key are kept in the vault so the device unlocks without the handle being typed; when off (the default), nothing of the identity touches the disk and it is all derived again from the handle and the ira (owner, 2026-10-28: "sand on the beach with no keys anywhere"). Read at start and whenever roots are set.
    persist: bool,
    /// Where this device stands with the identity (V_*), and when the seed was last asked.
    verdict: u8,
    last_lookup_us: u64,
    /// When fgtw.org last answered a fetch (`now_us`), 0 never: the Fleet page says how long ago (2026-11-08: offline it said nothing).
    seed_heard_us: u64,
    /// When the seed's chain was last read for this device's Add while joining (`join_check`).
    join_checked_us: u64,
    /// The seed's address, once resolved.
    seed_ip: Option<[u8; 4]>,
    /// A sponsor named at spawn (its address; the key is learnt from its frames), handed the request directly.
    given: Option<Addr>,
    given_asked_t: i64,
    /// The departure stamp the named sponsor was handed last (0 = none yet).
    given_depart_t: i64,
    /// The chains and fan-outs this device holds (fgtw::held, the rule Photon's held set follows): the seed's copy when it answers, this one when it does not.
    held: core::cell::RefCell<fgtw::held::HeldChains>,
    held_fan: core::cell::RefCell<fgtw::held::HeldFanouts>,
    /// Sponsoring: the add flow, the binding requests handed over the LAN (checked, the newest per device) and where each came from and when, and the beacon ids the Bluetooth driver heard.
    adding: Adding,
    lan_reqs: Vec<fgtw::fleet::BindRequest>,
    lan_from: Vec<([u8; 32], Addr, u64)>,
    ble_heard: Vec<([u8; fgtw::pair::BEACON_UUID_LEN], u64)>,
    /// Whether the radios are listening for newcomers (the BLE scan, the NFC reader) while the add flow is open (`sponsor_radios`).
    listening: bool,
    /// Loop passes since the radios were last told to listen: told again every 64 while the add flow is open, so a driver restarted meanwhile listens again (both answer a repeat with nothing changed). And what they last answered, so only a change is said.
    listen_pass: u32,
    listen_said: (u8, u8),
    /// The beacon ids heard and the candidates heard by Bluetooth at the last look, so only a change is said.
    ble_said: (usize, usize),
    /// A sibling's departure waiting for approval here.
    depart_ask: Option<DepartAsk>,
    /// Departure requests denied here or by another fleet device: (the leaving device, the request's salt, its consent stamp), kept in the vault (`DENIED_KEY`), so the same request is never asked or approved again; the newest 32.
    denied: Vec<([u8; 32], [u8; 32], i64)>,
    /// This device's own departure denied: the device that denied it, shown on the Fleet page until the person asks again.
    depart_denied: Option<[u8; 32]>,
    /// The members of the latest chain for our handle handed to this device before it was bound (a sponsor pushes it ahead of its `bind_select`): what a select's sender is checked against.
    offered: Vec<[u8; 32]>,
    /// A `bind_select` for this device that came before any chain its sender is in (the frames crossed): its sender, checked again when a chain arrives.
    select_waiting: Option<[u8; 32]>,
    /// The member that selected this device, once its select checked out: the green screen until the join completes.
    selected_by: Option<[u8; 32]>,
    /// Words wanted from the person (op 15, the Fleet page): 0 none, 1 a newcomer's pairing words (the last resort when a tap and BLE fail; a departure takes no words since 2026-11-10). The keyboard's host (the attest screen, ahau) asks for it (op 16), sends what is typed (ops 11, 14), and closes when the words match or the person cancels (op 15 [0]).
    entry: u8,
    /// The person's identity space (space.rs, docs/IDENTITY-SPACE.md): their kists, answers and home screen, kept here and synced with their fleet.
    space: space::SpaceSync,
    /// The doorbell (op 21; owner, 2026-11-10: "No polling! ... It should be push or doorbell"): callers waiting for a change (their reply tokens), the change count they are answered with, and what they watch (`watch_state`) when it was last counted.
    watchers: Vec<[u8; 32]>,
    watch_gen: u32,
    watch_seen: Vec<u8>,
    /// When this lookup asked FGTW (the person's devices on this network and the fleet peers known) before fgtw.org (`ask_fgtw`); 0 not yet. The owner, 2026-11-10: "are we consulting the FGTW first or fgtw.org?"
    fgtw_asked_us: u64,
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
        let mut held: Option<Vec<u8>> = None;
        if let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) {
            // Read whole, piece by piece, exactly its own size (one transfer holds at most 4096 bytes, and a fleet chain grows past that; a 128 KiB buffer asked of a 160 KiB heap that has been running a while need not find one free run).
            if let Some(mut chain) = self.store_get_whole(b"chain") {
                let n = chain.len();
                if n > 0 {
                    if let Ok((blob, bound)) = join::adopt_chain(&hp, &seed, &self.me, &chain[..n]) {
                        self.bound = bound;
                        self.note_members(&blob);
                        self.log.put(b"fgtw: chain held, ");
                        self.log.dec(blob.ops.len() as u64);
                        self.log.put(if bound { b" op(s), this device is a member" } else { b" op(s), this device is not in it" });
                        self.log.line();
                        chain.truncate(n);
                        held = Some(chain);
                    }
                }
            }
        }
        // A departure that was under way when the device last stopped: resumed, and finished at once if the chain held here already lacks this device. The requests denied before, so none is asked again.
        self.load_denied();
        self.resume_depart();
        if let Some(c) = held {
            self.depart_check(&c);
        }
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
        // Leaving (a departure resumed before the chain is adopted again): the approval words are what this device shows, never the pairing words.
        if self.depart.is_some() {
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
        // Leaving: the departure request to the named sponsor too, once per stamp (a departure resumed after a restart reaches it here).
        self.depart_hand_given();
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
                self.sponsors.push(Sponsor { key, addr, depart_t: 0, asked_t: 0, heard_us: t, online: false });
                // A device beaconing under our handle: the identity has a fleet, whatever the seed said.
                if matches!(self.verdict, V_LOOKING | V_FRESH | V_UNREACHABLE) {
                    self.verdict = V_JOIN;
                }
                self.sponsors.len() - 1
            }
        };
        self.depart_hand(idx);
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
            self.persist_set(b"fleet_key", &k);
            self.log.put(b"fgtw: the fleet key recovered from the published fan-out");
            self.log.line();
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
                self.persist_set(b"chain", chain);
                self.became_bound();
                self.bound
            }
            _ => false,
        }
    }


    /// Keep the members a chain folds to (the Fleet page's list).
    fn note_members(&mut self, blob: &fgtw::fleet::MembershipBlob) {
        if let Ok(m) = blob.fold() {
            self.members = m;
            self.say_chain(blob);
            // The held copy too (fgtw::held): what a sponsor's Add extends when the seed does not answer.
            fgtw::held::ChainHold::hold(&self.held, blob);
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
    /// Op 5: leave the fleet (pairing-v2's bilateral removal, the app's sign-out): this device's consent eggs over the departure bytes, three fresh approval words, and the signed `depart_req` (fgtw::protocol) handed to every sibling heard on this network, and to each one heard later. A surviving device's user types the words shown here and publishes the consented Remove; once the chain no longer holds this device, the identity is forgotten here (`depart_check`). `intent` is 1 (to a new owner: the approver also releases the device, so another fleet can bind it) or 2 (to the desk). Returns 0 started, 1 not a member of a fleet, 2 an intent that is neither, 3 the frame did not build.
    fn leave(&mut self, intent: u8) -> u8 {
        let Some(hp) = self.hp else { return 1 };
        if !self.bound {
            return 1;
        }
        if intent != join::DEPART_NEW_OWNER && intent != join::DEPART_DESK {
            return 2;
        }
        let t = eagle();
        let consent = join::depart_consent(&self.keys, &hp, t);
        // A fresh salt names this request (owner, 2026-11-10: "upon deny another request will have to use a new salt").
        let mut salt = [0u8; 32];
        kernel_entropy(&mut salt);
        let req = join::DepartReq { t, consent_sig: consent, intent, salt };
        let frame = match join::depart_req(&req, &self.keys, t) {
            Ok(f) => f,
            Err(e) => {
                self.log.put(b"fgtw: the departure request did not build: ");
                self.log.put(e.as_bytes());
                self.log.line();
                return 3;
            }
        };
        self.log.put(if intent == join::DEPART_NEW_OWNER { b"fgtw: leaving the fleet (to a new owner): asking this identity's devices, Approve or Deny on any of them".as_slice() } else { b"fgtw: leaving the fleet: asking this identity's devices, Approve or Deny on any of them" });
        self.log.line();
        self.depart_denied = None;
        self.depart = Some(Depart { t, salt, frame, checked_us: 0 });
        self.keep_depart();
        let request = self.depart.as_ref().map(|d| d.frame.clone()).unwrap_or_default();
        self.space_put_depart(&request);
        for i in 0..self.sponsors.len() {
            self.depart_hand(i);
        }
        self.depart_hand_given();
        0
    }

    /// Op 20: the departure under way given up: 0, or 1 when there is none.
    fn give_up_depart(&mut self) -> u8 {
        if self.depart.take().is_none() {
            return 1;
        }
        let _ = ferros_sys::store_delete(cap(self.log.grants, S_STORE), DEPART_KEY);
        let me = self.me;
        self.space_drop_depart(&me);
        self.log.put(b"fgtw: the departure given up: this device stays in the fleet");
        self.log.line();
        0
    }

    /// The departure under way into the vault (`DEPART_KEY`), so a reboot before a device of the fleet approves resumes it with the same request (the same salt).
    /// Kept whether or not unlock-at-boot is on: it holds no root, and without it a reboot would leave the fleet's devices asking about a request this device had forgotten.
    fn keep_depart(&mut self) {
        let Some(d) = &self.depart else { return };
        let mut section = vsf::VsfSection::new("depart");
        section.add_field("rq", vsf::VsfType::v(b'b', d.frame.clone()));
        let kept = match vsf::VsfBuilder::new().creation_time_oscillations(eagle()).add_section_direct(section).build() {
            Ok(doc) => self.store_set_whole(DEPART_KEY, &doc),
            Err(_) => false,
        };
        if !kept {
            self.log.put(b"fgtw: the departure was not kept in the vault: a reboot before it is approved drops it");
            self.log.line();
        }
    }

    /// A departure kept in the vault (`keep_depart`), taken up again once the roots are in hand: its request and words as they were, handed to siblings as they are heard and checked against the chain as `leave` leaves them.
    /// Taken up only for the identity it leaves: the request must be this device's, and its consent's Ed25519 egg must verify over these roots' departure bytes (`fleet::departreq_signing_bytes`); a departure from another identity stays in the vault untouched.
    fn resume_depart(&mut self) {
        let Some(hp) = self.hp else { return };
        if self.depart.is_some() || self.identity_seed.is_none() {
            return;
        }
        let Some(doc) = self.store_get_whole(DEPART_KEY) else { return };
        let Ok((header, end)) = vsf::verification::read_verified(&doc, None) else {
            self.log.put(b"fgtw: a kept departure does not verify: left as it is");
            self.log.line();
            return;
        };
        let Ok(section) = header.primary_section(&doc, end) else { return };
        let Some(frame) = section.get_field("rq").and_then(|f| f.values.first()).and_then(|v| v.as_bytes()).map(|b| b.to_vec()) else { return };
        // A request kept by a build from before 2026-11-10 (words, no salt) does not parse: dropped, and the person asks again.
        let Ok((req, leaver)) = fgtw::protocol::parse_depart_req(&frame) else {
            let _ = ferros_sys::store_delete(cap(self.log.grants, S_STORE), DEPART_KEY);
            return;
        };
        let msg = fgtw::fleet::departreq_signing_bytes(&hp, &self.me, req.t);
        let ours = leaver == self.me
            && fgtw::pq::eggs_from_bytes(&req.consent_sig).is_ok_and(|eggs| eggs.iter().any(|e| e.scheme == fgtw::fleet::scheme::ED25519 && fgtw::pq::verify_egg(e.scheme, &self.me, &msg, &e.sig)));
        if !ours {
            return;
        }
        self.log.put(b"fgtw: resuming the departure kept from before the restart: asking this identity's devices again");
        self.log.line();
        self.depart = Some(Depart { t: req.t, salt: req.salt, frame, checked_us: 0 });
        let request = self.depart.as_ref().map(|d| d.frame.clone()).unwrap_or_default();
        self.space_put_depart(&request);
        for i in 0..self.sponsors.len() {
            self.depart_hand(i);
        }
    }

    /// Hand the departure request to the named sponsor (spawn input or `fgtw.sponsor`, the address the binding request goes to directly), once per request: on a network whose access point passes no multicast between its wireless clients no sibling is ever heard beaconing, and the named sponsor is the one device of the fleet this device can reach.
    fn depart_hand_given(&mut self) {
        let (Some(d), Some(to)) = (&self.depart, self.given) else { return };
        if self.given_depart_t == d.t {
            return;
        }
        self.given_depart_t = d.t;
        let frame = d.frame.clone();
        self.log.put(b"fgtw: handing our departure request to the named sponsor, ");
        self.log.addr(&to);
        self.log.line();
        let out = self.pt.send(to, frame, now_us(), eagle());
        self.send_all(out);
    }

    /// Hand the departure request to sibling `i`, once per request.
    fn depart_hand(&mut self, i: usize) {
        let Some(d) = &self.depart else { return };
        if self.sponsors[i].depart_t == d.t {
            return;
        }
        self.sponsors[i].depart_t = d.t;
        let (frame, to, key) = (d.frame.clone(), self.sponsors[i].addr, self.sponsors[i].key);
        self.log.put(b"fgtw: handing our departure request to ");
        self.log.fp(&key);
        self.log.line();
        let out = self.pt.send(to, frame, now_us(), eagle());
        self.send_all(out);
    }

    /// A chain read while leaving: once it folds without this device, the departure is complete — the identity's roots, chain and key are forgotten here, on disk and in RAM.
    fn depart_check(&mut self, chain: &[u8]) {
        if self.depart.is_none() {
            return;
        }
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
        if let Ok((_, false)) = join::adopt_chain(&hp, &seed, &self.me, chain) {
            self.log.put(b"fgtw: the fleet's chain no longer holds this device: departed; the identity is forgotten here");
            self.log.line();
            self.depart = None;
            let _ = ferros_sys::store_delete(cap(self.log.grants, S_STORE), DEPART_KEY);
            self.forget_disk();
            self.forget_identity();
            if !ferros_sys::release_identity(cap(unsafe { GRANTS }, S_UNLOCK)) {
                self.log.put(b"fgtw: the kernel did not release the device's identity space");
                self.log.line();
            }
        }
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
    fn forget_identity(&mut self) {
        self.depart = None;
        if self.radios_on {
            self.radios(None);
        }
        self.identity_seed = None;
        self.hp = None;
        self.bound = false;
        self.fleet_key = None;
        self.members.clear();
        self.offered.clear();
        self.select_waiting = None;
        self.selected_by = None;
        self.sponsors.clear();
        self.request_t = 0;
        self.request_frame.clear();
        self.verdict = V_NONE;
        self.space = space::SpaceSync::new();
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
                        self.persist_set(b"fleet_key", &k);
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
                }
                return;
            }
            Some(Frame::Pong { key, .. }) => {
                if let Some(s) = self.sponsors.iter_mut().find(|s| s.key == key) {
                    s.heard_us = t;
                    if !s.online {
                        s.online = true;
                        self.log.put(b"fgtw: fleet device ");
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
                        self.persist_set(b"chain", &chain);
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
                        self.depart_check(&chain);
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
                            self.persist_set(b"fleet_key", &key);
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
            Ok((Inbound::Other(name), _)) if name == "bind_req" => self.on_bind_req(from, bytes),
            Ok((Inbound::Other(name), _)) if name == "depart_req" => self.on_depart_req(bytes),
            Ok((Inbound::Other(name), _)) if name == "depart_deny" => self.on_depart_deny(bytes),
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
        if self.bound {
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
        self.read_persist();
        if !self.persist {
            self.forget_disk();
        }
        self.load();
        if self.hp.is_some() {
            self.verdict = V_LOOKING;
        }
        self.show_words();
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
                                self.log.put(b"fgtw: a frame to ");
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
            // The attest screen: the state, or its roots (op 4, 65 bytes).
            // On the stack: a syscall's buffers must be in the arc. 512 bytes hold the longest words entry (23 voca words, at most 18 letters each, with spaces).
            let mut req = [0u8; 512];
            let mut tok = [0u8; 32];
            let n = ferros_sys::recv(cap(self.log.grants, S_SERVE), &mut req, &mut tok, true);
            if n != ERR && n != ferros_sys::NOTHING {
                busy = true;
                // WHY the min: `n` is what the caller sent, which may be more than the buffer took.
                let got = (n as usize).min(req.len());
                // The sponsor's and the approver's ops (the Fleet page's add flow and approvals, fgtw::sponsor): each answers alone, nothing of the state after it. 8 [on] open or close the add flow → [0]; 9 → the add flow (`adding_reply`); 10 [key 8] pick a nearby candidate → [result]; 11 [words] the words typed so far → the match (`type_words`); 12 confirm a nearby pick → [result]; 13 → a departure waiting (`depart_ask_reply`); 14 [words] approve it → [result]; 15 [kind] words wanted from the person (0 none, 1 a newcomer's, 2 a leaver's) → [0]; 16 → the words wanted now [kind] (the keyboard's host asks).
                // The identity space (space.rs): 17 → its state for the map (`space_reply`: the generation the map reloads its circles on, this device's home-screen sync, the first pull done, the facts); 18 [on] this device's home screen follows the fleet's or keeps its own → [0 done, 1 not yet: before the first pull].
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
                if n >= 1 && (8..=16).contains(&req[0]) {
                    let text = core::str::from_utf8(&req[1..got]).unwrap_or("");
                    let r: Vec<u8> = match req[0] {
                        8 if n >= 2 => {
                            self.open_add(req[1] != 0);
                            alloc::vec![0]
                        }
                        9 => self.adding_reply(),
                        10 if n >= 9 => {
                            let mut k = [0u8; 8];
                            k.copy_from_slice(&req[1..9]);
                            alloc::vec![self.pick_nearby(&k)]
                        }
                        11 => self.type_words(text),
                        12 => alloc::vec![self.confirm_add()],
                        13 => self.depart_ask_reply(),
                        14 if n >= 2 => alloc::vec![self.answer_depart(req[1] != 0)],
                        15 if n >= 2 && req[1] <= 1 => {
                            self.entry = req[1];
                            alloc::vec![0]
                        }
                        16 => alloc::vec![self.entry_wanted()],
                        _ => alloc::vec![ERR as u8],
                    };
                    svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, r.as_ptr() as u64 - self.udp.base, r.len() as u64, 0, 0);
                    continue;
                }
                let mut reply = Vec::with_capacity(400);
                // Op 6: the fleet's members, for the Fleet page (`members_reply`): its own reply, nothing of the state after it.
                // Op 7: how this device reaches its fleet, for the Fleet page: [the network up (IPv4) u8][seconds since fgtw.org last answered, u32 LE, all ones for never][fleet devices heard on this network u8].
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
                // Op 2: mint (the attest screen, after the permanence warning and the retype) → [result: 0 minted, else why not], then the state as op 1 gives it.
                if n >= 1 && req[0] == 2 {
                    let r = self.mint();
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
                    self.forget_identity();
                    self.read_persist();
                    if !self.persist {
                        self.forget_disk();
                    }
                    self.persist_set(b"identity_seed", &seed);
                    self.persist_set(b"handle_proof", &hp);
                    self.identity_seed = Some(seed);
                    self.hp = Some(hp);
                    self.verdict = V_LOOKING;
                    self.last_lookup_us = 0;
                    self.fgtw_asked_us = 0;
                    self.log.put(if self.persist { b"fgtw: roots set in RAM and kept on disk (unlock at boot on)" } else { b"fgtw: roots set in RAM only (nothing of the identity on disk)" });
                    self.log.line();
                    // A departure kept from before a restart: these roots are the first chance to take it up (with unlock-at-boot off, nothing of the identity is on disk at `load`).
                    self.resume_depart();
                }
                // Op 5: leave the fleet, `[5][intent]` (1 to a new owner, 2 to the desk) → [result: 0 started, else why not], then the state (8 leaving, with the approval words).
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
                let state = if self.depart.is_some() { V_LEAVING } else { state };
                // Selected by a member and not bound yet: the green screen, the selecting device's name after the state.
                let state = if !self.bound && self.depart.is_none() && self.selected_by.is_some() { V_SELECTED } else { state };
                reply.push(state);
                reply.push(self.sponsors.len().min(255) as u8);
                if let (Some(seed), V_SELECTED, Some(by)) = (self.identity_seed, state, self.selected_by) {
                    reply.extend_from_slice(fgtw::pair::device_name_default(&by, &seed).as_bytes());
                }
                // Joining's pairing words follow the state, the last resort when a tap and BLE fail; a departure shows none (owner, 2026-11-10), even one resumed before the chain is adopted again.
                if let (Some(seed), false, V_JOIN, None, None) = (self.identity_seed, self.bound, self.verdict, &self.depart, self.selected_by) {
                    reply.extend_from_slice(join::pair_words(&self.me, &seed).as_bytes());
                }
                svc(ferros_sys::nr::REPLY, tok.as_ptr() as u64 - self.udp.base, reply.as_ptr() as u64 - self.udp.base, reply.len() as u64, 0, 0);
            }
            // Joining: the seed's copy of the chain every 16 s, for the sponsor's Add. A sponsor hands the chain and the fan-out to a newcomer at the address its request came from on the LAN; one it knows only from the registry or the tap has no address, and the newcomer has to look (2026-11-04: Brittany's phone bound this device by the tap and the words stayed up). The clock only moves forward, so `t` is never behind the last check.
            if !self.bound && self.depart.is_none() && self.verdict == V_JOIN && t - self.join_checked_us >= 16_000_000 {
                self.join_checked_us = t;
                self.join_check();
            }
            // Leaving: the seed's copy of the chain every 16 s, for the approver's consented Remove (a sibling's push may never reach a device it no longer counts as a sibling). The clock only moves forward, so `t` is never behind the last check.
            if self.depart.as_ref().is_some_and(|d| t - d.checked_us >= 16_000_000) {
                if let Some(d) = self.depart.as_mut() {
                    d.checked_us = t;
                }
                if let Some(hp) = self.hp {
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
                            self.depart_check(&bytes);
                        }
                    }
                }
            }
            // Sponsoring and approving (fgtw::sponsor): the add flow's listing, binding and growing, the radios listening for newcomers, an approved departure.
            self.sponsor_step(t);
            // The person's identity space: the first pull, then this device's copy joined to it and handed on (space.rs).
            self.space_step(t);
            // The verdict: looked up once the network is up, and again every 30 s while GREY or while a fresh handle waits (someone else may mint it meanwhile).
            // The client serves from the start, network or not (the attest screen waits on it); the network coming up is a reason to look up at once, not 30 s later.
            let up = self.udp.flags().is_some_and(|f| f & 1 != 0);
            if up && !self.net_up {
                self.net_up = true;
                self.last_lookup_us = 0;
                self.fgtw_asked_us = 0;
                self.log.put(b"fgtw: the network is up (IPv4)");
                self.log.line();
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
            // The binding request only once there is a fleet to ask (the verdict says joining): minting it signs in three families, SLH-DSA-128s among them, which takes seconds; it is done here in the loop, never inside a call another process waits on.
            if !self.bound && self.depart.is_none() && self.hp.is_some() && self.verdict == V_JOIN && self.request_frame.is_empty() {
                self.mint_request();
            }
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
            if !busy && !self.pt.busy() {
                ferros_sys::sleep_until(now() + ferros_sys::freq() / 50);
            } else {
                ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
            }
        }
    }

    /// What the Fleet page and the map read, in one piece: the state (op 1's), the members (op 6), a departure waiting (op 13), the add flow (op 9), this device's own departure denied, and the space (op 17).
    fn watch_state(&mut self) -> Vec<u8> {
        let mut v = alloc::vec![self.hp.is_some() as u8, self.bound as u8, self.fleet_key.is_some() as u8, self.depart.is_some() as u8, self.verdict, self.sponsors.len() as u8];
        // The select too: the attest screen turns green on it.
        v.extend_from_slice(self.selected_by.as_ref().map_or(&[0u8; 32][..], |s| &s[..]));
        v.extend_from_slice(&self.members_reply());
        v.extend_from_slice(&self.depart_ask_reply());
        v.extend_from_slice(&self.adding_reply());
        v.extend_from_slice(self.depart_denied.as_ref().map_or(&[0u8; 32][..], |d| &d[..]));
        v.extend_from_slice(&self.space_reply());
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


/// The sponsor's and the approver's side (fgtw::sponsor, the code Photon's Devices screen runs): adding another device to this fleet, and approving a sibling's departure. Everything that signs or reaches the seed runs in the loop (`sponsor_step`), never inside a call another process waits on; the Fleet page's ops only record what the person asked.
impl Client {
    /// The candidate rows again, from the registry's last list and the LAN's requests, each marked nearby when its request came over the LAN or its beacon was heard within `NEARBY_US`.
    fn rebuild_candidates(&mut self) {
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
        fgtw::sponsor::retain_fresh(&mut self.lan_reqs, eagle());
        let t = now_us();
        // The clock only moves forward, so `t` is never behind a stamp taken earlier.
        self.ble_heard.retain(|(_, at)| t - at < NEARBY_US);
        let reqs = fgtw::sponsor::merge_requests(&self.adding.registry, &self.lan_reqs);
        let ble: Vec<[u8; fgtw::pair::BEACON_UUID_LEN]> = self.ble_heard.iter().map(|(u, _)| *u).collect();
        let lan: Vec<[u8; 32]> = self.lan_from.iter().filter(|(_, _, at)| t - at < NEARBY_US).map(|(k, _, _)| *k).collect();
        self.adding.cands = fgtw::sponsor::candidates(reqs, &seed, &hp, &ble, &lan);
        // Measuring the nearby path: how many beacon ids Bluetooth hears, and how many devices asking match one (fgtw::pair::beacon_matches), said when either changes.
        let matched = self.adding.cands.iter().filter(|c| c.heard_ble).count();
        if (ble.len(), matched) != self.ble_said {
            self.ble_said = (ble.len(), matched);
            self.log.put(b"fgtw: Bluetooth hears ");
            self.log.dec(ble.len() as u64);
            self.log.put(b" service id(s); ");
            self.log.dec(matched as u64);
            self.log.put(b" of the ");
            self.log.dec(self.adding.cands.len() as u64);
            self.log.put(b" device(s) asking to join match one");
            self.log.line();
        }
        // A device asking is known only from fgtw.org's registry or a request on this network: a beacon alone names no request to match. With neither, the page says so.
        if self.adding.phase == ADD_OPEN {
            self.adding.note.clear();
            if self.adding.cands.is_empty() {
                self.adding.note.push_str(if self.net_up { "waiting for a device to ask (fgtw.org, this network)" } else { "no network: a device asking is found through fgtw.org (cellular or WiFi)" });
            }
        }
    }

    /// A newcomer's binding request handed to this device over the LAN (`bind_req`): taken only by a member of the fleet it names, checked exactly as a registry row is (fgtw::sponsor::check_lan_request), kept as the newest from its device, and its address remembered for the hand-over.
    fn on_bind_req(&mut self, from: Addr, bytes: &[u8]) {
        let Ok((hp, req, _)) = fgtw::protocol::parse_bind_req(bytes) else { return };
        let (Some(our_hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
        if hp != our_hp || !self.bound {
            return;
        }
        let key = req.device_pubkey;
        match fgtw::sponsor::check_lan_request(&req, &hp, &seed, eagle()) {
            Ok(()) => {}
            Err(why) => {
                self.log.put(b"fgtw: a binding request from ");
                self.log.fp(&key);
                self.log.put(if why == fgtw::sponsor::LanRefusal::Stale { b" is stale: ignored".as_slice() } else { b" does not verify for our fleet: ignored" });
                self.log.line();
                return;
            }
        }
        if fgtw::sponsor::offer_lan(&mut self.lan_reqs, req) == fgtw::sponsor::Offered::Unchanged {
            return;
        }
        let t = now_us();
        match self.lan_from.iter_mut().find(|(k, _, _)| *k == key) {
            Some(e) => {
                e.1 = from;
                e.2 = t;
            }
            None => {
                self.log.put(b"fgtw: ");
                self.log.put(fgtw::pair::device_name_default(&key, &seed).as_bytes());
                self.log.put(b" (");
                self.log.fp(&key);
                self.log.put(b") asks to join from ");
                self.log.addr(&from);
                self.log.line();
                self.lan_from.push((key, from, t));
            }
        }
        self.rebuild_candidates();
    }

    /// Op 8: the Fleet page's add flow opened (1) or closed (0). Opening lists the registry at once and starts the radios listening; closing stops them, and forgets a finished or failed add.
    fn open_add(&mut self, on: bool) {
        // Back from a selected device: the list again, nothing to undo (nothing was bound).
        if on && self.adding.phase == ADD_SELECTED {
            self.adding.selected = None;
            self.adding.note.clear();
            self.adding.phase = ADD_OPEN;
            return;
        }
        if on && self.adding.phase == ADD_CLOSED {
            self.adding.phase = ADD_OPEN;
            self.adding.listed_us = 0;
            self.adding.words.clear();
            self.adding.note.clear();
            self.adding.picked = None;
            self.adding.bound = None;
            self.adding.confirm = false;
            self.rebuild_candidates();
            self.log.put(b"fgtw: the add flow is open: listing devices asking to join");
            self.log.line();
        } else if !on && self.adding.phase != ADD_CLOSED {
            // A bind or a grow under way finishes in the loop; only the screen closes.
            if matches!(self.adding.phase, ADD_OPEN | ADD_DONE | ADD_FAILED | ADD_SELECTED) {
                self.adding.phase = ADD_CLOSED;
            }
            self.adding.registry.clear();
        }
    }

    /// Op 10: a candidate heard nearby, picked by the first 8 bytes of its key: selected, nothing bound (`select`); Add (op 12) binds it. Returns 0 selected, 1 no such nearby candidate, 2 busy.
    fn pick_nearby(&mut self, key8: &[u8]) -> u8 {
        if self.adding.phase != ADD_OPEN {
            return 2;
        }
        let Some(c) = self.adding.cands.iter().find(|c| c.nearby() && c.req.device_pubkey[..8] == *key8) else { return 1 };
        let req = c.req.clone();
        self.select(req, "its row on the Fleet page");
        0
    }

    /// Op 11: the words typed so far, matched live (fgtw::sponsor::match_words). A full match picks the candidate, and its key follows at once (the typed 256-bit match is the confirmation). Returns [kind][count], then the word to flag or the matched name: kind 0 no candidates yet, 1 nothing typed, 2 matched, 3 no device has this word, 4 one device still matches, 5 several do, 6 the add flow is not open for words.
    fn type_words(&mut self, text: &str) -> Vec<u8> {
        use fgtw::sponsor::WordMatch;
        self.adding.words.clear();
        self.adding.words.push_str(text);
        let mut r = Vec::with_capacity(48);
        if self.adding.phase != ADD_OPEN {
            r.extend_from_slice(&[6, 0]);
            return r;
        }
        match fgtw::sponsor::match_words(text, &self.adding.cands) {
            WordMatch::NoCandidates { typo } => {
                r.extend_from_slice(&[0, 0]);
                if let Some(w) = typo {
                    r.extend_from_slice(w.as_bytes());
                }
            }
            // WHY the cap: a byte on the wire; past 255 devices asking the page has nothing different to say.
            WordMatch::Nothing { asking } => r.extend_from_slice(&[1, asking.min(255) as u8]),
            WordMatch::Full(i) => {
                let c = &self.adding.cands[i];
                r.extend_from_slice(&[2, 1]);
                r.extend_from_slice(c.name.as_bytes());
                self.adding.picked = Some((c.req.clone(), "its words matched"));
                self.adding.phase = ADD_BINDING;
            }
            WordMatch::NoMatch(w) => {
                r.extend_from_slice(&[3, 0]);
                r.extend_from_slice(w.as_bytes());
            }
            WordMatch::One(i) => {
                r.extend_from_slice(&[4, 1]);
                r.extend_from_slice(self.adding.cands[i].name.as_bytes());
            }
            // WHY the cap: as above.
            WordMatch::Many(k) => r.extend_from_slice(&[5, k.min(255) as u8]),
        }
        r
    }

    /// Op 12: Add, on a selected device the person saw turn green: the bind and the fan-out grow as one act (`bind_picked`, then `grow_for_newcomer`), the chain and the fan-out pushed to it. Returns 0 taken, 1 nothing selected.
    fn confirm_add(&mut self) -> u8 {
        if self.adding.phase != ADD_SELECTED {
            return 1;
        }
        let Some(req) = self.adding.selected.take() else { return 1 };
        self.adding.picked = Some((req, "Add pressed on the selected device"));
        self.adding.phase = ADD_BINDING;
        0
    }

    /// Op 16: the words wanted from the person now, dropped once nothing is left to type them for (the add flow past picking, the departure approved or gone).
    fn entry_wanted(&mut self) -> u8 {
        if self.entry == 1 && self.adding.phase != ADD_OPEN {
            self.entry = 0;
        }
        if self.entry == 2 && !self.depart_ask.as_ref().is_some_and(|d| d.result == 0 || d.result == 3) {
            self.entry = 0;
        }
        self.entry
    }

    /// Op 9, the add flow for the Fleet page: [phase][rows], each row [the key's first 8 bytes][flags: bit 0 its beacon heard, bit 1 its request heard on the LAN, bit 2 the device just bound][name length][name], then [note length][note].
    fn adding_reply(&self) -> Vec<u8> {
        let mut r = Vec::with_capacity(256);
        r.push(self.adding.phase);
        // WHY the cap: the Fleet page lays out at most 8 candidates, and the count is one byte.
        let n = self.adding.cands.len().min(8);
        r.push(n as u8);
        for c in &self.adding.cands[..n] {
            r.extend_from_slice(&c.req.device_pubkey[..8]);
            r.push(c.heard_ble as u8 | (c.heard_lan as u8) << 1 | ((self.adding.bound == Some(c.req.device_pubkey)) as u8) << 2);
            r.push(c.name.len() as u8);
            r.extend_from_slice(c.name.as_bytes());
        }
        // WHY the cap: a length byte; every note written here is a short sentence.
        let note = &self.adding.note.as_bytes()[..self.adding.note.len().min(255)];
        r.push(note.len() as u8);
        r.extend_from_slice(note);
        // The selected device (phase ADD_SELECTED): [its name, length first][its masked words, the same the device shows, length u16 LE], both empty otherwise.
        let (name, words) = match (&self.adding.selected, self.identity_seed) {
            (Some(req), Some(seed)) => (fgtw::pair::device_name_default(&req.device_pubkey, &seed), fgtw::pair::masked_device_words(&req.device_pubkey, &seed)),
            _ => (alloc::string::String::new(), alloc::string::String::new()),
        };
        // WHY the caps: a length byte, and two for the words; a device name is two words and the masked words a few hundred bytes.
        let name = &name.as_bytes()[..name.len().min(255)];
        r.push(name.len() as u8);
        r.extend_from_slice(name);
        let words = &words.as_bytes()[..words.len().min(0xFFFF)];
        r.extend_from_slice(&(words.len() as u16).to_le_bytes());
        r.extend_from_slice(words);
        r
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

    /// The newcomer's address on this network, if its request came over the LAN.
    fn newcomer_addr(&self, key: &[u8; 32]) -> Option<Addr> {
        self.lan_from.iter().find(|(k, _, _)| k == key).map(|(_, a, _)| *a)
    }

    fn add_failed(&mut self, why: &str) {
        self.log.put(b"fgtw: adding a device failed: ");
        self.log.put(why.as_bytes());
        self.log.line();
        self.adding.note.clear();
        self.adding.note.push_str(why);
        self.adding.phase = ADD_FAILED;
    }

    /// The Add for the picked request (fgtw::sponsor::bind_device_held), signed here on the X3: appended to the chain held (refreshed from the seed when it answers), held, mirrored to the seed, kept, and handed to the fleet and the newcomer. A words or tap pick grows the fan-out next; a nearby pick waits for the confirm.
    fn bind_picked(&mut self) {
        let Some((req, how)) = self.adding.picked.take() else { return };
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
        let key = req.device_pubkey;
        if fgtw::sponsor::plate_locked(&self.held, &hp, &req) {
            self.add_failed("this is a device you locked out");
            return;
        }
        self.log.put(b"fgtw: adding ");
        self.log.put(fgtw::pair::device_name_default(&key, &seed).as_bytes());
        self.log.put(b" (");
        self.log.fp(&key);
        self.log.put(b") to the fleet: ");
        self.log.put(how.as_bytes());
        self.log.line();
        let t0 = now();
        ferros_sys::pin(ferros_sys::CORES_BIG);
        ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
        fgtw::set_clock(eagle());
        let r = {
            let s = self.seed();
            let r = fgtw::sponsor::bind_device_held(&s, &self.held, &self.keys, &hp, &req);
            self.seed_ip = s.ip.get();
            r
        };
        ferros_sys::pin(0);
        let blob = match r {
            Ok(b) => b,
            Err(e) => {
                self.add_failed(&e);
                return;
            }
        };
        let Ok(bytes) = blob.to_vsf_bytes() else {
            self.add_failed("the extended chain did not encode");
            return;
        };
        self.log.put(b"fgtw: the Add is in the chain (");
        self.log.dec(blob.ops.len() as u64);
        self.log.put(b" ops) in ");
        self.log.dec(ferros_sys::us_since(t0) / 1000);
        self.log.put(b" ms: handing it to the fleet and the newcomer");
        self.log.line();
        self.note_members(&blob);
        self.persist_set(b"chain", &bytes);
        self.adding.bound = Some(key);
        match fgtw::protocol::fleet_resp(&hp, &bytes, &self.keys, eagle()) {
            Ok(frame) => {
                let to = self.newcomer_addr(&key);
                self.push_to_fleet(frame, to);
            }
            Err(e) => {
                self.log.put(b"fgtw: the chain frame did not build: ");
                self.log.put(e.as_bytes());
                self.log.line();
            }
        }
        // The key follows at once: the words matched, or the person pressed Add on a selected device (one act since 2026-11-10; a pick no longer binds).
        self.adding.note.clear();
        self.adding.confirm = true;
        self.adding.phase = ADD_GROWING;
    }

    /// A device selected (the Fleet page's button on a nearby row; an NFC tap adds at once instead): nothing is written to the chain. The fleet's chain and a `bind_select` go to it over this network (it turns green on the select, checking our key against the chain), and the page shows Add, which binds and grows as one act. With no address for it on this network it is told nothing (ferros has no relay): it stays dark, and the person presses Back and types its words.
    fn select(&mut self, req: fgtw::fleet::BindRequest, by: &str) {
        let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) else { return };
        let key = req.device_pubkey;
        if fgtw::sponsor::plate_locked(&self.held, &hp, &req) {
            self.add_failed("this is a device you locked out");
            return;
        }
        self.log.put(b"fgtw: selected ");
        self.log.put(fgtw::pair::device_name_default(&key, &seed).as_bytes());
        self.log.put(b" (");
        self.log.fp(&key);
        self.log.put(b") by ");
        self.log.put(by.as_bytes());
        self.log.put(b": nothing bound, Add binds it");
        self.log.line();
        self.adding.selected = Some(req);
        self.adding.note.clear();
        self.adding.phase = ADD_SELECTED;
        let Some(to) = self.newcomer_addr(&key) else {
            self.log.put(b"fgtw: the selected device has no address on this network: it is not told (no relay here)");
            self.log.line();
            return;
        };
        let t = eagle();
        // The chain first, so the select's sender is a member of a chain the newcomer already has (it holds a select that comes first until a chain arrives).
        if let Some(chain) = self.store_get_whole(b"chain") {
            if let Ok(frame) = fgtw::protocol::fleet_resp(&hp, &chain, &self.keys, t) {
                let out = self.pt.send(to, frame, now_us(), t);
                self.send_all(out);
            }
        }
        match fgtw::protocol::bind_select(&hp, &key, &self.keys, t) {
            Ok(frame) => {
                let out = self.pt.send(to, frame, now_us(), t);
                self.send_all(out);
            }
            Err(e) => {
                self.log.put(b"fgtw: the select did not build: ");
                self.log.put(e.as_bytes());
                self.log.line();
            }
        }
    }

    /// The newcomer's key: the fan-out grown to the chain's members under the SAME fleet key (fgtw::held::fanout_grow_held), held, mirrored to the seed, and handed to the newcomer and the fleet.
    fn grow_for_newcomer(&mut self) {
        self.adding.confirm = false;
        let (Some(hp), Some(seed), Some(key)) = (self.hp, self.identity_seed, self.fleet_key) else {
            self.add_failed("this device does not hold the fleet key");
            return;
        };
        let members = self.members.clone();
        ferros_sys::pin(ferros_sys::CORES_BIG);
        ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
        fgtw::set_clock(eagle());
        let r = {
            let s = self.seed();
            let r = fgtw::held::fanout_grow_held(&s, &self.held_fan, &hp, &self.keys, &key, &members, &seed);
            self.seed_ip = s.ip.get();
            r
        };
        ferros_sys::pin(0);
        let (revision, blob) = match r {
            Ok(x) => x,
            Err(e) => {
                self.add_failed(&e);
                return;
            }
        };
        self.log.put(b"fgtw: the fan-out grown to revision ");
        self.log.dec(revision);
        self.log.put(b" for ");
        self.log.dec(members.len() as u64);
        self.log.put(b" devices: handing it to the newcomer and the fleet");
        self.log.line();
        match fgtw::protocol::fanout_resp(&hp, &blob, &self.keys, eagle()) {
            Ok(frame) => {
                let to = self.adding.bound.and_then(|k| self.newcomer_addr(&k));
                self.push_to_fleet(frame, to);
            }
            Err(e) => {
                self.log.put(b"fgtw: the fan-out frame did not build: ");
                self.log.put(e.as_bytes());
                self.log.line();
            }
        }
        // The newcomer is a member now: its request is done with.
        if let Some(k) = self.adding.bound {
            self.lan_reqs.retain(|r| r.device_pubkey != k);
            self.adding.registry.retain(|r| r.device_pubkey != k);
            self.rebuild_candidates();
        }
        self.adding.note.clear();
        self.adding.note.push_str("added");
        self.adding.phase = ADD_DONE;
    }

    /// A sibling's departure request (`depart_req`): kept for this device's person to approve when the leaver is a current member other than this device and its consent's Ed25519 egg verifies (fgtw::sponsor::depart_receipt_valid). The countersign happens only on the person's approval with the leaver's words.
    pub fn on_depart_req(&mut self, bytes: &[u8]) {
        let Ok((req, leaver)) = fgtw::protocol::parse_depart_req(bytes) else { return };
        let Some(hp) = self.hp else { return };
        if !self.bound || leaver == self.me || !self.members.contains(&leaver) {
            return;
        }
        if !fgtw::sponsor::depart_receipt_valid(&hp, &leaver, req.t, &req.consent_sig) {
            self.log.put(b"fgtw: a departure request from ");
            self.log.fp(&leaver);
            self.log.put(b" carries an invalid signature: dropped");
            self.log.line();
            return;
        }
        if self.depart_ask.as_ref().is_some_and(|d| d.leaver == leaver && d.t == req.t) {
            return;
        }
        // A request denied once is never asked again, by its salt or by its consent stamp (the same consent under a new salt is the same request).
        if self.denied.iter().any(|&(l, s, t)| l == leaver && (s == req.salt || t == req.t)) {
            return;
        }
        self.log.put(b"fgtw: ");
        if let Some(seed) = self.identity_seed {
            self.log.put(fgtw::pair::device_name_default(&leaver, &seed).as_bytes());
            self.log.put(b" (");
            self.log.fp(&leaver);
            self.log.put(b")");
        }
        self.log.put(b" asks to leave the fleet: Approve or Deny on the Fleet page");
        self.log.line();
        self.depart_ask = Some(DepartAsk { leaver, t: req.t, sig: req.consent_sig, intent: req.intent, salt: req.salt, approve: false, result: 0 });
    }

    /// Op 13, a departure waiting here: [1][intent][result: 0 waiting, 1 approving, 2 approved, 3 failed][name length][name]; or this device's own departure denied: [2][0][0][name length][the name of the device that denied it]; else [0][0][0][0].
    fn depart_ask_reply(&self) -> Vec<u8> {
        let mut r = Vec::with_capacity(48);
        match (&self.depart_ask, self.depart_denied, self.identity_seed) {
            (Some(d), _, Some(seed)) => {
                let name = fgtw::pair::device_name_default(&d.leaver, &seed);
                r.extend_from_slice(&[1, d.intent, d.result, name.len() as u8]);
                r.extend_from_slice(name.as_bytes());
            }
            (None, Some(by), Some(seed)) => {
                let name = fgtw::pair::device_name_default(&by, &seed);
                r.extend_from_slice(&[2, 0, 0, name.len() as u8]);
                r.extend_from_slice(name.as_bytes());
            }
            _ => r.extend_from_slice(&[0, 0, 0, 0]),
        }
        r
    }

    /// Op 14, the person's answer to the departure waiting here (owner, 2026-11-10: a yes or no question): `[14][1]` Approve (the loop countersigns), `[14][0]` Deny. Returns 0 done, 1 no departure waiting.
    fn answer_depart(&mut self, approve: bool) -> u8 {
        let Some(d) = self.depart_ask.as_mut() else { return 1 };
        if approve {
            d.approve = true;
            d.result = 1;
            return 0;
        }
        let (leaver, salt, t) = (d.leaver, d.salt, d.t);
        self.depart_ask = None;
        self.note_denied(leaver, salt, t);
        let Some(hp) = self.hp else { return 0 };
        self.log.put(b"fgtw: the departure of ");
        self.log.fp(&leaver);
        self.log.put(b" denied here: the fleet's devices are told, and that request is never asked again");
        self.log.line();
        // To every fleet device heard (the leaving one among them), and into the space for the rest (the entry's value becomes the deny: fgtw.org carries it to a device away from here).
        if let Ok(frame) = fgtw::protocol::depart_deny(&hp, &leaver, &salt, t, self.keys.keypair(), eagle()) {
            for i in 0..self.sponsors.len() {
                let to = self.sponsors[i].addr;
                let out = self.pt.send(to, frame.clone(), now_us(), eagle());
                self.send_all(out);
            }
            self.space_deny_depart(&leaver, &frame);
        }
        0
    }

    /// A departure denied, remembered so it is never asked again (`denied`, kept in the vault as `DENIED_KEY`).
    fn note_denied(&mut self, leaver: [u8; 32], salt: [u8; 32], t: i64) {
        if self.denied.iter().any(|&(l, s, _)| l == leaver && s == salt) {
            return;
        }
        self.denied.push((leaver, salt, t));
        // WHY the cap: a list kept whole in the vault; the newest 32 cover every request a person could have pending.
        if self.denied.len() > 32 {
            self.denied.remove(0);
        }
        let mut section = vsf::VsfSection::new("denied");
        for &(l, s, t) in &self.denied {
            section.add_field_multi("request", alloc::vec![vsf::VsfType::ke(l.to_vec()), vsf::VsfType::hb(s.to_vec()), vsf::VsfType::e(vsf::types::EtType::e6(t))]);
        }
        if let Ok(doc) = vsf::VsfBuilder::new().creation_time_oscillations(eagle()).add_section_direct(section).build() {
            if !self.store_set_whole(DENIED_KEY, &doc) {
                self.log.put(b"fgtw: the denied request was not kept in the vault");
                self.log.line();
            }
        }
    }

    /// The denied requests kept in the vault (`note_denied`), read once the roots are in hand.
    fn load_denied(&mut self) {
        let Some(doc) = self.store_get_whole(DENIED_KEY) else { return };
        let Ok((header, end)) = vsf::verification::read_verified(&doc, None) else { return };
        let Ok(section) = header.primary_section(&doc, end) else { return };
        self.denied.clear();
        for f in section.get_fields("request") {
            let v = &f.values;
            if let (Some(vsf::VsfType::ke(l)), Some(vsf::VsfType::hb(s)), Some(t)) = (v.first(), v.get(1), v.get(2).and_then(|x| x.as_eagle())) {
                if let (Ok(l), Ok(s)) = (<[u8; 32]>::try_from(l.as_slice()), <[u8; 32]>::try_from(s.as_slice())) {
                    self.denied.push((l, s, t));
                }
            }
        }
    }

    /// A `depart_deny` from a fleet device (on the LAN, or the space's departure entry): the request it names cleared here and never asked again; this device's own request denied ends its departure (the person may ask again: a new request, a new salt).
    pub fn on_depart_deny(&mut self, bytes: &[u8]) {
        let Ok((hp, leaver, salt, t, denier)) = fgtw::protocol::parse_depart_deny(bytes) else { return };
        if self.hp != Some(hp) || !self.bound || !self.members.contains(&denier) || denier == leaver {
            return;
        }
        let known = self.denied.iter().any(|&(l, s, _)| l == leaver && s == salt);
        self.note_denied(leaver, salt, t);
        if self.depart_ask.as_ref().is_some_and(|d| d.leaver == leaver && (d.salt == salt || d.t == t)) {
            self.depart_ask = None;
        }
        if leaver == self.me && self.depart.as_ref().is_some_and(|d| d.salt == salt) {
            self.depart = None;
            let _ = ferros_sys::store_delete(cap(self.log.grants, S_STORE), DEPART_KEY);
            self.depart_denied = Some(denier);
            self.log.put(b"fgtw: our departure was denied by ");
            self.log.fp(&denier);
            self.log.put(b": this device stays in the fleet");
            self.log.line();
        } else if !known {
            self.log.put(b"fgtw: the departure of ");
            self.log.fp(&leaver);
            self.log.put(b" was denied by ");
            self.log.fp(&denier);
            self.log.line();
        }
    }

    /// The approved departure countersigned and published (fgtw::sponsor::approve_departure), the device's brand released when it leaves to a new owner; the chain is read again so the members drop the leaver.
    fn approve_now(&mut self) {
        let Some(hp) = self.hp else { return };
        let Some(d) = self.depart_ask.as_mut() else { return };
        d.approve = false;
        let (leaver, t, sig, intent) = (d.leaver, d.t, d.sig.clone(), d.intent);
        ferros_sys::pin(ferros_sys::CORES_BIG);
        ferros_sys::sleep_until(now() + ferros_sys::freq() / 1000);
        fgtw::set_clock(eagle());
        let r = {
            let s = self.seed();
            let r = fgtw::sponsor::approve_departure(&s, &self.keys, &hp, &leaver, t, &sig, intent);
            self.seed_ip = s.ip.get();
            r
        };
        ferros_sys::pin(0);
        let ok = r.is_ok();
        self.log.put(b"fgtw: the departure of ");
        self.log.fp(&leaver);
        match r {
            Ok(fgtw::sponsor::Approved::BrandKept) => self.log.put(b" approved: the consented Remove is published"),
            Ok(fgtw::sponsor::Approved::Released(Ok(()))) => self.log.put(b" approved: the consented Remove is published and the device released for its new owner"),
            Ok(fgtw::sponsor::Approved::Released(Err(e))) => {
                self.log.put(b" approved: the consented Remove is published; the release failed: ");
                self.log.put(e.as_bytes());
            }
            Err(e) => {
                self.log.put(b" was not published: ");
                self.log.put(e.as_bytes());
            }
        }
        self.log.line();
        if let Some(d) = self.depart_ask.as_mut() {
            d.result = if ok { 2 } else { 3 };
        }
        if ok {
            self.space_drop_depart(&leaver);
            // The members without the leaver: the chain as the seed (or the held copy) now has it.
            fgtw::set_clock(eagle());
            let fetched = {
                let s = self.seed();
                fgtw::held::fetch(&s, &self.held, &hp)
            };
            if let Ok(Some(b)) = fetched {
                self.note_members(&b);
                if let Ok(bytes) = b.to_vsf_bytes() {
                    self.persist_set(b"chain", &bytes);
                }
            }
        }
    }

    /// The radios listening for newcomers while the add flow is open: the Bluetooth driver scanning for pairing beacons (op 3 on, op 4 off) and the NFC driver reading a tapped device's tag (op 4 on, op 5 off). CALL_LIVE: a driver that is down fails at once.
    fn sponsor_radios(&mut self, on: bool) {
        let (nfc, bt) = (cap(unsafe { GRANTS }, S_NFC), cap(unsafe { GRANTS }, S_BT));
        let (mut ra, mut rb) = ([0u8; 4], [0u8; 4]);
        let a = ferros_sys::call_live(nfc, &[if on { 4 } else { 5 }], &mut ra);
        let b = ferros_sys::call_live(bt, &[if on { 3 } else { 4 }], &mut rb);
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
        self.log.put(if on { b"fgtw: listening for newcomers: nfc reader ".as_slice() } else { b"fgtw: no longer listening for newcomers: nfc reader " });
        self.log.put(said(a, &ra));
        self.log.put(b", bt scan ");
        self.log.put(said(b, &rb));
        self.log.line();
    }

    /// What the radios heard: the Bluetooth driver's pairing beacons (op 5: [count][count × 16 bytes]) and the NFC driver's last tag read (op 6: [32 bytes] once, then nothing). A tag whose secret opens a candidate's commitment picks it, its key following at once (Photon's tap).
    fn sponsor_hear(&mut self) {
        let (nfc, bt) = (cap(unsafe { GRANTS }, S_NFC), cap(unsafe { GRANTS }, S_BT));
        let mut r = [0u8; 1 + 16 * fgtw::pair::BEACON_UUID_LEN];
        let n = ferros_sys::call_live(bt, &[5], &mut r);
        let mut changed = false;
        if n != ERR && n >= 1 {
            // WHY: the count is the driver's word, and the reply's length may run past this buffer (it says what was sent): only the ids both hold are read.
            let k = (r[0] as usize).min((n as usize - 1) / fgtw::pair::BEACON_UUID_LEN).min(16);
            let t = now_us();
            for i in 0..k {
                let mut u = [0u8; fgtw::pair::BEACON_UUID_LEN];
                u.copy_from_slice(&r[1 + i * fgtw::pair::BEACON_UUID_LEN..1 + (i + 1) * fgtw::pair::BEACON_UUID_LEN]);
                match self.ble_heard.iter_mut().find(|(x, _)| *x == u) {
                    Some(e) => e.1 = t,
                    None => {
                        self.ble_heard.push((u, t));
                        changed = true;
                    }
                }
            }
        }
        let mut s = [0u8; 32];
        if ferros_sys::call_live(nfc, &[6], &mut s) == 32 {
            match fgtw::sponsor::nfc_match(&s, &self.adding.cands) {
                // NFC adds at once (owner, 2026-11-10: "Touch on screen ... required for BLE OR no touch on screen, just physical proximity (NFC). Both of which need the fleet page open"): the device held within a few centimetres, its tag's secret opening its request's commitment, the Fleet page open here and the newcomer attested is the whole consent. Bound and grown as one act, as Add does. A device picked on the screen still selects, turns green, and waits for Add.
                Some(i) if matches!(self.adding.phase, ADD_OPEN | ADD_SELECTED) => {
                    self.log.put(b"fgtw: a tapped device's tag matches ");
                    self.log.put(self.adding.cands[i].name.as_bytes());
                    self.log.put(b": adding it");
                    self.log.line();
                    self.adding.selected = None;
                    self.adding.picked = Some((self.adding.cands[i].req.clone(), "an NFC tap"));
                    self.adding.phase = ADD_BINDING;
                }
                Some(_) => {}
                None => {
                    self.log.put(b"fgtw: a tapped tag's secret matches no device asking to join (a stale request, or not a newcomer's)");
                    self.log.line();
                }
            }
        }
        if changed {
            self.rebuild_candidates();
        }
    }

    /// The sponsor's work, once a pass of the loop: the registry listed every 8 s while the add flow is open, the radios listening while it is, a pick bound, a confirm grown, an approval countersigned.
    fn sponsor_step(&mut self, t: u64) {
        let open = self.adding.phase != ADD_CLOSED && self.bound;
        self.listen_pass += 1;
        if open != self.listening || (open && self.listen_pass >= 64) {
            self.sponsor_radios(open);
        }
        if open {
            self.sponsor_hear();
        }
        if self.adding.phase == ADD_OPEN && self.net_up && t - self.adding.listed_us >= 8_000_000 {
            self.adding.listed_us = t;
            if let (Some(hp), Some(seed)) = (self.hp, self.identity_seed) {
                fgtw::set_clock(eagle());
                let r = {
                    let s = self.seed();
                    let r = fgtw::client::bindreq_list(&s, &self.keys, &hp, &seed);
                    self.seed_ip = s.ip.get();
                    r
                };
                match r {
                    Ok(reqs) => {
                        self.seed_heard_us = now_us();
                        if reqs.len() != self.adding.registry.len() {
                            self.log.put(b"fgtw: fgtw.org's registry lists ");
                            self.log.dec(reqs.len() as u64);
                            self.log.put(b" device(s) asking to join");
                            self.log.line();
                        }
                        self.adding.registry = reqs;
                        self.rebuild_candidates();
                    }
                    Err(e) => {
                        self.log.put(b"fgtw: the registry's list failed: ");
                        self.log.put(e.as_bytes());
                        self.log.line();
                    }
                }
            }
        }
        if self.adding.phase == ADD_BINDING && self.adding.picked.is_some() {
            self.bind_picked();
        }
        if self.adding.phase == ADD_GROWING && self.adding.confirm {
            self.grow_for_newcomer();
        }
        if self.depart_ask.as_ref().is_some_and(|d| d.approve) {
            self.approve_now();
        }
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
    // No wait for the network here: the `fgtw` endpoint is served from the start, so the attest screen always gets an answer (until 2026-10-31 this thread waited for an IPv4 lease first, and with no network attest froze on the roots hand-over). The UDP ports and beacon groups are taken now; the IP process keeps the groups and reports them once the link is up; look-ups wait for IPv4 in the loop.
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
    let mut c = Client { log, udp, keys, me, identity_seed: None, hp: None, bound: false, fleet_key: None, members: Vec::new(), chain_seen: 0, pt, sponsors: Vec::new(), request_t: 0, request_frame: Vec::new(), radios_on: false, radios_last: None, radios_missed: false, radios_at: 0, depart: None, net_up: false, offline_looks: 0, last_beacon_us: 0, last_ping_us: 0, last_words_us: 0, persist: false, verdict: V_NONE, last_lookup_us: 0, seed_heard_us: 0, join_checked_us: 0, seed_ip: None, given, given_asked_t: 0, given_depart_t: 0, held: core::cell::RefCell::new(fgtw::held::HeldChains::new()), held_fan: core::cell::RefCell::new(fgtw::held::HeldFanouts::new()), adding: Adding::new(), lan_reqs: Vec::new(), lan_from: Vec::new(), ble_heard: Vec::new(), listening: false, listen_pass: 0, listen_said: (255, 255), ble_said: (0, 0), depart_ask: None, denied: Vec::new(), depart_denied: None, offered: Vec::new(), select_waiting: None, selected_by: None, entry: 0, space: space::SpaceSync::new(), watchers: Vec::new(), watch_gen: 0, watch_seen: Vec::new(), fgtw_asked_us: 0 };
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
    ferros_sys::panic_exit()
}
