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
const EVT_CONTROLLER_READY: u8 = 0x03;
const EVT_ENTER: u8 = 0x13;
const EVT_MOTION: u8 = 0x23;
const EVT_LEAVE: u8 = 0x33;
const EVT_STATUS: u8 = 0x43;
const EVT_ERROR: u8 = 0xF3;

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

/// The touch resolution last read (X << 16 | Y), 0 until `up` reads it.
static RES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn evt_hex(e: &[u8; EVT_SIZE]) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for b in e {
        s.push_str(&alloc::format!("{b:02X}"));
    }
    s
}

/// Poll the FIFO until an event whose first byte is `id` arrives or `ms` pass; every other event is reported.
fn wait_event(rep: &mut Vec<u8>, id: u8, ms: u64) -> Option<[u8; EVT_SIZE]> {
    let t = now() + ticks_us(ms * 1000);
    let mut shown = 0;
    while now() < t {
        match read_event() {
            Ok(e) if e[0] == id => return Some(e),
            Ok(e) if e[0] == 0x00 || e[0] == 0xFF => spin_us(4000),
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
    match transact(&[SCAN_MODE, 0x00, 0xFF], &mut []) {
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
    let mut last_xy = [(0u32, 0u32); 16];
    while now() < t_end {
        crate::platform::pet_watchdogs();
        let irq = rd(GPN0 + 4) & 1 == 0;
        if !irq && now() - last < ticks_us(16_000) {
            continue;
        }
        last = now();
        loop {
            let e = match read_event() {
                Ok(e) => e,
                Err(err) => {
                    line(rep, alloc::format!("SPI: {err}\n"));
                    return;
                }
            };
            if e[0] == 0x00 || e[0] == 0xFF {
                break;
            }
            events += 1;
            let tid = (e[1] >> 4) as usize;
            let x = ((e[3] as u32 & 0xF) << 8) | e[2] as u32;
            let y = ((e[4] as u32) << 4) | (e[3] as u32 >> 4);
            let ms_at = (now() - t0) / ticks_us(1000);
            match e[0] {
                EVT_ENTER | EVT_LEAVE => {
                    if e[0] == EVT_ENTER {
                        last_xy[tid & 15] = (x, y);
                    }
                    if lines < 64 {
                        let what = if e[0] == EVT_ENTER { "down" } else { "up" };
                        let (lx, ly) = last_xy[tid & 15];
                        let (px, py) = if e[0] == EVT_ENTER { (x, y) } else { (lx, ly) };
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
                    if lines < 64 && (e[0] == EVT_ERROR || e[0] == EVT_STATUS || e[0] == EVT_CONTROLLER_READY) {
                        line(rep, alloc::format!("A#{ms_at} ms: event G#{}\n", evt_hex(&e)));
                        lines += 1;
                    }
                }
            }
        }
    }
    line(rep, alloc::format!("A#{events} events in A#{ms} ms: A#{motion} motion, A#{other} other\n"));
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
            loop {
                let e = match read_event() {
                    Ok(e) => e,
                    Err(err) => {
                        line(rep, alloc::format!("SPI: {err}\n"));
                        return;
                    }
                };
                if e[0] == 0x00 || e[0] == 0xFF {
                    break;
                }
                events += 1;
                let tid = (e[1] >> 4) as usize & 15;
                let x = ((e[3] as u32 & 0xF) << 8) | e[2] as u32;
                let y = ((e[4] as u32) << 4) | (e[3] as u32 >> 4);
                match e[0] {
                    EVT_ENTER | EVT_MOTION => {
                        if e[0] == EVT_ENTER {
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
