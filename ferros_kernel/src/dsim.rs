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

fn hex(out: &mut Vec<u8>, v: u32) {
    for i in (0..8).rev() {
        out.push(b"0123456789abcdef"[((v >> (i * 4)) & 0xF) as usize]);
    }
}
