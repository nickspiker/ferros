//! Minimal Qualcomm DPU (Display Processing Unit) driver for QCM6490.
//!
//! ## Register layout (DPU 7.2 — SC7280 / QCM6490 / SM7325)
//!
//! All offsets relative to MDP base (`0x0AE0_1000`):
//!
//! ```text
//! MDSS top:    0x0AE0_0000  (HW_VERSION)
//! MDP core:    0x0AE0_1000
//! CTL_0..3:    MDP + 0x15000, stride 0x1000
//! SSPP VIG0:   MDP + 0x04000
//! SSPP DMA0:   MDP + 0x24000
//! LM_0:        MDP + 0x44000
//! INTF_1(DSI): MDP + 0x35000
//! ```
//!
//! Source: Linux `dpu_7_2_sc7280.h` catalog (QCM6490 = SC7280 display block).

use crate::mmio;

// ---------------------------------------------------------------------------
// Base addresses
// ---------------------------------------------------------------------------

pub const MDSS_BASE: usize = 0x0AE0_0000;
pub const MDP_BASE: usize = MDSS_BASE + 0x1000;

// CTL blocks — DPU 7.2 uses 0x15000 base with 0x1000 stride
const CTL_0: usize = MDP_BASE + 0x15000;
const CTL_1: usize = MDP_BASE + 0x16000;
const CTL_2: usize = MDP_BASE + 0x17000;
const CTL_3: usize = MDP_BASE + 0x18000;
const CTLS: [usize; 4] = [CTL_0, CTL_1, CTL_2, CTL_3];

// SSPP blocks
pub const SSPP_VIG0: usize = MDP_BASE + 0x4000;
pub const SSPP_DMA0: usize = MDP_BASE + 0x24000;

// Layer Mixer
pub const LM_0: usize = MDP_BASE + 0x44000;

// Interface — INTF_1 = DSI0 on QCM6490
pub const INTF_1: usize = MDP_BASE + 0x35000;

// ---------------------------------------------------------------------------
// CTL register offsets
// ---------------------------------------------------------------------------

/// CTL_LAYER(LM_0) — blend stage config for LM_0.
const CTL_LAYER_LM0: usize = 0x000;
/// CTL_LAYER_EXT(LM_0)
const CTL_LAYER_EXT_LM0: usize = 0x040;
/// CTL_LAYER_EXT2(LM_0)
const CTL_LAYER_EXT2_LM0: usize = 0x070;
/// CTL_LAYER_EXT3(LM_0)
const CTL_LAYER_EXT3_LM0: usize = 0x0A0;

#[allow(dead_code)] const CTL_TOP: usize = 0x014;
const CTL_FLUSH: usize = 0x018;
const CTL_START: usize = 0x01C;
const CTL_INTF_ACTIVE: usize = 0x0F4;
const CTL_FETCH_PIPE_ACTIVE: usize = 0x0FC;
const CTL_INTF_FLUSH: usize = 0x110;

// ---------------------------------------------------------------------------
// SSPP register offsets
// ---------------------------------------------------------------------------

const SSPP_SRC_SIZE: usize = 0x00;
const SSPP_SRC_IMG_SIZE: usize = 0x04;
const SSPP_SRC_XY: usize = 0x08;
const SSPP_OUT_SIZE: usize = 0x0C;
const SSPP_OUT_XY: usize = 0x10;
const SSPP_SRC0_ADDR: usize = 0x14;
const SSPP_SRC_YSTRIDE0: usize = 0x24;
const SSPP_SRC_FORMAT: usize = 0x30;
const SSPP_SRC_UNPACK: usize = 0x34;
const SSPP_SRC_OP_MODE: usize = 0x38;

// Pixel extension registers
const SSPP_SW_PIX_EXT_C0_LR: usize = 0x100;
const SSPP_SW_PIX_EXT_C0_TB: usize = 0x104;
const SSPP_SW_PIX_EXT_C0_REQ: usize = 0x108;
const SSPP_SW_PIX_EXT_C1C2_LR: usize = 0x110;
const SSPP_SW_PIX_EXT_C1C2_TB: usize = 0x114;
const SSPP_SW_PIX_EXT_C1C2_REQ: usize = 0x118;
const SSPP_SW_PIX_EXT_C3_LR: usize = 0x120;
const SSPP_SW_PIX_EXT_C3_TB: usize = 0x124;
const SSPP_SW_PIX_EXT_C3_REQ: usize = 0x128;

// ---------------------------------------------------------------------------
// LM register offsets
// ---------------------------------------------------------------------------

const LM_OUT_SIZE: usize = 0x04;
// Blend stage 0 base
const LM_BLEND0_OP: usize = 0x20;
const LM_BLEND0_CONST_ALPHA: usize = 0x24;

// ---------------------------------------------------------------------------
// INTF register offsets
// ---------------------------------------------------------------------------

#[allow(dead_code)] const INTF_TIMING_ENGINE_EN: usize = 0x000;

// ---------------------------------------------------------------------------
// XRGB8888 format constants (from Linux mdp_format.c)
// ---------------------------------------------------------------------------

/// SSPP_SRC_FORMAT for XRGB8888 (linear, interleaved).
///
/// Built from: chroma_samp=0, fetch_type=0(interleaved),
/// bpc_a=3, bpc_r=3, bpc_b=3, bpc_g=3 (all 8-bit),
/// unpack_count=4-1=3, unpack_tight=1, bpp=4-1=3.
const XRGB8888_SRC_FORMAT: u32 = 0x0002_36FF;

/// SSPP_SRC_UNPACK_PATTERN for XRGB8888.
/// Component order: B=1(C1), G=0(C0), R=2(C2), A=3(C3).
const XRGB8888_UNPACK: u32 = 0x0302_0001;

/// PE_OVERRIDE bit in SSPP_SRC_OP_MODE.
const OP_PE_OVERRIDE: u32 = 1 << 31;

// ---------------------------------------------------------------------------
// CTL flush bits
// ---------------------------------------------------------------------------

const FLUSH_VIG0: u32 = 1 << 0;
const FLUSH_LM0: u32 = 1 << 6;
const FLUSH_CTL: u32 = 1 << 17;
const FLUSH_INTF: u32 = 1 << 31;

// Fetch pipe active bits
const FETCH_VIG0: u32 = 1 << 16;
const FETCH_DMA0: u32 = 1 << 0;

// INTF active bits
const INTF1_BIT: u32 = 1 << 1; // INTF_1 = BIT(1)

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read MDSS hardware version. Non-zero = MDSS is powered/clocked.
pub fn hw_version() -> u32 {
    unsafe { mmio::read32(MDSS_BASE) }
}

/// Read a DPU register (diagnostics).
pub fn read_reg(addr: usize) -> u32 {
    unsafe { mmio::read32(addr) }
}

/// Try ABL splash handoff: just flush + start all CTLs.
///
/// If ABL left the DPU pipeline configured and clocked, this should
/// cause a new frame to be sent to the panel with our framebuffer data.
pub fn try_splash_handoff() {
    for &ctl in &CTLS {
        unsafe {
            mmio::write32(ctl + CTL_FLUSH, 0xFFFF_FFFF);
            mmio::write32(ctl + CTL_START, 1);
        }
    }
    dsb();
}

/// Full pipeline setup: VIG0 → LM_0 → CTL_0 → INTF_1 (DSI0).
///
/// Programs SSPP, Layer Mixer, and CTL from scratch, then flushes.
/// Assumes display clocks are still running from ABL and the INTF
/// timing generator is still enabled.
pub fn setup_pipeline(fb_addr: u32, width: u32, height: u32, stride: u32) {
    let size = (height << 16) | width;

    // -- Step 1: Program SSPP VIG0 --
    unsafe {
        let s = SSPP_VIG0;
        mmio::write32(s + SSPP_SRC_SIZE, size);
        mmio::write32(s + SSPP_SRC_IMG_SIZE, size);
        mmio::write32(s + SSPP_SRC_XY, 0);
        mmio::write32(s + SSPP_OUT_SIZE, size);
        mmio::write32(s + SSPP_OUT_XY, 0);
        mmio::write32(s + SSPP_SRC0_ADDR, fb_addr);
        mmio::write32(s + SSPP_SRC_YSTRIDE0, stride);
        mmio::write32(s + SSPP_SRC_FORMAT, XRGB8888_SRC_FORMAT);
        mmio::write32(s + SSPP_SRC_UNPACK, XRGB8888_UNPACK);
        mmio::write32(s + SSPP_SRC_OP_MODE, OP_PE_OVERRIDE);

        // Pixel extension (1:1, no scaling)
        mmio::write32(s + SSPP_SW_PIX_EXT_C0_LR, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C0_TB, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C0_REQ, size);
        mmio::write32(s + SSPP_SW_PIX_EXT_C1C2_LR, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C1C2_TB, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C1C2_REQ, size);
        mmio::write32(s + SSPP_SW_PIX_EXT_C3_LR, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C3_TB, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C3_REQ, size);
    }
    dsb();

    // -- Step 2: Program LM_0 --
    unsafe {
        mmio::write32(LM_0 + LM_OUT_SIZE, size);
        // Blend stage 0: opaque foreground
        mmio::write32(LM_0 + LM_BLEND0_OP, 0x10);
        // fg_alpha=0xFF, bg_alpha=0x00
        mmio::write32(LM_0 + LM_BLEND0_CONST_ALPHA, 0x00FF_0000);
    }
    dsb();

    // -- Step 3: Program CTL_0 --
    unsafe {
        let c = CTL_0;

        // Connect VIG0 to LM_0 at blend stage 0
        // VIG0 mix value = (stage_0 + 1) & 0x7 = 1, at bit 0
        mmio::write32(c + CTL_LAYER_LM0, 0x0000_0001);
        mmio::write32(c + CTL_LAYER_EXT_LM0, 0);
        mmio::write32(c + CTL_LAYER_EXT2_LM0, 0);
        mmio::write32(c + CTL_LAYER_EXT3_LM0, 0);

        // Activate INTF_1 (DSI)
        mmio::write32(c + CTL_INTF_ACTIVE, INTF1_BIT);

        // Activate VIG0 fetch pipe
        mmio::write32(c + CTL_FETCH_PIPE_ACTIVE, FETCH_VIG0);

        // Flush INTF_1 sub-register (v1 active CTL path)
        mmio::write32(c + CTL_INTF_FLUSH, INTF1_BIT);

        // Master flush: VIG0 + LM_0 + CTL + INTF
        mmio::write32(c + CTL_FLUSH, FLUSH_VIG0 | FLUSH_LM0 | FLUSH_CTL | FLUSH_INTF);
    }
    dsb();

    // -- Step 4: Trigger frame start --
    unsafe {
        mmio::write32(CTL_0 + CTL_START, 1);
    }
    dsb();
}

/// Try DMA0 instead of VIG0 (ABL might use DMA pipe for splash).
pub fn setup_pipeline_dma0(fb_addr: u32, width: u32, height: u32, stride: u32) {
    let size = (height << 16) | width;

    // Program SSPP DMA0
    unsafe {
        let s = SSPP_DMA0;
        mmio::write32(s + SSPP_SRC_SIZE, size);
        mmio::write32(s + SSPP_SRC_IMG_SIZE, size);
        mmio::write32(s + SSPP_SRC_XY, 0);
        mmio::write32(s + SSPP_OUT_SIZE, size);
        mmio::write32(s + SSPP_OUT_XY, 0);
        mmio::write32(s + SSPP_SRC0_ADDR, fb_addr);
        mmio::write32(s + SSPP_SRC_YSTRIDE0, stride);
        mmio::write32(s + SSPP_SRC_FORMAT, XRGB8888_SRC_FORMAT);
        mmio::write32(s + SSPP_SRC_UNPACK, XRGB8888_UNPACK);
        mmio::write32(s + SSPP_SRC_OP_MODE, OP_PE_OVERRIDE);

        mmio::write32(s + SSPP_SW_PIX_EXT_C0_LR, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C0_TB, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C0_REQ, size);
        mmio::write32(s + SSPP_SW_PIX_EXT_C1C2_LR, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C1C2_TB, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C1C2_REQ, size);
        mmio::write32(s + SSPP_SW_PIX_EXT_C3_LR, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C3_TB, 0);
        mmio::write32(s + SSPP_SW_PIX_EXT_C3_REQ, size);
    }
    dsb();

    // LM_0
    unsafe {
        mmio::write32(LM_0 + LM_OUT_SIZE, size);
        mmio::write32(LM_0 + LM_BLEND0_OP, 0x10);
        mmio::write32(LM_0 + LM_BLEND0_CONST_ALPHA, 0x00FF_0000);
    }
    dsb();

    // CTL_0 — DMA0 at blend stage 0
    // DMA0 mix=1 at bit 18 in CTL_LAYER
    unsafe {
        let c = CTL_0;
        mmio::write32(c + CTL_LAYER_LM0, 1 << 18); // DMA0 at stage 0
        mmio::write32(c + CTL_LAYER_EXT_LM0, 0);
        mmio::write32(c + CTL_LAYER_EXT2_LM0, 0);
        mmio::write32(c + CTL_LAYER_EXT3_LM0, 0);
        mmio::write32(c + CTL_INTF_ACTIVE, INTF1_BIT);
        mmio::write32(c + CTL_FETCH_PIPE_ACTIVE, FETCH_DMA0);

        // DMA0 flush bit = BIT(11)
        let flush_dma0: u32 = 1 << 11;
        mmio::write32(c + CTL_INTF_FLUSH, INTF1_BIT);
        mmio::write32(c + CTL_FLUSH, flush_dma0 | FLUSH_LM0 | FLUSH_CTL | FLUSH_INTF);
    }
    dsb();

    unsafe {
        mmio::write32(CTL_0 + CTL_START, 1);
    }
    dsb();
}

#[inline(always)]
fn dsb() {
    unsafe { core::arch::asm!("dsb sy") };
}
