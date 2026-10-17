//! DSIM0: the MIPI DSI host that drives the panel (DISPLAY-HK3.md §7). The first thing ferros says to the panel itself: DCS packets through the DSIM's SFR packet FIFOs, exactly the bytes the oracle shows Android sending (oracle/display/husky-hk3/*/dsi-commands.txt).
//!
//! Command mode: frames go out only while the painter unmasks the TE trigger (display.rs `present`), so between presents the link is idle and a packet written to the FIFO transmits at once. Packet-go batching (CMD_CONFIG) is off, as Android leaves it. A short DCS write (type G#05 with no parameter, G#15 with one) is one header; a long write (G#39) or the PPS (G#0A) is the payload in 4-byte words, then the header carrying the length. Transmission is confirmed by polling FIFOCTRL's empty bits.

//!
//! Since 2026-10-16 night the packet path itself is `ferros_dsi::Dsim` (shared with the display server, which reaches the same registers through its grant); this module is the kernel's direct-MMIO `Link` plus the bench's existing entry points.

use alloc::vec::Vec;
use ferros_dsi::Dsim;
#[allow(unused_imports)]
pub use ferros_dsi::{DCS_LONG_WRITE, DCS_READ, DCS_SHORT_WRITE_0, DCS_SHORT_WRITE_1, PPS_LONG_WRITE};

const DSIM0: usize = 0x1944_0000;

/// Direct MMIO on DSIM0 with the generic timer at `cntfrq`.
struct Mmio(u64);

impl ferros_dsi::Link for Mmio {
    fn rd(&self, off: usize) -> u32 {
        unsafe { core::ptr::read_volatile((DSIM0 + off) as *const u32) }
    }
    fn wr(&self, off: usize, v: u32) {
        unsafe { core::ptr::write_volatile((DSIM0 + off) as *mut u32, v) }
    }
    fn now(&self) -> u64 {
        let t: u64;
        unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t) };
        t
    }
    fn freq(&self) -> u64 {
        self.0
    }
}

fn link(cntfrq: u64) -> Dsim<Mmio> {
    Dsim(Mmio(if cntfrq == 0 { 24_576_000 } else { cntfrq }))
}

fn rd(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((DSIM0 + off) as *const u32) }
}

/// Whether the DSIM is up at all (CONFIG with data lanes enabled). With the DPU domain off a read returns 0; the caller must have checked the domain through EL3 first (display.rs).
pub fn alive() -> bool {
    link(0).alive()
}

pub fn send(kind: u8, payload: &[u8], cntfrq: u64) -> Result<(), u32> {
    link(cntfrq).send(kind, payload)
}

pub fn dcs(bytes: &[u8], cntfrq: u64) -> Result<(), u32> {
    link(cntfrq).dcs(bytes)
}

pub fn sequence(script: &[u8], cntfrq: u64) -> (usize, Option<u32>) {
    link(cntfrq).sequence(script)
}

/// A DCS read of `len` bytes (at most 64) from register `cmd`.
pub fn read(cmd: u8, len: usize, cntfrq: u64) -> Result<Vec<u8>, u32> {
    let mut buf = [0u8; 64];
    let n = link(cntfrq).read(cmd, &mut buf[..len.clamp(1, 64)])?;
    Ok(Vec::from(&buf[..n]))
}

/// The HS bit rate the link runs at, Mbps: 1368 as ABL brings it up, 1346 as Android runs it (DISPLAY-HK3.md §7). Read from the DPHY PLL the first time it is asked for (a reload must not assume), set by dpu::modeset when it changes the rate.
static HS_CLK_MBPS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub fn hs_clk() -> u32 {
    let v = HS_CLK_MBPS.load(core::sync::atomic::Ordering::Relaxed);
    if v != 0 {
        return v;
    }
    let v = pll_hs_clk();
    HS_CLK_MBPS.store(v, core::sync::atomic::Ordering::Relaxed);
    v
}

/// The link rate the DPHY PLL is programmed for: hs = (M + K / 2¹⁶) × 24.576 MHz / P / 2^S × 2, from PLL_CON0 (P [5:0], S [10:8]), PLL_CON1 (K, two's complement) and PLL_CON2 (M), rounded to the Mbps the driver's tables use.
pub fn pll_hs_clk() -> u32 {
    ferros_dsi::pll_hs_clk(|off| unsafe { core::ptr::read_volatile((0x1946_0100 + off) as *const u32) })
}

pub fn set_hs_clk(mbps: u32) {
    HS_CLK_MBPS.store(mbps, core::sync::atomic::Ordering::Relaxed);
}

/// The command-mode TE timers for a refresh rate, the stock driver's formula (dsim_reg_get_cmd_timer: TE protect = hs × 95 × 100 / fps / 16; TE timeout = hs × 110 × 100 / fps / 16, integer division in that order; stable VFP = hactive × 2 / 100 with DSC). Reproduces Android's G#1A03_1E1F at 120 Hz with its 1346 Mbps link.
pub fn set_cmd_te(fps: u32) {
    link(0).set_cmd_te(fps, hs_clk(), 1344);
}

/// The DSIM's link registers, G#00..G#124, as the oracle's `DSIM 0 LINK SFR DUMP` lays them out, for diffing against it.
pub fn dump(out: &mut Vec<u8>) {
    for row in (0..0x128).step_by(0x20) {
        out.extend_from_slice(b"[");
        hex(out, row as u32);
        out.extend_from_slice(b"]");
        for col in (0..0x20).step_by(4) {
            if row + col >= 0x128 {
                break;
            }
            out.push(b' ');
            hex(out, rd(row + col));
        }
        out.push(b'\n');
    }
}

/// The DPHY registers in the stock dump's layout (dsim_reg.c __dphy_dump): BIAS at G#1946_0000, then PLL, clock lane (MC) and data lanes (MD0..3) at G#1946_0100 + the offsets shown.
pub fn dump_phy(out: &mut Vec<u8>) {
    const BIAS: usize = 0x1946_0000;
    const PHY: usize = 0x1946_0100;
    let mut block = |label: &[u8], base: usize, from: usize, len: usize| {
        out.extend_from_slice(label);
        out.push(b'\n');
        for row in (from..from + len).step_by(0x20) {
            out.extend_from_slice(b"[");
            hex(out, row as u32);
            out.extend_from_slice(b"]");
            for col in (0..0x20).step_by(4) {
                if row + col >= from + len {
                    break;
                }
                out.push(b' ');
                hex(out, unsafe { core::ptr::read_volatile((base + row + col) as *const u32) });
            }
            out.push(b'\n');
        }
    };
    block(b"-[BIAS]-", BIAS, 0, 0x14);
    block(b"-[PLL]-", PHY, 0, 0x44);
    block(b"-[MC]-", PHY, 0x200, 0x48);
    block(b"-[MC]-", PHY, 0x2E0, 0x10);
    for (i, lane) in [(0usize, &b"-[MD0]-"[..]), (1, b"-[MD1]-"), (2, b"-[MD2]-")].iter().map(|(i, l)| (*i, *l)) {
        block(lane, PHY, 0x300 + i * 0x100, 0x70);
        block(lane, PHY, 0x3E0 + i * 0xE0, 0x40);
    }
    block(b"-[MD3]-", PHY, 0x600, 0x48);
    block(b"-[MD3]-", PHY, 0x6C0, 0x20);
}

fn hex(out: &mut Vec<u8>, v: u32) {
    for i in (0..8).rev() {
        out.push(b"0123456789abcdef"[((v >> (i * 4)) & 0xF) as usize]);
    }
}
