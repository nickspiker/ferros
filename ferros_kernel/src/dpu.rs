//! The display pipe under ferros's own control: DECON0 and DSIM0 stopped, re-initialised and started the way the stock driver does for a resolution change (DISPLAY-MODESET.md), on a pipe ABL left running. A one-frame live rewrite of the geometry registers wedges the hardware (DISPLAY.md); the modeset is the way.
//!
//! Everything the sequence does not set is snapshotted before the resets and written back after them, so the configuration ABL (or a previous modeset) proved comes back word for word — the DPHY's bias, PLL and lane tuning, DECON's blender/QoS/SRAM/trigger fields, the DSC encoders — and only the raster-dependent words change: DECON's image and out-FIFO sizes, the DSC group count and PPS, the layer's geometry, DSIM's resolution, compressed width, slice widths and timers.

use crate::display::{dpu_on, GPP0_DAT};
use alloc::vec::Vec;

const DECON0: usize = 0x1947_0000;
const DECON_WIN: usize = 0x1948_0000;
const DECON_SUB: usize = 0x1949_0000;
const DECON_WINCON: usize = 0x194A_0000;
const DSIM0: usize = 0x1944_0000;
const DPHY_BIAS: usize = 0x1946_0000;
const DPHY: usize = 0x1946_0100;
const DPU_SYSREG: usize = 0x1942_1000;
const DMA0: usize = 0x1990_0000;
const DPP0: usize = 0x1993_0000;
const SRAMC0: usize = 0x1995_0000;
const HDRCOMM0: usize = 0x1996_0000;
const DQE0: usize = 0x194D_0000;
const DQE_CGC0: usize = 0x194F_0000;
/// The base window ABL uses, on layer 0.
const WIN: usize = 5;

// DECON main (regs-decon.h)
const GLOBAL_CON: usize = 0x0020;
const GLOBAL_CON_SRESET: u32 = 1 << 28;
const GLOBAL_CON_RUN_STATUS: u32 = 1 << 4;
const GLOBAL_CON_DECON_EN: u32 = 1 << 1;
const GLOBAL_CON_DECON_EN_F: u32 = 1 << 0;
/// DECON blends at 10 bits per component (decon_reg_set_bpc). Needs the DQE in the enhance path with its display dither on, or the first frame times out and DECON never recovers (measured 2026-10-16).
const GLOBAL_CON_TEN_BPC: u32 = 1 << 20;
const DATA_PATH_CON_0: usize = 0x0200;
const ENHANCE_PATH_MASK: u32 = 0xF << 12;
const ENHANCE_DITHER_ON: u32 = 1 << 12;
const ENHANCE_DQE_ON: u32 = 2 << 12;
const SHD_REG_UP_REQ_DQE: u32 = 1 << 24;
// DQE (regs-dqe.h): the top block's sizes, the display dither, the ATC and CGC enables.
const DQE_TOP_IMG_SIZE: usize = 0x0004;
const DQE_TOP_FRM_SIZE: usize = 0x0008;
const DQE_TOP_FRM_PXL_NUM: usize = 0x000C;
const DQE_ATC_CONTROL: usize = 0x0400;
const DQE_CGC_DITHER: usize = 0x2400;
const DQE_DISP_DITHER: usize = 0x2800;
const DITHER_EN: u32 = 1 << 0;
const DQE_CGC_CON: usize = 0x0000;
// The layer's 10 bpc mode bits (dpp_reg_set_bpc, hdrc_reg_set_bpc_mode).
const DPP_COM_IO_CON: usize = 0x0038;
const LSI_COMM_IO_CON: usize = 0x000C;
const BPC_MODE_10: u32 = 1 << 6;
const TRIG_CON: usize = 0x0030;
const HW_TRIG_EN: u32 = 1 << 0;
const HW_TRIG_MASK_DECON: u32 = 1 << 4;
const SHD_REG_UP_REQ: usize = 0x0050;
const SHD_GLOBAL: u32 = 1 << 31;
const SHD_CMP: u32 = 1 << 20;
const SHD_ALL_WINDOWS: u32 = 0x3FFF;
const DECON_INT_PEND: usize = 0x0070;
const DECON_INT_PEND_EXTRA: usize = 0x0074;
const INT_PEND_FRAME_START: u32 = 1 << 12;
const BLD_BG_IMG_SIZE_PRI: usize = 0x0220;
const OF_SIZE_0: usize = 0x0290;
const OF_SIZE_1: usize = 0x0294;
const OF_SIZE_2: usize = 0x0298;
const OF_TH_TYPE: usize = 0x029C;
// DECON sub: the DSC encoders and the DSIM interface
const DSC_CONTROL3: usize = 0x000C;
const DSC_PPS: usize = 0x0040;
const DSIMIF_SEL0: usize = 0x8000;
// DSIM link (regs-dsim.h)
const DSIM_SWRST: usize = 0x0004;
const DSIM_DPHY_STATUS: usize = 0x001C;
const STOPSTATE_CLK: u32 = 1 << 8;
const DSIM_CLK_CTRL: usize = 0x0020;
const CLK_CLOCK_SEL: u32 = 1 << 26;
const CLK_NONCONT: u32 = 1 << 25;
const CLK_TX_REQUEST_HSCLK: u32 = 1 << 20;
const CLK_WORDCLK_EN: u32 = 1 << 17;
const CLK_ESCCLK_EN: u32 = 1 << 16;
const CLK_LANE_ESCCLK_EN: u32 = 0x1F << 8;
const CLK_ESC_PRESCALER_MASK: u32 = 0xFF;
const DSIM_DESKEW_CTRL: usize = 0x0024;
const DESKEW_HW_EN: u32 = 1 << 15;
const DSIM_TIMEOUT: usize = 0x0028;
const DSIM_ESCMODE: usize = 0x002C;
const DSIM_NUM_OF_TRANSFER: usize = 0x0030;
const DSIM_UNDERRUN_CTRL: usize = 0x0034;
const DSIM_THRESHOLD: usize = 0x0038;
const DSIM_RESOL: usize = 0x003C;
const DSIM_CONFIG: usize = 0x004C;
const CONFIG_LANES_EN: u32 = 0x1F;
/// CONFIG as the working link runs it (both rasters): 4 data lanes, EOTP, pixel format G#3E, compression on.
const CONFIG_RUNNING: u32 = 0x0488_7CFF;
const DSIM_INTSRC: usize = 0x0050;
const DSIM_INTMSK: usize = 0x0054;
const DSIM_SFR_CTRL: usize = 0x0064;
const SFR_SHADOW_EN: u32 = 1 << 0;
const SFR_SHADOW_READ_EN: u32 = 1 << 1;
const DSIM_CPRS_CTRL: usize = 0x0074;
const DSIM_SLICE01: usize = 0x0078;
const DSIM_CMD_CONFIG: usize = 0x0080;
const DSIM_CMD_TE_CTRL0: usize = 0x0084;
const DSIM_CMD_TE_CTRL1: usize = 0x0088;
const DSIM_OPTION_SUITE: usize = 0x010C;
const OPT_TE_ON_CMD_ALLOW: u32 = 1 << 10;
// DPHY (regs-dsim.h, offsets from DPHY)
const PLL_CON0: usize = 0x0000;
const PLL_EN: u32 = 1 << 12;
const PLL_STAT0: usize = 0x0040;
const PLL_LOCK: u32 = 1 << 0;
const MC_GNR_CON0: usize = 0x0200;
const fn md_gnr_con0(lane: usize) -> usize {
    0x0300 + 0x100 * lane
}
const PHY_ENABLE: u32 = 1 << 0;
const PHY_READY: u32 = 1 << 1;
/// A link rate: the DPHY words that differ between the two rates the stock stack knows (DISPLAY-HK3.md §7) and the panel's FFC table for it. The PLL's PMSK and lock count, MC_ANA_CON2, and the clock and data lanes' timing words for the rate's row of the driver's table; 1346 from Android's live dump, 1368 from the state ABL leaves.
pub struct LinkRate {
    pub mbps: u32,
    phy: &'static [(usize, u32)],
    ffc: &'static [u8],
}

pub const LINK_1368: LinkRate = LinkRate {
    mbps: 1368,
    phy: &[(0x0000, 0x1201), (0x0004, 0x5400), (0x0008, 0x006F), (0x001C, 0x01F4), (0x0210, 2), (0x0230, 0x1290), (0x0234, 0x1536), (0x0238, 0x0842), (0x0240, 0x01A9), (0x0330, 0x1290), (0x0334, 0x0836), (0x0338, 0x0843), (0x0340, 0x01A9), (0x0430, 0x1290), (0x0434, 0x0836), (0x0438, 0x0843), (0x0440, 0x01A9), (0x0530, 0x1290), (0x0534, 0x0836), (0x0538, 0x0843), (0x0540, 0x01A9), (0x0630, 0x1290), (0x0634, 0x0836), (0x0638, 0x0843), (0x0640, 0x01A9)],
    ffc: crate::hk3::FFC_1368,
};

pub const LINK_1346: LinkRate = LinkRate {
    mbps: 1346,
    phy: &[(0x0000, 0x1202), (0x0004, 0x1355), (0x0008, 0x00DB), (0x001C, 0x03E8), (0x0210, 2), (0x0230, 0x1280), (0x0234, 0x1535), (0x0238, 0x0841), (0x0240, 0x0190), (0x0330, 0x1280), (0x0334, 0x0835), (0x0338, 0x0842), (0x0340, 0x0190), (0x0430, 0x1280), (0x0434, 0x0835), (0x0438, 0x0842), (0x0440, 0x0190), (0x0530, 0x1280), (0x0534, 0x0835), (0x0538, 0x0842), (0x0540, 0x0190), (0x0630, 0x1280), (0x0634, 0x0835), (0x0638, 0x0842), (0x0640, 0x0190)],
    ffc: crate::hk3::FFC_1346,
};

// DPU sysreg
const DISP_DPU_MIPI_PHY_CON: usize = 0x0008;
const M_RESETN_M0: u32 = 1 << 0;
const SEL_RESET_DPHY0: u32 = 1 << 4;

fn rd(a: usize) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}

fn wr(a: usize, v: u32) {
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}

fn now() -> u64 {
    let t: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t) };
    t
}

/// Spin until `(rd(a) & mask) == want`, up to `us` microseconds; false on timeout.
fn poll(a: usize, mask: u32, want: u32, us: u64, cntfrq: u64) -> bool {
    let start = now();
    while rd(a) & mask != want {
        if now().wrapping_sub(start) > cntfrq * us / 1_000_000 {
            return false;
        }
    }
    true
}

fn wait_us(us: u64, cntfrq: u64) {
    let start = now();
    while now().wrapping_sub(start) < cntfrq * us / 1_000_000 {}
}

fn snapshot(base: usize, from: usize, len: usize) -> Vec<u32> {
    (from..from + len).step_by(4).map(|o| rd(base + o)).collect()
}

fn restore(base: usize, from: usize, words: &[u32], skip: &[usize]) {
    for (i, &w) in words.iter().enumerate() {
        let off = from + 4 * i;
        if !skip.contains(&off) {
            wr(base + off, w);
        }
    }
}

/// The geometry of one raster, as DECON and DSIM need it.
struct Raster {
    cfg: ferros_dsc::Config,
    w: u32,
    h: u32,
    /// Compressed slice width in 3-byte groups.
    comp: u32,
    /// DSIM's transfer threshold: the compressed slice width aligned to 4, times two slices.
    threshold: u32,
    underrun: u32,
}

fn raster(fhd: bool, link10: bool) -> Raster {
    let cfg = if fhd { ferros_dsc::HUSKY_FHD } else { ferros_dsc::HUSKY_WQHD };
    let cfg = if link10 { ferros_dsc::ten_bpc(cfg) } else { cfg };
    let sw = cfg.slice_width as u32;
    let comp = sw / 3;
    let threshold = (sw.div_ceil(3) + 3) / 4 * 4 * 2;
    Raster { cfg, w: cfg.pic_width as u32, h: cfg.pic_height as u32, comp, threshold, underrun: if fhd { 0x0ED8 } else { 0x063A } }
}

/// DECON stop with reset (decon_reg_stop, DISPLAY-MODESET.md).
fn decon_stop(cntfrq: u64, rep: &mut Vec<u8>) {
    let t = rd(DECON0 + TRIG_CON);
    wr(DECON0 + TRIG_CON, (t | HW_TRIG_MASK_DECON) & !HW_TRIG_EN);
    wr(DECON0 + GLOBAL_CON, rd(DECON0 + GLOBAL_CON) & !GLOBAL_CON_DECON_EN_F);
    wr(DECON0 + SHD_REG_UP_REQ, rd(DECON0 + SHD_REG_UP_REQ) | SHD_GLOBAL | SHD_CMP);
    if !poll(DECON0 + GLOBAL_CON, GLOBAL_CON_RUN_STATUS, 0, 12_500, cntfrq) {
        rep.extend_from_slice(b"decon: per-frame stop timed out, instant off\n");
        wr(DECON0 + GLOBAL_CON, rd(DECON0 + GLOBAL_CON) & !(GLOBAL_CON_DECON_EN | GLOBAL_CON_DECON_EN_F));
        wr(DECON0 + SHD_REG_UP_REQ, rd(DECON0 + SHD_REG_UP_REQ) | SHD_GLOBAL | SHD_CMP);
        let _ = poll(DECON0 + GLOBAL_CON, GLOBAL_CON_RUN_STATUS, 0, 12_500, cntfrq);
    }
    wr(DECON0 + GLOBAL_CON, rd(DECON0 + GLOBAL_CON) | GLOBAL_CON_SRESET);
    if !poll(DECON0 + GLOBAL_CON, GLOBAL_CON_SRESET, 0, 2_000, cntfrq) {
        rep.extend_from_slice(b"decon: SRESET did not clear\n");
    }
    wr(DECON0 + DECON_INT_PEND, (1 << 21) | (1 << 20) | (1 << 13) | (1 << 12));
    wr(DECON0 + DECON_INT_PEND_EXTRA, (1 << 4) | 1);
}

/// DSIM stop (dsim_reg_stop): HS clock off, OSC, DPHY reset, lanes off, escape clock off, PLL off, software reset.
fn dsim_stop(cntfrq: u64, rep: &mut Vec<u8>) {
    wr(DSIM0 + DSIM_INTSRC, !0);
    wr(DSIM0 + DSIM_INTMSK, !0);
    wr(DSIM0 + DSIM_CLK_CTRL, rd(DSIM0 + DSIM_CLK_CTRL) & !CLK_TX_REQUEST_HSCLK);
    if !poll(DSIM0 + DSIM_DPHY_STATUS, STOPSTATE_CLK, STOPSTATE_CLK, 2_000, cntfrq) {
        rep.extend_from_slice(b"dsim: clock lane never reached stop state\n");
    }
    wr(DSIM0 + DSIM_CLK_CTRL, rd(DSIM0 + DSIM_CLK_CTRL) & !CLK_CLOCK_SEL);
    wr(DPU_SYSREG + DISP_DPU_MIPI_PHY_CON, rd(DPU_SYSREG + DISP_DPU_MIPI_PHY_CON) & !M_RESETN_M0);
    for lane in [MC_GNR_CON0, md_gnr_con0(0), md_gnr_con0(1), md_gnr_con0(2), md_gnr_con0(3)] {
        wr(DPHY + lane, rd(DPHY + lane) & !PHY_ENABLE);
        let _ = poll(DPHY + lane, PHY_READY, 0, 2_000, cntfrq);
    }
    let c = rd(DSIM0 + DSIM_CLK_CTRL) & !(CLK_LANE_ESCCLK_EN | CLK_ESCCLK_EN | CLK_ESC_PRESCALER_MASK);
    wr(DSIM0 + DSIM_CLK_CTRL, c | 0xFF);
    wr(DPHY + PLL_CON0, rd(DPHY + PLL_CON0) & !PLL_EN);
    if !poll(DPHY + PLL_STAT0, PLL_LOCK, 0, 2_000, cntfrq) {
        rep.extend_from_slice(b"dsim: PLL did not unlock\n");
    }
    wr(DSIM0 + DSIM_SWRST, 1);
    if !poll(DSIM0 + DSIM_SWRST, 1, 0, 2_000, cntfrq) {
        rep.extend_from_slice(b"dsim: SWRST did not clear\n");
    }
}

/// DSIM init + start (dsim_reg_init, dsim_reg_start) for a raster, the DPHY's proven words restored around the PLL re-lock.
fn dsim_init(r: &Raster, link: &[u32], bias: &[u32], phy: &[u32], rate: Option<&LinkRate>, cntfrq: u64, rep: &mut Vec<u8>) {
    // DPHY reset under sysreg control, OSC clock, software reset.
    wr(DPU_SYSREG + DISP_DPU_MIPI_PHY_CON, rd(DPU_SYSREG + DISP_DPU_MIPI_PHY_CON) & !SEL_RESET_DPHY0);
    wr(DSIM0 + DSIM_CLK_CTRL, rd(DSIM0 + DSIM_CLK_CTRL) & !CLK_CLOCK_SEL);
    wr(DSIM0 + DSIM_SWRST, 1);
    let _ = poll(DSIM0 + DSIM_SWRST, 1, 0, 2_000, cntfrq);
    // The link's proven configuration back, except what the sequence orders explicitly and what is status or FIFO.
    restore(DSIM0, 0, link, &[DSIM_SWRST, DSIM_DPHY_STATUS, DSIM_CLK_CTRL, DSIM_CONFIG, DSIM_INTSRC, DSIM_INTMSK, 0x0058, 0x005C, 0x0060, DSIM_SFR_CTRL, 0x0068]);
    wr(DSIM0 + DSIM_CONFIG, CONFIG_LANES_EN);
    wr(DSIM0 + DSIM_CLK_CTRL, CLK_LANE_ESCCLK_EN | CLK_WORDCLK_EN | CLK_ESCCLK_EN | 5);
    // DPHY reset asserted while the PLL and lanes come up.
    wr(DPU_SYSREG + DISP_DPU_MIPI_PHY_CON, rd(DPU_SYSREG + DISP_DPU_MIPI_PHY_CON) & !M_RESETN_M0);
    restore(DPHY_BIAS, 0, bias, &[]);
    let mut phy = Vec::from(phy);
    if let Some(lr) = rate {
        for &(off, w) in lr.phy {
            phy[off / 4] = w;
        }
        crate::dsim::set_hs_clk(lr.mbps);
    }
    for (i, &w) in phy.iter().enumerate() {
        let off = 4 * i;
        let w = match off {
            PLL_CON0 => w & !PLL_EN,
            o if o == MC_GNR_CON0 || o == md_gnr_con0(0) || o == md_gnr_con0(1) || o == md_gnr_con0(2) || o == md_gnr_con0(3) => w & !PHY_ENABLE,
            PLL_STAT0 => continue,
            _ => w,
        };
        wr(DPHY + off, w);
    }
    wr(DSIM0 + DSIM_INTSRC, 1 << 31);
    wr(DPHY + PLL_CON0, rd(DPHY + PLL_CON0) | PLL_EN);
    if !poll(DPHY + PLL_STAT0, PLL_LOCK, PLL_LOCK, 2_000, cntfrq) {
        rep.extend_from_slice(b"dsim: PLL did not lock\n");
    }
    for lane in [MC_GNR_CON0, md_gnr_con0(0), md_gnr_con0(1), md_gnr_con0(2), md_gnr_con0(3)] {
        wr(DPHY + lane, rd(DPHY + lane) | PHY_ENABLE);
        if !poll(DPHY + lane, PHY_READY, PHY_READY, 2_000, cntfrq) {
            rep.extend_from_slice(b"dsim: a lane never became ready\n");
        }
    }
    wr(DPU_SYSREG + DISP_DPU_MIPI_PHY_CON, rd(DPU_SYSREG + DISP_DPU_MIPI_PHY_CON) | M_RESETN_M0);
    wr(DSIM0 + DSIM_CLK_CTRL, rd(DSIM0 + DSIM_CLK_CTRL) | CLK_CLOCK_SEL | CLK_NONCONT);
    // Configuration (dsim_reg_set_config), geometry from the raster.
    wr(DSIM0 + DSIM_SFR_CTRL, SFR_SHADOW_READ_EN);
    wr(DSIM0 + DSIM_DESKEW_CTRL, rd(DSIM0 + DSIM_DESKEW_CTRL) & !DESKEW_HW_EN);
    wr(DSIM0 + DSIM_TIMEOUT, 0x00FF_FFFF);
    wr(DSIM0 + DSIM_ESCMODE, 0xA << 21);
    wr(DSIM0 + DSIM_UNDERRUN_CTRL, r.underrun);
    wr(DSIM0 + DSIM_OPTION_SUITE, rd(DSIM0 + DSIM_OPTION_SUITE) | OPT_TE_ON_CMD_ALLOW);
    wr(DSIM0 + DSIM_THRESHOLD, r.threshold);
    wr(DSIM0 + DSIM_RESOL, r.h << 16 | r.threshold);
    wr(DSIM0 + DSIM_NUM_OF_TRANSFER, r.h);
    wr(DSIM0 + DSIM_CONFIG, CONFIG_RUNNING);
    wr(DSIM0 + DSIM_SFR_CTRL, SFR_SHADOW_READ_EN | SFR_SHADOW_EN);
    // The stock driver sets command mode and compression after the shadow is on; written before it, the compression bit did not reach the applied register (measured: CONFIG read back G#0480_7CFF).
    wr(DSIM0 + DSIM_CONFIG, CONFIG_RUNNING);
    wr(DSIM0 + DSIM_CPRS_CTRL, 0xA);
    let sw = r.cfg.slice_width as u32;
    wr(DSIM0 + DSIM_SLICE01, sw << 16 | sw);
    wr(DSIM0 + DSIM_CMD_CONFIG, 0);
    wr(DSIM0 + DSIM_CMD_TE_CTRL0, r.w * 2 / 100);
    let hs = crate::dsim::hs_clk();
    wr(DSIM0 + DSIM_CMD_TE_CTRL1, (hs * 95 * 100 / 120 / 16) << 16 | (hs * 110 * 100 / 120 / 16));
    // Start: request the HS clock, interrupts as they were, status clear.
    wr(DSIM0 + DSIM_CLK_CTRL, rd(DSIM0 + DSIM_CLK_CTRL) | CLK_TX_REQUEST_HSCLK);
    wr(DSIM0 + DSIM_INTMSK, link[DSIM_INTMSK / 4]);
    wr(DSIM0 + DSIM_INTSRC, !0);
}

/// DECON init for a raster (decon_reg_init) on top of the restored snapshot: geometry, the DSC encoders, the DSIM interface select, the out-FIFO.
fn decon_init(r: &Raster, main: &[u32], sub0: &[u32], sub1: &[u32], win: &[u32], wincon: u32, ten_bpc: bool, link10: bool) {
    restore(DECON0, 0, main, &[0x0004, GLOBAL_CON, 0x0038, 0x003C, SHD_REG_UP_REQ, DECON_INT_PEND, DECON_INT_PEND_EXTRA, 0x0100, 0x0104, 0x0108, 0x010C, 0x0110, 0x0114, 0x0118, 0x011C, 0x0120, 0x0124]);
    wr(DECON0 + GLOBAL_CON, 1 << 8 | if ten_bpc { GLOBAL_CON_TEN_BPC } else { 0 });
    // 10 bpc the way Android runs it (decon_reg_set_bpc_and_dither_path, __exynos_dqe_update): the DQE in the enhance path, sized for the raster, its display dither taking the blend down to the link's 8 bits; no CGC, no ATC. The layer's DPP and HDR-common blocks take the same bit. 8 bpc is ABL's path: no enhance path at all.
    // With a 10 bpc stream to the panel nothing is thrown away, so no dither (in_bpc == out_bpc in the stock driver's terms); the DQE stays in the path.
    let enhance = if ten_bpc && link10 { ENHANCE_DQE_ON } else if ten_bpc { ENHANCE_DQE_ON | ENHANCE_DITHER_ON } else { 0 };
    wr(DECON0 + DATA_PATH_CON_0, (rd(DECON0 + DATA_PATH_CON_0) & !ENHANCE_PATH_MASK) | enhance);
    wr(DQE0 + DQE_TOP_IMG_SIZE, r.h << 16 | r.w);
    wr(DQE0 + DQE_TOP_FRM_SIZE, r.h << 16 | r.w);
    wr(DQE0 + DQE_TOP_FRM_PXL_NUM, r.w * r.h);
    wr(DQE0 + DQE_ATC_CONTROL, 0);
    wr(DQE0 + DQE_CGC_DITHER, 0);
    wr(DQE0 + DQE_DISP_DITHER, if ten_bpc && !link10 { DITHER_EN } else { 0 });
    wr(DQE_CGC0 + DQE_CGC_CON, 0);
    let bpc = if ten_bpc { BPC_MODE_10 } else { 0 };
    wr(DPP0 + DPP_COM_IO_CON, (rd(DPP0 + DPP_COM_IO_CON) & !BPC_MODE_10) | bpc);
    wr(HDRCOMM0 + LSI_COMM_IO_CON, (rd(HDRCOMM0 + LSI_COMM_IO_CON) & !BPC_MODE_10) | bpc);
    // Android's SRAM allocation, out-FIFO QoS and interrupt enables (display::configure_like_android), so a modeset lands in its state.
    wr(DECON0 + 0x0300, 0x1111_1111);
    wr(DECON0 + 0x0304, 0x111);
    wr(DECON0 + 0x0320, 0);
    wr(DECON0 + 0x02C0, 1);
    wr(DECON0 + 0x02C4, 0x0800_0400);
    wr(DECON0 + 0x02C8, 0x10);
    wr(DECON0 + 0x02D0, 1);
    wr(DECON0 + 0x02D4, 0x3200_0600);
    wr(DECON0 + 0x0060, 0x0030_3011);
    wr(DECON0 + 0x0064, 0x11);
    wr(DECON0 + BLD_BG_IMG_SIZE_PRI, r.h << 16 | r.w);
    wr(DECON_SUB + DSIMIF_SEL0, 0);
    let pps = ferros_dsc::pps(&r.cfg);
    for (dsc, snap) in [(DECON_SUB, sub0), (DECON_SUB + 0x1000, sub1)] {
        restore(dsc, 0, snap, &[]);
        wr(dsc + DSC_CONTROL3, 3 << 12 | r.comp);
        for i in 0..22 {
            wr(dsc + DSC_PPS + 4 * i, u32::from_be_bytes([pps[4 * i], pps[4 * i + 1], pps[4 * i + 2], pps[4 * i + 3]]));
        }
    }
    wr(DECON0 + OF_SIZE_0, r.h << 16 | r.comp);
    wr(DECON0 + OF_TH_TYPE, (rd(DECON0 + OF_TH_TYPE) & !0x7) | 5);
    wr(DECON0 + OF_SIZE_1, r.comp);
    wr(DECON0 + OF_SIZE_2, (r.cfg.slice_height as u32) << 16 | r.comp);
    wr(DECON0 + GLOBAL_CON, rd(DECON0 + GLOBAL_CON) & !GLOBAL_CON_DECON_EN_F);
    wr(DECON0 + DECON_INT_PEND, !0);
    wr(DECON0 + DECON_INT_PEND_EXTRA, !0);
    // The window and the layer: back as they were, then the raster's geometry.
    restore(DECON_WIN + WIN * 0x1000, 0, win, &[]);
    wr(DECON_WINCON + WIN * 0x1000, wincon);
    wr(DECON_WIN + WIN * 0x1000 + 0x000C, 0);
    wr(DECON_WIN + WIN * 0x1000 + 0x0010, (r.h - 1) << 16 | (r.w - 1));
    wr(DMA0 + 0x0010, r.w);
    wr(DMA0 + 0x0014, r.h);
    wr(DMA0 + 0x001C, r.h << 16 | r.w);
    wr(DPP0 + 0x003C, r.h << 16 | r.w);
    wr(SRAMC0 + 0x0014, (r.h - 1) << 16);
    wr(HDRCOMM0 + 0x0020, r.h << 16 | r.w);
}

/// Wait for the next rising edge on the panel's TE pin, then the stock driver's margin (273 × 1.05 µs).
fn after_te(cntfrq: u64) {
    let start = now();
    let mut last = rd(GPP0_DAT) >> 3 & 1;
    loop {
        let pin = rd(GPP0_DAT) >> 3 & 1;
        if pin == 1 && last == 0 {
            break;
        }
        last = pin;
        if now().wrapping_sub(start) > cntfrq / 20 {
            break;
        }
    }
    wait_us(287, cntfrq);
}

/// The whole modeset to `fhd` or WQHD, then one frame. Reports every step that misbehaved, the panel's raster register and the TE rate afterwards.
/// `rate`: 0 keeps the link rate, 1 = 1368 Mbps, 2 = 1346 Mbps. `bpc`: 0 keeps the blend depth, 1 = 8 bpc, 2 = 10 bpc, 3 = 10 bpc blend **and a 10 bpc DSC stream to the panel** (the experiment; `f2` is then the byte written to the panel's F2 register at offset 1 in place of the raster's 01 / 81, 0 = leave the raster's). Bridge `display modeset fhd|wqhd [1368|1346] [8|10|link10] [f2=XX]`.
pub fn modeset(fhd: bool, rate: u8, bpc: u8, f2_byte: u8, cntfrq: u64, rep: &mut Vec<u8>) {
    modeset_with(fhd, rate, bpc, f2_byte, false, cntfrq, rep)
}

/// `modeset`, and with `quiet` nothing after DECON is running again: no frame painted, no TE measurement (half a second), no readbacks. For the boot path, where the next frame is the shell's (service.rs `start_home`).
pub fn modeset_with(fhd: bool, rate: u8, bpc: u8, f2_byte: u8, quiet: bool, cntfrq: u64, rep: &mut Vec<u8>) {
    if !dpu_on() || !crate::dsim::alive() {
        rep.extend_from_slice(b"display not running\n");
        return;
    }
    let rate = match rate {
        1 => Some(&LINK_1368),
        2 => Some(&LINK_1346),
        _ => None,
    };
    // The panel's FFC off before its link clock changes (pre_update_ffc), as the stock stack does.
    if rate.map(|lr| lr.mbps != crate::dsim::hs_clk()).unwrap_or(false) {
        let _ = crate::dsim::sequence(crate::hk3::FFC_OFF, cntfrq);
    }
    let link10 = bpc == 3;
    let r = raster(fhd, link10);
    let main = snapshot(DECON0, 0, 0x0344);
    let sub0 = snapshot(DECON_SUB, 0, 0x0100);
    let sub1 = snapshot(DECON_SUB + 0x1000, 0, 0x0100);
    let win = snapshot(DECON_WIN + WIN * 0x1000, 0, 0x0020);
    let wincon = rd(DECON_WINCON + WIN * 0x1000);
    let link = snapshot(DSIM0, 0, 0x0128);
    let bias = snapshot(DPHY_BIAS, 0, 0x0014);
    let phy = snapshot(DPHY, 0, 0x0700);
    let ten_bpc = match bpc {
        1 => false,
        2 | 3 => true,
        _ => main[GLOBAL_CON / 4] & GLOBAL_CON_TEN_BPC != 0,
    };

    decon_stop(cntfrq, rep);
    dsim_stop(cntfrq, rep);
    dsim_init(&r, &link, &bias, &phy, rate, cntfrq, rep);
    decon_init(&r, &main, &sub0, &sub1, &win, wincon, ten_bpc, link10);

    // The panel, just after a TE: DSC on, the PPS, the raster, the full feature block for 120 Hz, control display, FFC on.
    after_te(cntfrq);
    let pps = ferros_dsc::pps(&r.cfg);
    if crate::dsim::send(crate::dsim::DCS_SHORT_WRITE_1, &[0x9D, 0x01], cntfrq).is_err() || crate::dsim::send(crate::dsim::PPS_LONG_WRITE, &pps, cntfrq).is_err() {
        rep.extend_from_slice(b"panel: PPS stuck\n");
    }
    let (_, f1) = crate::dsim::sequence(if fhd { crate::hk3::RES_FHD } else { crate::hk3::RES_WQHD }, cntfrq);
    if f2_byte != 0 {
        let _ = crate::dsim::sequence(&[0x39, 3, 0xF0, 0x5A, 0x5A, 0, 0x39, 4, 0xB0, 0x00, 0x01, 0xF2, 0, 0x15, 2, 0xF2, f2_byte, 0, 0x39, 3, 0xF0, 0xA5, 0xA5, 0], cntfrq);
    }
    let (_, f2) = crate::dsim::sequence(crate::hk3::REFRESH_120, cntfrq);
    let f3 = crate::dsim::dcs(&[0x53, 0x20], cntfrq).err();
    if let Some(lr) = rate {
        let _ = crate::dsim::sequence(lr.ffc, cntfrq);
    }
    let (_, f4) = crate::dsim::sequence(crate::hk3::FFC_ON, cntfrq);
    if f1.is_some() || f2.is_some() || f3.is_some() || f4.is_some() {
        rep.extend_from_slice(b"panel: a command set stuck\n");
    }

    // Start: every window's shadow, DECON on, running, and the painter's masked trigger.
    wr(DECON0 + SHD_REG_UP_REQ, rd(DECON0 + SHD_REG_UP_REQ) | SHD_ALL_WINDOWS);
    wr(DECON0 + GLOBAL_CON, rd(DECON0 + GLOBAL_CON) | GLOBAL_CON_DECON_EN | GLOBAL_CON_DECON_EN_F);
    wr(DECON0 + SHD_REG_UP_REQ, rd(DECON0 + SHD_REG_UP_REQ) | SHD_GLOBAL | SHD_CMP | if ten_bpc { SHD_REG_UP_REQ_DQE } else { 0 });
    wr(DECON0 + DECON_INT_PEND, INT_PEND_FRAME_START);
    if !poll(DECON0 + GLOBAL_CON, GLOBAL_CON_RUN_STATUS, GLOBAL_CON_RUN_STATUS, 2_000, cntfrq) {
        rep.extend_from_slice(b"decon: not running after start\n");
    }
    let t = rd(DECON0 + TRIG_CON);
    wr(DECON0 + TRIG_CON, (t & !HW_TRIG_EN) | HW_TRIG_MASK_DECON);
    if quiet {
        return;
    }

    let r2 = crate::display::show(if fhd { b"ferros FHD modeset" } else { b"ferros WQHD modeset" }, cntfrq);
    rep.extend_from_slice(&r2);
    let _ = crate::dsim::dcs(&[0xF0, 0x5A, 0x5A], cntfrq);
    match crate::dsim::read(0xC3, 1, cntfrq) {
        Ok(v) => crate::display::append_hex(rep, b"PANEL_C3=", v[0] as u32),
        Err(e) => crate::display::append_hex(rep, b"PANEL_C3_READ_FAILED=", e),
    }
    let _ = crate::dsim::dcs(&[0xF0, 0xA5, 0xA5], cntfrq);
    let ((te, _), width) = crate::display::te_profile(cntfrq);
    crate::display::append_hex(rep, b"TE_HZ=", te);
    crate::display::append_hex(rep, b"TE_US=", width);
    crate::display::append_hex(rep, b"HS_MBPS=", crate::dsim::hs_clk());
    crate::display::append_hex(rep, b"BPC=", if rd(DECON0 + GLOBAL_CON) & GLOBAL_CON_TEN_BPC != 0 { 10 } else { 8 });
    crate::display::append_hex(rep, b"LINK_BPC=", if link10 { 10 } else { 8 });
    if f2_byte != 0 {
        let _ = crate::dsim::dcs(&[0xF0, 0x5A, 0x5A], cntfrq);
        let _ = crate::dsim::dcs(&[0xB0, 0x00, 0x01, 0xF2], cntfrq);
        match crate::dsim::read(0xF2, 1, cntfrq) {
            Ok(v) => crate::display::append_hex(rep, b"PANEL_F2=", v[0] as u32),
            Err(e) => crate::display::append_hex(rep, b"PANEL_F2_READ_FAILED=", e),
        }
        let _ = crate::dsim::dcs(&[0xF0, 0xA5, 0xA5], cntfrq);
    }
    crate::display::append_hex(rep, b"FRAMES=", rd(DECON0 + 0x0004));
}
