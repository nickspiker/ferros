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
//! - `1` OPEN + id: `[0]` open, `[1]` no such camera, `[2]` another holds it, `[3]` it would not start (the console says why).
//! - `2` FRAME + bin: waits for the next frame; a 64-byte header, then the mosaic, `bin` x `bin` same-colour samples averaged, 16 bits a sample. bin 2, 4 or 8 (a whole readout is past the largest reply).
//! - `3` SET + what + value (8 bytes): what 0 exposure (ns), 1 analogue gain (1024 unity), 2 focus. `[0]` taken, `[1]` outside the range.
//! - `4` CLOSE: the stream stopped, the switch off, `[0]`, and the driver ends; its frames go back to the pool wiped.
//! A holder that ends without closing: its waiting FRAME's reply fails, or 64 frames pass with nobody asking; either closes.
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

/// The front end answered once: its aggregate and loch0's image status read, every acknowledgement made in Linux's order, loch0's paths enabled after the first SHADOW_UPDATE, and on a frame's DMA_DONE the engine's index advanced and the frame counted.
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
        // The next descriptor. Never round the ring: back to 0 the front end stops for good (`RING`). At the ring's end the index is left where it is, the console says so, and the stream ends with its last descriptor (the FRAME that finds no frame closes).
        if *idx < RING_N.load(Ordering::Relaxed) as u32 * STEP {
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
    /// Where this process reads the region (its physical start went into the job).
    va: usize,
    /// The sensor's settings now (lines, the gain code, the lens's target) and the frame counted at the last FRAME request.
    exposure_lines: u64,
    gain: u64,
    focus: u64,
    asked_at: u64,
    /// The outputs' extents have been reported (after the eighth frame).
    probed: bool,
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
    // The job's region and one frame more, for the engine's long ring (`RING`).
    let frames = job::MAIN.bytes / job::FRAME + 1;
    let mut hint = [0u8; 8];
    let low_first = get(g, b"region", &mut hint) != Some(&b"high"[..]);
    let ask = |low: u64| ferros_sys::svc(ferros_sys::nr::DMA, cap(g, S_REGION), 3, frames, low, 0);
    let mut pa = ask(low_first as u64);
    if pa == ERR {
        pa = ask(!low_first as u64);
    }
    if pa == ERR {
        return Err(Fail::Region);
    }
    let va = ferros_sys::dma(cap(g, S_REGION));
    if va == ERR {
        return Err(Fail::Region);
    }
    let va = va as usize;
    say!(g, "camera: A#{frames} frames at G#{pa:X}, read at G#{va:X}\n");
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
    let m = job::relocate(&mut jb[..jn], pa, &job::MAIN).map_err(Fail::Job)?;
    let pieces = job::pieces(&jb[..jn]).map_err(Fail::Job)?;
    for &(old, at, len) in pieces.iter() {
        let dst = va + (old - job::OLD_JOB) as usize;
        for (i, &b) in jb[at..at + len].iter().enumerate() {
            unsafe { core::ptr::write_volatile((dst + i) as *mut u8, b) };
        }
    }
    for &(off, size) in job::MAIN.slots.iter().filter(|s| s.1 != 0) {
        let p = (va + off as usize) as *mut u64;
        for i in 0..size as usize / 8 {
            unsafe { core::ptr::write_volatile(p.add(i), PATTERN) };
        }
    }
    ferros_sys::dma_barrier();
    say!(g, "camera: job placed: A#{} address registers, A#{} high halves (A#{} left as captured), A#{} loads, A#{} descriptors\n", m.addr_regs, m.highs, m.highs_left, m.loads, m.ring);
    // The CPU's setup writes before the engine starts, in the capture's order; without them the engine applies the program and the frame writer stays idle (2026-10-20).
    let mut n = 0;
    for (a, v) in prog.before.iter() {
        if inputs::cpu_write_allowed(a) {
            wr(g, a, v);
            n += 1;
        }
    }
    // The long ring in the region's last frame: every descriptor the job's first (moved already, so it names the program in the region), the job's own ring left at the region's start for an engine whose count does not hold it.
    let long = job::MAIN.bytes as usize;
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
    let (ring, ring_n) = if held { (pa + long as u64, RING) } else { (pa, RING_JOB) };
    RING_N.store(ring_n, Ordering::Relaxed);
    wr(g, pd + 0x2C, 1);
    wr(g, pd + 0x40, 0x1F);
    wr(g, pd + 0x20, 0);
    wr(g, pd + ferros_cam::PDMA_BASE_LO, ring as u32);
    wr(g, pd + ferros_cam::PDMA_BASE_HI, (ring >> 32) as u32);
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
    say!(g, "camera: engine started (ring G#{ring:X}, A#{ring_n} descriptors{}), receiver on, A#{n} CPU writes, A#{refused} refused\n", if held { "" } else { ": the count did not hold the long ring" });
    let o = Open { va, exposure_lines: ferros_cam::MAIN_EXPOSURE_MAX_LINES, gain: 1 << 10, focus: 0x3800, asked_at: 0, probed: false };
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
    for (k, &(off, size)) in job::MAIN.slots.iter().enumerate().filter(|s| s.1 .1 != 0) {
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
const HEADER: usize = 1 << 6;
/// The largest reply: bin 2, 2040 x 1536 samples of 16 bits, and the header.
static mut OUT: [u8; HEADER + (W / 2) * (H / 2) * 2] = [0; HEADER + (W / 2) * (H / 2) * 2];
/// The lines a binned row pair reads (2 x bin, at most 16), copied out of the non-cacheable region in 64-byte reads, and unpacked to 16 bits.
const BIN_MOST: usize = 1 << 3;
static mut RAW: [u64; 2 * BIN_MOST * STRIDE / 8] = [0; 2 * BIN_MOST * STRIDE / 8];
static mut PIX: [u16; 2 * BIN_MOST * W] = [0; 2 * BIN_MOST * W];
/// The summed line `bin_row` works in.
static mut ACC: [u16; W] = [0; W];

/// Counter ticks spent copying lines out of the region, summed over frames since the last console line (the rest of the binning is unpacking and averaging in cached memory).
static COPY_TICKS: AtomicU64 = AtomicU64::new(0);

/// The image body (loch0's image path, its output slot 2 in the region): a RAW10 frame as the front end writes it.
fn image(o: &Open) -> usize {
    o.va + job::MAIN.slots[job::OLD_SLOT_IMAGE].0 as usize
}

/// The frame into `OUT`, binned `bin` x `bin` per colour: output sample (ox, oy) is the mean of the bin x bin same-colour samples of its 2 bin x 2 bin block, so the mosaic keeps its phase (G B / R G). The header: count, width and height after binning, bin, cfa, black and white, exposure (ns), gain, focus, when it ended (Eagle ns). Its length.
fn bin_frame(o: &Open, bin: usize, count: u64) -> usize {
    let (bw, bh) = (W / bin, H / bin);
    let out = unsafe { &mut *(&raw mut OUT) };
    let raw = unsafe { &mut *(&raw mut RAW) };
    let pix = unsafe { &mut *(&raw mut PIX) };
    let src = image(o);
    let rows = 2 * bin;
    let words = STRIDE / 8;
    for pair in 0..bh / 2 {
        // The 2 x bin lines this row pair reads, copied out of the region whole (8-byte reads of non-cacheable memory), then unpacked (ferros_cam raw, host-tested against std/camreplay's binning of a recorded frame).
        for r in 0..rows {
            // 64 bytes a load (four NEON registers), not 8: each load of non-cacheable memory is its own transaction, and 8-byte ones took 121 ms a frame (the Pro, 2026-11-14), the whole of the binning's budget and more.
            let t = ferros_sys::now();
            let line = (src + (pair * rows + r) * STRIDE) as *const u8;
            let dst = unsafe { raw.as_mut_ptr().add(r * words) } as *mut u8;
            for i in 0..STRIDE / 64 {
                unsafe {
                    let v = core::arch::aarch64::vld1q_u8_x4(line.add(i * 64));
                    core::arch::aarch64::vst1q_u8_x4(dst.add(i * 64), v);
                }
            }
            COPY_TICKS.fetch_add(ferros_sys::now() - t, Ordering::Relaxed);
            let bytes = unsafe { core::slice::from_raw_parts(raw.as_ptr().add(r * words) as *const u8, STRIDE) };
            ferros_cam::raw::unpack(bytes, &mut pix[r * W..(r + 1) * W]);
        }
        for phase in 0..2 {
            let oy = pair * 2 + phase;
            ferros_cam::raw::bin_row(&pix[..rows * W], W, bin, phase, unsafe { &mut *(&raw mut ACC) }, &mut out[HEADER + oy * bw * 2..HEADER + (oy + 1) * bw * 2]);
        }
    }
    let h = &mut out[..HEADER];
    h.fill(0);
    h[0..4].copy_from_slice(&(count as u32).to_le_bytes());
    h[4..6].copy_from_slice(&(bw as u16).to_le_bytes());
    h[6..8].copy_from_slice(&(bh as u16).to_le_bytes());
    h[8] = bin as u8;
    h[9] = 1 | 2 << 2 | 0 << 4 | 1 << 6;
    h[10..12].copy_from_slice(&64u16.to_le_bytes());
    h[12..14].copy_from_slice(&1023u16.to_le_bytes());
    h[16..24].copy_from_slice(&(o.exposure_lines * ferros_cam::MAIN_LINE_PS / 1000).to_le_bytes());
    h[24..28].copy_from_slice(&(o.gain as u32).to_le_bytes());
    h[28..32].copy_from_slice(&(o.focus as u32).to_le_bytes());
    h[32..40].copy_from_slice(&FRAME_AT.load(Ordering::Acquire).to_le_bytes());
    HEADER + bw * bh * 2
}

// ---------------------------------------------------------------- the endpoint

/// Frames that may pass with nobody asking before the holder is taken for gone (about 4.4 s at 14.5 a second).
const UNASKED: u64 = 1 << 6;
/// How long a FRAME waits for its frame before the stream is taken for stopped: about thirty frame periods, past the sensor's start after stream on.
const FRAME_LATE_US: u64 = 1 << 21;

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(g: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    ferros_sys::thread_spawn(frame_thread, unsafe { &mut *(&raw mut FRAME_STACK) }, g);
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
                reply(&token, &[r]);
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
                // The next frame: one counted after this request came. Bounded: the sensor sends one every 67.8 ms, so none in `FRAME_LATE_US` is a stream that stopped (the front end stops for good when it is not answered in time), and the holder is told rather than kept waiting.
                let want = FRAMES.load(Ordering::Acquire) + 1;
                let until = ferros_sys::now() + ferros_sys::freq() * FRAME_LATE_US / 1_000_000;
                loop {
                    let n = FRAMES.load(Ordering::Acquire);
                    if n >= want || ferros_sys::now() >= until {
                        break;
                    }
                    ferros_sys::wait(&FRAMES, n, until);
                }
                let n = FRAMES.load(Ordering::Acquire);
                if n < want {
                    say!(g, "camera: no frame in A#{} ms (A#{n} so far; A#{} answers, A#{} on the line; image status bits seen G#{:X}); closing\n", FRAME_LATE_US / 1000, ANSWERED.load(Ordering::Relaxed), ON_LINE.load(Ordering::Relaxed), IMG_SEEN.load(Ordering::Relaxed));
                    close(g);
                    reply(&token, &[]);
                    ferros_sys::exit(0);
                }
                o.asked_at = n;
                if n >= 8 && !o.probed {
                    o.probed = true;
                    probe(g, o);
                }
                let t = ferros_sys::now();
                let len = bin_frame(o, bin, n);
                bin_us += ferros_sys::us_since(t);
                served += 1;
                if served % 64 == 0 {
                    let us = ferros_sys::us_since(since);
                    let copy_us = COPY_TICKS.swap(0, Ordering::Relaxed) * 1_000_000 / ferros_sys::freq();
                    say!(g, "camera: A#{served} frames served, the last 64 in A#{} ms (A#{} us apart), binning A#{} us each (copying out of the region A#{} us of it); A#{} taken by the front end\n", us / 1000, us / 64, bin_us / 64, copy_us / 64, n);
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
                            0
                        } else {
                            1
                        }
                    }
                    1 if value == 1 << 10 => 0,
                    2 if (0x0800..=0x7800).contains(&value) && focus_set(&bus, value).is_ok() => {
                        o.focus = value;
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
