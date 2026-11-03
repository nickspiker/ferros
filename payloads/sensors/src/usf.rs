//! USF, the AoC's sensor framework, spoken over the AoC's IPC rings from this process: the kernel's usf.rs (where it was proven, 2026-10-23 to 10-25) on this process's grants. Every message is `[u32 channel word][FlatBuffer]`: the channel's low 31 bits, bit 31 for a non-wake-up message. The FlatBuffer is an outer table {f0 kind, f1 [ubyte] payload} (kind 1 request, 2 response, 3 fragment, 5 event, 9 sample event); a request's payload is {f0 type, f1 transaction, f2 handle, f3 [ubyte] body}. The requests are the sensor HAL's own, captured 2026-10-23, with the per-session words patched in.
//!
//! The rings live in the AoC's IPC block, Device memory mapped for this process (`aoc-ipc`): every access is an aligned word or a single byte, never a copy that could go unaligned. The AoC is signalled through its mailbox block (`aoc-mbox`).

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use ferros_sys::sensor_block as sb;
use ferros_sys::{cap, freq, mmio_write, now};

/// ferros's channel on com.google.usf (the HAL used 6 and 7; the kernel uses this same one, so taking the conversation over closes its subscriptions).
const CHAN: u32 = 2;

static mut G: u64 = 0;
/// The IPC block's base as this process sees it (the grant's address: Device memory, identity-mapped).
static mut IPC: usize = 0;
/// The two services' headers: com.google.usf and its non-wake-up twin.
static mut SVC: [usize; 2] = [0; 2];
static mut TXN: u32 = 100;
static mut SUB_ID: u32 = 0;

pub fn init(grants: u64) {
    unsafe {
        G = grants;
        IPC = ferros_sys::dma(cap(grants, crate::S_IPC)) as usize;
    }
}

/// The AoC's IPC region through its handle (SANDBOX.md, "Handles"): every access `base + (offset & mask)`, the same instructions native and masked. The addresses this file computes are absolute (IPC plus an offset the AoC gave); the handle takes the offset.
type Ipc = ferros_sys::Handle<{ crate::S_IPC }>;
fn rd32(a: usize) -> u32 {
    Ipc::read32((a - unsafe { IPC }) as u64)
}
fn wr32(a: usize, v: u32) {
    Ipc::write32((a - unsafe { IPC }) as u64, v)
}
fn rd8(a: usize) -> u8 {
    Ipc::read8((a - unsafe { IPC }) as u64)
}
fn wr8(a: usize, v: u8) {
    Ipc::write8((a - unsafe { IPC }) as u64, v)
}

// ── The AoC's IPC control block and its queue services (aoc.rs) ──
const AOC_MAGIC: u32 = 0xA0C0_0A0C;
const SVC_HDR: usize = 92;
const REGION: usize = 28;
/// The three mailbox blocks (G#1520_0000, G#1522_0000, G#1524_0000) as offsets in the `aoc-mbox` grant, and INTGR0 in each.
const MBOX_BLOCKS: [u64; 3] = [0, 0x2_0000, 0x4_0000];
const MB_INTGR0: u64 = 0x20;

/// Whether the AoC's firmware is up: its control block holds the magic.
pub fn aoc_online() -> bool {
    rd32(unsafe { IPC }) == AOC_MAGIC
}

/// A service's header by name (a walk of the table: some three thousand Device reads, so done once).
fn service(name: &str) -> usize {
    let ipc = unsafe { IPC };
    if rd32(ipc) != AOC_MAGIC {
        return 0;
    }
    let count = rd32(ipc + 24) as usize;
    let base = ipc + rd32(ipc + 32) as usize;
    // The control block's own count, held to the table's room in the window (Device memory outside the grant would fault): external input, written by the AoC.
    for i in 0..count.min(128) {
        let h = base + i * SVC_HDR;
        let mut k = 0;
        let matched = loop {
            let c = rd8(h + k);
            let want = name.as_bytes().get(k).copied().unwrap_or(0);
            if c != want {
                break false;
            }
            if c == 0 {
                break true;
            }
            k += 1;
            if k == 32 {
                break false;
            }
        };
        if matched {
            return h;
        }
    }
    0
}

fn region(h: usize, dir: usize) -> usize {
    h + 36 + dir * REGION
}

fn mbox_signal(idx: u32) {
    let block = MBOX_BLOCKS[(idx / 16) as usize % 3];
    mmio_write(cap(unsafe { G }, crate::S_MBOX), block + MB_INTGR0, 1 << (idx % 16));
}

/// One message onto a queue service's DOWN ring (AP->AoC), then the AoC signalled: an 8-byte header {u16 len, 0, 0, 0}, then the payload, a byte at a time.
fn ipc_write(h: usize, payload: &[u8]) -> bool {
    let ipc = unsafe { IPC };
    let r = region(h, 1);
    let (off, size, slots, tx, rx) = (rd32(r) as usize, rd32(r + 4) as usize, rd32(r + 8), rd32(r + 12), rd32(r + 16));
    if slots == 0 || payload.len() + 8 > size || tx.wrapping_sub(rx) >= slots {
        return false;
    }
    let dst = ipc + off + size * (tx % slots) as usize;
    wr32(dst, payload.len() as u32 & 0xFFFF);
    wr32(dst + 4, 0);
    for (i, &b) in payload.iter().enumerate() {
        wr8(dst + 8 + i, b);
    }
    unsafe { core::arch::asm!("dsb sy") };
    wr32(r + 12, tx.wrapping_add(1));
    mbox_signal((rd32(h + 32) >> 19) & 0x1F);
    true
}

/// One message from a queue service's UP ring (AoC->AP), if one waits: the bytes copied.
fn ipc_read(h: usize, buf: &mut [u8]) -> Option<usize> {
    let ipc = unsafe { IPC };
    let r = region(h, 0);
    let (off, size, slots, tx, rx) = (rd32(r) as usize, rd32(r + 4) as usize, rd32(r + 8), rd32(r + 12), rd32(r + 16));
    if slots == 0 || tx == rx {
        return None;
    }
    let src = ipc + off + size * (rx % slots) as usize;
    // The length is the AoC's: held to the buffer and the slot.
    let n = ((rd32(src) & 0xFFFF) as usize).min(buf.len()).min(size - 8);
    for i in 0..n {
        buf[i] = rd8(src + 8 + i);
    }
    wr32(r + 16, rx.wrapping_add(1));
    // A ring that was full before this read: the AoC is told there is room again (aoc_service_read), or a stream left full waits for a signal that never comes.
    if tx.wrapping_sub(rx) >= slots {
        mbox_signal((rd32(h + 32) >> 19) & 0x1F);
    }
    Some(n)
}

// ── A FlatBuffer reader, as much as these messages need. Every offset in a message is the AoC's: each read goes through `get`, so a bad one is None, never a panic. ──
fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn root(b: &[u8]) -> Option<usize> {
    u32_at(b, 0).map(|o| o as usize)
}
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
fn vector(b: &[u8], p: usize) -> Option<&[u8]> {
    let v = p + u32_at(b, p)? as usize;
    let n = u32_at(b, v)? as usize;
    b.get(v + 4..v + 4 + n)
}
fn outer(msg: &[u8]) -> Option<(u32, u8, &[u8])> {
    let ch = u32_at(msg, 0)?;
    let fb = msg.get(4..)?;
    let t = root(fb)?;
    let kind = *fb.get(field(fb, t, 0)?)?;
    let payload = vector(fb, field(fb, t, 1)?)?;
    Some((ch, kind, payload))
}
fn response(p: &[u8]) -> Option<(u32, u32, &[u8])> {
    let t = root(p)?;
    let txn = u32_at(p, field(p, t, 1)?)?;
    let status = field(p, t, 2).and_then(|f| u32_at(p, f)).unwrap_or(0);
    let body = field(p, t, 3).and_then(|f| vector(p, f)).unwrap_or(&[]);
    Some((txn, status, body))
}
fn body_u32(b: &[u8], i: usize) -> Option<u32> {
    u32_at(b, field(b, root(b)?, i)?)
}
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
/// A request with no body: handle at 52, type at 56, transaction at 60.
const BARE: [u8; 64] = [
    0x00, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x00, 0x08, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x10, 0x00, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00,
];
/// Subscribe (type 8): handle at 52, transaction at 60; f10 (the HAL's own handle) at 112, f5 non-wake-up at 126, f3 at 127, f2 latency ns (u64) at 128, f1 period ns (u64) at 136, f0 reporting mode at 148.
const SUBSCRIBE: [u8; 152] = [
    0x00, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x00, 0x08, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x78, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x14, 0x00, 0x08, 0x00, 0x0c, 0x00, 0x04, 0x00, 0x10, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x48, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x1c, 0x00, 0x30, 0x00, 0x2c, 0x00, 0x20, 0x00, 0x18, 0x00, 0x17, 0x00, 0x00, 0x00, 0x16, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x07, 0x00, 0x1c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x94, 0x35, 0x77, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc2, 0xeb, 0x0b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
];

fn put32(m: &mut [u8], at: usize, v: u32) {
    m[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn next_txn() -> u32 {
    unsafe {
        let t = TXN;
        TXN = TXN.wrapping_add(1);
        t
    }
}

/// One request on our channel; its transaction.
fn send(mut m: Vec<u8>, txn_at: Option<usize>) -> Option<u32> {
    let h = unsafe { SVC[0] };
    if h == 0 {
        return None;
    }
    put32(&mut m, 0, CHAN);
    let txn = next_txn();
    if let Some(at) = txn_at {
        put32(&mut m, at, txn);
    }
    ipc_write(h, &m).then_some(txn)
}

/// Drain both UP rings: samples into the block; the reply to `want` (a transaction) returned as (status, body).
pub fn drain(want: Option<u32>) -> Option<(u32, Vec<u8>)> {
    let mut got = None;
    for k in 0..2 {
        let h = unsafe { SVC[k] };
        if h == 0 {
            continue;
        }
        let mut buf = [0u8; 1024];
        while let Some(n) = ipc_read(h, &mut buf) {
            let Some((ch, kind, payload)) = outer(&buf[..n]) else { continue };
            if ch & 0x7FFF_FFFF != CHAN {
                continue;
            }
            dispatch(kind, payload, want, &mut got);
        }
    }
    got
}

/// A fragmented message being joined: the bytes so far and the whole length.
static mut FRAG: (Vec<u8>, usize) = (Vec::new(), 0);

/// One outer message: 2 a response, 9 a packed sample event, 5 an event wrapping an inner message (13 the light waveform's format, 4 its data), 3 a fragment of a longer message (joined, then dispatched whole).
fn dispatch(kind: u8, payload: &[u8], want: Option<u32>, got: &mut Option<(u32, Vec<u8>)>) {
    match kind {
        2 => {
            if let Some((txn, status, body)) = response(payload) {
                if Some(txn) == want {
                    *got = Some((status, body.to_vec()));
                }
            }
        }
        9 => sample(payload),
        5 => {
            let inner = (|| {
                let t = root(payload)?;
                let k = *payload.get(field(payload, t, 0)?)?;
                let b = vector(payload, field(payload, t, 1)?)?;
                Some((k, b))
            })();
            if let Some((k, b)) = inner {
                if k == 13 {
                    flk_format(b);
                } else if k == 4 {
                    flk_data(b);
                }
            }
        }
        3 => {
            let parts = (|| {
                let t = root(payload)?;
                let total = u32_at(payload, field(payload, t, 1)?)? as usize;
                let off = field(payload, t, 2).and_then(|f| u32_at(payload, f)).unwrap_or(0) as usize;
                let chunk = vector(payload, field(payload, t, 3)?)?;
                Some((total, off, chunk))
            })();
            let Some((total, off, chunk)) = parts else { return };
            let f = unsafe { &mut *(&raw mut FRAG) };
            if off == 0 {
                f.0.clear();
                f.1 = total;
            }
            if off != f.0.len() || total != f.1 || total > 64 * 1024 {
                f.0.clear();
                return;
            }
            f.0.extend_from_slice(chunk);
            if f.0.len() >= f.1 {
                let whole = core::mem::take(&mut f.0);
                let parsed = (|| {
                    let t = root(&whole)?;
                    let k = *whole.get(field(&whole, t, 0)?)?;
                    let b = vector(&whole, field(&whole, t, 1)?)?;
                    Some((k, b.to_vec()))
                })();
                if let Some((k, b)) = parsed {
                    if k != 3 {
                        dispatch(k, &b, want, got);
                    }
                }
            }
        }
        _ => {}
    }
}

// ── The samples, as the kernel kept them (usf.rs), then copied into the block by `publish` ──
static mut GRAVITY: (u64, u64) = (0, 0);
static mut PAIR: [u64; 6] = [0; 6];
static mut SPECTRAL: [u64; 7] = [0; 7];
static mut PROX: [u64; 3] = [0; 3];
static mut SCOPE_FMT: u64 = 0;
static mut SCOPE_N: u64 = 0;
static mut SCOPE_MASK: u64 = 0x20;

fn block() -> *mut u64 {
    unsafe { (*(&raw mut crate::BLOCK)).0.as_mut_ptr() }
}
fn put(i: usize, v: u64) {
    unsafe { core::ptr::write_volatile(block().add(i), v) };
}

/// Copy the samples into the block the kernel reads (the scope's ring is written there directly).
pub fn publish() {
    unsafe {
        put(sb::GRAVITY_XY, GRAVITY.0);
        put(sb::GRAVITY_Z, GRAVITY.1);
        for i in 0..6 {
            put(sb::PAIR + i, PAIR[i]);
        }
        for i in 0..7 {
            put(sb::SPECTRAL + i, SPECTRAL[i]);
        }
        for i in 0..3 {
            put(sb::PROX + i, PROX[i]);
        }
        put(sb::SCOPE_FMT, SCOPE_FMT);
        put(sb::SCOPE_N, SCOPE_N);
        put(sb::SCOPE_MASK, SCOPE_MASK);
    }
}

/// A sample event: {u32 1, u32 the subscriber's handle, u32 subscription, u16 sensor type, u8 count, u8 bytes per sample}, then each sample {u64 timestamp, the values}.
fn sample(p: &[u8]) {
    let (Some(ty), Some(&count), Some(&size)) = (u16_at(p, 12), p.get(14), p.get(15)) else { return };
    if count == 0 {
        return;
    }
    let at = 16 + (count as usize - 1) * (8 + size as usize) + 8;
    let w = |o: usize| u32_at(p, at + 4 * o).unwrap_or(0) as u64;
    unsafe {
        if ty == 12 && size >= 48 {
            // The rear spectral sensor (VD6282): the newest sample's six channel counts and six gains.
            for i in (0..6).step_by(2) {
                SPECTRAL[i / 2] = w(i) | w(i + 1) << 32;
                SPECTRAL[3 + i / 2] = w(6 + i) | w(7 + i) << 32;
            }
            SPECTRAL[6] = SPECTRAL[6].wrapping_add(1);
        }
        if ty == 3 && size >= 32 {
            // Proximity (TMD3719): distance (0 near, 5 far), processed pdata0/pdata1, the raw counts.
            PROX[0] = w(0) | w(1) << 32;
            PROX[1] = w(2) | w(6) << 32;
            let n = (PROX[2] >> 32).wrapping_add(1).max(1);
            PROX[2] = w(7) | n << 32;
        }
        if ty != 22 || size < 12 {
            return;
        }
        // Gravity (type 22): three f32, m/s²; the last two samples with their times on ferros's counter.
        let n = (GRAVITY.1 >> 32) as u32;
        GRAVITY = (w(0) | w(1) << 32, w(2) | (n.wrapping_add(1).max(1) as u64) << 32);
        let fq = freq() as u128;
        for k in 0..count as usize {
            let a = 16 + k * (8 + size as usize);
            let ts = u32_at(p, a).unwrap_or(0) as u64 | (u32_at(p, a + 4).unwrap_or(0) as u64) << 32;
            let v = [u32_at(p, a + 8).unwrap_or(0), u32_at(p, a + 12).unwrap_or(0), u32_at(p, a + 16).unwrap_or(0)];
            let ticks = ((ts & 0x00FF_FFFF_FFFF_FFFF) as u128 * fq / 1_000_000_000) as u64;
            for i in 0..3 {
                PAIR[3 + i] = PAIR[i];
            }
            PAIR[0] = v[0] as u64 | (v[1] as u64) << 32;
            PAIR[1] = v[2] as u64;
            PAIR[2] = ticks;
        }
    }
}

/// The light waveform's format (FlickerFormat: f0 rate Hz, f1 code — bytes per sample —, f2 little-endian, f3 low bits to ignore).
fn flk_format(p: &[u8]) {
    let Some(t) = root(p) else { return };
    let rate = field(p, t, 0).and_then(|f| u32_at(p, f)).unwrap_or(0) as u64;
    let code = field(p, t, 1).and_then(|f| p.get(f).copied()).unwrap_or(0) as u64;
    let le = field(p, t, 2).and_then(|f| p.get(f).copied()).unwrap_or(0) as u64;
    let ign = field(p, t, 3).and_then(|f| p.get(f).copied()).unwrap_or(0) as u64;
    unsafe { SCOPE_FMT = rate | code << 32 | ign << 40 | le << 48 };
}

/// FlickerData (f0 the byte offset of these samples in the stream, f1 the samples): decoded straight into the block's ring at their own sample numbers. Uncompressed formats only (code 2/3/4).
fn flk_data(p: &[u8]) {
    let fmt = unsafe { SCOPE_FMT };
    let (code, ign, le) = (((fmt >> 32) & 0xFF) as usize, ((fmt >> 40) & 0xFF) as u32, (fmt >> 48) & 1 == 1);
    if !(2..=4).contains(&code) {
        return;
    }
    let Some(t) = root(p) else { return };
    let off = field(p, t, 0).and_then(|f| u32_at(p, f)).unwrap_or(0) as u64;
    let Some(d) = field(p, t, 1).and_then(|f| vector(p, f)) else { return };
    let first = off / code as u64;
    let ring = unsafe { block().add(sb::SCOPE) as *mut i32 };
    for (i, s) in d.chunks_exact(code).enumerate() {
        let mut raw: u32 = 0;
        for (k, &b) in s.iter().enumerate() {
            raw |= (b as u32) << (8 * if le { k } else { code - 1 - k });
        }
        let v = ((raw << (32 - 8 * code as u32)) as i32) >> (32 - 8 * code as u32);
        unsafe { core::ptr::write_volatile(ring.add(((first + i as u64) % sb::SCOPE_LEN as u64) as usize), v >> ign) };
    }
    let end = first + (d.len() / code) as u64;
    unsafe {
        if end > SCOPE_N {
            SCOPE_N = end;
        }
    }
}

/// Wait up to `ms` for the reply to `txn`.
fn reply(txn: u32, ms: u64) -> Option<(u32, Vec<u8>)> {
    let f = freq();
    let t0 = now();
    while now().wrapping_sub(t0) < f * ms / 1000 {
        if let Some(r) = drain(Some(txn)) {
            return Some(r);
        }
    }
    None
}

/// A sensor's name (sensor info, type 4).
fn info(handle: u32) -> Option<String> {
    let mut m = BARE.to_vec();
    put32(&mut m, 52, handle);
    put32(&mut m, 56, 4);
    let txn = send(m, Some(60))?;
    let (_, body) = reply(txn, 64)?;
    body_text(&body, 0).map(String::from)
}

/// The handle of the first listed sensor whose name starts with `name`.
pub fn find(name: &str) -> Option<u32> {
    (1..128).find(|&k| info(k).is_some_and(|n| n.starts_with(name)))
}

/// Find the services, close our channel (ending whatever the kernel had subscribed on it), open it again, and ask for the sensor manager: what the rest needs. Whether the AoC answered.
pub fn open(out: &mut String) -> bool {
    unsafe {
        SVC[0] = service("com.google.usf");
        SVC[1] = service("com.google.usf.non_wake_up");
    }
    let h = unsafe { SVC[0] };
    if h == 0 {
        out.push_str("usf: com.google.usf not found\n");
        return false;
    }
    let close = [0u32.to_le_bytes(), 1u32.to_le_bytes(), CHAN.to_le_bytes()].concat();
    ipc_write(h, &close);
    let open = [0u32.to_le_bytes(), 0u32.to_le_bytes(), CHAN.to_le_bytes()].concat();
    let opened = ipc_write(h, &open);
    let server = send(GET_SERVER.to_vec(), Some(60)).and_then(|t| reply(t, 64));
    let _ = writeln!(out, "usf: channel {CHAN} open {opened}, sensor manager {:?}", server.as_ref().map(|(s, b)| (*s, body_u32(b, 0))));
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

// ── Building messages ──
/// A FlatBuffer field: a 4-byte scalar, a [ubyte] vector, a vector of 4-byte elements, a vector of tables, or absent.
pub enum F<'a> {
    U(u32),
    B(&'a [u8]),
    W(&'a [u32]),
    T(Vec<Vec<F<'a>>>),
    N,
}

/// Lay one table out at the end of `b`, forward: its vtable, then the table, then each field's vector or nested tables after it, so every uoffset points forward.
fn emit(b: &mut Vec<u8>, fields: &[F]) -> usize {
    let pad4 = |b: &mut Vec<u8>| {
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
    let mut slots = Vec::new();
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
    let patch = |b: &mut Vec<u8>, at: usize, to: usize| b[at..at + 4].copy_from_slice(&((to - at) as u32).to_le_bytes());
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
            F::W(v) => {
                pad4(b);
                let v0 = b.len();
                b.extend_from_slice(&(v.len() as u32).to_le_bytes());
                for w in v.iter() {
                    b.extend_from_slice(&w.to_le_bytes());
                }
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
fn fb(fields: &[F]) -> Vec<u8> {
    let mut b = alloc::vec![0u8; 4];
    let t = emit(&mut b, fields);
    b[0..4].copy_from_slice(&(t as u32).to_le_bytes());
    b
}

/// The largest message the transport sends whole (libusf EfwTransportClient, without the channel word); a longer one goes as fragments (kind 3).
const MAX_MSG: usize = 1012;

/// One finished outer message on our channel, as fragments when longer than MAX_MSG; a full ring is waited on up to 64 ms.
fn send_msg(outer: &[u8]) -> bool {
    let h = unsafe { SVC[0] };
    if h == 0 {
        return false;
    }
    let put = |m: &[u8]| {
        let mut w = CHAN.to_le_bytes().to_vec();
        w.extend_from_slice(m);
        let f = freq();
        let t0 = now();
        while !ipc_write(h, &w) {
            if now().wrapping_sub(t0) > f / 16 {
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

/// A request on our channel: outer {kind 1, payload {type, transaction, handle, body}}; its transaction.
fn request(handle: u32, ty: u32, body: Option<&[u8]>) -> Option<u32> {
    let txn = next_txn();
    let inner = fb(&[F::U(ty), F::U(txn), F::U(handle), body.map(F::B).unwrap_or(F::N)]);
    send_msg(&fb(&[F::U(1), F::B(&inner)])).then_some(txn)
}

/// Open a server by its 16-byte UUID (type 3 to handle 1): its handle.
fn server(uuid: &[u8; 16]) -> Option<u32> {
    let txn = request(1, 3, Some(&fb(&[F::B(uuid)])))?;
    let (_, body) = reply(txn, 64)?;
    body_u32(&body, 0)
}

/// The registry server's UUID.
const REGISTRY: [u8; 16] = [0xd7, 0x6a, 0x14, 0xcc, 0x26, 0x4d, 0xee, 0xa1, 0xc6, 0x40, 0x6f, 0x8b, 0x46, 0x16, 0x82, 0xc0];

/// A vault value in the `aoc.` scope, read in 4 KiB pieces.
fn vault(key: &str) -> Option<Vec<u8>> {
    let c = cap(unsafe { G }, crate::S_STORE);
    let mut piece = [0u8; 4096];
    let total = ferros_sys::store_get_at(c, key.as_bytes(), 0, &mut piece)?;
    let mut v = Vec::with_capacity(total);
    let mut at = 0;
    while at < total {
        let n = (total - at).min(piece.len());
        if at > 0 {
            ferros_sys::store_get_at(c, key.as_bytes(), at, &mut piece[..n])?;
        }
        v.extend_from_slice(&piece[..n]);
        at += n;
    }
    Some(v)
}

/// Push the sensor registry, as the sensor HAL does on connect: each script whole as a LoadScript request (type 24, body {f0 the text and its NUL, f1 the board's CDT, f2 its MLB number}), in order, stopping at the first error, then a Set of "/" with loaded=1, on which the AoC probes its buses and the sensors appear. Skipped when the registry already says loaded. The scripts are the vault's `aoc.reg.0`, `aoc.reg.1`, … (tools/husky-aoc-reg.sh).
pub fn registry(out: &mut String) -> bool {
    let Some(rh) = server(&REGISTRY) else {
        out.push_str("registry server: no answer\n");
        return false;
    };
    if let Some(txn) = request(rh, 21, Some(&fb(&[F::B(b"/\0")]))) {
        if let Some((0, body)) = reply(txn, 64) {
            if body.windows(7).any(|w| w == b"loaded\0") {
                out.push_str("registry: loaded already\n");
                return true;
            }
        }
    }
    let (cdt, mlb) = ferros_sys::sensors_board(cap(unsafe { G }, crate::S_SENSORS));
    let _ = writeln!(out, "registry: board CDT G#{cdt:X} MLB A#{mlb}");
    let mut n = 0;
    for i in 0.. {
        let Some(text) = vault(&alloc::format!("reg.{i}")) else { break };
        let mut text = replace_lines(&strip(&text, KEEP_OFF), OVERRIDE);
        text.push(0);
        let body = fb(&[F::B(&text), if cdt == 0 { F::N } else { F::U(cdt) }, if mlb == 0 { F::N } else { F::U(mlb) }]);
        let Some(txn) = request(rh, 24, Some(&body)) else {
            let _ = writeln!(out, "registry: script {i} not sent");
            return false;
        };
        match reply(txn, 512) {
            Some((0, _)) => n += 1,
            r => {
                let _ = writeln!(out, "registry: script {i} ({} bytes): {:?}", text.len(), r.map(|(s, _)| s));
                return false;
            }
        }
    }
    if n == 0 {
        out.push_str("registry: no aoc.reg.* in the vault (tools/husky-aoc-reg.sh)\n");
        return false;
    }
    let set = fb(&[F::B(b"/\0"), F::T(alloc::vec![alloc::vec![F::B(b"loaded\0"), F::B(b"1\0")]])]);
    let r = request(rh, 22, Some(&set)).and_then(|t| reply(t, 512));
    let _ = writeln!(out, "registry: {n} scripts loaded, loaded=1: {:?}", r.as_ref().map(|(s, _)| *s));
    matches!(r, Some((0, _)))
}

/// Registry nodes ferros does not hand the AoC: the touch controller (`/dev/ftm5`) and the touch bus arbitration (`/tbn`): with them the AoC takes the touch IC's bus and ferros's touch goes dead (2026-10-24).
const KEEP_OFF: &[&[u8]] = &[b"+/dev/ftm5", b"+/tbn"];

/// Registry properties ferros sets differently from Google's scripts: the rear spectral sensor reports every reading (`g_change_thresh=0 0`, `c1_…`), which the scope widget wants (owner, 2026-10-25).
const OVERRIDE: &[(&[u8], &[u8])] = &[(b"  g_change_thresh=100 0.1", b"  g_change_thresh=0 0"), (b"  c1_change_thresh=100 0.1", b"  c1_change_thresh=0 0")];

fn replace_lines(text: &[u8], pairs: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for line in text.split_inclusive(|&c| c == b'\n') {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        let body = body.strip_suffix(b"\r").unwrap_or(body);
        match pairs.iter().find(|(from, _)| body == *from) {
            Some((_, to)) => {
                out.extend_from_slice(to);
                out.push(b'\n');
            }
            None => out.extend_from_slice(line),
        }
    }
    out
}

fn strip(text: &[u8], drop: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
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

/// Subscribe to any sensor: `mode` 1 continuous, 2 on change, 3 one-shot; `period_ms`; no batching. The subscription (0: refused).
pub fn subscribe(handle: u32, period_ms: u64, mode: u32, wake: bool, out: &mut String) -> u32 {
    let mut m = SUBSCRIBE.to_vec();
    put32(&mut m, 52, handle);
    m[126] = if wake { 0 } else { 1 };
    m[127] = 0;
    m[128..136].copy_from_slice(&0u64.to_le_bytes());
    m[136..144].copy_from_slice(&(period_ms * 1_000_000).to_le_bytes());
    put32(&mut m, 148, mode);
    let Some(txn) = send(m, Some(60)) else {
        out.push_str("usf: subscribe not sent (ring full)\n");
        return 0;
    };
    let r = reply(txn, 256);
    let id = r.as_ref().and_then(|(_, b)| body_u32(b, 0)).unwrap_or(0);
    let _ = writeln!(out, "usf: subscribe handle {handle} every {period_ms} ms mode {mode}: status {:?}, subscription {id}", r.as_ref().map(|(s, _)| *s));
    if id != 0 && unsafe { SUB_ID } == 0 {
        unsafe { SUB_ID = id };
    }
    id
}

/// Subscribe to the Gravity sensor (handle 54 in this firmware; looked up by name if it moved) every `period_ms`.
pub fn subscribe_gravity(period_ms: u64, wake: bool, out: &mut String) -> bool {
    let is_gravity = |n: &Option<String>| n.as_deref().is_some_and(|n| n.starts_with("Gravity"));
    let mut handle = 54;
    if !is_gravity(&info(handle)) {
        handle = (18..96).find(|&k| is_gravity(&info(k))).unwrap_or(0);
        if handle == 0 {
            out.push_str("usf: no Gravity sensor\n");
            return false;
        }
    }
    subscribe(handle, period_ms, 1, wake, out) != 0
}

/// The DisplayInfo server's UUID.
const DISPLAY_INFO: [u8; 16] = [0xa8, 0x8e, 0xc6, 0x24, 0x10, 0x40, 0x2a, 0xed, 0xca, 0xb9, 0xa5, 0xaa, 0xd6, 0xa2, 0xee, 0xd2];

/// Tell the AoC the display's state (type 1001 to the DisplayInfo server): f0 brightness as DBV, f1 status (0 off, 1 on), f2 the refresh rate, f6 the panel's operating rate, f8 PWM mode, f9 the message version (2), f10 luma, f11 leakage. The under-display TMD3719 times its light and proximity bursts to the panel by these.
pub fn display_info(dbv: u32, on: bool, hz: u32, out: &mut String) -> bool {
    let Some(h) = server(&DISPLAY_INFO) else {
        out.push_str("display info server: no answer\n");
        return false;
    };
    let nz = |v: u32| if v == 0 { F::N } else { F::U(v) };
    let luma = [0u32; 3];
    let leak = [0u32; 1];
    let body = fb(&[nz(dbv), nz(on as u32), nz(hz), F::N, F::N, F::N, nz(hz), F::N, F::U(0xFF), F::U(2), F::W(&luma), F::W(&leak)]);
    let r = request(h, 1001, Some(&body)).and_then(|t| reply(t, 128));
    let _ = writeln!(out, "display info (handle {h}): DBV {dbv}, {}, {hz} Hz: {:?}", if on { "on" } else { "off" }, r.as_ref().map(|(s, _)| *s));
    r.is_some()
}

/// The VD6282 access request (type 1001 to the sensor's own handle): get or set one parameter (3 FlickerChannel, its channel mask in f4).
fn vd_access(handle: u32, set: bool, param: u32, f4: Option<u32>, out: &mut String) {
    let opt = |v: Option<u32>| v.map(F::U).unwrap_or(F::N);
    // The same ten fields as the kernel sent (f7 and f9 absent), so the bytes match what was proven.
    let body = fb(&[F::U(set as u32), F::U(param), F::N, F::N, opt(f4), F::N, F::N, F::N, F::N, F::N]);
    let r = request(handle, 1001, Some(&body)).and_then(|t| reply(t, 256));
    let _ = writeln!(out, "flicker access param {param}: {:?}", r.map(|(s, _)| s));
}

/// Follow preference 7 (the light scope): 1–6 subscribes the flicker stream (once) and points the flicker engine at that channel; 0 leaves it streaming (no unsubscribe is known) and the display stops drawing it.
pub fn scope_follow(ch: u8, out: &mut String) {
    if ch == 0 || ch > 6 {
        return;
    }
    let Some(h) = find("VD6282 Flicker") else {
        out.push_str("no VD6282 Flicker sensor listed\n");
        return;
    };
    if unsafe { SCOPE_N } == 0 {
        // As libusf's EnableFlickerSensor: mode 1, a 10 s period, no latency, non-wake, client id G#30002.
        let mut m = SUBSCRIBE.to_vec();
        put32(&mut m, 52, h);
        m[126] = 1;
        m[127] = 0;
        put32(&mut m, 112, 0x0003_0002);
        m[128..136].copy_from_slice(&0u64.to_le_bytes());
        m[136..144].copy_from_slice(&10_000_000_000u64.to_le_bytes());
        put32(&mut m, 148, 1);
        let r = send(m, Some(60)).and_then(|t| reply(t, 256));
        let _ = writeln!(out, "flicker subscribe (handle {h}): {:?}", r.map(|(s, b)| (s, body_u32(&b, 0))));
    }
    let mask = 1u32 << (ch - 1);
    vd_access(h, true, 3, Some(mask), out);
    unsafe { SCOPE_MASK = mask as u64 };
}
