//! GCC (Global Clock Controller) — clock branch and RCG configuration.
//!
//! QCM6490 GCC base: 0x0010_0000. Register offsets from Linux gcc-sc7280.c.
//!
//! CBCR (Clock Branch Control Register):
//!   - bit 0: CLK_ENABLE
//!   - bit 31: CLK_OFF (read-only, 1 = off, 0 = running)
//!
//! RCG2 (Root Clock Generator v2):
//!   - CMD_RCGR + 0x0: CMD (bit 0 = UPDATE trigger)
//!   - CMD_RCGR + 0x4: CFG (source select [8:10], divider [0:4], MND mode [12:13])
//!   - CMD_RCGR + 0x8: M value
//!   - CMD_RCGR + 0xC: N value (inverted: NOT(N-M))
//!   - CMD_RCGR + 0x10: D value (inverted: NOT(2*D))
//!
//! Frequency = SRC_CLK * M / N / (2 * HALF_DIV + 1) where HALF_DIV = CFG[4:0]

const GCC_BASE: usize = 0x0010_0000;

// SDC2 (microSD) clock registers
#[allow(dead_code)]
const GCC_SDCC2_BCR: usize = GCC_BASE + 0x14000;
const GCC_SDCC2_APPS_CBCR: usize = GCC_BASE + 0x14004;
const GCC_SDCC2_AHB_CBCR: usize = GCC_BASE + 0x14008;
const GCC_SDCC2_APPS_CMD_RCGR: usize = GCC_BASE + 0x1400C;
// CFG at CMD+4, M at CMD+8, N at CMD+0xC, D at CMD+0x10

// RCG2 source select values (from parent_map_9 in gcc-sc7280.c)
#[allow(dead_code)]
const RCG_SRC_TCXO: u32 = 0;       // BI_TCXO = 19.2 MHz
#[allow(dead_code)]
const RCG_SRC_GPLL0: u32 = 1;      // GPLL0_OUT_MAIN = 600 MHz
#[allow(dead_code)]
const RCG_SRC_GPLL0_EVEN: u32 = 6; // GPLL0_OUT_EVEN = 300 MHz

/// Enable a GCC clock branch (CBCR register). Returns true if the clock started running within the timeout.
unsafe fn cbcr_enable(addr: usize) -> bool {
    let val = crate::mmio::read32(addr);
    crate::mmio::write32(addr, val | 1); // set CLK_ENABLE

    // Poll CLK_OFF (bit 31) — wait for it to clear
    for _ in 0..100_000 {
        if crate::mmio::read32(addr) & (1 << 31) == 0 {
            return true;
        }
    }
    false
}

/// Read CLK_OFF status from a CBCR register. True = clock is off.
unsafe fn cbcr_is_off(addr: usize) -> bool {
    crate::mmio::read32(addr) & (1 << 31) != 0
}

/// Configure RCG2 clock rate for SDCC2 APPS clock.
///
/// From gcc-sc7280.c freq_tbl: 400KHz:  TCXO(19.2MHz), div=12, M=1, N=4  → 19.2M / 12 / 4 = 400K 25MHz:   GPLL0_EVEN(300MHz), div=12         → 300M / 12 = 25M 50MHz:   GPLL0_EVEN(300MHz), div=6           → 300M / 6 = 50M 100MHz:  GPLL0_EVEN(300MHz), div=3           → 300M / 3 = 100M
///
/// CFG_RCGR format: [4:0]  = 2*d - 1 (half-integer divider, 0 = div-1) [8:10] = source select [12:13] = MND mode (0 = bypass, 2 = dual-edge MND)
unsafe fn rcg2_configure(cmd_rcgr: usize, src: u32, div2m1: u32, m: u32, n: u32) {
    let cfg_addr = cmd_rcgr + 4;
    let m_addr = cmd_rcgr + 8;
    let n_addr = cmd_rcgr + 0xC;
    let d_addr = cmd_rcgr + 0x10;

    // Set M, N, D values
    if n > 0 {
        crate::mmio::write32(m_addr, m);
        // N register = NOT(N-M) for the MND counter
        crate::mmio::write32(n_addr, !(n - m) & 0xFF);
        // D register = NOT(2*D) where D = N (for 50% duty cycle)
        crate::mmio::write32(d_addr, !(n) & 0xFF);
    }

    // Build CFG: source select + divider + MND mode
    let mnd_mode = if n > 0 { 2u32 << 12 } else { 0 }; // dual-edge MND if using M/N
    let cfg = (src << 8) | div2m1 | mnd_mode;
    crate::mmio::write32(cfg_addr, cfg);

    // Trigger UPDATE (bit 0 of CMD_RCGR)
    crate::mmio::write32(cmd_rcgr, crate::mmio::read32(cmd_rcgr) | 1);

    // Wait for UPDATE to clear (config applied)
    for _ in 0..100_000 {
        if crate::mmio::read32(cmd_rcgr) & 1 == 0 {
            return;
        }
    }
}

/// Configure SDC2 APPS clock to 400KHz (identification mode). TCXO 19.2MHz / div12 with MND M=1,N=4 → 400KHz.
pub fn sdc2_set_400khz() {
    unsafe {
        // div = 12 → 2*d-1 = 23 = 0x17
        rcg2_configure(GCC_SDCC2_APPS_CMD_RCGR, RCG_SRC_TCXO, 0x17, 1, 4);
    }
}

/// Configure SDC2 APPS clock to 25MHz (data transfer mode). GPLL0_OUT_EVEN 300MHz / div12 → 25MHz.
pub fn sdc2_set_25mhz() {
    unsafe {
        // div = 12 → 2*d-1 = 23 = 0x17
        rcg2_configure(GCC_SDCC2_APPS_CMD_RCGR, RCG_SRC_GPLL0_EVEN, 0x17, 0, 0);
    }
}

/// Configure SDC2 APPS clock to 50MHz (high-speed mode). GPLL0_OUT_EVEN 300MHz / div6 → 50MHz.
#[allow(dead_code)]
pub fn sdc2_set_50mhz() {
    unsafe {
        // div = 6 → 2*d-1 = 11 = 0x0B
        rcg2_configure(GCC_SDCC2_APPS_CMD_RCGR, RCG_SRC_GPLL0_EVEN, 0x0B, 0, 0);
    }
}

/// Deassert SDC2 block reset (BCR). Must be called before enabling clocks or accessing SDHCI registers.
fn sdc2_deassert_reset() {
    unsafe {
        // Read current BCR — if bit 0 is set, block is held in reset
        let bcr = crate::mmio::read32(GCC_SDCC2_BCR);
        if bcr & 1 != 0 {
            // Deassert reset
            crate::mmio::write32(GCC_SDCC2_BCR, bcr & !1);
            // Short delay for reset release
            for _ in 0..1000 { core::hint::spin_loop(); }
        }
    }
}

/// Full GCC block reset: assert BCR, wait, deassert, wait. This resets the entire SDHCI controller hardware — clears all stale state.
pub fn sdc2_block_reset() {
    unsafe {
        // Assert reset (set bit 0)
        crate::mmio::write32(GCC_SDCC2_BCR, 1);
        for _ in 0..10_000u32 { core::hint::spin_loop(); }
        // Deassert reset (clear bit 0)
        crate::mmio::write32(GCC_SDCC2_BCR, 0);
        for _ in 0..10_000u32 { core::hint::spin_loop(); }
    }
}

/// Read the BCR register value (for diagnostics).
pub fn sdc2_bcr_raw() -> u32 {
    unsafe { crate::mmio::read32(GCC_SDCC2_BCR) }
}

/// Full SDC2 clock init: deassert reset, configure rate, enable branches. Returns (ahb_ok, apps_ok).
pub fn sdc2_clock_init() -> (bool, bool) {
    // 1. Deassert block reset
    sdc2_deassert_reset();

    // 2. Configure clock rate to 400KHz
    sdc2_set_400khz();

    // 3. Enable AHB (bus) and APPS (core) clock branches
    unsafe {
        let ahb = cbcr_enable(GCC_SDCC2_AHB_CBCR);
        let apps = cbcr_enable(GCC_SDCC2_APPS_CBCR);
        (ahb, apps)
    }
}

/// Enable SDC2 (microSD) clocks (branches only, assumes RCG configured). Returns (ahb_ok, apps_ok).
pub fn sdc2_clock_enable() -> (bool, bool) {
    unsafe {
        let ahb = cbcr_enable(GCC_SDCC2_AHB_CBCR);
        let apps = cbcr_enable(GCC_SDCC2_APPS_CBCR);
        (ahb, apps)
    }
}

/// Check if SDC2 clocks are currently running. Returns (ahb_running, apps_running).
pub fn sdc2_clock_status() -> (bool, bool) {
    unsafe {
        let ahb = !cbcr_is_off(GCC_SDCC2_AHB_CBCR);
        let apps = !cbcr_is_off(GCC_SDCC2_APPS_CBCR);
        (ahb, apps)
    }
}

/// Read the SDCC2 APPS CMD_RCGR register (clock rate config).
pub fn sdc2_cmd_rcgr() -> u32 {
    unsafe { crate::mmio::read32(GCC_SDCC2_APPS_CMD_RCGR) }
}

/// Read the SDCC2 APPS CFG_RCGR register (source select + divider).
pub fn sdc2_cfg_rcgr() -> u32 {
    unsafe { crate::mmio::read32(GCC_SDCC2_APPS_CMD_RCGR + 4) }
}

/// Read the raw CBCR values for diagnostics. Returns (ahb_cbcr, apps_cbcr).
pub fn sdc2_cbcr_raw() -> (u32, u32) {
    unsafe {
        (crate::mmio::read32(GCC_SDCC2_AHB_CBCR),
         crate::mmio::read32(GCC_SDCC2_APPS_CBCR))
    }
}
