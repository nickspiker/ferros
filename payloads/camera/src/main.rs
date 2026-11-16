//! The camera driver (docs/CAMERA.md "The driver"; owner, 2026-11-14: "Can we get Lumis wired up?"): the Pro's cameras served on the `camera` endpoint, started by the kernel when a call on it finds no server (process.rs `ON_DEMAND`), ended by itself once its holder closes.
//! The main camera since 2026-11-14; the ultrawide, and the front camera as far as it answers, since 2026-11-15 (owner: "would be nice to get the other cams online"; docs/CAMERA.md "The other cameras"). Each camera's facts are ferros_cam cams, one camera open at a time.
//!
//! The kernel keeps what a process may not do behind the `camera` power switch (kernel camera_power.rs): the camera buses' pins and clocks, the module's rails behind the overvoltage guard (which module: the switch's argument, ferros_sys `power_arg`), the front end's power domain and clocks, the D-PHY's isolation, the SYSMMUs in bypass and the S2MPUs fenced to this driver's frames. Here, thru its grants: the open camera's sensor and lens actuator on its bus, the front end and its PDMA engine, the CSI links and PHY, and the front end's line 0.
//!
//! An open, in the order a camera open does it (the kernel's `diag cam pdma`, which this replaces): the region taken (a run of frames from the pool, zeroed, fenced), the switch on for that camera's module, the sensor's identity read, its init replayed from the vault (`cam.<key>.init`; the main's also `cam.frame`), the PDMA job from the vault (`cam.<key>.pdma`; its IDT `cam.<key>.idt` beside them, for the list) moved into the region (ferros_cam job) and placed there, the CPU's setup writes before the engine (`cam.<key>.prog`; the main's `cam.ispfe.prog`), the engine started, the camera's receiver last, the CPU's writes after it, the lens set, the stream on. The keys: boitata (main), oksoko (ultrawide), dokkaebi (front), kraken (tele).
//! Then the front end is answered on its interrupt: every acknowledgement in the order Linux makes them (the camera's, ferros_cam cams `ack`), loch0's paths enabled after the first SHADOW_UPDATE (G#81 to G#1C32_00A0), the engine's descriptor index advanced once a frame. Left unanswered past about a frame period the front end stops for good (docs/oracle/CAMERA.md "Focus, exposure"), which is why this is its own thread on an interrupt and not the kernel's spin.
//!
//! Spawn by hand (the kernel also starts it itself on a call to `camera`):
//! ferros-bridge spawn camera.bin --name camera --serve camera --hw hsi2c1 --hw ispfe --hw csis --hw ispfe-sysreg --store cam --power camera --dma camera --irq ispfe0 --hw hsi2c3 --hw hsi2c2 --hw hsi2c4
//! Grant slots, in that order: 0 console, 1 serve camera, 2 hsi2c_1, 3 the front end, 4 the CSI links and PHY, 5 SYSREG_ISPFE, 6 the vault scope `cam.`, 7 the switch, 8 the region, 9 the line, 10 hsi2c_3 (the ultrawide), 11 hsi2c_2 (the front), 12 hsi2c_4 (the tele).
//!
//! The `camera` endpoint (docs/CAMERA.md "The endpoint"), first byte the op:
//! - `0` LIST: the list, a VSF document built here (ferros_cam list): a section per camera that can stream whose inputs and IDT are in the vault, each with its own Absolute IDT and sensitivity read from `cam.<key>.idt`.
//! - `1` OPEN + id (+ `1` for the view): `[0]` open, `[1]` no such camera (or one that cannot stream), `[2]` another holds it, `[3]` it would not start (the console says why). With the view asked for and given, `[0]` then its token and the ring's shape (`VIEW_REPLY`).
//! - `2` FRAME + bin: waits for the next frame; a 64-byte header, then the mosaic, `bin` x `bin` same-colour samples averaged, 16 bits a sample. bin 2, 4 or 8 (a whole readout is past the largest reply).
//! - `3` SET + what + value (8 bytes): what 0 exposure (ns), 1 analogue gain (1024 unity), 2 focus. `[0]` taken, `[1]` outside the range.
//! - `4` CLOSE: the view taken back, the stream stopped, the switch off, `[0]`, and the driver ends; its frames go back to the pool wiped.
//! - `5` MAP + the buffer to give back (G#FF none), with the view: waits for a frame newer than the last mapped and holds its buffer; the frame's 64-byte header (bin 1, the buffer at +40), the samples read in place thru the view (docs/CAMERA.md "The view").
//! - `6` RELEASE + buffer: the held buffer given back; `[0]`, `[1]` when it was not held.
//! - `7` PROBE + id (development, with no camera open): that camera's module powered, its sensor's identity read, the module powered down; `[0]` and the two bytes read when it answered as itself, `[1]` and them when another identity answered, `[2]` and the bus's error when it did not answer, `[3]` when the switch refused (the flash log says why), `[4]` no such camera. The tele's whole bring-up so far, and the front camera's first measure (docs/CAMERA.md "Device test plan").
//! - `8` STATS: the stream's counters (`stats`): frames the front end wrote, answers and how many came on the line, the frame thread's time in them, frames not where the ring put them, the mode's shape.
//! - `9` SLITS + after (u32): with a slit ring, waits for a frame past `after`; the ring's place and shape (`slits`). Frame n is in buffer (n - 1) mod the ring's count, its record (count, counter, Eagle time, whether its probe moved) right after its rows.
//! - `10` RATE + windows: the sensor's own frame counter (CCS G#0005) read every 2^16 us for `windows` windows (16 if none) beside the front end's count: the sensor's rate against what reached memory (`rate`).
//! - `11` CAPS (a camera open): the sensor's data formats (CCS G#00C0, its descriptors from G#00C2), its integration limits (G#1004) and the mode registers read back (`caps`).
//!
//! OPEN's mode block (docs/CAMERA.md "Modes, the bench and the slit"; owner, 2026-11-15: "straight as fast as possible full frame 1:1 and 2x binned for all back cams ... Then I want the slitscan wired up"): after `[1][id][view]`, `[kind][rows: u16][fll: u16][exposure: u16][flags][ring: log2][line length: u16]`, every field optional (`Spec`).
//! The mode is written inside the sensor's hold after its init and before stream on, the front end's geometry rewritten to match (ferros_cam ccs, geom), its buffers sized to the mode's frame.
//! A holder that ends without closing: its waiting FRAME's reply fails, or 2^22 us pass with nobody asking; either closes. The kernel takes the view back when either ends.
#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_cam::cams::{self, Cam};
use ferros_cam::{ccs, geom, inputs, job};
use ferros_sys::{cap, console, mmio_read, mmio_write, recv, reply, ERR, NOTHING};

const S_SERVE: u64 = 1;
const S_FE: u64 = 3;
const S_CSIS: u64 = 4;
const S_SYSREG: u64 = 5;
const S_STORE: u64 = 6;
const S_POWER: u64 = 7;
const S_REGION: u64 = 8;
const S_IRQ: u64 = 9;

/// Each camera's sensor bus's grant slot, by id: hsi2c_1 (the main) at 2 as before, the other three after the line (the kernel's `camera_grants`).
const S_BUS: [u64; 4] = [2, 10, 11, 12];

/// The open camera's id (ferros_cam cams), set by the open before anything reads it.
static CAM: AtomicU64 = AtomicU64::new(0);
fn cam() -> &'static Cam {
    cams::CAMS[CAM.load(Ordering::Relaxed) as usize]
}

/// The heap the list is built in (ferros_cam list: an IDT's document parsed, the list's sections and document made), at LIST only. A list of three cameras is about 1.3 KB and an IDT 7.7 KB; 2^20 bytes leaves room for vsf's working copies many times over.
const HEAP_BYTES: usize = 1 << 20;
#[repr(C, align(16))]
struct Region<const N: usize>(core::cell::UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Region<N> {}
static HEAP: Region<HEAP_BYTES> = Region(core::cell::UnsafeCell::new([0; HEAP_BYTES]));
#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

/// The cameras the list can name: those that stream.
const LISTED: [u8; 3] = [0, 1, 2];
/// The largest IDT the driver reads (each `.idt.vsf` is 7764 bytes).
const IDT_MOST: usize = 1 << 14;
static mut IDT: [u8; IDT_MOST] = [0; IDT_MOST];

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

/// The list (docs/CAMERA.md "The endpoint"): a section per camera that streams and whose inputs and IDT are in the vault (`inputs_held`, `cam.<key>.idt`), each carrying its own Absolute IDT and sensitivity read from that entry (ferros_cam list). A camera whose IDT does not read is left out and said.
fn list(g: u64) -> alloc::vec::Vec<u8> {
    let mut secs = alloc::vec::Vec::new();
    for &id in LISTED.iter() {
        let c = cams::CAMS[id as usize];
        if !inputs_held(g, c) {
            continue;
        }
        let mut kb = [0u8; 32];
        let Some(idt) = get(g, input_key(&mut kb, c, b"idt"), unsafe { &mut *(&raw mut IDT) }) else { continue };
        match ferros_cam::list::section(c, idt) {
            Ok(s) => secs.push(s),
            Err(e) => say!(g, "camera: cam.{}.idt does not read ({e:?}); {} left out of the list\n", c.key, c.name),
        }
    }
    ferros_cam::list::document(secs).unwrap_or_default()
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

// ---------------------------------------------------------------- the sensor's bus

// The HS-I2C controller (kernel hsi2c.rs, i2c-exynos5.c auto mode), as payloads/torch drives hsi2c_14: the open camera's (hsi2c_1, 3, 2 or 4, the same block each).
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

/// A camera's sensor bus, thru its grant (`S_BUS`).
struct Bus {
    g: u64,
    s: u64,
}

impl Bus {
    /// The bus of camera `c`.
    fn of(g: u64, c: &Cam) -> Bus {
        Bus { g, s: S_BUS[c.id as usize] }
    }
    fn rd(&self, off: u64) -> u32 {
        mmio_read(cap(self.g, self.s), off) as u32
    }
    fn wr(&self, off: u64, v: u32) {
        mmio_write(cap(self.g, self.s), off, v);
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

/// How long the frame thread waits for the line's first interrupt once the stream is on before it answers on a tick instead, and that tick: the camera's `tick_us` (ferros_cam cams), under the time the front end waits unanswered before it stops for good (the main's 2^15 µs, under the 50 ms its stream held at; the ultrawide's 2^13, under the 15 ms its stream held at, 2026-10-22).
fn tick_us() -> u64 {
    cam().tick_us
}
/// With the line working, the longest the frame thread waits on it before it looks whether it was asked to stop: two of the main camera's frame periods (67.8 ms each), four of the ultrawide's, so it wakes on its own only once the stream has stopped, and a close never waits on a line that will not fire again.
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
/// Where things are, set by the open before the frame thread starts (`RUN`): the first buffer as this process reads it; the full program's copies as the engine reads them (physical) and their length, the slim ones' likewise (`SLIM`); the descriptor ring as read; a descriptor's flags word (the job's own).
static RING_VA: AtomicU64 = AtomicU64::new(0);
static PROG_PA: AtomicU64 = AtomicU64::new(0);
static PROG_LEN: AtomicU64 = AtomicU64::new(0);
static SLIM_PA: AtomicU64 = AtomicU64::new(0);
static SLIM_LEN: AtomicU64 = AtomicU64::new(0);
static DESC_VA: AtomicU64 = AtomicU64::new(0);
static DESC_FLAGS: AtomicU64 = AtomicU64::new(0);
/// The engine's area past the job's region (`open`): the long ring's descriptors from its start (2^16 of 16 bytes at most, 1 MiB), the program's copies from `PROG_AT`, a full copy every `PROG_STRIDE` (the main camera's program is 6620 bytes), the slim copies after them every `SLIM_STRIDE` (176 or 680 bytes for the main's).
const PROG_AT: usize = 1 << 20;
const PROG_STRIDE: usize = 1 << 13;
const SLIM_STRIDE: usize = 1 << 10;

/// The open's shape (`open`): a buffer's bytes in the ring (the mode's rows and line in whole frames of the pool, ferros_cam job `buffer_bytes`: the Pro's binned main 16 MiB, its 1:1 frame 60 MiB, a slit a few KiB, `slit_bytes`), the frame's rows, width and line bytes, whether the descriptors after the first run the slim program, whether loch0's PD path is enabled, and the slit ring's buffer count (0: a ring of whole frames with holds, `ring_turn`; else `slit_turn`).
static BUF: AtomicU64 = AtomicU64::new(job::IMAGE_BYTES);
static ROWS: AtomicU64 = AtomicU64::new(cams::H_MOST as u64);
static COLS: AtomicU64 = AtomicU64::new(cams::W_MOST as u64);
static LINE: AtomicU64 = AtomicU64::new(cams::STRIDE_MOST as u64);
static SLIM: AtomicU64 = AtomicU64::new(0);
static PD_ON: AtomicU64 = AtomicU64::new(1);
static SLITS: AtomicU64 = AtomicU64::new(0);
/// The open's frame length, line length and integration (lines), for SET's range and STATS.
static FLL: AtomicU64 = AtomicU64::new(0);
static LLP: AtomicU64 = AtomicU64::new(0);
/// Bits a sample on the link: 10, or 12 for the RAW12 attempt.
static NOW_FORMAT: AtomicU64 = AtomicU64::new(10);
/// The frame thread's time answering the front end (counter ticks, summed and the most in one answer) and frames whose buffer was not the one the ring named (a slit ring's: the frame's probe did not move, so it landed elsewhere, its descriptor released after the frame began).
static BUSY: AtomicU64 = AtomicU64::new(0);
static BUSY_MOST: AtomicU64 = AtomicU64::new(0);
static LATE: AtomicU64 = AtomicU64::new(0);
/// Set when the long ring's last descriptor was given: the stream ends there (`RING`).
static RING_END: AtomicU64 = AtomicU64::new(0);

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
    (RING_VA.load(Ordering::Relaxed) as usize + b * BUF.load(Ordering::Relaxed) as usize + probe_off()) as *mut u64
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
    desc_for(e, r);
    ferros_sys::dma_barrier();
}

/// Descriptor `e` of the engine's ring names a copy of the program that names buffer `r`: the full program's copy, or with `SLIM` set and past the first descriptor, the slim one's (the first frame needs the whole setup: the CPU's writes leave the processing blocks and the LUTs to the program).
fn desc_for(e: u64, r: usize) {
    let (pa, len) = if SLIM.load(Ordering::Relaxed) != 0 && e != 0 {
        (SLIM_PA.load(Ordering::Relaxed) + (r * SLIM_STRIDE) as u64, SLIM_LEN.load(Ordering::Relaxed))
    } else {
        (PROG_PA.load(Ordering::Relaxed) + (r * PROG_STRIDE) as u64, PROG_LEN.load(Ordering::Relaxed))
    };
    let d = [DESC_FLAGS.load(Ordering::Relaxed) as u32, pa as u32, (pa >> 32) as u32, len as u32];
    unsafe { core::ptr::write_volatile((DESC_VA.load(Ordering::Relaxed) + e * 16) as *mut [u32; 4], d) };
}

/// A frame's DMA_DONE with a slit ring (docs/CAMERA.md "The slit"): no choice and no hold, every descriptor named its buffer at the open (descriptor e buffer e mod the ring), so frame n lands in buffer (n - 1) mod the ring when its descriptor was released before it began. Its probe says whether it did (else it went where the last one did: `LATE`), its record is written after its rows (ferros_cam job `slit_bytes` leaves it 64 bytes: the frame's count, the counter at its DMA_DONE, its Eagle time, 1 when its probe moved), and the buffer the next descriptor names has its probe patterned before that descriptor is released (the consumer keeps clear of the oldest buffer: SLITS).
fn slit_turn(n: u64) {
    let m = SLITS.load(Ordering::Relaxed) as usize;
    let b = ((n - 1) % m as u64) as usize;
    let moved = probe_read(b) != [PATTERN; 8];
    if !moved {
        LATE.fetch_add(1, Ordering::Relaxed);
    }
    let rec = (RING_VA.load(Ordering::Relaxed) as usize + b * BUF.load(Ordering::Relaxed) as usize + (ROWS.load(Ordering::Relaxed) * LINE.load(Ordering::Relaxed)) as usize) as *mut u64;
    unsafe {
        core::ptr::write_volatile(rec, n);
        core::ptr::write_volatile(rec.add(1), ferros_sys::now());
        core::ptr::write_volatile(rec.add(2), eagle_ns());
        core::ptr::write_volatile(rec.add(3), moved as u64);
    }
    probe_pattern((n % m as u64) as usize);
    ferros_sys::dma_barrier();
}

/// The front end answered once: its aggregate and loch0's image status read, every acknowledgement made in Linux's order, loch0's paths enabled after the first SHADOW_UPDATE, and on a frame's DMA_DONE the ring's turn (`ring_turn`), the engine's index advanced and the frame counted.
fn answer(g: u64, idx: &mut u32, enabled: &mut bool) {
    let t0 = ferros_sys::now();
    let img = rd(g, ferros_cam::IMG_STATUS);
    for &a in cam().ack.iter() {
        let v = rd(g, a);
        wr(g, a, v);
    }
    IMG_SEEN.fetch_or(img as u64, Ordering::Relaxed);
    ANSWERED.fetch_add(1, Ordering::Relaxed);
    if !*enabled && img & ferros_cam::IMG_SHADOW != 0 {
        // The image path alone when the PD path is off (a mode whose PD rows' slot is sized for the binned frame, or a slit): G#01 of Linux's G#81.
        let pd = PD_ON.load(Ordering::Relaxed) != 0;
        wr(g, ferros_cam::LOCH_ENABLE, if pd { ferros_cam::LOCH_ENABLE_IMG_PD } else { ferros_cam::LOCH_ENABLE_IMG_PD & 1 });
        *enabled = true;
        say!(g, "camera: SHADOW_UPDATE; loch0's image path enabled{}\n", if pd { " with its PD path" } else { ", its PD path left off" });
    }
    if img & ferros_cam::IMG_DMA_DONE != 0 {
        // The buffer this frame landed in, and the next descriptor's buffer: the descriptor released by the index write below is the one at the index now.
        let more = *idx < RING_N.load(Ordering::Relaxed) as u32 * STEP;
        let n = FRAMES.load(Ordering::Relaxed) + 1;
        if SLITS.load(Ordering::Relaxed) != 0 {
            slit_turn(n);
        } else {
            ring_turn(n, more.then_some((*idx / STEP) as u64));
        }
        // The next descriptor. Never round the ring: back to 0 the front end stops for good (`RING`). At the ring's end the index is left where it is, the console says so, and the stream ends with its last descriptor (the FRAME that finds no frame closes).
        if more {
            *idx += STEP;
            wr(g, ferros_cam::PDMA + ferros_cam::PDMA_INDEX, *idx);
            if *idx == RING_N.load(Ordering::Relaxed) as u32 * STEP {
                RING_END.store(1, Ordering::Relaxed);
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
    let t = ferros_sys::now() - t0;
    BUSY.fetch_add(t, Ordering::Relaxed);
    BUSY_MOST.fetch_max(t, Ordering::Relaxed);
}

extern "C" fn frame_thread(g: u64) -> ! {
    // Not pinned (measured on the Pro, b898, 2026-11-16): pinned to the X3 the main's stream at 96 a second stopped after 10 frames and the ultrawide's above about 38, pinned to one A715 (core 7) the main's stopped too; unpinned, with a consumer leaving one A715 free, 256 frames at 97 (main) and 48 (ultrawide) a second ran clean. A consumer on all four A715s at those rates starves it (cambench, b897): leave one free.
    let line = cap(g, S_IRQ);
    // Idle until the stream starts: the server rings RUN.
    while RUN.load(Ordering::Acquire) == 0 {
        ferros_sys::wait(&RUN, 0, 0);
    }
    let (mut idx, mut enabled, mut ticking) = (STEP, false, false);
    let started = ferros_sys::now();
    let ticks = |us: u64| ferros_sys::now() + ferros_sys::freq() * us / 1_000_000;
    loop {
        let wait = if ticking || ON_LINE.load(Ordering::Relaxed) == 0 { tick_us() } else { QUIET_US };
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
            say!(g, "camera: no interrupt on SPI 479 in A#{} us of the stream; answering on a A#{} us tick (front end aggregate G#{:X}, image G#{:X}, engine G#{:X} index G#{:X})\n", ferros_sys::us_since(started), tick_us(), rd(g, ferros_cam::CMN_STATUS), rd(g, ferros_cam::IMG_STATUS), rd(g, ferros_cam::PDMA), rd(g, ferros_cam::PDMA + ferros_cam::PDMA_INDEX));
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
    /// The counter when the holder last asked for anything (a FRAME, MAP, SLITS, STATS or RATE): 2^22 us past it with nobody asking, the holder is taken for gone.
    asked_tick: u64,
    /// The front end writes the captured geometry (the init's mode, or only its timing changed): FRAME by copy bins it; any other shape is the view's alone.
    captured: bool,
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

/// The lens actuator (the camera's `actuator`) to `pos`: reset (02 <- G#40), at least 5 ms quiet (straight after it every write NACKs), 02 <- 0, F1 <- 0 once, then the target into 00/01 (docs/oracle/CAMERA.md "Focus"). The main's protocol; the front camera's actuator takes it untried.
fn focus_init(bus: &Bus, c: &Cam) -> Result<(), u32> {
    bus.write(c.actuator, &[0x02, 0x40])?;
    ferros_sys::delay_us(1 << 13);
    bus.write(c.actuator, &[0x02, 0x00])?;
    bus.write(c.actuator, &[0xF1, 0x00])
}
fn focus_set(bus: &Bus, c: &Cam, pos: u64) -> Result<(), u32> {
    bus.write(c.actuator, &[0x00, (pos >> 8) as u8])?;
    bus.write(c.actuator, &[0x01, pos as u8])
}

/// The sensor's exposure in lines (CCS coarse_integration_time, G#0202 and G#0203, the same on every camera), inside its grouped-parameter hold (the main's G#0104 is one byte: the hold is 01 04 01 01, its release 01 04 00 01, Linux's per-frame block, camera-main-open-transfers.txt 771-778; the ultrawide's 8-bit registers take 01 04 01 and 01 04 00, the end of its captured init).
fn exposure_set(bus: &Bus, c: &Cam, lines: u64) -> Result<(), u32> {
    bus.write(c.addr, c.hold)?;
    let r = bus.write(c.addr, &[0x02, 0x02, (lines >> 8) as u8, lines as u8]);
    bus.write(c.addr, c.release)?;
    r
}

/// A vault key for camera `c`'s input: `<key>.<what>` in the scope `cam.` (the grant's), into `buf`.
fn input_key<'a>(buf: &'a mut [u8; 32], c: &Cam, what: &[u8]) -> &'a [u8] {
    let (k, n) = (c.key.as_bytes(), c.key.len());
    buf[..n].copy_from_slice(k);
    buf[n] = b'.';
    buf[n + 1..n + 1 + what.len()].copy_from_slice(what);
    &buf[..n + 1 + what.len()]
}

/// Whether the vault holds `key` (its length asked with a buffer of none: the kernel answers a value's whole length).
fn has(g: u64, key: &[u8]) -> bool {
    ferros_sys::store_get(cap(g, S_STORE), key, &mut [0u8; 8]).is_some()
}

/// Whether camera `c`'s inputs are in the vault: its IDT and its job, and its init and CPU program (the main's also as the bundle `cam.frame`, or by their older names).
fn inputs_held(g: u64, c: &Cam) -> bool {
    let mut b = [0u8; 32];
    let both = has(g, input_key(&mut b, c, b"init")) && has(g, input_key(&mut [0u8; 32], c, b"prog"));
    let main = c.id == 0 && (has(g, b"frame") || has(g, b"ispfe.prog") && has(g, b"boitata.init"));
    has(g, input_key(&mut [0u8; 32], c, b"idt")) && has(g, input_key(&mut [0u8; 32], c, b"pdma")) && (both || main)
}

/// PROBE (op 7, no camera open): camera `c`'s module powered thru the switch, its sensor's identity read, every 7-bit address on its bus asked for one byte (its register 0) to see which acknowledge, the module powered down.
/// The reply: `[code][id: 2][the error]` and the 16-byte map of the addresses that acknowledged (bit a of byte a / 8): code 0 it answered as itself, 1 another identity answered, 2 it did not answer (the bus's error after the identity), 3 the switch refused (the flash log says why).
/// The map says whether the bus and the module's 1.8 V side live when the sensor does not: the front camera's EEPROM (G#51) and lens actuator (G#0C) sit on its bus, the ultrawide's EEPROM (G#50) and actuator (G#0F) on its (docs/oracle/CAMERA.md "The buses").
fn probe_id(g: u64, c: &'static Cam) -> [u8; 20] {
    let mut out = [0u8; 20];
    CAM.store(c.id as u64, Ordering::Relaxed);
    if !ferros_sys::power_arg(cap(g, S_POWER), true, c.id) {
        out[0] = 3;
        say!(g, "camera: probe {}: the switch refused on (the flash log says why)\n", c.name);
        return out;
    }
    let bus = Bus::of(g, c);
    let mut id = [0u8; 2];
    let r = bus.write_read(c.addr, &c.id_reg, &mut id);
    out[1..3].copy_from_slice(&id);
    out[0] = match r {
        Ok(()) if id == c.id_val => 0,
        Ok(()) => 1,
        Err(e) => {
            out[3] = e as u8;
            2
        }
    };
    let mut acks = 0u32;
    for a in 0x08u8..0x78 {
        let mut b = [0u8; 1];
        if bus.write_read(a, &[0x00], &mut b).is_ok() {
            out[4 + (a >> 3) as usize] |= 1 << (a & 7);
            acks += 1;
        }
    }
    let off = ferros_sys::power(cap(g, S_POWER), false);
    say!(g, "camera: probe {}: identity at G#{:02X}{:02X} {} G#{:02X}{:02X} (wanted G#{:02X}{:02X}), A#{acks} addresses acknowledged on its bus; switch off {}\n", c.name, c.id_reg[0], c.id_reg[1], match out[0] { 0 => "answered", 1 => "answered another,", _ => "did not answer, error" }, id[0], id[1], c.id_val[0], c.id_val[1], if off { "done" } else { "refused" });
    if out[0] == 2 {
        say!(g, "camera: probe {}: the bus's error G#{:X}\n", c.name, out[3]);
    }
    for a in 0x08u8..0x78 {
        if out[4 + (a >> 3) as usize] >> (a & 7) & 1 == 1 {
            say!(g, "camera: probe {}: G#{a:02X} acknowledged\n", c.name);
        }
    }
    out
}

/// Why an open did not happen.
enum Fail {
    Region,
    Switch,
    Sensor(u32),
    Inputs(&'static str),
    Init(u32, u32),
    Job(job::Error),
    Mode(ccs::Error),
}

/// What an OPEN asks of the sensor (docs/CAMERA.md "Modes, the bench and the slit"): its mode block after `[1][id][view]`, every field optional, a shorter OPEN taking the defaults (the init's mode untouched, as every open before the bench).
/// `kind`: 0 the init's mode (Android's binned), 1 the binned mode with its frame and integration cut, 2 the 1:1 readout (a guess until a capture), 3 a binned slit, 4 a 1:1 slit.
/// `rows` a slit's output rows (binned: whole 2-row Bayer cells; 1:1, whole 4-row quad cells), `fll` the frame in lines (0: the rows and `BLANK` more; G#FFFF the captured mode's own), `exposure` the integration in lines (0: `EXPOSURE`), `flags` the `F_*` bits, `ring` a slit ring's buffers as a power of two (0: 2^8), `llp` the line length (0: the mode's).
#[derive(Clone, Copy)]
struct Spec {
    kind: u8,
    rows: u16,
    fll: u16,
    exposure: u16,
    flags: u8,
    ring: u8,
    llp: u16,
}
const K_CAPTURED: u8 = 0;
const K_BINNED: u8 = 1;
const K_FULL: u8 = 2;
const K_SLIT: u8 = 3;
const K_SLIT_FULL: u8 = 4;
/// RAW12 (the sensor's data format, the link's data type, the front end's stride; the core's own format word is not known, docs/CAMERA.md "RAW12").
const F_RAW12: u8 = 1 << 0;
/// The mode written after the first frame instead of before stream on: the one order that streamed a slit in October (2026-10-21, one change a session), and the one that left the sensor stuck until a cold boot.
const F_MID: u8 = 1 << 1;
/// loch0's PD path enabled (Linux's G#81); off by default in every mode but the init's, whose PD rows' slot is sized for the binned frame.
const F_PD: u8 = 1 << 2;
/// The front end's geometry rewritten everywhere the program and the CPU write it (`geom::Scope::All`, October's reach), not only the frame's way into memory.
const F_ALL: u8 = 1 << 3;
/// Every descriptor after the first runs the slim program (level 1: loch0's paths; with `F_SLIM2` also the processing blocks' request words).
const F_SLIM: u8 = 1 << 4;
const F_SLIM2: u8 = 1 << 5;
/// The front end left at the captured geometry (the sensor's mode alone changes: October's slit experiments).
const F_KEEP_FE: u8 = 1 << 6;
/// `fll`'s word for the mode's own frame length.
const FLL_OWN: u16 = 0xFFFF;
/// Lines of frame past a mode's rows when `fll` is 0: a 4-row slit ran at 96 lines and stopped at 68 (2026-10-21), so 2^7 past the rows holds the slit with room; a whole frame gets 2^8.
const BLANK: u16 = 1 << 7;
/// The integration (lines) when `exposure` is 0: short, so the bench measures the readout and not the light.
const EXPOSURE: u16 = 1 << 4;
/// A slit ring's buffers: 2^8 by default (a binned 4-row slit's 32 KiB each, 8 MiB), at most 2^12 (the slim copies' 4 MiB).
const SLIT_RING: u8 = 8;
const SLIT_RING_MOST: u8 = 12;
/// The long ring's descriptors for a slit: 2^16 (1 MiB; 54 s at 1212 slits a second), if the count register holds it (`RING` else, `RING_JOB` last).
const RING_SLIT: u64 = 1 << 16;

impl Spec {
    fn of(msg: &[u8], got: usize) -> Spec {
        // WHY the length test: the block is optional and the message's tail past `got` is the last call's bytes, not this one's.
        let b = |k: usize| if k < got { msg[k] } else { 0 };
        let h = |k: usize| u16::from_le_bytes([b(k), b(k + 1)]);
        Spec { kind: b(3), rows: h(4), fll: h(6), exposure: h(8), flags: b(10), ring: b(11), llp: h(12) }
    }
    fn slit(&self) -> bool {
        matches!(self.kind, K_SLIT | K_SLIT_FULL)
    }
}

/// The sensor's mode for `s` (None: the init's, untouched), built from the init's own (`ccs::Mode::captured`).
fn mode_of(c: &Cam, init: &[u8], s: &Spec) -> Result<Option<ccs::Mode>, Fail> {
    if s.kind == K_CAPTURED {
        return Ok(None);
    }
    let a = &ccs::ARRAYS[c.id as usize];
    let own = ccs::Mode::captured(init, a).map_err(Fail::Mode)?;
    let base = match s.kind {
        K_BINNED | K_SLIT => own,
        K_FULL | K_SLIT_FULL => own.unbinned(a).map_err(Fail::Mode)?,
        _ => return Err(Fail::Inputs("no such mode kind")),
    };
    let exp = if s.exposure == 0 { EXPOSURE } else { s.exposure };
    let rows = if s.slit() { s.rows } else { base.y_out };
    let fll = match s.fll {
        0 if s.slit() => rows + BLANK,
        0 => rows + (BLANK << 1),
        FLL_OWN => base.fll,
        f => f,
    };
    let mut m = if s.slit() { base.slit(a, rows, fll, exp) } else { base.timed(fll, exp) }.map_err(Fail::Mode)?;
    if s.llp != 0 {
        m.llp = s.llp;
    }
    if s.flags & F_RAW12 != 0 {
        m = m.raw12();
    }
    Ok(Some(m))
}

/// The engine's area past the job's region, in bytes: the long ring's descriptors, then `full` copies of the program and `slim` slim ones (`PROG_AT`).
fn engine_bytes(full: usize, slim: usize) -> u64 {
    (PROG_AT + full * PROG_STRIDE + slim * SLIM_STRIDE) as u64
}

fn open(g: u64, c: &'static Cam, s: &Spec) -> Result<Open, Fail> {
    OPEN_AT.store(ferros_sys::now(), Ordering::Relaxed);
    let t0 = ferros_sys::now();
    CAM.store(c.id as u64, Ordering::Relaxed);
    let bus = Bus::of(g, c);
    // The inputs first (the vault's, nothing powered yet): the camera's own (`cam.<key>.init`, `.prog`, `.pdma`); the main's also as the bundle `cam.frame` or by their older names, as before. The mode is read out of the init, and the region is sized to it.
    let mut kb = [0u8; 32];
    let (init, prog) = unsafe {
        let own = (get(g, input_key(&mut kb, c, b"init"), &mut *(&raw mut INIT)), get(g, input_key(&mut [0u8; 32], c, b"prog"), &mut *(&raw mut PROG)));
        match own {
            (Some(i), Some(p)) => (i, p),
            _ if c.id != 0 => return Err(Fail::Inputs("no cam.<key>.init or cam.<key>.prog for this camera in the vault")),
            _ => match get(g, b"frame", &mut *(&raw mut BUNDLE)).and_then(inputs::split_bundle) {
                Some(both) => both,
                None => (
                    get(g, b"boitata.init", &mut *(&raw mut INIT)).ok_or(Fail::Inputs("no cam.boitata.init (nor cam.frame) in the vault"))?,
                    get(g, b"ispfe.prog", &mut *(&raw mut PROG)).ok_or(Fail::Inputs("no cam.ispfe.prog (nor cam.frame) in the vault"))?,
                ),
            },
        }
    };
    let prog = inputs::prog(prog).ok_or(Fail::Inputs("the CPU program is not a CPR2"))?;
    let jb = unsafe { &mut *(&raw mut JOB) };
    let jn = get(g, input_key(&mut kb, c, b"pdma"), jb).ok_or(Fail::Inputs("no cam.<key>.pdma for this camera in the vault"))?.len();
    let mode = mode_of(c, init, s)?;
    // The front end's geometry: the camera's captured frame, and the one the mode sends (ferros_cam geom): its line on a 256-byte boundary, or the camera's own captured stride where the width and the format are the captured ones.
    let from = geom::Geom { w: c.width as u16, h: c.height as u16, stride: c.stride as u32, pd_h: (c.height / 4) as u16, dt: geom::DT_RAW10 };
    let to = match mode {
        None => from,
        Some(m) => {
            let g = geom::Geom::of(m.x_out, m.y_out, m.bits());
            if m.x_out as usize == c.width && m.bits() == 10 { g.with_stride(c.stride as u32) } else { g }
        }
    };
    let fe = if s.flags & F_KEEP_FE != 0 { from } else { to };
    let scope = if s.flags & F_ALL != 0 { geom::Scope::All } else { geom::Scope::Core };
    let slit = s.slit() && s.flags & F_KEEP_FE == 0;
    let slim = s.flags & F_SLIM != 0;
    let slim_level = if s.flags & F_SLIM2 != 0 { 2 } else { 1 };
    // WHY the bound: the ring's exponent is the caller's byte; past `SLIT_RING_MOST` the slim copies outgrow the engine's area and the view's window.
    let slit_n = if s.ring == 0 { 1usize << SLIT_RING } else if s.ring <= SLIT_RING_MOST { 1usize << s.ring } else { return Err(Fail::Inputs("a slit ring past 2^12 buffers")) };
    let buf = if slit { job::slit_bytes(fe.h as u64, fe.stride as u64) } else { job::buffer_bytes(fe.h as u64, fe.stride as u64) };
    // The region: one run of frames side by side, under 4 GiB first, as every October run had it (coordinator, 2026-11-14); measured on the Pro on 2026-11-15 to work above 4 GiB too (the front end drives the high address halves in bypass), where it is taken when nothing lower is free. A vault entry `cam.region` reading `high` asks higher first.
    // The job's region with an image ring (`RINGS`, the most that fits first; a slit ring its own count) of `buf`-byte buffers, and the engine's area past it (`engine_bytes`).
    let mut hint = [0u8; 8];
    let low_first = get(g, b"region", &mut hint) != Some(&b"high"[..]);
    let ask = |frames: u64, low: bool| ferros_sys::svc(ferros_sys::nr::DMA, cap(g, S_REGION), 3, frames, low as u64, 0);
    // A slit ring the count asked for, then each half of it down to 2^3, the most the window and the pool take first (a 64-row 1:1 slit's 644 KiB buffers fit 2^8 of them in the window, not 2^10).
    let mut slit_rings = [0usize; 1 + SLIT_RING_MOST as usize];
    let mut ns = 0;
    let mut k = slit_n;
    loop {
        slit_rings[ns] = k;
        ns += 1;
        if k <= 1 << 3 {
            break;
        }
        k >>= 1;
    }
    let rings: &[usize] = if slit { &slit_rings[..ns] } else { &RINGS };
    let copies = |n: usize| -> (usize, usize) {
        match (slit, slim) {
            (true, true) => (1, n),
            (_, true) => (n, n),
            (_, false) => (n, 0),
        }
    };
    let mut got = None;
    // The biggest ring first, under 4 GiB and then above it for each count: a ring of one gives no view, so a smaller ring under 4 GiB is not worth more than a bigger one above it (the Pro, b898: the low ranges held a ring of one, and the bench's open came back without a view).
    'ask: for &n in rings.iter() {
        for low in [low_first, !low_first] {
            let l = job::with_image(&c.sizes, &c.skip, n as u64 * buf);
            let (f, sl) = copies(n);
            let frames = l.bytes / job::FRAME + engine_bytes(f, sl).div_ceil(job::FRAME);
            let pa = ask(frames, low);
            if pa != ERR {
                got = Some((pa, n, frames, l));
                break 'ask;
            }
        }
    }
    let Some((pa, ring, frames, l)) = got else {
        return Err(Fail::Region);
    };
    let va = ferros_sys::dma(cap(g, S_REGION));
    if va == ERR {
        return Err(Fail::Region);
    }
    let va = va as usize;
    say!(g, "camera: {}: A#{frames} frames at G#{pa:X}, read at G#{va:X}, a{} ring of A#{ring} buffers of A#{} KiB\n", c.name, if slit { " slit" } else { "n image" }, buf >> 10);
    if !ferros_sys::power_arg(cap(g, S_POWER), true, c.id) {
        return Err(Fail::Switch);
    }
    let powered = ferros_sys::us_since(t0);
    let mut id = [0u8; 2];
    bus.write_read(c.addr, &c.id_reg, &mut id).map_err(Fail::Sensor)?;
    if id != c.id_val {
        return Err(Fail::Sensor(u16::from_be_bytes(id) as u32 | 1 << 16));
    }
    // The sensor's init, record by record.
    let t_init = ferros_sys::now();
    let (mut w, mut r) = (0u32, 0u32);
    for (k, rec) in inputs::records(init).enumerate() {
        let res = match rec {
            Ok(inputs::Rec::Write(b)) => {
                w += 1;
                bus.write(c.addr, b)
            }
            Ok(inputs::Rec::Read { count, reg }) => {
                r += 1;
                let mut v = [0u8; 8];
                // WHY the min: the record's count is the table's, and a read past this buffer would index out of it; the value is dropped anyway.
                bus.write_read(c.addr, &reg, &mut v[..(count as usize).min(8)])
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
    say!(g, "camera: {}: switch on in A#{} ms; sensor G#{:02X}{:02X}; init A#{w} writes, A#{r} reads in A#{} ms\n", c.name, powered / 1000, id[0], id[1], ferros_sys::us_since(t_init) / 1000);
    // The mode, inside the sensor's hold, after its init and before stream on (unless asked for mid-stream: after the first frame, below).
    let arr = &ccs::ARRAYS[c.id as usize];
    if let Some(m) = mode {
        say!(g, "camera: mode A#{}: A#{} x A#{} (x G#{:X} to G#{:X}, y G#{:X} to G#{:X}, increments A#{} A#{}, binning G#{:02X}{:02X}), frame A#{} lines of A#{} clocks, integration A#{} lines, format G#{:X}{}\n", s.kind, m.x_out, m.y_out, m.x_start, m.x_end, m.y_start, m.y_end, m.inc[0], m.inc[2], m.binning[0], m.binning[1], m.fll, m.llp, m.coarse, m.format, if s.flags & F_MID != 0 { "; written after the first frame" } else { "" });
        if s.flags & F_MID == 0 {
            for wr_ in m.writes(arr, c).iter() {
                bus.write(c.addr, wr_).map_err(Fail::Sensor)?;
            }
        }
    }
    // The job moved into the region, its geometry rewritten to the front end's (`fe`), then placed there: the ring and the program in the region's first frame, the outputs patterned.
    let m_ = job::relocate(&mut jb[..jn], pa, &l).map_err(Fail::Job)?;
    let (list_at, list_len) = job::list_piece(&jb[..jn]).map_err(Fail::Job)?;
    let regeom = if fe != from { geom::rewrite(&mut jb[list_at..list_at + list_len], &from, &fe, scope) } else { 0 };
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
    // The program's copies in the engine's area, copy b naming buffer b where the program writes the image body's address (ferros_cam job `image_words`); everything else in them the job's own, moved. Full copies every `PROG_STRIDE`, then the slim ones (geom `slim`) every `SLIM_STRIDE`.
    let long = l.bytes as usize;
    let (w_lo, w_hi) = job::image_words(&jb[list_at..list_at + list_len]).map_err(Fail::Job)?;
    if list_len > PROG_STRIDE {
        return Err(Fail::Inputs("the job's program is longer than a copy's room"));
    }
    let ring_off = l.slots[job::OLD_SLOT_IMAGE].0 as usize;
    let buf_pa = |b: usize| pa + (ring_off + b * buf as usize) as u64;
    let (n_full, n_slim) = copies(ring);
    let put = |dst: usize, words: &[u8], lo: usize, hi: usize, b: usize| {
        for i in 0..words.len() / 4 {
            let w = if i == lo {
                buf_pa(b) as u32
            } else if i == hi {
                (buf_pa(b) >> 32) as u32
            } else {
                u32::from_le_bytes(words[i * 4..i * 4 + 4].try_into().unwrap())
            };
            unsafe { core::ptr::write_volatile((dst + i * 4) as *mut u32, w) };
        }
    };
    for b in 0..n_full {
        put(va + long + PROG_AT + b * PROG_STRIDE, &jb[list_at..list_at + list_len], w_lo, w_hi, b);
    }
    let slim_at = PROG_AT + n_full * PROG_STRIDE;
    let mut slim_len = 0;
    if slim {
        let mut sl = [0u8; SLIM_STRIDE];
        slim_len = geom::slim(&jb[list_at..list_at + list_len], slim_level, &mut sl).ok_or(Fail::Inputs("the slim program is longer than a copy's room"))?;
        let (s_lo, s_hi) = job::image_words(&sl[..slim_len]).map_err(Fail::Job)?;
        for b in 0..n_slim {
            put(va + long + slim_at + b * SLIM_STRIDE, &sl[..slim_len], s_lo, s_hi, b);
        }
    }
    ferros_sys::dma_barrier();
    say!(g, "camera: job placed: A#{} address registers, A#{} high halves (A#{} left as captured), A#{} loads, A#{} descriptors; A#{n_full} full copies of A#{list_len} bytes (the image's words A#{w_lo} and A#{w_hi}), A#{n_slim} slim of A#{slim_len}; A#{regeom} program words of geometry rewritten ({:?}) for A#{} x A#{} at G#{:X} a line\n", m_.addr_regs, m_.highs, m_.highs_left, m_.loads, m_.ring, scope, fe.w, fe.h, fe.stride);
    // The CPU's setup writes before the engine starts, in the capture's order, their geometry the front end's; without them the engine applies the program and the frame writer stays idle (2026-10-20).
    let (mut n, mut cpu_geom) = (0, 0);
    let mut cpu = |a: usize, v: u32| {
        let nv = geom::value(a as u32, v, &from, &fe, scope);
        cpu_geom += (nv != v) as u32;
        wr(g, a, nv);
    };
    for (a, v) in prog.before.iter() {
        if inputs::cpu_write_allowed(a) {
            cpu(a, v);
            n += 1;
        }
    }
    // The shape the frame thread and the endpoint work to.
    BUF.store(buf, Ordering::Relaxed);
    ROWS.store(fe.h as u64, Ordering::Relaxed);
    COLS.store(fe.w as u64, Ordering::Relaxed);
    LINE.store(fe.stride as u64, Ordering::Relaxed);
    SLIM.store(slim as u64, Ordering::Relaxed);
    PD_ON.store((s.kind == K_CAPTURED || s.flags & F_PD != 0) as u64, Ordering::Relaxed);
    SLITS.store(if slit { ring as u64 } else { 0 }, Ordering::Relaxed);
    FLL.store(mode.map_or(0, |m| m.fll as u64), Ordering::Relaxed);
    LLP.store(mode.map_or(0, |m| m.llp as u64), Ordering::Relaxed);
    NOW_FORMAT.store(mode.map_or(10, |m| m.bits() as u64), Ordering::Relaxed);
    for x in [&LATE, &BUSY, &BUSY_MOST, &RING_END] {
        x.store(0, Ordering::Relaxed);
    }
    // The engine's places: the copies, the descriptors' flags (the job's own: its first descriptor's).
    let first = unsafe { core::ptr::read_volatile(va as *const [u32; 4]) };
    DESC_FLAGS.store(first[0] as u64, Ordering::Relaxed);
    PROG_PA.store(pa + (long + PROG_AT) as u64, Ordering::Relaxed);
    PROG_LEN.store(list_len as u64, Ordering::Relaxed);
    SLIM_PA.store(pa + (long + slim_at) as u64, Ordering::Relaxed);
    SLIM_LEN.store(slim_len as u64, Ordering::Relaxed);
    RING_VA.store((va + ring_off) as u64, Ordering::Relaxed);
    // The engine, exactly as a camera open starts it, its long ring at the engine's area: the count asked for read back, the next smaller tried, the job's own ring at the region's start last (`RING_JOB`).
    let pd = ferros_cam::PDMA;
    let want = if slit { RING_SLIT } else { RING };
    let mut ring_n = RING_JOB;
    let mut ring_pa = pa;
    for k in [want, RING] {
        wr(g, pd + ferros_cam::PDMA_COUNT, k as u32);
        if rd(g, pd + ferros_cam::PDMA_COUNT) as u64 == k {
            (ring_n, ring_pa) = (k, pa + long as u64);
            break;
        }
    }
    RING_N.store(ring_n, Ordering::Relaxed);
    DESC_VA.store((va + (ring_pa - pa) as usize) as u64, Ordering::Relaxed);
    // Every descriptor named at once: a slit ring's for good (descriptor e buffer e mod the ring), an image ring's as the frame thread will rewrite each before it is released (`ring_turn`), all naming buffer 0 meanwhile.
    for e in 0..ring_n {
        desc_for(e, if slit { (e % ring as u64) as usize } else { 0 });
    }
    // The image ring's state, its first buffer given to descriptor 0 (released with the stream below): every buffer patterned above, so its probe shows the frame that lands.
    for b in 0..job::RING_MOST {
        B_COUNT[b].store(0, Ordering::Relaxed);
    }
    for a in AGE.iter() {
        a.store(0, Ordering::Relaxed);
    }
    for x in [&UNSEEN, &CROWDED, &NONE_FREE] {
        x.store(0, Ordering::Relaxed);
    }
    SEQ.store(1, Ordering::Relaxed);
    B_SEQ[0].store(0, Ordering::Relaxed);
    LAST_R.store(0, Ordering::Relaxed);
    STATE.store(1 << QUEUED, Ordering::Release);
    RING_BUFS.store(if slit { 0 } else { ring as u64 }, Ordering::Release);
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
    // The camera's receiver last, as a camera open does it, its channels' sizes and types the front end's: enabling the link first left the front end latching its configuration and never seeing a frame start (2026-10-20).
    let mut rx_geom = 0;
    for &(a, v) in c.rx_start.iter() {
        let nv = geom::value(a as u32, v, &from, &fe, scope);
        rx_geom += (nv != v) as u32;
        wr(g, a, nv);
    }
    for (a, v) in prog.after.iter() {
        if inputs::cpu_write_allowed(a) {
            cpu(a, v);
            n += 1;
        }
    }
    let refused = REFUSED.load(Ordering::Relaxed);
    say!(g, "camera: engine started (ring G#{ring_pa:X}, A#{ring_n} descriptors{}), receiver on (A#{rx_geom} of its words' geometry rewritten), A#{n} CPU writes (A#{cpu_geom} rewritten), A#{refused} refused\n", if ring_n == want { "" } else { ": the count did not hold the long ring asked for" });
    // The lens to about a metre (the main's measured sharpest over a floor) where focus is offered; elsewhere it stays where it rests (the ultrawide's actuator never answered ferros, the front's range is not measured).
    let focus = if c.focus.1 != 0 { 0x3800 } else { 0 };
    let exposure_lines = mode.map_or(c.exposure_lines, |m| m.coarse as u64);
    let o = Open { va, l, ring, exposure_lines, gain: 1 << 10, focus, asked_at: 0, asked_tick: ferros_sys::now(), captured: fe == from, probed: false, view: false, given: 0, held: None, maps: 0, lat_sum: 0, lat_max: 0, overwritten: 0 };
    NOW_LINES.store(o.exposure_lines, Ordering::Relaxed);
    NOW_GAIN.store(o.gain, Ordering::Relaxed);
    NOW_FOCUS.store(o.focus, Ordering::Relaxed);
    if c.focus.1 != 0 {
        if let Err(e) = focus_init(&bus, c).and_then(|_| focus_set(&bus, c, o.focus)) {
            say!(g, "camera: the lens actuator did not answer (G#{e:X}); it stays where it rests\n");
        }
    }
    FRAMES.store(0, Ordering::Release);
    bus.write(c.addr, c.stream_on).map_err(Fail::Sensor)?;
    // The first descriptor released and the frame thread on (Linux's start writes +G#10 = G#10 right after the engine's start).
    wr(g, pd + ferros_cam::PDMA_INDEX, STEP);
    RUN.store(1, Ordering::Release);
    ferros_sys::wake(&RUN, 1);
    say!(g, "camera: streaming A#{} ms after the open\n", ferros_sys::us_since(t0) / 1000);
    // Mid-stream, when asked: the mode written once the first frame has landed (October's order; the sensor took one change a session and then needed a cold boot).
    if let (Some(m), true) = (mode, s.flags & F_MID != 0) {
        let until = ferros_sys::now() + ferros_sys::freq() * FRAME_LATE_US / 1_000_000;
        while FRAMES.load(Ordering::Acquire) == 0 && ferros_sys::now() < until {
            ferros_sys::wait(&FRAMES, 0, until);
        }
        for wr_ in m.writes(arr, c).iter() {
            bus.write(c.addr, wr_).map_err(Fail::Sensor)?;
        }
        say!(g, "camera: the mode written mid-stream after A#{} frames\n", FRAMES.load(Ordering::Relaxed));
    }
    Ok(o)
}

/// The stream stopped and the switch off: the sensor's stream off, a frame's time for its last frame to land (67.8 ms measured; 2^16 µs, as `cam pdma` waited 50 ms before stopping the engine), the frame thread idle, then the switch, whose off stops the engine, replays Linux's close, shuts the S2MPUs and powers the module down (kernel camera_power.rs).
fn close(g: u64) {
    // The view taken back first (SYS_DMA x1 = 5): its blocks the blank frame, the holder's token dead, before the stream stops. Nothing when none was given.
    ferros_sys::svc(ferros_sys::nr::DMA, cap(g, S_REGION), 5, 0, 0, 0);
    let c = cam();
    let _ = Bus::of(g, c).write(c.addr, c.stream_off);
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
    say!(g, "camera: {}: A#{} of its A#{} outputs written (bits G#{written:X})\n", cam().name, written.count_ones(), o.l.slots.iter().filter(|s| s.1 != 0).count());
}

// ---------------------------------------------------------------- frames

/// The largest readout the buffers below are sized for (the main camera's 4080 x 3072, G#1400 a line; ferros_cam cams `W_MOST`), and the open camera's own: a RAW10 line is four pixels in five bytes, on the front end's stride (the main's 4080 on a 256-byte boundary, the ultrawide's 4000 on 64, the front's 3840 exactly).
const W: usize = cams::W_MOST;
const H: usize = cams::H_MOST;
const STRIDE: usize = cams::STRIDE_MOST;
/// A ring buffer's probe: its last line's first 64 bytes, written last of the frame (October's extent: the main's image body to +G#EFFF00, the ultrawide's to +G#E77100).
fn probe_off() -> usize {
    ((ROWS.load(Ordering::Relaxed) - 1) * LINE.load(Ordering::Relaxed)) as usize
}
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
    let c = cam();
    let (w, stride) = (c.width, c.stride);
    let (bw, bh) = c.binned(bin);
    let s = unsafe { &mut (*(&raw mut SCRATCH))[k] };
    let out = (&raw mut OUT) as *mut u8;
    let rows = 2 * bin;
    let words = STRIDE / 8;
    for pair in (k..bh / 2).step_by(BINNERS) {
        // The 2 x bin lines this row pair reads, copied out of the region whole, then unpacked (ferros_cam raw, host-tested against std/camreplay's binning of a recorded frame).
        for r in 0..rows {
            // 64 bytes a load (four NEON registers), not 8: each load of non-cacheable memory is its own transaction, and 8-byte ones took 121 ms a frame (the Pro, 2026-11-14). Every camera's stride is a multiple of 64 (ferros_cam cams' test).
            let t = ferros_sys::now();
            let line = (src + (pair * rows + r) * stride) as *const u8;
            let dst = unsafe { s.raw.as_mut_ptr().add(r * words) } as *mut u8;
            for i in 0..stride / 64 {
                unsafe {
                    let v = core::arch::aarch64::vld1q_u8_x4(line.add(i * 64));
                    core::arch::aarch64::vst1q_u8_x4(dst.add(i * 64), v);
                }
            }
            COPY_TICKS.fetch_add(ferros_sys::now() - t, Ordering::Relaxed);
            let bytes = unsafe { core::slice::from_raw_parts(s.raw.as_ptr().add(r * words) as *const u8, stride) };
            ferros_cam::raw::unpack(bytes, &mut s.pix[r * w..(r + 1) * w]);
        }
        for phase in 0..2 {
            let oy = pair * 2 + phase;
            // SAFETY: row oy of the output is this binner's alone (its row pair), inside `OUT` (bw x bh samples of 2 bytes after the header, bw at most W / 2 and bh at most H / 2: cams `binned`, at bin 2 or more).
            let row = unsafe { core::slice::from_raw_parts_mut(out.add(HEADER + oy * bw * 2), bw * 2) };
            ferros_cam::raw::bin_row(&s.pix[..rows * w], w, bin, phase, &mut s.acc, row);
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
    o.va + o.l.slots[job::OLD_SLOT_IMAGE].0 as usize + b * BUF.load(Ordering::Relaxed) as usize
}

/// A frame's header (docs/CAMERA.md "The endpoint"), for buffer `b`'s frame at `bin`: count, width and height after binning, bin, cfa, black and white, exposure (ns), gain, focus, when it ended (Eagle ns); for the view (bin 1) also the buffer at +40, the format at +41 (1: MIPI RAW10, four samples in five bytes), the line's bytes at +44 and the buffer's place in the view at +48.
fn header(h: &mut [u8], b: usize, bin: usize) {
    let c = cam();
    // The view's frame (bin 1) is the open mode's shape; FRAME by copy bins only the captured one (`Open::captured`).
    let (bw, bh) = if bin == 1 { (COLS.load(Ordering::Relaxed) as usize, ROWS.load(Ordering::Relaxed) as usize) } else { c.binned(bin) };
    h.fill(0);
    h[0..4].copy_from_slice(&(B_COUNT[b].load(Ordering::Relaxed) as u32).to_le_bytes());
    h[4..6].copy_from_slice(&(bw as u16).to_le_bytes());
    h[6..8].copy_from_slice(&(bh as u16).to_le_bytes());
    h[8] = bin as u8;
    h[9] = c.cfa;
    h[10..12].copy_from_slice(&c.black.to_le_bytes());
    h[12..14].copy_from_slice(&c.white.to_le_bytes());
    h[16..24].copy_from_slice(&(B_LINES[b].load(Ordering::Relaxed) * c.line_ps / 1000).to_le_bytes());
    h[24..28].copy_from_slice(&(B_GAIN[b].load(Ordering::Relaxed) as u32).to_le_bytes());
    h[28..32].copy_from_slice(&(B_FOCUS[b].load(Ordering::Relaxed) as u32).to_le_bytes());
    h[32..40].copy_from_slice(&B_AT[b].load(Ordering::Relaxed).to_le_bytes());
    if bin == 1 {
        h[40] = b as u8;
        h[41] = 1;
        h[44..48].copy_from_slice(&(LINE.load(Ordering::Relaxed) as u32).to_le_bytes());
        h[48..56].copy_from_slice(&(b as u64 * BUF.load(Ordering::Relaxed)).to_le_bytes());
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

/// Buffer `b`'s frame into `OUT`, binned `bin` x `bin` per colour: output sample (ox, oy) is the mean of the bin x bin same-colour samples of its 2 bin x 2 bin block, so the mosaic keeps its phase (the camera's cfa), after its header (`header`). Its length.
fn bin_frame(o: &Open, bin: usize, b: usize) -> usize {
    let (bw, bh) = cam().binned(bin);
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

/// An OPEN's reply with the view: `[0]`, the view's token (32 bytes, at 1), the ring's buffers (33), the format (34: 1 MIPI RAW10, 2 RAW12), width and height (u16 at 36, 38), a line's bytes (u32 at 40), a buffer's bytes in the view (u32 at 44).
/// An OPEN that carried a mode block gets `VIEW_REPLY_MODE` bytes: also the ring's buffers as a u32 (at 48: a slit ring's hundreds), 1 for a slit ring (52), the record's place in a slit buffer (u32 at 56: its rows' bytes); an OPEN without one the 48 bytes Lumis reads.
const VIEW_REPLY: usize = 48;
const VIEW_REPLY_MODE: usize = 64;

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
    // The camera's CSI link as it stands (its base: the link the receiver's start word G#205E01 went to): its interrupt words (+G#14 errors, +G#1C line end, lost frame and size mismatch, +G#24 frame starts, +G#2C frame ends per channel) and its per-channel counters (+G#634 to +G#6C4, October's 21 words), so a stream that stopped says whether frames still reached the link and what it made of them (docs/oracle/CAMERA.md: a mismatched slit added size errors there).
    let Some(&(a, _)) = cam().rx_start.iter().find(|w| w.1 == 0x205E01) else { return };
    let link = a & !0xFFFF;
    say!(g, "camera: link G#{link:X}: +14 G#{:X} +1C G#{:X} +24 G#{:X} +2C G#{:X}; image path G#{:X}, aggregate G#{:X}\n", rd(g, link + 0x14), rd(g, link + 0x1C), rd(g, link + 0x24), rd(g, link + 0x2C), rd(g, ferros_cam::IMG_STATUS), rd(g, ferros_cam::CMN_STATUS));
    let mut l = Line { b: [0; 256], n: 0 };
    let _ = write!(l, "camera: link counters from +634:");
    for k in 0..(0x6C8 - 0x634) / 4 {
        let _ = write!(l, " {:X}", rd(g, link + 0x634 + 4 * k));
    }
    let _ = write!(l, "\n");
    console(g, &l.b[..l.n]);
}

/// How long the stream may run with nobody asking before the holder is taken for gone: 2^22 us (it was 64 frames, 4.4 s at 14.5 a second; a slit runs 64 frames in 53 ms).
const UNASKED_US: u64 = 1 << 22;
/// The word the idle server waits on: never rung, so only a call or its deadline end the wait.
static QUIET: AtomicU64 = AtomicU64::new(0);
/// How long a FRAME waits for its frame before the stream is taken for stopped: about thirty frame periods, past the sensor's start after stream on.
const FRAME_LATE_US: u64 = 1 << 21;

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(g: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_BYTES) };
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
                    // Open: wait for a call until the holder's deadline (a word nothing rings, so a slit's thousand frames a second do not wake this thread). A holder gone quiet past `UNASKED_US` is gone.
                    let until = o.asked_tick + ferros_sys::freq() * UNASKED_US / 1_000_000;
                    if ferros_sys::now() >= until {
                        say!(g, "camera: A#{} ms with nobody asking; its holder is gone\n", UNASKED_US / 1000);
                        close(g);
                        ferros_sys::exit(0);
                    }
                    ferros_sys::wait_world_or_call(&QUIET, 0, until);
                    continue;
                }
            }
        }
        if got == ERR || got == 0 {
            continue;
        }
        if let Some(o) = open_cam.as_mut() {
            o.asked_tick = ferros_sys::now();
        }
        match msg[0] {
            // LIST: the cameras that can stream whose inputs and IDT are in the vault (`list`), so a camera shows once its init, program, job and calibration are stored on this device.
            0 => {
                reply(&token, &list(g));
            }
            1 => {
                let c = if got >= 2 { cams::by_id(msg[1]).filter(|c| c.streams) } else { None };
                let r = if c.is_none() {
                    1
                } else if open_cam.is_some() {
                    2
                } else {
                    let c = c.unwrap();
                    match open(g, c, &Spec::of(&msg, got as usize)) {
                        Ok(o) => {
                            open_cam = Some(o);
                            0
                        }
                        Err(f) => {
                            match f {
                                Fail::Region => say!(g, "camera: no region (the console's `camera region` line says why)\n"),
                                Fail::Switch => say!(g, "camera: the switch refused on (the flash log says why)\n"),
                                Fail::Sensor(e) => say!(g, "camera: the sensor did not answer as {} (G#{e:X})\n", c.key),
                                Fail::Inputs(why) => say!(g, "camera: {why}\n"),
                                Fail::Init(k, e) => say!(g, "camera: the init failed at record A#{k} (G#{e:X})\n"),
                                Fail::Job(e) => say!(g, "camera: the job would not move into the region ({e:?})\n"),
                                Fail::Mode(e) => say!(g, "camera: no such mode ({e:?})\n"),
                            }
                            if !matches!(f, Fail::Region | Fail::Switch) {
                                let _ = ferros_sys::power(cap(g, S_POWER), false);
                            }
                            3
                        }
                    }
                };
                // The view, when the open asked for it (`[1][id][1]`): the kernel gives the caller a read-only view of the image ring and a token for it (SYS_DMA x1 = 4), which the reply carries with the ring's shape; a view the kernel refuses (the console says why) leaves the open as a plain one, `[0]` alone, and frames by copy still work.
                let mut out = [0u8; VIEW_REPLY_MODE];
                out[0] = r;
                let mut len = 1;
                if let (0, Some(o)) = (r, open_cam.as_mut()) {
                    if got >= 3 && msg[2] == 1 && o.ring < VIEW_LEAST {
                        say!(g, "camera: no view: the pool gave a ring of one buffer; frames by copy only\n");
                    } else if got >= 3 && msg[2] == 1 {
                        // The image slot whole: a ring of whole frames, or a slit ring rounded up to whole frames of the pool (ferros_cam job `with_image`).
                        let first = o.l.slots[job::OLD_SLOT_IMAGE].0 / job::FRAME;
                        let n = o.l.slots[job::OLD_SLOT_IMAGE].1 / job::FRAME;
                        let mut t = [0u8; 32];
                        let base = ferros_sys::svc(ferros_sys::nr::DMA, cap(g, S_REGION), 4, ferros_sys::off(token.as_ptr()), first | n << 32, ferros_sys::off(t.as_mut_ptr()));
                        if base != ERR {
                            o.view = true;
                            out[1..33].copy_from_slice(&t);
                            out[33] = o.ring as u8;
                            out[34] = if NOW_FORMAT.load(Ordering::Relaxed) == 12 { 2 } else { 1 };
                            out[36..38].copy_from_slice(&(COLS.load(Ordering::Relaxed) as u16).to_le_bytes());
                            out[38..40].copy_from_slice(&(ROWS.load(Ordering::Relaxed) as u16).to_le_bytes());
                            out[40..44].copy_from_slice(&(LINE.load(Ordering::Relaxed) as u32).to_le_bytes());
                            out[44..48].copy_from_slice(&(BUF.load(Ordering::Relaxed) as u32).to_le_bytes());
                            out[48..52].copy_from_slice(&(o.ring as u32).to_le_bytes());
                            out[52] = (SLITS.load(Ordering::Relaxed) != 0) as u8;
                            out[56..60].copy_from_slice(&((ROWS.load(Ordering::Relaxed) * LINE.load(Ordering::Relaxed)) as u32).to_le_bytes());
                            len = if got > 3 { VIEW_REPLY_MODE } else { VIEW_REPLY };
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
                // Only the captured geometry bins by copy: a 1:1 frame, a slit or RAW12 is the view's (a slit ring has no holds either).
                if !matches!(bin, 2 | 4 | 8) || !o.captured || SLITS.load(Ordering::Relaxed) != 0 {
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
                let c = cam();
                let bus = Bus::of(g, c);
                let r = match what {
                    0 => {
                        // Every camera that streams has its line time (ferros_cam cams' test), the front's a guess.
                        let lines = value * 1000 / c.line_ps;
                        // A mode's own frame bounds its integration (ferros_cam ccs `MARGIN`); the init's mode the camera's table.
                        let most = match FLL.load(Ordering::Relaxed) {
                            0 => c.exposure_max_lines,
                            f => f - ccs::MARGIN as u64,
                        };
                        if (1..=most).contains(&lines) && exposure_set(&bus, c, lines).is_ok() {
                            o.exposure_lines = lines;
                            NOW_LINES.store(lines, Ordering::Relaxed);
                            0
                        } else {
                            1
                        }
                    }
                    1 if value == 1 << 10 => 0,
                    2 if c.focus.1 != 0 && (c.focus.0..=c.focus.1).contains(&value) && focus_set(&bus, c, value).is_ok() => {
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
                // A slit ring has no holds: its frames are read by SLITS.
                if !o.view || SLITS.load(Ordering::Relaxed) != 0 {
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
            // PROBE `[7][id]` (development): `probe_id`'s reply; `[4]` no such camera, `[5]` a camera is open (its module holds the rails).
            7 => {
                let c = if got >= 2 { cams::by_id(msg[1]) } else { None };
                let mut out = [0u8; 20];
                match (open_cam.is_some(), c) {
                    (true, _) => out[0] = 5,
                    (false, None) => out[0] = 4,
                    (false, Some(c)) => out = probe_id(g, c),
                }
                reply(&token, &out);
            }
            // STATS `[8]`: the stream's counters (`stats`).
            8 => {
                reply(&token, &stats());
            }
            // SLITS `[9][after: u32]`: a slit ring's frames past `after` (`slits`).
            9 => {
                let Some(_) = open_cam.as_ref() else {
                    reply(&token, &[]);
                    continue;
                };
                if SLITS.load(Ordering::Relaxed) == 0 {
                    reply(&token, &[]);
                    continue;
                }
                let after = if got >= 5 { u32::from_le_bytes(msg[1..5].try_into().unwrap()) as u64 } else { 0 };
                let until = ferros_sys::now() + ferros_sys::freq() * FRAME_LATE_US / 1_000_000;
                loop {
                    let n = FRAMES.load(Ordering::Acquire);
                    if n > after || ferros_sys::now() >= until {
                        break;
                    }
                    ferros_sys::wait(&FRAMES, n, until);
                }
                if FRAMES.load(Ordering::Acquire) <= after {
                    late(g);
                    close(g);
                    reply(&token, &[]);
                    ferros_sys::exit(0);
                }
                reply(&token, &slits());
            }
            // RATE `[10][windows]`: the sensor's own frame count against the front end's (`rate`).
            10 => {
                if open_cam.is_none() {
                    reply(&token, &[]);
                    continue;
                }
                let w = if got >= 2 && msg[1] != 0 { msg[1] } else { 1 << 4 };
                reply(&token, &rate(g, w));
            }
            // CAPS `[11]`: the sensor's formats, integration limits and mode registers read back (`caps`).
            11 => {
                if open_cam.is_none() {
                    reply(&token, &[]);
                    continue;
                }
                reply(&token, &caps(g));
            }
            _ => {
                reply(&token, &[]);
            }
        }
    }
}

// ---------------------------------------------------------------- the bench's ops

/// STATS' reply: little-endian u64s, in order: frames the front end wrote (DMA_DONE), answers, answers on the line, the frame thread's ticks answering and the most in one answer, slit frames not where the ring put them, ring frames whose buffer was not found, the long ring's descriptors and whether its last was given, the counter now and its frequency, register accesses refused, the image status bits seen, the frame's rows, width, line bytes and a buffer's bytes, the slit ring's buffers (0 none), the mode's frame and line length, the integration (lines), bits a sample, whether the slim program runs, whether the PD path is on, whether the frame thread answers on its tick, when the open began (the counter).
const STATS_WORDS: usize = 26;
fn stats() -> [u8; 8 * STATS_WORDS] {
    let w: [u64; STATS_WORDS] = [
        FRAMES.load(Ordering::Acquire),
        ANSWERED.load(Ordering::Relaxed),
        ON_LINE.load(Ordering::Relaxed),
        BUSY.load(Ordering::Relaxed),
        BUSY_MOST.load(Ordering::Relaxed),
        LATE.load(Ordering::Relaxed),
        UNSEEN.load(Ordering::Relaxed),
        RING_N.load(Ordering::Relaxed),
        RING_END.load(Ordering::Relaxed),
        ferros_sys::now(),
        ferros_sys::freq(),
        REFUSED.load(Ordering::Relaxed),
        IMG_SEEN.load(Ordering::Relaxed),
        ROWS.load(Ordering::Relaxed),
        COLS.load(Ordering::Relaxed),
        LINE.load(Ordering::Relaxed),
        BUF.load(Ordering::Relaxed),
        SLITS.load(Ordering::Relaxed),
        FLL.load(Ordering::Relaxed),
        LLP.load(Ordering::Relaxed),
        NOW_LINES.load(Ordering::Relaxed),
        NOW_FORMAT.load(Ordering::Relaxed),
        SLIM.load(Ordering::Relaxed),
        PD_ON.load(Ordering::Relaxed),
        TICKING.load(Ordering::Relaxed),
        OPEN_AT.load(Ordering::Relaxed),
    ];
    let mut out = [0u8; 8 * STATS_WORDS];
    for (k, v) in w.iter().enumerate() {
        out[8 * k..8 * k + 8].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// SLITS' reply: little-endian u64s: the newest frame's count, the ring's buffers, a buffer's bytes, the rows, a line's bytes, the width, frames not where the ring put them, whether the long ring's last descriptor was given. Frame n is in buffer (n - 1) mod the ring, at n's buffer times its bytes in the view; its record (`slit_turn`: count, counter, Eagle time, probe moved) right after the rows. The oldest buffer is the next the engine writes (its probe patterned as its descriptor is released), so a reader keeps to the newest ring - 1.
fn slits() -> [u8; 64] {
    let w: [u64; 8] = [
        FRAMES.load(Ordering::Acquire),
        SLITS.load(Ordering::Relaxed),
        BUF.load(Ordering::Relaxed),
        ROWS.load(Ordering::Relaxed),
        LINE.load(Ordering::Relaxed),
        COLS.load(Ordering::Relaxed),
        LATE.load(Ordering::Relaxed),
        RING_END.load(Ordering::Relaxed),
    ];
    let mut out = [0u8; 64];
    for (k, v) in w.iter().enumerate() {
        out[8 * k..8 * k + 8].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// RATE: the sensor's frame counter (CCS G#0005, eight bits) read every 2^16 us for `windows` windows, each window's step taken mod 256, beside the front end's frames over the same span. The reads are a deadline, not a poll: an eight-bit counter wraps after 256 frames, so it must be read more often than 256 frame periods (at 2^16 us, any rate under 3906 frames a second reads true). Reply: little-endian u64s, the sensor's frames, the front end's, the span in counter ticks, the counter's frequency, the counter reads that failed.
fn rate(g: u64, windows: u8) -> [u8; 40] {
    let c = cam();
    let bus = Bus::of(g, c);
    let read = || -> Option<u8> {
        let mut b = [0u8; 1];
        bus.write_read(c.addr, &[(ccs::FRAME_COUNT >> 8) as u8, ccs::FRAME_COUNT as u8], &mut b).ok().map(|_| b[0])
    };
    let step = ferros_sys::freq() * (1 << 16) / 1_000_000;
    let t0 = ferros_sys::now();
    let f0 = FRAMES.load(Ordering::Acquire);
    let (mut last, mut sensor, mut fails) = (read(), 0u64, 0u64);
    for k in 1..=windows as u64 {
        ferros_sys::sleep_until(t0 + k * step);
        let v = read();
        match (last, v) {
            (Some(a), Some(b)) => sensor += b.wrapping_sub(a) as u64,
            _ => fails += 1,
        }
        if v.is_some() {
            last = v;
        }
    }
    let w = [sensor, FRAMES.load(Ordering::Acquire) - f0, ferros_sys::now() - t0, ferros_sys::freq(), fails];
    let mut out = [0u8; 40];
    for (k, v) in w.iter().enumerate() {
        out[8 * k..8 * k + 8].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// CAPS: the sensor's registers read back, each as it answers (zeros where the read failed), at fixed places: +0 its data format model type and subtype (G#00C0, two bytes), +2 its sixteen data format descriptors (G#00C2, 32 bytes: (uncompressed bits << 8) | compressed bits each), +34 the integration's least and its margin (G#1004, four bytes), +38 the frame and line length (G#0340, four), +42 the window and output size (G#0344, twelve), +54 the increments (G#0380, eight), +62 the binning (G#0900, three), +65 the data format (G#0112, two), +67 the integration (G#0202, two), +69 mode select (G#0100, one), +70 the frame count (G#0005, one), +71 two bytes, a bit per read that failed (bit 0 the first).
fn caps(g: u64) -> [u8; 73] {
    let c = cam();
    let bus = Bus::of(g, c);
    let mut out = [0u8; 73];
    let mut failed = 0u16;
    let reads: [(u16, usize, usize); 11] = [(0x00C0, 0, 2), (0x00C2, 2, 32), (0x1004, 34, 4), (0x0340, 38, 4), (0x0344, 42, 12), (0x0380, 54, 8), (0x0900, 62, 3), (0x0112, 65, 2), (0x0202, 67, 2), (0x0100, 69, 1), (0x0005, 70, 1)];
    for (k, &(a, at, n)) in reads.iter().enumerate() {
        if bus.write_read(c.addr, &[(a >> 8) as u8, a as u8], &mut out[at..at + n]).is_err() {
            failed |= 1 << k;
        }
    }
    out[71..73].copy_from_slice(&failed.to_le_bytes());
    out
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
