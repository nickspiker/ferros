//! The camera driver (docs/CAMERA.md "The driver"; owner, 2026-11-14: "Can we get Lumis wired up?"): the Pro's cameras served on the `camera` endpoint, started by the kernel when a call on it finds no server (process.rs `ON_DEMAND`), ended by itself once its holder closes.
//! The main camera since 2026-11-14; the ultrawide, and the front camera as far as it answers, since 2026-11-15 (owner: "would be nice to get the other cams online"; docs/CAMERA.md "The other cameras"). Each camera's facts are ferros_cam cams, one camera open at a time.
//!
//! The kernel keeps what a process may not do behind the `camera` power switch (kernel camera_power.rs): the camera buses' pins and clocks, the module's rails behind the overvoltage guard (which module: the switch's argument, ferros_sys `power_arg`), the front end's power domain and clocks, the D-PHY's isolation, the SYSMMUs in bypass and the S2MPUs fenced to this driver's frames. Here, thru its grants: the open camera's sensor and lens actuator on its bus, the front end and its PDMA engine, the CSI links and PHY, and the front end's line 0.
//!
//! An open, in the order a camera open does it (the kernel's `diag cam pdma`, which this replaces): the region taken (a run of frames from the pool, zeroed, fenced), the switch on for that camera's module, the sensor's identity read, its init replayed from the vault (`cam.<key>.init`; the main's also `cam.frame`; a 1:1 mode's `cam.<key>.init.full`), the PDMA job from the vault (`cam.<key>.pdma`; its IDT `cam.<key>.idt` beside them, for the list) moved into the region (ferros_cam job) and placed there, the CPU's setup writes before the engine (`cam.<key>.prog`; the main's `cam.ispfe.prog`), the engine started, the camera's receiver last, the CPU's writes after it, the lens set, the stream on. The keys: boitata (main), oksoko (ultrawide), dokkaebi (front), kraken (tele).
//! Then the front end is answered on its interrupt: every acknowledgement in the order Linux makes them (the camera's, ferros_cam cams `ack`), loch0's paths enabled after the first SHADOW_UPDATE (G#81 to G#1C32_00A0), the engine's descriptor index advanced once a frame. Left unanswered past about a frame period the front end stops for good (docs/oracle/CAMERA.md "Focus, exposure"), which is why this is its own thread on an interrupt and not the kernel's spin.
//!
//! Spawn by hand (the kernel also starts it itself on a call to `camera`):
//! ferros-bridge spawn camera.bin --name camera --serve camera --hw hsi2c1 --hw ispfe --hw csis --hw ispfe-sysreg --store cam --power camera --dma camera --irq ispfe0 --hw hsi2c3 --hw hsi2c2 --hw hsi2c4
//! Grant slots, in that order: 0 console, 1 serve camera, 2 hsi2c_1, 3 the front end, 4 the CSI links and PHY, 5 SYSREG_ISPFE, 6 the vault scope `cam.`, 7 the switch, 8 the region, 9 the line, 10 hsi2c_3 (the ultrawide), 11 hsi2c_2 (the front), 12 hsi2c_4 (the tele).
//!
//! The `camera` endpoint (docs/CAMERA.md "The endpoint"), first byte the op:
//! - `0` LIST: the list, a VSF document built here (ferros_cam list): a section per camera that can stream whose inputs and IDT are in the vault, each with its own Absolute IDT and sensitivity read from `cam.<key>.idt`; a camera whose 1:1 init is in the vault (`cam.<key>.init.full`) has a second section, the same `id` and `name`, its `mode` 2 and its own size, `cfa_period`, `frame_ns` and exposure range (ferros_cam list `Readout`).
//! - `1` OPEN + id (+ `1` for the view): `[0]` open, `[1]` no such camera (or one that cannot stream), `[2]` another holds it, `[3]` it would not start (the console says why). With the view asked for and given, `[0]` then its token and the ring's shape (`VIEW_REPLY`).
//! - `2` FRAME + bin: waits for the next frame; a 64-byte header, then the mosaic, `bin` x `bin` same-colour samples averaged, 16 bits a sample. bin 2, 4 or 8 (a whole readout is past the largest reply).
//! - `3` SET + what + value (8 bytes): what 0 exposure (ns), 1 analogue gain (1024 unity), 2 focus, 3 (a device test) the vendor shift every later exposure takes (0 the least that fits; a forced shift is not held to the sensor's frame counter, the plain exposure is: refused past the list's `exposure_max`, the longest the counter holds). `[0]` taken, `[1]` outside the range.
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
//! After the whole block (its `[cols: u16]` too) one byte of view flags (`VIEW_FLAGS`): bit 0 the view write-back (docs/CAMERA.md "A cacheable view").
//! The mode is written inside the sensor's hold after its init and before stream on, the front end's geometry rewritten to match (ferros_cam ccs, geom), its buffers sized to the mode's frame.
//! A call that waits for a frame (FRAME, MAP, SNAP, SLITS) is held, its reply token kept, and every other call is answered meanwhile (LIST, SET, CLOSE, STATS): a frame 14 years long blocks nothing (docs/CAMERA.md "Serving while a frame waits"). A SET whose frame would end long before the frame in flight cuts that frame (`exposure_cut`).
//! A holder that ends without closing: the kernel marks the driver's doorbell the moment its process ends (the watch made at its OPEN, kernel SYS_CALLER x1) and the stream closes then; on a kernel without the watch, its held call's reply failing, another's OPEN finding that call's caller gone, or 2^22 us with nobody asking. The kernel takes the view back when either ends.
#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// An open's step, said on the console and kept in the flash log before it is taken (ferros_sys `console_flash`): an open that hangs the SoC then names its last step after the reset, when the RAM is gone (2026-11-16: the Pro reset in an open after its switch on, nothing of the later steps kept).
macro_rules! step {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line { b: [0; 256], n: 0 };
        let _ = write!(l, $($t)*);
        ferros_sys::console_flash($g, &l.b[..l.n]);
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
        if !LIST_SAID.load(Ordering::Relaxed) {
            let t = ferros_cam::list::Readout::binned(c).timing;
            say!(g, "camera: LIST {}: mode A#0, exposure_max A#{} ns\n", c.name, t.most_ns());
        }
        match ferros_cam::list::section(c, idt) {
            Ok(s) => secs.push(s),
            Err(e) => {
                say!(g, "camera: cam.{}.idt does not read ({e:?}); {} left out of the list\n", c.key, c.name);
                continue;
            }
        }
        // Its 1:1 readout, a second section with the same `id` and `name`, when its 1:1 init is in the vault (`cam.<key>.init.full`; docs/CAMERA.md "1:1 on the main"): its own size, mosaic period, frame period and exposure range, read off that init; opened by the mode block's kind 2.
        if let Some(full) = full_init(g, c) {
            if let (false, Ok(r)) = (LIST_SAID.load(Ordering::Relaxed), ferros_cam::list::Readout::full(c, full)) {
                say!(g, "camera: LIST {}: mode A#{}, exposure_max A#{} ns\n", c.name, r.kind, r.timing.most_ns());
            }
            match ferros_cam::list::Readout::full(c, full).and_then(|r| ferros_cam::list::section_of(c, idt, &r)) {
                Ok(s) => secs.push(s),
                Err(e) => say!(g, "camera: cam.{}.init.full gives no 1:1 readout ({e:?}); {}'s 1:1 left out of the list\n", c.key, c.name),
            }
        }
    }
    LIST_SAID.store(true, Ordering::Relaxed);
    ferros_cam::list::document(secs).unwrap_or_default()
}

/// Whether this run's first LIST said each readout's longest exposure (once a run: a holder lists at each start).
static LIST_SAID: AtomicBool = AtomicBool::new(false);

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
/// Each acknowledged word's bits since the open and in the last answer (`ack_say`), by its place in the camera's list.
const ACK_MOST: usize = 1 << 5;
static ACK_OR: [AtomicU64; ACK_MOST] = [const { AtomicU64::new(0) }; ACK_MOST];
static ACK_LAST: [AtomicU64; ACK_MOST] = [const { AtomicU64::new(0) }; ACK_MOST];
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
/// The engine's ring: its count register and its index are both in bytes of the ring, a descriptor is `STEP` (16) bytes, and the engine's own read place wraps at the count (docs/CAMERA.md "The ring drained"): so the ring holds count / 16 descriptors, and the index goes round it, `(index + 16) mod count`, as Android's HAL writes it (count G#3E8, the index G#3E0 then G#8: the oracle capture uw-pdma-raw, 2026-10-22).
/// Read as descriptors until 2026-11-17, the index never wrapped: the engine went on past it unreleased, reading descriptor slots ferros had stopped writing, and Lumis's held buffers were written over from the front end's 1024th frame on (count 2^14: 1024 slots; the Pro, 2026-11-17, the preview-split run: none over in 938 frames, 21 by 1091, 386 by 2194). And an index written as 0 while the engine's place was 0 (2026-11-14, the job's count 1024: 64 slots) stopped it for good after the 1025th frame, both the same model.
/// `RING` for whole frames (1024 descriptors), `RING_SLIT` for slits (4096); `RING_JOB` (64), the job's own ring, if the count register holds neither.
const RING: u64 = 1 << 14;
const RING_JOB: u64 = 1 << 10;
const STEP: u32 = 0x10;
/// The count the engine was given (bytes: `RING`, `RING_SLIT` or `RING_JOB`), its descriptors a sixteenth of it.
static RING_N: AtomicU64 = AtomicU64::new(RING_JOB);
/// A device test of the wrap (the hand spawn's fourth input byte, log2 of the count to ask instead; 0 none): a count of 2^8 goes round every 16 frames.
static RING_TEST: AtomicU64 = AtomicU64::new(0);

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
/// The open mode's mosaic period in pixels (2 Bayer; a 1:1 mode's quad array, ferros_cam ccs `Array::period`), for the frame header's +14.
static CFA_PERIOD: AtomicU64 = AtomicU64::new(2);
static ROWS: AtomicU64 = AtomicU64::new(cams::H_MOST as u64);
static COLS: AtomicU64 = AtomicU64::new(cams::W_MOST as u64);
static LINE: AtomicU64 = AtomicU64::new(cams::STRIDE_MOST as u64);
static SLIM: AtomicU64 = AtomicU64::new(0);
static PD_ON: AtomicU64 = AtomicU64::new(1);
/// Where a ring buffer holds its frame's phase-detect rows (bytes past the buffer's start; 0 none: a slit, or the PD path off) and how many (the main's 768, at the image's line), so MAP's header hands both (docs/CAMERA.md "Phase detect as data").
static PD_OFF: AtomicU64 = AtomicU64::new(0);
static PD_ROWS: AtomicU64 = AtomicU64::new(0);
static SLITS: AtomicU64 = AtomicU64::new(0);
/// The sensor's frame length and line length now (the mode's, or as the last exposure moved them), for STATS.
static FLL: AtomicU64 = AtomicU64::new(0);
static LLP: AtomicU64 = AtomicU64::new(0);
/// The sensor's frame period now and before the last change of it (ns: a 14-year frame at shift 31 passes 64 bits of ps), and the frames counted at that change: a frame begun under the old timing may still be on its way for two frames (the sensor takes a hold at a frame's start, and a rolling shutter's next frame integrates during this one), so every deadline that waits for a frame reckons with both until then (`period_ns`).
static PERIOD_NS: AtomicU64 = AtomicU64::new(0);
static PERIOD_PREV_NS: AtomicU64 = AtomicU64::new(0);
static PERIOD_AT: AtomicU64 = AtomicU64::new(0);
/// The readout as memory sees it (owner, 2026-11-17: "measure the readout skew directly"), with `MEASURE` set: from the frame's first row landing in its buffer to its last row landing (counter ticks), the last, least and most, and how many, since the timing last changed; and frames whose rows were not both seen (missed). The front end cannot say it: its line fires once a frame at DMA_DONE, SOF and EOF already latched together (G#13), and once more for SHADOW_UPDATE (G#8) about 150 us later (the Pro, b933), so the rows are watched instead (`row_timing`).
static READ: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
fn readout_reset() {
    for (k, r) in READ.iter().enumerate() {
        r.store(if k == 1 { u64::MAX } else { 0 }, Ordering::Relaxed);
    }
}
/// The readout measured since the last reset, said.
fn readout_say(g: u64, what: &str) {
    let us = |t: u64| t * 1_000_000 / ferros_sys::freq();
    let n = READ[3].load(Ordering::Relaxed);
    if n == 0 {
        say!(g, "camera: readout {what}: no frame's first and last rows both seen (A#{} missed)\n", READ[4].load(Ordering::Relaxed));
        return;
    }
    say!(g, "camera: readout {what} (the first row to the last landing in memory): last A#{} us, least A#{} us, most A#{} us over A#{n} frames; A#{} missed\n", us(READ[0].load(Ordering::Relaxed)), us(READ[1].load(Ordering::Relaxed)), us(READ[2].load(Ordering::Relaxed)), READ[4].load(Ordering::Relaxed));
}
/// The most shift the vendor's long exposure may take, for a device test of shifts past the most measured (the hand spawn's second input byte; 0: the table's).
static LTE_MOST: AtomicU64 = AtomicU64::new(0);
/// The readout test (`row_timing`), the hand spawn's third input byte 1: a device test only, as it keeps a core spinning thru the last part of each frame.
static MEASURE: AtomicU64 = AtomicU64::new(0);
/// A device test, the hand spawn's fifth byte: a side channel off at the sensor in any mode (bit 0 phase detect, bit 1 embedded data).
static SIDE_TEST: AtomicU64 = AtomicU64::new(0);
/// The hand spawn's eighth byte (`Spec::shape`): 0 the defaults (a windowed mode on the main detached and slim); with G#80 an A/B override, its low four bits the processing blocks' switches exactly (ferros_cam geom `DETACH`: bit 0 FC ctx3, bit 1 the context at G#1C33_B000, bit 2 the crossbar, bit 3 the block at G#1C34) and the program the open's flags ask.
static DETACH: AtomicU64 = AtomicU64::new(0);
/// Descriptors released ahead of the engine (the hand spawn's sixth byte, a device test; 0 or 1: one, as Linux's start does): the stream on releases this many, each DMA_DONE one more, so a late answer has this many frames of slack instead of the frame's blanking (camera-full-rate, 2026-11-18: every death so far was one answer later than the blanking, the kernel lock held elsewhere).
static LEAD: AtomicU64 = AtomicU64::new(1);
/// A late answer made on purpose (the hand spawn's seventh byte k, a device test; 0 none): every 2^8th frame's DMA_DONE answered 2^k us late, as a kernel lock held elsewhere makes it (the 2026-11-18 death: 3.3 ms behind a set_pref).
static LATE_TEST: AtomicU64 = AtomicU64::new(0);
/// The engine's place (+G#14) read at the first 2^3 DMA_DONE answers before the index write, against the index then: whether the engine runs one released program a frame (its place one frame behind the index with a lead of two) or every released program at once.
static PLACE_LOG: [AtomicU64; 1 << 3] = [const { AtomicU64::new(0) }; 1 << 3];
/// DMA_DONEs whose descriptor the kernel had released already when this thread answered (camera_power.rs `ferros_cam_fast`): the engine's index read past the thread's own.
static BY_KERNEL: AtomicU64 = AtomicU64::new(0);
/// Answers that saw more than one frame (an open program's slits released while the thread was away), and the most frames one answer saw.
static MULTI: AtomicU64 = AtomicU64::new(0);
static MULTI_MOST: AtomicU64 = AtomicU64::new(0);
/// The first 2^5 answers' image status (low byte) and counter (above it), said at the close.
static IMG_LOG: [AtomicU64; 1 << 5] = [const { AtomicU64::new(0) }; 1 << 5];
/// The same answers' length (counter ticks, from the status read to the end of `answer`, before the line's ack).
static IMG_DUR: [AtomicU64; 1 << 5] = [const { AtomicU64::new(0) }; 1 << 5];
fn img_log_say(g: u64) {
    let us = |t: u64| t * 1_000_000 / ferros_sys::freq();
    let first = IMG_LOG[0].load(Ordering::Relaxed) >> 8;
    let mut l = Line { b: [0; 256], n: 0 };
    // The first one's time on the counter (us), to set them beside the kernel's armed runs (`diag "irqs runs"`); each as status@start+its length, the first 2^4 (a line holds no more).
    let _ = write!(l, "camera: the first answers' image status, from A#{} us on the counter (status@us from the first+us it took):", us(first));
    for (e, d) in IMG_LOG.iter().zip(IMG_DUR.iter()).take(1 << 4) {
        let v = e.load(Ordering::Relaxed);
        if v == 0 {
            break;
        }
        let _ = write!(l, " {:X}@{}+{}", v & 0xFF, us((v >> 8) - first), us(d.load(Ordering::Relaxed)));
    }
    let _ = write!(l, "\n");
    console(g, &l.b[..l.n]);
    let mut p = Line { b: [0; 256], n: 0 };
    let _ = write!(p, "camera: lead A#{}; the engine's place against the index at the first DMA_DONEs:", LEAD.load(Ordering::Relaxed));
    for e in PLACE_LOG.iter() {
        let v = e.load(Ordering::Relaxed);
        let _ = write!(p, " G#{:X}/G#{:X}", v >> 32, v & 0xFFFF_FFFF);
    }
    let _ = write!(p, "\n");
    console(g, &p.b[..p.n]);
}
/// The last 2^8 answers, for the console when a stream stops (camera-full-rate, 2026-11-17: which interrupt came last and how late): per answer the counter at its start (above bit 8) and the image status (low byte); and the index it wrote (low 16 bits, G#FFFF none) with the answer's ticks above.
static TL: [AtomicU64; 1 << 8] = [const { AtomicU64::new(0) }; 1 << 8];
static TL_IDX: [AtomicU64; 1 << 8] = [const { AtomicU64::new(0) }; 1 << 8];
/// From a frame's DMA_DONE answer to its SHADOW_UPDATE answer (the engine running the released program, and the wake), counter ticks: answers counted, summed, the most, and a histogram in powers of two of microseconds.
static LAG: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static LAG_HIST: [AtomicU64; 1 << 4] = [const { AtomicU64::new(0) }; 1 << 4];
/// Between successive DMA_DONE answers (ticks): the least and the most, against the frame period (a late answer shows as one long gap and one short one).
static GAP: [AtomicU64; 2] = [AtomicU64::new(u64::MAX), AtomicU64::new(0)];
/// The counter at the last DMA_DONE answer (0 none since the open).
static DONE_T: AtomicU64 = AtomicU64::new(0);
fn tl_reset() {
    for e in TL.iter().chain(TL_IDX.iter()).chain(LAG.iter()).chain(LAG_HIST.iter()) {
        e.store(0, Ordering::Relaxed);
    }
    GAP[0].store(u64::MAX, Ordering::Relaxed);
    GAP[1].store(0, Ordering::Relaxed);
    DONE_T.store(0, Ordering::Relaxed);
}
/// The last 2^5 answers said (us after the one before, status, index written), then the lag and gap measures.
fn tl_say(g: u64) {
    let us = |t: u64| t * 1_000_000 / ferros_sys::freq();
    let n = ANSWERED.load(Ordering::Relaxed) as usize;
    // WHY saturating: an open with fewer than 2^5 answers starts at its first.
    let k0 = n.saturating_sub(1 << 5);
    let mut l = Line { b: [0; 256], n: 0 };
    let mut prev = TL[k0 & 0xFF].load(Ordering::Relaxed) >> 8;
    // The first one's time on the counter (us), to set the answers beside the kernel's armed runs (`diag "irqs runs"`, the same counter).
    let _ = write!(l, "camera: the last answers (from A#{} us on the counter; us after the one before, status, index written, !error words):", us(prev));
    for k in k0..n {
        let (v, x) = (TL[k & 0xFF].load(Ordering::Relaxed), TL_IDX[k & 0xFF].load(Ordering::Relaxed));
        let _ = write!(l, " {}:{:X}", us((v >> 8) - prev), v & 0xFF);
        if x & 0xFFFF != 0xFFFF {
            let _ = write!(l, ":{:X}", x & 0xFFFF);
        }
        let e = TL_ERR[k & 0xFF].load(Ordering::Relaxed);
        if e != 0 {
            let _ = write!(l, "!{e:X}");
        }
        prev = v >> 8;
        if l.n > 224 {
            let _ = write!(l, "\n");
            console(g, &l.b[..l.n]);
            l.n = 0;
        }
    }
    let _ = write!(l, "\n");
    console(g, &l.b[..l.n]);
    let c = LAG[0].load(Ordering::Relaxed);
    let mut h = Line { b: [0; 256], n: 0 };
    // WHY the max: an open with no frame pair has no lags, its mean read as 0 rather than divided by zero.
    let _ = write!(h, "camera: DMA_DONE to SHADOW_UPDATE: A#{c} frames, A#{} us on average, A#{} us the most; by 2^k us:", us(LAG[1].load(Ordering::Relaxed)) / c.max(1), us(LAG[2].load(Ordering::Relaxed)));
    for (k, e) in LAG_HIST.iter().enumerate() {
        let v = e.load(Ordering::Relaxed);
        if v != 0 {
            let _ = write!(h, " {k}:{v}");
        }
    }
    let _ = write!(h, "; DMA_DONE gaps A#{} to A#{} us\n", us(GAP[0].load(Ordering::Relaxed)), us(GAP[1].load(Ordering::Relaxed)));
    console(g, &h.b[..h.n]);
}
/// Per answer (the timeline's index), loch0's image path's error word (low 16 bits) and its PD path's (bits 16 to 31), as the acknowledgement read them (SHORT_VB bit 1, EBUF_OVERFLOW bit 3, LATE_CONFIG bit 4, WDMA bit 5, SHORT_VB_FATAL bit 13): read and cleared every answer, so this and `ACK_OR` are their only record.
static TL_ERR: [AtomicU64; 1 << 8] = [const { AtomicU64::new(0) }; 1 << 8];
/// The first answer whose error words were not 0: its frame count + 1 (0 none) and the words.
static ERR_FIRST: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
/// The error words' addresses (zuma-isp-event-info.dtsi isp_fe_loch0_csis_core_img_err and pd_err) and the image and error words' overflow words (the same event latched again while still pending: an interrupt missed).
const IMG_ERR: usize = 0x1C32_0438;
const PD_ERR: usize = 0x1C32_2038;
const IMG_OVF: usize = 0x1C32_0428;
const IMG_ERR_OVF: usize = 0x1C32_043C;
fn err_reset() {
    for e in TL_ERR.iter().chain(ERR_FIRST.iter()) {
        e.store(0, Ordering::Relaxed);
    }
    BY_KERNEL.store(0, Ordering::Relaxed);
    MULTI.store(0, Ordering::Relaxed);
    MULTI_MOST.store(0, Ordering::Relaxed);
}
/// What the front end said over the open and says now: the answers' timeline (`tl_say`), the first error, the overflow words, then every acknowledged word and the engine's words (`ack_say`).
fn fe_say(g: u64, what: &str) {
    tl_say(g);
    let mut l = Line { b: [0; 256], n: 0 };
    let _ = write!(l, "camera: {what}: ");
    match ERR_FIRST[0].load(Ordering::Relaxed) {
        0 => {
            let _ = write!(l, "no error word ever set");
        }
        f => {
            let _ = write!(l, "the first error at frame A#{} (G#{:X})", f - 1, ERR_FIRST[1].load(Ordering::Relaxed));
        }
    }
    let _ = write!(l, "; overflow words: image G#{:X}, errors G#{:X}\n", rd(g, IMG_OVF), rd(g, IMG_ERR_OVF));
    console(g, &l.b[..l.n]);
    ack_say(g);
}
/// Bits a sample on the link: 10, or 12 for the RAW12 attempt.
static NOW_FORMAT: AtomicU64 = AtomicU64::new(10);
/// The frame thread's time answering the front end (counter ticks, summed and the most in one answer) and frames whose buffer was not the one the ring named (a slit ring's: the frame's probe did not move, so it landed elsewhere, its descriptor released after the frame began).
static BUSY: AtomicU64 = AtomicU64::new(0);
static BUSY_MOST: AtomicU64 = AtomicU64::new(0);
/// From an answer's start to its index write (counter ticks: answers that wrote it, summed, the most): the part of the answer the frame's blanking must hold (2026-11-16, kernlock).
static IDX: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
/// Quiet spells on a working line: with the front end's status holding bits (an answer its interrupt never brought), and with it clear.
static QUIET_FOUND: AtomicU64 = AtomicU64::new(0);
static QUIET_SEEN: AtomicU64 = AtomicU64::new(0);
static LATE: AtomicU64 = AtomicU64::new(0);
/// Times the index went round the ring (`RING`): a stream past them is the drain working.
static WRAPS: AtomicU64 = AtomicU64::new(0);

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

/// Per buffer, of the frame it holds: its count, when it ended (Eagle ns), the counter at its DMA_DONE, the exposure (ns), gain and focus the sensor had been told; and when it was released to the engine (a sequence number).
static B_COUNT: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_AT: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_TICK: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_NS: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_GAIN: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_FOCUS: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static B_SEQ: [AtomicU64; job::RING_MOST] = [const { AtomicU64::new(0) }; job::RING_MOST];
static SEQ: AtomicU64 = AtomicU64::new(0);
/// The sensor's settings now, for each frame's record (the server sets them at SET): the integration in lines (for STATS) and as an exposure in ns (the frame's header), the gain, the lens.
static NOW_LINES: AtomicU64 = AtomicU64::new(ferros_cam::MAIN_EXPOSURE_MAX_LINES);
static NOW_NS: AtomicU64 = AtomicU64::new(0);
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
            B_NS[b].store(NOW_NS.load(Ordering::Relaxed), Ordering::Relaxed);
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

/// A frame's DMA_DONE with a slit ring (docs/CAMERA.md "The slit"): no choice and no hold, every descriptor named its buffer at the open (descriptor e buffer e mod the ring), so frame n lands in buffer (n - 1) mod the ring when its descriptor was released before it began. Its probe says whether it did (else it went where the last one did: `LATE`), its record is written after its rows (ferros_cam job `slit_bytes` leaves it 64 bytes: the frame's count, the counter at its DMA_DONE, its Eagle time, 1 when its probe moved), and the buffer two frames on has its probe patterned (the next frame's buffer may be under the engine already: since 2026-11-18 the kernel releases its descriptor in the interrupt, before this thread runs, so that probe was patterned a frame earlier; the consumer keeps clear of the two oldest buffers: SLITS).
/// `newest`: this is the last slit the answer handles (an open program's answer may handle several at once): only then is the probe two on patterned, as the earlier ones' next buffers have landed already.
fn slit_turn(n: u64, newest: bool) {
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
    if newest {
        probe_pattern(((n + 1) % m as u64) as usize);
    }
    ferros_sys::dma_barrier();
}

/// The front end's grant's start (`grant_of`): the armed program's offsets are from it.
const FE_BASE: usize = 0x1C30_0000;
/// Whether the kernel took the armed program at this open (`arm`), for the close's line.
static ARMED: AtomicU64 = AtomicU64::new(0);

/// The next descriptor's release armed on the front end's line (SYS_IRQ_ARM, docs/KERNEL.md "Armed actions"): at each interrupt the kernel, before it masks the line and wakes the frame thread and without its lock, tests loch0's DMA_DONE, tests that the engine ran every released program (its place at its index), and advances the index round the ring as `answer` does. The release then never waits for the lock or a core (2026-11-18: the wake 3.3 ms behind a set_pref's hold, a release 2 ms late, both stopped the stream with LATE_CONFIG). `answer` writes the index only while the engine still reads its own, so a kernel that refuses (or lacks) the call leaves the release to the frame thread, as before.
/// A slit ring's program also clears the frame's image bits (SOF, EOF, SHADOW_UPDATE, DMA_DONE) and leaves the line open (ferros_layout irqarm UNMASK), so each slit's DMA_DONE fires its own interrupt and release whatever the frame thread is doing: the frame thread counts the slits from the engine's index instead (`answer`), and loch0's image interrupt is DMA_DONE alone once its path is on (`IMG_MASK`).
/// Why (2026-11-18, slit-robust, the Pro): slits 812 us apart under Lumis, and every death was a frame thread one period late with the line masked; scheduling fixes took the deaths from every run to about half, the rest a thread late for other reasons each time. Not for whole frames: their descriptors are named a frame ahead by the frame thread (`ring_turn`), which a release it did not see would skip.
static OPEN: AtomicU64 = AtomicU64::new(0);
/// loch0's image interrupt mask (zuma-isp-event-info.dtsi irq-mask-reg of isp_fe_loch0_csis_core_img_proc; the init leaves G#1A: EOF, SHADOW_UPDATE, DMA_DONE).
const IMG_MASK: usize = 0x1C32_0430;
/// The image bits an open program clears: SOF, EOF, SHADOW_UPDATE, DMA_DONE.
const IMG_FRAME_BITS: u32 = ferros_cam::IMG_SOF | ferros_cam::IMG_EOF | ferros_cam::IMG_SHADOW | ferros_cam::IMG_DMA_DONE;

fn arm(g: u64) {
    use ferros_layout::irqarm::{encode, Op, ADVANCE, OPS_MOST, OP_BYTES, TEST_BITS, TEST_EQ, UNMASK, WRITE};
    let index = (ferros_cam::PDMA + ferros_cam::PDMA_INDEX - FE_BASE) as u32;
    let status = (ferros_cam::IMG_STATUS - FE_BASE) as u32;
    let ops = [
        Op { op: TEST_BITS, off: status, a: ferros_cam::IMG_DMA_DONE, b: 0 },
        Op { op: TEST_EQ, off: (ferros_cam::PDMA + 0x14 - FE_BASE) as u32, a: 0, b: index },
        Op { op: ADVANCE, off: index, a: STEP, b: RING_N.load(Ordering::Relaxed) as u32 - 1 },
        Op { op: WRITE, off: status, a: 0, b: IMG_FRAME_BITS },
        Op { op: UNMASK, off: status, a: 0, b: 0 },
    ];
    let mut b = [0u8; OPS_MOST * OP_BYTES];
    // A slit ring tries the open program first; a kernel without UNMASK refuses it, and gets the three operations.
    let open = SLITS.load(Ordering::Relaxed) != 0 && NO_OPEN.load(Ordering::Relaxed) == 0;
    let mut r = 1;
    if open {
        let n = encode(&ops, &mut b);
        r = ferros_sys::irq_arm(cap(g, S_IRQ), cap(g, S_FE), &b[..n]);
    }
    OPEN.store((open && r == 0) as u64, Ordering::Relaxed);
    if r != 0 {
        let n = encode(&ops[..3], &mut b);
        r = ferros_sys::irq_arm(cap(g, S_IRQ), cap(g, S_FE), &b[..n]);
    }
    ARMED.store((r == 0) as u64, Ordering::Relaxed);
    say!(g, "camera: the descriptor's release {}\n", if r != 0 { "not armed (the kernel refused or lacks SYS_IRQ_ARM): the frame thread releases it" } else if OPEN.load(Ordering::Relaxed) != 0 { "armed on the front end's line, open (the kernel releases it and clears the slit's bits at each interrupt, the line never masked)" } else { "armed on the front end's line (the kernel releases it at the interrupt)" });
}
/// A device test (the hand spawn's tenth input byte 1): a slit ring armed with the three operations, the line masked at each interrupt as before, for the A/B.
static NO_OPEN: AtomicU64 = AtomicU64::new(0);

/// The front end answered once: loch0's image status read; on a frame's DMA_DONE the ring's turn (`ring_turn`) and the engine's index advanced at once; then every acknowledgement made in Linux's order, loch0's paths enabled after the first SHADOW_UPDATE, and the frame counted.
/// The index first (2026-11-16, kernlock): the next descriptor must be released before the next frame begins, so the frame's blanking (128 lines, about 400 us, at the main's 96 a second) has to hold the interrupt's wake and everything the answer does before the index write; with the 56 acknowledgements ahead of it (about 170 us of register syscalls) the stream lost every other frame and stopped.
/// The acknowledgements clear the front end's status words (loch0's image status among them, read again there); the descriptor engine has its own index, so the order between the two is not the front end's.
fn answer(g: u64, idx: &mut u32, enabled: &mut bool) {
    let t0 = ferros_sys::now();
    let img = rd(g, ferros_cam::IMG_STATUS);
    // The first answers' image status and their counter, for the console at the close (what each of a frame's interrupts carries, the readout's ends among them).
    let k = ANSWERED.load(Ordering::Relaxed) as usize;
    if k < IMG_LOG.len() {
        IMG_LOG[k].store((t0 << 8) | img as u64 & 0xFF, Ordering::Relaxed);
    }
    TL[k & 0xFF].store((t0 << 8) | img as u64 & 0xFF, Ordering::Relaxed);
    let mut wrote = 0xFFFFu64;
    let last_done = DONE_T.load(Ordering::Relaxed);
    // The answer before this one. WHY wrapping: the first answer (k 0) looks at slot 255, still 0 then.
    let before = TL[k.wrapping_sub(1) & 0xFF].load(Ordering::Relaxed);
    // The frames this answer sees. With an open program (`OPEN`) the kernel released each slit and cleared its bits at its own interrupt, so the engine's index past ours counts them (DMA_DONE is no longer in the status); a slit whose release the kernel's test refused still shows DMA_DONE with the index ours, and this thread releases it. Otherwise one frame on DMA_DONE, released by the kernel already when the index reads past ours, as before.
    let count = RING_N.load(Ordering::Relaxed) as u32;
    let dma = img & ferros_cam::IMG_DMA_DONE != 0;
    let hw = if dma || OPEN.load(Ordering::Relaxed) != 0 { rd(g, ferros_cam::PDMA + ferros_cam::PDMA_INDEX) } else { *idx };
    // WHY wrapping: the index goes round the ring (`count`, a power of two), and the mask makes the difference the ring's.
    let by_kernel = (hw.wrapping_sub(*idx) & (count - 1)) / STEP;
    let nf = if by_kernel != 0 { by_kernel } else { dma as u32 };
    if nf != 0 {
        if last_done != 0 {
            GAP[0].fetch_min(t0 - last_done, Ordering::Relaxed);
            GAP[1].fetch_max(t0 - last_done, Ordering::Relaxed);
        }
        DONE_T.store(t0, Ordering::Relaxed);
    } else if img & ferros_cam::IMG_SHADOW != 0 && last_done != 0 && before & ferros_cam::IMG_DMA_DONE as u64 != 0 {
        let d = t0 - last_done;
        LAG[0].fetch_add(1, Ordering::Relaxed);
        LAG[1].fetch_add(d, Ordering::Relaxed);
        LAG[2].fetch_max(d, Ordering::Relaxed);
        let us = d * 1_000_000 / ferros_sys::freq();
        // WHY the min: the histogram has 2^4 bins, a lag of 2^15 us or more lands in the last.
        LAG_HIST[((64 - us.leading_zeros()) as usize).min(15)].fetch_add(1, Ordering::Relaxed);
    }
    if nf != 0 {
        let late = LATE_TEST.load(Ordering::Relaxed);
        // WHY the bound: the byte is the person's; a shift of 64 or more is no number of microseconds, and past 2^20 (a second) the test is a stall, not a late answer.
        // Bit 7 set: the index on time and the acknowledgements late instead (below), which says whether the index alone is the deadline.
        if late != 0 && late <= 20 && (FRAMES.load(Ordering::Relaxed) + 1) & 0xFF == 0 {
            ferros_sys::delay_us(1 << late);
        }
        // The next descriptor released first, round the ring as the engine's own place goes round it (`RING`): the index is never the engine's place while a frame is owed, so the stream runs as long as it is answered.
        // Released by the kernel already when it reads past ours (irqarm.rs, in the interrupt, before this thread is woken): then only our count follows it. The deadline is the frame's blanking (2026-11-18: a release 2 ms late at 96 a second stopped the stream with LATE_CONFIG), which a wake behind the kernel lock can miss.
        if by_kernel == 0 {
            wr(g, ferros_cam::PDMA + ferros_cam::PDMA_INDEX, (*idx + STEP) & (count - 1));
        } else {
            BY_KERNEL.fetch_add(by_kernel as u64, Ordering::Relaxed);
        }
        let done = IDX[0].load(Ordering::Relaxed) as usize;
        if done < PLACE_LOG.len() {
            PLACE_LOG[done].store((rd(g, ferros_cam::PDMA + 0x14) as u64) << 32 | hw as u64, Ordering::Relaxed);
        }
        if nf > 1 {
            MULTI.fetch_add(1, Ordering::Relaxed);
            MULTI_MOST.fetch_max(nf as u64, Ordering::Relaxed);
        }
        let t = ferros_sys::now() - t0;
        IDX[0].fetch_add(1, Ordering::Relaxed);
        IDX[1].fetch_add(t, Ordering::Relaxed);
        IDX[2].fetch_max(t, Ordering::Relaxed);
        // Each frame: the buffer it landed in, and (whole frames) a buffer named for the descriptor at the index now, the one released at the next DMA_DONE (by the kernel or by this thread), so it is named a whole frame before it is needed.
        for j in 0..nf {
            *idx = (*idx + STEP) & (count - 1);
            if *idx == 0 {
                WRAPS.fetch_add(1, Ordering::Relaxed);
            }
            let n = FRAMES.load(Ordering::Relaxed) + 1 + j as u64;
            if SLITS.load(Ordering::Relaxed) != 0 {
                slit_turn(n, j + 1 == nf);
            } else {
                ring_turn(n, Some((*idx / STEP) as u64));
            }
        }
        wrote = *idx as u64;
        let late = LATE_TEST.load(Ordering::Relaxed) & 0x7F;
        if LATE_TEST.load(Ordering::Relaxed) & 0x80 != 0 && late != 0 && late <= 20 && (FRAMES.load(Ordering::Relaxed) + 1) & 0xFF == 0 {
            ferros_sys::delay_us(1 << late);
        }
    }
    let mut err = 0u64;
    for (j, &a) in cam().ack.iter().enumerate() {
        let v = rd(g, a);
        // loch0's image status: a DMA_DONE this answer did not see (latched since its first read) is left latched, so the line fires again at the ack and the kernel's armed action (or the next answer) releases its descriptor (2026-11-18, slit-robust: an answer 786 us late read one DMA_DONE, the next landed during its acknowledgements and was cleared unanswered, its descriptor never released, and the frame after died on LATE_CONFIG).
        let w = if a == ferros_cam::IMG_STATUS { v & (img | !ferros_cam::IMG_DMA_DONE) } else { v };
        // A word that read 0 is not written back: writing a write-one-to-clear word 0 clears nothing, and each write is a register syscall (2026-11-18, slit-robust: 56 a answer, about 91 us of the frame thread's every 812 us in a slit stream, past the eighth of a core under which the scheduler keeps a woken device thread at the front; with its credit spent the thread queued behind Lumis's and the slit stream died).
        if w != 0 {
            wr(g, a, w);
        }
        ACK_OR[j].fetch_or(v as u64, Ordering::Relaxed);
        ACK_LAST[j].store(v as u64, Ordering::Relaxed);
        if a == IMG_ERR {
            err |= v as u64 & 0xFFFF;
        } else if a == PD_ERR {
            err |= (v as u64 & 0xFFFF) << 16;
        }
    }
    TL_ERR[k & 0xFF].store(err, Ordering::Relaxed);
    if err != 0 && ERR_FIRST[0].load(Ordering::Relaxed) == 0 {
        ERR_FIRST[1].store(err, Ordering::Relaxed);
        ERR_FIRST[0].store(FRAMES.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
    }
    IMG_SEEN.fetch_or(img as u64, Ordering::Relaxed);
    ANSWERED.fetch_add(1, Ordering::Relaxed);
    if !*enabled && img & ferros_cam::IMG_SHADOW != 0 {
        // The image path alone when the PD path is off (a mode whose PD rows' slot is sized for the binned frame, or a slit): G#01 of Linux's G#81.
        let pd = PD_ON.load(Ordering::Relaxed) != 0;
        wr(g, ferros_cam::LOCH_ENABLE, if pd { ferros_cam::LOCH_ENABLE_IMG_PD } else { ferros_cam::LOCH_ENABLE_IMG_PD & 1 });
        *enabled = true;
        // An open program clears the frame's bits itself: only DMA_DONE raises loch0's image interrupt from now on, so an EOF or SHADOW_UPDATE after the release never fires a second interrupt that masks the line (mask G#1A as the init leaves it: EOF, SHADOW_UPDATE, DMA_DONE).
        if OPEN.load(Ordering::Relaxed) != 0 {
            wr(g, IMG_MASK, ferros_cam::IMG_DMA_DONE);
        }
        say!(g, "camera: SHADOW_UPDATE; loch0's image path enabled{}{}\n", if pd { " with its PD path" } else { ", its PD path left off" }, if OPEN.load(Ordering::Relaxed) != 0 { "; its image interrupt DMA_DONE alone" } else { "" });
    }
    if nf != 0 {
        FRAME_AT.store(eagle_ns(), Ordering::Release);
        // The frame now in flight began at this DMA_DONE, at the timing written before it (`exposure_cut` reckons its end from both).
        FLIGHT_AT.store(t0, Ordering::Relaxed);
        FLIGHT_NS.store(PERIOD_NS.load(Ordering::Relaxed), Ordering::Relaxed);
        let n = FRAMES.fetch_add(nf as u64, Ordering::AcqRel) + 1;
        if n == 1 {
            let us = ferros_sys::us_since(OPEN_AT.load(Ordering::Relaxed));
            step!(g, "camera: first frame A#{} ms after the open (open step: first frame)\n", us / 1000);
        }
        // The server's doorbell: counted always, rung only while it holds a call that waits for a frame (`WANT`), so a server holding none is not woken a thousand times a second by slits (`door_ring`).
        door_ring();
    }
    let t = ferros_sys::now() - t0;
    if k < IMG_DUR.len() {
        IMG_DUR[k].store(t, Ordering::Relaxed);
    }
    BUSY.fetch_add(t, Ordering::Relaxed);
    TL_IDX[k & 0xFF].store(wrote | t << 16, Ordering::Relaxed);
    BUSY_MOST.fetch_max(t, Ordering::Relaxed);
}

extern "C" fn frame_thread(g: u64) -> ! {
    // Not pinned (measured on the Pro, b898 and kernlock, 2026-11-16): pinned to the X3 the main's stream at 96 a second stopped within 19 frames (the X3 runs the service loop, the render thread and ahau), pinned to core 7 within 5 (a consumer thread there is never preempted for it); unpinned with the kernel's `dev first` (or `dev core 7` with it) 2048 frames ran clean with consumers on all four A715s (docs/CAMERA.md "The kernel lock and the frame thread").
    let line = cap(g, S_IRQ);
    // A measuring build pins it (FERROS_CAM_PIN, a core mask in hex: G#100 the X3, G#80 core 7), then sleeps a millisecond so the pin takes at the next reschedule (kernlock, 2026-11-16: the wake's latency pinned and not).
    if let Some(m) = option_env!("FERROS_CAM_PIN").and_then(|s| u64::from_str_radix(s, 16).ok()) {
        ferros_sys::pin(m);
        ferros_sys::sleep_until(ferros_sys::now() + (ferros_sys::freq() >> 10));
    }
    // Idle until the stream starts: the server rings RUN.
    while RUN.load(Ordering::Acquire) == 0 {
        ferros_sys::wait(&RUN, 0, 0);
    }
    let (mut idx, mut enabled, mut ticking) = (STEP * LEAD.load(Ordering::Relaxed) as u32, false, false);
    let started = ferros_sys::now();
    // The line's last interrupt: a long exposure's frame leaves the line quiet for its whole period, which is not a stream that stopped.
    let mut last = started;
    // Whether the last answer on the line was a frame's DMA_DONE: the readout watch waits for the answer after it (the frame's SHADOW_UPDATE, about 150 us later), as the front end must have that one answered too (b933: a watch begun at DMA_DONE left SHADOW_UPDATE unanswered for a period, and the next frame began and never ended).
    let mut done_last = false;
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
            let before = FRAMES.load(Ordering::Relaxed);
            answer(g, &mut idx, &mut enabled);
            last = ferros_sys::now();
            ON_LINE.fetch_add(1, Ordering::Relaxed);
            ferros_sys::irq_ack(line);
            let done = FRAMES.load(Ordering::Relaxed) != before;
            if MEASURE.load(Ordering::Relaxed) == 1 && done_last && !done && SLITS.load(Ordering::Relaxed) == 0 {
                row_timing();
            }
            done_last = done;
        } else if ticking {
            answer(g, &mut idx, &mut enabled);
        } else if ON_LINE.load(Ordering::Relaxed) == 0 {
            // No interrupt on the line in the front end's own deadline: answered on the tick from now on, and the console says so with the words that would say why.
            ticking = true;
            TICKING.store(1, Ordering::Relaxed);
            say!(g, "camera: no interrupt on SPI 479 in A#{} us of the stream; answering on a A#{} us tick (front end aggregate G#{:X}, image G#{:X}, engine G#{:X} index G#{:X})\n", ferros_sys::us_since(started), tick_us(), rd(g, ferros_cam::CMN_STATUS), rd(g, ferros_cam::IMG_STATUS), rd(g, ferros_cam::PDMA), rd(g, ferros_cam::PDMA + ferros_cam::PDMA_INDEX));
            answer(g, &mut idx, &mut enabled);
        } else {
            // The line works and was quiet for `QUIET_US`: either the stream stopped, or the front end waits on an answer whose interrupt never came (2026-11-16, kernlock: streams at 96 a second stopped with every answer prompt). Its status words say which; one that holds bits is answered, counted, and said once.
            let (cmn, img) = (rd(g, ferros_cam::CMN_STATUS), rd(g, ferros_cam::IMG_STATUS));
            if cmn | img != 0 {
                if QUIET_FOUND.fetch_add(1, Ordering::Relaxed) == 0 {
                    say!(g, "camera: the line quiet A#{} us with the front end waiting (aggregate G#{cmn:X}, image G#{img:X}, engine G#{:X} index G#{:X}, our index G#{idx:X}); answered\n", QUIET_US, rd(g, ferros_cam::PDMA), rd(g, ferros_cam::PDMA + ferros_cam::PDMA_INDEX));
                    fe_say(g, "the line quiet");
                }
                answer(g, &mut idx, &mut enabled);
            } else if ferros_sys::us_since(last) > QUIET_US + (period_ns() << 1) / 1000 && QUIET_SEEN.fetch_add(1, Ordering::Relaxed) == 0 {
                say!(g, "camera: the line quiet A#{} us, past two frame periods, the front end's status clear (engine G#{:X} index G#{:X}, our index G#{idx:X}): the stream stopped by itself\n", ferros_sys::us_since(last), rd(g, ferros_cam::PDMA), rd(g, ferros_cam::PDMA + ferros_cam::PDMA_INDEX));
            }
        }
    }
}

// ---------------------------------------------------------------- the open

/// Buffers for the vault's values: the init (about 22 KB), the CPU's program (about 37 KB), the bundle of both (about 60 KB), the job (about 35 KB).
const VALUE: usize = 1 << 16;
static mut INIT: [u8; VALUE] = [0; VALUE];
static mut PROG: [u8; VALUE] = [0; VALUE];
static mut BUNDLE: [u8; 2 * VALUE] = [0; 2 * VALUE];
static mut JOB: [u8; VALUE] = [0; VALUE];
/// The 1:1 init (`cam.<key>.init.full`, the main's 15499 bytes), for an OPEN of a 1:1 kind and for LIST's 1:1 section.
static mut FULL: [u8; VALUE] = [0; VALUE];
/// A device test with nothing stored: the 1:1 init's length when the hand spawn's input carried it (`FULL_INPUT_AT`), held in `FULL` from the start and used in place of the vault's; 0 none.
static FULL_INPUT: AtomicU64 = AtomicU64::new(0);
/// Where the spawn's input carries the 1:1 init: after 16 knob bytes, framed as ferros-bridge's `--input-file` frames a file (u32 LE length, then the bytes): `spawn camera.bin 01000000000000000000000000000000 --input-file target/cam/main-full.init`.
const FULL_INPUT_AT: u64 = 1 << 4;

/// Camera `c`'s 1:1 init: the spawn's (a device test, `FULL_INPUT`), else the vault's `cam.<key>.init.full`; None when neither.
fn full_init(g: u64, c: &Cam) -> Option<&'static [u8]> {
    let n = FULL_INPUT.load(Ordering::Relaxed) as usize;
    if n != 0 {
        return Some(unsafe { &(&*(&raw const FULL))[..n] });
    }
    let mut kf = [0u8; 32];
    get(g, input_key(&mut kf, c, b"init.full"), unsafe { &mut *(&raw mut FULL) })
}

/// A fill no frame holds, so a write anywhere in an output shows (docs/oracle/CAMERA.md "the write test was blind to dark frames": a point reading anything else, zero included, was written).
const PATTERN: u64 = 0xA5A5_A5A5_A5A5_A5A5;

/// The open camera.
struct Open {
    /// Where this process reads the region and where it lies (its physical start went into the job), and the region's layout with its image ring of `ring` buffers.
    va: usize,
    pa: u64,
    l: job::Layout,
    ring: usize,
    /// The sensor's frame timing (ferros_cam ccs `Timing`: the mode's line and frame, the integration's least and margin as the sensor answered them) and what its exposure now set it to (`Plan`).
    timing: ccs::Timing,
    now: ccs::Plan,
    /// A device test's forced vendor shift (SET 3; 0: the plan's least): every exposure SET after it at exactly that shift (ferros_cam ccs `Timing::plan_shift`), to find how far the shift field reaches.
    force: u8,
    /// The sensor's settings now (the gain code, the lens's target) and the frame counted at the last FRAME or MAP request.
    gain: u64,
    focus: u64,
    asked_at: u64,
    /// The counter when the holder last asked for anything (a FRAME, MAP, SLITS, STATS or RATE): 2^22 us past it with nobody asking, the holder is taken for gone.
    asked_tick: u64,
    /// The front end writes the captured geometry (the init's mode, or only its timing changed): FRAME by copy bins it; any other shape is the view's alone.
    captured: bool,
    /// The sensor's gain model from its own capability words (ferros_cam ccs `Gain`), read at the open; None when they describe none (SET then takes the captured gain alone, as before 2026-11-17).
    gain_model: Option<ccs::Gain>,
    /// The outputs' extents have been reported (after the eighth frame).
    probed: bool,
    /// The view was given to the holder (an OPEN that asked for it).
    view: bool,
    /// The view is mapped write-back (docs/CAMERA.md "A cacheable view"; the OPEN's `VIEW_FLAGS` byte with `VIEW_CACHED`): every buffer handed over by MAP has its lines cleaned and invalidated first (`lines`), and a held buffer's probe line before its release reads it.
    cached: bool,
    /// The maintenance's ticks at MAP (summed and the most, for the console every 64).
    inv_sum: u64,
    inv_max: u64,
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

/// The sensor's exposure as planned (ferros_cam ccs `Timing::plan`), inside its grouped-parameter hold, so the line, the frame and the integration land together at one frame's start (the main's G#0104 is one byte: the hold is 01 04 01 01, its release 01 04 00 01, Linux's per-frame block, camera-main-open-transfers.txt 771-778; the ultrawide's 8-bit registers take 01 04 01 and 01 04 00, the end of its captured init).
/// The integration (CCS coarse_integration_time, G#0202) always; the vendor's shift words when the camera has them, as Android writes them in every hold; the line length (G#0342) and frame length (G#0340) only when they move, so an exposure inside the mode's frame writes what it wrote before 2026-11-17.
/// The frame inside the hold with the integration is Android's own way: its per-frame block on the main is the hold, the shift words, the gains, the integration and the frame length, every frame (target/hv/cam-open3, 2026-10-19). The line moving mid-stream is not in any capture: the device test measures it first (docs/CAMERA.md "Long exposures").
fn exposure_set(bus: &Bus, c: &Cam, t: &ccs::Timing, now: &ccs::Plan, p: &ccs::Plan) -> Result<(), u32> {
    let w16 = |a: u16, v: u16| bus.write(c.addr, &[(a >> 8) as u8, a as u8, (v >> 8) as u8, v as u8]);
    bus.write(c.addr, c.hold)?;
    let r = (|| {
        if p.llp != now.llp {
            w16(ccs::LINE_LENGTH_PCK, p.llp)?;
        }
        if let Some(l) = t.lte {
            if l.byte {
                // Sony's shift, one byte at its register alone (ccs `Lte::byte`): a 16-bit write would also write the register after it.
                bus.write(c.addr, &[(l.fll_reg >> 8) as u8, l.fll_reg as u8, l.word(p.shift) as u8])?;
            } else {
                w16(l.fll_reg, l.word(p.shift))?;
                w16(l.coarse_reg, l.word(p.shift))?;
            }
        }
        w16(ccs::COARSE_INTEGRATION, p.coarse)?;
        if p.fll != now.fll {
            w16(ccs::FRAME_LENGTH_LINES, p.fll)?;
        }
        Ok(())
    })();
    bus.write(c.addr, c.release)?;
    r
}

/// CCS fast_standby_ctrl (G#0106, one byte): 1, a stream off truncates the frame in flight at once instead of finishing it (CCS's "frame truncation"); 0 (CCS's default, unread on these sensors) finishes it. None of the four local inits writes G#0106 directly (scanned 2026-11-18; Samsung's indirect window not decoded for it).
const FAST_STANDBY: [u8; 3] = [0x01, 0x06, 0x01];

/// The frame in flight dropped and the exposure's first frame begun now (the coordinator, 2026-11-18, for Lumis: "A SET whose new exposure would end before the frame in flight should drop that frame and start the new one at once ... The cost is one dropped frame"), when `cut_due` says so.
/// Every exposure change measured mid-stream lands at a frame's start (the hold: the 16 s change of 2026-11-17 night took the frames in flight first, the 8 s step four of them), so nothing in the hold cuts a frame short; the stream goes off and on instead: frame truncation asked (`FAST_STANDBY`), the stream off, the new timing written inside the hold while the sensor stands by (the open's own order: the mode in the hold, then stream on), the stream on.
/// Unmeasured: whether this sensor truncates (CCS allows a sensor to finish the frame regardless; then the stream on finds it still streaming and the new timing lands at the next frame's start, as without the cut), and whether the front end takes a stream off and on with no close between (the link goes quiet and comes back as at an open's stream on; the front end sees no frame start before the readout, at the frame's end, which the cut never reaches: `CUT_US`).
fn exposure_cut(bus: &Bus, c: &Cam, t: &ccs::Timing, now: &ccs::Plan, p: &ccs::Plan) -> Result<(), u32> {
    bus.write(c.addr, &FAST_STANDBY)?;
    bus.write(c.addr, c.stream_off)?;
    let r = exposure_set(bus, c, t, now, p);
    // On again whatever the writes did: a stream left off sends no frame, and the held call would wait out its whole deadline.
    bus.write(c.addr, c.stream_on)?;
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
/// `kind`: 0 the init's mode (Android's binned), 1 the binned mode with its frame and integration cut, 2 the 1:1 readout (the 1:1 init `cam.<key>.init.full` replayed in place of the binned one: Android's own maximum-resolution mode, the main's 8160 x 6144 from the 16 s capture; docs/CAMERA.md "1:1 on the main"), 3 a binned slit, 4 a 1:1 slit (from the same 1:1 init).
/// `rows` a slit's output rows (binned: whole 2-row Bayer cells; 1:1, whole 4-row quad cells), `fll` the frame in lines (0: the rows and `BLANK` more, or for kind 2 the 1:1 init's own; G#FFFF the captured mode's own), `exposure` the integration in lines (0: `EXPOSURE`, or for kind 2 the 1:1 init's own: an all-zero block of kind 2 is Android's 1:1 mode as its init leaves it, as an all-zero block is the binned init's), `flags` the `F_*` bits, `ring` a slit ring's buffers as a power of two (0: 2^8), `llp` the line length (0: the mode's), `cols` the window's output columns across the middle (0: the mode's whole width; on 2^4, ferros_cam ccs `Mode::cols`).
/// A windowed mode (a slit, or `cols` set) and a 1:1 mode turn the sensor's phase detect and embedded data off in its hold (ferros_cam ccs `Side`), unless `F_LOUD`: both are laid out for the whole binned frame (the 1:1 init writes the same 2040 x 768 PD block as the binned one, and the PD channel's size at 1:1 was never captured).
#[derive(Clone, Copy)]
struct Spec {
    kind: u8,
    rows: u16,
    fll: u16,
    exposure: u16,
    flags: u8,
    ring: u8,
    llp: u16,
    cols: u16,
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
/// A windowed mode keeps the sensor's phase detect and embedded data as the init left them (the 2026-11-16 slits' way: ERR_CRC_PH, one frame an open).
const F_LOUD: u8 = 1 << 7;
/// `fll`'s word for the mode's own frame length.
const FLL_OWN: u16 = 0xFFFF;
/// Lines of frame past a mode's rows when `fll` is 0: a 4-row slit ran at 96 lines and stopped at 68 (2026-10-21), so 2^7 past the rows holds the slit with room; a whole frame gets 2^8.
const BLANK: u16 = 1 << 7;
/// The integration (lines) when `exposure` is 0: short, so the bench measures the readout and not the light.
const EXPOSURE: u16 = 1 << 4;
/// A slit ring's buffers: 2^8 by default (a binned 4-row slit's 32 KiB each, 8 MiB), at most 2^12 (the slim copies' 4 MiB).
const SLIT_RING: u8 = 8;
const SLIT_RING_MOST: u8 = 12;
/// The engine ring's count for a slit: 2^16 bytes, 4096 descriptors (`RING`), so every slit ring (to 2^12 buffers) divides them; if the count register holds it (`RING` else, `RING_JOB` last). The index goes round it: the stream runs as long as it is answered.
const RING_SLIT: u64 = 1 << 16;

impl Spec {
    fn of(msg: &[u8], got: usize) -> Spec {
        // WHY the length test: the block is optional and the message's tail past `got` is the last call's bytes, not this one's.
        let b = |k: usize| if k < got { msg[k] } else { 0 };
        let h = |k: usize| u16::from_le_bytes([b(k), b(k + 1)]);
        Spec { kind: b(3), rows: h(4), fll: h(6), exposure: h(8), flags: b(10), ring: b(11), llp: h(12), cols: h(14) }
    }
    fn slit(&self) -> bool {
        matches!(self.kind, K_SLIT | K_SLIT_FULL)
    }
    /// A mode with its own window (a slit, or an x window), as against a whole frame.
    fn windowed(&self) -> bool {
        self.slit() || self.cols != 0
    }
    /// A mode from the 1:1 init (`cam.<key>.init.full`): its whole frame or a slit of it.
    fn full(&self) -> bool {
        matches!(self.kind, K_FULL | K_SLIT_FULL)
    }
    /// The processing blocks' switches off and whether the slim program runs (ferros_cam geom `shape`, host-tested), from the hand spawn's eighth byte (`DETACH`). The defaults are the main's alone: the switches are read off its program, and the ultrawide's slim program took one frame (2026-11-16). A 1:1 frame takes a window's defaults: the statistics pipeline is laid out for the binned frame.
    fn shape(&self, main: bool) -> (u8, bool) {
        geom::shape(DETACH.load(Ordering::Relaxed) as u8, self.windowed() || self.full(), main, self.flags & F_SLIM != 0)
    }
}

/// The sensor's mode for `s` (None: the init's, untouched), built from the init's own (`ccs::Mode::captured`; for the 1:1 kinds the 1:1 init's, `ccs::Mode::captured_full`, which `init` then is).
/// The 1:1 frame (kind 2) defaults to the 1:1 init's own frame and integration, and is always written in the hold (its own registers again, with phase detect and embedded data off unless `F_LOUD`).
fn mode_of(c: &Cam, init: &[u8], s: &Spec) -> Result<Option<ccs::Mode>, Fail> {
    if s.kind == K_CAPTURED {
        return Ok(None);
    }
    let a = &ccs::ARRAYS[c.id as usize];
    let base = match s.kind {
        K_BINNED | K_SLIT => ccs::Mode::captured(init, a).map_err(Fail::Mode)?,
        K_FULL | K_SLIT_FULL => ccs::Mode::captured_full(init, a).map_err(Fail::Mode)?,
        _ => return Err(Fail::Inputs("no such mode kind")),
    };
    let exp = match s.exposure {
        0 if s.kind == K_FULL => base.coarse,
        0 => EXPOSURE,
        e => e,
    };
    let rows = if s.slit() { s.rows } else { base.y_out };
    let fll = match s.fll {
        0 if s.slit() => rows + BLANK,
        0 if s.kind == K_FULL => base.fll,
        0 => rows + (BLANK << 1),
        FLL_OWN => base.fll,
        f => f,
    };
    let mut m = if s.slit() { base.slit(a, rows, fll, exp) } else { base.timed(fll, exp) }.map_err(Fail::Mode)?;
    if s.llp != 0 {
        m.llp = s.llp;
    }
    if s.cols != 0 {
        m = m.cols(a, s.cols).map_err(Fail::Mode)?;
    }
    if (s.windowed() || s.full()) && s.flags & F_LOUD == 0 {
        m = m.quiet();
    }
    if s.flags & F_RAW12 != 0 {
        m = m.raw12();
    }
    Ok(Some(m))
}

/// The sensor's gain model (its CCS words G#0080 to G#0093 and G#1084 to G#1089, ferros_cam ccs `Gain`) and the gain its init left (G#0204 and G#020E as the init wrote them last), said on the console with the words themselves; (None, 1024) when the words describe no model or do not read.
fn gain_of(g: u64, bus: &Bus, c: &Cam, init: &[u8]) -> (Option<ccs::Gain>, u64) {
    let (mut a, mut d) = ([0u8; ccs::GAIN_CAPS_LEN], [0u8; ccs::DGAIN_CAPS_LEN]);
    let read = bus.write_read(c.addr, &ccs::GAIN_CAPS.to_be_bytes(), &mut a).is_ok() && bus.write_read(c.addr, &ccs::DGAIN_CAPS.to_be_bytes(), &mut d).is_ok();
    let be = |b: &[u8], k: usize| u16::from_be_bytes([b[k], b[k + 1]]);
    let mut l = Line { b: [0; 256], n: 0 };
    let _ = write!(l, "camera: {}: gain words G#0080:", c.name);
    for k in 0..ccs::GAIN_CAPS_LEN / 2 {
        let _ = write!(l, " {:X}", be(&a, 2 * k));
    }
    let _ = write!(l, "; G#1084: {:X} {:X} {:X}{}\n", be(&d, 0), be(&d, 2), be(&d, 4), if read { "" } else { " (a read failed)" });
    console(g, &l.b[..l.n]);
    let model = if read { ccs::Gain::of(&a, &d) } else { None };
    let p = ccs::Page::of(init);
    let Some(m) = model else {
        say!(g, "camera: {}: no gain model in those words; gain stays at the init's code G#{:X}\n", c.name, p.get(ccs::ANALOGUE_GAIN).unwrap_or(0));
        return (None, ccs::UNITY);
    };
    let code = p.get(ccs::ANALOGUE_GAIN).map_or(m.code_min, |v| v as u16);
    let dig = p.get(ccs::DIGITAL_GAIN).unwrap_or(0x100) as u64;
    let now = m.analogue(code) * dig >> 8;
    let (lo, hi) = m.range();
    say!(g, "camera: {}: gain = (A#{} code + A#{}) / (A#{} code + A#{}), codes A#{} to A#{} step A#{} (analogue A#{} to A#{}), digital G#{:X} to G#{:X} step G#{:X} (never used: analogue only): A#{lo} to A#{hi} offered (1024 one); the init left code G#{code:X}, A#{now}\n", c.name, m.m0, m.c0, m.m1, m.c1, m.code_min, m.code_max, m.step, m.analogue(m.code_min), m.analogue(m.code_max), m.d_min, m.d_max, m.d_step);
    (Some(m), now)
}

/// The gain set inside the sensor's hold (as exposure is: the hold makes the change land whole at a frame's start): the analogue code, then digital gain as `s` has it (always one: analogue only).
fn gain_set(bus: &Bus, c: &Cam, s: &ccs::GainSet) -> Result<(), u32> {
    bus.write(c.addr, c.hold)?;
    let a = ccs::ANALOGUE_GAIN.to_be_bytes();
    let d = ccs::DIGITAL_GAIN.to_be_bytes();
    let r = bus.write(c.addr, &[a[0], a[1], (s.code >> 8) as u8, s.code as u8]).and_then(|_| bus.write(c.addr, &[d[0], d[1], (s.digital >> 8) as u8, s.digital as u8]));
    bus.write(c.addr, c.release)?;
    r
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
    // A 1:1 mode replays the 1:1 init in place of the binned one (`cam.<key>.init.full`, docs/CAMERA.md "1:1 on the main"); the CPU's writes and the job stay the binned session's, their geometry rewritten to the 1:1 frame as any mode's is.
    let init = if s.full() { full_init(g, c).ok_or(Fail::Inputs("no cam.<key>.init.full (the 1:1 init) for this camera in the vault"))? } else { init };
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
    let (detach, slim) = s.shape(c.id == 0);
    let slim_level = if s.flags & F_SLIM2 != 0 { 2 } else { 1 };
    // WHY the bound: the ring's exponent is the caller's byte; past `SLIT_RING_MOST` the slim copies outgrow the engine's area and the view's window.
    let slit_n = if s.ring == 0 { 1usize << SLIT_RING } else if s.ring <= SLIT_RING_MOST { 1usize << s.ring } else { return Err(Fail::Inputs("a slit ring past 2^12 buffers")) };
    // The PD path on (the init's mode, or asked): each ring buffer holds its frame's PD rows right after its image (owner, 2026-11-17: phase detect as data, one MAP giving both; docs/CAMERA.md "Phase detect as data"), the main's 768 lines at the image's stride past its 3072, so a buffer is 20 MiB, not 16. `pd_off` the rows' place in the buffer, 0 none.
    let pd_on = s.kind == K_CAPTURED || s.flags & F_PD != 0;
    let pd_off = if pd_on && !slit { fe.h as u64 * fe.stride as u64 } else { 0 };
    let buf = if slit { job::slit_bytes(fe.h as u64, fe.stride as u64) } else if pd_off != 0 { job::buffer_bytes(fe.h as u64 + fe.pd_h as u64, fe.stride as u64) } else { job::buffer_bytes(fe.h as u64, fe.stride as u64) };
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
    step!(g, "camera: open step: switch on; the sensor's identity and init next\n");
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
    step!(g, "camera: {}: switch on in A#{} ms; sensor G#{:02X}{:02X}; init A#{w} writes, A#{r} reads in A#{} ms (open step: init done)\n", c.name, powered / 1000, id[0], id[1], ferros_sys::us_since(t_init) / 1000);
    let arr = &ccs::ARRAYS[c.id as usize];
    if MEASURE.load(Ordering::Relaxed) == 2 {
        // Samsung's two 16-bit words only: the test writes whole words, which on a one-byte shift (Sony's G#3100) would write the register after it.
        if let Some(l) = arr.lte.filter(|l| !l.byte) {
            shift_width(g, &bus, c, l);
        }
    }
    // The mode, inside the sensor's hold, after its init and before stream on (unless asked for mid-stream: after the first frame, below).
    if let Some(m) = mode {
        say!(g, "camera: mode A#{}: A#{} x A#{} (x G#{:X} to G#{:X}, y G#{:X} to G#{:X}, increments A#{} A#{}, binning G#{:02X}{:02X}), frame A#{} lines of A#{} clocks, integration A#{} lines, format G#{:X}; the sensor's phase detect and embedded data {}{}\n", s.kind, m.x_out, m.y_out, m.x_start, m.x_end, m.y_start, m.y_end, m.inc[0], m.inc[2], m.binning[0], m.binning[1], m.fll, m.llp, m.coarse, m.format, if m.quiet { "off" } else { "as the init left them" }, if s.flags & F_MID != 0 { "; written after the first frame" } else { "" });
        if s.flags & F_MID == 0 {
            for wr_ in m.writes(arr, c).iter() {
                bus.write(c.addr, wr_).map_err(Fail::Sensor)?;
            }
        }
    }
    // A device test (the hand spawn's fifth byte, `SIDE_TEST`): one side channel off at the sensor in any mode, whole frames too (bit 0 phase detect, bit 1 embedded data), so a whole frame says which of the two the front end's SHORT_VB_FATAL follows.
    let side = SIDE_TEST.load(Ordering::Relaxed);
    if let (true, Some(sd)) = (side != 0, arr.side) {
        let page = [(ccs::SAMSUNG_PAGE >> 8) as u8, ccs::SAMSUNG_PAGE as u8, (ccs::CCS_PAGE >> 8) as u8, ccs::CCS_PAGE as u8];
        bus.write(c.addr, &page).map_err(Fail::Sensor)?;
        bus.write(c.addr, c.hold).map_err(Fail::Sensor)?;
        if let (true, Some(p)) = (side & 1 != 0, sd.pd) {
            bus.write(c.addr, &[(p >> 8) as u8, p as u8, 0, 0]).map_err(Fail::Sensor)?;
        }
        if side & 2 != 0 {
            bus.write(c.addr, &[(sd.emb >> 8) as u8, sd.emb as u8, 0, 0]).map_err(Fail::Sensor)?;
        }
        bus.write(c.addr, c.release).map_err(Fail::Sensor)?;
        say!(g, "camera: side test: phase detect {}, embedded data {}\n", if side & 1 != 0 { "off" } else { "left" }, if side & 2 != 0 { "off" } else { "left" });
    }
    // The job moved into the region, its geometry rewritten to the front end's (`fe`), then placed there: the ring and the program in the region's first frame, the outputs patterned.
    let m_ = job::relocate(&mut jb[..jn], pa, &l).map_err(Fail::Job)?;
    let (list_at, list_len) = job::list_piece(&jb[..jn]).map_err(Fail::Job)?;
    let regeom = if fe != from { geom::rewrite(&mut jb[list_at..list_at + list_len], &from, &fe, scope) } else { 0 };
    // The processing blocks' switches off (ferros_cam geom `DETACH`, `Spec::shape`: a windowed mode's default), so loch0's frame goes to memory alone.
    if detach != 0 {
        let k = geom::detach(&mut jb[list_at..list_at + list_len], detach);
        say!(g, "camera: detach G#{detach:X}: A#{k} program words switched off; the CPU's crossbar words as Linux's close leaves them; the {} program each frame\n", if slim { "slim" } else { "full" });
    }
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
    // With the PD rows in the ring (`pd_off`), also where the program writes loch0's PD path address (`pd_words`): each copy names its buffer past its image.
    let pd_w = if pd_off != 0 { Some(job::pd_words(&jb[list_at..list_at + list_len]).map_err(Fail::Job)?) } else { None };
    if list_len > PROG_STRIDE {
        return Err(Fail::Inputs("the job's program is longer than a copy's room"));
    }
    let ring_off = l.slots[job::OLD_SLOT_IMAGE].0 as usize;
    let buf_pa = |b: usize| pa + (ring_off + b * buf as usize) as u64;
    let (n_full, n_slim) = copies(ring);
    let put = |dst: usize, words: &[u8], lo: usize, hi: usize, pd: Option<(usize, usize)>, b: usize| {
        let pd_pa = buf_pa(b) + pd_off;
        for i in 0..words.len() / 4 {
            let w = if i == lo {
                buf_pa(b) as u32
            } else if i == hi {
                (buf_pa(b) >> 32) as u32
            } else if pd.is_some_and(|p| p.0 == i) {
                pd_pa as u32
            } else if pd.is_some_and(|p| p.1 == i) {
                (pd_pa >> 32) as u32
            } else {
                u32::from_le_bytes(words[i * 4..i * 4 + 4].try_into().unwrap())
            };
            unsafe { core::ptr::write_volatile((dst + i * 4) as *mut u32, w) };
        }
    };
    for b in 0..n_full {
        put(va + long + PROG_AT + b * PROG_STRIDE, &jb[list_at..list_at + list_len], w_lo, w_hi, pd_w, b);
    }
    let slim_at = PROG_AT + n_full * PROG_STRIDE;
    let mut slim_len = 0;
    if slim {
        let mut sl = [0u8; SLIM_STRIDE];
        slim_len = geom::slim(&jb[list_at..list_at + list_len], slim_level, &mut sl).ok_or(Fail::Inputs("the slim program is longer than a copy's room"))?;
        let (s_lo, s_hi) = job::image_words(&sl[..slim_len]).map_err(Fail::Job)?;
        let s_pd = if pd_off != 0 { Some(job::pd_words(&sl[..slim_len]).map_err(Fail::Job)?) } else { None };
        for b in 0..n_slim {
            put(va + long + slim_at + b * SLIM_STRIDE, &sl[..slim_len], s_lo, s_hi, s_pd, b);
        }
    }
    ferros_sys::dma_barrier();
    step!(g, "camera: open step: job placed; the CPU's writes and the engine next\n");
    say!(g, "camera: job placed: A#{} address registers, A#{} high halves (A#{} left as captured), A#{} loads, A#{} descriptors; A#{n_full} full copies of A#{list_len} bytes (the image's words A#{w_lo} and A#{w_hi}), A#{n_slim} slim of A#{slim_len}; A#{regeom} program words of geometry rewritten ({:?}) for A#{} x A#{} at G#{:X} a line\n", m_.addr_regs, m_.highs, m_.highs_left, m_.loads, m_.ring, scope, fe.w, fe.h, fe.stride);
    // The CPU's setup writes before the engine starts, in the capture's order, their geometry the front end's; without them the engine applies the program and the frame writer stays idle (2026-10-20).
    let (mut n, mut cpu_geom) = (0, 0);
    let mut cpu = |a: usize, v: u32| {
        let nv = geom::detached(a as u32, geom::value(a as u32, v, &from, &fe, scope), detach);
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
    CFA_PERIOD.store(if s.full() { ccs::ARRAYS[c.id as usize].period as u64 } else { 2 }, Ordering::Relaxed);
    ROWS.store(fe.h as u64, Ordering::Relaxed);
    COLS.store(fe.w as u64, Ordering::Relaxed);
    LINE.store(fe.stride as u64, Ordering::Relaxed);
    SLIM.store(slim as u64, Ordering::Relaxed);
    PD_ON.store(pd_on as u64, Ordering::Relaxed);
    PD_OFF.store(pd_off, Ordering::Relaxed);
    PD_ROWS.store(if pd_off != 0 { fe.pd_h as u64 } else { 0 }, Ordering::Relaxed);
    SLITS.store(if slit { ring as u64 } else { 0 }, Ordering::Relaxed);
    NOW_FORMAT.store(mode.map_or(10, |m| m.bits() as u64), Ordering::Relaxed);
    for x in [&LATE, &BUSY, &BUSY_MOST, &WRAPS] {
        x.store(0, Ordering::Relaxed);
    }
    for k in 0..ACK_MOST {
        ACK_OR[k].store(0, Ordering::Relaxed);
        ACK_LAST[k].store(0, Ordering::Relaxed);
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
    let want = match RING_TEST.load(Ordering::Relaxed) {
        0 if slit => RING_SLIT,
        0 => RING,
        // WHY the range: the byte is the person's; under 2^5 the ring holds one descriptor (the index would be the engine's place at once), past 2^16 the shift leaves the count register's measured reach (and past 63 it overflows).
        k @ 5..=16 => 1 << k,
        _ => return Err(Fail::Inputs("a ring test count outside 2^5 to 2^16")),
    };
    let mut ring_n = RING_JOB;
    let mut ring_pa = pa;
    for k in [want, RING] {
        wr(g, pd + ferros_cam::PDMA_COUNT, k as u32);
        if rd(g, pd + ferros_cam::PDMA_COUNT) as u64 == k {
            (ring_n, ring_pa) = (k, pa + long as u64);
            break;
        }
    }
    // The ring's descriptors: a sixteenth of the count (`RING`). A slit ring's buffers must divide them, so slot e's buffer (e mod the buffers) is frame e + 1's in every round of the ring.
    let slots = ring_n / STEP as u64;
    if slit && slots % ring as u64 != 0 {
        return Err(Fail::Inputs("the engine's ring holds fewer descriptors than the slit ring has buffers"));
    }
    RING_N.store(ring_n, Ordering::Relaxed);
    DESC_VA.store((va + (ring_pa - pa) as usize) as u64, Ordering::Relaxed);
    // Every descriptor named at once: a slit ring's for good (descriptor e buffer e mod the ring), an image ring's as the frame thread will rewrite each before it is released (`ring_turn`), all naming buffer 0 meanwhile.
    for e in 0..slots {
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
    // Descriptors 1 to the lead named now, as each DMA_DONE names the one after the index (`ring_turn`): those under the lead released with the first at stream on, the one at the lead released at the first DMA_DONE (by the kernel's interrupt or the frame thread, `answer`). Before any frame, so no buffer is found landed (and none counted unseen).
    // WHY the bound: the byte is the person's; a lead past a sixteenth of the ring's descriptors would release the engine's whole ring at once.
    let lead = LEAD.load(Ordering::Relaxed);
    if lead > slots >> 4 {
        return Err(Fail::Inputs("a lead past a sixteenth of the engine's descriptors"));
    }
    if !slit {
        for e in 1..lead + 1 {
            ring_turn(0, Some(e));
        }
        UNSEEN.store(0, Ordering::Relaxed);
    }
    for p in PLACE_LOG.iter() {
        p.store(0, Ordering::Relaxed);
    }
    ferros_sys::dma_barrier();
    step!(g, "camera: open step: the engine starting\n");
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
    step!(g, "camera: open step: engine started; the receiver next\n");
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
    say!(g, "camera: engine started (ring G#{ring_pa:X}, count A#{ring_n}, A#{slots} descriptors round which the index goes{}), receiver on (A#{rx_geom} of its words' geometry rewritten), A#{n} CPU writes (A#{cpu_geom} rewritten), A#{refused} refused\n", if ring_n == want { "" } else { ": the count did not hold the long ring asked for" });
    // The lens to about a metre (the main's measured sharpest over a floor) where focus is offered; elsewhere it stays where it rests (the ultrawide's actuator never answered ferros, the front's range is not measured).
    let focus = if c.focus.1 != 0 { 0x3800 } else { 0 };
    // The sensor's frame timing, for exposures (ferros_cam ccs `Timing`): the mode's line and frame (the init's own where no mode was asked), and the integration's least and margin as the sensor answers them (CCS G#1004 and G#1006), read here, never assumed.
    let page = ccs::Page::of(init);
    let (llp, fll) = match mode {
        Some(m) => (m.llp, m.fll),
        None => (page.get(ccs::LINE_LENGTH_PCK).map_or(c.llp, |v| v as u16), page.get(ccs::FRAME_LENGTH_LINES).map_or((c.exposure_max_lines + ccs::MARGIN as u64) as u16, |v| v as u16)),
    };
    let mut cw = [0u8; 4];
    let read = bus.write_read(c.addr, &ccs::COARSE_CAPS.to_be_bytes(), &mut cw).is_ok();
    let (cmin, margin) = (u16::from_be_bytes([cw[0], cw[1]]), u16::from_be_bytes([cw[2], cw[3]]));
    // WHY the tests: the words are the sensor's; a margin not under the mode's own frame describes none (the main's limit block at G#1100 aliases to G#4140), and a least of 0 is taken as one line, the least SET ever took.
    let sane = read && margin < fll;
    let timing = ccs::Timing { line_ps: ccs::line_ps_for(c, &page), captured_llp: c.llp, llp, fll, coarse_min: if sane && cmin != 0 { cmin } else { 1 }, margin: if sane { margin } else { ccs::MARGIN }, long: sane && arr.margin.is_some(), lte: arr.lte.map(|l| match LTE_MOST.load(Ordering::Relaxed) { 0 => l, m => ccs::Lte { most: m as u8, ..l } }), stretch: false };
    let listed = arr.margin.map_or(alloc::string::String::from("none"), |m| alloc::format!("A#{m}"));
    say!(g, "camera: {}: integration least A#{cmin} and margin A#{margin}{} (the list's margin {listed}); frame A#{fll} lines of A#{llp} clocks, a line A#{} ns (the PLL G#0306 = A#{}), a frame every A#{} us; exposures to A#{} ms{}\n", c.name, if read { "" } else { " (the read failed)" }, timing.line_ps_at(llp) / 1000, page.get(ccs::PLL_MULTIPLIER).unwrap_or(0), timing.frame_ns(&timing.own(1)) / 1000, timing.most_ns() / 1_000_000, if timing.long { "" } else { ", inside the mode's frame" });
    say!(g, "camera: {}: the vendor shift to A#{}{}, the frame counter A#{} bits (the longest exposure at shift A#{}); the readout A#{} us at the mode's line (A#{} rows), unchanged at every exposure (the line is never stretched by default)\n", c.name, timing.lte.map_or(0, |l| l.most), if LTE_MOST.load(Ordering::Relaxed) != 0 { " (the spawn's override, a device test)" } else { "" }, timing.lte.map_or(0, |l| l.frame_bits), timing.lte.map_or(0, |l| timing.shift_most(&l)), timing.readout_ps(&timing.own(1), fe.h as u64) / 1_000_000, fe.h);
    readout_reset();
    if sane && arr.margin.is_some_and(|m| m != margin) {
        say!(g, "camera: {}: the sensor's margin is not the list's: the list's longest exposure is off by the difference\n", c.name);
    }
    let exposure_lines = mode.map_or(c.exposure_lines, |m| m.coarse as u64);
    let now = timing.own(exposure_lines as u16);
    // The gain model from the sensor's own words (owner, 2026-11-17: "the ISO doesn't do anything in lumis"), and the gain the init left (its G#0204 code and G#020E digital gain) in the endpoint's units; with no model, the captured gain is called one (1024), as before.
    let (gain_model, gain) = gain_of(g, &bus, c, init);
    let o = Open { va, pa, l, ring, timing, now, force: 0, gain, focus, asked_at: 0, asked_tick: ferros_sys::now(), captured: fe == from, gain_model, probed: false, view: false, cached: false, inv_sum: 0, inv_max: 0, given: 0, held: None, maps: 0, lat_sum: 0, lat_max: 0, overwritten: 0 };
    NOW_LINES.store(now.coarse as u64, Ordering::Relaxed);
    NOW_NS.store(timing.ns(&now), Ordering::Relaxed);
    FLL.store(now.fll as u64, Ordering::Relaxed);
    LLP.store(now.llp as u64, Ordering::Relaxed);
    PERIOD_NS.store(timing.frame_ns(&now), Ordering::Relaxed);
    PERIOD_PREV_NS.store(0, Ordering::Relaxed);
    PERIOD_AT.store(0, Ordering::Relaxed);
    NOW_GAIN.store(o.gain, Ordering::Relaxed);
    NOW_FOCUS.store(o.focus, Ordering::Relaxed);
    if c.focus.1 != 0 {
        if let Err(e) = focus_init(&bus, c).and_then(|_| focus_set(&bus, c, o.focus)) {
            say!(g, "camera: the lens actuator did not answer (G#{e:X}); it stays where it rests\n");
        }
    }
    FRAMES.store(0, Ordering::Release);
    tl_reset();
    err_reset();
    step!(g, "camera: open step: receiver on, lens set; stream on next\n");
    bus.write(c.addr, c.stream_on).map_err(Fail::Sensor)?;
    // The first descriptor released and the frame thread on (Linux's start writes +G#10 = G#10 right after the engine's start), or the lead's first descriptors (`LEAD`).
    arm(g);
    wr(g, pd + ferros_cam::PDMA_INDEX, STEP * LEAD.load(Ordering::Relaxed) as u32);
    FLIGHT_AT.store(ferros_sys::now(), Ordering::Relaxed);
    FLIGHT_NS.store(PERIOD_NS.load(Ordering::Relaxed), Ordering::Relaxed);
    CUT_AT.store(0, Ordering::Relaxed);
    RUN.store(1, Ordering::Release);
    ferros_sys::wake(&RUN, 1);
    step!(g, "camera: streaming A#{} ms after the open (open step: stream on)\n", ferros_sys::us_since(t0) / 1000);
    // Mid-stream, when asked: the mode written once the first frame has landed (October's order; the sensor took one change a session and then needed a cold boot).
    if let (Some(m), true) = (mode, s.flags & F_MID != 0) {
        let until = late_tick();
        WANT.store(1, Ordering::SeqCst);
        loop {
            let d = DOOR.load(Ordering::SeqCst);
            if FRAMES.load(Ordering::Acquire) != 0 || ferros_sys::now() >= until {
                break;
            }
            ferros_sys::wait(&DOOR, d, until);
        }
        WANT.store(0, Ordering::SeqCst);
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
    link_say(g);
    fe_say(g, "at the close");
    say!(g, "camera: closing after A#{} frames, the ring's index round A#{} times\n", FRAMES.load(Ordering::Relaxed), WRAPS.load(Ordering::Relaxed));
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
    readout_say(g, "at the close");
    img_log_say(g);
    say!(g, "camera: closed after A#{} frames (A#{} answers, A#{} on the line{}); image status bits seen G#{:X}; switch off {}\n", FRAMES.load(Ordering::Relaxed), ANSWERED.load(Ordering::Relaxed), ON_LINE.load(Ordering::Relaxed), if TICKING.load(Ordering::Relaxed) == 1 { ", the rest on the tick" } else { "" }, IMG_SEEN.load(Ordering::Relaxed), if ok { "done" } else { "refused" });
    // The answer's cost: in all, and to its index write (the part the blanking must hold).
    // WHY the max: an open that never streamed has no answers, and its mean is read as 0 rather than divided by zero.
    let us = |t: u64| t * 1_000_000 / ferros_sys::freq();
    let (n, a) = (IDX[0].load(Ordering::Relaxed), ANSWERED.load(Ordering::Relaxed));
    say!(g, "camera: answers A#{a}, A#{} us each on average, A#{} us the most; DMA_DONEs A#{n}, A#{} released by the kernel's interrupt before this thread, the index A#{} us into the answer on average, A#{} us the most; answers that took several frames A#{} (A#{} the most)\n", us(BUSY.load(Ordering::Relaxed)) / a.max(1), us(BUSY_MOST.load(Ordering::Relaxed)), BY_KERNEL.load(Ordering::Relaxed), us(IDX[1].load(Ordering::Relaxed)) / n.max(1), us(IDX[2].load(Ordering::Relaxed)), MULTI.load(Ordering::Relaxed), MULTI_MOST.load(Ordering::Relaxed));
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
    // The mosaic's period (2 Bayer, 4 the quad array of a 1:1 frame: `cfa` then names its 2 x 2 blocks), at +14; a driver from before leaves it 0, which reads as 2.
    h[14] = CFA_PERIOD.load(Ordering::Relaxed) as u8;
    h[10..12].copy_from_slice(&c.black.to_le_bytes());
    h[12..14].copy_from_slice(&c.white.to_le_bytes());
    h[16..24].copy_from_slice(&B_NS[b].load(Ordering::Relaxed).to_le_bytes());
    h[24..28].copy_from_slice(&(B_GAIN[b].load(Ordering::Relaxed) as u32).to_le_bytes());
    h[28..32].copy_from_slice(&(B_FOCUS[b].load(Ordering::Relaxed) as u32).to_le_bytes());
    h[32..40].copy_from_slice(&B_AT[b].load(Ordering::Relaxed).to_le_bytes());
    if bin == 1 {
        h[40] = b as u8;
        h[41] = 1;
        h[44..48].copy_from_slice(&(LINE.load(Ordering::Relaxed) as u32).to_le_bytes());
        h[48..56].copy_from_slice(&(b as u64 * BUF.load(Ordering::Relaxed)).to_le_bytes());
        // The frame's phase-detect rows in the same buffer: their place past its start (u32, 0 none) and their rows (u16), at the image's width and line.
        h[56..60].copy_from_slice(&(PD_OFF.load(Ordering::Relaxed) as u32).to_le_bytes());
        h[60..62].copy_from_slice(&(PD_ROWS.load(Ordering::Relaxed) as u16).to_le_bytes());
    }
}

/// SNAP's reply past the frame's header: the image's physical address, its PD rows' (the same buffer, `PD_OFF` past it), the PD rows, width and line bytes, a status (bit 0: the PD rows' first 64 bytes still the open's pattern, so the PD path never wrote this buffer).
const SNAP_REPLY: usize = HEADER + 32;

/// An image and its phase-detect rows as one pair for the host (owner, 2026-11-17: phase detect as data; docs/CAMERA.md "Phase detect as data"): the next frame's buffer held as a MAP holds it (`HOLD_KIST`: given back by the next SNAP or MAP, by RELEASE, or at the close), its image and its PD rows (every ring buffer carries its frame's, `PD_OFF`) said by physical address, so `ferros-bridge read` pulls both while the holder keeps it (tools/camera-pd-pair.sh). Only with the PD rows in the ring (the init's mode, or the PD path asked) and a ring of two or more.
/// `snap_begin` says whether this open can (and gives back the buffer the kist held); the next frame is then held for it as a MAP's is (`Want::Snap`), and `snap_answer` fills the reply.
fn snap_begin(g: u64, o: &mut Open) -> bool {
    if PD_OFF.load(Ordering::Relaxed) == 0 || o.ring < 2 {
        return false;
    }
    if let Some((b, _)) = o.held {
        release(g, o, b);
    }
    true
}
fn snap_answer(o: &mut Open, b: usize, out: &mut [u8; SNAP_REPLY]) {
    let pd_off = PD_OFF.load(Ordering::Relaxed);
    o.given = B_COUNT[b].load(Ordering::Relaxed);
    o.held = Some((b, probe_read(b)));
    let buf_pa = o.pa + o.l.slots[job::OLD_SLOT_IMAGE].0 + b as u64 * BUF.load(Ordering::Relaxed);
    let pd_first = unsafe { core::ptr::read_volatile((image(o, b) + pd_off as usize) as *const [u64; 8]) };
    header(&mut out[..HEADER], b, 1);
    out[HEADER..HEADER + 8].copy_from_slice(&buf_pa.to_le_bytes());
    out[HEADER + 8..HEADER + 16].copy_from_slice(&(buf_pa + pd_off).to_le_bytes());
    out[HEADER + 16..HEADER + 20].copy_from_slice(&(PD_ROWS.load(Ordering::Relaxed) as u32).to_le_bytes());
    out[HEADER + 20..HEADER + 24].copy_from_slice(&(COLS.load(Ordering::Relaxed) as u32).to_le_bytes());
    out[HEADER + 24..HEADER + 28].copy_from_slice(&(LINE.load(Ordering::Relaxed) as u32).to_le_bytes());
    out[HEADER + 28..HEADER + 32].copy_from_slice(&((pd_first == [PATTERN; 8]) as u32).to_le_bytes());
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
/// An OPEN that asked for the view write-back (`VIEW_CACHED`) gets the 64 bytes too, and 1 at 53 when it was given so (0: non-cacheable, a kernel from before or a slit ring).
const VIEW_REPLY: usize = 48;
const VIEW_REPLY_MODE: usize = 64;
/// The OPEN's view byte (`[1][id][view]`): bit 0 the view.
const VIEW_ASKED: u8 = 1 << 0;
/// The OPEN's byte after the whole mode block (`VIEW_FLAGS`, its seventeenth), bit 0 (2026-11-18, docs/CAMERA.md "A cacheable view"): the view mapped write-back, each buffer's lines cleaned and invalidated by the kernel as MAP hands it over, so the holder reads it thru its caches.
/// There, not in the view byte, so a holder that asks it of a driver from before gets the non-cacheable view (that driver reads no byte past the block) rather than none.
const VIEW_FLAGS: usize = 16;
const VIEW_CACHED: u8 = 1 << 0;
/// A device test only (the hand spawn's ninth input byte 1): a write-back view's buffers handed over by MAP without their maintenance, to show what a reader sees without it (stale lines from the last time round the ring).
static NO_LINES: AtomicU64 = AtomicU64::new(0);

/// Bytes `at` to `at + len` of ring buffer `b` cleaned and invalidated in every core's caches by the kernel (SYS_DMA x1 = 6, process.rs `cam_lines`), for a write-back view (docs/CAMERA.md "A cacheable view"): the counter's ticks it took.
fn lines(g: u64, o: &Open, b: usize, at: usize, len: usize) -> u64 {
    let off = RING_VA.load(Ordering::Relaxed) as usize - o.va + b * BUF.load(Ordering::Relaxed) as usize + at;
    ferros_sys::svc(ferros_sys::nr::DMA, cap(g, S_REGION), 6, off as u64, len as u64, 0)
}

/// The kist's held buffer given back: its hold cleared, and its probe compared with what it read when handed out, which tells whether anything wrote the buffer while it was held (the ring's promise: never; counted, and said the first time).
/// With a write-back view the probe's line is dropped from the caches first: the holder read it thru them, and a non-cacheable read meeting the line a core kept is not promised to see DRAM (the architecture's mismatched attributes), so a write while held could read as none.
fn release(g: u64, o: &mut Open, b: usize) {
    let Some((h, then)) = o.held else { return };
    if h != b {
        return;
    }
    if o.cached {
        lines(g, o, b, probe_off(), 64);
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

/// No frame came in `late_us`: said with the words that tell why.
fn late(g: u64) {
    say!(g, "camera: no frame in A#{} ms (A#{} so far; A#{} answers, A#{} on the line; image status bits seen G#{:X}; ring frames unseen A#{}); closing\n", late_us() / 1000, FRAMES.load(Ordering::Relaxed), ANSWERED.load(Ordering::Relaxed), ON_LINE.load(Ordering::Relaxed), IMG_SEEN.load(Ordering::Relaxed), UNSEEN.load(Ordering::Relaxed));
}

/// The camera's CSI link as it stands, said: at a stream that stopped (`late`) and at every close, so a run says which channels the sensor sent (frame starts per channel at +G#24) and whether the link took them whole (+G#14: G#20 ERR_CRC_PH).
/// Every acknowledged status word's bits seen since the open (or) and in the last answer (last), and the PDMA engine's first 2^4 words as they stand: a stream whose frames begin and never end (a slit's third frame, 2026-11-17 night) says which block held its bits.
fn ack_say(g: u64) {
    // Eight words a line, so a line stays inside the console's 256 bytes.
    for part in cam().ack.chunks(1 << 3).enumerate() {
        let mut l = Line { b: [0; 256], n: 0 };
        let _ = write!(l, "camera: acks (or/last):");
        for (i, &a) in part.1.iter().enumerate() {
            let k = (part.0 << 3) + i;
            let (o, last) = (ACK_OR[k].load(Ordering::Relaxed), ACK_LAST[k].load(Ordering::Relaxed));
            let _ = write!(l, " {:X}:{o:X}/{last:X}", a & 0xFFFFF);
        }
        let _ = write!(l, "\n");
        console(g, &l.b[..l.n]);
    }
    let mut l = Line { b: [0; 256], n: 0 };
    let _ = write!(l, "camera: engine words from +0:");
    for k in 0..1 << 4 {
        let _ = write!(l, " {:X}", rd(g, ferros_cam::PDMA + 4 * k));
    }
    let _ = write!(l, "\n");
    console(g, &l.b[..l.n]);
}
fn link_say(g: u64) {
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

/// How long the stream may run with nobody asking before the holder is taken for gone: 2^22 us (it was 64 frames, 4.4 s at 14.5 a second; a slit runs 64 frames in 53 ms). Never while a call of its waits for a frame (`Pending`): that holder is asking.
const UNASKED_US: u64 = 1 << 22;

// ---------------------------------------------------------------- serving while a frame waits
//
// The server answers every call while a MAP, FRAME, SNAP or SLITS waits for its frame (docs/CAMERA.md "Serving while a frame waits"; the Pro, 2026-11-18: a frame about 13.6 years long held the one server thread in MAP's wait, for a holder already gone, and a new Lumis sat in LIST).
// Such a call is held (`Pending`: its reply token kept, the server back in its loop), and answered when its frame lands, when the holder closes (empty), or when the stream is taken for stopped (empty, then the driver ends, as before).
// One wait covers all three: the doorbell `DOOR`, which the frame thread counts at each frame and the kernel marks when the holder's process ends (`CALLER_ENDED`, kernel SYS_CALLER x1), with the calls on the endpoint ending it too (ferros_sys `wait_or_call`).

/// The server's doorbell: frames counted in its low bits (`door_ring`, by the frame thread), `CALLER_ENDED` set by the kernel when the holder's process ends (the watch made at its OPEN, ferros_sys `caller_watch`).
/// One word for both, so no wake is lost between the server's look and its wait: the kernel's wait returns at once when the word moved since the look, whichever of the two moved it.
static DOOR: AtomicU64 = AtomicU64::new(0);
/// Whether the server holds a call that waits for a frame: only then does the frame thread wake it (`door_ring`), so a server holding none sleeps thru a slit stream's thousand frames a second.
static WANT: AtomicU64 = AtomicU64::new(0);
/// Whether the kernel watches the holder's end (its answer at the OPEN carried `CALLER_WATCHED`); a kernel from before does not, and a second OPEN then asks whether the holder's held call still has a caller (`caller_alive`).
static WATCHED: AtomicU64 = AtomicU64::new(0);

/// A frame landed: the doorbell counted, and the server woken when it waits for one (`WANT`). Order: the count first, then the look at `WANT`, both sequentially consistent, against the server's `WANT` then its look at the count: either the server's look sees this frame, or this sees its `WANT` and wakes it.
fn door_ring() {
    DOOR.fetch_add(1, Ordering::SeqCst);
    if WANT.load(Ordering::SeqCst) != 0 {
        ferros_sys::wake(&DOOR, 1);
    }
}

/// The frame in flight: the counter at its start (the DMA_DONE that ended the one before, the stream on for the first) and its period as written before it began (ns), for `cut_due`.
static FLIGHT_AT: AtomicU64 = AtomicU64::new(0);
static FLIGHT_NS: AtomicU64 = AtomicU64::new(0);
/// The last cut (`exposure_cut`): the counter when the stream came back on (0 none since) and the frames counted then, so the first frame after it is said with its time.
static CUT_AT: AtomicU64 = AtomicU64::new(0);
static CUT_N: AtomicU64 = AtomicU64::new(0);
/// How much sooner than the frame in flight a new exposure's frame must end for SET to cut that frame (`cut_due`): 2^20 us. A cut drops the frame in flight and costs a stream off and on; under about a second of difference the frame in flight is simply let finish, as every exposure SET did before, and a slit's or a short frame's change never cuts.
const CUT_US: u64 = 1 << 20;

/// What a held call waits for: MAP the newest frame past the last it handed out, FRAME the next frame at its bin, SNAP the next frame with its PD rows, SLITS a slit past its count.
#[derive(Clone, Copy)]
enum Want {
    Map,
    Frame(usize),
    Snap,
    Slits,
}

/// A call held while its frame is on its way: its reply token (SYS_REPLY's, kept until answered), what it waits for, the count its frame must be past, and the counter at which the stream is taken for stopped (`late_tick` at the call, moved later by an exposure that lengthens the frame).
struct Pending {
    token: [u8; 32],
    want: Want,
    after: u64,
    until: u64,
}

/// What became of a held call when looked at.
enum Tried {
    Waiting,
    Answered,
    /// Its reply failed: its caller has ended.
    Gone,
}

/// Binning's measures for the console, every 64 frames served by copy.
struct Served {
    n: u64,
    bin_us: u64,
    since: u64,
}

/// A MAP's frame, buffer `b` held for the kist already (`hold_newest`): its record made, a write-back view's lines dropped, its header the reply. False when the reply failed (the holder has ended).
fn map_answer(g: u64, o: &mut Open, b: usize, token: &[u8; 32]) -> bool {
    let n = FRAMES.load(Ordering::Acquire);
    o.asked_at = n;
    o.given = B_COUNT[b].load(Ordering::Relaxed);
    // A write-back view: the buffer's lines dropped from every core's caches before the holder hears of it (docs/CAMERA.md "A cacheable view").
    // Its frame's DMA_DONE came before the hold (`ring_turn` makes a buffer the newest only then), and the front end writes it again only after the holder gives it back (`ring_turn` never names a held buffer), so what a core kept from the last time round the ring, or fetched by speculation while the front end wrote it, is gone and the holder reads this frame.
    if o.cached && NO_LINES.load(Ordering::Relaxed) == 0 {
        let t = lines(g, o, b, 0, BUF.load(Ordering::Relaxed) as usize);
        o.inv_sum += t;
        o.inv_max = o.inv_max.max(t);
    }
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
        if o.cached {
            say!(g, "camera: the write-back view's maintenance at MAP (A#{} bytes a buffer): A#{} us on average, A#{} us at most\n", BUF.load(Ordering::Relaxed), us(o.inv_sum / 64), us(o.inv_max));
        }
        (o.lat_sum, o.lat_max, o.inv_sum, o.inv_max) = (0, 0, 0, 0);
    }
    reply(token, &h) != ERR
}

/// A FRAME's frame, buffer `b` held for the binning already (`hold_newest`): binned into `OUT` and given back to the ring, the reply the binned frame. False when the reply failed (the holder has ended).
fn frame_answer(g: u64, o: &mut Open, bin: usize, b: usize, token: &[u8; 32], s: &mut Served) -> bool {
    let n = FRAMES.load(Ordering::Acquire);
    o.asked_at = n;
    if n >= 8 && !o.probed {
        o.probed = true;
        probe(g, o);
    }
    let t = ferros_sys::now();
    let len = bin_frame(o, bin, b);
    unhold(HOLD_BIN, b);
    s.bin_us += ferros_sys::us_since(t);
    s.n += 1;
    if s.n % 64 == 0 {
        let us = ferros_sys::us_since(s.since);
        let copy_us = COPY_TICKS.swap(0, Ordering::Relaxed) * 1_000_000 / ferros_sys::freq();
        say!(g, "camera: A#{} frames served, the last 64 in A#{} ms (A#{} us apart), binning A#{} us each on A#{BINNERS} threads (copying out of the region A#{} us of their time); A#{} taken by the front end\n", s.n, us / 1000, us / 64, s.bin_us / 64, copy_us / 64, n);
        (s.bin_us, s.since) = (0, ferros_sys::now());
    }
    reply(token, unsafe { &(&*(&raw const OUT))[..len] }) != ERR
}

/// The held call looked at: answered when its frame has landed (the newest buffer past its count held for it with one compare-and-swap against the frame thread's turn, `hold_newest`, so a buffer is never both held and given to the engine).
fn try_pending(g: u64, o: &mut Open, p: &Pending, s: &mut Served) -> Tried {
    let ok = match p.want {
        Want::Slits => {
            if FRAMES.load(Ordering::Acquire) <= p.after {
                return Tried::Waiting;
            }
            reply(&p.token, &slits());
            true
        }
        Want::Map => {
            let Some(b) = hold_newest(HOLD_KIST, p.after) else { return Tried::Waiting };
            map_answer(g, o, b, &p.token)
        }
        Want::Frame(bin) => {
            let Some(b) = hold_newest(HOLD_BIN, p.after) else { return Tried::Waiting };
            frame_answer(g, o, bin, b, &p.token, s)
        }
        Want::Snap => {
            let Some(b) = hold_newest(HOLD_KIST, p.after) else { return Tried::Waiting };
            let mut out = [0u8; SNAP_REPLY];
            snap_answer(o, b, &mut out);
            let w = |k: usize| u64::from_le_bytes(out[HEADER + k..HEADER + k + 8].try_into().unwrap());
            say!(g, "camera: SNAP frame A#{}: image at G#{:X} (A#{} x A#{} RAW10, G#{:X} a line), its PD rows at G#{:X} (A#{} rows), status G#{:X} (1 the PD rows never written); held as a MAP holds\n", u32::from_le_bytes(out[0..4].try_into().unwrap()), w(0), COLS.load(Ordering::Relaxed), ROWS.load(Ordering::Relaxed), LINE.load(Ordering::Relaxed), w(8), w(16) as u32, (w(24) >> 32) as u32);
            reply(&p.token, &out);
            true
        }
    };
    if ok { Tried::Answered } else { Tried::Gone }
}

/// Whether an exposure whose frame lasts `new_ns` should cut the frame in flight (`exposure_cut`): its frame, begun now, would end more than `CUT_US` before the frame in flight does (the coordinator, 2026-11-18, for Lumis: dragging the shutter to 14 years and back must come back at once, not after the 14-year frame).
/// Never with a slit ring (the open program's armed release and the slits' records follow every slit), and never before the stream's first frame has a start (`FLIGHT_AT` 0).
fn cut_due(new_ns: u64) -> bool {
    let at = FLIGHT_AT.load(Ordering::Relaxed);
    if SLITS.load(Ordering::Relaxed) != 0 || at == 0 {
        return false;
    }
    ccs::Timing::cut_due(at, FLIGHT_NS.load(Ordering::Relaxed), ferros_sys::now(), new_ns, ferros_sys::freq(), CUT_US)
}
/// How long a FRAME waits for its frame before the stream is taken for stopped, past the frame period itself (`late_us`): about thirty of the main's ordinary frames, past the sensor's start after stream on.
const FRAME_LATE_US: u64 = 1 << 21;

/// The longest frame period that may be on its way now (ns): the timing's own, and for two frames after a change of it also the one before (`PERIOD_AT`), summed, which bounds the longer of the two.
fn period_ns() -> u64 {
    let now = PERIOD_NS.load(Ordering::Relaxed);
    if FRAMES.load(Ordering::Acquire) < PERIOD_AT.load(Ordering::Relaxed) + 2 { now + PERIOD_PREV_NS.load(Ordering::Relaxed) } else { now }
}

/// How long a wait for the next frame may take before the stream is taken for stopped (us): `FRAME_LATE_US` past two frame periods, so a frame of 16 s is waited for (it was 2^21 us flat until 2026-11-17, which closed any frame longer than about two seconds).
fn late_us() -> u64 {
    FRAME_LATE_US + (period_ns() << 1) / 1000
}
/// The counter's tick at `late_us` from now, in 128 bits: past about 8.7 days of us the product with the counter's 24.576 MHz passes 64 bits (the main's widest shift has 14-year frames); the ticks themselves fit.
fn late_tick() -> u64 {
    ferros_sys::now() + (ferros_sys::freq() as u128 * late_us() as u128 / 1_000_000) as u64
}

/// The sensor's frame period becomes `ns` (an open's mode, an exposure that moved the frame).
fn period_set(ns: u64) {
    PERIOD_PREV_NS.store(PERIOD_NS.swap(ns, Ordering::Relaxed), Ordering::Relaxed);
    PERIOD_AT.store(FRAMES.load(Ordering::Acquire), Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(g: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    // Spawned by hand with the input `01` (a test of a driver not yet stored): it stays between calls with nothing open, so a holder's LIST and the OPEN after it reach this driver, not the vault's (2026-11-17: a hand-spawned driver ended after camtest's LIST, and the OPEN started the stored one). It still ends at a CLOSE.
    let stay = input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 1;
    // The second input byte, for a device test only: the most vendor shift to try (`LTE_MOST`; 0 the table's).
    if input_len > 1 {
        LTE_MOST.store(unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 1) as *const u8) } as u64, Ordering::Relaxed);
    }
    if input_len > 2 {
        MEASURE.store(unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 2) as *const u8) } as u64, Ordering::Relaxed);
    }
    // The fourth, a device test of the ring's wrap only: log2 of the count to ask (`RING_TEST`; 0 the mode's).
    if input_len > 3 {
        RING_TEST.store(unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 3) as *const u8) } as u64, Ordering::Relaxed);
    }
    if input_len > 4 {
        SIDE_TEST.store(unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 4) as *const u8) } as u64, Ordering::Relaxed);
    }
    if input_len > 5 {
        // WHY the max: 0 means the default lead of one, as the other test bytes' 0 means the default.
        LEAD.store((unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 5) as *const u8) } as u64).max(1), Ordering::Relaxed);
    }
    if input_len > 6 {
        LATE_TEST.store(unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 6) as *const u8) } as u64, Ordering::Relaxed);
    }
    if input_len > 7 {
        DETACH.store(unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 7) as *const u8) } as u64, Ordering::Relaxed);
    }
    if input_len > 8 {
        NO_LINES.store(unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 8) as *const u8) } as u64, Ordering::Relaxed);
    }
    if input_len > 9 {
        NO_OPEN.store(unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 9) as *const u8) } as u64, Ordering::Relaxed);
    }
    // A device test with nothing stored: the 1:1 init in the input after the 16 knob bytes (`FULL_INPUT_AT`), u32 LE length and the table, as `--input-file` frames it.
    if input_len > FULL_INPUT_AT + 4 {
        let at = ferros_sys::arc_base() + input + FULL_INPUT_AT;
        let n = u32::from_le_bytes(core::array::from_fn(|k| unsafe { core::ptr::read_volatile((at + k as u64) as *const u8) })) as u64;
        // WHY the tests: the length is the spawn's bytes; one that does not end exactly at the input's end is not the framing asked for, and one past `FULL` would write past it.
        if FULL_INPUT_AT + 4 + n == input_len && n as usize <= VALUE {
            let full = unsafe { &mut *(&raw mut FULL) };
            for k in 0..n as usize {
                full[k] = unsafe { core::ptr::read_volatile((at + 4 + k as u64) as *const u8) };
            }
            FULL_INPUT.store(n, Ordering::Relaxed);
            say!(g, "camera: the 1:1 init from the spawn's input, A#{n} bytes (a device test: used in place of the vault's cam.<key>.init.full)\n");
        } else {
            say!(g, "camera: the spawn's input past its knobs is not a framed 1:1 init (A#{} bytes); ignored\n", input_len - FULL_INPUT_AT);
        }
    }
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_BYTES) };
    ferros_sys::thread_spawn(frame_thread, unsafe { &mut *(&raw mut FRAME_STACK) }, g);
    for k in 1..BINNERS {
        ferros_sys::thread_spawn(binner, unsafe { &mut (*(&raw mut BIN_STACKS))[k - 1] }, k as u64);
    }
    let mut msg = [0u8; 64];
    let mut token = [0u8; 32];
    let mut open_cam: Option<Open> = None;
    // The call held while its frame is on its way (`Pending`): at most one; a second call that would wait while one is held is answered empty (one holder, one thread reading frames).
    let mut pending: Option<Pending> = None;
    // A driver spawned to stay that has closed its one stream (`stay`): it refuses the next open.
    let mut closed_once = false;
    // Binning's measures, for the console every 64 frames served.
    let mut served = Served { n: 0, bin_us: 0, since: ferros_sys::now() };
    // The first receive waits (the call that started this driver is queued already; one spawned by hand waits for its first); after it, a look, so a driver with nothing open and nothing waiting ends.
    let mut poll = false;
    loop {
        // The doorbell read before anything is looked at: a frame or the holder's end after this read makes the wait below return at once (`DOOR`).
        WANT.store(pending.is_some() as u64, Ordering::SeqCst);
        let door = DOOR.load(Ordering::SeqCst);
        // The holder's process has ended (the kernel's watch, `CALLER_ENDED`): its stream closed now, whatever it waits for, its held call (whose caller is gone with it) let go.
        if open_cam.is_some() && door & ferros_sys::CALLER_ENDED != 0 {
            say!(g, "camera: its holder's process has ended (the kernel's watch); closing\n");
            close(g);
            if let Some(p) = pending.take() {
                reply(&p.token, &[]);
            }
            open_cam = None;
            if !stay {
                ferros_sys::exit(0);
            }
            closed_once = true;
            continue;
        }
        // The held call looked at: answered once its frame has landed; its reply failing is its holder gone; past its deadline the stream is taken for stopped, as before.
        if let (Some(p), Some(o)) = (pending.as_ref(), open_cam.as_mut()) {
            match try_pending(g, o, p, &mut served) {
                Tried::Answered => {
                    pending = None;
                    o.asked_tick = ferros_sys::now();
                    if CUT_AT.load(Ordering::Relaxed) != 0 && FRAMES.load(Ordering::Acquire) > CUT_N.load(Ordering::Relaxed) {
                        say!(g, "camera: the first frame after the cut landed A#{} ms after the stream came back on\n", ferros_sys::us_since(CUT_AT.swap(0, Ordering::Relaxed)) / 1000);
                    }
                    continue;
                }
                Tried::Gone => {
                    say!(g, "camera: a frame's reply failed; its holder is gone\n");
                    close(g);
                    ferros_sys::exit(0);
                }
                Tried::Waiting if ferros_sys::now() >= p.until => {
                    late(g);
                    close(g);
                    reply(&p.token, &[]);
                    ferros_sys::exit(0);
                }
                Tried::Waiting => {}
            }
        }
        let got = recv(cap(g, S_SERVE), &mut msg, &mut token, poll);
        poll = true;
        if got == NOTHING {
            match (&open_cam, &pending) {
                // Nothing open and nothing waiting: end (a call after this look starts another driver, process.rs `ON_DEMAND`), unless spawned to stay: then the next receive waits.
                (None, _) if stay => {
                    poll = false;
                    continue;
                }
                (None, _) => ferros_sys::exit(0),
                // A call held for its frame: wait for a frame, a call or the holder's end (all three end this wait), until the held call's deadline.
                (Some(_), Some(p)) => {
                    ferros_sys::wait_or_call(&DOOR, door, p.until);
                    continue;
                }
                (Some(o), None) => {
                    // Open, nothing held: wait for a call or the holder's end (frames do not ring the doorbell now, `WANT`) until the holder's deadline. A holder gone quiet past `UNASKED_US` is gone.
                    let until = o.asked_tick + ferros_sys::freq() * UNASKED_US / 1_000_000;
                    if ferros_sys::now() >= until {
                        say!(g, "camera: A#{} ms with nobody asking; its holder is gone\n", UNASKED_US / 1000);
                        close(g);
                        ferros_sys::exit(0);
                    }
                    ferros_sys::wait_world_or_call(&DOOR, door, until);
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
        // A call that would wait for a frame while one is held already: answered empty (MAP, FRAME, SNAP and SLITS come from the holder's one reading thread).
        if pending.is_some() && matches!(msg[0], 2 | 5 | 9 | 12) {
            say!(g, "camera: a second call waiting for a frame (op A#{}) while one is held: answered empty\n", msg[0]);
            reply(&token, &[]);
            continue;
        }
        match msg[0] {
            // LIST: the cameras that can stream whose inputs and IDT are in the vault (`list`), so a camera shows once its init, program, job and calibration are stored on this device. Answered while a frame waits too.
            0 => {
                reply(&token, &list(g));
            }
            1 => {
                let c = if got >= 2 { cams::by_id(msg[1]).filter(|c| c.streams) } else { None };
                // Held by a holder whose process has ended, on a kernel that does not watch ends (`WATCHED` 0): its held call's caller is gone (`caller_alive`), so its stream is closed now and this open is answered 3, to be asked of a driver started fresh (the frame thread here ended with that stream).
                if let (true, Some(p)) = (open_cam.is_some() && WATCHED.load(Ordering::Relaxed) == 0, pending.as_ref()) {
                    if ferros_sys::caller_alive(&p.token) == Some(false) {
                        say!(g, "camera: the holder's held call has no caller (its process ended); closing; this open is answered 3, to be asked again\n");
                        close(g);
                        pending = None;
                        open_cam = None;
                        reply(&token, &[3]);
                        if !stay {
                            ferros_sys::exit(0);
                        }
                        closed_once = true;
                        continue;
                    }
                }
                let r = if c.is_none() {
                    1
                } else if open_cam.is_some() {
                    2
                } else if closed_once {
                    // A hand-spawned driver that stays: its frame thread ended with its first stream, so a second open is refused here rather than reaching the vault's driver (cambench's sweeps fell thru to it, 2026-11-18); a sweep spawns a driver a step (tools/cambench-sweep.sh).
                    say!(g, "camera: this hand-spawned driver has closed its one stream; spawn another for the next open\n");
                    4
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
                    let asked = got >= 3 && msg[2] & VIEW_ASKED != 0;
                    if asked && o.ring < VIEW_LEAST {
                        say!(g, "camera: no view: the pool gave a ring of one buffer; frames by copy only\n");
                    } else if asked {
                        // The image slot whole: a ring of whole frames, or a slit ring rounded up to whole frames of the pool (ferros_cam job `with_image`).
                        let first = o.l.slots[job::OLD_SLOT_IMAGE].0 / job::FRAME;
                        let n = o.l.slots[job::OLD_SLOT_IMAGE].1 / job::FRAME;
                        let mut t = [0u8; 32];
                        // Write-back when asked, but never for a slit ring: its buffers have no holds, so no hand-over at which to make them right (SLITS).
                        let want = got as usize > VIEW_FLAGS && msg[VIEW_FLAGS] & VIEW_CACHED != 0 && SLITS.load(Ordering::Relaxed) == 0;
                        let mut give = |cached: bool| ferros_sys::svc(ferros_sys::nr::DMA, cap(g, S_REGION), 4, ferros_sys::off(token.as_ptr()), first | n << 32 | (cached as u64) << 63, ferros_sys::off(t.as_mut_ptr()));
                        let mut base = give(want);
                        o.cached = want && base != ERR;
                        // A kernel from before the write-back view refuses the bit (it reads as frames past the run): the view as before.
                        if want && base == ERR {
                            say!(g, "camera: the kernel gave no write-back view; asking for the non-cacheable one\n");
                            base = give(false);
                        }
                        if base != ERR {
                            o.view = true;
                            out[53] = o.cached as u8;
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
                            len = if got > 3 || want { VIEW_REPLY_MODE } else { VIEW_REPLY };
                            t.fill(0);
                            say!(g, "camera: the view of A#{} buffers given to the holder\n", o.ring);
                        } else {
                            say!(g, "camera: the kernel gave no view (its console line says why); frames by copy only\n");
                        }
                    }
                }
                // The holder's end watched from now (kernel SYS_CALLER x1, ferros_sys `caller_watch`): the kernel marks `DOOR` the moment its process ends, any way, and the stream closes then, not after `UNASKED_US` of silence nor when a 14-year frame's reply fails.
                // Before the reply: the token is the proof the kernel asks for, and the reply spends it.
                let mut watch = ferros_sys::Watch::Unknown;
                if r == 0 {
                    watch = ferros_sys::caller_watch(&token, &DOOR);
                    WATCHED.store(matches!(watch, ferros_sys::Watch::Watched(_)) as u64, Ordering::Relaxed);
                    if !matches!(watch, ferros_sys::Watch::Watched(_)) {
                        say!(g, "camera: the kernel does not watch the holder's end ({watch:?}); a holder gone is found by its reply failing, by another's open, or after A#{} ms of silence\n", UNASKED_US / 1000);
                    }
                }
                // Its reply failing (or the kernel finding no caller behind the token) is the holder gone already: the stream it opened closed at once.
                let failed = reply(&token, &out[..len]) == ERR;
                if r == 0 && (failed || watch == ferros_sys::Watch::Gone) {
                    say!(g, "camera: the open's reply failed; its holder is gone; closing\n");
                    close(g);
                    ferros_sys::exit(0);
                }
                if r == 3 {
                    ferros_sys::exit(0);
                }
            }
            // FRAME `[2][bin]`: the next frame by copy, binned; held until it lands (`Want::Frame`).
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
                // The next frame: one counted after this request came, its buffer held while it is binned (the newest, `hold_newest`), so the engine is never given it meanwhile.
                pending = Some(Pending { token, want: Want::Frame(bin), after: FRAMES.load(Ordering::Acquire), until: late_tick() });
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
                    // Exposure (ns): the plan the sensor's timing makes of it (ferros_cam ccs `Timing::plan`: inside the mode's frame, else the frame lengthened, else the shift), refused past the longest, never capped. Answered while a frame waits too (docs/CAMERA.md "Serving while a frame waits"): it lands at the next frame's start (the hold), or, when the frame in flight would end more than `CUT_US` after the new one would, that frame is cut (`exposure_cut`).
                    0 => match if o.force != 0 { o.timing.plan_shift(value, o.force) } else { o.timing.plan(value) } {
                        Some(p) => {
                            let moved = (p.llp, p.fll, p.shift) != (o.now.llp, o.now.fll, o.now.shift);
                            let new_ns = o.timing.frame_ns(&p);
                            let cut = moved && cut_due(new_ns);
                            match if cut { exposure_cut(&bus, c, &o.timing, &o.now, &p) } else { exposure_set(&bus, c, &o.timing, &o.now, &p) } {
                                Ok(()) => {
                                    if moved {
                                        readout_say(g, "before");
                                        readout_reset();
                                        period_set(new_ns);
                                        say!(g, "camera: exposure A#{} us: frame A#{} lines of A#{} clocks, shift A#{}, integration A#{} lines; a frame every A#{} ms\n", o.timing.ns(&p) / 1000, p.fll, p.llp, p.shift, p.coarse, new_ns / 1_000_000);
                                        // A frame that lengthens moves a held call's deadline with it (`late_tick` counts the frame before and this one for two frames); one that shortens leaves it, as the frame in flight may still be the old one.
                                        if let Some(h) = pending.as_mut() {
                                            h.until = h.until.max(late_tick());
                                        }
                                    }
                                    if cut {
                                        // The frame begun now, at the new timing; the first frame after it is said when it lands.
                                        let t = ferros_sys::now();
                                        let left_ms = FLIGHT_AT.load(Ordering::Relaxed) as u128 * 1000 / ferros_sys::freq() as u128 + FLIGHT_NS.load(Ordering::Relaxed) as u128 / 1_000_000 - t as u128 * 1000 / ferros_sys::freq() as u128;
                                        FLIGHT_AT.store(t, Ordering::Relaxed);
                                        FLIGHT_NS.store(new_ns, Ordering::Relaxed);
                                        CUT_N.store(FRAMES.load(Ordering::Acquire), Ordering::Relaxed);
                                        CUT_AT.store(t, Ordering::Relaxed);
                                        say!(g, "camera: the frame in flight cut with about A#{left_ms} ms of it left: frame truncation (CCS G#0106), stream off, the new timing, stream on; the next frame due in A#{} ms\n", new_ns / 1_000_000);
                                    }
                                    o.now = p;
                                    NOW_LINES.store((p.coarse as u64) << p.shift, Ordering::Relaxed);
                                    NOW_NS.store(o.timing.ns(&p), Ordering::Relaxed);
                                    FLL.store(p.fll as u64, Ordering::Relaxed);
                                    LLP.store(p.llp as u64, Ordering::Relaxed);
                                    0
                                }
                                Err(e) => {
                                    // Some of the hold's writes may have landed: the line and frame are no longer known, so the next exposure writes both.
                                    o.now.llp = 0;
                                    o.now.fll = 0;
                                    say!(g, "camera: an exposure's writes failed (G#{e:X}){}\n", if cut { " in a cut (the stream on tried last)" } else { "" });
                                    1
                                }
                            }
                        }
                        None => 1,
                    },
                    // Gain (1024 one): the analogue code the sensor's own model gives, digital gain written as one (owner, 2026-11-17: digital gain is after the ADC, so analogue only); past the analogue's most refused, never capped; the frame's header then carries the gain the code makes. With no model, the captured gain alone (1024).
                    1 => match o.gain_model.and_then(|m| m.split(value)) {
                        Some(s) if gain_set(&bus, c, &s).is_ok() => {
                            o.gain = s.gain;
                            NOW_GAIN.store(s.gain, Ordering::Relaxed);
                            0
                        }
                        None if o.gain_model.is_none() && value == ccs::UNITY => 0,
                        _ => 1,
                    },
                    // A device test (owner, 2026-11-17: "no artificial exposure cap anywhere"): the shift every following exposure takes (0: the least that fits), to find the shift field's reach.
                    3 => {
                        o.force = value as u8;
                        0
                    }
                    2 if c.focus.1 != 0 && (c.focus.0..=c.focus.1).contains(&value) && focus_set(&bus, c, value).is_ok() => {
                        o.focus = value;
                        NOW_FOCUS.store(value, Ordering::Relaxed);
                        0
                    }
                    _ => 1,
                };
                reply(&token, &[r]);
            }
            // CLOSE `[4]`: the stream ended thru its own close (the sensor's stream off, the front end's close, the module down) whatever frame is in flight, a held call answered empty (the holder reads it as closed), then `[0]`.
            4 => {
                if open_cam.is_some() {
                    close(g);
                }
                if let Some(p) = pending.take() {
                    reply(&p.token, &[]);
                }
                reply(&token, &[0]);
                // Spawned to stay: it stays, closed, so the calls after it (a sweep's next OPEN) reach this driver and are refused, never the vault's.
                if !stay {
                    ferros_sys::exit(0);
                }
                open_cam = None;
                closed_once = true;
            }
            // MAP `[5][release]`: the buffer `release` names given back (G#FF none), then the newest written frame past the last one handed out, held until it is given back (here, or by RELEASE, or at the close): the reply its header (`header`, bin 1: the buffer at +40, its place in the view at +48), the samples read in place thru the view. Held until that frame lands (`Want::Map`).
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
                pending = Some(Pending { token, want: Want::Map, after: o.given, until: late_tick() });
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
            // SLITS `[9][after: u32]`: a slit ring's frames past `after` (`slits`), held until one lands (`Want::Slits`).
            9 => {
                if open_cam.is_none() || SLITS.load(Ordering::Relaxed) == 0 {
                    reply(&token, &[]);
                    continue;
                }
                let after = if got >= 5 { u32::from_le_bytes(msg[1..5].try_into().unwrap()) as u64 } else { 0 };
                pending = Some(Pending { token, want: Want::Slits, after, until: late_tick() });
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
            // SNAP `[12]`: the next frame held with its phase-detect rows, as a MAP holds it (`snap_begin`, `Want::Snap`): the frame's header (bin 1), then both physical addresses and the PD rows' shape; empty when this open cannot (no PD rows in the ring, a ring of one).
            12 => {
                let Some(o) = open_cam.as_mut() else {
                    reply(&token, &[]);
                    continue;
                };
                if !snap_begin(g, o) {
                    say!(g, "camera: SNAP refused (the PD rows in the ring and a ring of two or more only)\n");
                    reply(&token, &[]);
                    continue;
                }
                pending = Some(Pending { token, want: Want::Snap, after: FRAMES.load(Ordering::Acquire), until: late_tick() });
            }
            _ => {
                reply(&token, &[]);
            }
        }
        // The holder was answered just now: its quiet time (`UNASKED_US`) counts from here, not from its call, which a long exposure's frame kept waiting (a MAP answered after a 16 s frame would otherwise find the holder "gone" at once).
        if let Some(o) = open_cam.as_mut() {
            o.asked_tick = ferros_sys::now();
        }
    }
}

// ---------------------------------------------------------------- the bench's ops

/// STATS' reply: little-endian u64s, in order: frames the front end wrote (DMA_DONE), answers, answers on the line, the frame thread's ticks answering and the most in one answer, slit frames not where the ring put them, ring frames whose buffer was not found, the engine ring's count (bytes, a sixteenth of it descriptors) and how many times the index went round it, the counter now and its frequency, register accesses refused, the image status bits seen, the frame's rows, width, line bytes and a buffer's bytes, the slit ring's buffers (0 none), the mode's frame and line length, the integration (lines), bits a sample, whether the slim program runs, whether the PD path is on, whether the frame thread answers on its tick, when the open began (the counter).
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
        WRAPS.load(Ordering::Relaxed),
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

/// SLITS' reply, the drain (docs/CAMERA.md "The ring drained"): little-endian u64s: the newest frame's count, the ring's buffers, a buffer's bytes, the rows, a line's bytes, the width, frames not where the ring put them, how many times the engine's index went round its ring. Frame n is in buffer (n - 1) mod the ring, at n's buffer times its bytes in the view; its record (`slit_turn`: count, counter, Eagle time, probe moved) right after the rows. The two oldest buffers are the next the engine writes and the one whose probe is patterned for the frame after (the kernel may release the next descriptor before this driver answers), so a reader keeps to the newest ring - 2, and a slit whose record reads another count after its rows were copied was written over while read.
fn slits() -> [u8; 64] {
    let w: [u64; 8] = [
        FRAMES.load(Ordering::Acquire),
        SLITS.load(Ordering::Relaxed),
        BUF.load(Ordering::Relaxed),
        ROWS.load(Ordering::Relaxed),
        LINE.load(Ordering::Relaxed),
        COLS.load(Ordering::Relaxed),
        LATE.load(Ordering::Relaxed),
        WRAPS.load(Ordering::Relaxed),
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

/// CAPS: the sensor's registers read back, each as it answers (zeros where the read failed), at fixed places: +0 its data format model type and subtype (G#00C0, two bytes), +2 its sixteen data format descriptors (G#00C2, 32 bytes: (uncompressed bits << 8) | compressed bits each), +34 the integration's least and its margin (G#1004, four bytes), +38 the frame and line length (G#0340, four), +42 the window and output size (G#0344, twelve), +54 the increments (G#0380, eight), +62 the binning (G#0900, three), +65 the data format (G#0112, two), +67 the integration (G#0202, two), +69 mode select (G#0100, one), +70 the frame count (G#0005, one), +71 two bytes, a bit per read that failed (bit 0 the first); since 2026-11-17 also +73 the analogue gain capability (G#0080, twenty bytes), +93 the digital gain limits (G#1084, six), +99 the gain now (G#0204 and G#020E, four); since 2026-11-18 also +103 the vendor shift words (G#0702 and G#0704, four: the main's `Lte`).
fn caps(g: u64) -> [u8; 107] {
    let c = cam();
    let bus = Bus::of(g, c);
    let mut out = [0u8; 107];
    let mut failed = 0u16;
    let reads: [(u16, usize, usize); 16] = [(0x00C0, 0, 2), (0x00C2, 2, 32), (0x1004, 34, 4), (0x0340, 38, 4), (0x0344, 42, 12), (0x0380, 54, 8), (0x0900, 62, 3), (0x0112, 65, 2), (0x0202, 67, 2), (0x0100, 69, 1), (0x0005, 70, 1), (0x0080, 73, 20), (0x1084, 93, 6), (0x0204, 99, 2), (0x020E, 101, 2), (0x0702, 103, 4)];
    for (k, &(a, at, n)) in reads.iter().enumerate() {
        if bus.write_read(c.addr, &[(a >> 8) as u8, a as u8], &mut out[at..at + n]).is_err() {
            failed |= 1 << k;
        }
    }
    out[71..73].copy_from_slice(&failed.to_le_bytes());
    out
}

/// The vendor shift field's width, a device test (`MEASURE` 2, the hand spawn's third byte): after the init and before stream on, so no frame runs at a written shift, each word is written into both shift registers inside the hold and read back; the bits the sensor keeps are the field. Both go back to 0 at the end.
/// The coordinator, 2026-11-17: shifts 21 to 23 ran (frames 105.66 s apart at 23), so the field is wider than the 6 Android used; the main's most is the widest it keeps.
fn shift_width(g: u64, bus: &Bus, c: &Cam, l: ccs::Lte) {
    let page = [(ccs::SAMSUNG_PAGE >> 8) as u8, ccs::SAMSUNG_PAGE as u8, (ccs::CCS_PAGE >> 8) as u8, ccs::CCS_PAGE as u8];
    for word in [0x1800u16, 0x1F00, 0x2000, 0x3F00, 0x4000, 0x7F00, 0xFF00, 0xFFFF, 0] {
        let mut ok = bus.write(c.addr, &page).is_ok() && bus.write(c.addr, c.hold).is_ok();
        for r in [l.fll_reg, l.coarse_reg] {
            ok &= bus.write(c.addr, &[(r >> 8) as u8, r as u8, (word >> 8) as u8, word as u8]).is_ok();
        }
        ok &= bus.write(c.addr, c.release).is_ok();
        let (mut a, mut b) = ([0u8; 2], [0u8; 2]);
        ok &= bus.write_read(c.addr, &l.fll_reg.to_be_bytes(), &mut a).is_ok() && bus.write_read(c.addr, &l.coarse_reg.to_be_bytes(), &mut b).is_ok();
        say!(g, "camera: shift width: wrote G#{word:04X} to G#{:04X} and G#{:04X}, read back G#{:04X} and G#{:04X}{}\n", l.fll_reg, l.coarse_reg, u16::from_be_bytes(a), u16::from_be_bytes(b), if ok { "" } else { " (a transfer failed)" });
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}


/// The readout, watched in memory (`MEASURE`, a device test): after a frame's DMA_DONE and the SHADOW_UPDATE answered after it, the buffer the next frame lands in (the one its descriptor names, `LAST_R`) gets the pattern in its first row's first word (its last row's probe is patterned already, `ring_turn`); the thread sleeps thru the part of the period no readout can reach, then watches both words until the last row lands. From the first row's landing to the last's is the readout (`READ`).
/// Where a readout can start: if the vendor shift scaled the line and not only the counters, the readout would take the rows' share of the period (3072 of 40650 lines, 7.6 %, at the shifted 16 s), so the watch starts twice that share before the period ends, and gives up a quarter period after it.
fn row_timing() {
    let period = PERIOD_NS.load(Ordering::Relaxed) / 1000;
    let (rows, fll) = (ROWS.load(Ordering::Relaxed), FLL.load(Ordering::Relaxed));
    // WHY the test: a frame of fewer than twice its rows in lines leaves no part of the period free of the readout, and the watch starts at once.
    let quiet_us = if fll > rows << 1 { period * (fll - (rows << 1)) / fll } else { 0 };
    let b = LAST_R.load(Ordering::Relaxed) as usize;
    let first = (RING_VA.load(Ordering::Relaxed) as usize + b * BUF.load(Ordering::Relaxed) as usize) as *mut u64;
    let last = probe_at(b);
    unsafe { core::ptr::write_volatile(first, PATTERN) };
    let (t0, hz) = (ferros_sys::now(), ferros_sys::freq());
    ferros_sys::sleep_until(t0 + hz * quiet_us / 1_000_000);
    let until = t0 + hz * (period + (period >> 2)) / 1_000_000;
    let mut seen_first = 0u64;
    loop {
        let now = ferros_sys::now();
        if seen_first == 0 && unsafe { core::ptr::read_volatile(first) } != PATTERN {
            seen_first = now;
        }
        if unsafe { core::ptr::read_volatile(last) } != PATTERN {
            if seen_first != 0 {
                let d = now - seen_first;
                READ[0].store(d, Ordering::Relaxed);
                READ[1].fetch_min(d, Ordering::Relaxed);
                READ[2].fetch_max(d, Ordering::Relaxed);
                READ[3].fetch_add(1, Ordering::Relaxed);
            } else {
                READ[4].fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        if now >= until || RUN.load(Ordering::Acquire) != 1 {
            READ[4].fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
}
// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
