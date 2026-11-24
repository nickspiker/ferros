//! bttest: the Bluetooth driver's client ops end to end, against a peer that advertises manufacturer data with company G#FFFF and the 24-byte test tag (G#A0 ^ i) and echoes SDUs on an LE credit-based channel at PSM G#81 (docs/BT.md "Testing"): scan, connect, open the channel, echo SDUs of several sizes and measure their round trips, echo a run of 2 KiB SDUs back to back and measure the rate, disconnect, connect again and echo once more. Development only: spawned by hand with `use bt`.
//!
//! ferros-bridge spawn bttest.bin [hex input] --name bttest --use bt
//! Input, all optional: [flags u8][pings each size u8][SDUs in the run u16][interval min u8][interval max u8][timeout u16]; flags bit 0: end without closing anything (the driver must take the session down at this process's end), bit 1: no second connection.
//! A second thread holds the driver's EVENTS call all the time and records what comes; the first sends and waits on the second's count.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering::SeqCst};
use ferros_sys::{call, cap, freq, now};

struct Line {
    b: [u8; 512],
    n: usize,
}
impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line { b: [0; 512], n: 0 };
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!($($t)*));
        ferros_sys::console($g, &l.b[..l.n]);
    }};
}

const S_BT: u64 = 1;
const MTU: u16 = 2048;
const PSM: u16 = 0x81;

/// The receiving thread's count of events, rung on each (the sender waits on it).
static EVENTS: AtomicU64 = AtomicU64::new(0);
static SESSION: AtomicU64 = AtomicU64::new(0);
/// The advertiser found: address type in bits 48-55, the address below; 0 none yet.
static FOUND: AtomicU64 = AtomicU64::new(0);
static FOUND_RSSI: AtomicU32 = AtomicU32::new(0);
/// The link: status in bits 16-23 and the handle below, bit 31 set once it came; the interval and latency granted.
static LINK: AtomicU32 = AtomicU32::new(0);
static GRANTED: AtomicU64 = AtomicU64::new(0);
static GONE: AtomicU32 = AtomicU32::new(0);
/// The channel: bit 31 once answered, the result in bits 16-30 truncated, the cid below.
static CHAN: AtomicU32 = AtomicU32::new(0);
static CLOSED: AtomicU32 = AtomicU32::new(0);
/// SDUs echoed back: how many, their bytes, and those whose bytes were not what was sent.
static GOT: AtomicU32 = AtomicU32::new(0);
static GOT_BYTES: AtomicU64 = AtomicU64::new(0);
static BAD: AtomicU32 = AtomicU32::new(0);
static OUT_OF_ORDER: AtomicU32 = AtomicU32::new(0);
/// When each SDU came back, by its sequence number (the run's numbers fit: it is at most 2^12 SDUs).
const MOST: usize = 1 << 12;
static mut BACK_AT: [u64; MOST] = [0; MOST];
static mut RX_STACK: [u8; 1 << 16] = [0; 1 << 16];
static mut RX_BUF: [u8; 1 << 14] = [0; 1 << 14];
static mut TX_BUF: [u8; 16 + MTU as usize] = [0; 16 + MTU as usize];

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn us(t: u64) -> u64 {
    t * 1_000_000 / freq()
}

/// Wait until `done` holds, or `ms` passes; whether it held.
fn until(ms: u64, done: impl Fn() -> bool) -> bool {
    let end = now() + freq() / 1000 * ms;
    loop {
        let seen = EVENTS.load(SeqCst);
        if done() {
            return true;
        }
        if now() >= end {
            return false;
        }
        ferros_sys::wait(&EVENTS, seen, end);
    }
}

/// The receiving thread: EVENTS held all the time, each event recorded.
extern "C" fn rx_thread(grants: u64) -> ! {
    let buf = unsafe { &mut *(&raw mut RX_BUF) };
    let mut last_seq: i64 = -1;
    loop {
        let mut req = [0u8; 13];
        req[0] = 7;
        req[1..9].copy_from_slice(&SESSION.load(SeqCst).to_le_bytes());
        req[9..13].copy_from_slice(&(buf.len() as u32).to_le_bytes());
        let n = call(cap(grants, S_BT), &req, buf);
        if n == 0 || n == ferros_sys::ERR {
            say!(grants, "bttest: EVENTS answered empty: the session is gone\n");
            ferros_sys::thread_exit(0);
        }
        let t = now();
        let b = &buf[..n as usize];
        let mut at = 0;
        while at + 3 <= b.len() {
            let (kind, len) = (b[at], le16(b, at + 1) as usize);
            let e = &b[at + 3..at + 3 + len];
            at += 3 + len;
            match kind {
                0 if e.len() >= 9 => {
                    let data = &e[9..];
                    let mut k = 0;
                    while k + 1 < data.len() {
                        let l = data[k] as usize;
                        if l == 0 || k + 1 + l > data.len() {
                            break;
                        }
                        // The test peer's tag: company G#FFFF, then 24 bytes G#A0 ^ i (the Spin mount's shape, BT.md "Testing"); any other advertiser with G#FFFF data is not it.
                        let tag = l == 27 && data[k + 2] == 0xFF && data[k + 3] == 0xFF && (0..24).all(|i| data[k + 4 + i] == 0xA0 ^ i as u8);
                        if data[k + 1] == 0xFF && tag && FOUND.load(SeqCst) == 0 {
                            let mut a = [0u8; 8];
                            a[..6].copy_from_slice(&e[2..8]);
                            a[6] = e[1];
                            FOUND_RSSI.store(e[8] as i8 as i32 as u32, SeqCst);
                            FOUND.store(u64::from_le_bytes(a) | 1 << 63, SeqCst);
                            say!(grants, "bttest: heard G#FFFF manufacturer data ({} bytes) from a type A#{} address, event type A#{}, RSSI A#{} dBm\n", l - 3, e[1], e[0], e[8] as i8);
                        }
                        k += 1 + l;
                    }
                }
                1 if e.len() >= 16 => {
                    LINK.store(1 << 31 | (e[0] as u32) << 16 | le16(e, 1) as u32, SeqCst);
                    GRANTED.store(le16(e, 10) as u64 | (le16(e, 12) as u64) << 16 | (le16(e, 14) as u64) << 32, SeqCst);
                }
                2 if e.len() >= 4 => {
                    GONE.store(1 << 31 | (e[2] as u32) << 8 | e[3] as u32, SeqCst);
                    say!(grants, "bttest: disconnected: why A#{} reason G#{:02X}\n", e[2], e[3]);
                }
                3 if e.len() >= 16 => {
                    CHAN.store(1 << 31 | ((le16(e, 4) as u32) & 0x7FFF) << 16 | le16(e, 2) as u32, SeqCst);
                    say!(grants, "bttest: channel answered: result G#{:X}, the peer's MTU A#{} MPS A#{} credits A#{}, ours MTU A#{} MPS A#{}\n", le16(e, 4), le16(e, 6), le16(e, 8), le16(e, 10), le16(e, 12), le16(e, 14));
                }
                4 if e.len() >= 8 => {
                    let sdu = &e[4..];
                    let seq = u32::from_le_bytes([sdu[0], sdu[1], sdu[2], sdu[3]]);
                    let good = sdu[4..].iter().enumerate().all(|(i, &v)| v == (seq as usize + i) as u8);
                    if !good {
                        BAD.fetch_add(1, SeqCst);
                    }
                    if seq as i64 != last_seq + 1 && last_seq >= 0 && (seq as i64) > last_seq {
                        OUT_OF_ORDER.fetch_add(1, SeqCst);
                    }
                    last_seq = seq as i64;
                    if (seq as usize) < MOST {
                        unsafe { (*(&raw mut BACK_AT))[seq as usize] = t };
                    }
                    GOT_BYTES.fetch_add(sdu.len() as u64, SeqCst);
                    GOT.fetch_add(1, SeqCst);
                }
                5 if e.len() >= 5 => {
                    CLOSED.store(1 << 31 | e[4] as u32, SeqCst);
                    say!(grants, "bttest: channel G#{:X} closed: why A#{}\n", le16(e, 2), e[4]);
                }
                6 if e.len() >= 9 => say!(grants, "bttest: parameters now: status G#{:02X} interval A#{} latency A#{} timeout A#{}\n", e[2], le16(e, 3), le16(e, 5), le16(e, 7)),
                7 if e.len() >= 6 => say!(grants, "bttest: data length: TX A#{} RX A#{} octets\n", le16(e, 2), le16(e, 4)),
                8 if e.len() >= 4 => say!(grants, "bttest: PHY: TX A#{} RX A#{}\n", e[2], e[3]),
                _ => say!(grants, "bttest: event A#{kind}, A#{len} bytes\n"),
            }
        }
        EVENTS.fetch_add(1, SeqCst);
        ferros_sys::wake(&EVENTS, 1);
    }
}

/// The driver's STATE line, on the console.
fn state(grants: u64) {
    let mut r = [0u8; 512];
    let n = call(cap(grants, S_BT), &[0], &mut r);
    if n != ferros_sys::ERR {
        say!(grants, "bttest: driver: {}\n", core::str::from_utf8(&r[..n as usize]).unwrap_or("?"));
    }
}

fn op(grants: u64, m: &[u8]) -> u8 {
    let mut r = [0u8; 16];
    let n = call(cap(grants, S_BT), m, &mut r);
    if n == 0 || n == ferros_sys::ERR { 0xEE } else { r[0] }
}

fn with_session(op: u8, rest: &[u8], out: &mut [u8; 64]) -> usize {
    out[0] = op;
    out[1..9].copy_from_slice(&SESSION.load(SeqCst).to_le_bytes());
    out[9..9 + rest.len()].copy_from_slice(rest);
    9 + rest.len()
}

/// SDU `seq` of `len` bytes (at least 4): its number, then bytes counting on from it.
fn send(grants: u64, handle: u16, cid: u16, seq: u32, len: usize) -> u8 {
    let tx = unsafe { &mut *(&raw mut TX_BUF) };
    tx[0] = 13;
    tx[1..9].copy_from_slice(&SESSION.load(SeqCst).to_le_bytes());
    tx[9..11].copy_from_slice(&handle.to_le_bytes());
    tx[11..13].copy_from_slice(&cid.to_le_bytes());
    let sdu = &mut tx[13..13 + len];
    sdu[..4].copy_from_slice(&seq.to_le_bytes());
    for (i, v) in sdu[4..].iter_mut().enumerate() {
        *v = (seq as usize + i) as u8;
    }
    let mut r = [0u8; 4];
    let n = call(cap(grants, S_BT), &tx[..13 + len], &mut r);
    if n == 0 || n == ferros_sys::ERR { 0xEE } else { r[0] }
}

/// Connect to the advertiser and open the channel; the link's handle and the channel's id, or None (said why).
fn link_up(grants: u64, kind: u8, addr: &[u8], p: [u16; 4]) -> Option<(u16, u16)> {
    LINK.store(0, SeqCst);
    GONE.store(0, SeqCst);
    CHAN.store(0, SeqCst);
    CLOSED.store(0, SeqCst);
    let mut m = [0u8; 64];
    let mut rest = [0u8; 15];
    rest[0] = kind;
    rest[1..7].copy_from_slice(addr);
    for (k, v) in p.iter().enumerate() {
        rest[7 + 2 * k..9 + 2 * k].copy_from_slice(&v.to_le_bytes());
    }
    let n = with_session(10, &rest, &mut m);
    let t0 = now();
    let r = op(grants, &m[..n]);
    if r != 0 {
        say!(grants, "bttest: CONNECT refused: A#{r}\n");
        return None;
    }
    if !until(1 << 14, || LINK.load(SeqCst) != 0) {
        let n = with_session(11, &[0xFF, 0xFF], &mut m);
        op(grants, &m[..n]);
        say!(grants, "bttest: no connection in 16 s: cancelled\n");
        return None;
    }
    let l = LINK.load(SeqCst);
    let g = GRANTED.load(SeqCst);
    say!(grants, "bttest: connected in A#{} ms: status G#{:02X}, link G#{:X}; asked interval A#{}-A#{} latency A#{} timeout A#{}, granted interval A#{} ({} ms) latency A#{} timeout A#{}\n", us(now() - t0) / 1000, (l >> 16) & 0xFF, l & 0xFFFF, p[0], p[1], p[2], p[3], g & 0xFFFF, (g & 0xFFFF) as f32 * 1.25, (g >> 16) & 0xFFFF, g >> 32);
    if (l >> 16) & 0xFF != 0 {
        return None;
    }
    let handle = (l & 0xFFFF) as u16;
    let mut rest = [0u8; 6];
    rest[0..2].copy_from_slice(&handle.to_le_bytes());
    rest[2..4].copy_from_slice(&PSM.to_le_bytes());
    rest[4..6].copy_from_slice(&MTU.to_le_bytes());
    let n = with_session(12, &rest, &mut m);
    let mut r = [0u8; 8];
    let t1 = now();
    let got = call(cap(grants, S_BT), &m[..n], &mut r);
    if got != 3 || r[0] != 0 {
        say!(grants, "bttest: COC_OPEN refused\n");
        return None;
    }
    if !until(1 << 13, || CHAN.load(SeqCst) != 0) {
        say!(grants, "bttest: the channel was not answered in 8 s\n");
        state(grants);
        return None;
    }
    let c = CHAN.load(SeqCst);
    if (c >> 16) & 0x7FFF != 0 {
        return None;
    }
    say!(grants, "bttest: channel G#{:X} open in A#{} ms\n", c & 0xFFFF, us(now() - t1) / 1000);
    Some((handle, (c & 0xFFFF) as u16))
}

/// `pings` echoes of each size, one at a time: each round trip from the send's call to its echo's arrival.
fn pings(grants: u64, handle: u16, cid: u16, seq: &mut u32, pings: u32) -> bool {
    for size in [8usize, 200, 1024, MTU as usize] {
        let mut rtt = [0u64; 256];
        // WHY the min: the input's count, kept to the 256 round trips this array holds.
        let n = (pings as usize).min(rtt.len());
        for k in 0..n {
            let want = GOT.load(SeqCst) + 1;
            let t0 = now();
            if send(grants, handle, cid, *seq, size) != 0 {
                say!(grants, "bttest: SEND refused\n");
                return false;
            }
            if !until(1 << 12, || GOT.load(SeqCst) >= want) {
                say!(grants, "bttest: no echo of SDU A#{} ({} bytes) in 4 s\n", *seq, size);
                return false;
            }
            let back = unsafe { (*(&raw const BACK_AT))[*seq as usize % MOST] };
            rtt[k] = us(back - t0);
            *seq += 1;
        }
        let r = &mut rtt[..n];
        r.sort_unstable();
        say!(grants, "bttest: echo of A#{size}-byte SDUs, A#{n} one at a time: round trip min A#{} us, median A#{} us, max A#{} us\n", r[0], r[n / 2], r[n - 1]);
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut inp = [0u8; 16];
    let k = (input_len as usize).min(inp.len());
    for (i, v) in inp[..k].iter_mut().enumerate() {
        *v = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + i as u64) as *const u8) };
    }
    let flags = inp[0];
    let npings = if k > 1 { inp[1] as u32 } else { 16 };
    let bulk = if k > 3 { le16(&inp, 2) as u32 } else { 256 };
    // The accessories' connection (lumis ACCESSORIES.md "Decisions"): an interval of 7.5 to 15 ms (6 to 12), latency 0, a 1 s supervision timeout (100).
    let p = [if k > 4 { inp[4] as u16 } else { 6 }, if k > 5 { inp[5] as u16 } else { 12 }, 0, if k > 7 { le16(&inp, 6) } else { 100 }];
    // OPEN.
    let mut r = [0u8; 16];
    let n = call(cap(grants, S_BT), &[6], &mut r);
    if n != 9 || r[0] != 0 {
        say!(grants, "bttest: OPEN refused\n");
        ferros_sys::exit(1);
    }
    SESSION.store(u64::from_le_bytes(r[1..9].try_into().unwrap()), SeqCst);
    ferros_sys::thread_spawn(rx_thread, unsafe { &mut *(&raw mut RX_STACK) }, grants);
    let mut m = [0u8; 64];
    // An active scan until the advertiser is heard.
    let n = with_session(8, &[1], &mut m);
    let t0 = now();
    if op(grants, &m[..n]) != 0 {
        say!(grants, "bttest: SCAN refused\n");
        ferros_sys::exit(1);
    }
    if !until(1 << 15, || FOUND.load(SeqCst) != 0) {
        say!(grants, "bttest: nothing with G#FFFF manufacturer data heard in 32 s\n");
        ferros_sys::exit(1);
    }
    let found = FOUND.load(SeqCst).to_le_bytes();
    say!(grants, "bttest: found in A#{} ms\n", us(now() - t0) / 1000);
    let n = with_session(9, &[], &mut m);
    op(grants, &m[..n]);
    let Some((handle, cid)) = link_up(grants, found[6], &found[..6], p) else { ferros_sys::exit(1) };
    let mut seq = 0u32;
    if !pings(grants, handle, cid, &mut seq, npings) {
        ferros_sys::exit(1);
    }
    // The run: 2 KiB SDUs back to back, each send's call answered once its K-frames are in the transport; the rate is the bytes echoed back over the time from the first send to the last echo (each way at once).
    // WHY the min: the input's count, kept to the sequence numbers `BACK_AT` holds.
    let bulk = bulk.min(MOST as u32 - seq);
    let want = GOT.load(SeqCst) + bulk;
    let bytes0 = GOT_BYTES.load(SeqCst);
    let t0 = now();
    for _ in 0..bulk {
        if send(grants, handle, cid, seq, MTU as usize) != 0 {
            say!(grants, "bttest: SEND refused in the run\n");
            ferros_sys::exit(1);
        }
        seq += 1;
    }
    let sent = now();
    if !until(1 << 15, || GOT.load(SeqCst) >= want) {
        say!(grants, "bttest: the run: A#{} of A#{} echoed in 32 s\n", GOT.load(SeqCst) + bulk - want, bulk);
    }
    let t = us(now() - t0);
    let b = GOT_BYTES.load(SeqCst) - bytes0;
    say!(grants, "bttest: run of A#{bulk} SDUs of A#{MTU} bytes: sent in A#{} ms, all echoed in A#{} ms: A#{} bytes back, A#{} kB/s each way; bad A#{}, out of order A#{}\n", us(sent - t0) / 1000, t / 1000, b, b * 1000 / (t | 1), BAD.load(SeqCst), OUT_OF_ORDER.load(SeqCst));
    if flags & 1 != 0 {
        say!(grants, "bttest: ending with the link up (the driver must take it down)\n");
        ferros_sys::exit(0);
    }
    // Disconnect, then the same again.
    let n = with_session(11, &handle.to_le_bytes(), &mut m);
    let t0 = now();
    op(grants, &m[..n]);
    until(1 << 12, || GONE.load(SeqCst) != 0);
    say!(grants, "bttest: disconnected in A#{} ms\n", us(now() - t0) / 1000);
    if flags & 2 == 0 {
        let Some((handle, cid)) = link_up(grants, found[6], &found[..6], p) else { ferros_sys::exit(1) };
        if !pings(grants, handle, cid, &mut seq, 4) {
            ferros_sys::exit(1);
        }
        let n = with_session(11, &handle.to_le_bytes(), &mut m);
        op(grants, &m[..n]);
        until(1 << 12, || GONE.load(SeqCst) != 0);
    }
    let n = with_session(15, &[], &mut m);
    op(grants, &m[..n]);
    state(grants);
    say!(grants, "bttest: done: bad A#{}, out of order A#{}, RSSI A#{} dBm\n", BAD.load(SeqCst), OUT_OF_ORDER.load(SeqCst), FOUND_RSSI.load(SeqCst) as i32);
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

ferros_sys::entry!(payload_main);
