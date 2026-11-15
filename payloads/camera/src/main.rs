//! The camera driver (docs/CAMERA.md "The driver"; owner, 2026-11-14: "Can we get Lumis wired up?"): the Pro's main camera served on the `camera` endpoint, started by the kernel when a call on it finds no server (process.rs `ON_DEMAND`), ended by itself once its holder closes.
//!
//! The kernel keeps what a process may not do behind the `camera` power switch (kernel camera_power.rs): both camera buses' pins and clocks, the module's rails behind the overvoltage guard, the front end's power domain and clocks, the D-PHY's isolation, the SYSMMUs in bypass and the S2MPUs fenced to this driver's frames. Here, thru its grants: the sensor and its lens actuator on hsi2c_1, the front end and its PDMA engine, the CSI links and PHY, and the front end's line 0.
//!
//! An open, in the order a camera open does it (the kernel's `diag cam pdma`, which this replaces): the region taken (a run of frames from the pool, zeroed, fenced), the switch on, the sensor's identity read, its init replayed from the vault (`cam.boitata.init`, or the bundle `cam.frame`), the PDMA job from the vault (`cam.boitata.pdma`) moved into the region (ferros_cam job) and placed there, the CPU's setup writes before the engine (`cam.ispfe.prog`), the engine started, the receiver last, the CPU's writes after it, the lens set, the stream on.
//! Then the front end is answered on its interrupt: every acknowledgement in the order Linux makes them (ferros_cam `FRAME_ACK`), loch0's paths enabled after the first SHADOW_UPDATE (G#81 to G#1C32_00A0), the engine's descriptor index advanced once a frame. Left unanswered for about 50 ms the front end stops for good (docs/oracle/CAMERA.md "Focus, exposure"), which is why this is its own thread on an interrupt and not the kernel's spin.
//!
//! Spawn by hand (the kernel also starts it itself on a call to `camera`):
//! ferros-bridge spawn camera.bin --name camera --serve camera --hw hsi2c1 --hw ispfe --hw csis --hw ispfe-sysreg --store cam --power camera --dma camera --irq ispfe0
//! Grant slots, in that order: 0 console, 1 serve camera, 2 hsi2c_1, 3 the front end, 4 the CSI links and PHY, 5 SYSREG_ISPFE, 6 the vault scope `cam.`, 7 the switch, 8 the region, 9 the line.
//!
//! The `camera` endpoint (docs/CAMERA.md "The endpoint"), first byte the op:
//! - `0` LIST: the list, a VSF document made at build time (build.rs).
//! - `1` OPEN + id (+ `1` for the view): `[0]` open, `[1]` no such camera, `[2]` another holds it, `[3]` it would not start (the console says why). With the view asked for and given, `[0]` then its token and the ring's shape (`VIEW_REPLY`).
//! - `2` FRAME + bin: waits for the next frame; a 64-byte header, then the mosaic, `bin` x `bin` same-colour samples averaged, 16 bits a sample. bin 2, 4 or 8 (a whole readout is past the largest reply).
//! - `3` SET + what + value (8 bytes): what 0 exposure (ns), 1 analogue gain (1024 unity), 2 focus. `[0]` taken, `[1]` outside the range.
//! - `4` CLOSE: the view taken back, the stream stopped, the switch off, `[0]`, and the driver ends; its frames go back to the pool wiped.
//! - `5` MAP + the buffer to give back (G#FF none), with the view: waits for a frame newer than the last mapped and holds its buffer; the frame's 64-byte header (bin 1, the buffer at +40), the samples read in place thru the view (docs/CAMERA.md "The view").
//! - `6` RELEASE + buffer: the held buffer given back; `[0]`, `[1]` when it was not held.
//! A holder that ends without closing: its waiting FRAME's reply fails, or 64 frames pass with nobody asking; either closes. The kernel takes the view back when either ends.
#![no_std]
#![no_main]

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_cam::{inputs, job};
use ferros_sys::{cap, console, mmio_read, mmio_write, recv, reply, ERR, NOTHING};

const S_SERVE: u64 = 1;
const S_BUS: u64 = 2;
const S_FE: u64 = 3;
const S_CSIS: u64 = 4;
const S_SYSREG: u64 = 5;
const S_STORE: u64 = 6;
const S_POWER: u64 = 7;
const S_REGION: u64 = 8;
const S_IRQ: u64 = 9;

/// The list (build.rs).
static LIST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/list.vsf"));

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

// ---------------------------------------------------------------- registers

/// Which grant a front-end address lies in, and its offset there: the front end (G#1C30_0000, 1 MiB), the CSI links and PHY (G#1C04_0000 up to the first SYSMMU), SYSREG_ISPFE's first page. Anything else is none of this driver's.
fn grant_of(a: usize) -> Option<(u64, u64)> {
    if (0x1C30_0000..0x1C40_0000).contains(&a) {
        Some((S_FE, (a - 0x1C30_0000) as u64))
    } else if (0x1C04_0000..0x1C11_0000).contains(&a) {
        Some((S_CSIS, (a - 0x1C04_0000) as u64))
    } else if (0x1C02_0000..0x1C02_1000).contains(&a) {
        Some((S_SYSREG, (a - 0x1C02_0000) as u64))
    } else {
        None
    }
}

/// Counts of register accesses refused here (an address in none of the grants): a table that names something else is said, not written.
static REFUSED: AtomicU64 = AtomicU64::new(0);

fn wr(g: u64, a: usize, v: u32) {
    match grant_of(a) {
        Some((s, o)) => {
            mmio_write(cap(g, s), o, v);
        }
        None => {
            REFUSED.fetch_add(1, Ordering::Relaxed);
        }
    }
}
fn rd(g: u64, a: usize) -> u32 {
    match grant_of(a) {
        Some((s, o)) => mmio_read(cap(g, s), o) as u32,
        None => {
            REFUSED.fetch_add(1, Ordering::Relaxed);
            0
        }
    }
}

// ---------------------------------------------------------------- hsi2c_1

// The HS-I2C controller (kernel hsi2c.rs, i2c-exynos5.c auto mode), as payloads/torch drives hsi2c_14.
const CTL: u64 = 0x00;
const FIFO_CTL: u64 = 0x04;
const INT_ENABLE: u64 = 0x20;
const INT_STATUS: u64 = 0x24;
const FIFO_STATUS: u64 = 0x30;
const TX_DATA: u64 = 0x34;
const RX_DATA: u64 = 0x38;
const AUTO_CONF: u64 = 0x44;
const TRANS_STATUS: u64 = 0x50;
const ADDR: u64 = 0x70;
const CTL_MASTER: u32 = 1 << 3;
const CTL_RXCHON: u32 = 1 << 6;
const CTL_TXCHON: u32 = 1 << 7;
const INT_TRANSFER_DONE: u32 = 1 << 7;
const INT_ERRORS: u32 = 0xF << 8;
const INT_NODEV: u32 = 1 << 10;
const FIFO_TX_EMPTY: u32 = 1 << 8;
const FIFO_RX_EMPTY: u32 = 1 << 24;
const AUTO_READ: u32 = 1 << 16;
const AUTO_STOP_AFTER_TRANS: u32 = 1 << 17;
const AUTO_MASTER_RUN: u32 = 1 << 31;
const TRANS_MASTER_BUSY: u32 = 1 << 17;
const ADDR_MASTER_ID_7: u32 = 7 << 24;
/// Polls per wait, a syscall each: thousands of microseconds, far past a byte at 192 kHz.
const WAIT_POLLS: u32 = 1 << 14;
/// The receive FIFO's depth, and how full the transmit FIFO is kept (under 48 of 64, as the kernel feeds it).
const FIFO_DEPTH: u32 = 1 << 6;
const TX_KEEP: u32 = 48;

const E_BUSY: u32 = 2;
const E_NODEV: u32 = 4;
const E_BUS: u32 = 8;
const E_TIMEOUT: u32 = 0x10;

/// The main sensor (boitata, Samsung, G#3D) and its lens actuator (cornerfolk, G#0C), both on hsi2c_1 (docs/oracle/CAMERA.md).
const SENSOR: u8 = 0x3D;
const ACTUATOR: u8 = 0x0C;

struct Bus {
    g: u64,
}

impl Bus {
    fn rd(&self, off: u64) -> u32 {
        mmio_read(cap(self.g, S_BUS), off) as u32
    }
    fn wr(&self, off: u64, v: u32) {
        mmio_write(cap(self.g, S_BUS), off, v);
    }
    fn fail(&self, st: u32) -> Option<u32> {
        if st & INT_NODEV != 0 {
            self.wr(INT_STATUS, st);
            return Some(E_NODEV);
        }
        if st & INT_ERRORS != 0 {
            self.wr(INT_STATUS, st);
            return Some(E_BUS);
        }
        None
    }
    fn wait_done(&self) -> Result<(), u32> {
        for _ in 0..WAIT_POLLS {
            let st = self.rd(INT_STATUS);
            if let Some(e) = self.fail(st) {
                return Err(e);
            }
            if st & INT_TRANSFER_DONE != 0 {
                self.wr(INT_STATUS, st);
                return Ok(());
            }
        }
        Err(E_TIMEOUT)
    }
    /// Whatever the receive FIFO holds, read and dropped. WHY the bound: the FIFO is 64 deep, so a status still saying "not empty" after 64 reads is the controller misreporting, and this stops rather than spins.
    fn drain_rx(&self) {
        for _ in 0..FIFO_DEPTH {
            if self.rd(FIFO_STATUS) & FIFO_RX_EMPTY != 0 {
                return;
            }
            self.rd(RX_DATA);
        }
    }
    /// One auto-mode transfer to `target`: `tx` written (any length: the transmit FIFO fed as it drains, the sensor's 6.5 KB settings upload is one transfer), or `rx` filled; STOP after it unless `hold` keeps the bus for the read that follows.
    fn xfer(&self, target: u8, tx: &[u8], rx: &mut [u8], hold: bool) -> Result<(), u32> {
        let mut idle = false;
        for _ in 0..WAIT_POLLS {
            if self.rd(TRANS_STATUS) & TRANS_MASTER_BUSY == 0 {
                idle = true;
                break;
            }
        }
        if !idle {
            return Err(E_BUSY);
        }
        self.drain_rx();
        let read = !rx.is_empty();
        let len = if read { rx.len() } else { tx.len() } as u32;
        self.wr(FIFO_CTL, if read { 0x0002_0023 } else { 0x0001_0013 });
        self.wr(CTL, CTL_MASTER | if read { CTL_RXCHON } else { CTL_TXCHON });
        self.wr(INT_STATUS, self.rd(INT_STATUS));
        self.wr(ADDR, ADDR_MASTER_ID_7 | (target as u32) << 10);
        self.wr(INT_ENABLE, INT_TRANSFER_DONE | INT_ERRORS);
        let auto = len | if read { AUTO_READ } else { 0 } | if hold { 0 } else { AUTO_STOP_AFTER_TRANS };
        self.wr(AUTO_CONF, auto);
        self.wr(AUTO_CONF, auto | AUTO_MASTER_RUN);
        if read {
            self.wait_done()?;
            for b in rx.iter_mut() {
                if self.rd(FIFO_STATUS) & FIFO_RX_EMPTY != 0 {
                    return Err(E_TIMEOUT);
                }
                *b = self.rd(RX_DATA) as u8;
            }
        } else {
            for &b in tx {
                let mut room = false;
                for _ in 0..WAIT_POLLS {
                    if self.rd(FIFO_STATUS) & 0x7F < TX_KEEP {
                        room = true;
                        break;
                    }
                    if let Some(e) = self.fail(self.rd(INT_STATUS)) {
                        return Err(e);
                    }
                }
                if !room {
                    return Err(E_TIMEOUT);
                }
                self.wr(TX_DATA, b as u32);
            }
            self.wait_done()?;
            for _ in 0..WAIT_POLLS {
                if self.rd(FIFO_STATUS) & FIFO_TX_EMPTY != 0 {
                    break;
                }
            }
        }
        Ok(())
    }
    fn write(&self, target: u8, w: &[u8]) -> Result<(), u32> {
        self.xfer(target, w, &mut [], false)
    }
    fn write_read(&self, target: u8, w: &[u8], r: &mut [u8]) -> Result<(), u32> {
        self.xfer(target, w, &mut [], true)?;
        self.xfer(target, &[], r, false)
    }
}

// ---------------------------------------------------------------- the frame thread

/// Frames the front end has written since the open (its DMA_DONE on loch0's image path), and the Eagle time the last one ended.
static FRAMES: AtomicU64 = AtomicU64::new(0);
static FRAME_AT: AtomicU64 = AtomicU64::new(0);
/// The frame thread's work: 0 idle (the stream not started), 1 answering, 2 asked to stop, 3 stopped (it touches the front end no more: once the engine is stopped a read in G#1C34 or G#1C35 stalls the bus, docs/oracle/CAMERA.md, and the switch's off stops it).
static RUN: AtomicU64 = AtomicU64::new(0);
/// The front end's interrupts answered, and how many came on the line rather than on the thread's own tick (`TICK_US`).
static ANSWERED: AtomicU64 = AtomicU64::new(0);
static ON_LINE: AtomicU64 = AtomicU64::new(0);
/// The image path's status words seen, or-ed over the run, for the console.
static IMG_SEEN: AtomicU64 = AtomicU64::new(0);
/// When the open began (the counter), for the time to the first frame.
static OPEN_AT: AtomicU64 = AtomicU64::new(0);
/// Set by the frame thread when no interrupt came on the line within `FIRST_LINE_US` of the stream starting: it answers on its own tick from then on.
static TICKING: AtomicU64 = AtomicU64::new(0);

/// How long the frame thread waits for the line's first interrupt once the stream is on before it answers on a tick instead: the front end stops for good unanswered past about 50 ms (2026-10-21), so this is that deadline and no longer.
const FIRST_LINE_US: u64 = 1 << 15;
/// The tick, if the line never comes: well under the 50 ms the front end waits (the kernel's harness answered every 50 ms at the main camera's 14.5 frames a second and that held).
const TICK_US: u64 = 1 << 15;
/// With the line working, the longest the frame thread waits on it before it looks whether it was asked to stop: two frame periods (67.8 ms each), so it wakes on its own only once the stream has stopped, and a close never waits on a line that will not fire again.
const QUIET_US: u64 = 1 << 17;
/// The ring's descriptors and the index's step. The job's own ring has 1024, and an index written back to 0 after its last stops the front end for good (the Pro, 2026-11-14: no frame after the 1025th), so the engine gets a ring of its own, every descriptor the job's first, in a frame past the job's region: 2^14 of them, about 18 minutes at 14.7 frames a second, if the count register holds it (`RING_JOB` otherwise).
const RING: u64 = 1 << 14;
const RING_JOB: u64 = 1 << 10;
const STEP: u32 = 0x10;
/// The descriptors the engine was given (`RING`, or `RING_JOB` when the count register did not hold `RING`).
static RING_N: AtomicU64 = AtomicU64::new(RING_JOB);

static mut FRAME_STACK: [u8; 1 << 15] = [0; 1 << 15];

fn eagle_ns() -> u64 {
    // The Eagle epoch (1969-07-20 20:17:40 UTC) is 14182940 s before the count the kernel's wall clock keeps (AGENT.md "Eagle Time"); the counter's ticks into that second in nanoseconds.
    const EAGLE_FROM_KERNEL_S: i64 = 14_182_940;
    match ferros_sys::utc_time() {
        Some((s, ticks, hz)) => ((s + EAGLE_FROM_KERNEL_S) as u64) * 1_000_000_000 + ticks * 1_000_000_000 / hz,
        None => 0,
    }
}

// ---------------------------------------------------------------- the image ring

// The ring (docs/CAMERA.md "The view"; owner, 2026-11-15: "why the ridiculously low camera resolution?"): the image body goes to one of `RING_BUFS` buffers of whole sensor frames, chosen per frame by the descriptor the frame thread releases (each names its own copy of the job's program, which names that buffer), so a frame the kist holds is never written over and the newest written frame stays whole until a newer one is.
// Which frame lands in which buffer is measured, not assumed: a buffer's probe (the first 64 bytes of its last line) is patterned as it is given to the engine, and the frame's DMA_DONE finds the buffer whose probe moved.

/// The buffers this open (0 before one).
static RING_BUFS: AtomicU64 = AtomicU64::new(0);
/// Where things are, set by the open before the frame thread starts (`RUN`): the first buffer as this process reads it; the program copies as read and as the engine reads them (physical); the descriptor ring as read; a descriptor's first and last words (the job's own flags and the program's length).
static RING_VA: AtomicU64 = AtomicU64::new(0);
static PROG_VA: AtomicU64 = AtomicU64::new(0);
static PROG_PA: AtomicU64 = AtomicU64::new(0);
static DESC_VA: AtomicU64 = AtomicU64::new(0);
static DESC_WORDS: AtomicU64 = AtomicU64::new(0);
/// The program copies' place in the long ring's frame (past its 2^14 descriptors of 16 bytes) and their spacing: the main camera's program is 6620 bytes.
const PROG_AT: usize = 1 << 18;
const PROG_STRIDE: usize = 1 << 13;

/// The ring's state, one word so either thread changes it whole (compare-and-swap): bits 0-7 the buffers the kist holds, 8-15 the buffers the driver bins from, 16-23 the buffers released to the engine whose frame has not been seen yet, 24-27 the newest written buffer, bit 28 set once there is one.
static STATE: AtomicU64 = AtomicU64::new(0);
const HOLD_KIST: u32 = 0;
const HOLD_BIN: u32 = 8;
const QUEUED: u32 = 16;
const NEWEST: u32 = 24;
const HAS_NEWEST: u64 = 1 << 28;
fn newest_of(s: u64) -> Option<usize> {
    (s & HAS_NEWEST != 0).then_some(((s >> NEWEST) & 0xF) as usize)
}

/// Per buffer, of the frame it holds: its count, when it ended (Eagle ns), the counter at its DMA_DONE, the exposure (lines), gain and focus the sensor had been told; and when it was released to the engine (a sequence number).
static B_COUNT: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_AT: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_TICK: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_LINES: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_GAIN: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_FOCUS: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_SEQ: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static SEQ: AtomicU64 = AtomicU64::new(0);
/// The sensor's settings now, for each frame's record (the server sets them at SET).
static NOW_LINES: AtomicU64 = AtomicU64::new(ferros_cam::MAIN_EXPOSURE_MAX_LINES);
static NOW_GAIN: AtomicU64 = AtomicU64::new(1 << 10);
static NOW_FOCUS: AtomicU64 = AtomicU64::new(0x3800);

/// The ring's measures, for the console: frames by how many releases back their buffer was given (0 the last, 1 the one before, 2 older), frames whose buffer was not found (no probe moved), turns that found no buffer free of the kist's, the engine's and the newest (the newest given again), and turns that found none at all (the descriptor names the last buffer again).
static AGE: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static UNSEEN: AtomicU64 = AtomicU64::new(0);
static CROWDED: AtomicU64 = AtomicU64::new(0);
static NONE_FREE: AtomicU64 = AtomicU64::new(0);
/// The buffer the last descriptor named.
static LAST_R: AtomicU64 = AtomicU64::new(0);

/// Buffer `b`'s probe: the first 64 bytes of its last line, as one load would read them.
fn probe_at(b: usize) -> *mut u64 {
    (RING_VA.load(Ordering::Relaxed) as usize + b * job::IMAGE_BYTES as usize + PROBE_AT) as *mut u64
}
fn probe_read(b: usize) -> [u64; 8] {
    let p = probe_at(b);
    core::array::from_fn(|i| unsafe { core::ptr::read_volatile(p.add(i)) })
}
fn probe_pattern(b: usize) {
    let p = probe_at(b);
    for i in 0..8 {
        unsafe { core::ptr::write_volatile(p.add(i), PATTERN) };
    }
}

/// A frame's DMA_DONE, the ring's turn: the buffer it landed in found (the oldest released whose probe moved), its record kept and it made the newest; then, when `desc` names the descriptor about to be released, a buffer for it chosen (none the kist or the binning holds, none still with the engine, not the newest), patterned and named in that descriptor. Frame `n` (the count it will have).
fn ring_turn(n: u64, desc: Option<u64>) {
    let k = RING_BUFS.load(Ordering::Relaxed) as usize;
    if k == 0 {
        return;
    }
    let queued = (STATE.load(Ordering::Acquire) >> QUEUED) & 0xFF;
    let (mut done, mut seq) = (None, u64::MAX);
    for b in 0..k {
        if queued >> b & 1 == 1 && probe_read(b) != [PATTERN; 8] && B_SEQ[b].load(Ordering::Relaxed) < seq {
            seq = B_SEQ[b].load(Ordering::Relaxed);
            done = Some(b);
        }
    }
    match done {
        Some(b) => {
            B_COUNT[b].store(n, Ordering::Relaxed);
            B_AT[b].store(eagle_ns(), Ordering::Relaxed);
            B_TICK[b].store(ferros_sys::now(), Ordering::Relaxed);
            B_LINES[b].store(NOW_LINES.load(Ordering::Relaxed), Ordering::Relaxed);
            B_GAIN[b].store(NOW_GAIN.load(Ordering::Relaxed), Ordering::Relaxed);
            B_FOCUS[b].store(NOW_FOCUS.load(Ordering::Relaxed), Ordering::Relaxed);
            // How many releases back: 0 the descriptor released at the last DMA_DONE. WHY the min: the measure has three bins; anything older lands in the last.
            let back = (SEQ.load(Ordering::Relaxed) - 1 - seq).min(2) as usize;
            AGE[back].fetch_add(1, Ordering::Relaxed);
        }
        None => {
            UNSEEN.fetch_add(1, Ordering::Relaxed);
        }
    }
    let all = (1u64 << k) - 1;
    let mut r;
    loop {
        let s = STATE.load(Ordering::Acquire);
        let held = s & 0xFFFF;
        let holds = (held | held >> 8) & 0xFF;
        let mut q = (s >> QUEUED) & 0xFF;
        let mut newest = newest_of(s);
        if let Some(b) = done {
            q &= !(1 << b);
            newest = Some(b);
        }
        let newest_bit = newest.map_or(0, |b| 1 << b);
        r = None;
        if desc.is_some() {
            let free = all & !(holds | q | newest_bit);
            let spare = all & !(holds | q);
            r = if free != 0 {
                Some(free.trailing_zeros() as usize)
            } else if spare != 0 {
                Some(spare.trailing_zeros() as usize)
            } else {
                None
            };
        }
        // The buffer the descriptor will name: the one chosen, else the last one named (below); either is with the engine from now on, so its frame is found at the next DMA_DONE (the last one named unpatterned, so found at once: a ring of one, held while it is binned, is the driver before the view).
        let named = r.or_else(|| desc.map(|_| LAST_R.load(Ordering::Relaxed) as usize));
        let q2 = q | named.map_or(0, |r| 1 << r);
        let ns = held | q2 << QUEUED | newest.map_or(0, |b| (b as u64) << NEWEST | HAS_NEWEST);
        if STATE.compare_exchange(s, ns, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            if desc.is_some() && r.is_some_and(|r| newest_bit >> r & 1 == 1) {
                CROWDED.fetch_add(1, Ordering::Relaxed);
            }
            break;
        }
    }
    let Some(e) = desc else { return };
    let r = match r {
        Some(r) => {
            probe_pattern(r);
            B_SEQ[r].store(SEQ.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
            LAST_R.store(r as u64, Ordering::Relaxed);
            r
        }
        None => {
            // Every buffer held or with the engine: the descriptor names the buffer the last one did, which is with the engine already and nobody else's.
            NONE_FREE.fetch_add(1, Ordering::Relaxed);
            LAST_R.load(Ordering::Relaxed) as usize
        }
    };
    desc_write(e, r);
    ferros_sys::dma_barrier();
}

/// Descriptor `e` of the engine's ring names program copy `r` (which names buffer `r`): the job's own flags and length around the copy's address.
fn desc_write(e: u64, r: usize) {
    let w = DESC_WORDS.load(Ordering::Relaxed);
    let list = PROG_PA.load(Ordering::Relaxed) + (r * PROG_STRIDE) as u64;
    let d = [w as u32, list as u32, (list >> 32) as u32, (w >> 32) as u32];
    unsafe { core::ptr::write_volatile((DESC_VA.load(Ordering::Relaxed) + e * 16) as *mut [u32; 4], d) };
}

/// The front end answered once: its aggregate and loch0's image status read, every acknowledgement made in Linux's order, loch0's paths enabled after the first SHADOW_UPDATE, and on a frame's DMA_DONE the ring's turn (`ring_turn`), the engine's index advanced and the frame counted.
fn answer(g: u64, idx: &mut u32, enabled: &mut bool) {
    let img = rd(g, ferros_cam::IMG_STATUS);
    for &a in ferros_cam::FRAME_ACK.iter() {
        let v = rd(g, a);
        wr(g, a, v);
    }
    IMG_SEEN.fetch_or(img as u64, Ordering::Relaxed);
    ANSWERED.fetch_add(1, Ordering::Relaxed);
    if !*enabled && img & ferros_cam::IMG_SHADOW != 0 {
        wr(g, ferros_cam::LOCH_ENABLE, ferros_cam::LOCH_ENABLE_IMG_PD);
        *enabled = true;
        say!(g, "camera: SHADOW_UPDATE; loch0's image and PD paths enabled\n");
    }
    if img & ferros_cam::IMG_DMA_DONE != 0 {
        // The buffer this frame landed in, and the next descriptor's buffer: the descriptor released by the index write below is the one at the index now.
        let more = *idx < RING_N.load(Ordering::Relaxed) as u32 * STEP;
        ring_turn(FRAMES.load(Ordering::Relaxed) + 1, more.then_some((*idx / STEP) as u64));
        // The next descriptor. Never round the ring: back to 0 the front end stops for good (`RING`). At the ring's end the index is left where it is, the console says so, and the stream ends with its last descriptor (the FRAME that finds no frame closes).
        if more {
            *idx += STEP;
            wr(g, ferros_cam::PDMA + ferros_cam::PDMA_INDEX, *idx);
            if *idx == RING_N.load(Ordering::Relaxed) as u32 * STEP {
                say!(g, "camera: the ring's last descriptor given after A#{} frames\n", FRAMES.load(Ordering::Relaxed) + 1);
            }
        }
        FRAME_AT.store(eagle_ns(), Ordering::Release);
        let n = FRAMES.fetch_add(1, Ordering::AcqRel) + 1;
        if n == 1 {
            let us = ferros_sys::us_since(OPEN_AT.load(Ordering::Relaxed));
            say!(g, "camera: first frame A#{} ms after the open\n", us / 1000);
        }
        ferros_sys::wake(&FRAMES, 4);
    }
}

extern "C" fn frame_thread(g: u64) -> ! {
    let line = cap(g, S_IRQ);
    // Idle until the stream starts: the server rings RUN.
    while RUN.load(Ordering::Acquire) == 0 {
        ferros_sys::wait(&RUN, 0, 0);
    }
    let (mut idx, mut enabled, mut ticking) = (STEP, false, false);
    let started = ferros_sys::now();
    let ticks = |us: u64| ferros_sys::now() + ferros_sys::freq() * us / 1_000_000;
    loop {
        let wait = if ticking { TICK_US } else if ON_LINE.load(Ordering::Relaxed) == 0 { FIRST_LINE_US } else { QUIET_US };
        let r = ferros_sys::irq_wait(line, ticks(wait));
        if RUN.load(Ordering::Acquire) != 1 {
            RUN.store(3, Ordering::Release);
            ferros_sys::wake(&RUN, 1);
            ferros_sys::thread_exit(0);
        }
        if r == 0 {
            answer(g, &mut idx, &mut enabled);
            ON_LINE.fetch_add(1, Ordering::Relaxed);
            ferros_sys::irq_ack(line);
        } else if ticking {
            answer(g, &mut idx, &mut enabled);
        } else if ON_LINE.load(Ordering::Relaxed) == 0 {
            // No interrupt on the line in the front end's own deadline: answered on the tick from now on, and the console says so with the words that would say why.
            ticking = true;
            TICKING.store(1, Ordering::Relaxed);
            say!(g, "camera: no interrupt on SPI 479 in A#{} us of the stream; answering on a A#{} us tick (front end aggregate G#{:X}, image G#{:X}, engine G#{:X} index G#{:X})\n", ferros_sys::us_since(started), TICK_US, rd(g, ferros_cam::CMN_STATUS), rd(g, ferros_cam::IMG_STATUS), rd(g, ferros_cam::PDMA), rd(g, ferros_cam::PDMA + ferros_cam::PDMA_INDEX));
            answer(g, &mut idx, &mut enabled);
        }
        // Else the line works and was quiet for two frame periods: nothing is pending, and the stream has likely stopped.
    }
}

// ---------------------------------------------------------------- the open

/// Buffers for the vault's values: the init (about 22 KB), the CPU's program (about 37 KB), the bundle of both (about 60 KB), the job (about 35 KB).
const VALUE: usize = 1 << 16;
static mut INIT: [u8; VALUE] = [0; VALUE];
static mut PROG: [u8; VALUE] = [0; VALUE];
static mut BUNDLE: [u8; 2 * VALUE] = [0; 2 * VALUE];
static mut JOB: [u8; VALUE] = [0; VALUE];

/// A fill no frame holds, so a write anywhere in an output shows (docs/oracle/CAMERA.md "the write test was blind to dark frames": a point reading anything else, zero included, was written).
const PATTERN: u64 = 0xA5A5_A5A5_A5A5_A5A5;

/// The open camera.
struct Open {
    /// Where this process reads the region (its physical start went into the job), and the region's layout with its image ring of `ring` buffers.
    va: usize,
    l: job::Layout,
    ring: usize,
    /// The sensor's settings now (lines, the gain code, the lens's target) and the frame counted at the last FRAME or MAP request.
    exposure_lines: u64,
    gain: u64,
    focus: u64,
    asked_at: u64,
    /// The outputs' extents have been reported (after the eighth frame).
    probed: bool,
    /// The view was given to the holder (an OPEN that asked for it).
    view: bool,
    /// The count of the last frame handed to the kist (MAP), the buffer it holds and that buffer's probe as handed out, to tell at its release whether anything wrote it meanwhile.
    given: u64,
    held: Option<(usize, [u64; 8])>,
    /// MAP's measures, for the console every 64: from a frame's DMA_DONE to its MAP reply (counter ticks, summed and the most), holds found written over at their release.
    maps: u64,
    lat_sum: u64,
    lat_max: u64,
    overwritten: u64,
}

fn get<'a>(g: u64, key: &[u8], buf: &'a mut [u8]) -> Option<&'a [u8]> {
    match ferros_sys::store_get(cap(g, S_STORE), key, buf) {
        Some(n) if n <= buf.len() => Some(&buf[..n]),
        Some(n) => {
            say!(g, "camera: cam.{} is A#{n} bytes, past the A#{} this driver reads\n", core::str::from_utf8(key).unwrap_or("?"), buf.len());
            None
        }
        None => None,
    }
}

/// The lens actuator to `pos`: reset (02 <- G#40), at least 5 ms quiet (straight after it every write NACKs), 02 <- 0, F1 <- 0 once, then the target into 00/01 (docs/oracle/CAMERA.md "Focus").
fn focus_init(bus: &Bus) -> Result<(), u32> {
    bus.write(ACTUATOR, &[0x02, 0x40])?;
    ferros_sys::delay_us(1 << 13);
    bus.write(ACTUATOR, &[0x02, 0x00])?;
    bus.write(ACTUATOR, &[0xF1, 0x00])
}
fn focus_set(bus: &Bus, pos: u64) -> Result<(), u32> {
    bus.write(ACTUATOR, &[0x00, (pos >> 8) as u8])?;
    bus.write(ACTUATOR, &[0x01, pos as u8])
}

/// The sensor's exposure in lines, inside the grouped-parameter hold (G#0104 is one byte: the hold is 01 04 01 01, its release 01 04 00 01; Linux's per-frame block, camera-main-open-transfers.txt 771-778).
fn exposure_set(bus: &Bus, lines: u64) -> Result<(), u32> {
    bus.write(SENSOR, &[0x01, 0x04, 0x01, 0x01])?;
    let r = bus.write(SENSOR, &[0x02, 0x02, (lines >> 8) as u8, lines as u8]);
    bus.write(SENSOR, &[0x01, 0x04, 0x00, 0x01])?;
    r
}

/// Why an open did not happen.
enum Fail {
    Region,
    Switch,
    Sensor(u32),
    Inputs(&'static str),
    Init(u32, u32),
    Job(job::Error),
}

fn open(g: u64) -> Result<Open, Fail> {
    OPEN_AT.store(ferros_sys::now(), Ordering::Relaxed);
    let t0 = ferros_sys::now();
    let bus = Bus { g };
    // The region: one run of frames side by side, under 4 GiB first, as every October run had it (coordinator, 2026-11-14). Linux never shows whether the front end drives the high address halves: its ISP DMA always goes thru the SYSMMU, whose window the driver clamps to 32 bits (samsung-iommu-v9.c `dma-window`), so the halves are always zero there. Only when no run is free under 4 GiB is one taken higher (the job carries the high halves, ferros_cam job; a write that loses them faults at the fence, `diag`). A vault entry `cam.region` reading `high` asks higher first, to measure that.
    // The job's region with an image ring (`RINGS`, the most that fits first) and one frame more, for the engine's long ring (`RING`) and the program's copies (`PROG_AT`).
    let mut hint = [0u8; 8];
    let low_first = get(g, b"region", &mut hint) != Some(&b"high"[..]);
    let ask = |frames: u64, low: bool| ferros_sys::svc(ferros_sys::nr::DMA, cap(g, S_REGION), 3, frames, low as u64, 0);
    let mut got = None;
    'ask: for low in [low_first, !low_first] {
        for &n in RINGS.iter() {
            let frames = job::main_ring(n).bytes / job::FRAME + 1;
            let pa = ask(frames, low);
            if pa != ERR {
                got = Some((pa, n, frames));
                break 'ask;
            }
        }
    }
    let Some((pa, ring, frames)) = got else {
        return Err(Fail::Region);
    };
    let l = job::main_ring(ring);
    let va = ferros_sys::dma(cap(g, S_REGION));
    if va == ERR {
        return Err(Fail::Region);
    }
    let va = va as usize;
    say!(g, "camera: A#{frames} frames at G#{pa:X}, read at G#{va:X}, an image ring of A#{ring}\n");
    if !ferros_sys::power(cap(g, S_POWER), true) {
        return Err(Fail::Switch);
    }
    let powered = ferros_sys::us_since(t0);
    let mut id = [0u8; 2];
    bus.write_read(SENSOR, &[0x00, 0x00], &mut id).map_err(Fail::Sensor)?;
    if id != [0x08, 0xEF] {
        return Err(Fail::Sensor(u16::from_be_bytes(id) as u32 | 1 << 16));
    }
    // The inputs: the bundle if the vault holds one, else the init and the program apart; the job.
    let (init, prog) = unsafe {
        match get(g, b"frame", &mut *(&raw mut BUNDLE)).and_then(inputs::split_bundle) {
            Some(both) => both,
            None => (
                get(g, b"boitata.init", &mut *(&raw mut INIT)).ok_or(Fail::Inputs("no cam.boitata.init (nor cam.frame) in the vault"))?,
                get(g, b"ispfe.prog", &mut *(&raw mut PROG)).ok_or(Fail::Inputs("no cam.ispfe.prog (nor cam.frame) in the vault"))?,
            ),
        }
    };
    let prog = inputs::prog(prog).ok_or(Fail::Inputs("the CPU program is not a CPR2"))?;
    let jb = unsafe { &mut *(&raw mut JOB) };
    let jn = get(g, b"boitata.pdma", jb).ok_or(Fail::Inputs("no cam.boitata.pdma in the vault"))?.len();
    // The sensor's init, record by record.
    let t_init = ferros_sys::now();
    let (mut w, mut r) = (0u32, 0u32);
    for (k, rec) in inputs::records(init).enumerate() {
        let res = match rec {
            Ok(inputs::Rec::Write(b)) => {
                w += 1;
                bus.write(SENSOR, b)
            }
            Ok(inputs::Rec::Read { count, reg }) => {
                r += 1;
                let mut v = [0u8; 8];
                // WHY the min: the record's count is the table's, and a read past this buffer would index out of it; the value is dropped anyway.
                bus.write_read(SENSOR, &reg, &mut v[..(count as usize).min(8)])
            }
            Ok(inputs::Rec::Delay(us)) => {
                ferros_sys::delay_us(us as u64);
                Ok(())
            }
            Err(_) => Err(0xBAD),
        };
        if let Err(e) = res {
            return Err(Fail::Init(k as u32, e));
        }
    }
    say!(g, "camera: switch on in A#{} ms; sensor G#08EF; init A#{w} writes, A#{r} reads in A#{} ms\n", powered / 1000, ferros_sys::us_since(t_init) / 1000);
    // The job moved into the region, then placed there: the ring and the program in the region's first frame, the outputs patterned.
    let m = job::relocate(&mut jb[..jn], pa, &l).map_err(Fail::Job)?;
    let pieces = job::pieces(&jb[..jn]).map_err(Fail::Job)?;
    for &(old, at, len) in pieces.iter() {
        let dst = va + (old - job::OLD_JOB) as usize;
        for (i, &b) in jb[at..at + len].iter().enumerate() {
            unsafe { core::ptr::write_volatile((dst + i) as *mut u8, b) };
        }
    }
    for &(off, size) in l.slots.iter().filter(|s| s.1 != 0) {
        let p = (va + off as usize) as *mut u64;
        for i in 0..size as usize / 8 {
            unsafe { core::ptr::write_volatile(p.add(i), PATTERN) };
        }
    }
    // The ring's program copies in the long ring's frame, copy b naming buffer b where the program writes the image body's address (ferros_cam job `image_words`); everything else in them the job's own, moved.
    let long = l.bytes as usize;
    let (list_at, list_len) = job::list_piece(&jb[..jn]).map_err(Fail::Job)?;
    let (w_lo, w_hi) = job::image_words(&jb[list_at..list_at + list_len]).map_err(Fail::Job)?;
    if list_len > PROG_STRIDE {
        return Err(Fail::Inputs("the job's program is longer than a ring copy's room"));
    }
    let ring_off = l.slots[job::OLD_SLOT_IMAGE].0 as usize;
    for b in 0..ring {
        let dst = va + long + PROG_AT + b * PROG_STRIDE;
        let buf = pa + (ring_off + b * job::IMAGE_BYTES as usize) as u64;
        for i in 0..list_len / 4 {
            let w = if i == w_lo {
                buf as u32
            } else if i == w_hi {
                (buf >> 32) as u32
            } else {
                u32::from_le_bytes(jb[list_at + i * 4..list_at + i * 4 + 4].try_into().unwrap())
            };
            unsafe { core::ptr::write_volatile((dst + i * 4) as *mut u32, w) };
        }
    }
    ferros_sys::dma_barrier();
    say!(g, "camera: job placed: A#{} address registers, A#{} high halves (A#{} left as captured), A#{} loads, A#{} descriptors; A#{ring} program copies of A#{list_len} bytes, the image's words A#{w_lo} and A#{w_hi}\n", m.addr_regs, m.highs, m.highs_left, m.loads, m.ring);
    // The CPU's setup writes before the engine starts, in the capture's order; without them the engine applies the program and the frame writer stays idle (2026-10-20).
    let mut n = 0;
    for (a, v) in prog.before.iter() {
        if inputs::cpu_write_allowed(a) {
            wr(g, a, v);
            n += 1;
        }
    }
    // The long ring in the region's last frame: every descriptor the job's first (moved already, so it names the program in the region), the job's own ring left at the region's start for an engine whose count does not hold it. Each is made to name a ring buffer's program copy as it is released (`ring_turn`).
    let first = unsafe { core::ptr::read_volatile(va as *const [u32; 4]) };
    for e in 0..RING as usize {
        let d = (va + long + e * 16) as *mut [u32; 4];
        unsafe { core::ptr::write_volatile(d, first) };
    }
    ferros_sys::dma_barrier();
    // The engine, exactly as a camera open starts it, its ring in the region; its count read back, and the job's ring in its place if it does not hold the long one's.
    let pd = ferros_cam::PDMA;
    wr(g, pd + ferros_cam::PDMA_COUNT, RING as u32);
    let held = rd(g, pd + ferros_cam::PDMA_COUNT) as u64 == RING;
    let (ring_pa, ring_n) = if held { (pa + long as u64, RING) } else { (pa, RING_JOB) };
    RING_N.store(ring_n, Ordering::Relaxed);
    // The image ring's places for the frame thread, and its first buffer given to descriptor 0 (released with the stream below): every buffer patterned above, so its probe shows the frame that lands.
    RING_VA.store((va + ring_off) as u64, Ordering::Relaxed);
    PROG_VA.store((va + long + PROG_AT) as u64, Ordering::Relaxed);
    PROG_PA.store(pa + (long + PROG_AT) as u64, Ordering::Relaxed);
    DESC_VA.store((va + (ring_pa - pa) as usize) as u64, Ordering::Relaxed);
    DESC_WORDS.store(first[0] as u64 | (first[3] as u64) << 32, Ordering::Relaxed);
    for b in 0..job::RING_MOST {
        B_COUNT[b].store(0, Ordering::Relaxed);
    }
    for a in AGE.iter() {
        a.store(0, Ordering::Relaxed);
    }
    for c in [&UNSEEN, &CROWDED, &NONE_FREE] {
        c.store(0, Ordering::Relaxed);
    }
    SEQ.store(1, Ordering::Relaxed);
    B_SEQ[0].store(0, Ordering::Relaxed);
    LAST_R.store(0, Ordering::Relaxed);
    STATE.store(1 << QUEUED, Ordering::Release);
    RING_BUFS.store(ring as u64, Ordering::Release);
    desc_write(0, 0);
    ferros_sys::dma_barrier();
    wr(g, pd + 0x2C, 1);
    wr(g, pd + 0x40, 0x1F);
    wr(g, pd + 0x20, 0);
    wr(g, pd + ferros_cam::PDMA_BASE_LO, ring_pa as u32);
    wr(g, pd + ferros_cam::PDMA_BASE_HI, (ring_pa >> 32) as u32);
    wr(g, pd + ferros_cam::PDMA_COUNT, ring_n as u32);
    wr(g, pd + ferros_cam::PDMA_INDEX, 0);
    wr(g, pd + ferros_cam::PDMA_CTRL, 1);
    wr(g, pd + 0x34, 0);
    wr(g, pd + 0x28, 1);
    // The receiver last, as a camera open does it: enabling the link first left the front end latching its configuration and never seeing a frame start (2026-10-20).
    for &(a, v) in ferros_cam::rx::RX_START.iter() {
        wr(g, a, v);
    }
    for (a, v) in prog.after.iter() {
        if inputs::cpu_write_allowed(a) {
            wr(g, a, v);
            n += 1;
        }
    }
    let refused = REFUSED.load(Ordering::Relaxed);
    say!(g, "camera: engine started (ring G#{ring_pa:X}, A#{ring_n} descriptors{}), receiver on, A#{n} CPU writes, A#{refused} refused\n", if held { "" } else { ": the count did not hold the long ring" });
    let o = Open { va, l, ring, exposure_lines: ferros_cam::MAIN_EXPOSURE_MAX_LINES, gain: 1 << 10, focus: 0x3800, asked_at: 0, probed: false, view: false, given: 0, held: None, maps: 0, lat_sum: 0, lat_max: 0, overwritten: 0 };
    NOW_LINES.store(o.exposure_lines, Ordering::Relaxed);
    NOW_GAIN.store(o.gain, Ordering::Relaxed);
    NOW_FOCUS.store(o.focus, Ordering::Relaxed);
    // The lens to about a metre (its measured sharpest over a floor), then the stream.
    if let Err(e) = focus_init(&bus).and_then(|_| focus_set(&bus, o.focus)) {
        say!(g, "camera: the lens actuator did not answer (G#{e:X}); it stays where it rests\n");
    }
    FRAMES.store(0, Ordering::Release);
    bus.write(SENSOR, &[0x01, 0x00, 0x01, 0x03]).map_err(Fail::Sensor)?;
    // The first descriptor released and the frame thread on (Linux's start writes +G#10 = G#10 right after the engine's start).
    wr(g, pd + ferros_cam::PDMA_INDEX, STEP);
    RUN.store(1, Ordering::Release);
    ferros_sys::wake(&RUN, 1);
    say!(g, "camera: streaming A#{} ms after the open\n", ferros_sys::us_since(t0) / 1000);
    Ok(o)
}

/// The stream stopped and the switch off: the sensor's stream off, a frame's time for its last frame to land (67.8 ms measured; 2^16 µs, as `cam pdma` waited 50 ms before stopping the engine), the frame thread idle, then the switch, whose off stops the engine, replays Linux's close, shuts the S2MPUs and powers the module down (kernel camera_power.rs).
fn close(g: u64) {
    // The view taken back first (SYS_DMA x1 = 5): its blocks the blank frame, the holder's token dead, before the stream stops. Nothing when none was given.
    ferros_sys::svc(ferros_sys::nr::DMA, cap(g, S_REGION), 5, 0, 0, 0);
    let bus = Bus { g };
    let _ = bus.write(SENSOR, &[0x01, 0x00, 0x00, 0x00]);
    ferros_sys::delay_us(1 << 16);
    // The frame thread stopped before the switch stops the engine: it is in a wait of at most `QUIET_US` (or the tick), and answers no more once it sees this. Bounded at twice that, so a thread that never answers cannot keep the switch on.
    if RUN.swap(2, Ordering::AcqRel) == 1 {
        let until = ferros_sys::now() + ferros_sys::freq() * (QUIET_US << 1) / 1_000_000;
        while RUN.load(Ordering::Acquire) == 2 && ferros_sys::now() < until {
            ferros_sys::wait(&RUN, 2, until);
        }
    }
    // Tried again when refused: `power` moves this thread to the kernel's core with a millisecond's sleep, and a call arriving on the endpoint it polls ends that sleep early, so the switch can be asked from a worker core and refused there (the Pro, 2026-11-14, a holder's CLOSE right behind its last FRAME). Four tries; the kernel turns the switch off itself when this process ends regardless.
    let mut ok = false;
    for _ in 0..4 {
        ok = ferros_sys::power(cap(g, S_POWER), false);
        if ok {
            break;
        }
    }
    say!(g, "camera: closed after A#{} frames (A#{} answers, A#{} on the line{}); image status bits seen G#{:X}; switch off {}\n", FRAMES.load(Ordering::Relaxed), ANSWERED.load(Ordering::Relaxed), ON_LINE.load(Ordering::Relaxed), if TICKING.load(Ordering::Relaxed) == 1 { ", the rest on the tick" } else { "" }, IMG_SEEN.load(Ordering::Relaxed), if ok { "done" } else { "refused" });
}

/// After the eighth frame, once an open: every output's extent, sampled every 256 bytes against the pattern (docs/CAMERA.md: "measure which of the 22 the main camera's job really writes"): its slot, how many points moved, the last.
fn probe(g: u64, o: &Open) {
    let mut written = 0u32;
    for (k, &(off, size)) in o.l.slots.iter().enumerate().filter(|s| s.1 .1 != 0) {
        let base = o.va + off as usize;
        let (mut moved, mut last) = (0u64, 0u64);
        for at in (0..size).step_by(256) {
            if unsafe { core::ptr::read_volatile((base + at as usize) as *const u64) } != PATTERN {
                moved += 1;
                last = at;
            }
        }
        if moved != 0 {
            written |= 1 << k;
            say!(g, "camera: output A#{k} (A#{} KiB at +G#{off:X}): A#{moved} of A#{} points written, the last at +G#{last:X}\n", size >> 10, size / 256);
        }
    }
    say!(g, "camera: A#{} of the A#22 outputs written (bits G#{written:X})\n", written.count_ones());
}

// ---------------------------------------------------------------- frames

const W: usize = 4080;
const H: usize = 3072;
/// One RAW10 line: four pixels in five bytes, 4080 pixels on a 256-byte boundary (G#1400).
const STRIDE: usize = 0x1400;
/// A ring buffer's probe: its last line's first 64 bytes, written last of the frame (October's extent: the image body to +G#EFFF00).
const PROBE_AT: usize = (H - 1) * STRIDE;
const HEADER: usize = 1 << 6;
/// The image rings the open asks for, the most first, under 4 GiB first (`open`): four buffers leave one free for the engine with one held by the kist, one by the binning or the newest, and one with the engine; three and two still never write a held one, but may write over the newest; one is the region before the view (40 frames with the long ring's), so a pool with room for no more (a jumped session's, about 150 MiB free on 2026-11-15) still opens the camera as before, with frames by copy only and no view (`VIEW_LEAST`).
const RINGS: [usize; 4] = [4, 3, 2, 1];
/// The fewest buffers a view is given with: with one, the engine's next frame could only go where the kist holds.
const VIEW_LEAST: usize = 2;
/// The largest reply: bin 2, 2040 x 1536 samples of 16 bits, and the header.
static mut OUT: [u8; HEADER + (W / 2) * (H / 2) * 2] = [0; HEADER + (W / 2) * (H / 2) * 2];
const BIN_MOST: usize = 1 << 3;
/// How many threads bin a frame: the server thread and three binners, one for each A715 (coordinator, 2026-11-14). On one thread a frame was a 36 ms burst every 67 ms, which filled the clock governor's 15.6 ms windows, so it ramped the A715s to their top and held the active profile while streaming; four quarters of it fill none.
const BINNERS: usize = 4;
/// Each binner's lines: the 2 x bin lines a row pair reads (at most 16), copied out of the non-cacheable region in 64-byte reads, unpacked to 16 bits, and the summed line `bin_row` works in.
struct Scratch {
    raw: [u64; 2 * BIN_MOST * STRIDE / 8],
    pix: [u16; 2 * BIN_MOST * W],
    acc: [u16; W],
}
static mut SCRATCH: [Scratch; BINNERS] = [const { Scratch { raw: [0; 2 * BIN_MOST * STRIDE / 8], pix: [0; 2 * BIN_MOST * W], acc: [0; W] } }; BINNERS];
static mut BIN_STACKS: [[u8; 1 << 14]; BINNERS - 1] = [[0; 1 << 14]; BINNERS - 1];
/// A frame's binning handed to the binners: its count (a new one each frame, the binners' doorbell), the image's address and the bin; and how many binners have finished it.
static BIN_JOB: AtomicU64 = AtomicU64::new(0);
static BIN_SRC: AtomicU64 = AtomicU64::new(0);
static BIN_BIN: AtomicU64 = AtomicU64::new(0);
static BIN_DONE: AtomicU64 = AtomicU64::new(0);

/// Counter ticks spent copying lines out of the region, summed over frames and binners since the last console line (the rest of the binning is unpacking and averaging in cached memory).
static COPY_TICKS: AtomicU64 = AtomicU64::new(0);

/// Binner `k`'s share of a frame: every `BINNERS`th row pair from its own, into `OUT`. Row pairs are disjoint between binners, so no two write the same bytes of `OUT`.
fn bin_part(k: usize, src: usize, bin: usize) {
    let (bw, bh) = (W / bin, H / bin);
    let s = unsafe { &mut (*(&raw mut SCRATCH))[k] };
    let out = (&raw mut OUT) as *mut u8;
    let rows = 2 * bin;
    let words = STRIDE / 8;
    for pair in (k..bh / 2).step_by(BINNERS) {
        // The 2 x bin lines this row pair reads, copied out of the region whole, then unpacked (ferros_cam raw, host-tested against std/camreplay's binning of a recorded frame).
        for r in 0..rows {
            // 64 bytes a load (four NEON registers), not 8: each load of non-cacheable memory is its own transaction, and 8-byte ones took 121 ms a frame (the Pro, 2026-11-14).
            let t = ferros_sys::now();
            let line = (src + (pair * rows + r) * STRIDE) as *const u8;
            let dst = unsafe { s.raw.as_mut_ptr().add(r * words) } as *mut u8;
            for i in 0..STRIDE / 64 {
                unsafe {
                    let v = core::arch::aarch64::vld1q_u8_x4(line.add(i * 64));
                    core::arch::aarch64::vst1q_u8_x4(dst.add(i * 64), v);
                }
            }
            COPY_TICKS.fetch_add(ferros_sys::now() - t, Ordering::Relaxed);
            let bytes = unsafe { core::slice::from_raw_parts(s.raw.as_ptr().add(r * words) as *const u8, STRIDE) };
            ferros_cam::raw::unpack(bytes, &mut s.pix[r * W..(r + 1) * W]);
        }
        for phase in 0..2 {
            let oy = pair * 2 + phase;
            // SAFETY: row oy of the output is this binner's alone (its row pair), inside `OUT` (bw x bh samples of 2 bytes after the header, bh at most H / 2).
            let row = unsafe { core::slice::from_raw_parts_mut(out.add(HEADER + oy * bw * 2), bw * 2) };
            ferros_cam::raw::bin_row(&s.pix[..rows * W], W, bin, phase, &mut s.acc, row);
        }
    }
}

/// A binner: waits on the frame's doorbell, does its share, says so.
extern "C" fn binner(k: u64) -> ! {
    let mut seen = 0;
    loop {
        let job = BIN_JOB.load(Ordering::Acquire);
        if job == seen {
            ferros_sys::wait(&BIN_JOB, job, 0);
            continue;
        }
        seen = job;
        bin_part(k as usize, BIN_SRC.load(Ordering::Acquire) as usize, BIN_BIN.load(Ordering::Acquire) as usize);
        BIN_DONE.fetch_add(1, Ordering::AcqRel);
        ferros_sys::wake(&BIN_DONE, 1);
    }
}

/// Ring buffer `b`'s image body: a RAW10 frame as the front end writes it.
fn image(o: &Open, b: usize) -> usize {
    o.va + o.l.slots[job::OLD_SLOT_IMAGE].0 as usize + b * job::IMAGE_BYTES as usize
}

/// A frame's header (docs/CAMERA.md "The endpoint"), for buffer `b`'s frame at `bin`: count, width and height after binning, bin, cfa, black and white, exposure (ns), gain, focus, when it ended (Eagle ns); for the view (bin 1) also the buffer at +40, the format at +41 (1: MIPI RAW10, four samples in five bytes), the line's bytes at +44 and the buffer's place in the view at +48.
fn header(h: &mut [u8], b: usize, bin: usize) {
    let (bw, bh) = (W / bin, H / bin);
    h.fill(0);
    h[0..4].copy_from_slice(&(B_COUNT[b].load(Ordering::Relaxed) as u32).to_le_bytes());
    h[4..6].copy_from_slice(&(bw as u16).to_le_bytes());
    h[6..8].copy_from_slice(&(bh as u16).to_le_bytes());
    h[8] = bin as u8;
    h[9] = 1 | 2 << 2 | 0 << 4 | 1 << 6;
    h[10..12].copy_from_slice(&64u16.to_le_bytes());
    h[12..14].copy_from_slice(&1023u16.to_le_bytes());
    h[16..24].copy_from_slice(&(B_LINES[b].load(Ordering::Relaxed) * ferros_cam::MAIN_LINE_PS / 1000).to_le_bytes());
    h[24..28].copy_from_slice(&(B_GAIN[b].load(Ordering::Relaxed) as u32).to_le_bytes());
    h[28..32].copy_from_slice(&(B_FOCUS[b].load(Ordering::Relaxed) as u32).to_le_bytes());
    h[32..40].copy_from_slice(&B_AT[b].load(Ordering::Relaxed).to_le_bytes());
    if bin == 1 {
        h[40] = b as u8;
        h[41] = 1;
        h[44..48].copy_from_slice(&(STRIDE as u32).to_le_bytes());
        h[48..56].copy_from_slice(&(b as u64 * job::IMAGE_BYTES).to_le_bytes());
    }
}

/// The newest written buffer, held for `bit` (`HOLD_KIST` or `HOLD_BIN`) when its frame's count is past `after`: one compare-and-swap with the frame thread's turn, so a buffer is never both held and given to the engine. None when the newest is not past `after` (or there is none yet).
fn hold_newest(bit: u32, after: u64) -> Option<usize> {
    loop {
        let s = STATE.load(Ordering::Acquire);
        let b = newest_of(s)?;
        if B_COUNT[b].load(Ordering::Relaxed) <= after {
            return None;
        }
        if STATE.compare_exchange(s, s | 1 << (bit + b as u32), Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return Some(b);
        }
    }
}
fn unhold(bit: u32, b: usize) {
    STATE.fetch_and(!(1 << (bit + b as u32)), Ordering::AcqRel);
}

/// Wait for a buffer whose frame is past `after` and hold it for `bit`, until `FRAME_LATE_US` passes: the sensor sends one every 67.8 ms, so none by then is a stream that stopped (the front end stops for good when it is not answered in time), and the caller is told rather than kept waiting.
fn wait_hold(bit: u32, after: u64) -> Option<usize> {
    let until = ferros_sys::now() + ferros_sys::freq() * FRAME_LATE_US / 1_000_000;
    loop {
        let n = FRAMES.load(Ordering::Acquire);
        if let Some(b) = hold_newest(bit, after) {
            return Some(b);
        }
        if ferros_sys::now() >= until {
            return None;
        }
        ferros_sys::wait(&FRAMES, n, until);
    }
}

/// Buffer `b`'s frame into `OUT`, binned `bin` x `bin` per colour: output sample (ox, oy) is the mean of the bin x bin same-colour samples of its 2 bin x 2 bin block, so the mosaic keeps its phase (G B / R G), after its header (`header`). Its length.
fn bin_frame(o: &Open, bin: usize, b: usize) -> usize {
    let (bw, bh) = (W / bin, H / bin);
    let src = image(o, b);
    // The binners rung with this frame, this thread doing its own share, then waiting for theirs.
    BIN_SRC.store(src as u64, Ordering::Release);
    BIN_BIN.store(bin as u64, Ordering::Release);
    BIN_DONE.store(0, Ordering::Release);
    BIN_JOB.fetch_add(1, Ordering::AcqRel);
    ferros_sys::wake(&BIN_JOB, (BINNERS - 1) as u64);
    bin_part(0, src, bin);
    loop {
        let d = BIN_DONE.load(Ordering::Acquire);
        if d == (BINNERS - 1) as u64 {
            break;
        }
        ferros_sys::wait(&BIN_DONE, d, 0);
    }
    let out = unsafe { &mut *(&raw mut OUT) };
    header(&mut out[..HEADER], b, bin);
    HEADER + bw * bh * 2
}

// ---------------------------------------------------------------- the endpoint

/// An OPEN's reply with the view: `[0]`, the view's token (32 bytes, at 1), the ring's buffers (33), the format (34: 1 MIPI RAW10), width and height (u16 at 36, 38), a line's bytes (u32 at 40), a buffer's bytes in the view (u32 at 44).
const VIEW_REPLY: usize = 48;

/// The kist's held buffer given back: its hold cleared, and its probe compared with what it read when handed out, which tells whether anything wrote the buffer while it was held (the ring's promise: never; counted, and said the first time).
fn release(g: u64, o: &mut Open, b: usize) {
    let Some((h, then)) = o.held else { return };
    if h != b {
        return;
    }
    if probe_read(b) != then {
        o.overwritten += 1;
        if o.overwritten == 1 {
            say!(g, "camera: buffer A#{b} was written while the kist held it (frame A#{})\n", o.given);
        }
    }
    unhold(HOLD_KIST, b);
    o.held = None;
}

/// No frame came in `FRAME_LATE_US`: said with the words that tell why.
fn late(g: u64) {
    say!(g, "camera: no frame in A#{} ms (A#{} so far; A#{} answers, A#{} on the line; image status bits seen G#{:X}; ring frames unseen A#{}); closing\n", FRAME_LATE_US / 1000, FRAMES.load(Ordering::Relaxed), ANSWERED.load(Ordering::Relaxed), ON_LINE.load(Ordering::Relaxed), IMG_SEEN.load(Ordering::Relaxed), UNSEEN.load(Ordering::Relaxed));
}

/// Frames that may pass with nobody asking before the holder is taken for gone (about 4.4 s at 14.5 a second).
const UNASKED: u64 = 1 << 6;
/// How long a FRAME waits for its frame before the stream is taken for stopped: about thirty frame periods, past the sensor's start after stream on.
const FRAME_LATE_US: u64 = 1 << 21;

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(g: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    ferros_sys::thread_spawn(frame_thread, unsafe { &mut *(&raw mut FRAME_STACK) }, g);
    for k in 1..BINNERS {
        ferros_sys::thread_spawn(binner, unsafe { &mut (*(&raw mut BIN_STACKS))[k - 1] }, k as u64);
    }
    let mut msg = [0u8; 64];
    let mut token = [0u8; 32];
    let mut open_cam: Option<Open> = None;
    // Binning's measures, for the console every 64 frames served.
    let (mut served, mut bin_us, mut since) = (0u64, 0u64, ferros_sys::now());
    // The first receive waits (the call that started this driver is queued already; one spawned by hand waits for its first); after it, a look, so a driver with nothing open and nothing waiting ends.
    let mut poll = false;
    loop {
        let got = recv(cap(g, S_SERVE), &mut msg, &mut token, poll);
        poll = true;
        if got == NOTHING {
            match &open_cam {
                // Nothing open and nothing waiting: end (a call after this look starts another driver, process.rs `ON_DEMAND`).
                None => ferros_sys::exit(0),
                Some(o) => {
                    // Open: wait for a frame or a call. A holder gone quiet past `UNASKED` frames is gone.
                    let seen = FRAMES.load(Ordering::Acquire);
                    if seen > o.asked_at + UNASKED {
                        say!(g, "camera: A#{UNASKED} frames with nobody asking; its holder is gone\n");
                        close(g);
                        ferros_sys::exit(0);
                    }
                    ferros_sys::wait_world_or_call(&FRAMES, seen, 0);
                    continue;
                }
            }
        }
        if got == ERR || got == 0 {
            continue;
        }
        match msg[0] {
            0 => {
                reply(&token, LIST);
            }
            1 => {
                let r = if got < 2 || msg[1] != 0 {
                    1
                } else if open_cam.is_some() {
                    2
                } else {
                    match open(g) {
                        Ok(o) => {
                            open_cam = Some(o);
                            0
                        }
                        Err(f) => {
                            match f {
                                Fail::Region => say!(g, "camera: no region (the console's `camera region` line says why)\n"),
                                Fail::Switch => say!(g, "camera: the switch refused on (the flash log says why)\n"),
                                Fail::Sensor(e) => say!(g, "camera: the sensor did not answer as boitata (G#{e:X})\n"),
                                Fail::Inputs(why) => say!(g, "camera: {why}\n"),
                                Fail::Init(k, e) => say!(g, "camera: the init failed at record A#{k} (G#{e:X})\n"),
                                Fail::Job(e) => say!(g, "camera: the job would not move into the region ({e:?})\n"),
                            }
                            if !matches!(f, Fail::Region | Fail::Switch) {
                                let _ = ferros_sys::power(cap(g, S_POWER), false);
                            }
                            3
                        }
                    }
                };
                // The view, when the open asked for it (`[1][id][1]`): the kernel gives the caller a read-only view of the image ring and a token for it (SYS_DMA x1 = 4), which the reply carries with the ring's shape; a view the kernel refuses (the console says why) leaves the open as a plain one, `[0]` alone, and frames by copy still work.
                let mut out = [0u8; VIEW_REPLY];
                out[0] = r;
                let mut len = 1;
                if let (0, Some(o)) = (r, open_cam.as_mut()) {
                    if got >= 3 && msg[2] == 1 && o.ring < VIEW_LEAST {
                        say!(g, "camera: no view: the pool gave a ring of one buffer; frames by copy only\n");
                    } else if got >= 3 && msg[2] == 1 {
                        let first = o.l.slots[job::OLD_SLOT_IMAGE].0 / job::FRAME;
                        let n = o.ring as u64 * job::IMAGE_BYTES / job::FRAME;
                        let mut t = [0u8; 32];
                        let base = ferros_sys::svc(ferros_sys::nr::DMA, cap(g, S_REGION), 4, ferros_sys::off(token.as_ptr()), first | n << 32, ferros_sys::off(t.as_mut_ptr()));
                        if base != ERR {
                            o.view = true;
                            out[1..33].copy_from_slice(&t);
                            out[33] = o.ring as u8;
                            out[34] = 1;
                            out[36..38].copy_from_slice(&(W as u16).to_le_bytes());
                            out[38..40].copy_from_slice(&(H as u16).to_le_bytes());
                            out[40..44].copy_from_slice(&(STRIDE as u32).to_le_bytes());
                            out[44..48].copy_from_slice(&(job::IMAGE_BYTES as u32).to_le_bytes());
                            len = VIEW_REPLY;
                            t.fill(0);
                            say!(g, "camera: the view of A#{} buffers given to the holder\n", o.ring);
                        } else {
                            say!(g, "camera: the kernel gave no view (its console line says why); frames by copy only\n");
                        }
                    }
                }
                reply(&token, &out[..len]);
                if r == 3 {
                    ferros_sys::exit(0);
                }
            }
            2 => {
                let Some(o) = open_cam.as_mut() else {
                    reply(&token, &[]);
                    continue;
                };
                let bin = if got >= 2 { msg[1] as usize } else { 0 };
                if !matches!(bin, 2 | 4 | 8) {
                    reply(&token, &[]);
                    continue;
                }
                // The next frame: one counted after this request came, its buffer held while it is binned (the newest, `wait_hold`), so the engine is never given it meanwhile.
                let Some(b) = wait_hold(HOLD_BIN, FRAMES.load(Ordering::Acquire)) else {
                    late(g);
                    close(g);
                    reply(&token, &[]);
                    ferros_sys::exit(0);
                };
                let n = FRAMES.load(Ordering::Acquire);
                o.asked_at = n;
                if n >= 8 && !o.probed {
                    o.probed = true;
                    probe(g, o);
                }
                let t = ferros_sys::now();
                let len = bin_frame(o, bin, b);
                unhold(HOLD_BIN, b);
                bin_us += ferros_sys::us_since(t);
                served += 1;
                if served % 64 == 0 {
                    let us = ferros_sys::us_since(since);
                    let copy_us = COPY_TICKS.swap(0, Ordering::Relaxed) * 1_000_000 / ferros_sys::freq();
                    say!(g, "camera: A#{served} frames served, the last 64 in A#{} ms (A#{} us apart), binning A#{} us each on A#{BINNERS} threads (copying out of the region A#{} us of their time); A#{} taken by the front end\n", us / 1000, us / 64, bin_us / 64, copy_us / 64, n);
                    (bin_us, since) = (0, ferros_sys::now());
                }
                if reply(&token, unsafe { &(&*(&raw const OUT))[..len] }) == ERR {
                    say!(g, "camera: a frame's reply failed; its holder is gone\n");
                    close(g);
                    ferros_sys::exit(0);
                }
            }
            3 => {
                let Some(o) = open_cam.as_mut() else {
                    reply(&token, &[1]);
                    continue;
                };
                let what = if got >= 2 { msg[1] } else { 0xFF };
                let value = if got >= 10 { u64::from_le_bytes(msg[2..10].try_into().unwrap()) } else { u64::MAX };
                let bus = Bus { g };
                let r = match what {
                    0 => {
                        let lines = value * 1000 / ferros_cam::MAIN_LINE_PS;
                        if (1..=ferros_cam::MAIN_EXPOSURE_MAX_LINES).contains(&lines) && exposure_set(&bus, lines).is_ok() {
                            o.exposure_lines = lines;
                            NOW_LINES.store(lines, Ordering::Relaxed);
                            0
                        } else {
                            1
                        }
                    }
                    1 if value == 1 << 10 => 0,
                    2 if (0x0800..=0x7800).contains(&value) && focus_set(&bus, value).is_ok() => {
                        o.focus = value;
                        NOW_FOCUS.store(value, Ordering::Relaxed);
                        0
                    }
                    _ => 1,
                };
                reply(&token, &[r]);
            }
            4 => {
                if open_cam.is_some() {
                    close(g);
                }
                reply(&token, &[0]);
                ferros_sys::exit(0);
            }
            // MAP `[5][release]`: the buffer `release` names given back (G#FF none), then the newest written frame past the last one handed out, held until it is given back (here, or by RELEASE, or at the close): the reply its header (`header`, bin 1: the buffer at +40, its place in the view at +48), the samples read in place thru the view.
            5 => {
                let Some(o) = open_cam.as_mut() else {
                    reply(&token, &[]);
                    continue;
                };
                if !o.view {
                    reply(&token, &[]);
                    continue;
                }
                if got >= 2 && (msg[1] as usize) < o.ring {
                    release(g, o, msg[1] as usize);
                }
                // One buffer held at a time: a MAP while one is held gives that one back first.
                if let Some((b, _)) = o.held {
                    release(g, o, b);
                }
                let Some(b) = wait_hold(HOLD_KIST, o.given) else {
                    late(g);
                    close(g);
                    reply(&token, &[]);
                    ferros_sys::exit(0);
                };
                let n = FRAMES.load(Ordering::Acquire);
                o.asked_at = n;
                o.given = B_COUNT[b].load(Ordering::Relaxed);
                o.held = Some((b, probe_read(b)));
                let mut h = [0u8; HEADER];
                header(&mut h, b, 1);
                let lat = ferros_sys::now() - B_TICK[b].load(Ordering::Relaxed);
                o.lat_sum += lat;
                o.lat_max = o.lat_max.max(lat);
                o.maps += 1;
                if o.maps % 64 == 0 {
                    let us = |t: u64| t * 1_000_000 / ferros_sys::freq();
                    say!(g, "camera: A#{} frames mapped; DMA_DONE to the MAP's answer A#{} us on average, A#{} us at most; held buffers written over A#{}; frames by buffer age (releases back) A#{} A#{} A#{}, unseen A#{}, crowded A#{}, none free A#{}; A#{} taken by the front end\n", o.maps, us(o.lat_sum / 64), us(o.lat_max), o.overwritten, AGE[0].load(Ordering::Relaxed), AGE[1].load(Ordering::Relaxed), AGE[2].load(Ordering::Relaxed), UNSEEN.load(Ordering::Relaxed), CROWDED.load(Ordering::Relaxed), NONE_FREE.load(Ordering::Relaxed), n);
                    (o.lat_sum, o.lat_max) = (0, 0);
                }
                if reply(&token, &h) == ERR {
                    say!(g, "camera: a MAP's reply failed; its holder is gone\n");
                    close(g);
                    ferros_sys::exit(0);
                }
            }
            // RELEASE `[6][buffer]`: the held buffer given back → `[0]`, `[1]` when it was not held.
            6 => {
                let Some(o) = open_cam.as_mut() else {
                    reply(&token, &[1]);
                    continue;
                };
                let r = match o.held {
                    Some((b, _)) if got >= 2 && msg[1] as usize == b => {
                        release(g, o, b);
                        0
                    }
                    _ => 1,
                };
                reply(&token, &[r]);
            }
            _ => {
                reply(&token, &[]);
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
