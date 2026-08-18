//! Zuma UFS M-PHY / UNIPRO link calibration — Rust port of Samsung's `ufs-cal-if` (zuma tables, kernel `drivers/ufs/zuma/`).
//!
//! The Exynos UFS host does not bring up a working UniPro link from a bare HCE enable: the M-PHY analog block (PMA), the PHY PCS, and a handful of UNIPRO debug registers must be programmed between HCE=1 and DME_LINKSTARTUP (`pre_link`), again after the link is up (`post_link`), and around the power-mode change to HS gears (`pre_pmc`).
//! ABL runs the LK build of this exact library at boot; this is the same table data (evt0 == evt1 for zuma) replayed from our own driver so the controller can be fully re-initialized without ABL.
//!
//! Fixed parameters, measured from ABL's programmed state on husky (payloads/ufsdump):
//! - mclk (UNIPRO main clock) = 133.33 MHz: ABL's UNIPRO_DBG_PRD reads A#120 = (16*1000*1e6)/rate, PLL_SHARED0(2133MHz)/4/4
//! - 38.4 MHz refclk variant (USE_38_4_MHZ), 2 lanes available/connected
//! - evt_ver 0, board filter unused (all rows BRD_ALL), AH8 cal off (no samsung,support-ah8 in the zuma DT) so the HCI_AH8_* rows are omitted entirely
//!
//! CRITICAL access rule, the cause of every historical "vendor region hangs the AP" freeze: the PMA APB clock (and UNIPRO mclk) are HARDWARE-auto-gated when the link idles, controlled by the auto-stop enables in HCI_FORCE_HCS (vendor block +G#B4).
//! Any PMA read with MPHY_APBCLK_STOP_EN set bus-hangs the AP with no fault.
//! The caller (ufs::full_init) must clear the FORCE_HCS enables before calling in here — mirroring the reference driver's `ufs_call_cal` bracket.

/// Region bases on Pixel 8 (zuma), from the zuma-ufs.dtsi reg list.
pub mod base {
    /// Standard UFSHCI block.
    pub const STD: usize = 0x1320_0000;
    /// Vendor-specified HCI block ("reg_hci").
    pub const HCI: usize = 0x1320_1100;
    /// UNIPRO block (also carries PCS access via the AXI AUX lane-select window).
    pub const UNIPRO: usize = 0x1328_0000;
    /// M-PHY PMA block — APB is auto-gated, see module docs.
    pub const PMA: usize = 0x1320_4000;
}

/// UNIPRO main clock, Hz. See module docs for the derivation.
/// UNIPRO main clock, Hz. 178 MHz — confirmed two ways: Linux's exynos-ufs logs `mclk: 178000000` (clk_get_rate of clk_unipro), and HCI_1US_TO_CNT_VAL (= mclk/1e6) reads G#B2 = 178 on husky. The earlier 133 MHz derivation (from DBG_PRD=G#78) was WRONG — calibrating the M-PHY for 133 against the real 178 mistunes the PHY and DME_LINKSTARTUP fails with the device silent. All derived constants below recompute from this.
pub const MCLK_RATE: u64 = 178_000_000;

/// 1e9 / MCLK_RATE truncated (`__get_mclk_period`) — mclk period in ns. = 5 at 178 MHz.
const MCLK_PERIOD: u32 = (1_000_000_000u64 / MCLK_RATE) as u32;
/// 1e9 / MCLK_RATE rounded half-up (`__get_round_off`) — the PCS PRD registers take the rounded value. = 6 at 178 MHz.
const MCLK_PERIOD_RND: u32 = ((1_000_000_000u64 + MCLK_RATE / 2) / MCLK_RATE) as u32;
/// (16 * 1000 * 1_000_000) / MCLK_RATE = 1.6e10 / rate — the UNIPRO "period for 1.8" debug value (`__get_mclk_period_unipro_18`). = 89 at 178 MHz.
const MCLK_PERIOD_UNIPRO_18: u32 = (16_000_000_000u64 / MCLK_RATE) as u32;

/// TX line reset time, ticks of mclk for 3200us.
const TX_LINE_RESET_TICKS: u32 = ((MCLK_RATE * 3200) / 1_000_000) as u32;
/// RX line reset detect time, ticks of mclk for 1000us.
const RX_LINE_RESET_TICKS: u32 = ((MCLK_RATE * 1000) / 1_000_000) as u32;

const AUX_FIELD: usize = 0x040; // UNIP_COMP_AXI_AUX_FIELD — PCS lane-select window
const WSTRB: u32 = 0xF << 24;
const TX_LANE_0: u32 = 0;
const RX_LANE_0: u32 = 4;
const PMA_LANE_STRIDE: usize = 0x800;

fn rd(addr: usize) -> u32 {
    trace_read32(addr)
}
fn wr(addr: usize, val: u32) {
    trace_write32(addr, val);
}

/// MMIO write trace for a ferros-side FWTRACE: records (addr, val) pairs so full_init's exact register sequence can be diffed against Linux's captured working trace. Flat array of [addr, val, addr, val, ...].
const TRACE_CAP: usize = 4096;
pub static mut TRACE: [u32; TRACE_CAP] = [0; TRACE_CAP];
pub static mut TRACE_N: usize = 0;
pub static mut TRACE_ON: bool = false;

/// Begin recording MMIO writes (clears the buffer).
pub fn trace_reset() {
    unsafe {
        TRACE_N = 0;
        TRACE_ON = true;
    }
}
/// Stop recording.
pub fn trace_stop() {
    unsafe { TRACE_ON = false };
}
/// Number of recorded (addr,val) pairs.
pub fn trace_len() -> usize {
    unsafe { TRACE_N / 2 }
}
/// Recorded pair i as (addr, val).
pub fn trace_get(i: usize) -> (u32, u32) {
    unsafe { (TRACE[i * 2], TRACE[i * 2 + 1]) }
}

/// Write an MMIO register, recording it when tracing is on. All UFS register writes (cal + driver) route through this.
pub fn trace_write32(addr: usize, val: u32) {
    unsafe {
        if TRACE_ON && TRACE_N + 2 <= TRACE_CAP {
            TRACE[TRACE_N] = addr as u32;
            TRACE[TRACE_N + 1] = val;
            TRACE_N += 2;
        }
        crate::mmio::write32(addr, val);
    }
}

/// Read an MMIO register, recording (addr | bit31, value) when tracing is on. Bit31 tags the entry as a READ — all UFS MMIO physical addresses sit under G#8000_0000, so the bit is free. Consecutive duplicate reads (same addr, same value as the previous entry) are collapsed to match Linux's FRTRACE dedup, so the two read streams align for a first-divergence diff.
pub fn trace_read32(addr: usize) -> u32 {
    unsafe {
        let val = crate::mmio::read32(addr);
        if TRACE_ON && TRACE_N + 2 <= TRACE_CAP {
            let tagged = addr as u32 | 0x8000_0000;
            let dup = TRACE_N >= 2 && TRACE[TRACE_N - 2] == tagged && TRACE[TRACE_N - 1] == val;
            if !dup {
                TRACE[TRACE_N] = tagged;
                TRACE[TRACE_N + 1] = val;
                TRACE_N += 2;
            }
        }
        val
    }
}

/// Busy-wait using the generic timer (CNTPCT/CNTFRQ) — no dependency on kernel time infra.
pub fn udelay(us: u64) {
    let freq: u64;
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq) };
    let ticks = (freq * us) / 1_000_000;
    let start = crate::qtimer::read_qtimer();
    while crate::qtimer::read_qtimer().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

/// One calibration row. Addresses are offsets into their region; PMA TRSV rows add G#800 per lane, PCS rows go through the AUX lane-select window.
#[derive(Clone, Copy)]
enum Cfg {
    /// UNIPRO_DBG_PRD — write MCLK_PERIOD_UNIPRO_18 to a UNIPRO reg.
    DbgPrd(u16),
    /// PHY_PCS_COMN — lane-independent PCS reg (applied once).
    PcsComn(u16, u32),
    /// PHY_PCS_RX / PHY_PCS_TX — per-lane PCS reg.
    PcsRx(u16, u32),
    PcsTx(u16, u32),
    /// PHY_PCS_{RX,TX}_PRD_ROUND_OFF — per-lane PCS reg taking MCLK_PERIOD_RND.
    PcsRxPrdRnd(u16),
    PcsTxPrdRnd(u16),
    /// PHY_PCS_{RX,TX}_LR_PRD — three consecutive PCS regs taking the 24-bit line-reset tick count.
    PcsRxLrPrd(u16),
    PcsTxLrPrd(u16),
    /// UNIPRO_STD_MIB / UNIPRO_DBG_MIB / UNIPRO_DBG_APB — plain UNIPRO reg writes (applied once).
    UStd(u16, u32),
    UDbg(u16, u32),
    UApb(u16, u32),
    /// UNIPRO_ADAPT_LENGTH — read-modify-write PA adapt length quirk (applied once).
    UAdapt(u16),
    /// PHY_PMA_COMN — lane-independent PMA reg (applied once).
    PmaComn(u16, u32),
    /// PHY_PMA_TRSV — per-lane PMA reg.
    PmaTrsv(u16, u32),
    /// PHY_EMB_CAL_WAIT — poll a per-lane PMA reg for a mask, ~100 x 40us. The reference kernel build treats timeout as non-fatal; we mirror that but report it.
    EmbCalWait(u16, u32),
}
use Cfg::*;

/// Lane counts for the cal engine. `available_lane` bounds the per-lane loop; `active_rx_lane`/`connected_rx_lane` gate CDR/SQ rows (unused by the zuma non-AH8 tables we carry, kept for fidelity).
#[derive(Clone, Copy)]
pub struct CalParams {
    pub available_lane: u32,
    pub connected_rx_lane: u32,
    pub active_rx_lane: u32,
}

/// PCS write: select the lane through the AUX window, write, restore write-strobe-only.
fn set_pcs(lane: u32, offset: u16, value: u32) {
    wr(base::UNIPRO + AUX_FIELD, WSTRB | (lane & 0xFFFF));
    wr(base::UNIPRO + offset as usize, value);
    wr(base::UNIPRO + AUX_FIELD, WSTRB);
}

/// PCS read-back: select the lane through the AUX window WITHOUT the write strobe, then read the offset. Used to verify the PCS cal actually landed (the AUX-window path, distinct from direct PMA writes). RX lanes use index 4+lane, TX lanes 0+lane.
pub fn read_pcs(lane: u32, offset: u16) -> u32 {
    wr(base::UNIPRO + AUX_FIELD, lane & 0xFFFF);
    let v = rd(base::UNIPRO + offset as usize);
    wr(base::UNIPRO + AUX_FIELD, WSTRB);
    v
}

/// RX lane 0 AUX index (matches the cal engine's RX_LANE_0).
pub const RX_LANE0: u32 = 4;
/// TX lane 0 AUX index.
pub const TX_LANE0: u32 = 0;

fn pcs_lr_prd(lane: u32, offset: u16, ticks: u32) {
    set_pcs(lane, offset, (ticks >> 16) & 0xFF);
    set_pcs(lane, offset + 4, (ticks >> 8) & 0xFF);
    set_pcs(lane, offset + 8, ticks & 0xFF);
}

/// UNIPRO_ADAPT_LENGTH quirk from `__config_uic`: value with bit7 set is a "skip" encoding needing a minimum of 2; otherwise round up so (value+1) is a multiple of 4.
fn adapt_length(offset: u16) {
    let addr = base::UNIPRO + offset as usize;
    let v = rd(addr);
    if v & 0x80 != 0 {
        if v & 0x7F < 2 {
            wr(addr, 0x82);
        }
    } else if (v + 1) % 4 != 0 {
        let mut n = v;
        while (n + 1) % 4 != 0 {
            n += 1;
        }
        wr(addr, n);
    }
}

/// Apply one table. Returns the number of EmbCalWait rows that timed out (0 = fully clean).
fn apply(table: &[Cfg], p: CalParams) -> u32 {
    let mut wait_timeouts = 0u32;
    for &cfg in table {
        for lane in 0..p.available_lane {
            // Lane-independent rows apply only on the first lane pass, mirroring the reference skip table.
            if lane > 0 {
                match cfg {
                    PcsComn(..) | DbgPrd(..) | UStd(..) | UDbg(..) | UApb(..) | UAdapt(..)
                    | PmaComn(..) => continue,
                    _ => {}
                }
            }
            let pma_lane = base::PMA + PMA_LANE_STRIDE * lane as usize;
            match cfg {
                DbgPrd(off) => wr(base::UNIPRO + off as usize, MCLK_PERIOD_UNIPRO_18),
                PcsComn(off, v) => wr(base::UNIPRO + off as usize, v),
                PcsRx(off, v) => set_pcs(RX_LANE_0 + lane, off, v),
                PcsTx(off, v) => set_pcs(TX_LANE_0 + lane, off, v),
                PcsRxPrdRnd(off) => set_pcs(RX_LANE_0 + lane, off, MCLK_PERIOD_RND),
                PcsTxPrdRnd(off) => set_pcs(TX_LANE_0 + lane, off, MCLK_PERIOD_RND),
                PcsRxLrPrd(off) => pcs_lr_prd(RX_LANE_0 + lane, off, RX_LINE_RESET_TICKS),
                PcsTxLrPrd(off) => pcs_lr_prd(TX_LANE_0 + lane, off, TX_LINE_RESET_TICKS),
                UStd(off, v) | UDbg(off, v) | UApb(off, v) => wr(base::UNIPRO + off as usize, v),
                UAdapt(off) => adapt_length(off),
                PmaComn(off, v) => wr(base::PMA + off as usize, v),
                PmaTrsv(off, v) => wr(pma_lane + off as usize, v),
                EmbCalWait(off, mask) => {
                    let mut ok = false;
                    for _ in 0..128u32 {
                        udelay(40);
                        if rd(pma_lane + off as usize) & mask == mask {
                            ok = true;
                            break;
                        }
                    }
                    if !ok {
                        wait_timeouts += 1;
                    }
                }
            }
        }
    }
    wait_timeouts
}

/// `init_cfg_evt0` — pre-link calibration (between HCE=1 and DME_LINKSTARTUP). 38.4 MHz refclk variant.
#[rustfmt::skip]
static INIT_CFG: [Cfg; 57] = [
    DbgPrd(0x44),
    PcsComn(0x2800, 0x40),
    PcsComn(0x2808, 0x22),
    PcsRxPrdRnd(0x2048),
    PcsTxPrdRnd(0x22A8),
    PcsTx(0x22A4, 0x02),
    PcsTxLrPrd(0x22AC),
    PcsRx(0x2044, 0x00),
    PcsRxLrPrd(0x206C),
    PcsRx(0x20BC, 0x79),
    PcsRx(0x2210, 0x01),
    PcsTx(0x2010, 0x01),
    PcsRx(0x2094, 0xF6),
    PcsTx(0x21FC, 0x00),
    PcsComn(0x2800, 0x00),
    UStd(0x3178, 0x0),
    UStd(0x5000, 0x0),
    UStd(0x5004, 0x1),
    UStd(0x6084, 0x1),
    UStd(0x6080, 0x1),
    PmaComn(0x140, 0x08),
    PmaComn(0x014, 0x19),
    PmaComn(0x02C, 0x44),
    PmaComn(0x030, 0xC4),
    PmaComn(0x034, 0xC3),
    PmaComn(0x03C, 0x88),
    PmaComn(0x058, 0x1A),
    PmaComn(0x064, 0x04),
    PmaComn(0x150, 0x88),
    PmaComn(0x19C, 0x4C),
    PmaComn(0x1A0, 0x4C),
    PmaTrsv(0x804, 0x44),
    PmaTrsv(0x808, 0x44),
    PmaTrsv(0x80C, 0x00),
    PmaTrsv(0x810, 0x18),
    PmaTrsv(0x814, 0xC0),
    PmaTrsv(0x81C, 0x1C),
    PmaTrsv(0xBB0, 0x8C),
    PmaTrsv(0x9F0, 0xD0),
    PmaTrsv(0xA20, 0xFA),
    PmaTrsv(0xA24, 0x60),
    PmaTrsv(0x8D0, 0x30),
    PmaTrsv(0x8E4, 0x05),
    PmaTrsv(0x8F4, 0x05),
    PmaTrsv(0x934, 0x1A),
    PmaTrsv(0x938, 0x12),
    PmaTrsv(0x93C, 0x5E),
    PmaTrsv(0x964, 0x2A),
    PmaTrsv(0x980, 0x54),
    PmaTrsv(0x998, 0x54),
    PmaTrsv(0x9CC, 0x00),
    PmaTrsv(0x9D0, 0x00),
    PmaTrsv(0xAAC, 0x00),
    PmaTrsv(0xAB0, 0x02),
    PmaComn(0x140, 0x0C),
    PmaComn(0x140, 0x00),
    EmbCalWait(0xC74, 0x01),
];

/// `post_init_cfg_evt0` — post-link calibration (after DME_LINKSTARTUP succeeds).
#[rustfmt::skip]
static POST_INIT_CFG: [Cfg; 5] = [
    UAdapt(0x3348),
    UAdapt(0x334C),
    UDbg(0x38A4, 0x01),
    UStd(0x3290, 0x3E8),
    UDbg(0x38A4, 0x00),
];

/// `calib_of_hs_rate_b` — pre power-mode-change calibration for HS series B (any HS gear).
#[rustfmt::skip]
static CALIB_HS_RATE_B: [Cfg; 13] = [
    UStd(0x3350, 0x1),
    UStd(0x4104, 8064),
    UStd(0x4108, 28224),
    UStd(0x410C, 20160),
    UStd(0x32C0, 12000),
    UStd(0x32C4, 32000),
    UStd(0x32C8, 16000),
    UApb(0x7888, 8064),
    UApb(0x788C, 28224),
    UApb(0x7890, 20160),
    UApb(0x78B8, 12000),
    UApb(0x78BC, 32000),
    UApb(0x78C0, 16000),
];

/// Pre-link cal. Caller must have HCE=1 and the FORCE_HCS clock-stop enables cleared.
pub fn pre_link(p: CalParams) -> u32 {
    apply(&INIT_CFG, p)
}

/// Post-link cal. If only 1 lane connected, also powers down lane 1's squelch (`lane1_sq_off` — the reference PMA_TRSV_LANE1_SQ_OFF layer applies only to the unconnected lane). Unused on husky (2 lanes connected).
pub fn post_link(p: CalParams) -> u32 {
    let t = apply(&POST_INIT_CFG, p);
    if p.available_lane == 2 && p.connected_rx_lane == 1 {
        let lane1 = base::PMA + PMA_LANE_STRIDE;
        wr(lane1 + 0x9F4, 0x08);
        wr(lane1 + 0xA00, 0x3A);
    }
    t
}

/// Pre-hibernate-EXIT cal (`ufs_cal_pre_h8_exit`, zuma `pre_h8_exit` table). ABL parks the link in HIBERN8 before handoff; the Exynos hibern8_notify EXIT/PRE_CHANGE hook ungates the internal clock and runs THIS before issuing DME_HIBERN8_EXIT. Without it the exit reaches the PHY in an untuned state and UPMCRS goes fatal. Caller must have the FORCE_HCS clock-stop enables cleared (unlock_clocks). Rows: PMA_COMN 0x000=0x11 (once), wait ~10us, per-connected-lane PMA_TRSV_SQ 0x9F4=0x00 / 0xA00=0x30, per-lane PMA_TRSV 0xB64=0x32 then 0x22 (write-then-write, table order is per-row-then-per-lane).
pub fn pre_h8_exit(p: CalParams) -> u32 {
    wr(base::PMA + 0x000, 0x11);
    udelay(10);
    for lane in 0..p.connected_rx_lane {
        let l = base::PMA + PMA_LANE_STRIDE * lane as usize;
        wr(l + 0x9F4, 0x00);
        wr(l + 0xA00, 0x30);
    }
    for lane in 0..p.available_lane {
        wr(base::PMA + PMA_LANE_STRIDE * lane as usize + 0xB64, 0x32);
    }
    for lane in 0..p.available_lane {
        wr(base::PMA + PMA_LANE_STRIDE * lane as usize + 0xB64, 0x22);
    }
    0
}

/// Pre power-mode-change cal for FAST mode, HS series B. Also masks PA_ERROR_IND_RECEIVED in the DL error IRQ shadow, as the reference does before every PMC.
pub fn pre_pmc_hs_b(p: CalParams) -> u32 {
    const DL_ERROR_IRQ_MASK: usize = 0x4844;
    const PA_ERROR_IND_RECEIVED: u32 = 1 << 15;
    let v = rd(base::UNIPRO + DL_ERROR_IRQ_MASK) | PA_ERROR_IND_RECEIVED;
    wr(base::UNIPRO + DL_ERROR_IRQ_MASK, v);
    apply(&CALIB_HS_RATE_B, p)
}
