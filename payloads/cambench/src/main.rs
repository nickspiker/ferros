//! cambench: how fast the rear cameras give frames and slits, and how fast the cores add them (owner, 2026-11-15: "straight as fast as possible full frame 1:1 and 2x binned for all back cams integrating into the buffer. straight add/accumulate and see what numbers we can hit. Then I want the slitscan wired up ... how fast can we capture?"; docs/CAMERA.md "Modes, the bench and the slit"). Development, headless: nothing on the screen moves.
//!
//! ferros-bridge spawn cambench.bin <hex input> --name cambench --use camera
//! Its input, bytes given in hex (u16 little-endian):
//! - +0 the camera (ferros_cam cams: 0 main, 1 ultrawide, 3 tele), +1 the mode kind (0 the init's, 1 binned with its frame cut, 2 1:1, 3 binned slit, 4 1:1 slit),
//! - +2 a slit's rows (u16), +4 the frame in lines (u16; 0 the driver's default, FFFF the mode's own), +6 the integration in lines (u16; 0 the driver's 16),
//! - +8 the driver's flags (payloads/camera `F_*`: 1 RAW12, 2 mid-stream, 4 PD path, 8 geometry everywhere, 16 slim program, 32 slim level 2, 64 front end left as captured), +9 a slit ring's buffers as a power of two (0: 2^8), +10 the line length (u16; 0 the mode's),
//! - +12 the work per frame: 0 map only (the capture's ceiling), 1 read every line out of the view (64-byte loads), 2 read and add into u16 sums folded into u32 every 64 frames, 3 read and add into u32 sums,
//! - +13 frames (or slits) in 16s (0: 64), +14 a sweep (0 none; 1 the slit's rows from 64 down to its cell, a new open each; 2 the frame length from the mode's own down, a new open each; 3 the line length from 6320 clocks down, a new open each), +15 threads for whole frames (0: the four A715s),
//! - optionally +16 the window's output columns across the middle (u16; 0 the mode's width; on 2^4), +18 the frames' count moved left this many bits (to 12: long slit runs, the ring's drain).
//! The driver's flag 128 keeps the sensor's phase detect and embedded data on in a windowed mode (off by default there: docs/CAMERA.md "The slit for real").
//! A windowed mode on the main also runs detached and slim by default (the statistics pipeline off, loch0's paths alone each frame: docs/CAMERA.md "The slit runs"); the driver's eighth hand-spawn byte overrides it for an A/B.
//! The sweeps (+14) open again in this process: with the vault's driver each open starts a fresh one; a driver spawned by hand serves one stream and refuses the next open, so a sweep of a hand-spawned driver is tools/cambench-sweep.sh (one driver an open).
//! e.g. `00030400000000000000000002080000`: the main, a binned slit of 4 rows, 128 slits added into u16; `00010000000000000000000002040000`: the main binned at its default cut frame, 64 frames added (every byte of the 16 written, little-endian halves: rows 4 is `0400`).
//!
//! What it says, per open: the driver's answer and the view's shape; the sensor's data formats and its mode registers read back (CAPS); the sensor's own frame rate against the front end's (RATE); per frame the read and add time on each thread, frames skipped, the cores used and left; the driver's counters (STATS); the link's and the bus's bytes a second.
#![no_std]
#![no_main]

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_cam::raw;
use ferros_sys::{call, cap, console, ERR};

struct Line {
    b: [u8; 256],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // WHY the min: a formatted line longer than the 256-byte buffer would index past it and panic the process; the console gets the line cut instead.
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line { b: [0; 256], n: 0 };
        let _ = write!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

const OP_OPEN: u8 = 1;
const OP_CLOSE: u8 = 4;
const OP_MAP: u8 = 5;
const OP_STATS: u8 = 8;
const OP_SLITS: u8 = 9;
const OP_RATE: u8 = 10;
const OP_CAPS: u8 = 11;
const K_SLIT_FULL: u8 = 4;

/// The heap's ceiling (the image header's: the kernel sizes the arc's window for it, `ferros_sys::arc::grow` maps frames as asked): the main's 1:1 sums, 8192 x 6144 samples of u16 and u32, are 302 MB.
const CEILING: usize = 1 << 30;
/// The longest line a view gives (the 8a's 1:1 main RAW12 would be 13872 bytes): a thread's copy of one, and the slack its last block reads past.
const LINE_MOST: usize = 1 << 14;
/// The bench's own slit ring (Lumis's slitscan keeps a ring of slices): 2^10 slits unpacked to 16 bits.
const SLIT_KEEP: usize = 1 << 10;
const THREADS_MOST: usize = 4;

/// What one open is asked to do.
#[derive(Clone, Copy)]
struct Args {
    id: u8,
    kind: u8,
    rows: u16,
    fll: u16,
    exposure: u16,
    flags: u8,
    ring: u8,
    llp: u16,
    work: u8,
    frames: u32,
    sweep: u8,
    cols: u16,
}

impl Args {
    fn open_msg(&self) -> [u8; 16] {
        let mut m = [0u8; 16];
        m[0] = OP_OPEN;
        m[1] = self.id;
        m[2] = 1;
        m[3] = self.kind;
        m[4..6].copy_from_slice(&self.rows.to_le_bytes());
        m[6..8].copy_from_slice(&self.fll.to_le_bytes());
        m[8..10].copy_from_slice(&self.exposure.to_le_bytes());
        m[10] = self.flags;
        m[11] = self.ring;
        m[12..14].copy_from_slice(&self.llp.to_le_bytes());
        m[14..16].copy_from_slice(&self.cols.to_le_bytes());
        m
    }
}

/// The view an open gave (payloads/camera `VIEW_REPLY_MODE`).
#[derive(Clone, Copy)]
struct View {
    base: usize,
    ring: u32,
    format: u8,
    w: usize,
    h: usize,
    line: usize,
    buf: usize,
    slit: bool,
}

// ---------------------------------------------------------------- the workers

/// A frame handed to the workers: its count (a new one each frame, their doorbell), where its samples start in the view, and how many have finished it.
static JOB: AtomicU64 = AtomicU64::new(0);
static SRC: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
/// The shape the workers work to, set before the first frame: rows, a line's bytes, blocks a line (32 samples each), the sums' row pitch (samples), the work (`Args::work`), bits a sample, threads, the u16 and u32 sums (addresses), the frame's place in its 64 (the u16 sums fold at 63).
static ROWS: AtomicU64 = AtomicU64::new(0);
static LINE_B: AtomicU64 = AtomicU64::new(0);
static BLOCKS: AtomicU64 = AtomicU64::new(0);
static PITCH: AtomicU64 = AtomicU64::new(0);
static WORK: AtomicU64 = AtomicU64::new(0);
static BITS: AtomicU64 = AtomicU64::new(10);
static THREADS: AtomicU64 = AtomicU64::new(1);
static ACC16: AtomicU64 = AtomicU64::new(0);
static ACC32: AtomicU64 = AtomicU64::new(0);
static FOLD: AtomicU64 = AtomicU64::new(0);
/// Each thread's counter ticks reading lines out of the view and adding them, summed since the open.
static READ_T: [AtomicU64; THREADS_MOST] = [const { AtomicU64::new(0) }; THREADS_MOST];
static ADD_T: [AtomicU64; THREADS_MOST] = [const { AtomicU64::new(0) }; THREADS_MOST];

#[repr(C, align(64))]
struct LineBuf([u8; LINE_MOST + 64]);
static mut LINES: [LineBuf; THREADS_MOST] = [const { LineBuf([0; LINE_MOST + 64]) }; THREADS_MOST];
static mut STACKS: [[u8; 1 << 14]; THREADS_MOST - 1] = [[0; 1 << 14]; THREADS_MOST - 1];
static mut CHECK: [[u16; LINE_MOST]; 2] = [[0; LINE_MOST]; 2];

/// One line out of the view (non-cacheable: the front end does not snoop) into thread `k`'s cached copy, 64 bytes a load, as the driver and Lumis read it (8-byte loads took 121 ms a frame, 2026-11-14).
fn read_line(k: usize, at: usize, bytes: usize) -> &'static [u8] {
    let dst = unsafe { ((&raw mut LINES) as *mut LineBuf).add(k) as *mut u8 };
    for i in 0..bytes / 64 {
        // SAFETY: a line of `bytes` (a multiple of 64, every stride the driver gives) lies inside the view's buffer; the copy is `LINE_MOST + 64`, past any line.
        unsafe {
            let v = core::arch::aarch64::vld1q_u8_x4((at + i * 64) as *const u8);
            core::arch::aarch64::vst1q_u8_x4(dst.add(i * 64), v);
        }
    }
    // SAFETY: thread k's copy, `LINE_MOST + 64` bytes, past any line and its slack.
    unsafe { core::slice::from_raw_parts(((&raw const LINES) as *const LineBuf).add(k) as *const u8, bytes + 64) }
}

/// Thread `k`'s share of a frame: its quarter of the rows (whole rows side by side, so each thread's sums are one run of memory), each read out of the view and, as the work asks, added.
fn part(k: usize, src: usize) {
    let (rows, line, blocks, pitch) = (ROWS.load(Ordering::Relaxed) as usize, LINE_B.load(Ordering::Relaxed) as usize, BLOCKS.load(Ordering::Relaxed) as usize, PITCH.load(Ordering::Relaxed) as usize);
    let (work, threads) = (WORK.load(Ordering::Relaxed), THREADS.load(Ordering::Relaxed) as usize);
    let raw12 = BITS.load(Ordering::Relaxed) == 12;
    let fold = FOLD.load(Ordering::Relaxed) == 1;
    let (a16, a32) = (ACC16.load(Ordering::Relaxed) as *mut u16, ACC32.load(Ordering::Relaxed) as *mut u32);
    let (mut rt, mut at) = (0u64, 0u64);
    for y in k * rows / threads..(k + 1) * rows / threads {
        let t = ferros_sys::now();
        let l = read_line(k, src + y * line, line);
        let t2 = ferros_sys::now();
        rt += t2 - t;
        // SAFETY: row y of the sums, `pitch` samples, inside the sums the open grew (rows x pitch of each).
        let (r16, r32) = unsafe { (core::slice::from_raw_parts_mut(a16.add(y * pitch), pitch), core::slice::from_raw_parts_mut(a32.add(y * pitch), pitch)) };
        // The work is the open's, one choice a line (not a sample): RAW12 adds into the u32 sums only.
        match (work, raw12) {
            (2, false) => {
                raw::add10_u16(l, r16, blocks);
                if fold {
                    raw::fold(r16, r32);
                }
            }
            (3, false) => raw::add10_u32(l, r32, blocks),
            (2 | 3, true) => raw::add12_u32(l, r32, COLS.load(Ordering::Relaxed) as usize),
            _ => {}
        }
        at += ferros_sys::now() - t2;
    }
    READ_T[k].fetch_add(rt, Ordering::Relaxed);
    ADD_T[k].fetch_add(at, Ordering::Relaxed);
}
/// The frame's width (the RAW12 add's sample count).
static COLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn worker(k: u64) -> ! {
    // One A715 each (the cores `CORES_WORKERS` names: 4 to 7), the main thread on the first.
    ferros_sys::pin(1 << (4 + k));
    let mut seen = 0;
    loop {
        let job = JOB.load(Ordering::Acquire);
        if job == seen {
            ferros_sys::wait(&JOB, job, 0);
            continue;
        }
        seen = job;
        part(k as usize, SRC.load(Ordering::Acquire) as usize);
        DONE.fetch_add(1, Ordering::AcqRel);
        ferros_sys::wake(&DONE, 1);
    }
}

/// A frame read and added across the threads: the workers rung, this thread's share done, then theirs waited for.
fn frame(src: usize) {
    let threads = THREADS.load(Ordering::Relaxed);
    DONE.store(0, Ordering::Release);
    SRC.store(src as u64, Ordering::Release);
    JOB.fetch_add(1, Ordering::AcqRel);
    ferros_sys::wake(&JOB, threads - 1);
    part(0, src);
    loop {
        let d = DONE.load(Ordering::Acquire);
        if d == threads - 1 {
            break;
        }
        ferros_sys::wait(&DONE, d, 0);
    }
}

// ---------------------------------------------------------------- the arc's memory

/// The memory grown so far past the arc's first top: its start (an address) and bytes.
static mut GROWN: (usize, usize) = (0, 0);

/// At least `bytes` of zeroed memory past the arc's top (grown once, reused and zeroed by each open after): its address, or None when the kernel refuses (past the ceiling, too few free frames: the console says).
fn memory(bytes: usize) -> Option<usize> {
    let (start, have) = unsafe { GROWN };
    if bytes <= have {
        // WHY the zeroing: an earlier open's sums are still there; a fresh run starts from none.
        unsafe { core::ptr::write_bytes(start as *mut u8, 0, bytes) };
        return Some(start);
    }
    let (top, was) = ferros_sys::arc::grow(bytes - have).ok()?;
    let start = if have == 0 { ferros_sys::arc_base() as usize + was } else { start };
    unsafe { GROWN = (start, top - (start - ferros_sys::arc_base() as usize)) };
    unsafe { core::ptr::write_bytes(start as *mut u8, 0, have) };
    Some(start)
}

// ---------------------------------------------------------------- one open

/// The view's token, where the kernel looks for it (an arc offset).
static mut TOKEN: [u8; 32] = [0; 32];

fn u64_at(b: &[u8], k: usize) -> u64 {
    u64::from_le_bytes(b[8 * k..8 * k + 8].try_into().unwrap())
}

/// An open with the view: its shape, or None (the driver's answer said).
fn open(g: u64, ep: u64, a: &Args) -> Option<View> {
    let t = ferros_sys::now();
    let mut r = [0u8; 64];
    let n = call(ep, &a.open_msg(), &mut r);
    let ms = ferros_sys::us_since(t) / 1000;
    if n == ERR || n < 64 || r[0] != 0 {
        say!(g, "cambench: OPEN camera A#{} kind A#{} rows A#{} fll A#{} exposure A#{} flags G#{:X} answered G#{:X} in A#{ms} ms (the driver's console lines say why)\n", a.id, a.kind, a.rows, a.fll, a.exposure, a.flags, if n == ERR { 0xEEE } else { r[0] as u64 });
        return None;
    }
    let tok = unsafe { &mut *(&raw mut TOKEN) };
    tok.copy_from_slice(&r[1..33]);
    let toff = ferros_sys::off(tok.as_ptr());
    let base = ferros_sys::dma(toff);
    if base == ERR {
        say!(g, "cambench: the view's token was refused\n");
        return None;
    }
    let u16_ = |k: usize| u16::from_le_bytes([r[k], r[k + 1]]) as usize;
    let u32_ = |k: usize| u32::from_le_bytes(r[k..k + 4].try_into().unwrap());
    let v = View { base: base as usize, ring: u32_(48), format: r[34], w: u16_(36), h: u16_(38), line: u32_(40) as usize, buf: u32_(44) as usize, slit: r[52] == 1 };
    say!(g, "cambench: OPEN in A#{ms} ms: the view at G#{:X}, a{} ring of A#{} buffers of A#{} KiB, A#{} x A#{}, A#{} bytes a line, format A#{}\n", v.base, if v.slit { " slit" } else { "n image" }, v.ring, v.buf >> 10, v.w, v.h, v.line, v.format);
    Some(v)
}

/// CAPS, said: the sensor's data formats (CCS descriptors: (uncompressed << 8) | compressed), its integration limits, the mode as the sensor holds it.
fn caps(g: u64, ep: u64) {
    let mut c = [0u8; 80];
    let n = call(ep, &[OP_CAPS], &mut c);
    if n == ERR || n < 73 {
        say!(g, "cambench: CAPS gave nothing\n");
        return;
    }
    let be = |k: usize| u16::from_be_bytes([c[k], c[k + 1]]);
    let count = (c[1] & 0xF) as usize;
    let mut l = Line { b: [0; 256], n: 0 };
    let _ = write!(l, "cambench: CAPS: data format model G#{:02X} G#{:02X}, A#{count} formats:", c[0], c[1]);
    for k in 0..count {
        let _ = write!(l, " G#{:04X}", be(2 + 2 * k));
    }
    let raw12 = (0..count).any(|k| be(2 + 2 * k) == 0x0C0C);
    let _ = write!(l, "{}\n", if raw12 { " (RAW12 G#0C0C offered)" } else { " (no G#0C0C)" });
    console(g, &l.b[..l.n]);
    say!(g, "cambench: CAPS: integration least A#{} lines, margin A#{}; frame A#{} lines of A#{}; x G#{:X} to G#{:X}, y G#{:X} to G#{:X}, out A#{} x A#{}; increments A#{} A#{} A#{} A#{}; binning G#{:02X} G#{:02X} G#{:02X}; format G#{:04X}; integration A#{}; mode select G#{:X}; frame count A#{}; reads failed G#{:X}\n", be(34), be(36), be(38), be(40), be(42), be(46), be(44), be(48), be(50), be(52), be(54), be(56), be(58), be(60), c[62], c[63], c[64], be(65), be(67), c[69], c[70], u16::from_le_bytes([c[71], c[72]]));
}

/// RATE, said: the sensor's own frames a second against the front end's over the same windows. Returns both (frames a second times 16, for fractions).
fn rate(g: u64, ep: u64) -> (u64, u64) {
    let mut r = [0u8; 40];
    let n = call(ep, &[OP_RATE, 1 << 4], &mut r);
    if n == ERR || n < 40 {
        say!(g, "cambench: RATE gave nothing\n");
        return (0, 0);
    }
    let (sensor, fe, ticks, hz, fails) = (u64_at(&r, 0), u64_at(&r, 1), u64_at(&r, 2), u64_at(&r, 3), u64_at(&r, 4));
    // The span is the driver's sixteen sleeps of 2^16 us, never none, and the counter's frequency is never zero.
    let per = |f: u64| f * hz * 16 / ticks;
    say!(g, "cambench: RATE over A#{} ms: the sensor's counter A#{sensor} frames (A#{} a second), the front end A#{fe} (A#{} a second); counter reads failed A#{fails}\n", ticks * 1000 / hz, per(sensor) / 16, per(fe) / 16);
    (per(sensor), per(fe))
}

/// STATS, said.
fn stats(g: u64, ep: u64) -> [u8; 208] {
    let mut s = [0u8; 208];
    let n = call(ep, &[OP_STATS], &mut s);
    if n == ERR || n < 208 {
        say!(g, "cambench: STATS gave nothing\n");
        return s;
    }
    let hz = u64_at(&s, 10);
    // WHY the max: an open whose front end never answered (no frame) has no answers, and the mean would divide by zero and panic the bench before it says so.
    let answers = u64_at(&s, 1).max(1);
    say!(g, "cambench: STATS: A#{} frames written, A#{} answers (A#{} on the line{}), the frame thread A#{} us an answer on average, A#{} us the most; slit frames not where the ring put them A#{}, ring frames unseen A#{}; the long ring A#{} descriptors{}; registers refused A#{}; image status bits G#{:X}\n", u64_at(&s, 0), u64_at(&s, 1), u64_at(&s, 2), if u64_at(&s, 24) == 1 { ", the rest on the tick" } else { "" }, u64_at(&s, 3) * 1_000_000 / hz / answers, u64_at(&s, 4) * 1_000_000 / hz, u64_at(&s, 5), u64_at(&s, 6), u64_at(&s, 7), if u64_at(&s, 8) == 1 { ", its last given" } else { "" }, u64_at(&s, 11), u64_at(&s, 12));
    say!(g, "cambench: STATS: A#{} x A#{} at A#{} bytes a line, buffers of A#{} KiB, slit ring A#{}, frame A#{} lines of A#{}, integration A#{} lines, A#{} bits, slim A#{}, PD path A#{}\n", u64_at(&s, 14), u64_at(&s, 13), u64_at(&s, 15), u64_at(&s, 16) >> 10, u64_at(&s, 17), u64_at(&s, 18), u64_at(&s, 19), u64_at(&s, 20), u64_at(&s, 21), u64_at(&s, 22), u64_at(&s, 23));
    s
}

fn close(g: u64, ep: u64) {
    let mut c = [0u8; 4];
    let n = call(ep, &[OP_CLOSE], &mut c);
    if n == ERR || c[0] != 0 {
        say!(g, "cambench: CLOSE answered G#{:X}\n", if n == ERR { 0xEEE } else { c[0] as u64 });
    }
}

/// Whole frames: MAP each, read and add it across the threads, and say what it cost. True when the stream ran.
fn frames(g: u64, ep: u64, a: &Args, v: &View, sensor16: u64) -> bool {
    let blocks = raw::blocks(v.w);
    let pitch = blocks * raw::BLOCK_SAMPLES;
    let (n16, n32) = (v.h * pitch * 2, v.h * pitch * 4);
    let Some(mem) = memory(n16 + n32) else {
        say!(g, "cambench: no memory for the sums (A#{} MiB)\n", (n16 + n32) >> 20);
        return false;
    };
    ACC16.store(mem as u64, Ordering::Relaxed);
    ACC32.store((mem + n16) as u64, Ordering::Relaxed);
    ROWS.store(v.h as u64, Ordering::Relaxed);
    COLS.store(v.w as u64, Ordering::Relaxed);
    LINE_B.store(v.line as u64, Ordering::Relaxed);
    BLOCKS.store(blocks as u64, Ordering::Relaxed);
    PITCH.store(pitch as u64, Ordering::Relaxed);
    WORK.store(a.work as u64, Ordering::Relaxed);
    BITS.store(if v.format == 2 { 12 } else { 10 }, Ordering::Relaxed);
    for k in 0..THREADS_MOST {
        READ_T[k].store(0, Ordering::Relaxed);
        ADD_T[k].store(0, Ordering::Relaxed);
    }
    let hz = ferros_sys::freq();
    let (mut held, mut got, mut skipped, mut last) = (0xFFu8, 0u64, 0u64, 0u32);
    let (mut work_t, mut work_most, mut first_t, mut last_t) = (0u64, 0u64, 0u64, 0u64);
    let mut h = [0u8; 64];
    for k in 0..a.frames {
        let n = call(ep, &[OP_MAP, held], &mut h);
        if n == ERR || n < 64 {
            say!(g, "cambench: MAP A#{k}: no frame (G#{:X})\n", n);
            break;
        }
        let now = ferros_sys::now();
        held = h[40];
        let count = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
        if k == 0 {
            first_t = now;
        } else {
            skipped += count.wrapping_sub(last).wrapping_sub(1) as u64;
        }
        last = count;
        last_t = now;
        got += 1;
        let src = v.base + u64::from_le_bytes(h[48..56].try_into().unwrap()) as usize;
        if k == 0 && v.format == 1 {
            // The NEON unpack checked against the plain one on the frame's middle line, here on the device it runs on.
            let l = read_line(0, src + v.h / 2 * v.line, v.line);
            let chk = unsafe { &mut *(&raw mut CHECK) };
            raw::unpack(&l[..blocks * raw::BLOCK_BYTES], &mut chk[0][..pitch]);
            raw::unpack10(l, &mut chk[1][..pitch], blocks);
            let same = chk[0][..v.w] == chk[1][..v.w];
            let mean = chk[0][..v.w].iter().map(|&s| s as u64).sum::<u64>() / v.w as u64;
            say!(g, "cambench: the NEON unpack {} the plain one on line A#{} (its mean sample A#{mean})\n", if same { "matches" } else { "DIFFERS FROM" }, v.h / 2);
        }
        if a.work != 0 {
            // The u16 sums fold into the u32 every 64 frames (64 of 1023 fit 16 bits).
            FOLD.store((a.work == 2 && got % 64 == 0) as u64, Ordering::Relaxed);
            frame(src);
            let t = ferros_sys::now() - now;
            work_t += t;
            work_most = work_most.max(t);
        }
    }
    if got < 2 {
        say!(g, "cambench: A#{got} frames mapped: too few to time\n");
        return got > 0;
    }
    let us = |t: u64| t * 1_000_000 / hz;
    // Two frames mapped at least, so the span between the first and the last is the counter's advance over them, never none.
    let span = last_t - first_t;
    let threads = THREADS.load(Ordering::Relaxed);
    let (rt, at): (u64, u64) = (READ_T.iter().map(|x| x.load(Ordering::Relaxed)).sum(), ADD_T.iter().map(|x| x.load(Ordering::Relaxed)).sum());
    // Frames a second the bench took (times 16), and per frame what the threads spent.
    let fps16 = (got - 1) * hz * 16 / span;
    say!(g, "cambench: A#{got} frames mapped, A#{skipped} skipped between them, A#{} a second; per frame A#{} us of work on A#{threads} threads (the most A#{}): reading A#{} us, adding A#{} us of thread time\n", fps16 / 16, us(work_t / got), us(work_most), us(rt / got), us(at / got));
    // Cores used: the threads' time a frame over the frame's period; what is left may be under none (the work outran the frames, which then were skipped).
    if a.work != 0 {
        let period = span / (got - 1);
        let used = ((rt + at) / got * 100 / period) as i64;
        say!(g, "cambench: the cores used A#{} hundredths of the A#{threads} (A#{} hundredths left) at a frame each A#{} us\n", used, threads as i64 * 100 - used, us(period));
    }
    // The bytes: the sensor's link (the image rows; the PD rows a quarter more when on, four embedded lines), the front end's writes (every line on its stride).
    let bits = BITS.load(Ordering::Relaxed);
    let link = sensor16 * (v.h * v.w) as u64 * bits / 8 / 16;
    let bus = fps16 * (v.h * v.line) as u64 / 16;
    say!(g, "cambench: the image on the link A#{} MB a second at the sensor's rate, the front end writing A#{} MB a second at the taken rate\n", link / 1_000_000, bus / 1_000_000);
    if a.work >= 2 {
        // The sums' mean, a sample per frame: the first 64 rows, so the light's level says the adds landed.
        if a.work == 2 {
            for y in 0..v.h {
                let (r16, r32) = unsafe { (core::slice::from_raw_parts_mut((mem as *mut u16).add(y * pitch), pitch), core::slice::from_raw_parts_mut(((mem + n16) as *mut u32).add(y * pitch), pitch)) };
                raw::fold(r16, r32);
            }
        }
        // WHY the min: a frame shorter than 64 rows (the front end left at the captured geometry takes whole frames of a slit) has fewer to sum.
        let rows = v.h.min(1 << 6);
        let mut sum = 0u64;
        for y in 0..rows {
            let r32 = unsafe { core::slice::from_raw_parts(((mem + n16) as *const u32).add(y * pitch), v.w) };
            sum += r32.iter().map(|&s| s as u64).sum::<u64>();
        }
        say!(g, "cambench: the sums' mean over the first A#{rows} rows: A#{} a frame\n", sum / (rows * v.w) as u64 / got);
    }
    got > 0
}

/// Slits: SLITS for each batch, every new slit read out of the view, unpacked into the bench's own ring and, as the work asks, added into one slit's sums. True when the stream ran.
fn slits(g: u64, ep: u64, a: &Args, v: &View) -> bool {
    let blocks = raw::blocks(v.w);
    let pitch = blocks * raw::BLOCK_SAMPLES;
    let slit = v.h * pitch;
    let (n_ring, n_sum) = (SLIT_KEEP * slit * 2, slit * 4);
    let Some(mem) = memory(n_ring + n_sum) else {
        say!(g, "cambench: no memory for the slit ring (A#{} MiB)\n", (n_ring + n_sum) >> 20);
        return false;
    };
    let hz = ferros_sys::freq();
    let (mut last, mut got, mut lost, mut wrong, mut calls, mut wraps) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut read_t, mut add_t) = (0u64, 0u64);
    let mut r = [0u8; 64];
    let t0 = ferros_sys::now();
    let mut first_n = 0u64;
    while got < a.frames as u64 {
        let mut m = [OP_SLITS, 0, 0, 0, 0];
        m[1..5].copy_from_slice(&(last as u32).to_le_bytes());
        let n = call(ep, &m, &mut r);
        if n == ERR || n < 64 {
            say!(g, "cambench: SLITS after A#{last}: none (G#{:X})\n", n);
            break;
        }
        calls += 1;
        let (newest, ring, buf, rows, line, end) = (u64_at(&r, 0), u64_at(&r, 1), u64_at(&r, 2) as usize, u64_at(&r, 3) as usize, u64_at(&r, 4) as usize, u64_at(&r, 7));
        if first_n == 0 {
            first_n = newest;
            last = newest - 1;
        }
        // The oldest buffer is the next the engine writes: keep to the newest ring - 2 (the one after it has its probe patterned for the frame after the next: payloads/camera `slit_turn`).
        // WHY the saturating subtraction: in the ring's first `ring - 3` frames every buffer written is safe, and newest + 3 - ring would wrap below zero; the max then keeps to the frames not yet read.
        let from = (last + 1).max((newest + 3).saturating_sub(ring));
        lost += from - (last + 1);
        for f in from..=newest {
            let at = v.base + ((f - 1) % ring) as usize * buf;
            let keep = unsafe { (mem as *mut u16).add((got as usize % SLIT_KEEP) * slit) };
            let t = ferros_sys::now();
            for y in 0..rows {
                let l = read_line(0, at + y * line, line);
                // SAFETY: slit `got % SLIT_KEEP` of the ring, `rows * pitch` samples, inside the memory grown for it.
                let dst = unsafe { core::slice::from_raw_parts_mut(keep.add(y * pitch), pitch) };
                raw::unpack10(l, dst, blocks);
            }
            let t2 = ferros_sys::now();
            read_t += t2 - t;
            if a.work >= 2 {
                let sum = unsafe { core::slice::from_raw_parts_mut((mem + n_ring) as *mut u32, slit) };
                let kept = unsafe { core::slice::from_raw_parts(keep, slit) };
                for (s, &k) in sum.iter_mut().zip(kept) {
                    *s += k as u32;
                }
            }
            add_t += ferros_sys::now() - t2;
            // The record after the rows: the frame the buffer held when the driver wrote it; another means the engine wrote it over while it was read (the reader fell a ring behind).
            let rec = unsafe { core::ptr::read_volatile((at + rows * line) as *const u64) };
            wrong += (rec != f) as u64;
            got += 1;
        }
        last = newest;
        wraps = end;
    }
    if got == 0 {
        say!(g, "cambench: no slit taken\n");
        return false;
    }
    // A slit taken means at least one SLITS call between t0 and now: the span is never none.
    let span = ferros_sys::now() - t0;
    let us = |t: u64| t * 1_000_000 / hz;
    say!(g, "cambench: A#{got} slits in A#{} ms (A#{} a second), A#{calls} SLITS calls; lost to the ring A#{lost}, read over A#{wrong}; the engine ring round A#{wraps} times; per slit reading and unpacking A#{} ns, adding A#{} ns\n", us(span) / 1000, got * hz / span, us(read_t * 1000 / got), us(add_t * 1000 / got));
    true
}

/// One open, measured, closed: the frame's rows when it streamed (so a sweep goes on), None when it did not.
fn run(g: u64, ep: u64, a: &Args) -> Option<u16> {
    let v = open(g, ep, a)?;
    caps(g, ep);
    let (sensor16, fe16) = rate(g, ep);
    let ok = if v.slit { slits(g, ep, a, &v) } else { frames(g, ep, a, &v, sensor16) };
    stats(g, ep);
    close(g, ep);
    // A slit's sweep goes on while the sensor streams, whatever the front end took: its RATE is the sensor's floor at each step (2026-11-17 night: the front end takes two slit frames and stops, SHORT_VB_FATAL, at any frame length).
    let streamed = if v.slit { sensor16 != 0 } else { ok && sensor16 != 0 && fe16 != 0 };
    streamed.then_some(v.h as u16)
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(g: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let b = |k: u64| if k < input_len { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + k) as *const u8) } } else { 0 };
    let h = |k: u64| u16::from_le_bytes([b(k), b(k + 1)]);
    let threads = match b(15) as usize {
        0 => THREADS_MOST,
        // WHY the min: the byte is the person's; more threads than the A715s (and the workers' stacks) are not there to start.
        n => n.min(THREADS_MOST),
    };
    // Past the 16 bytes: +16 the window's columns (u16; 0 the mode's width), +18 the frames' count moved left this many bits (a slit run of 4080 << 6, 261120 slits, is 3.6 min at 1212 a second: the drain).
    // WHY the min: the shift is the person's byte, and past 12 the count leaves the u32.
    let frames = if b(13) == 0 { 1 << 6 } else { b(13) as u32 * 16 } << b(18).min(12);
    let a = Args { id: b(0), kind: b(1), rows: h(2), fll: h(4), exposure: h(6), flags: b(8), ring: b(9), llp: h(10), work: b(12), frames, sweep: b(14), cols: h(16) };
    ferros_sys::pin(1 << 4);
    THREADS.store(threads as u64, Ordering::Relaxed);
    for k in 1..threads {
        ferros_sys::thread_spawn(worker, unsafe { &mut (*(&raw mut STACKS))[k - 1] }, k as u64);
    }
    // Off the spawn first: the bridge's spawn returns once this blocks, before any call that may wait seconds on the driver (an open replays the sensor's init).
    ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / (1 << 4));
    let ep = cap(g, 1);
    match a.sweep {
        // The slit's rows from 64 down to its cell (binned 2, 1:1 4), the tallest first: each a new open (the switch powers the module down between them), the sweep ends at the first that does not stream.
        1 => {
            let cell = if a.kind == K_SLIT_FULL { 4 } else { 2 };
            let mut rows = 1u16 << 6;
            while rows >= cell {
                say!(g, "cambench: sweep: a slit of A#{rows} rows\n");
                if run(g, ep, &Args { rows, ..a }).is_none() {
                    say!(g, "cambench: sweep: stopped at A#{rows} rows (it did not stream; the next open powers the module from cold, a stuck sensor needs a cold boot)\n");
                    break;
                }
                rows >>= 1;
            }
        }
        // The frame from the mode's own down: its rows plus 2^12, 2^10, 2^8, then halving to 2^0 lines of blanking, each a new open; ends at the first that does not stream (or that the driver refuses: the integration needs its margin, ferros_cam ccs `MARGIN`). The integration 4 lines unless given, so the frame can come down to the rows and 36.
        2 => {
            let exposure = if a.exposure == 0 { 1 << 2 } else { a.exposure };
            // The mode's own frame first; its rows (the view's) set the rest.
            let first = Args { fll: 0xFFFF, exposure, ..a };
            say!(g, "cambench: sweep: the mode's own frame\n");
            let Some(rows) = run(g, ep, &first) else {
                say!(g, "cambench: sweep: the mode's own frame did not stream\n");
                say!(g, "cambench: done\n");
                ferros_sys::exit(1);
            };
            for blank in [1u16 << 12, 1 << 10, 1 << 8, 1 << 7, 1 << 6, 1 << 5, 1 << 4, 1 << 3, 1 << 2, 1 << 1, 1] {
                let fll = rows + blank;
                say!(g, "cambench: sweep: a frame of A#{fll} lines (A#{blank} past the rows)\n");
                if run(g, ep, &Args { fll, exposure, ..a }).is_none() {
                    say!(g, "cambench: sweep: stopped at a frame of A#{fll} lines\n");
                    break;
                }
            }
        }
        // The line from the init's 6320 clocks down (the main's: 5888 the front end's least at the whole width on 2026-11-16, about 5370 the sensor's), each a new open at the frame asked; with a narrow window (+16) the line's pixels on the link fall, so the sensor's row time and the front end's own pixel rate show apart: RATE's sensor rate times the frame's lines is the line the sensor really ran, the front end's beside it. Ends at the first that does not stream.
        3 => {
            for llp in [6320u16, 5888, 5376, 4864, 4096, 3584, 3072, 2560, 2048, 1536, 1024, 768, 512] {
                say!(g, "cambench: sweep: a line of A#{llp} clocks\n");
                if run(g, ep, &Args { llp, ..a }).is_none() {
                    say!(g, "cambench: sweep: stopped at a line of A#{llp} clocks\n");
                    break;
                }
            }
        }
        _ => {
            run(g, ep, &a);
        }
    }
    say!(g, "cambench: done\n");
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

ferros_sys::entry!(payload_main, grows CEILING);
