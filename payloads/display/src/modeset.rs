//! The boot modeset, in the display server since 2026-11-01 (KERNEL-OUT.md decision 6; until then the kernel's `dpu::modeset_with`, run in `start_home` before init). ABL hands the pipe over at 8 bpc; the shell draws in 10 bits (DISPLAY-CALIB.md, 2026-10-22): the 10 bpc blend with the DQE's dither onto the 8 bpc link, the mode measured to carry a 10-bit layer's every code. The full modeset is the only proven way across, so the display server runs it once, at its start, before its first frame — and only while the pipe is still at 8 bpc, so a display server started again finds it done.
//!
//! The kernel's quiet path, ported word for word: WQHD, the link's own rate, the 10 bpc blend with the dither. DECON0 and DSIM0 stopped, re-initialised and started the way the stock driver does for a resolution change (DISPLAY-MODESET.md). Everything the sequence does not set is snapshotted before the resets and written back after them, so the configuration ABL proved comes back word for word — the DPHY's bias, PLL and lane tuning, DECON's blender/QoS/SRAM/trigger fields, the DSC encoders — and only the raster-dependent words change.
//!
//! The kernel checked the DPU's power domain through EL3 first; the display server runs only on a lit pipe (the boot screen is on it), so it does not.

use crate::{Pipe, S_DECON, S_DECON_SUB, S_DMA0, S_DPHY, S_DPP0, S_DPU_SYSREG, S_DQE, S_DQE_CGC, S_GPP0, S_HDRCOMM0, S_SRAMC0, S_WIN, S_WINCON, S_DSIM};
use alloc::vec::Vec;
use ferros_sys::{freq, now};

/// The base window ABL uses, on layer 0.
const WIN: u64 = 5;
/// The DPHY's two blocks inside the dphy0 grant: the bias block at its start, the PHY at +G#100.
const DPHY_BIAS: u64 = 0;
const DPHY: u64 = 0x100;
/// The second DSC encoder, G#1000 into the sub block.
const DSC1: u64 = 0x1000;

// DECON main (regs-decon.h)
const GLOBAL_CON: u64 = 0x0020;
const GLOBAL_CON_SRESET: u32 = 1 << 28;
const GLOBAL_CON_RUN_STATUS: u32 = 1 << 4;
const GLOBAL_CON_DECON_EN: u32 = 1 << 1;
const GLOBAL_CON_DECON_EN_F: u32 = 1 << 0;
const GLOBAL_CON_TEN_BPC: u32 = 1 << 20;
const DATA_PATH_CON_0: u64 = 0x0200;
const ENHANCE_PATH_MASK: u32 = 0xF << 12;
const ENHANCE_DITHER_ON: u32 = 1 << 12;
const ENHANCE_DQE_ON: u32 = 2 << 12;
const SHD_REG_UP_REQ_DQE: u32 = 1 << 24;
const DQE_TOP_IMG_SIZE: u64 = 0x0004;
const DQE_TOP_FRM_SIZE: u64 = 0x0008;
const DQE_TOP_FRM_PXL_NUM: u64 = 0x000C;
const DQE_ATC_CONTROL: u64 = 0x0400;
const DQE_CGC_DITHER: u64 = 0x2400;
const DQE_DISP_DITHER: u64 = 0x2800;
const DITHER_EN: u32 = 1 << 0;
const DQE_CGC_CON: u64 = 0x0000;
const DPP_COM_IO_CON: u64 = 0x0038;
const LSI_COMM_IO_CON: u64 = 0x000C;
const BPC_MODE_10: u32 = 1 << 6;
const TRIG_CON: u64 = 0x0030;
const HW_TRIG_EN: u32 = 1 << 0;
const HW_TRIG_MASK_DECON: u32 = 1 << 4;
const SHD_REG_UP_REQ: u64 = 0x0050;
const SHD_GLOBAL: u32 = 1 << 31;
const SHD_CMP: u32 = 1 << 20;
const SHD_ALL_WINDOWS: u32 = 0x3FFF;
const DECON_INT_PEND: u64 = 0x0070;
const DECON_INT_PEND_EXTRA: u64 = 0x0074;
const INT_PEND_FRAME_START: u32 = 1 << 12;
const BLD_BG_IMG_SIZE_PRI: u64 = 0x0220;
const OF_SIZE_0: u64 = 0x0290;
const OF_SIZE_1: u64 = 0x0294;
const OF_SIZE_2: u64 = 0x0298;
const OF_TH_TYPE: u64 = 0x029C;
// DECON sub: the DSC encoders and the DSIM interface select.
const DSC_CONTROL3: u64 = 0x000C;
const DSC_PPS: u64 = 0x0040;
const DSIMIF_SEL0: u64 = 0x8000;
// DSIM (regs-dsim.h)
const DSIM_SWRST: u64 = 0x0004;
const DSIM_DPHY_STATUS: u64 = 0x001C;
const STOPSTATE_CLK: u32 = 1 << 8;
const DSIM_CLK_CTRL: u64 = 0x0020;
const CLK_CLOCK_SEL: u32 = 1 << 26;
const CLK_NONCONT: u32 = 1 << 25;
const CLK_TX_REQUEST_HSCLK: u32 = 1 << 20;
const CLK_WORDCLK_EN: u32 = 1 << 17;
const CLK_ESCCLK_EN: u32 = 1 << 16;
const CLK_LANE_ESCCLK_EN: u32 = 0x1F << 8;
const CLK_ESC_PRESCALER_MASK: u32 = 0xFF;
const DSIM_DESKEW_CTRL: u64 = 0x0024;
const DESKEW_HW_EN: u32 = 1 << 15;
const DSIM_TIMEOUT: u64 = 0x0028;
const DSIM_ESCMODE: u64 = 0x002C;
const DSIM_NUM_OF_TRANSFER: u64 = 0x0030;
const DSIM_UNDERRUN_CTRL: u64 = 0x0034;
const DSIM_THRESHOLD: u64 = 0x0038;
const DSIM_RESOL: u64 = 0x003C;
const DSIM_CONFIG: u64 = 0x004C;
const CONFIG_LANES_EN: u32 = 0x1F;
/// CONFIG as the working link runs it (both rasters): 4 data lanes, EOTP, pixel format G#3E, compression on.
const CONFIG_RUNNING: u32 = 0x0488_7CFF;
const DSIM_INTSRC: u64 = 0x0050;
const DSIM_INTMSK: u64 = 0x0054;
const DSIM_SFR_CTRL: u64 = 0x0064;
const SFR_SHADOW_EN: u32 = 1 << 0;
const SFR_SHADOW_READ_EN: u32 = 1 << 1;
const DSIM_CPRS_CTRL: u64 = 0x0074;
const DSIM_SLICE01: u64 = 0x0078;
const DSIM_CMD_CONFIG: u64 = 0x0080;
const DSIM_CMD_TE_CTRL0: u64 = 0x0084;
const DSIM_CMD_TE_CTRL1: u64 = 0x0088;
const DSIM_OPTION_SUITE: u64 = 0x010C;
const OPT_TE_ON_CMD_ALLOW: u32 = 1 << 10;
// DPHY (inside the dphy0 grant at DPHY)
const PLL_CON0: u64 = 0x0000;
const PLL_EN: u32 = 1 << 12;
const PLL_STAT0: u64 = 0x0040;
const PLL_LOCK: u32 = 1 << 0;
const MC_GNR_CON0: u64 = 0x0200;
const fn md_gnr_con0(lane: u64) -> u64 {
    0x0300 + 0x100 * lane
}
const PHY_ENABLE: u32 = 1 << 0;
const PHY_READY: u32 = 1 << 1;
const LANES: [u64; 5] = [MC_GNR_CON0, md_gnr_con0(0), md_gnr_con0(1), md_gnr_con0(2), md_gnr_con0(3)];
// The DPU sysreg: the D-PHY's resets.
const DISP_DPU_MIPI_PHY_CON: u64 = 0x0008;
const M_RESETN_M0: u32 = 1 << 0;
const SEL_RESET_DPHY0: u32 = 1 << 4;
/// The panel's TE pin: GPP0 DAT bit 3.
const GPP0_DAT: u64 = 0x0004;

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

/// WQHD on the 8 bpc link.
fn raster() -> Raster {
    let cfg = ferros_dsc::HUSKY_WQHD;
    let sw = cfg.slice_width as u32;
    let comp = sw / 3;
    let threshold = (sw.div_ceil(3) + 3) / 4 * 4 * 2;
    Raster { cfg, w: cfg.pic_width as u32, h: cfg.pic_height as u32, comp, threshold, underrun: 0x063A }
}

struct M<'a> {
    p: &'a Pipe,
    f: u64,
    rep: Vec<u8>,
}

impl M<'_> {
    fn rd(&self, s: u64, o: u64) -> u32 {
        self.p.rd(s, o)
    }
    fn wr(&self, s: u64, o: u64, v: u32) {
        self.p.wr(s, o, v)
    }
    fn set(&self, s: u64, o: u64, bits: u32) {
        self.wr(s, o, self.rd(s, o) | bits)
    }
    fn clear(&self, s: u64, o: u64, bits: u32) {
        self.wr(s, o, self.rd(s, o) & !bits)
    }
    /// Until `(rd & mask) == want`, up to `us` microseconds; false on timeout.
    fn poll(&self, s: u64, o: u64, mask: u32, want: u32, us: u64) -> bool {
        let start = now();
        while self.rd(s, o) & mask != want {
            if now().wrapping_sub(start) > self.f * us / 1_000_000 {
                return false;
            }
        }
        true
    }
    fn wait_us(&self, us: u64) {
        let start = now();
        while now().wrapping_sub(start) < self.f * us / 1_000_000 {}
    }
    fn snapshot(&self, s: u64, base: u64, len: u64) -> Vec<u32> {
        (0..len).step_by(4).map(|o| self.rd(s, base + o)).collect()
    }
    fn restore(&self, s: u64, base: u64, words: &[u32], skip: &[u64]) {
        for (i, &w) in words.iter().enumerate() {
            let off = 4 * i as u64;
            if !skip.contains(&off) {
                self.wr(s, base + off, w);
            }
        }
    }
    fn say(&mut self, t: &[u8]) {
        self.rep.extend_from_slice(t);
    }

    /// DECON stop with reset (decon_reg_stop, DISPLAY-MODESET.md).
    fn decon_stop(&mut self) {
        let t = self.rd(S_DECON, TRIG_CON);
        self.wr(S_DECON, TRIG_CON, (t | HW_TRIG_MASK_DECON) & !HW_TRIG_EN);
        self.clear(S_DECON, GLOBAL_CON, GLOBAL_CON_DECON_EN_F);
        self.set(S_DECON, SHD_REG_UP_REQ, SHD_GLOBAL | SHD_CMP);
        if !self.poll(S_DECON, GLOBAL_CON, GLOBAL_CON_RUN_STATUS, 0, 12_500) {
            self.say(b"decon: per-frame stop timed out, instant off\n");
            self.clear(S_DECON, GLOBAL_CON, GLOBAL_CON_DECON_EN | GLOBAL_CON_DECON_EN_F);
            self.set(S_DECON, SHD_REG_UP_REQ, SHD_GLOBAL | SHD_CMP);
            let _ = self.poll(S_DECON, GLOBAL_CON, GLOBAL_CON_RUN_STATUS, 0, 12_500);
        }
        self.set(S_DECON, GLOBAL_CON, GLOBAL_CON_SRESET);
        if !self.poll(S_DECON, GLOBAL_CON, GLOBAL_CON_SRESET, 0, 2_000) {
            self.say(b"decon: SRESET did not clear\n");
        }
        self.wr(S_DECON, DECON_INT_PEND, (1 << 21) | (1 << 20) | (1 << 13) | (1 << 12));
        self.wr(S_DECON, DECON_INT_PEND_EXTRA, (1 << 4) | 1);
    }

    /// DSIM stop (dsim_reg_stop): HS clock off, OSC, DPHY reset, lanes off, escape clock off, PLL off, software reset.
    fn dsim_stop(&mut self) {
        self.wr(S_DSIM, DSIM_INTSRC, !0);
        self.wr(S_DSIM, DSIM_INTMSK, !0);
        self.clear(S_DSIM, DSIM_CLK_CTRL, CLK_TX_REQUEST_HSCLK);
        if !self.poll(S_DSIM, DSIM_DPHY_STATUS, STOPSTATE_CLK, STOPSTATE_CLK, 2_000) {
            self.say(b"dsim: clock lane never reached stop state\n");
        }
        self.clear(S_DSIM, DSIM_CLK_CTRL, CLK_CLOCK_SEL);
        self.clear(S_DPU_SYSREG, DISP_DPU_MIPI_PHY_CON, M_RESETN_M0);
        for lane in LANES {
            self.clear(S_DPHY, DPHY + lane, PHY_ENABLE);
            let _ = self.poll(S_DPHY, DPHY + lane, PHY_READY, 0, 2_000);
        }
        let c = self.rd(S_DSIM, DSIM_CLK_CTRL) & !(CLK_LANE_ESCCLK_EN | CLK_ESCCLK_EN | CLK_ESC_PRESCALER_MASK);
        self.wr(S_DSIM, DSIM_CLK_CTRL, c | 0xFF);
        self.clear(S_DPHY, DPHY + PLL_CON0, PLL_EN);
        if !self.poll(S_DPHY, DPHY + PLL_STAT0, PLL_LOCK, 0, 2_000) {
            self.say(b"dsim: PLL did not unlock\n");
        }
        self.wr(S_DSIM, DSIM_SWRST, 1);
        if !self.poll(S_DSIM, DSIM_SWRST, 1, 0, 2_000) {
            self.say(b"dsim: SWRST did not clear\n");
        }
    }

    /// DSIM init + start (dsim_reg_init, dsim_reg_start) for the raster, the DPHY's proven words restored around the PLL re-lock. `hs`: the link's rate, Mbps, unchanged.
    fn dsim_init(&mut self, r: &Raster, link: &[u32], bias: &[u32], phy: &[u32], hs: u32) {
        // DPHY reset under sysreg control, OSC clock, software reset.
        self.clear(S_DPU_SYSREG, DISP_DPU_MIPI_PHY_CON, SEL_RESET_DPHY0);
        self.clear(S_DSIM, DSIM_CLK_CTRL, CLK_CLOCK_SEL);
        self.wr(S_DSIM, DSIM_SWRST, 1);
        let _ = self.poll(S_DSIM, DSIM_SWRST, 1, 0, 2_000);
        // The link's proven configuration back, except what the sequence orders explicitly and what is status or FIFO.
        self.restore(S_DSIM, 0, link, &[DSIM_SWRST, DSIM_DPHY_STATUS, DSIM_CLK_CTRL, DSIM_CONFIG, DSIM_INTSRC, DSIM_INTMSK, 0x0058, 0x005C, 0x0060, DSIM_SFR_CTRL, 0x0068]);
        self.wr(S_DSIM, DSIM_CONFIG, CONFIG_LANES_EN);
        self.wr(S_DSIM, DSIM_CLK_CTRL, CLK_LANE_ESCCLK_EN | CLK_WORDCLK_EN | CLK_ESCCLK_EN | 5);
        // DPHY reset asserted while the PLL and lanes come up.
        self.clear(S_DPU_SYSREG, DISP_DPU_MIPI_PHY_CON, M_RESETN_M0);
        self.restore(S_DPHY, DPHY_BIAS, bias, &[]);
        for (i, &w) in phy.iter().enumerate() {
            let off = 4 * i as u64;
            let w = match off {
                PLL_CON0 => w & !PLL_EN,
                o if LANES.contains(&o) => w & !PHY_ENABLE,
                PLL_STAT0 => continue,
                _ => w,
            };
            self.wr(S_DPHY, DPHY + off, w);
        }
        self.wr(S_DSIM, DSIM_INTSRC, 1 << 31);
        self.set(S_DPHY, DPHY + PLL_CON0, PLL_EN);
        if !self.poll(S_DPHY, DPHY + PLL_STAT0, PLL_LOCK, PLL_LOCK, 2_000) {
            self.say(b"dsim: PLL did not lock\n");
        }
        for lane in LANES {
            self.set(S_DPHY, DPHY + lane, PHY_ENABLE);
            if !self.poll(S_DPHY, DPHY + lane, PHY_READY, PHY_READY, 2_000) {
                self.say(b"dsim: a lane never became ready\n");
            }
        }
        self.set(S_DPU_SYSREG, DISP_DPU_MIPI_PHY_CON, M_RESETN_M0);
        self.set(S_DSIM, DSIM_CLK_CTRL, CLK_CLOCK_SEL | CLK_NONCONT);
        // Configuration (dsim_reg_set_config), geometry from the raster.
        self.wr(S_DSIM, DSIM_SFR_CTRL, SFR_SHADOW_READ_EN);
        self.clear(S_DSIM, DSIM_DESKEW_CTRL, DESKEW_HW_EN);
        self.wr(S_DSIM, DSIM_TIMEOUT, 0x00FF_FFFF);
        self.wr(S_DSIM, DSIM_ESCMODE, 0xA << 21);
        self.wr(S_DSIM, DSIM_UNDERRUN_CTRL, r.underrun);
        self.set(S_DSIM, DSIM_OPTION_SUITE, OPT_TE_ON_CMD_ALLOW);
        self.wr(S_DSIM, DSIM_THRESHOLD, r.threshold);
        self.wr(S_DSIM, DSIM_RESOL, r.h << 16 | r.threshold);
        self.wr(S_DSIM, DSIM_NUM_OF_TRANSFER, r.h);
        self.wr(S_DSIM, DSIM_CONFIG, CONFIG_RUNNING);
        self.wr(S_DSIM, DSIM_SFR_CTRL, SFR_SHADOW_READ_EN | SFR_SHADOW_EN);
        // The stock driver sets command mode and compression after the shadow is on; written before it, the compression bit did not reach the applied register (measured: CONFIG read back G#0480_7CFF).
        self.wr(S_DSIM, DSIM_CONFIG, CONFIG_RUNNING);
        self.wr(S_DSIM, DSIM_CPRS_CTRL, 0xA);
        let sw = r.cfg.slice_width as u32;
        self.wr(S_DSIM, DSIM_SLICE01, sw << 16 | sw);
        self.wr(S_DSIM, DSIM_CMD_CONFIG, 0);
        self.wr(S_DSIM, DSIM_CMD_TE_CTRL0, r.w * 2 / 100);
        self.wr(S_DSIM, DSIM_CMD_TE_CTRL1, (hs * 95 * 100 / 120 / 16) << 16 | (hs * 110 * 100 / 120 / 16));
        // Start: request the HS clock, interrupts as they were, status clear.
        self.set(S_DSIM, DSIM_CLK_CTRL, CLK_TX_REQUEST_HSCLK);
        self.wr(S_DSIM, DSIM_INTMSK, link[(DSIM_INTMSK / 4) as usize]);
        self.wr(S_DSIM, DSIM_INTSRC, !0);
    }

    /// DECON init for the raster (decon_reg_init) on top of the restored snapshot: geometry, the DSC encoders, the DSIM interface select, the out-FIFO; the 10 bpc blend with the DQE's display dither onto the 8 bpc link.
    fn decon_init(&mut self, r: &Raster, main: &[u32], sub0: &[u32], sub1: &[u32], win: &[u32], wincon: u32) {
        self.restore(S_DECON, 0, main, &[0x0004, GLOBAL_CON, 0x0038, 0x003C, SHD_REG_UP_REQ, DECON_INT_PEND, DECON_INT_PEND_EXTRA, 0x0100, 0x0104, 0x0108, 0x010C, 0x0110, 0x0114, 0x0118, 0x011C, 0x0120, 0x0124]);
        self.wr(S_DECON, GLOBAL_CON, 1 << 8 | GLOBAL_CON_TEN_BPC);
        // 10 bpc the way Android runs it (decon_reg_set_bpc_and_dither_path, __exynos_dqe_update): the DQE in the enhance path, sized for the raster, its display dither taking the blend down to the link's 8 bits; no CGC, no ATC. The layer's DPP and HDR-common blocks take the same bit.
        let p = (self.rd(S_DECON, DATA_PATH_CON_0) & !ENHANCE_PATH_MASK) | ENHANCE_DQE_ON | ENHANCE_DITHER_ON;
        self.wr(S_DECON, DATA_PATH_CON_0, p);
        self.wr(S_DQE, DQE_TOP_IMG_SIZE, r.h << 16 | r.w);
        self.wr(S_DQE, DQE_TOP_FRM_SIZE, r.h << 16 | r.w);
        self.wr(S_DQE, DQE_TOP_FRM_PXL_NUM, r.w * r.h);
        self.wr(S_DQE, DQE_ATC_CONTROL, 0);
        self.wr(S_DQE, DQE_CGC_DITHER, 0);
        self.wr(S_DQE, DQE_DISP_DITHER, DITHER_EN);
        self.wr(S_DQE_CGC, DQE_CGC_CON, 0);
        self.set(S_DPP0, DPP_COM_IO_CON, BPC_MODE_10);
        self.set(S_HDRCOMM0, LSI_COMM_IO_CON, BPC_MODE_10);
        // Android's SRAM allocation, out-FIFO QoS and interrupt enables (the kernel's display::configure_like_android), so a modeset lands in its state.
        self.wr(S_DECON, 0x0300, 0x1111_1111);
        self.wr(S_DECON, 0x0304, 0x111);
        self.wr(S_DECON, 0x0320, 0);
        self.wr(S_DECON, 0x02C0, 1);
        self.wr(S_DECON, 0x02C4, 0x0800_0400);
        self.wr(S_DECON, 0x02C8, 0x10);
        self.wr(S_DECON, 0x02D0, 1);
        self.wr(S_DECON, 0x02D4, 0x3200_0600);
        self.wr(S_DECON, 0x0060, 0x0030_3011);
        self.wr(S_DECON, 0x0064, 0x11);
        self.wr(S_DECON, BLD_BG_IMG_SIZE_PRI, r.h << 16 | r.w);
        self.wr(S_DECON_SUB, DSIMIF_SEL0, 0);
        let pps = ferros_dsc::pps(&r.cfg);
        for (dsc, snap) in [(0, sub0), (DSC1, sub1)] {
            self.restore(S_DECON_SUB, dsc, snap, &[]);
            self.wr(S_DECON_SUB, dsc + DSC_CONTROL3, 3 << 12 | r.comp);
            for i in 0..22 {
                self.wr(S_DECON_SUB, dsc + DSC_PPS + 4 * i as u64, u32::from_be_bytes([pps[4 * i], pps[4 * i + 1], pps[4 * i + 2], pps[4 * i + 3]]));
            }
        }
        self.wr(S_DECON, OF_SIZE_0, r.h << 16 | r.comp);
        self.wr(S_DECON, OF_TH_TYPE, (self.rd(S_DECON, OF_TH_TYPE) & !0x7) | 5);
        self.wr(S_DECON, OF_SIZE_1, r.comp);
        self.wr(S_DECON, OF_SIZE_2, (r.cfg.slice_height as u32) << 16 | r.comp);
        self.clear(S_DECON, GLOBAL_CON, GLOBAL_CON_DECON_EN_F);
        self.wr(S_DECON, DECON_INT_PEND, !0);
        self.wr(S_DECON, DECON_INT_PEND_EXTRA, !0);
        // The window and the layer: back as they were, then the raster's geometry.
        self.restore(S_WIN, WIN * 0x1000, win, &[]);
        self.wr(S_WINCON, WIN * 0x1000, wincon);
        self.wr(S_WIN, WIN * 0x1000 + 0x000C, 0);
        self.wr(S_WIN, WIN * 0x1000 + 0x0010, (r.h - 1) << 16 | (r.w - 1));
        self.wr(S_DMA0, 0x0010, r.w);
        self.wr(S_DMA0, 0x0014, r.h);
        self.wr(S_DMA0, 0x001C, r.h << 16 | r.w);
        self.wr(S_DPP0, 0x003C, r.h << 16 | r.w);
        self.wr(S_SRAMC0, 0x0014, (r.h - 1) << 16);
        self.wr(S_HDRCOMM0, 0x0020, r.h << 16 | r.w);
    }

    /// The next rising edge on the panel's TE pin, then the stock driver's margin (273 × 1.05 µs).
    fn after_te(&self) {
        let start = now();
        let mut last = self.rd(S_GPP0, GPP0_DAT) >> 3 & 1;
        loop {
            let pin = self.rd(S_GPP0, GPP0_DAT) >> 3 & 1;
            if pin == 1 && last == 0 {
                break;
            }
            last = pin;
            if now().wrapping_sub(start) > self.f / 20 {
                break;
            }
        }
        self.wait_us(287);
    }
}

/// The modeset when the pipe is still at 8 bpc (ABL's), then the DQE's dither in shift mode either way. Returns what it did and what misbehaved, for the console; empty when the pipe was already at 10 bpc.
/// `force`: run it even when DECON is already at 10 bpc (after the jump from Android, whose raster and panel mode may not be ferros's).
pub fn ten_bpc(p: &Pipe, dsi: &ferros_dsi::Dsim<crate::DsiGrant>, force: bool) -> Vec<u8> {
    let mut m = M { p, f: freq(), rep: Vec::new() };
    if force || m.rd(S_DECON, GLOBAL_CON) & GLOBAL_CON_TEN_BPC == 0 {
        if !dsi.alive() {
            m.say(b"modeset: DSIM not running, left at 8 bpc\n");
            return m.rep;
        }
        let t0 = now();
        let r = raster();
        let hs = ferros_dsi::pll_hs_clk(|off| m.rd(S_DPHY, DPHY + off as u64));
        let main = m.snapshot(S_DECON, 0, 0x0344);
        let sub0 = m.snapshot(S_DECON_SUB, 0, 0x0100);
        let sub1 = m.snapshot(S_DECON_SUB, DSC1, 0x0100);
        let win = m.snapshot(S_WIN, WIN * 0x1000, 0x0020);
        let wincon = m.rd(S_WINCON, WIN * 0x1000);
        let link = m.snapshot(S_DSIM, 0, 0x0128);
        let bias = m.snapshot(S_DPHY, DPHY_BIAS, 0x0014);
        let phy = m.snapshot(S_DPHY, DPHY, 0x0700);

        m.decon_stop();
        m.dsim_stop();
        m.dsim_init(&r, &link, &bias, &phy, hs);
        m.decon_init(&r, &main, &sub0, &sub1, &win, wincon);

        // The panel, just after a TE: DSC on, the PPS, the raster, the full feature block for 120 Hz, control display, FFC on.
        m.after_te();
        let pps = ferros_dsc::pps(&r.cfg);
        if dsi.send(ferros_dsi::DCS_SHORT_WRITE_1, &[0x9D, 0x01]).is_err() || dsi.send(ferros_dsi::PPS_LONG_WRITE, &pps).is_err() {
            m.say(b"panel: PPS stuck\n");
        }
        let (_, f1) = dsi.sequence(ferros_dsi::hk3::RES_WQHD);
        let (_, f2) = dsi.sequence(ferros_dsi::hk3::REFRESH_120);
        let f3 = dsi.dcs(&[0x53, 0x20]).err();
        let (_, f4) = dsi.sequence(ferros_dsi::hk3::FFC_ON);
        if f1.is_some() || f2.is_some() || f3.is_some() || f4.is_some() {
            m.say(b"panel: a command set stuck\n");
        }

        // Start: every window's shadow, DECON on, running, and the painter's masked trigger.
        m.set(S_DECON, SHD_REG_UP_REQ, SHD_ALL_WINDOWS);
        m.set(S_DECON, GLOBAL_CON, GLOBAL_CON_DECON_EN | GLOBAL_CON_DECON_EN_F);
        m.set(S_DECON, SHD_REG_UP_REQ, SHD_GLOBAL | SHD_CMP | SHD_REG_UP_REQ_DQE);
        m.wr(S_DECON, DECON_INT_PEND, INT_PEND_FRAME_START);
        if !m.poll(S_DECON, GLOBAL_CON, GLOBAL_CON_RUN_STATUS, GLOBAL_CON_RUN_STATUS, 2_000) {
            m.say(b"decon: not running after start\n");
        }
        let t = m.rd(S_DECON, TRIG_CON);
        m.wr(S_DECON, TRIG_CON, (t & !HW_TRIG_EN) | HW_TRIG_MASK_DECON);
        let ms = now().wrapping_sub(t0) * 1000 / m.f;
        m.say(alloc::format!("modeset: 8 to 10 bpc in {ms} ms\n").as_bytes());
    }
    // The DQE's dither in shift mode (EN | MODE = plain truncation of the two low bits): the display server dithers to 8-bit codes itself, from its 14-bit table, with the low two bits of each 10-bit code zero, so the panel gets exactly that pattern (the kernel set it after its modeset until 2026-11-01).
    m.wr(S_DQE, DQE_DISP_DITHER, 3);
    m.set(S_DECON, SHD_REG_UP_REQ, SHD_REG_UP_REQ_DQE);
    m.rep
}

/// After the jump from Android, with the pipe already at 10 bpc so `ten_bpc` has nothing to do: the DQE as `decon_init` leaves it on a cold boot, written over Android's colour stages (gamut correction, adaptive tone, the gamma and matrix stages: colours off and the gradients posterized on the thirty-fifth jump, 2026-11-02), and the stages' shadow latched at the next frame. Nothing of DECON's timing, DSIM or the panel is touched.
pub fn dqe_quiet(p: &Pipe, w: u32, h: u32) {
    let m = M { p, f: freq(), rep: Vec::new() };
    let path = (m.rd(S_DECON, DATA_PATH_CON_0) & !ENHANCE_PATH_MASK) | ENHANCE_DQE_ON | ENHANCE_DITHER_ON;
    m.wr(S_DECON, DATA_PATH_CON_0, path);
    m.wr(S_DQE, DQE_TOP_IMG_SIZE, h << 16 | w);
    m.wr(S_DQE, DQE_TOP_FRM_SIZE, h << 16 | w);
    m.wr(S_DQE, DQE_TOP_FRM_PXL_NUM, w * h);
    // Every colour stage off: ATC, HSC, the gamma matrix, degamma, the linear matrix, the three regamma blocks (regs-dqe.h), and the CGC with its dither.
    for off in [DQE_ATC_CONTROL, 0x0800, 0x0C00, 0x1000, 0x1400, 0x2000, 0x4800, 0x4C00] {
        m.wr(S_DQE, off, 0);
    }
    m.wr(S_DQE, DQE_CGC_DITHER, 0);
    m.wr(S_DQE, DQE_DISP_DITHER, DITHER_EN);
    m.wr(S_DQE_CGC, DQE_CGC_CON, 0);
    m.set(S_DPP0, DPP_COM_IO_CON, BPC_MODE_10);
    m.set(S_HDRCOMM0, LSI_COMM_IO_CON, BPC_MODE_10);
    m.set(S_DECON, SHD_REG_UP_REQ, SHD_GLOBAL | SHD_CMP | SHD_REG_UP_REQ_DQE);
}
