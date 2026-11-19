//! rawbench: the RAW video shooter's measures on the device (docs/RAW-VIDEO.md, owner 2026-11-19: "a RAW video shooter on ferros, graded later, lossless if at all possible").
//! Development, headless: nothing on the screen moves, nothing is stored.
//!
//! `ferros-bridge spawn rawbench.bin <hex input> --name rawbench --blocks rawbench`
//! Grant slots: 0 console, 1 the `rawbench` block window (process.rs WINDOWS: 32 GiB from 32 GiB into the ferros partition, where no code keeps anything).
//! Its input, bytes in hex:
//! - +0 the job: 0 sequential writes, 1 sequential reads, 2 writes then reads over what was written;
//! - +1 the blocks a call as a power of two (0: 2^8, one UFS command of 1 MiB, the most SYS_BLOCK moves);
//! - +2 the seconds of each run (0: 64);
//! - +3 the reads' span as a power of two of blocks (0: what the writes covered, else 2^22 for a run of reads alone);
//! - +4 the cores: 0 the A715s (G#00F0), 1 the A510s (G#000F), 2 the X3 (G#0100).
//! What it says: each second's MB/s, the calls' time (most, mean) and the share inside the UFS command (the kernel's timed SYS_BLOCK: from the call's start to the command's end, the copy in included for a write); at the end the whole run's rate and the calls by time (a power of two of us each).

use std::time::Instant;

const SYS_BLOCK: u64 = 29;
const SYS_PIN: u64 = 20;
const SYS_INPUT: u64 = 8;
const S_BLOCKS: u64 = 1;
const ERR: u64 = u64::MAX;
/// The window's blocks (process.rs WINDOWS `rawbench`): 2^23 of 4 KiB, 32 GiB.
const WINDOW_LOG2: u32 = 23;

fn arc_base() -> u64 {
    let b: u64;
    unsafe { core::arch::asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
    b
}

fn off<T: ?Sized>(p: *const T) -> u64 {
    (p as *const u8 as u64).wrapping_sub(arc_base())
}

fn cap(slot: u64) -> u64 {
    0x204 + 32 * slot
}

fn now() -> u64 {
    let t: u64;
    unsafe { core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) t, options(nomem, nostack, preserves_flags)) };
    t
}

fn freq() -> u64 {
    let f: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) f, options(nomem, nostack, preserves_flags)) };
    f
}

fn pin(mask: u64) -> u64 {
    let r: u64;
    unsafe { core::arch::asm!("svc #0", inlateout("x0") mask => r, in("x1") 0u64, in("x2") 0u64, in("x3") 0u64, in("x4") 0u64, in("x8") SYS_PIN, options(nostack)) };
    r
}

/// The whole spawn input (the hex bytes, then any `--input-file` framed as a u32 LE length and its bytes), a MiB a call.
fn input() -> Vec<u8> {
    let mut all = Vec::new();
    let mut b = vec![0u8; 1 << 20];
    loop {
        let n: u64;
        unsafe { core::arch::asm!("svc #0", inlateout("x0") all.len() as u64 => n, in("x1") off(b.as_mut_ptr()), in("x2") b.len() as u64, in("x3") 0u64, in("x4") 0u64, in("x8") SYS_INPUT, options(nostack)) };
        all.extend_from_slice(&b[..n as usize]);
        if (n as usize) < b.len() {
            return all;
        }
    }
}

/// One timed SYS_BLOCK call: (0 or ERR, the ticks from the call's start to the UFS command's end, the ticks after it).
fn block(write: bool, at: u64, buf: &mut [u8]) -> (u64, u64, u64) {
    let op = write as u64 | 2 | ((buf.len() as u64 >> 12) & 0xFFFF) << 8;
    let (r, a, b): (u64, u64, u64);
    unsafe { core::arch::asm!("svc #0", inlateout("x0") cap(S_BLOCKS) => r, inlateout("x1") op => a, inlateout("x2") at => b, in("x3") off(buf.as_mut_ptr()), in("x4") 0u64, in("x8") SYS_BLOCK, options(nostack)) };
    (r, a, b)
}

/// A run: calls of `buf` from block 0 of the window for `secs` seconds, the block number wrapping at `span` blocks; a line a second and the run's summary. Returns the blocks moved.
fn run(write: bool, buf: &mut [u8], secs: u64, span: u64) -> u64 {
    let what = if write { "write" } else { "read" };
    let f = freq();
    let n = (buf.len() >> 12) as u64;
    let us = |t: u64| t * 1_000_000 / f;
    let start = Instant::now();
    let t_start = now();
    let (mut at, mut moved, mut calls) = (0u64, 0u64, 0u64);
    // This second's: calls, ticks in calls, ticks to the command's end, the longest call.
    let (mut s_calls, mut s_ticks, mut s_cmd, mut s_most) = (0u64, 0u64, 0u64, 0u64);
    // The calls by their time: bucket k counts calls of 2^k to 2^(k+1) us (64 buckets: every u64 of us has its place).
    let mut hist = [0u64; 1 << 6];
    let mut second = 1u64;
    let mut s_t0 = t_start;
    let mut sum_cmd = 0u64;
    while now() - t_start < secs * f {
        // A fresh word at the head of every block written, so no two writes carry the same bytes.
        if write {
            for k in 0..n as usize {
                buf[k << 12..(k << 12) + 8].copy_from_slice(&(at + k as u64).to_le_bytes());
            }
        }
        let t0 = now();
        let (r, cmd, _) = block(write, at, buf);
        let t = now() - t0;
        if r == ERR {
            println!("rawbench: {what} refused at block A#{at} after A#{calls} calls");
            return moved;
        }
        calls += 1;
        moved += n;
        s_calls += 1;
        s_ticks += t;
        s_cmd += cmd;
        sum_cmd += cmd;
        s_most = s_most.max(t);
        hist[(63 - (us(t) | 1).leading_zeros()) as usize] += 1;
        at = (at + n) & (span - 1);
        if now() - t_start >= second * f {
            println!(
                "rawbench: {what} second A#{second}: A#{} MB/s (10^6 bytes), A#{s_calls} calls, A#{} us a call (most A#{}), the command's share A#{} of 2^8",
                ((s_calls * n) << 12) * f / (now() - s_t0) / 1_000_000,
                us(s_ticks) / s_calls.max(1),
                us(s_most),
                (s_cmd << 8) / s_ticks.max(1)
            );
            second += 1;
            s_t0 = now();
            (s_calls, s_ticks, s_cmd, s_most) = (0, 0, 0, 0);
        }
    }
    let el = start.elapsed().as_micros() as u64;
    println!(
        "rawbench: {what} done: A#{} MiB in A#{} ms, A#{} MB/s (10^6 bytes), A#{calls} calls of A#{} KiB, to the command's end A#{} us a call",
        (moved << 12) >> 20,
        el / 1000,
        (moved << 12) / el.max(1),
        n << 2,
        us(sum_cmd) / calls.max(1)
    );
    let line: String = hist.iter().enumerate().filter(|(_, c)| **c != 0).map(|(k, c)| format!(" 2^A#{k} us: A#{c}")).collect();
    println!("rawbench: {what} calls by time:{line}");
    moved
}

/// The queued calls (SYS_BLOCK op bit 2, development): `cmds` commands of `buf` at consecutive places from `at`, `depth` in flight. (0 or ERR, the ticks from the first doorbell to the last completion, the commands completed well.)
fn block_queued(write: bool, at: u64, buf: &mut [u8], cmds: u64, depth: u64) -> (u64, u64, u64) {
    let op = write as u64 | 4 | ((buf.len() as u64 >> 12) & 0xFFFF) << 8;
    let (r, a, b): (u64, u64, u64);
    unsafe { core::arch::asm!("svc #0", inlateout("x0") cap(S_BLOCKS) => r, inlateout("x1") op => a, inlateout("x2") at => b, in("x3") off(buf.as_mut_ptr()), in("x4") cmds | depth << 32, in("x8") SYS_BLOCK, options(nostack)) };
    (r, a, b)
}

/// A queued run: calls of `cmds` commands each, `depth` in flight, for `secs` seconds from block 0, wrapping at the window's end; a line a second and the summary.
fn run_queued(write: bool, buf: &mut [u8], secs: u64, cmds: u64, depth: u64) {
    let what = if write { "queued write" } else { "queued read" };
    let f = freq();
    let n = (buf.len() >> 12) as u64;
    let window = 1u64 << WINDOW_LOG2;
    let t_start = now();
    let (mut at, mut moved, mut dev) = (0u64, 0u64, 0u64);
    let (mut s_bytes, mut s_t0, mut s_dev, mut second) = (0u64, t_start, 0u64, 1u64);
    while now() - t_start < secs * f {
        if at + cmds * n > window {
            at = 0;
        }
        let (r, ticks, ok) = block_queued(write, at, buf, cmds, depth);
        if r == ERR {
            println!("rawbench: {what} refused at block A#{at}: A#{ok} of A#{cmds} commands well");
            return;
        }
        at += cmds * n;
        moved += cmds * n;
        dev += ticks;
        s_bytes += (cmds * n) << 12;
        s_dev += ticks;
        if now() - t_start >= second * f {
            println!("rawbench: {what} depth A#{depth} second A#{second}: A#{} MB/s (10^6 bytes), at the device A#{} MB/s", s_bytes * f / (now() - s_t0) / 1_000_000, s_bytes * f / s_dev.max(1) / 1_000_000);
            second += 1;
            (s_bytes, s_t0, s_dev) = (0, now(), 0);
        }
    }
    let el = (now() - t_start) * 1_000_000 / f;
    println!("rawbench: {what} depth A#{depth} done: A#{} MiB in A#{} ms, A#{} MB/s, at the device A#{} MB/s, A#{} KiB a command", (moved << 12) >> 20, el / 1000, (moved << 12) / el.max(1), (moved << 12) * f / dev.max(1) / 1_000_000, n << 2);
}

const SYS_CALL: u64 = 1;
const SYS_DMA: u64 = 7;
/// The camera endpoint's slot when the job is the camera's (`--use camera` its only grant).
const S_CAMERA: u64 = 1;

fn call(slot: u64, msg: &[u8], reply: &mut [u8]) -> u64 {
    let r: u64;
    unsafe { core::arch::asm!("svc #0", inlateout("x0") cap(slot) => r, in("x1") off(msg.as_ptr()), in("x2") msg.len() as u64, in("x3") off(reply.as_mut_ptr()), in("x4") reply.len() as u64, in("x8") SYS_CALL, options(nostack)) };
    r
}

fn dma(token: &[u8; 32], op: u64) -> u64 {
    let r: u64;
    unsafe { core::arch::asm!("svc #0", inlateout("x0") off(token.as_ptr()) => r, in("x1") op, in("x2") 0u64, in("x3") 0u64, in("x4") 0u64, in("x8") SYS_DMA, options(nostack)) };
    r
}

fn set(slot: u64, what: u8, v: u64) -> u8 {
    let mut m = [3u8, what, 0, 0, 0, 0, 0, 0, 0, 0];
    m[2..10].copy_from_slice(&v.to_le_bytes());
    let mut a = [0xEEu8; 1];
    let n = call(slot, &m, &mut a);
    if n == ERR { 0xEE } else { a[0] }
}

/// The camera's job: the main opened with the view (the mode block's kind `kind`: 0 the init's binned readout, 2 the 1:1), `settle` frames let pass, the next held and copied out whole; its place in the view said for the host (`ferros-bridge read`, 64 KiB a read, the driver's region line giving the physical base) and held `hold` seconds, STATS every 2 s so the driver keeps the open; then CLOSE. The copy (the frame as the front end wrote it, MIPI RAW10 packed, the view's stride) and its shape back.
fn camera(kind: u8, settle: u32, hold: u64, exp_log2: u8, gain_log2: u8) -> Option<(Vec<u8>, usize, usize, usize)> {
    let mut r = [0u8; 64];
    let open = [1u8, 0, 1, kind];
    let n = call(S_CAMERA, &open[..if kind == 0 { 3 } else { 4 }], &mut r);
    println!("rawbench: OPEN kind A#{kind} with the view answered G#{:X}, A#{} bytes", if n == ERR { 0xEEE } else { r[0] as u64 }, if n == ERR { 0 } else { n });
    if n == ERR || r[0] != 0 || n < 48 {
        return None;
    }
    let mut tok = [0u8; 32];
    tok.copy_from_slice(&r[1..33]);
    let base = dma(&tok, 0);
    let size = dma(&tok, 1);
    let (w, h) = (u16::from_le_bytes([r[36], r[37]]) as usize, u16::from_le_bytes([r[38], r[39]]) as usize);
    let stride = u32::from_le_bytes(r[40..44].try_into().unwrap()) as usize;
    println!("rawbench: the view at G#{base:X}, A#{} MiB, a ring of A#{}, A#{w} x A#{h}, A#{stride} bytes a line, format A#{}", if size == ERR { 0 } else { size >> 20 }, r[33], r[34]);
    if base == ERR {
        let _ = call(S_CAMERA, &[4], &mut r);
        return None;
    }
    // WHY the mins: the bytes are the person's; 2^14 ms and 2^7 times unity are past the main's reach, and a larger shift would leave the u64 (the gain's byte is 1 for unity: never 0 here, so the subtraction holds).
    if exp_log2 != 0 {
        println!("rawbench: SET exposure A#{} ms answered A#{}", 1u64 << exp_log2.min(14), set(S_CAMERA, 0, 1_000_000u64 << exp_log2.min(14)));
    }
    if gain_log2 != 0 {
        println!("rawbench: SET gain A#{} times unity answered A#{}", 1u64 << (gain_log2.min(8) - 1), set(S_CAMERA, 1, 1024u64 << (gain_log2.min(8) - 1)));
    }
    let mut held = 0xFFu8;
    let mut hd = [0u8; 64];
    for k in 0..=settle {
        let n = call(S_CAMERA, &[5, held], &mut hd);
        if n == ERR || n < 64 {
            println!("rawbench: MAP A#{k}: no frame (G#{n:X})");
            let _ = call(S_CAMERA, &[4], &mut r);
            return None;
        }
        held = hd[40];
    }
    let w8 = |k: usize| u64::from_le_bytes(hd[k..k + 8].try_into().unwrap());
    let at = base as usize + w8(48) as usize;
    let t = Instant::now();
    let mut frame = vec![0u8; h * stride];
    // SAFETY: the view holds the ring's buffers (`size` bytes from `base`, the driver's answer), and this buffer's h lines of `stride` bytes from its offset are inside it; held, so the front end does not write it meanwhile.
    unsafe { core::ptr::copy_nonoverlapping(at as *const u8, frame.as_mut_ptr(), h * stride) };
    println!(
        "rawbench: HELD frame A#{} buffer A#{held} at view offset G#{:X}: exposure_ns A#{} gain A#{} eagle_ns A#{}, copied out in A#{} ms; its first bytes {:02X?}",
        u32::from_le_bytes(hd[0..4].try_into().unwrap()),
        w8(48),
        w8(16),
        u32::from_le_bytes(hd[24..28].try_into().unwrap()),
        w8(32),
        t.elapsed().as_millis(),
        &frame[..16]
    );
    let f = freq();
    let start = now();
    let mut st = [0u8; 256];
    while now() - start < hold * f {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = call(S_CAMERA, &[8], &mut st);
    }
    let n = call(S_CAMERA, &[4], &mut r);
    println!("rawbench: CLOSE G#{:X}", if n == ERR { 0xEEE } else { r[0] as u64 });
    Some((frame, w, h, stride))
}

/// The lossless codec's prototype (tools/raw-codec) on this device's cores: the frame's bands of 2^6 rows split over `threads` threads pinned to `mask`, `runs` encodes, the coded bytes and the best and median times.
fn encode_on(frame: &[u8], stride: usize, w: usize, h: usize, cfa: raw_codec::Cfa, threads: usize, mask: u64, runs: usize) -> (usize, u64, u64) {
    const BAND: usize = 1 << 6;
    let bands = h.div_ceil(BAND);
    let mut times = Vec::new();
    let mut coded = 0;
    for _ in 0..runs {
        let t = Instant::now();
        coded = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..threads)
                .map(|k| {
                    sc.spawn(move || {
                        pin(mask);
                        let mut work = Box::new(raw_codec::Work::new());
                        let mut out = vec![0u8; raw_codec::max_encoded_len(w, BAND)];
                        let mut sum = 0usize;
                        // This thread's bands: a contiguous share, so each reads its own stretch of the frame.
                        let (b0, b1) = (bands * k / threads, bands * (k + 1) / threads);
                        for b in b0..b1 {
                            let y = b * BAND;
                            let rows = BAND.min(h - y);
                            sum += raw_codec::encode_band(&frame[y * stride..], stride, w, rows, cfa, &raw_codec::Params::DEFAULT, None, &mut work, &mut out) + 4;
                        }
                        sum
                    })
                })
                .collect();
            hs.into_iter().map(|j| j.join().unwrap()).sum()
        });
        times.push(t.elapsed().as_micros() as u64);
    }
    times.sort();
    (coded, times[0], times[times.len() >> 1])
}

/// Every band decoded on one core and compared with the frame's own samples: the device's build of the codec checked bit for bit.
fn verify_on(frame: &[u8], stride: usize, w: usize, h: usize, cfa: raw_codec::Cfa) -> bool {
    const BAND: usize = 1 << 6;
    let mut work = Box::new(raw_codec::Work::new());
    let mut out = vec![0u8; raw_codec::max_encoded_len(w, BAND)];
    let mut dst = vec![0u16; w * BAND];
    let mut y = 0;
    while y < h {
        let rows = BAND.min(h - y);
        let n = raw_codec::encode_band(&frame[y * stride..], stride, w, rows, cfa, &raw_codec::Params::DEFAULT, None, &mut work, &mut out);
        let used = raw_codec::decode_band(&out[..n], w, rows, cfa, &raw_codec::Params::DEFAULT, &mut work, &mut dst, w);
        if used != n {
            println!("rawbench: verify: band at row A#{y} decoded A#{used} of A#{n} bytes");
            return false;
        }
        for r in 0..rows {
            let line = &frame[(y + r) * stride..];
            for x in 0..w {
                let g = x >> 2;
                let v = (line[g * 5 + (x & 3)] as u16) << 2 | (line[g * 5 + 4] as u16 >> ((x & 3) << 1)) & 3;
                if dst[r * w + x] != v {
                    println!("rawbench: verify: row A#{} column A#{x}: A#{} back for A#{v}", y + r, dst[r * w + x]);
                    return false;
                }
            }
        }
        y += rows;
    }
    true
}

/// The codec's measures on a frame: verified once, then encoded on one A715, four A715s, one A510, four A510s.
fn codec(frame: &[u8], stride: usize, w: usize, h: usize, cfa: raw_codec::Cfa, runs: usize) {
    let mut mean = 0u64;
    for y in (0..h).step_by(1 << 4) {
        for x in (0..w * 5 / 4).step_by(65) {
            mean += frame[y * stride + x] as u64;
        }
    }
    let samples = (h >> 4) as u64 * (w * 5 / 4).div_ceil(65) as u64;
    println!("rawbench: codec on A#{w} x A#{h} ({cfa:?}), mean high byte A#{} (about A#{} DN)", mean / samples, (mean << 2) / samples);
    let t = Instant::now();
    println!("rawbench: codec verify: {} in A#{} ms", if verify_on(frame, stride, w, h, cfa) { "every sample back exact" } else { "MISMATCH" }, t.elapsed().as_millis());
    let packed = (w * h * 10 / 8) as u64;
    for (what, threads, mask) in [("one A715", 1, 0x0010u64), ("four A715s", 4, 0x00F0), ("one A510", 1, 0x0001), ("four A510s", 4, 0x000F)] {
        let (coded, best, med) = encode_on(frame, stride, w, h, cfa, threads, mask, runs);
        println!(
            "rawbench: codec {what}: A#{coded} bytes (A#{:.3} bits a pixel, ratio A#{:.3} to packed RAW10), best A#{} us, median A#{} us, A#{} Mpix/s a frame's best",
            coded as f64 * 8. / (w * h) as f64,
            packed as f64 / coded as f64,
            best,
            med,
            (w * h) as u64 / best.max(1)
        );
    }
}

/// A clip's worth of encoding: a frame every 1/`fps` s for `secs` s, each on the four A715s; a line a second with the frames done, those that ran past their period, and the longest.
fn sustain(frame: &[u8], stride: usize, w: usize, h: usize, cfa: raw_codec::Cfa, secs: u64, fps: u64) {
    let f = freq();
    let period = f / fps;
    let t0 = now();
    let mut next = t0;
    let (mut done, mut late, mut most, mut second) = (0u64, 0u64, 0u64, 1u64);
    while now() - t0 < secs * f {
        let s = now();
        let _ = encode_on(frame, stride, w, h, cfa, 4, 0x00F0, 1);
        let took = now() - s;
        done += 1;
        late += (took > period) as u64;
        most = most.max(took);
        next += period;
        while now() < next {
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
        if now() - t0 >= second * f {
            println!("rawbench: sustain second A#{second}: A#{done} frames, A#{late} past their period, the longest A#{} us", most * 1_000_000 / f);
            (done, late, most) = (0, 0, 0);
            second += 1;
        }
    }
}

fn main() {
    let inp = input();
    let arg = |k: usize| inp.get(k).copied().unwrap_or(0);
    let job = arg(0);
    // WHY the min: the byte is the person's, and SYS_BLOCK moves at most 2^8 blocks a call.
    let log = if arg(1) == 0 { 8 } else { arg(1).min(8) } as u32;
    let secs = if arg(2) == 0 { 64 } else { arg(2) } as u64;
    let mask = match arg(4) {
        1 => 0x000F,
        2 => 0x0100,
        _ => 0x00F0,
    };
    pin(mask);
    println!("rawbench: job A#{job}, A#{} KiB a call, A#{secs} s a run, cores G#{mask:04X}, the window A#{} GiB", 4u64 << log, 1u64 << (WINDOW_LOG2 - 18));
    let mut buf = vec![0u8; 4096 << log];
    // Incompressible bytes (xorshift), so no device could store less than it was given.
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for w in buf.chunks_exact_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        w.copy_from_slice(&x.to_le_bytes());
    }
    let window = 1u64 << WINDOW_LOG2;
    match job {
        0 => {
            run(true, &mut buf, secs, window);
        }
        1 => {
            let span = 1u64 << if arg(3) == 0 { 22 } else { (arg(3) as u32).min(WINDOW_LOG2) };
            run(false, &mut buf, secs, span);
        }
        2 => {
            let wrote = run(true, &mut buf, secs, window);
            // The reads over what the writes covered, rounded down to a power of two so the wrap is a mask.
            let span = if arg(3) == 0 { 1u64 << (63 - (wrote | 1).leading_zeros()).min(WINDOW_LOG2) } else { 1u64 << (arg(3) as u32).min(WINDOW_LOG2) };
            println!("rawbench: reading back over A#{} MiB", span >> 8);
            run(false, &mut buf, secs, span);
        }
        4 | 5 => {
            // Queued (op bit 2): +5 the depth (0: 4), +6 the commands a call as a power of two (0: 2^6, 64 MiB at 1 MiB a command).
            let depth = if arg(5) == 0 { 4 } else { arg(5) } as u64;
            let cmds = 1u64 << if arg(6) == 0 { 6 } else { arg(6).min(12) };
            run_queued(job == 4, &mut buf, secs, cmds, depth);
        }
        6 => {
            // The codec on a frame from the host: the hex input is 13 bytes (+5 the period, 2 or 4; +6 width u16, +8 height u16, +10 stride u16, +12 the runs, 0: 4), then `--input-file` the packed frame.
            let le = |k: usize| u16::from_le_bytes([arg(k), arg(k + 1)]) as usize;
            let (w, h, stride) = (le(6), le(8), le(10));
            let runs = if arg(12) == 0 { 4 } else { arg(12) as usize };
            // The file's frame follows the hex bytes, after its u32 length.
            let at = 13 + 4;
            if inp.len() < at + h * stride || w == 0 {
                println!("rawbench: codec: the input is short (A#{} bytes in all for A#{w} x A#{h} at A#{stride})", inp.len());
            } else {
                codec(&inp[at..at + h * stride], stride, w, h, if arg(5) == 4 { raw_codec::Cfa::Quad } else { raw_codec::Cfa::Bayer }, runs);
            }
        }
        3 => {
            // The camera's: +5 the kind (0 binned, 2 the 1:1), +6 frames let pass (0: 16), +7 the hold as 2^n s for the host to read the frame (0: none), +8 the exposure 2^n ms, +9 the gain 2^(n-1) times unity (0: the driver's own, 1: unity).
            let settle = if arg(6) == 0 { 16 } else { arg(6) as u32 };
            let hold = if arg(7) == 0 { 0 } else { 1u64 << arg(7).min(12) };
            // +10 the codec's runs (0: none).
            if let Some((frame, w, h, stride)) = camera(arg(5), settle, hold, arg(8), arg(9)) {
                println!("rawbench: kept A#{} bytes, A#{w} x A#{h}, stride A#{stride}", frame.len());
                let cfa = if arg(5) == 2 { raw_codec::Cfa::Quad } else { raw_codec::Cfa::Bayer };
                if arg(10) != 0 {
                    codec(&frame, stride, w, h, cfa, arg(10) as usize);
                }
                // +11 a sustained run's seconds (0: none) at +12 frames a second, each frame encoded on the four A715s: the heat and the clock over a clip's length (the host reads `diag thermal` meanwhile).
                if arg(11) != 0 && arg(12) != 0 {
                    sustain(&frame, stride, w, h, cfa, arg(11) as u64, arg(12) as u64);
                }
            }
        }
        _ => println!("rawbench: no job A#{job}"),
    }
    println!("rawbench: ends");
}
