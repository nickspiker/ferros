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
    if ty != 22 {
        // Any other sensor: its latest event kept whole, for `diag usf ev`.
        unsafe {
            let t = &mut *(&raw mut LAST_EV);
            let i = t.iter().position(|e| e.0 == ty || e.1 == 0).unwrap_or(0);
            let n = p.len().min(128);
            t[i].0 = ty;
            t[i].1 = n;
            t[i].2[..n].copy_from_slice(&p[..n]);
            t[i].3 += 1;
        }
        return;
    }
    if size < 12 || count == 0 {
        return;
    }
    let at = 16 + (count as usize - 1) * (8 + size as usize) + 8;
    let (Some(x), Some(y), Some(z)) = (u32_at(p, at), u32_at(p, at + 4), u32_at(p, at + 8)) else { return };
    let n = (GRAVITY_Z.load(Ordering::Relaxed) >> 32) as u32;
    GRAVITY_XY.store(x as u64 | (y as u64) << 32, Ordering::Relaxed);
    GRAVITY_Z.store(z as u64 | (n.wrapping_add(1).max(1) as u64) << 32, Ordering::Relaxed);
    // The trace for `diag usf trace`: every sample of the event, with the AoC's timestamp (low 56 bits, ns) and when this pass saw it.
    let now = crate::platform::cntpct();
    let f = crate::platform::cntfrq() as u128;
    for k in 0..count as usize {
        let at = 16 + k * (8 + size as usize);
        let ts = u32_at(p, at).unwrap_or(0) as u64 | (u32_at(p, at + 4).unwrap_or(0) as u64) << 32;
        let v = [u32_at(p, at + 8).unwrap_or(0), u32_at(p, at + 12).unwrap_or(0), u32_at(p, at + 16).unwrap_or(0)];
        // The last two samples with their times on ferros's counter, for a reader that wants gravity at a fixed lag (the display's clock): the AoC stamps in ns on the same clock (measured 2026-10-24: each sample arrives ~1 ms after its stamp).
        let ticks = ((ts & 0x00FF_FFFF_FFFF_FFFF) as u128 * f / 1_000_000_000) as u64;
        for i in 0..3 {
            PAIR[3 + i].store(PAIR[i].load(Ordering::Relaxed), Ordering::Relaxed);
        }
        PAIR[0].store(v[0] as u64 | (v[1] as u64) << 32, Ordering::Relaxed);
        PAIR[1].store(v[2] as u64, Ordering::Relaxed);
        PAIR[2].store(ticks, Ordering::Relaxed);
        unsafe {
            let t = &mut *(&raw mut TRACE);
            t.0[t.1 % 32] = (ts & 0x00FF_FFFF_FFFF_FFFF, now, v);
            t.1 += 1;
        }
    }
}

/// The last two gravity samples: [x | y << 32, z, time in counter ticks] for the newest, then the same for the one before (f32 bits, m/s²). SYS_FACTS 14–19.
pub static PAIR: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// The latest event of each other sensor type seen: (USF sensor type, bytes, the event's first 128 bytes, how many came).
static mut LAST_EV: [(u16, usize, [u8; 128], u32); 8] = [(0, 0, [0; 128], 0); 8];

/// `diag usf ev`: each sensor type's latest event — the header (subscriber handle, subscription, type, sample count, bytes per sample) and each sample's timestamp and values as f32 and as raw words.
pub fn events(out: &mut alloc::vec::Vec<u8>) {
    let t = unsafe { &*(&raw const LAST_EV) };
    for &(ty, n, ref b, cnt) in t.iter().filter(|e| e.1 != 0) {
        let p = &b[..n];
        let (count, size) = (p.get(14).copied().unwrap_or(0) as usize, p.get(15).copied().unwrap_or(0) as usize);
        out.extend_from_slice(alloc::format!("type {ty}: {cnt} events, last A#{n} bytes, subscription {:?}, {count} samples of {size} bytes\n", u32_at(p, 8)).as_bytes());
        for k in 0..count {
            let at = 16 + k * (8 + size);
            let ts = u32_at(p, at).unwrap_or(0) as u64 | (u32_at(p, at + 4).unwrap_or(0) as u64) << 32;
            out.extend_from_slice(alloc::format!("  ts G#{ts:X}:").as_bytes());
            for w in 0..size / 4 {
                if let Some(v) = u32_at(p, at + 8 + 4 * w) {
                    out.extend_from_slice(alloc::format!(" {:.4} (G#{v:08X})", f32::from_bits(v)).as_bytes());
                }
            }
            out.push(b'\n');
        }
    }
}

/// `diag usf sensors`: every handle the AoC lists, with its name.
pub fn sensors(out: &mut alloc::vec::Vec<u8>) {
    for k in 1..128 {
        if let Some(n) = info(k) {
            out.extend_from_slice(alloc::format!("{k}: {n}\n").as_bytes());
        }
    }
}

/// Subscribe to any sensor: `mode` 1 continuous, 2 on change, 3 one-shot; `period_ms`; no batching. Returns the subscription (0: refused).
pub fn subscribe(handle: u32, period_ms: u64, mode: u32, wake: bool, out: &mut alloc::vec::Vec<u8>) -> u32 {
    let mut m = SUBSCRIBE.to_vec();
    put32(&mut m, 52, handle);
    m[126] = if wake { 0 } else { 1 };
    m[127] = 0;
    m[128..136].copy_from_slice(&0u64.to_le_bytes());
    m[136..144].copy_from_slice(&(period_ms * 1_000_000).to_le_bytes());
    put32(&mut m, 148, mode);
    let Some(txn) = send(m, Some(60)) else {
        out.extend_from_slice(b"usf: subscribe not sent (ring full)\n");
        return 0;
    };
    let r = reply(txn, 256);
    let id = r.as_ref().and_then(|(_, b)| body_u32(b, 0)).unwrap_or(0);
    out.extend_from_slice(alloc::format!("usf: subscribe handle {handle} every {period_ms} ms mode {mode}: status {:?}, subscription {id}\n", r.as_ref().map(|(s, _)| *s)).as_bytes());
    if id != 0 && SUB_ID.load(Ordering::Relaxed) == 0 {
        SUB_ID.store(id, Ordering::Relaxed);
    }
    id
}

/// The last 32 gravity samples: (AoC timestamp ns, counter at arrival, x y z bits), and how many were written.
static mut TRACE: ([(u64, u64, [u32; 3]); 32], usize) = ([(0, 0, [0; 3]); 32], 0);

/// `diag usf trace`: the last 32 samples, oldest first, with the AoC's timestamp against ferros's counter (in ns at its frequency).
pub fn trace(out: &mut alloc::vec::Vec<u8>) {
    let f = crate::platform::cntfrq();
    let now = crate::platform::cntpct();
    out.extend_from_slice(alloc::format!("counter A#{f} Hz, now A#{} ns\n", now as u128 * 1_000_000_000 / f as u128).as_bytes());
    let t = unsafe { &*(&raw const TRACE) };
    for i in 0..32.min(t.1) {
        let (ts, at, v) = t.0[(t.1 + 32 - 32.min(t.1) + i) % 32];
        let at_ns = (at as u128 * 1_000_000_000 / f as u128) as i128;
        let g = |b: u32| f32::from_bits(b);
        out.extend_from_slice(alloc::format!("ts A#{ts} arrival-ts A#{} us  ({:.4}, {:.4}, {:.4})\n", (at_ns - ts as i128) / 1000, g(v[0]), g(v[1]), g(v[2])).as_bytes());
    }
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

/// Open our channel and ask for the sensor manager (the HAL's first request): what the rest needs. Returns whether the AoC answered.
pub fn open(out: &mut alloc::vec::Vec<u8>) -> bool {
    SVC[0].store(aoc::service("com.google.usf"), Ordering::Relaxed);
    SVC[1].store(aoc::service("com.google.usf.non_wake_up"), Ordering::Relaxed);
    let h = usf();
    if h == 0 {
        out.extend_from_slice(b"usf: com.google.usf not found (the AoC is not up)\n");
        return false;
    }
    // AOCC_CMD_CLOSE_CHANNEL first ({channel 0, command 1, the channel}): a warm reload finds the AoC still serving the last kernel's subscriptions on this channel, and closing it ends them (the HAL's restart did the same; nothing came on a closed channel again), so samples are not delivered twice. Then AOCC_CMD_OPEN_CHANNEL {0, 0, the channel}.
    let close = [0u32.to_le_bytes(), 1u32.to_le_bytes(), CHAN.to_le_bytes()].concat();
    aoc::ipc_write(h, &close);
    let open = [0u32.to_le_bytes(), 0u32.to_le_bytes(), CHAN.to_le_bytes()].concat();
    let opened = aoc::ipc_write(h, &open);
    let server = send(GET_SERVER.to_vec(), Some(60)).and_then(|t| reply(t, 64));
    out.extend_from_slice(alloc::format!("usf: channel {CHAN} open {opened}, sensor manager {:?}\n", server.as_ref().map(|(s, b)| (*s, body_u32(b, 0)))).as_bytes());
    server.is_some()
}

/// The sensors the AoC lists (type 5 to the sensor manager, handle 13): how many. Zero until the registry is loaded and its buses probed.
pub fn sensor_count() -> Option<usize> {
    let mut m = BARE.to_vec();
    put32(&mut m, 52, 13);
    put32(&mut m, 56, 5);
    let txn = send(m, Some(60))?;
    let (_, body) = reply(txn, 64)?;
    let f = field(&body, root(&body)?, 0)?;
    u32_at(&body, f + u32_at(&body, f)? as usize).map(|n| n as usize)
}

/// Push the sensor registry, as the sensor HAL does on connect (sensors.usf.so SensorHal::LoadRegistry; decoded 2026-10-24): each script whole as a LoadScript request (type 24, body {f0 the text and its NUL, f1 the board's CDT, f2 its MLB number}) to the registry server, in order, stopping at the first error, then a Set (type 22) of "/" with loaded=1, on which the AoC probes its buses and the sensors appear. Skipped when the registry already says loaded (a warm reload). `get` reads the vault: the scripts are `aoc.reg.0`, `aoc.reg.1`, … (tools/husky-aoc-reg.sh).
pub fn registry(get: &mut dyn FnMut(&str) -> Option<alloc::vec::Vec<u8>>, out: &mut alloc::vec::Vec<u8>) -> bool {
    let Some(rh) = server(&REGISTRY) else {
        out.extend_from_slice(b"registry server: no answer\n");
        return false;
    };
    // Loaded already? A Get of "/" lists its props.
    if let Some(txn) = request(rh, 21, Some(&fb(&[F::B(b"/\0")]))) {
        if let Some((0, body)) = reply(txn, 64) {
            if body.windows(7).any(|w| w == b"loaded\0") {
                out.extend_from_slice(b"registry: loaded already\n");
                return true;
            }
        }
    }
    let (cdt, mlb) = (CDT.load(Ordering::Relaxed), MLB.load(Ordering::Relaxed));
    out.extend_from_slice(alloc::format!("registry: board CDT G#{cdt:X} MLB A#{mlb}\n").as_bytes());
    let mut n = 0;
    for i in 0.. {
        let Some(text) = get(&alloc::format!("aoc.reg.{i}")) else { break };
        let mut text = strip(&text, KEEP_OFF);
        text.push(0);
        let body = fb(&[F::B(&text), if cdt == 0 { F::N } else { F::U(cdt) }, if mlb == 0 { F::N } else { F::U(mlb) }]);
        let Some(txn) = request(rh, 24, Some(&body)) else {
            out.extend_from_slice(alloc::format!("registry: script {i} not sent\n").as_bytes());
            return false;
        };
        match reply(txn, 512) {
            Some((0, _)) => n += 1,
            r => {
                out.extend_from_slice(alloc::format!("registry: script {i} ({} bytes): {:?}\n", text.len(), r.map(|(s, _)| s)).as_bytes());
                return false;
            }
        }
    }
    if n == 0 {
        out.extend_from_slice(b"registry: no aoc.reg.* in the vault (tools/husky-aoc-reg.sh)\n");
        return false;
    }
    let set = fb(&[F::B(b"/\0"), F::T(alloc::vec![alloc::vec![F::B(b"loaded\0"), F::B(b"1\0")]])]);
    let r = request(rh, 22, Some(&set)).and_then(|t| reply(t, 512));
    out.extend_from_slice(alloc::format!("registry: {n} scripts loaded, loaded=1: {:?}\n", r.as_ref().map(|(s, _)| *s)).as_bytes());
    matches!(r, Some((0, _)))
}

/// Registry nodes ferros does not hand the AoC: the touch controller (`/dev/ftm5`, on the AoC's own SPI bus for its tap and long-press gestures) and the touch bus arbitration (`/tbn`). With them the AoC probes the touch IC and takes its bus, and ferros's touch goes dead (2026-10-24, the first boot with the registry). Touch is ferros's; the AoC's gestures can come back later as a deliberate handover.
const KEEP_OFF: &[&[u8]] = &[b"+/dev/ftm5", b"+/tbn"];

/// A registry script without the node blocks that start with any of `drop`: a block is its `+/path` line and the indented lines under it.
fn strip(text: &[u8], drop: &[&[u8]]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::with_capacity(text.len());
    let mut skipping = false;
    for line in text.split_inclusive(|&c| c == b'\n') {
        let indented = line.first().is_some_and(|&c| c == b' ' || c == b'\t');
        if !indented {
            skipping = drop.iter().any(|d| line.starts_with(d));
        }
        if !skipping {
            out.extend_from_slice(line);
        }
    }
    out
}

/// Subscribe to the Gravity sensor (handle 54 in this firmware; looked up by name in case it moves) every `period_ms`, no batching. `wake`: deliver on com.google.usf rather than the non-wake-up service.
pub fn subscribe_gravity(period_ms: u64, wake: bool, out: &mut alloc::vec::Vec<u8>) -> bool {
    let mut handle = 54;
    let mut name = info(handle);
    // Android's HAL lists it as "Gravity Sensor".
    let is_gravity = |n: &Option<alloc::string::String>| n.as_deref().is_some_and(|n| n.starts_with("Gravity"));
    if !is_gravity(&name) {
        out.extend_from_slice(alloc::format!("usf: handle 54 is {name:?}; looking for Gravity\n").as_bytes());
        handle = 0;
        for k in 18..96 {
            let n = info(k);
            if is_gravity(&n) {
                (handle, name) = (k, n);
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
    out.extend_from_slice(alloc::format!("usf: subscribe {name:?} (handle {handle}) every {period_ms} ms, {}: status {:?}, subscription {id}\n", if wake { "wake" } else { "non-wake" }, r.as_ref().map(|(s, _)| *s)).as_bytes());
    SUB_ID.store(id.max(1), Ordering::Relaxed);
    id != 0
}

/// `diag usf start [wake] [ms]`: open, push the registry, wait up to 2 s for sensors, subscribe — all at once (the boot does it a step a pass).
pub fn start(get: &mut dyn FnMut(&str) -> Option<alloc::vec::Vec<u8>>, period_ms: u64, wake: bool, out: &mut alloc::vec::Vec<u8>) -> bool {
    if !open(out) || !registry(get, out) {
        return false;
    }
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    let mut n = 0;
    while crate::platform::cntpct().wrapping_sub(t0) < 2 * f {
        n = sensor_count().unwrap_or(0);
        if n > 0 {
            break;
        }
    }
    out.extend_from_slice(alloc::format!("usf: A#{n} sensors after A#{} ms\n", crate::platform::cntpct().wrapping_sub(t0) * 1000 / f).as_bytes());
    subscribe_gravity(period_ms, wake, out)
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

// ── Building messages ──
/// A FlatBuffer field: a 4-byte scalar, a [ubyte] vector, a vector of tables, or absent (a FlatBuffer leaves a default-valued field out).
pub enum F<'a> {
    U(u32),
    B(&'a [u8]),
    T(alloc::vec::Vec<alloc::vec::Vec<F<'a>>>),
    N,
}

/// Lay one table out at the end of `b`, forward: its vtable, then the table (soffset, a 4-byte slot per present field), then each field's vector or nested tables after it, so every uoffset points forward. Returns the table's position. Any valid layout parses the same; the AoC's registry server answered these (2026-10-24).
fn emit(b: &mut alloc::vec::Vec<u8>, fields: &[F]) -> usize {
    let pad4 = |b: &mut alloc::vec::Vec<u8>| {
        while b.len() % 4 != 0 {
            b.push(0);
        }
    };
    pad4(b);
    let vt = b.len();
    let present = fields.iter().filter(|f| !matches!(f, F::N)).count();
    b.extend_from_slice(&((4 + 2 * fields.len()) as u16).to_le_bytes());
    b.extend_from_slice(&((4 + 4 * present) as u16).to_le_bytes());
    let mut k = 0;
    for f in fields {
        let o = if matches!(f, F::N) { 0 } else { k += 1; 4 * k };
        b.extend_from_slice(&(o as u16).to_le_bytes());
    }
    pad4(b);
    let t = b.len();
    b.extend_from_slice(&((t - vt) as i32).to_le_bytes());
    let mut slots = alloc::vec::Vec::new();
    for f in fields {
        match f {
            F::N => {}
            F::U(v) => b.extend_from_slice(&v.to_le_bytes()),
            _ => {
                slots.push(b.len());
                b.extend_from_slice(&0u32.to_le_bytes());
            }
        }
    }
    let patch = |b: &mut alloc::vec::Vec<u8>, at: usize, to: usize| b[at..at + 4].copy_from_slice(&((to - at) as u32).to_le_bytes());
    let mut si = 0;
    for f in fields {
        match f {
            F::B(v) => {
                pad4(b);
                let v0 = b.len();
                b.extend_from_slice(&(v.len() as u32).to_le_bytes());
                b.extend_from_slice(v);
                patch(b, slots[si], v0);
                si += 1;
            }
            F::T(list) => {
                pad4(b);
                let v0 = b.len();
                b.extend_from_slice(&(list.len() as u32).to_le_bytes());
                let first = b.len();
                b.resize(first + 4 * list.len(), 0);
                for (i, e) in list.iter().enumerate() {
                    let et = emit(b, e);
                    patch(b, first + 4 * i, et);
                }
                patch(b, slots[si], v0);
                si += 1;
            }
            _ => {}
        }
    }
    t
}

/// A whole FlatBuffer whose root table has these fields.
fn fb(fields: &[F]) -> alloc::vec::Vec<u8> {
    let mut b = alloc::vec![0u8; 4];
    let t = emit(&mut b, fields);
    b[0..4].copy_from_slice(&(t as u32).to_le_bytes());
    b
}

/// The largest message the transport sends whole (libusf EfwTransportClient: G#3F4, without the channel word); a longer one goes as fragments (kind 3).
const MAX_MSG: usize = 1012;

/// Put one finished outer message on our channel, as fragments when it is longer than MAX_MSG: each {f0 1, f1 the whole length, f2 the byte offset (absent at 0), f3 the slice} in a kind-3 message; the AoC joins the slices and parses the whole (libusf UsfTransport::SendMsgFrag).
fn send_msg(outer: &[u8]) -> bool {
    let h = usf();
    if h == 0 {
        return false;
    }
    let put = |m: &[u8]| {
        let mut w = CHAN.to_le_bytes().to_vec();
        w.extend_from_slice(m);
        // A full ring (256 slots) drains as the AoC reads: wait up to 64 ms for room.
        let f = crate::platform::cntfrq();
        let t0 = crate::platform::cntpct();
        while !aoc::ipc_write(h, &w) {
            if crate::platform::cntpct().wrapping_sub(t0) > f / 16 {
                return false;
            }
        }
        true
    };
    if outer.len() <= MAX_MSG {
        return put(outer);
    }
    let frag = |c: &[u8], off: usize| fb(&[F::U(3), F::B(&fb(&[F::U(1), F::U(outer.len() as u32), if off == 0 { F::N } else { F::U(off as u32) }, F::B(c)]))]);
    let overhead = frag(&[0; 4], 1).len() - 4;
    let chunk = (MAX_MSG - overhead) & !3;
    let mut off = 0;
    while off < outer.len() {
        let c = &outer[off..(off + chunk).min(outer.len())];
        if !put(&frag(c, off)) {
            return false;
        }
        off += c.len();
    }
    true
}

/// A request on our channel: outer {kind 1, payload: {type, transaction, handle, body}}; its transaction.
fn request(handle: u32, ty: u32, body: Option<&[u8]>) -> Option<u32> {
    let txn = TXN.fetch_add(1, Ordering::Relaxed);
    let inner = fb(&[F::U(ty), F::U(txn), F::U(handle), body.map(F::B).unwrap_or(F::N)]);
    send_msg(&fb(&[F::U(1), F::B(&inner)])).then_some(txn)
}

/// Open a server by its 16-byte UUID (type 3 to handle 1): its handle.
fn server(uuid: &[u8; 16]) -> Option<u32> {
    let txn = request(1, 3, Some(&fb(&[F::B(uuid)])))?;
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
    let Some(txn) = request(rh, 21, Some(&fb(&[F::B(&p)]))) else { return };
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

// ── The board's identity for the registry scripts ──
/// The board's CDT (product << 16 | stage << 12 | major << 8 | minor << 4 | variant) as ABL's device tree gives it in /chosen/plat, read at entry (`read_board`); 0 when absent. The registry scripts filter their sections on it (`?+0x20000:0xFFFF0000`, husky_dvt excludes proto and EVT stages); with 0 the AoC skips husky_common and husky_dvt and no sensor appears.
pub static CDT: AtomicU32 = AtomicU32::new(0);
/// The board's MLB number from /chosen/config/pcbcfg ("MLB-<n>"); 0 when absent.
pub static MLB: AtomicU32 = AtomicU32::new(0);

/// Read /chosen/plat {product, stage, major, minor, variant} (big-endian cells) and /chosen/config pcbcfg out of the FDT at `addr`, as libusf's UsfRegistryStoreFile::GetDevCdtInfo does from /sys/firmware/devicetree. A walk of the structure block tracking the node path.
pub fn read_board(addr: u64) {
    let be = |p: usize| unsafe { u32::from_be(core::ptr::read_unaligned(p as *const u32)) };
    let base = addr as usize;
    if be(base) != 0xD00D_FEED {
        return;
    }
    let size = be(base + 4) as usize;
    let (st, strs) = (base + be(base + 8) as usize, base + be(base + 12) as usize);
    let end = base + size.min(4 << 20);
    let name_at = |p: usize| -> &'static [u8] {
        let mut n = 0;
        while p + n < end && unsafe { *((p + n) as *const u8) } != 0 {
            n += 1;
        }
        unsafe { core::slice::from_raw_parts(p as *const u8, n) }
    };
    // The path as node names, depth 0 = root.
    let mut path: [&[u8]; 8] = [b""; 8];
    let mut depth = 0usize;
    let mut cells = [0u32; 5];
    let mut pos = st;
    while pos + 4 <= end {
        let tok = be(pos);
        pos += 4;
        match tok {
            1 => {
                let n = name_at(pos);
                if depth < 8 {
                    path[depth] = n;
                }
                depth += 1;
                pos = (pos + n.len() + 1 + 3) & !3;
            }
            2 => depth = depth.saturating_sub(1),
            3 => {
                let (len, off) = (be(pos) as usize, be(pos + 4) as usize);
                let data = pos + 8;
                let pname = name_at(strs + off);
                if depth == 3 && path[1] == b"chosen" && path[2] == b"plat" && len == 4 {
                    let k = [b"product".as_slice(), b"stage", b"major", b"minor", b"variant"].iter().position(|&k| k == pname);
                    if let Some(k) = k {
                        cells[k] = be(data);
                    }
                }
                if depth == 3 && path[1] == b"chosen" && path[2] == b"config" && pname == b"pcbcfg" {
                    let v = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
                    if let Some(rest) = v.strip_prefix(b"MLB-") {
                        let n = rest.iter().take_while(|c| c.is_ascii_digit()).fold(0u32, |a, &c| a * 10 + (c - b'0') as u32);
                        MLB.store(n, Ordering::Relaxed);
                    }
                }
                pos = (data + len + 3) & !3;
            }
            4 => {}
            _ => break,
        }
    }
    let [product, stage, major, minor, variant] = cells;
    CDT.store(product << 16 | (stage & 15) << 12 | (major & 15) << 8 | (minor & 15) << 4 | (variant & 15), Ordering::Relaxed);
}
