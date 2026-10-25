//! The sensor service on the AoC (USF, Google's sensor framework), spoken directly over its IPC rings (aoc.rs): look a sensor up, subscribe to it, read its samples (SENSORS.md "The USF protocol").
//!
//! Every message is `[u32 channel word][FlatBuffer]`: the channel's low 31 bits, bit 31 for a non-wake-up message. The FlatBuffer is an outer table {f0 kind, f1 [ubyte] payload} (kind 1 request, 2 response, 3 fragment, 9 sample event); a request's payload is {f0 type, f1 transaction, f2 handle, f3 [ubyte] body}, its body a FlatBuffer of its own. The requests here are the sensor HAL's own, captured 2026-10-23, with the per-session words patched in: a FlatBuffer's bytes do not move when a scalar in it changes.
//!
//! What ferros keeps from it now: the gravity vector (the fused Gravity sensor), for the clock that stands upright — SYS_FACTS 12 and 13.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::aoc;

/// ferros's channel on com.google.usf (the HAL used 6 and 7; any unused index works, the probe used 1).
const CHAN: u32 = 2;

/// The gravity vector's last sample: x | y << 32 and z | count << 32, f32 bits, m/s² in the panel's frame (x right, y up, z out of the glass; Android's convention, so a phone upright reads y ≈ +9.8). Zero count: none yet.
pub static GRAVITY_XY: AtomicU64 = AtomicU64::new(0);
pub static GRAVITY_Z: AtomicU64 = AtomicU64::new(0);
/// The subscription the samples carry (0: none), and the transaction counter.
static SUB_ID: AtomicU32 = AtomicU32::new(0);
static TXN: AtomicU32 = AtomicU32::new(100);
/// Messages seen up both rings, and sample events, for `diag usf`.
pub static SEEN: AtomicU32 = AtomicU32::new(0);
pub static EVENTS: AtomicU32 = AtomicU32::new(0);

// ── A FlatBuffer reader, as much as these messages need ──
fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
/// The root table's position.
fn root(b: &[u8]) -> Option<usize> {
    u32_at(b, 0).map(|o| o as usize)
}
/// Field `i` of the table at `t`: its position, None when absent.
fn field(b: &[u8], t: usize, i: usize) -> Option<usize> {
    let vt = (t as i64 - u32_at(b, t)? as i32 as i64) as usize;
    let vsize = u16_at(b, vt)? as usize;
    if 4 + 2 * i + 2 > vsize {
        return None;
    }
    match u16_at(b, vt + 4 + 2 * i)? {
        0 => None,
        o => Some(t + o as usize),
    }
}
/// The vector a field at `p` points to.
fn vector(b: &[u8], p: usize) -> Option<&[u8]> {
    let v = p + u32_at(b, p)? as usize;
    let n = u32_at(b, v)? as usize;
    b.get(v + 4..v + 4 + n)
}

/// A message read up a ring: its channel word and the outer table's kind and payload.
fn outer(msg: &[u8]) -> Option<(u32, u8, &[u8])> {
    let ch = u32_at(msg, 0)?;
    let fb = msg.get(4..)?;
    let t = root(fb)?;
    let kind = *fb.get(field(fb, t, 0)?)?;
    let payload = vector(fb, field(fb, t, 1)?)?;
    Some((ch, kind, payload))
}

/// A response's transaction, status (0 when absent) and body.
fn response(p: &[u8]) -> Option<(u32, u32, &[u8])> {
    let t = root(p)?;
    let txn = u32_at(p, field(p, t, 1)?)?;
    let status = field(p, t, 2).and_then(|f| u32_at(p, f)).unwrap_or(0);
    let body = field(p, t, 3).and_then(|f| vector(p, f)).unwrap_or(&[]);
    Some((txn, status, body))
}

/// Field `i` of a body's root table as a u32.
fn body_u32(b: &[u8], i: usize) -> Option<u32> {
    u32_at(b, field(b, root(b)?, i)?)
}
/// Field `i` of a body's root table as text (a NUL-terminated [ubyte]).
fn body_text(b: &[u8], i: usize) -> Option<&str> {
    let v = vector(b, field(b, root(b)?, i)?)?;
    let n = v.iter().position(|&c| c == 0).unwrap_or(v.len());
    core::str::from_utf8(&v[..n]).ok()
}

// ── The requests (the HAL's, captured; byte offsets include the channel word) ──
/// "Get server" (type 3 to handle 1, transaction at 60) for the sensor manager's UUID: the HAL's first request.
const GET_SERVER: [u8; 112] = {
    let t: [u8; 108] = [
        0x0c, 0x00, 0x00, 0x00, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x00, 0x08, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x14, 0x00, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x00, 0x10, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x28, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x08, 0x00, 0x04, 0x00, 0x06, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x6b, 0x31, 0xcb, 0xf0, 0x74, 0x4a, 0x14, 0xa1, 0xd4, 0xe8, 0xae, 0x8a, 0x3e, 0x09, 0xfc, 0x4d,
    ];
    let mut m = [0u8; 112];
    let mut i = 0;
    while i < 108 {
        m[4 + i] = t[i];
        i += 1;
    }
    m
};
/// A request with no body: handle at 52, type at 56, transaction at 60 (the HAL's sensor-list request; sensor info is the same with type 4 and the sensor's handle).
const BARE: [u8; 64] = [
    0x00, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x00, 0x08, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x10, 0x00, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00,
];
/// Subscribe (type 8): handle at 52, transaction at 60; its body's table at 104: f11 at 111, f10 (the HAL's own handle, echoed in every sample) at 112, f5 non-wake-up at 126, f3 at 127, f2 latency ns (u64) at 128, f1 period ns (u64) at 136, f0 reporting mode at 148.
const SUBSCRIBE: [u8; 152] = [
    0x00, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x00, 0x08, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x78, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x14, 0x00, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x00, 0x10, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x48, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x1c, 0x00, 0x30, 0x00, 0x2c, 0x00, 0x20, 0x00, 0x18, 0x00, 0x17, 0x00, 0x00, 0x00, 0x16, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x07, 0x00, 0x1c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x94, 0x35, 0x77, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc2, 0xeb, 0x0b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
];

fn put32(m: &mut [u8], at: usize, v: u32) {
    m[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// The two services' headers, found once by `start` (a walk of the table by name is some three thousand uncached reads: never per pass).
static SVC: [core::sync::atomic::AtomicUsize; 2] = [const { core::sync::atomic::AtomicUsize::new(0) }; 2];

fn usf() -> usize {
    SVC[0].load(Ordering::Relaxed)
}

/// Send one request on our channel; its transaction.
fn send(mut m: alloc::vec::Vec<u8>, txn_at: Option<usize>) -> Option<u32> {
    let h = usf();
    if h == 0 {
        return None;
    }
    put32(&mut m, 0, CHAN);
    let txn = TXN.fetch_add(1, Ordering::Relaxed);
    if let Some(at) = txn_at {
        put32(&mut m, at, txn);
    }
    aoc::ipc_write(h, &m).then_some(txn)
}

/// Drain both UP rings: samples go to the gravity vector; the reply to `want` (a transaction) is returned as (status, body).
fn drain(want: Option<u32>) -> Option<(u32, alloc::vec::Vec<u8>)> {
    let mut got = None;
    for svc in &SVC {
        let h = svc.load(Ordering::Relaxed);
        if h == 0 {
            continue;
        }
        let mut buf = [0u8; 1024];
        while let Some(n) = aoc::ipc_read(h, &mut buf) {
            SEEN.fetch_add(1, Ordering::Relaxed);
            let Some((ch, kind, payload)) = outer(&buf[..n]) else { continue };
            if ch & 0x7FFF_FFFF != CHAN {
                continue;
            }
            match kind {
                2 => {
                    if let Some((txn, status, body)) = response(payload) {
                        if Some(txn) == want {
                            got = Some((status, body.to_vec()));
                        }
                    }
                }
                9 => sample(payload),
                _ => {}
            }
        }
    }
    got
}

/// A sample event: {u32 1, u32 the subscriber's handle, u32 subscription, u16 sensor type, u8 count, u8 bytes per sample}, then each sample {u64 timestamp, the values}. Gravity (type 22): three f32, m/s².
fn sample(p: &[u8]) {
    EVENTS.fetch_add(1, Ordering::Relaxed);
    let (Some(ty), Some(&count), Some(&size)) = (u16_at(p, 12), p.get(14), p.get(15)) else { return };
    if ty != 22 || size < 12 || count == 0 {
        return;
    }
    let at = 16 + (count as usize - 1) * (8 + size as usize) + 8;
    let (Some(x), Some(y), Some(z)) = (u32_at(p, at), u32_at(p, at + 4), u32_at(p, at + 8)) else { return };
    let n = (GRAVITY_Z.load(Ordering::Relaxed) >> 32) as u32;
    GRAVITY_XY.store(x as u64 | (y as u64) << 32, Ordering::Relaxed);
    GRAVITY_Z.store(z as u64 | (n.wrapping_add(1).max(1) as u64) << 32, Ordering::Relaxed);
}

/// Call often (each pass of the service loop): reads whatever the AoC has sent. A few uncached reads when nothing has.
pub fn poll() {
    if SUB_ID.load(Ordering::Relaxed) != 0 {
        drain(None);
    }
}

/// Wait up to `ms` for the reply to `txn`.
fn reply(txn: u32, ms: u64) -> Option<(u32, alloc::vec::Vec<u8>)> {
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while crate::platform::cntpct().wrapping_sub(t0) < f * ms / 1000 {
        if let Some(r) = drain(Some(txn)) {
            return Some(r);
        }
    }
    None
}

/// A sensor's name (sensor info, type 4).
fn info(handle: u32) -> Option<alloc::string::String> {
    let mut m = BARE.to_vec();
    put32(&mut m, 52, handle);
    put32(&mut m, 56, 4);
    let txn = send(m, Some(60))?;
    let (_, body) = reply(txn, 64)?;
    body_text(&body, 0).map(alloc::string::String::from)
}

/// Bring the gravity stream up: open our channel, ask for the sensor manager (as the HAL does first), find the Gravity sensor (handle 54 in this firmware; looked up by name in case it moves), subscribe every `period_ms` with no batching. `wake`: deliver on com.google.usf rather than the non-wake-up service. Returns what happened, for the log.
pub fn start(period_ms: u64, wake: bool, out: &mut alloc::vec::Vec<u8>) -> bool {
    SVC[0].store(aoc::service("com.google.usf"), Ordering::Relaxed);
    SVC[1].store(aoc::service("com.google.usf.non_wake_up"), Ordering::Relaxed);
    let h = usf();
    if h == 0 {
        out.extend_from_slice(b"usf: com.google.usf not found (the AoC is not up)\n");
        return false;
    }
    // AOCC_CMD_OPEN_CHANNEL: {channel 0, command 0, the channel}.
    let open = [0u32.to_le_bytes(), 0u32.to_le_bytes(), CHAN.to_le_bytes()].concat();
    let opened = aoc::ipc_write(h, &open);
    let server = send(GET_SERVER.to_vec(), Some(60)).and_then(|t| reply(t, 64));
    out.extend_from_slice(alloc::format!("usf: channel {CHAN} open {opened}, sensor manager {:?}\n", server.as_ref().map(|(s, b)| (*s, body_u32(b, 0)))).as_bytes());
    let mut handle = 54;
    let name = info(handle);
    // Android's HAL lists it as "Gravity Sensor".
    let is_gravity = |n: &Option<alloc::string::String>| n.as_deref().is_some_and(|n| n.starts_with("Gravity"));
    if !is_gravity(&name) {
        out.extend_from_slice(alloc::format!("usf: handle 54 is {name:?}; looking for Gravity\n").as_bytes());
        handle = 0;
        for k in 18..96 {
            if is_gravity(&info(k)) {
                handle = k;
                break;
            }
        }
        if handle == 0 {
            out.extend_from_slice(b"usf: no Gravity sensor\n");
            return false;
        }
    }
    let mut m = SUBSCRIBE.to_vec();
    put32(&mut m, 52, handle);
    m[126] = if wake { 0 } else { 1 };
    m[127] = 0;
    m[128..136].copy_from_slice(&0u64.to_le_bytes());
    m[136..144].copy_from_slice(&(period_ms * 1_000_000).to_le_bytes());
    put32(&mut m, 148, 1);
    let Some(txn) = send(m, Some(60)) else {
        out.extend_from_slice(b"usf: subscribe not sent (ring full)\n");
        return false;
    };
    let r = reply(txn, 256);
    let id = r.as_ref().and_then(|(_, b)| body_u32(b, 0)).unwrap_or(0);
    out.extend_from_slice(alloc::format!("usf: subscribe Gravity (handle {handle}) every {period_ms} ms, {}: status {:?}, subscription {id}\n", if wake { "wake" } else { "non-wake" }, r.as_ref().map(|(s, _)| *s)).as_bytes());
    SUB_ID.store(id.max(1), Ordering::Relaxed);
    id != 0
}

/// The sensors' two rails on the sub PMIC (zuma-common.dtsi, the AoC's `sensor_power_list`): LDO7S 1.8 V (L7S_SENSORS_FPS) and LDO5S 3.3 V (L5S_PROX), control registers G#35 and G#33 (s2mpg15-register.h); bit 7 enables. Both are boot-on in the device tree.
pub const RAILS: [(&str, u32); 2] = [("LDO7S 1.8 V", 0x35), ("LDO5S 3.3 V", 0x33)];

/// Turn the sensors' rails on (their voltages are already set: 1.8 V, 3.3 V), as Linux's AoC driver does at probe, before the firmware starts (aoc.c reset_sensor_power): the firmware finds no sensors on dark rails, and its sensor list comes back empty (2026-10-24).
pub fn power(out: &mut alloc::vec::Vec<u8>) {
    for (name, reg) in RAILS {
        let r = crate::acpm::update(crate::acpm::SUB, crate::acpm::T_PMIC, reg, 0x80, 0x80).and_then(|_| crate::acpm::read(crate::acpm::SUB, crate::acpm::T_PMIC, reg));
        out.extend_from_slice(alloc::format!("{name} on: {:?}\n", r.map(|c| alloc::format!("G#{c:02X}"))).as_bytes());
    }
}

/// `diag usf`: the rails, the stream's counters and the last gravity vector.
pub fn report(out: &mut alloc::vec::Vec<u8>) {
    for (name, reg) in RAILS {
        out.extend_from_slice(alloc::format!("{name} ctrl: {:?}\n", crate::acpm::read(crate::acpm::SUB, crate::acpm::T_PMIC, reg).map(|c| alloc::format!("G#{c:02X}"))).as_bytes());
    }
    let (xy, z) = (GRAVITY_XY.load(Ordering::Relaxed), GRAVITY_Z.load(Ordering::Relaxed));
    let f = |b: u64| f32::from_bits(b as u32);
    out.extend_from_slice(alloc::format!("usf: subscription {}, messages {}, events {}, gravity samples {}, last ({:.3}, {:.3}, {:.3}) m/s²\n", SUB_ID.load(Ordering::Relaxed), SEEN.load(Ordering::Relaxed), EVENTS.load(Ordering::Relaxed), z >> 32, f(xy), f(xy >> 32), f(z)).as_bytes());
}

/// `diag usf raw <handle> <type>`: send a bare request (no body) and dump every message up both rings for 64 ms, raw.
pub fn raw(handle: u32, ty: u32, out: &mut alloc::vec::Vec<u8>) {
    let mut m = BARE.to_vec();
    put32(&mut m, 52, handle);
    put32(&mut m, 56, ty);
    let txn = send(m, Some(60));
    out.extend_from_slice(alloc::format!("type {ty} to {handle}: transaction {txn:?}\n").as_bytes());
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while crate::platform::cntpct().wrapping_sub(t0) < f / 16 {
        for (k, svc) in SVC.iter().enumerate() {
            let h = svc.load(Ordering::Relaxed);
            if h == 0 {
                continue;
            }
            let mut buf = [0u8; 1024];
            while let Some(n) = aoc::ipc_read(h, &mut buf) {
                out.extend_from_slice(alloc::format!("ring {k}: A#{n} bytes, outer {:?}\n", outer(&buf[..n]).map(|(c, k, p)| (c, k, p.len(), response(p).map(|(t, s, b)| (t, s, b.len()))))).as_bytes());
                for row in buf[..n].chunks(32) {
                    for b in row {
                        out.extend_from_slice(alloc::format!("{b:02x}").as_bytes());
                    }
                    out.push(b'\n');
                }
            }
        }
    }
}

// ── Building requests (the same shapes as the captured ones: each table's vtable just before it, its vector just after) ──
/// A FlatBuffer holding one table whose fields are 4-byte scalars and, last, one [ubyte] vector.
fn table(scalars: &[u32], vector: Option<&[u8]>) -> alloc::vec::Vec<u8> {
    let nf = scalars.len() + vector.is_some() as usize;
    let vsize = 4 + 2 * nf;
    let vt = 4;
    let t = (vt + vsize + 3) & !3;
    let mut b = alloc::vec::Vec::new();
    b.extend_from_slice(&(t as u32).to_le_bytes());
    b.extend_from_slice(&(vsize as u16).to_le_bytes());
    b.extend_from_slice(&((4 + 4 * nf) as u16).to_le_bytes());
    for i in 0..nf {
        b.extend_from_slice(&((4 + 4 * i) as u16).to_le_bytes());
    }
    b.resize(t, 0);
    b.extend_from_slice(&((t - vt) as i32).to_le_bytes());
    for &v in scalars {
        b.extend_from_slice(&v.to_le_bytes());
    }
    if let Some(v) = vector {
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&(v.len() as u32).to_le_bytes());
        b.extend_from_slice(v);
        while b.len() % 4 != 0 {
            b.push(0);
        }
    }
    b
}

/// A whole request on our channel: [channel][outer {kind 1, payload: inner {type, transaction, handle, body}}]; its transaction.
fn request(handle: u32, ty: u32, body: Option<&[u8]>) -> Option<u32> {
    let h = usf();
    if h == 0 {
        return None;
    }
    let txn = TXN.fetch_add(1, Ordering::Relaxed);
    let inner = table(&[ty, txn, handle], body);
    let mut m = CHAN.to_le_bytes().to_vec();
    m.extend_from_slice(&table(&[1], Some(&inner)));
    aoc::ipc_write(h, &m).then_some(txn)
}

/// Open a server by its 16-byte UUID (type 3 to handle 1): its handle.
fn server(uuid: &[u8; 16]) -> Option<u32> {
    let txn = request(1, 3, Some(&table(&[], Some(uuid))))?;
    let (_, body) = reply(txn, 64)?;
    body_u32(&body, 0)
}

/// The registry server's UUID (captured: the HAL opens it before each registry read).
const REGISTRY: [u8; 16] = [0xd7, 0x6a, 0x14, 0xcc, 0x26, 0x4d, 0xee, 0xa1, 0xc6, 0x40, 0x6f, 0x8b, 0x46, 0x16, 0x82, 0xc0];

/// `diag usf reg <path>`: read one registry node (type 21): its keys and values, and its children.
pub fn reg(path: &str, out: &mut alloc::vec::Vec<u8>) {
    let Some(rh) = server(&REGISTRY) else {
        out.extend_from_slice(b"registry server: no answer\n");
        return;
    };
    let mut p = path.as_bytes().to_vec();
    p.push(0);
    let Some(txn) = request(rh, 21, Some(&table(&[], Some(&p)))) else { return };
    let Some((status, body)) = reply(txn, 128) else {
        out.extend_from_slice(b"registry read: no answer\n");
        return;
    };
    out.extend_from_slice(alloc::format!("registry (handle {rh}) {path}: status {status}, body A#{} bytes\n", body.len()).as_bytes());
    let Some(t) = root(&body) else { return };
    // f0: [table {f0 key, f1 value}], f1: [table {f0 child name}]; a vector of tables holds an offset to each.
    for (fi, label) in [(0usize, "  "), (1, "  child ")] {
        let Some(f) = field(&body, t, fi) else { continue };
        let Some(v0) = u32_at(&body, f).map(|o| f + o as usize) else { continue };
        let n = u32_at(&body, v0).unwrap_or(0) as usize;
        for k in 0..n.min(64) {
            let e = v0 + 4 + 4 * k;
            let Some(et) = u32_at(&body, e).map(|o| e + o as usize) else { break };
            let text = |i: usize| field(&body, et, i).and_then(|f| vector(&body, f)).map(|v| alloc::string::String::from_utf8_lossy(&v[..v.iter().position(|&c| c == 0).unwrap_or(v.len())]).into_owned());
            out.extend_from_slice(alloc::format!("{label}{}{}\n", text(0).unwrap_or_default(), text(1).map(|v| alloc::format!(" = {v}")).unwrap_or_default()).as_bytes());
        }
    }
}
