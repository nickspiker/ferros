//! The touch controller: an ST FTM5 on the HSI0 USI2 SPI controller, driven by polling (docs/oracle/TOUCH.md).
//!
//! Everything here comes from two sources that agree: Google's public FTM5 driver (`ftsIO.c`, `ftsCore.c`, `fts.c`) and a clean register-oracle capture of its boot-time probe (`docs/oracle/touch-boot-probe-transfers.txt`).
//! The SPI controller runs in CPU mode with byte transfers, exactly as Android runs it: every transfer fits the 64-byte FIFO, so no DMA is needed.

use crate::acpm;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

/// Reset: gpp21-2 (`pinctrl@10C40000` bank G#40). Interrupt: gpn0-0 (`pinctrl@15060000` bank 0), active low.
const GPP21: usize = 0x10C4_0040;
const RESET_PIN: u32 = 2;
const GPN0: usize = 0x1506_0000;

/// The two rails on S2MPG14 (s2mpg14-register.h, s2mpg14-regulator.c): bit 7 enables, [5:0] selects the voltage.
const L25M_CTRL: u32 = 0x43;
const L4M_CTRL: u32 = 0x2E;
/// 1.8 V for DVDD: (1800000 - 700000) / 25000. 3.3 V for AVDD: (3300000 - 1800000) / 25000.
const L25M_1V8: u8 = 0x2C;
const L4M_3V3: u8 = 0x3C;

/// FTM5 commands (ftsHardware.h, SPI variant). Every read returns one dummy byte before the data.
const FIFO_READ: u8 = 0x87;
const HW_REG_R: u8 = 0xFB;
const HW_REG_W: u8 = 0xFA;
const SCAN_MODE: u8 = 0xA0;
const FB_READ: u8 = 0xA7;
const EVT_SIZE: usize = 8;
// The controller puts an event's identifier in the high nibble of byte zero and uses the low nibble for payload — the top two bits of a contact's major axis, for one — so every identifier here is a nibble and every comparison is against `e[0] >> 4`, as `fts_event_handler` (fts.c) dispatches on.
const EVT_CONTROLLER_READY: u8 = 0x0;
const EVT_ENTER: u8 = 0x1;
const EVT_MOTION: u8 = 0x2;
const EVT_LEAVE: u8 = 0x3;
const EVT_STATUS: u8 = 0x4;
const EVT_ERROR: u8 = 0xF;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn line(rep: &mut Vec<u8>, s: alloc::string::String) {
    rep.extend_from_slice(s.as_bytes());
}
fn now() -> u64 {
    crate::platform::cntpct()
}
fn ticks_us(us: u64) -> u64 {
    crate::platform::cntfrq() / 1_000_000 * us
}
fn spin_us(us: u64) {
    let t = now() + ticks_us(us);
    while now() < t {}
}

fn spi_clocked() -> bool {
    crate::spi::TOUCH.clocked()
}

fn spi_up() {
    crate::spi::TOUCH.up()
}

fn transact(cmd: &[u8], out: &mut [u8]) -> Result<(), &'static str> {
    crate::spi::TOUCH.transact(crate::spi::Cs::Native, cmd, out)
}

fn hex(r: Result<u8, u32>) -> alloc::string::String {
    match r {
        Ok(v) => alloc::format!("G#{v:02X}"),
        Err(e) => alloc::format!("error G#{e:X}"),
    }
}

/// Everything the bring-up depends on, read only. The SPI controller's own registers are read only when every gate on its path is open: a read into a gated block can hang the core.
pub fn state(rep: &mut Vec<u8>) {
    line(rep, alloc::format!("USI2: {}\n", crate::spi::TOUCH.describe()));
    line(rep, alloc::format!("gpp21 (reset = pin 2): CON G#{:X} DAT G#{:X} PUD G#{:X}\n", rd(GPP21), rd(GPP21 + 4), rd(GPP21 + 8)));
    line(rep, alloc::format!("gpn0 (interrupt = pin 0): CON G#{:X} DAT G#{:X} PUD G#{:X}\n", rd(GPN0), rd(GPN0 + 4), rd(GPN0 + 8)));
    line(rep, alloc::format!("LDO25M (DVDD 1.8 V) G#{:X}: {}\n", L25M_CTRL, hex(acpm::read(acpm::MAIN, acpm::T_PMIC, L25M_CTRL))));
    line(rep, alloc::format!("LDO4M (AVDD 3.3 V) G#{:X}: {}\n", L4M_CTRL, hex(acpm::read(acpm::MAIN, acpm::T_PMIC, L4M_CTRL))));
}

/// Read one event from the controller's FIFO (the dummy byte stripped).
fn read_event() -> Result<[u8; EVT_SIZE], &'static str> {
    let mut b = [0u8; EVT_SIZE + 1];
    transact(&[FIFO_READ], &mut b)?;
    let mut e = [0u8; EVT_SIZE];
    e.copy_from_slice(&b[1..]);
    Ok(e)
}

/// Drain the controller's event queue the way the vendor interrupt handler does: one read for the first event, whose byte seven carries in its low five bits how many are still queued, then bursts of the rest.
/// Seven events and the dummy byte fill one sixty-four-byte SPI segment, so the drain takes at most five transfers for a full thirty-two-event queue.
/// Reading events one at a time instead races the chip's queue cursor — the same event comes back a second time with its tail bytes zeroed, which reads as a contact jumping to the top of the screen at zero pressure.
fn drain_events(buf: &mut [[u8; EVT_SIZE]; 32]) -> Result<usize, &'static str> {
    let mut b = [0u8; 1 + EVT_SIZE * 7];
    transact(&[FIFO_READ], &mut b[..1 + EVT_SIZE])?;
    if b[1] == 0x00 || b[1] == 0xFF {
        return Ok(0);
    }
    buf[0].copy_from_slice(&b[1..1 + EVT_SIZE]);
    let mut n = 1;
    let mut queued = (buf[0][7] & 0x1F) as usize;
    while queued > 0 && n < buf.len() {
        let k = queued.min(7).min(buf.len() - n);
        transact(&[FIFO_READ], &mut b[..1 + EVT_SIZE * k])?;
        for i in 0..k {
            let e = &b[1 + i * EVT_SIZE..1 + (i + 1) * EVT_SIZE];
            if e[0] == 0x00 || e[0] == 0xFF {
                return Ok(n);
            }
            buf[n].copy_from_slice(e);
            n += 1;
        }
        queued -= k;
    }
    Ok(n)
}

/// Read `n` bytes of a 32-bit-addressed hardware register.
fn hw_read(addr: u32, out: &mut [u8]) -> Result<(), &'static str> {
    let a = addr.to_be_bytes();
    let mut b = [0u8; 17];
    let n = out.len().min(16);
    transact(&[HW_REG_R, a[0], a[1], a[2], a[3]], &mut b[..n + 1])?;
    out[..n].copy_from_slice(&b[1..n + 1]);
    Ok(())
}

fn hw_write(addr: u32, val: u8) -> Result<(), &'static str> {
    let a = addr.to_be_bytes();
    transact(&[HW_REG_W, a[0], a[1], a[2], a[3], val], &mut [])
}

/// Set the scanning mode, as `setScanMode` (ftsCore.c) does it: a two-byte zero write to G#2000_0000 first, then the mode command itself.
/// That register is the chip ID when read, so the write is not an ID change but a side-effect the controller requires before it will take a mode change; without it the mode command is accepted silently and no touch is ever reported.
fn scan_mode(mode: u8, settings: u8) -> Result<(), &'static str> {
    transact(&[HW_REG_W, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00], &mut [])?;
    transact(&[SCAN_MODE, mode, settings], &mut [])
}

/// Read `out.len()` bytes of the controller's framebuffer from `addr` (16-bit address, `FTS_CMD_FRAMEBUFFER_R`), in FIFO-sized chunks.
fn fb_read(addr: u16, out: &mut [u8]) -> Result<(), &'static str> {
    let mut b = [0u8; 64];
    let mut done = 0;
    while done < out.len() {
        let n = (out.len() - done).min(63);
        let a = (addr + done as u16).to_be_bytes();
        transact(&[FB_READ, a[0], a[1]], &mut b[..n + 1])?;
        out[done..done + n].copy_from_slice(&b[1..n + 1]);
        done += n;
    }
    Ok(())
}

/// The largest touch coordinates from the system-info block the firmware leaves in its framebuffer after a reset (`readSysInfo`): header G#A5 01 at offset 0, X and Y little-endian at 80 and 82.
/// Only those bytes are read: the block also holds a per-die identifier (offsets 32-47), which ferros has no reason to see here.
fn screen_resolution() -> Result<(u32, u32), &'static str> {
    let mut h = [0u8; 2];
    fb_read(0, &mut h)?;
    if h != [0xA5, 0x01] {
        return Err("system-info header missing");
    }
    let mut r = [0u8; 4];
    fb_read(80, &mut r)?;
    let (x, y) = (u16::from_le_bytes([r[0], r[1]]) as u32, u16::from_le_bytes([r[2], r[3]]) as u32);
    RES.store(x << 16 | y, core::sync::atomic::Ordering::Relaxed);
    Ok((x, y))
}

/// `diag touch info`: the chip's system-information block — the sensor grid and the addresses of its live capacitance frames (`readSysInfo`, ftsCore.c: resolution at 80, transmit/receive channel counts at 84/85, key and force lengths at 86/87, the mutual-sense frame addresses at 136 and the self-sense ones after them). Nothing per-die is read or printed.
pub fn info(rep: &mut Vec<u8>) {
    if !spi_clocked() {
        return line(rep, alloc::format!("SPI not clocked: run `diag touch up` first\n"));
    }
    let mut b = [0u8; 216];
    if let Err(e) = fb_read(0, &mut b) {
        return line(rep, alloc::format!("system info: {e}\n"));
    }
    if b[0] != 0xA5 || b[1] != 0x01 {
        return line(rep, alloc::format!("system info header missing (G#{:02X}{:02X})\n", b[0], b[1]));
    }
    let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let (tx, rx) = (b[84] as u32, b[85] as u32);
    line(rep, alloc::format!("grid: A#{tx} transmit x A#{rx} receive = A#{} nodes; largest coordinates A#{} and A#{}; key length A#{}, force length A#{}\n", tx * rx, u16at(80), u16at(82), b[86], b[87]));
    // Every frame the chip can hand over, in the order `readSysInfo` reads their addresses (ftsCore.c 736-800). A zero address means this panel does not carry that layer.
    const FRAMES: [(usize, &str); 28] = [
        (136, "mutual raw"), (138, "mutual filtered"), (140, "mutual strength"), (142, "mutual baseline"),
        (144, "self tx raw"), (146, "self tx filtered"), (148, "self tx strength"), (150, "self tx baseline"),
        (152, "self rx raw"), (154, "self rx filtered"), (156, "self rx strength"), (158, "self rx baseline"),
        (160, "key raw"), (162, "key filtered"), (164, "key strength"), (166, "key baseline"),
        (168, "force raw"), (170, "force filtered"), (172, "force strength"), (174, "force baseline"),
        (176, "hover tx raw"), (178, "hover tx filtered"), (180, "hover tx strength"), (182, "hover tx baseline"),
        (184, "hover rx raw"), (186, "hover rx filtered"), (188, "hover rx strength"), (190, "hover rx baseline"),
    ];
    let mut have = 0u32;
    let mut s = alloc::string::String::new();
    for (o, name) in FRAMES {
        let a = u16at(o);
        if a != 0 {
            have += 1;
            s.push_str(&alloc::format!("  {name}: G#{a:X}\n"));
        }
    }
    // The last four groups (proximity transmit/receive, detect) finish the block.
    for (o, name) in [(192usize, "proximity tx"), (200, "proximity rx"), (208, "detect")] {
        for (i, kind) in ["raw", "filtered", "strength", "baseline"].iter().enumerate() {
            let a = u16at(o + i * 2);
            if a != 0 {
                have += 1;
                s.push_str(&alloc::format!("  {name} {kind}: G#{a:X}\n"));
            }
        }
    }
    line(rep, alloc::format!("A#{have} of A#40 frame layers carried by this panel:\n"));
    line(rep, s);
    line(rep, alloc::format!("a mutual frame is A#{} bytes, a self-sense one A#{} or A#{}\n", tx * rx * 2, tx * 2, rx * 2));
}

/// The touch resolution last read (X << 16 | Y), 0 until `up` reads it.
static RES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn evt_hex(e: &[u8; EVT_SIZE]) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for b in e {
        s.push_str(&alloc::format!("{b:02X}"));
    }
    s
}

/// Poll the FIFO until an event whose identifier nibble is `id` arrives or `ms` pass; every other event is reported.
fn wait_event(rep: &mut Vec<u8>, id: u8, ms: u64) -> Option<[u8; EVT_SIZE]> {
    let t = now() + ticks_us(ms * 1000);
    let mut shown = 0;
    while now() < t {
        match read_event() {
            // An empty FIFO reads as all zeroes or all ones, and zero shares its high nibble with EVT_CONTROLLER_READY, so nothing-pending has to be ruled out before the identifier is matched.
            Ok(e) if e[0] == 0x00 || e[0] == 0xFF => spin_us(4000),
            Ok(e) if e[0] >> 4 == id => return Some(e),
            Ok(e) => {
                if shown < 8 {
                    line(rep, alloc::format!("  event G#{}\n", evt_hex(&e)));
                    shown += 1;
                }
            }
            Err(err) => {
                line(rep, alloc::format!("  SPI: {err}\n"));
                return None;
            }
        }
    }
    None
}

fn set_reset(high: bool) {
    let d = rd(GPP21 + 4);
    wr(GPP21 + 4, if high { d | 1 << RESET_PIN } else { d & !(1 << RESET_PIN) });
    let c = rd(GPP21);
    wr(GPP21, (c & !(0xF << (RESET_PIN * 4))) | 1 << (RESET_PIN * 4));
}

/// Program a rail's voltage, then enable it, then read it back.
fn rail_on(reg: u32, vsel: u8) -> Result<u8, u32> {
    acpm::update(acpm::MAIN, acpm::T_PMIC, reg, vsel, 0x3F)?;
    acpm::update(acpm::MAIN, acpm::T_PMIC, reg, 0x80, 0x80)?;
    acpm::read(acpm::MAIN, acpm::T_PMIC, reg)
}

/// Bring the touch controller up from a cold boot: SPI controller, then `fts_chip_powercycle` (reset low, DVDD, 1 ms, AVDD, 15 ms, reset high), then the probe's order from the capture: controller-ready event, the GPIO6 fix-up (`FTS_GPIO6_UNUSED`), chip ID, scan mode active with all sensing on.
pub fn up(rep: &mut Vec<u8>) {
    spi_up();
    if !spi_clocked() {
        line(rep, alloc::format!("SPI clock path still gated after enabling it; stopping\n"));
        state(rep);
        return;
    }
    line(rep, alloc::format!("SPI up: {}\n", crate::spi::TOUCH.describe()));
    set_reset(false);
    let d = rail_on(L25M_CTRL, L25M_1V8);
    line(rep, alloc::format!("DVDD (LDO25M): {}\n", hex(d)));
    spin_us(1000);
    let a = rail_on(L4M_CTRL, L4M_3V3);
    line(rep, alloc::format!("AVDD (LDO4M): {}\n", hex(a)));
    if d.is_err() || a.is_err() {
        line(rep, alloc::format!("rail failed; leaving reset low\n"));
        return;
    }
    spin_us(15_000);
    set_reset(true);
    let t0 = now();
    match wait_event(rep, EVT_CONTROLLER_READY, 2000) {
        Some(e) => line(rep, alloc::format!("controller ready after A#{} ms: G#{}\n", (now() - t0) / ticks_us(1000), evt_hex(&e))),
        None => {
            line(rep, alloc::format!("no controller-ready event in 2 s; interrupt pin G#{:X}\n", rd(GPN0 + 4) & 1));
            return;
        }
    }
    let mut g = [0u8; 1];
    match hw_read(0x2000_00DE, &mut g).and_then(|_| hw_write(0x2000_00DE, g[0] | 0x80)) {
        Ok(()) => line(rep, alloc::format!("GPIO6 register G#{:02X} -> G#{:02X}\n", g[0], g[0] | 0x80)),
        Err(e) => line(rep, alloc::format!("GPIO6 fix-up: {e}\n")),
    }
    match screen_resolution() {
        Ok((x, y)) => line(rep, alloc::format!("touch system info: largest coordinates A#{x} and A#{y}\n")),
        Err(e) => line(rep, alloc::format!("system info: {e}\n")),
    }
    let mut id = [0u8; 8];
    match hw_read(0x2000_0000, &mut id) {
        Ok(()) => line(rep, alloc::format!("chip ID G#{:02X}{:02X} (FTM5 parts: G#3648 ALIX, G#3640 SALIXP, G#3639), fw G#{:02X}{:02X}\n", id[0], id[1], id[5], id[4])),
        Err(e) => line(rep, alloc::format!("chip ID: {e}\n")),
    }
    match scan_mode(0x00, 0xFF) {
        Ok(()) => match wait_event(rep, EVT_STATUS, 256) {
            Some(e) => line(rep, alloc::format!("scan mode active: G#{}\n", evt_hex(&e))),
            None => line(rep, alloc::format!("scan mode written, no echo seen\n")),
        },
        Err(e) => line(rep, alloc::format!("scan mode: {e}\n")),
    }
}

/// Watch the touch event stream for `ms`, reading whenever the interrupt line is low (and every 16 ms regardless), and report the touches: one line per enter and leave, motion counted, at most 64 lines.
pub fn watch(rep: &mut Vec<u8>, ms: u64) {
    if !spi_clocked() {
        line(rep, alloc::format!("SPI not clocked: run `diag touch up` first\n"));
        return;
    }
    let t_end = now() + ticks_us(ms * 1000);
    let t0 = now();
    let mut last = 0u64;
    let (mut lines, mut motion, mut events, mut other) = (0u32, 0u32, 0u32, 0u32);
    // How often the controller's interrupt line was asserted, and how often the queue was polled: the line low with an empty queue would mean the reads are wrong, the line never low means the controller is reporting nothing at all.
    let (mut irq_low, mut polls) = (0u32, 0u32);
    let mut last_xy = [(0u32, 0u32); 16];
    while now() < t_end {
        crate::platform::pet_watchdogs();
        let irq = rd(GPN0 + 4) & 1 == 0;
        if irq {
            irq_low += 1;
        }
        if !irq && now() - last < ticks_us(16_000) {
            continue;
        }
        last = now();
        polls += 1;
        let mut batch = [[0u8; EVT_SIZE]; 32];
        let n = match drain_events(&mut batch) {
            Ok(n) => n,
            Err(err) => {
                line(rep, alloc::format!("SPI: {err}\n"));
                return;
            }
        };
        for e in batch[..n].iter().copied() {
            events += 1;
            let tid = (e[1] >> 4) as usize;
            let x = ((e[3] as u32 & 0xF) << 8) | e[2] as u32;
            let y = ((e[4] as u32) << 4) | (e[3] as u32 >> 4);
            let ms_at = (now() - t0) / ticks_us(1000);
            match e[0] >> 4 {
                EVT_ENTER | EVT_LEAVE => {
                    let enter = e[0] >> 4 == EVT_ENTER;
                    if enter {
                        last_xy[tid & 15] = (x, y);
                    }
                    if lines < 64 {
                        let what = if enter { "down" } else { "up" };
                        let (lx, ly) = last_xy[tid & 15];
                        let (px, py) = if enter { (x, y) } else { (lx, ly) };
                        line(rep, alloc::format!("A#{ms_at} ms: finger A#{tid} {what} at x A#{px} y A#{py}, z A#{}\n", e[5]));
                        lines += 1;
                    }
                }
                EVT_MOTION => {
                    motion += 1;
                    last_xy[tid & 15] = (x, y);
                }
                _ => {
                    other += 1;
                    if lines < 64 && matches!(e[0] >> 4, EVT_ERROR | EVT_STATUS | EVT_CONTROLLER_READY) {
                        line(rep, alloc::format!("A#{ms_at} ms: event G#{}\n", evt_hex(&e)));
                        lines += 1;
                    }
                }
            }
        }
    }
    line(rep, alloc::format!("A#{events} events in A#{ms} ms: A#{motion} motion, A#{other} other; the interrupt line was asserted on A#{irq_low} of A#{polls} polls\n"));
}

/// Power the touch controller down: reset low, AVDD off, DVDD off (the reverse of `up`).
pub fn off(rep: &mut Vec<u8>) {
    set_reset(false);
    let a = acpm::update(acpm::MAIN, acpm::T_PMIC, L4M_CTRL, 0, 0x80).and_then(|_| acpm::read(acpm::MAIN, acpm::T_PMIC, L4M_CTRL));
    let d = acpm::update(acpm::MAIN, acpm::T_PMIC, L25M_CTRL, 0, 0x80).and_then(|_| acpm::read(acpm::MAIN, acpm::T_PMIC, L25M_CTRL));
    line(rep, alloc::format!("touch off: AVDD {} DVDD {}\n", hex(a), hex(d)));
}

/// Finger colours for `paint` (G#00RRGGBB), indexed by touch ID.
const INK: [u32; 4] = [0x00FF_B000, 0x0000_C0FF, 0x0040_FF40, 0x00FF_4080];

/// Draw what the fingers do on the panel for `ms`: the surface is cleared, every enter/motion event leaves a dot (strokes are filled in between consecutive points), and a frame is pushed whenever something new was drawn, at most every 16 ms. The touch resolution read by `up` maps to the panel's width; both axes use that one scale.
pub fn paint(rep: &mut Vec<u8>, ms: u64) {
    if !spi_clocked() {
        line(rep, alloc::format!("SPI not clocked: run `diag touch up` first\n"));
        return;
    }
    let freq = crate::platform::cntfrq();
    let mut scratch = Vec::new();
    let Some(panel) = crate::display::adopt(freq, &mut scratch) else {
        rep.extend_from_slice(&scratch);
        line(rep, alloc::format!("no panel to paint on\n"));
        return;
    };
    let res = RES.load(core::sync::atomic::Ordering::Relaxed);
    // The system info gives the largest coordinates long side first (2991, 1343 on husky); touch events report x across the short side, as the input layer sees them (fts.c passes them through unswapped). The panel is portrait, so x maps to its width.
    let (a, b) = if res == 0 { (2991, 1343) } else { (res >> 16, res & 0xFFFF) };
    let (rx, ry) = (a.min(b) + 1, a.max(b) + 1);
    let put = |x: i64, y: i64, c: u32| {
        if x >= 0 && y >= 0 && (x as usize) < panel.width && (y as usize) < panel.rows {
            unsafe { write_volatile((panel.fb + (y as usize * panel.stride + x as usize) * 4) as *mut u32, c) };
        }
    };
    for y in 0..panel.rows as i64 {
        for x in 0..panel.width as i64 {
            put(x, y, 0);
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    scratch.clear();
    panel.present(&mut scratch);
    let dot = |cx: i64, cy: i64, c: u32| {
        for dy in -10i64..=10 {
            for dx in -10i64..=10 {
                if dx * dx + dy * dy <= 100 {
                    put(cx + dx, cy + dy, c);
                }
            }
        }
    };
    let scale = |v: u32| (v as i64) * panel.width as i64 / rx.max(1) as i64;
    let mut last: [Option<(i64, i64)>; 16] = [None; 16];
    let (mut events, mut frames, mut downs) = (0u32, 0u32, 0u32);
    let (mut max_x, mut max_y) = (0u32, 0u32);
    let t0 = now();
    let t_end = t0 + ticks_us(ms * 1000);
    let mut polled = 0u64;
    let mut presented = 0u64;
    let mut dirty = false;
    while now() < t_end {
        crate::platform::pet_watchdogs();
        let irq = rd(GPN0 + 4) & 1 == 0;
        if irq || now() - polled > ticks_us(16_000) {
            polled = now();
            let mut batch = [[0u8; EVT_SIZE]; 32];
            let n = match drain_events(&mut batch) {
                Ok(n) => n,
                Err(err) => {
                    line(rep, alloc::format!("SPI: {err}\n"));
                    return;
                }
            };
            for e in batch[..n].iter().copied() {
                events += 1;
                let tid = (e[1] >> 4) as usize & 15;
                let x = ((e[3] as u32 & 0xF) << 8) | e[2] as u32;
                let y = ((e[4] as u32) << 4) | (e[3] as u32 >> 4);
                match e[0] >> 4 {
                    EVT_ENTER | EVT_MOTION => {
                        if e[0] >> 4 == EVT_ENTER {
                            downs += 1;
                            last[tid] = None;
                        }
                        max_x = max_x.max(x);
                        max_y = max_y.max(y);
                        let (px, py) = (scale(x), scale(y));
                        let c = INK[tid % INK.len()];
                        match last[tid] {
                            Some((lx, ly)) => {
                                let steps = ((px - lx).abs().max((py - ly).abs()) / 4).max(1);
                                for i in 1..=steps {
                                    dot(lx + (px - lx) * i / steps, ly + (py - ly) * i / steps, c);
                                }
                            }
                            None => dot(px, py, c),
                        }
                        last[tid] = Some((px, py));
                        dirty = true;
                    }
                    EVT_LEAVE => last[tid] = None,
                    _ => {}
                }
            }
        }
        if dirty && now() - presented > ticks_us(16_000) {
            unsafe { core::arch::asm!("dsb sy") };
            scratch.clear();
            panel.present(&mut scratch);
            presented = now();
            frames += 1;
            dirty = false;
        }
    }
    line(rep, alloc::format!("paint: A#{events} events, A#{downs} touches, A#{frames} frames in A#{ms} ms; touch A#{rx} x A#{ry} on panel A#{} x A#{} (A#{} rows reachable); largest x A#{max_x} y A#{max_y}\n", panel.width, (ry as usize) * panel.width / (rx.max(1) as usize), panel.rows));
}

/// `diag touch map`: the chip's mutual-sense **strength** frame (raw minus its own baseline, so a finger shows as a positive bump) read live, painted on the panel as nearest-neighbour blocks and printed as an ASCII map.
///
/// One 16-bit signed value per crossing, 18 transmit by 39 receive on husky. The screen mapping is transmit across the width and receive down the height, which matches the panel's aspect; if a finger shows up transposed, swap them here.
pub fn map(rep: &mut Vec<u8>, ms: u64) {
    frames(rep, ms, true)
}

/// `diag touch raw`: the same, on the **mutual raw** frame — the rawest live layer the chip exposes, absolute capacitance per crossing with no baseline taken off. Auto-ranged per frame, so the picture stays readable while the values wander, and run long enough to watch it behave.
pub fn raw(rep: &mut Vec<u8>, ms: u64) {
    frames(rep, ms, false)
}

/// `strength` takes the chip's own baseline off; `raw` is the absolute reading.
fn frames(rep: &mut Vec<u8>, ms: u64, strength: bool) {
    if !spi_clocked() {
        return line(rep, alloc::format!("SPI not clocked: run `diag touch up` first\n"));
    }
    let mut blk = [0u8; 216];
    if fb_read(0, &mut blk).is_err() || blk[0] != 0xA5 {
        return line(rep, alloc::format!("could not read the system-information block\n"));
    }
    let (tx, rx) = (blk[84] as usize, blk[85] as usize);
    let at = if strength { 140 } else { 136 };
    let addr = u16::from_le_bytes([blk[at], blk[at + 1]]);
    if tx == 0 || rx == 0 || tx * rx > 4096 {
        return line(rep, alloc::format!("implausible grid A#{tx} x A#{rx}\n"));
    }
    line(rep, alloc::format!("{} frame at G#{addr:X}, A#{tx} x A#{rx}, A#{} bytes\n", if strength { "strength" } else { "raw" }, tx * rx * 2));
    let freq = crate::platform::cntfrq();
    let _ = crate::display::use_own_fb(true);
    let mut scratch = Vec::new();
    let panel = crate::display::adopt(freq, &mut scratch);
    if panel.is_none() {
        line(rep, alloc::format!("no panel: the ASCII map below only\n"));
    }
    let bw = panel.as_ref().map(|p| p.width / tx).unwrap_or(0);
    let bh = panel.as_ref().map(|p| p.rows / rx).unwrap_or(0);
    let mut raw = alloc::vec![0u8; tx * rx * 2];
    let mut last = alloc::vec![0i32; tx * rx];
    let mut nlo = alloc::vec![i32::MAX; tx * rx];
    let mut nhi = alloc::vec![i32::MIN; tx * rx];
    let (mut lo, mut hi) = (i32::MAX, i32::MIN);
    let (mut frames, mut paint_us) = (0u32, 0u64);
    let t_end = now() + ticks_us(ms * 1000);
    while now() < t_end {
        crate::platform::pet_watchdogs();
        if let Err(e) = fb_read(addr, &mut raw) {
            line(rep, alloc::format!("frame read: {e}\n"));
            break;
        }
        for i in 0..tx * rx {
            let v = i16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]) as i32;
            last[i] = v;
            lo = lo.min(v);
            hi = hi.max(v);
            nlo[i] = nlo[i].min(v);
            nhi[i] = nhi[i].max(v);
        }
        frames += 1;
        let Some(p) = panel.as_ref() else { continue };
        // Scale this frame's own range so the picture is readable whatever the baseline sits at.
        let (flo, fhi) = last.iter().fold((i32::MAX, i32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        let span = (fhi - flo).max(1);
        let t0 = now();
        for r in 0..rx {
            for t in 0..tx {
                let s = ((last[t * rx + r] - flo) * 255 / span) as u32;
                // Black to red to yellow to white: a finger reads as the bright end.
                let c = if s < 85 { s * 3 << 16 } else if s < 170 { (255 << 16) | ((s - 85) * 3 << 8) } else { (255 << 16) | (255 << 8) | (s - 170) * 3 };
                for y in 0..bh {
                    let row = p.fb + (r * bh + y) * p.stride * 4 + t * bw * 4;
                    for x in 0..bw {
                        unsafe { core::ptr::write_volatile((row + x * 4) as *mut u32, c) };
                    }
                }
            }
        }
        unsafe { core::arch::asm!("dsb sy") };
        scratch.clear();
        p.present(&mut scratch);
        paint_us += (now() - t0) / (crate::platform::cntfrq() / 1_000_000);
    }
    // Give the panel back: ferros's own framebuffer held the last heat-map frame, which looks like a frozen phone (2026-10-20). Switching back to the surface the boot left and pushing one frame restores it.
    if panel.is_some() {
        let _ = crate::display::use_own_fb(false);
        scratch.clear();
        if let Some(p) = crate::display::adopt(freq, &mut scratch) {
            p.present(&mut scratch);
        }
    }
    line(rep, alloc::format!("A#{frames} frames in A#{ms} ms; values A#{lo}..A#{hi}; A#{} ms per paint; panel restored\n", if frames > 0 { paint_us / frames as u64 / 1000 } else { 0 }));
    // The last frame as an ASCII map, scaled over its own range.
    let (flo, fhi) = last.iter().fold((i32::MAX, i32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let span = (fhi - flo).max(1);
    let ramp = b" .:-=+*#%@";
    line(rep, alloc::format!("last frame, A#{flo}..A#{fhi} (each row is one receive channel, each column one transmit):\n"));
    for r in 0..rx {
        let mut row = alloc::string::String::new();
        for t in 0..tx {
            let s = ((last[t * rx + r] - flo) * 9 / span) as usize;
            row.push(ramp[s.min(9)] as char);
        }
        line(rep, alloc::format!("  {row}\n"));
    }
    // How much each node moved over the whole run: fixed pattern shows as a steady picture, drift and noise as a loud one.
    let span_of: Vec<i32> = (0..tx * rx).map(|i| if nhi[i] > nlo[i] { nhi[i] - nlo[i] } else { 0 }).collect();
    let (slo, shi) = span_of.iter().fold((i32::MAX, i32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let sspan = (shi - slo).max(1);
    line(rep, alloc::format!("per-node spread over the run, A#{slo}..A#{shi}:\n"));
    for r in 0..rx {
        let mut row = alloc::string::String::new();
        for t in 0..tx {
            let v = ((span_of[t * rx + r] - slo) * 9 / sspan) as usize;
            row.push(ramp[v.min(9)] as char);
        }
        line(rep, alloc::format!("  {row}\n"));
    }
    let mut top: Vec<(i32, usize)> = last.iter().copied().zip(0..).collect();
    top.sort_by(|a, b| b.0.cmp(&a.0));
    let mut s = alloc::string::String::from("strongest nodes:");
    for &(v, i) in top.iter().take(6) {
        s.push_str(&alloc::format!(" (tx A#{}, rx A#{}) = A#{v};", i / rx, i % rx));
    }
    s.push('\n');
    line(rep, s);
}

/// `diag touch decay`: a decay screen at 1:1 in 10-bit-per-channel colour. Every frame each channel of every pixel loses one, and every touch reading stamps two filled circles at the contact: the contact's ellipse split across the channels: red sized by its major axis, blue by its minor, green by the reported pressure. So a finger leaves a fading trail whose red fringe outside the blue disc is the eccentricity of the contact, and whose green says how hard it pressed.
///
/// The surface is ferros's own framebuffer (`DISPLAY_FB`), one u32 per pixel with alpha in bits [31:30] and three 10-bit channels, so the fade has 1024 steps instead of 256. The layer's format and the panel go back as they were on the way out.
pub fn decay(rep: &mut Vec<u8>, ms: u64) {
    if !spi_clocked() {
        return line(rep, alloc::format!("SPI not clocked: run `diag touch up` first\n"));
    }
    let freq = crate::platform::cntfrq();
    if !crate::display::use_own_fb(true) {
        return line(rep, alloc::format!("no framebuffer region on this boot path\n"));
    }
    let mut scratch = Vec::new();
    let Some(p) = crate::display::adopt(freq, &mut scratch) else {
        let _ = crate::display::use_own_fb(false);
        return line(rep, alloc::format!("no panel\n"));
    };
    p.set_10bit(true);
    let (w, h, stride, fb) = (p.width, p.rows, p.stride, p.fb);
    // The canvas lives in the cacheable display heap, not in the framebuffer: the framebuffer is Normal non-cacheable, where every read is its own uncached transaction, and a read-modify-write of four million pixels there took 1079 ms a frame (2026-10-20). Cacheable read-modify-write plus one write-only blit is the same work at a usable rate.
    let (heap, heap_end) = crate::cachemap::DISPLAY_HEAP;
    let need = stride * h * 4;
    if heap == 0 || heap + need > heap_end {
        p.set_10bit(false);
        let _ = crate::display::use_own_fb(false);
        return line(rep, alloc::format!("the display heap holds A#{} bytes, the canvas needs A#{need}\n", heap_end - heap));
    }
    let px = |i: usize| unsafe { core::ptr::read_volatile((heap + i * 4) as *const u32) };
    let set = |i: usize, v: u32| unsafe { core::ptr::write_volatile((heap + i * 4) as *mut u32, v) };
    for i in 0..stride * h {
        set(i, 0xC000_0000);
    }
    // Opaque alpha, every channel dark.
    for i in 0..stride * h {
        unsafe { core::ptr::write_volatile((fb + i * 4) as *mut u32, 0xC000_0000) };
    }
    line(rep, alloc::format!("decay screen A#{w} x A#{h} at 1:1, 10 bits a channel\n"));
    let stamp = |cx: i64, cy: i64, r: i64, mask: u32| {
        if r <= 0 {
            return;
        }
        let r = r.min(400);
        for dy in -r..=r {
            let y = cy + dy;
            if y < 0 || y as usize >= h {
                continue;
            }
            let half = isqrt(r * r - dy * dy);
            for dx in -half..=half {
                let x = cx + dx;
                if x < 0 || x as usize >= w {
                    continue;
                }
                let i = y as usize * stride + x as usize;
                set(i, px(i) | mask);
            }
        }
    };
    let (mut frames, mut stamps, mut decay_us) = (0u32, 0u32, 0u64);
    let (mut pmax, mut mmax, mut nmax) = (0u32, 0u32, 0u32);
    let t_end = now() + ticks_us(ms * 1000);
    while now() < t_end {
        crate::platform::pet_watchdogs();
        // One off every channel, each saturating at zero; alpha untouched.
        let t0 = now();
        for i in 0..stride * h {
            let v = px(i);
            let (r, g, b) = (v & 0x3FF, (v >> 10) & 0x3FF, (v >> 20) & 0x3FF);
            let d = r.saturating_sub(1) | g.saturating_sub(1) << 10 | b.saturating_sub(1) << 20;
            if d != v & 0x3FFF_FFFF {
                set(i, 0xC000_0000 | d);
            }
        }
        decay_us += (now() - t0) / (freq / 1_000_000);
        // Every reading waiting in the chip's queue.
        let mut batch = [[0u8; EVT_SIZE]; 32];
        let n = match drain_events(&mut batch) {
            Ok(n) => n,
            Err(err) => {
                line(rep, alloc::format!("SPI: {err}\n"));
                0
            }
        };
        for e in batch[..n].iter().copied() {
            if !matches!(e[0] >> 4, EVT_ENTER | EVT_MOTION) {
                continue;
            }
            let x = ((e[3] as i64 & 0xF) << 8) | e[2] as i64;
            let y = ((e[4] as i64) << 4) | (e[3] as i64 >> 4);
            let pressure = e[5] as u32;
            // The contact's two axes, each six bits spread over three bytes and scaled by AREA_SCALE, as the vendor driver assembles them.
            let major = ((((e[0] as u32 & 0x0C) << 2) | ((e[6] as u32 & 0xF0) >> 4)) * 16).min(1023);
            let minor = ((((e[7] as u32 & 0xC0) >> 2) | (e[6] as u32 & 0x0F)) * 16).min(1023);
            pmax = pmax.max(pressure);
            mmax = mmax.max(major);
            nmax = nmax.max(minor);
            // The ellipse split across red and blue: red is the major axis, blue the minor, so the width of the red fringe outside the blue disc is the eccentricity of the contact. Green is pressure.
            stamp(x, y, major as i64 / 2, 0x3FF << 20);
            stamp(x, y, minor as i64 / 2, 0x3FF);
            stamp(x, y, pressure as i64, 0x3FF << 10);
            stamps += 1;
        }
        // The canvas to the surface the layer reads, write-only.
        for i in 0..stride * h {
            unsafe { core::ptr::write_volatile((fb + i * 4) as *mut u32, px(i)) };
        }
        unsafe { core::arch::asm!("dsb sy") };
        scratch.clear();
        p.present(&mut scratch);
        frames += 1;
        if frames % 16 == 0 {
            crate::flashlog::line(&alloc::format!("touch decay: A#{frames} frames, A#{stamps} readings"));
        }
    }
    p.set_10bit(false);
    let _ = crate::display::use_own_fb(false);
    scratch.clear();
    if let Some(q) = crate::display::adopt(freq, &mut scratch) {
        q.present(&mut scratch);
    }
    line(rep, alloc::format!("A#{frames} frames in A#{ms} ms, A#{stamps} readings stamped; A#{} ms a frame in the decay pass; largest pressure A#{pmax}, largest axes A#{mmax} by A#{nmax}; panel restored\n", if frames > 0 { decay_us / frames as u64 / 1000 } else { 0 }));
}

/// Integer square root, for the circle's half-width per row.
fn isqrt(v: i64) -> i64 {
    if v <= 0 {
        return 0;
    }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}
