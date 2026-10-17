//! The DSIM (Exynos MIPI DSI master) command path, written once for two callers: the kernel's display bench reaches the registers directly, the display server through its `dsim0` hardware grant (one syscall per access). Both implement [`Link`]; everything else is here. The register map is cal_9865 regs-dsim.h; the sequences in [`hk3`] are the panel driver's, byte for byte (DISPLAY-HK3.md, `oracle/display/husky-hk3/`).
//!
//! Reads work in HS on this link; the BTA timeout flag comes up early while the data still follows, so only the data's absence after 100 ms is a failure (measured 2026-10-16).

#![no_std]

pub mod hk3;

/// Register access to one DSIM block (offsets from its base) and a clock to wait with.
pub trait Link {
    fn rd(&self, off: usize) -> u32;
    fn wr(&self, off: usize, v: u32);
    /// The generic timer count and its frequency.
    fn now(&self) -> u64;
    fn freq(&self) -> u64;
}

pub const DSIM_CONFIG: usize = 0x004C;
pub const DSIM_INTSRC: usize = 0x0050;
pub const DSIM_PKTHDR: usize = 0x0058;
pub const DSIM_PAYLOAD: usize = 0x005C;
pub const DSIM_RXFIFO: usize = 0x0060;
pub const DSIM_FIFOCTRL: usize = 0x0068;
pub const DSIM_CMD_CONFIG: usize = 0x0080;
pub const DSIM_CMD_TE_CTRL0: usize = 0x0084;
pub const DSIM_CMD_TE_CTRL1: usize = 0x0088;
const FIFOCTRL_EMPTY_PH_SFR: u32 = 1 << 10;
const FIFOCTRL_EMPTY_PL_SFR: u32 = 1 << 8;
const FIFOCTRL_FULL_PH_SFR: u32 = 1 << 11;
const FIFOCTRL_FULL_PL_SFR: u32 = 1 << 9;
const FIFOCTRL_EMPTY_RX: u32 = 1 << 12;
const INTSRC_SFR_PH_FIFO_EMPTY: u32 = 1 << 28;
const INTSRC_SFR_PL_FIFO_EMPTY: u32 = 1 << 29;
const INTSRC_BTA_TOUT: u32 = 1 << 20;
const INTSRC_RX_DATA_DONE: u32 = 1 << 18;
const PKTHDR_BTA: u32 = 1 << 24;

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

/// DSI data types the panel driver uses.
pub const DCS_SHORT_WRITE_0: u8 = 0x05;
pub const DCS_SHORT_WRITE_1: u8 = 0x15;
pub const DCS_LONG_WRITE: u8 = 0x39;
pub const PPS_LONG_WRITE: u8 = 0x0A;

/// The DSIM behind a [`Link`].
pub struct Dsim<L: Link>(pub L);

impl<L: Link> Dsim<L> {
    fn wait_fifo(&self, mask: u32) -> bool {
        let start = self.0.now();
        while self.0.rd(DSIM_FIFOCTRL) & mask != mask {
            if self.0.now().wrapping_sub(start) > self.0.freq() / 50 {
                return false;
            }
        }
        true
    }

    pub fn delay_ms(&self, ms: u64) {
        let start = self.0.now();
        while self.0.now().wrapping_sub(start) < self.0.freq() * ms / 1000 {}
    }

    /// Whether the DSIM is up at all: its CONFIG reads back with data lanes enabled. With the DPU domain off a read returns 0; the caller checks the domain first.
    pub fn alive(&self) -> bool {
        let c = self.0.rd(DSIM_CONFIG);
        c != 0 && c & 0x1F != 0
    }

    /// Send one DSI packet of the given data type with the payload bytes (for a DCS packet the first byte is the command). Err carries FIFOCTRL when the FIFO never drains.
    pub fn send(&self, kind: u8, payload: &[u8]) -> Result<(), u32> {
        if !self.wait_fifo(FIFOCTRL_EMPTY_PH_SFR | FIFOCTRL_EMPTY_PL_SFR) {
            return Err(self.0.rd(DSIM_FIFOCTRL));
        }
        self.0.wr(DSIM_INTSRC, INTSRC_SFR_PH_FIFO_EMPTY | INTSRC_SFR_PL_FIFO_EMPTY);
        let long = matches!(kind, DCS_LONG_WRITE | PPS_LONG_WRITE) || payload.len() > 2;
        if long {
            for chunk in payload.chunks(4) {
                if self.0.rd(DSIM_FIFOCTRL) & FIFOCTRL_FULL_PL_SFR != 0 {
                    return Err(self.0.rd(DSIM_FIFOCTRL));
                }
                let mut w = 0u32;
                for (i, b) in chunk.iter().enumerate() {
                    w |= (*b as u32) << (8 * i);
                }
                self.0.wr(DSIM_PAYLOAD, w);
            }
            let len = payload.len() as u32;
            self.0.wr(DSIM_PKTHDR, kind as u32 | (len & 0xFF) << 8 | (len >> 8 & 0xFF) << 16);
            if !self.wait_fifo(FIFOCTRL_EMPTY_PL_SFR | FIFOCTRL_EMPTY_PH_SFR) {
                return Err(self.0.rd(DSIM_FIFOCTRL));
            }
        } else {
            if self.0.rd(DSIM_FIFOCTRL) & FIFOCTRL_FULL_PH_SFR != 0 {
                return Err(self.0.rd(DSIM_FIFOCTRL));
            }
            let d0 = payload.first().copied().unwrap_or(0) as u32;
            let d1 = payload.get(1).copied().unwrap_or(0) as u32;
            self.0.wr(DSIM_PKTHDR, kind as u32 | d0 << 8 | d1 << 16);
            if !self.wait_fifo(FIFOCTRL_EMPTY_PH_SFR) {
                return Err(self.0.rd(DSIM_FIFOCTRL));
            }
        }
        Ok(())
    }

    /// A DCS write with the data type chosen from the length, as the stock driver does (0 parameters G#05, 1 parameter G#15, more G#39).
    pub fn dcs(&self, bytes: &[u8]) -> Result<(), u32> {
        let kind = match bytes.len() {
            0 => return Ok(()),
            1 => DCS_SHORT_WRITE_0,
            2 => DCS_SHORT_WRITE_1,
            _ => DCS_LONG_WRITE,
        };
        self.send(kind, bytes)
    }

    /// A sequence of packets, each `[type][len][bytes…][wait_ms]` (the oracle's `wait N ms` after it). Stops at the first failure: (packets sent, the failure).
    pub fn sequence(&self, script: &[u8]) -> (usize, Option<u32>) {
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
            if let Err(f) = self.send(kind, payload) {
                return (sent, Some(f));
            }
            sent += 1;
            if wait_ms > 0 {
                self.delay_ms(wait_ms);
            }
            i += 2 + len + 1;
        }
        (sent, None)
    }

    /// A DCS read of up to `out.len()` bytes (at most 64) from register `cmd`: the maximum-return-size packet, the read with bus turn-around, the response from the RX FIFO. Returns how many bytes landed in `out`; Err carries INTSRC on a timeout or the unexpected RX header.
    pub fn read(&self, cmd: u8, out: &mut [u8]) -> Result<usize, u32> {
        let len = out.len().clamp(1, 64);
        if !self.wait_fifo(FIFOCTRL_EMPTY_PH_SFR | FIFOCTRL_EMPTY_PL_SFR) {
            return Err(self.0.rd(DSIM_FIFOCTRL));
        }
        while self.0.rd(DSIM_FIFOCTRL) & FIFOCTRL_EMPTY_RX == 0 {
            let _ = self.0.rd(DSIM_RXFIFO);
        }
        self.0.wr(DSIM_INTSRC, INTSRC_RX_DATA_DONE | INTSRC_BTA_TOUT | INTSRC_SFR_PH_FIFO_EMPTY);
        self.0.wr(DSIM_PKTHDR, SET_MAXIMUM_RETURN_PACKET_SIZE as u32 | (len as u32) << 8);
        self.0.wr(DSIM_PKTHDR, DCS_READ as u32 | (cmd as u32) << 8 | PKTHDR_BTA);
        if !self.wait_fifo(FIFOCTRL_EMPTY_PH_SFR) {
            return Err(self.0.rd(DSIM_FIFOCTRL));
        }
        let start = self.0.now();
        loop {
            let i = self.0.rd(DSIM_INTSRC);
            if i & INTSRC_RX_DATA_DONE != 0 {
                break;
            }
            if self.0.now().wrapping_sub(start) > self.0.freq() / 10 {
                return Err(i);
            }
        }
        let head = self.0.rd(DSIM_RXFIFO);
        let mut n = 0usize;
        let mut push = |b: u8, n: &mut usize| {
            if *n < len {
                out[*n] = b;
                *n += 1;
            }
        };
        match (head & 0xFF) as u8 {
            RX_DCS_SHORT_2 | RX_GENERIC_SHORT_2 => {
                push((head >> 8) as u8, &mut n);
                push((head >> 16) as u8, &mut n);
            }
            RX_DCS_SHORT_1 | RX_GENERIC_SHORT_1 => push((head >> 8) as u8, &mut n),
            RX_DCS_LONG | RX_GENERIC_LONG => {
                let total = ((head >> 8) & 0xFFFF) as usize;
                let want = total.min(len);
                while n < want {
                    let w = self.0.rd(DSIM_RXFIFO);
                    for k in 0..4 {
                        if n < want {
                            push((w >> (8 * k)) as u8, &mut n);
                        }
                    }
                }
            }
            RX_ACK_AND_ERROR | RX_END_OF_TRANSMISSION => return Err(head),
            _ => return Err(head),
        }
        while self.0.rd(DSIM_FIFOCTRL) & FIFOCTRL_EMPTY_RX == 0 {
            let _ = self.0.rd(DSIM_RXFIFO);
        }
        Ok(n)
    }

    /// The command-mode TE timers for a refresh rate at a link rate (dsim_reg_get_cmd_timer: protect = hs × 95 × 100 / fps / 16, timeout = hs × 110 × 100 / fps / 16, integer division in that order; stable VFP = hactive × 2 / 100 with DSC). Reproduces Android's G#1A03_1E1F at 120 Hz, 1346 Mbps.
    pub fn set_cmd_te(&self, fps: u32, hs_mbps: u32, hactive: u32) {
        let protect = hs_mbps * 95 * 100 / fps / 16;
        let timeout = hs_mbps * 110 * 100 / fps / 16;
        self.0.wr(DSIM_CMD_TE_CTRL0, hactive * 2 / 100);
        self.0.wr(DSIM_CMD_TE_CTRL1, (protect & 0xFFFF) << 16 | (timeout & 0xFFFF));
    }
}

/// The link rate the DPHY PLL is programmed for, from its first three words (PLL_CON0: P [5:0], S [10:8]; PLL_CON1: K two's complement; PLL_CON2: M): hs = (M + K / 2¹⁶) × 24.576 MHz / P / 2^S × 2, rounded to the Mbps the driver's tables use. `rd_phy` reads the PLL block (G#1946_0100 on husky) at a byte offset.
pub fn pll_hs_clk(rd_phy: impl Fn(usize) -> u32) -> u32 {
    let con0 = rd_phy(0);
    let k = rd_phy(4) & 0xFFFF;
    let m = rd_phy(8) & 0x3FF;
    let p = (con0 & 0x3F).max(1);
    let s = (con0 >> 8) & 0x7;
    let k = if k & 0x8000 != 0 { k as i64 - 0x1_0000 } else { k as i64 };
    let khz = (((m as i64) << 16) + k) * 24_576 * 2 / p as i64 / (1i64 << s) >> 16;
    ((khz + 500) / 1000) as u32
}
