//! DSIM0: the MIPI DSI host that drives the panel (DISPLAY-HK3.md §7). The first thing ferros says to the panel itself: DCS packets through the DSIM's SFR packet FIFOs, exactly the bytes the oracle shows Android sending (oracle/display/husky-hk3/*/dsi-commands.txt).
//!
//! Command mode: frames go out only while the painter unmasks the TE trigger (display.rs `present`), so between presents the link is idle and a packet written to the FIFO transmits at once. Packet-go batching (CMD_CONFIG) is off, as Android leaves it. A short DCS write (type G#05 with no parameter, G#15 with one) is one header; a long write (G#39) or the PPS (G#0A) is the payload in 4-byte words, then the header carrying the length. Transmission is confirmed by polling FIFOCTRL's empty bits.

use alloc::vec::Vec;

const DSIM0: usize = 0x1944_0000;
const DSIM_CONFIG: usize = 0x004C;
const DSIM_INTSRC: usize = 0x0050;
const DSIM_PKTHDR: usize = 0x0058;
const DSIM_PAYLOAD: usize = 0x005C;
const DSIM_FIFOCTRL: usize = 0x0068;
const DSIM_CMD_CONFIG: usize = 0x0080;
const FIFOCTRL_EMPTY_PH_SFR: u32 = 1 << 10;
const FIFOCTRL_EMPTY_PL_SFR: u32 = 1 << 8;
const FIFOCTRL_FULL_PH_SFR: u32 = 1 << 11;
const FIFOCTRL_FULL_PL_SFR: u32 = 1 << 9;
const INTSRC_SFR_PH_FIFO_EMPTY: u32 = 1 << 28;
const INTSRC_SFR_PL_FIFO_EMPTY: u32 = 1 << 29;
const INTSRC_BTA_TOUT: u32 = 1 << 20;
const INTSRC_RX_DATA_DONE: u32 = 1 << 18;
const DSIM_RXFIFO: usize = 0x0060;
const FIFOCTRL_EMPTY_RX: u32 = 1 << 12;
const PKTHDR_BTA: u32 = 1 << 24;
/// DSI data types of the read request and the responses (MIPI DSI 1.3 §8.7, §8.10).
const SET_MAXIMUM_RETURN_PACKET_SIZE: u8 = 0x37;
pub const DCS_READ: u8 = 0x06;
const RX_ACK_AND_ERROR: u8 = 0x02;
const RX_END_OF_TRANSMISSION: u8 = 0x08;
const RX_GENERIC_SHORT_1: u8 = 0x11;
const RX_GENERIC_SHORT_2: u8 = 0x12;
const RX_GENERIC_LONG: u8 = 0x1A;
const RX_DCS_LONG: u8 = 0x1C;
const RX_DCS_SHORT_1: u8 = 0x21;
const RX_DCS_SHORT_2: u8 = 0x22;

/// DSI data types (MIPI DSI 1.3 §8.7).
pub const DCS_SHORT_WRITE_0: u8 = 0x05;
pub const DCS_SHORT_WRITE_1: u8 = 0x15;
pub const DCS_LONG_WRITE: u8 = 0x39;
pub const PPS_LONG_WRITE: u8 = 0x0A;

fn rd(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((DSIM0 + off) as *const u32) }
}

fn wr(off: usize, v: u32) {
    unsafe { core::ptr::write_volatile((DSIM0 + off) as *mut u32, v) }
}

/// Spin until `mask` is set in FIFOCTRL, up to about 20 ms; false on timeout.
fn wait_fifo(mask: u32, cntfrq: u64) -> bool {
    let start = now();
    while rd(DSIM_FIFOCTRL) & mask != mask {
        if now().wrapping_sub(start) > cntfrq / 50 {
            return false;
        }
    }
    true
}

fn now() -> u64 {
    let t: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t) };
    t
}

/// Whether the DSIM is up at all: its CONFIG reads back with data lanes enabled (Android or ABL left it running). With the DPU domain off a read returns 0; the caller must have checked the domain through EL3 first (display.rs).
pub fn alive() -> bool {
    let c = rd(DSIM_CONFIG);
    c != 0 && c & 0x1F != 0
}

/// Send one DSI packet of the given data type with the payload bytes (for a DCS packet the first byte is the command). Returns Err with the FIFOCTRL value when the FIFO never drains.
pub fn send(kind: u8, payload: &[u8], cntfrq: u64) -> Result<(), u32> {
    // Only ever write into an empty FIFO: the previous packet must have gone.
    if !wait_fifo(FIFOCTRL_EMPTY_PH_SFR | FIFOCTRL_EMPTY_PL_SFR, cntfrq) {
        return Err(rd(DSIM_FIFOCTRL));
    }
    wr(DSIM_INTSRC, INTSRC_SFR_PH_FIFO_EMPTY | INTSRC_SFR_PL_FIFO_EMPTY);
    let long = matches!(kind, DCS_LONG_WRITE | PPS_LONG_WRITE) || payload.len() > 2;
    if long {
        for chunk in payload.chunks(4) {
            if rd(DSIM_FIFOCTRL) & FIFOCTRL_FULL_PL_SFR != 0 {
                return Err(rd(DSIM_FIFOCTRL));
            }
            let mut w = 0u32;
            for (i, b) in chunk.iter().enumerate() {
                w |= (*b as u32) << (8 * i);
            }
            wr(DSIM_PAYLOAD, w);
        }
        let len = payload.len() as u32;
        wr(DSIM_PKTHDR, kind as u32 | (len & 0xFF) << 8 | (len >> 8 & 0xFF) << 16);
        if !wait_fifo(FIFOCTRL_EMPTY_PL_SFR | FIFOCTRL_EMPTY_PH_SFR, cntfrq) {
            return Err(rd(DSIM_FIFOCTRL));
        }
    } else {
        if rd(DSIM_FIFOCTRL) & FIFOCTRL_FULL_PH_SFR != 0 {
            return Err(rd(DSIM_FIFOCTRL));
        }
        let d0 = payload.first().copied().unwrap_or(0) as u32;
        let d1 = payload.get(1).copied().unwrap_or(0) as u32;
        wr(DSIM_PKTHDR, kind as u32 | d0 << 8 | d1 << 16);
        if !wait_fifo(FIFOCTRL_EMPTY_PH_SFR, cntfrq) {
            return Err(rd(DSIM_FIFOCTRL));
        }
    }
    Ok(())
}

/// A DCS write with the data type chosen from the length, the way the stock driver does (0 parameters G#05, 1 parameter G#15, more G#39).
pub fn dcs(bytes: &[u8], cntfrq: u64) -> Result<(), u32> {
    let kind = match bytes.len() {
        0 => return Ok(()),
        1 => DCS_SHORT_WRITE_0,
        2 => DCS_SHORT_WRITE_1,
        _ => DCS_LONG_WRITE,
    };
    send(kind, bytes, cntfrq)
}

/// A sequence of packets, each `[type][len][bytes…]`, with an optional wait in ms after each (the oracle's `wait N ms`): `[type][len][bytes…][wait_ms]`. Stops at the first failure and reports how many went.
pub fn sequence(script: &[u8], cntfrq: u64) -> (usize, Option<u32>) {
    let mut i = 0;
    let mut sent = 0;
    while i + 2 <= script.len() {
        let kind = script[i];
        let len = script[i + 1] as usize;
        if i + 2 + len + 1 > script.len() {
            break;
        }
        let payload = &script[i + 2..i + 2 + len];
        let wait_ms = script[i + 2 + len] as u64;
        if let Err(f) = send(kind, payload, cntfrq) {
            return (sent, Some(f));
        }
        sent += 1;
        if wait_ms > 0 {
            let start = now();
            while now().wrapping_sub(start) < cntfrq * wait_ms / 1000 {}
        }
        i += 2 + len + 1;
    }
    (sent, None)
}

/// A DCS read of `len` bytes (at most 64) from register `cmd`: the maximum-return-size packet, then the read request with bus turn-around, then the response out of the RX FIFO, the way the stock driver does it. Err carries INTSRC on a timeout or the unexpected RX header.
pub fn read(cmd: u8, len: usize, cntfrq: u64) -> Result<Vec<u8>, u32> {
    let len = len.clamp(1, 64);
    if !wait_fifo(FIFOCTRL_EMPTY_PH_SFR | FIFOCTRL_EMPTY_PL_SFR, cntfrq) {
        return Err(rd(DSIM_FIFOCTRL));
    }
    // Drain anything stale, then clear the done and timeout flags.
    while rd(DSIM_FIFOCTRL) & FIFOCTRL_EMPTY_RX == 0 {
        let _ = rd(DSIM_RXFIFO);
    }
    wr(DSIM_INTSRC, INTSRC_RX_DATA_DONE | INTSRC_BTA_TOUT | INTSRC_SFR_PH_FIFO_EMPTY);
    wr(DSIM_PKTHDR, SET_MAXIMUM_RETURN_PACKET_SIZE as u32 | (len as u32) << 8);
    wr(DSIM_PKTHDR, DCS_READ as u32 | (cmd as u32) << 8 | PKTHDR_BTA);
    if !wait_fifo(FIFOCTRL_EMPTY_PH_SFR, cntfrq) {
        return Err(rd(DSIM_FIFOCTRL));
    }
    // Wait for the response. The BTA timeout flag comes up early on this link while the turn-around and the data still follow (measured), so only the data's absence after 100 ms is a failure.
    let start = now();
    loop {
        let i = rd(DSIM_INTSRC);
        if i & INTSRC_RX_DATA_DONE != 0 {
            break;
        }
        if now().wrapping_sub(start) > cntfrq / 10 {
            return Err(i);
        }
    }
    let head = rd(DSIM_RXFIFO);
    let mut out = Vec::with_capacity(len);
    match (head & 0xFF) as u8 {
        RX_DCS_SHORT_2 | RX_GENERIC_SHORT_2 => {
            out.push((head >> 8) as u8);
            out.push((head >> 16) as u8);
        }
        RX_DCS_SHORT_1 | RX_GENERIC_SHORT_1 => out.push((head >> 8) as u8),
        RX_DCS_LONG | RX_GENERIC_LONG => {
            let n = ((head >> 8) & 0xFFFF) as usize;
            while out.len() < n.min(len) {
                let w = rd(DSIM_RXFIFO);
                for k in 0..4 {
                    if out.len() < n.min(len) {
                        out.push((w >> (8 * k)) as u8);
                    }
                }
            }
        }
        RX_ACK_AND_ERROR | RX_END_OF_TRANSMISSION => return Err(head),
        _ => return Err(head),
    }
    while rd(DSIM_FIFOCTRL) & FIFOCTRL_EMPTY_RX == 0 {
        let _ = rd(DSIM_RXFIFO);
    }
    Ok(out)
}

const DSIM_CMD_TE_CTRL0: usize = 0x0084;
const DSIM_CMD_TE_CTRL1: usize = 0x0088;
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
    const PHY: usize = 0x1946_0100;
    let con0 = unsafe { core::ptr::read_volatile(PHY as *const u32) };
    let k = unsafe { core::ptr::read_volatile((PHY + 4) as *const u32) } & 0xFFFF;
    let m = unsafe { core::ptr::read_volatile((PHY + 8) as *const u32) } & 0x3FF;
    let p = (con0 & 0x3F).max(1);
    let s = (con0 >> 8) & 0x7;
    let k = if k & 0x8000 != 0 { k as i64 - 0x1_0000 } else { k as i64 };
    // In kHz with the fraction kept: ((M << 16) + K) × 24576 × 2 / P / 2^S / 2^16.
    let khz = (((m as i64) << 16) + k) * 24_576 * 2 / p as i64 / (1i64 << s) >> 16;
    ((khz + 500) / 1000) as u32
}

pub fn set_hs_clk(mbps: u32) {
    HS_CLK_MBPS.store(mbps, core::sync::atomic::Ordering::Relaxed);
}

/// The command-mode TE timers for a refresh rate, the stock driver's formula (dsim_reg_get_cmd_timer: TE protect = hs × 95 × 100 / fps / 16; TE timeout = hs × 110 × 100 / fps / 16, integer division in that order; stable VFP = hactive × 2 / 100 with DSC). Reproduces Android's G#1A03_1E1F at 120 Hz with its 1346 Mbps link.
pub fn set_cmd_te(fps: u32) {
    let hs = hs_clk();
    let protect = hs * 95 * 100 / fps / 16;
    let timeout = hs * 110 * 100 / fps / 16;
    wr(DSIM_CMD_TE_CTRL0, 1344 * 2 / 100);
    wr(DSIM_CMD_TE_CTRL1, (protect & 0xFFFF) << 16 | (timeout & 0xFFFF));
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
