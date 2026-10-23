//! The cold boot's UFS link-startup probe, behind `bridge diag ufs` (it used to run on every cold boot and fail).
//!
//! On a cold boot ABL leaves the UFS device POWERED OFF (see `power_on_device`), which is why every register-identical bring-up attempt met a silent device (UFS.md). With VCC restored, full_init's link startup succeeds. This keeps the measurement runnable on demand: the external-block and clock-tree state ABL left, the pristine register file, the PMA/PA snapshots before and after, the full_init report, the ferros-side MMIO trace and the full PMA dump, under the same labels as the old boot-time DIAG so new captures diff against the old ones.
//!
//! Changed from the boot-time version: the PMU reads go through the EL3 monitor (a direct EL2 PMU read makes the next EL3 call hang, which would break the fastboot exit), and the experiments FAITHFUL_REPLAY had already switched off are gone (git history and UFS.md keep them).

use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

use crate::platform::{append_hex, priv_reg};

const UFS_BASE: usize = 0x1320_0000;
/// HSI2 S2MPU in front of the UFS DMA master.
const S2MPU_HSI2: usize = 0x131F_0000;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}

/// GPIO_PERIC0 bank gpp0 (zuma.dtsi pinctrl@10840000): CON at +0 (4 bits per pin), DAT at +4. gpp0-1 switches the UFS device's VCC rail (zuma-ufs.dtsi ufs_fixed_vcc, enable-active-high).
const GPP0: usize = 0x1084_0000;

/// Power the UFS device back on. ABL's UFS shutdown (reverse-engineered from ripcurrent-16.4-14540574: vendor hook stage 10, called right after it reads the boot image and before it jumps to the kernel) hibernates the link, asserts the device reset, re-purposes the gph5-0 refclk pin and drives gpp0-1 LOW — the flash device is unpowered when ferros starts. Linux's regulator framework turns it back on at probe; ferros never did, which is the whole "device silent at link startup" wall in UFS.md. Measured: gpp0 CON G#2010 (pin 1 output), DAT G#0 at handoff; with DAT bit 1 set, link startup succeeds first try (LS_RES 0, MXGR 4, two RX lanes).
pub fn power_on_device() {
    let con = rd(GPP0);
    wr(GPP0, (con & !(0xF << 4)) | (1 << 4)); // gpp0-1 = output
    wr(GPP0 + 4, rd(GPP0 + 4) | (1 << 1)); // drive high: VCC on
    unsafe { core::arch::asm!("dsb sy") };
    // Let the rail settle before full_init's device reset pulse (the DT gives the fixed regulator no startup delay; 4 ms is generous).
    let freq = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while crate::platform::cntpct().wrapping_sub(t0) < freq / 250 {
        core::hint::spin_loop();
    }
}

/// Cold boot UFS bring-up for real use: DMA path open, VCC on, full_init, then one read to consume the power-on UNIT ATTENTION. Returns whether the device answers block reads (link up, NOP and LBA 0 read OK).
pub fn bring_up() -> bool {
    // UFS DMAs only into ferros's own buffers (`.bss.dma`, inside the window the fence allows).
    crate::s2mpu::fence();
    wr(0x1302_0710, rd(0x1302_0710) & !0x3); // non-coherent DMA (ferros does explicit cache maintenance)
    unsafe { core::arch::asm!("dsb sy") };
    let ufs = ferros_hal::ufs::UfsController::new(UFS_BASE);
    let answers = |ufs: &ferros_hal::ufs::UfsController| {
        for _ in 0..4 {
            if ufs.read_block(0) == 0 {
                let d = ufs.data_buffer();
                if d[0x1FE] == 0x55 && d[0x1FF] == 0xAA {
                    return true;
                }
            }
        }
        false
    };
    // A link already up (a RELOAD: the previous generation brought it up) is adopted — our own transfer list on it, proven by the block read — not reset. full_init on a live link came up and served the shell's first reads, then lost a command minutes later, every time (2026-10-23); the Linux handoff always adopted its link this way.
    // Only when the device is powered — gpp0-1 high, which only a previous generation of ferros does; ABL leaves HCE and the link bits set over a hibernated, unpowered device, and adopting that cost four 1 s timeouts on every cold boot (2026-10-23).
    if rd(GPP0 + 4) & (1 << 1) != 0 && ufs.link_is_up() {
        ufs.init_transfer_list();
        if answers(&ufs) {
            return true;
        }
    }
    power_on_device();
    let rep = ufs.full_init();
    if rep.fail_step != 0 && rep.nop_ocs != 0 {
        return false;
    }
    answers(&ufs)
}

/// Run the full cold UFS bring-up once and return the report text.
pub fn probe() -> Vec<u8> {
    let mut resp = Vec::new();
    resp.extend_from_slice(b"cold UFS link-startup probe (VCC on, then full_init from ABL's parked link)\n");
    let gpp0_dat_before = rd(GPP0 + 4);
    power_on_device();

    // UFS DMA straight to physical addresses (zuma UFS has no stage-1 SYSMMU; the S2MPU is the only gate): the fence, which allows ferros's window.
    crate::s2mpu::fence();

    // External-block state as ABL left it, read before any UFS touch (Linux's probe-time values: IOCC 3, PHY-ISO 1, UFSP RSECURITY G#FFFA6492, WSECURITY 0).
    let iocc = rd(0x1302_0710); // sysreg_ufs iocc
    let gph5con = rd(0x1306_0000); // refclk_out / reset_n pinmux
    let phy_iso = priv_reg(0x1546_3EC0, false, 0) as u32; // PMU ufs-phy-iso (EL3; G#FFFFFFEA = not whitelisted)
    let pd_peric0 = priv_reg(0x1546_2804, false, 0) as u32; // PMU PERIC0 domain status (EL3)
    let qch_peric0 = rd(0x2600_3290); // CMU_NOCL0 QCH_CON_SLH_AXI_SI_P_PERIC0
    // UFSP sits behind the auto-gated UFS clock domain: clear the auto-stop enables and forced stops before reading it (reading with the gate armed bus-hangs), then put ABL's values back so full_init starts from the state Linux's probe sees.
    let found_b4 = rd(0x1320_11B4); // HCI_FORCE_HCS, ABL leaves G#900
    let found_b0 = rd(0x1320_11B0); // HCI_CLKSTOP_CTRL, ABL leaves G#10
    wr(0x1320_11B4, found_b4 & !0xFF0);
    wr(0x1320_11B0, found_b0 & !0x1F);
    unsafe { core::arch::asm!("dsb sy") };
    let ufsp_rsec = rd(0x132A_0010);
    let ufsp_wsec = rd(0x132A_0110);
    let s2mpu_ctrl = rd(S2MPU_HSI2);
    let dbg_prd = rd(0x1328_0044); // pristine UNIPRO DBG_PRD: G#78 = 133 MHz (ABL), G#59 = 178 MHz
    // mclk clock tree (CMU_TOP G#2604_0000 + CMU_HSI2 G#1300_0000).
    let cmu = [
        rd(0x2604_014C), // PLL_CON3_PLL_SHARED0
        rd(0x2604_01CC), // PLL_CON3_PLL_SHARED2
        rd(0x2604_024C), // PLL_CON3_PLL_SPARE
        rd(0x2604_10B8), // MUX_CLKCMU_HSI2_UFS_EMBD
        rd(0x2604_18B0), // DIV_CLKCMU_HSI2_UFS_EMBD
        rd(0x2604_20E0), // GATE_CLKCMU_HSI2_UFS_EMBD
        rd(0x1300_0630), // PLL_CON0_MUX_CLKCMU_HSI2_UFS_EMBD_USER
        rd(0x1300_210C), // leaf gate I_ACLK
        rd(0x1300_2110), // leaf gate I_CLK_UNIPRO
        rd(0x1300_2114), // leaf gate I_FMP_CLK
    ];
    wr(0x1320_11B4, found_b4);
    wr(0x1320_11B0, found_b0);
    // ferros does non-coherent DMA (explicit cache maintenance), so clear the coherent/shareable bits ABL sets.
    wr(0x1302_0710, iocc & !0x3);
    unsafe { core::arch::asm!("dsb sy") };
    let iocc_after = rd(0x1302_0710);

    // Pristine register file, read-only.
    let r = |off: usize| rd(UFS_BASE + off);
    let regfile = [r(0x00), r(0x08), r(0x20), r(0x24), r(0x30), r(0x34), r(0x50), r(0x54), r(0x58), r(0x5C), r(0x60), r(0x70), r(0x80), r(0x90)];

    let ufs = ferros_hal::ufs::UfsController::new(UFS_BASE);
    let phys = ufs.utrd_phys();
    let hcs_pristine = r(0x30);
    let pmaw = ufs.snapshot_pma(); // ABL's working (hibernated) link
    let rep = ufs.full_init();
    let pmaf = ufs.snapshot_pma(); // after the startup (failed or not)
    // If the device answered, read LBA 0 (the GPT's protective MBR) — the end-to-end proof.
    // The first command after the device's power-on reset typically completes with OCS 0 but SCSI CHECK CONDITION (UNIT ATTENTION: power on occurred) and no data, so retry until the signature shows (first cold run: OCS 0, signature 0).
    let (mut read_ocs, mut mbr_sig, mut read_tries) = (0xFFu32, 0u32, 0u32);
    if rep.fail_step == 0 || rep.nop_ocs == 0 {
        for t in 1..=4u32 {
            read_tries = t;
            read_ocs = ufs.read_block(0) as u32;
            let d = ufs.data_buffer();
            mbr_sig = u16::from_le_bytes([d[0x1FE], d[0x1FF]]) as u32;
            if read_ocs == 0 && mbr_sig == 0xAA55 {
                break;
            }
        }
    }

    append_hex(&mut resp, b"UFS_GPP0_DAT_BEFORE=", gpp0_dat_before);
    append_hex(&mut resp, b"UFS_GPP0_DAT_AFTER=", rd(GPP0 + 4));
    append_hex(&mut resp, b"UFS_DBG_PRD=", dbg_prd);
    append_hex(&mut resp, b"UFS_EXT_IOCC=", iocc);
    append_hex(&mut resp, b"UFS_EXT_PHYISO=", phy_iso);
    append_hex(&mut resp, b"UFS_EXT_UFSP_RSEC=", ufsp_rsec);
    append_hex(&mut resp, b"UFS_EXT_UFSP_WSEC=", ufsp_wsec);
    append_hex(&mut resp, b"UFS_EXT_S2MPU_CTRL=", s2mpu_ctrl);
    append_hex(&mut resp, b"UFS_EXT_GPH5CON=", gph5con);
    append_hex(&mut resp, b"UFS_PD_PERIC0=", pd_peric0);
    append_hex(&mut resp, b"UFS_QCH_P_PERIC0=", qch_peric0);
    let cmu_labels: [&[u8]; 10] = [b"UFS_CMU_PLL_SH0=", b"UFS_CMU_PLL_SH2=", b"UFS_CMU_PLL_SPARE=", b"UFS_CMU_TOP_MUX=", b"UFS_CMU_TOP_DIV=", b"UFS_CMU_TOP_GATE=", b"UFS_CMU_USER_MUX=", b"UFS_CMU_LEAF_ACLK=", b"UFS_CMU_LEAF_UNIPRO=", b"UFS_CMU_LEAF_FMP="];
    for (l, v) in cmu_labels.iter().zip(cmu) {
        append_hex(&mut resp, l, v);
    }
    let rf_labels: [&[u8]; 14] = [b"UFS_RF_CAP=", b"UFS_RF_VER=", b"UFS_RF_IS=", b"UFS_RF_IE=", b"UFS_RF_HCS=", b"UFS_RF_HCE=", b"UFS_RF_UTRLBA=", b"UFS_RF_UTRLBAU=", b"UFS_RF_UTRLDBR=", b"UFS_RF_UTRLCLR=", b"UFS_RF_UTRLRSR=", b"UFS_RF_UTMRLBA=", b"UFS_RF_UTMRLRSR=", b"UFS_RF_UICCMD="];
    for (l, v) in rf_labels.iter().zip(regfile) {
        append_hex(&mut resp, l, v);
    }
    append_hex(&mut resp, b"UFS_IOCC_BEFORE=", iocc);
    append_hex(&mut resp, b"UFS_IOCC_AFTER=", iocc_after);
    append_hex(&mut resp, b"UFS_BUF_PHYS_HI=", (phys >> 32) as u32);
    append_hex(&mut resp, b"UFS_BUF_PHYS_LO=", phys as u32);
    append_hex(&mut resp, b"UFS_HCS_PRISTINE=", hcs_pristine);
    append_hex(&mut resp, b"UFS_READ_LBA0_OCS=", read_ocs);
    append_hex(&mut resp, b"UFS_MBR_SIG=", mbr_sig);
    append_hex(&mut resp, b"UFS_READ_TRIES=", read_tries);
    append_hex(&mut resp, b"UFS_STEPS=", rep.steps);
    append_hex(&mut resp, b"UFS_FAIL=", rep.fail_step);
    append_hex(&mut resp, b"UFS_LS_RES=", rep.linkstartup_res);
    append_hex(&mut resp, b"UFS_MXGR_AFTER=", rd(0x1328_0000 + 0x321C));
    append_hex(&mut resp, b"UFS_CAL_TIMEOUTS=", rep.cal_timeouts);
    append_hex(&mut resp, b"UFS_DME_ERR=", rep.dme_err);
    append_hex(&mut resp, b"UFS_PA_STATE=", rep.pa_state);
    append_hex(&mut resp, b"UFS_PA_TX_STATE=", rep.pa_tx_state);
    append_hex(&mut resp, b"UFS_GPH5CON_BEFORE=", rep.gph5_con_before);
    append_hex(&mut resp, b"UFS_GPH5CON_AFTER=", rep.gph5_con_after);
    append_hex(&mut resp, b"UFS_PCS_READBACK=", rep.pcs_readback);
    append_hex(&mut resp, b"UFS_GPH5_DAT=", rep.gph5_dat);
    append_hex(&mut resp, b"UFS_UEC_PACK=", rep.uec_pack);
    append_hex(&mut resp, b"UFS_AVAIL_RX=", rep.avail_rx);
    append_hex(&mut resp, b"UFS_LS_CNF=", rep.ls_cnf);
    append_hex(&mut resp, b"UFS_NOP_OCS=", rep.nop_ocs as u32);
    append_hex(&mut resp, b"UFS_HCS=", r(0x30));
    append_hex(&mut resp, b"UFS_HCS_LINK=", rep.hcs_after_link);
    append_hex(&mut resp, b"UFS_UECPA=", rep.uecpa);
    append_hex(&mut resp, b"UFS_LS_TRIES=", rep.linkstartup_tries);
    append_hex(&mut resp, b"UFS_CLKSTOP=", rep.clkstop_ctrl);
    append_hex(&mut resp, b"UFS_FORCEHCS=", rep.force_hcs);
    append_hex(&mut resp, b"UFS_MPHY_REFCLK_SEL=", rep.mphy_refclk_sel);
    append_hex(&mut resp, b"UFS_CMU_QCH=", rep.cmu_qch);
    append_hex(&mut resp, b"UFS_CMU_UNIPRO_GATE=", rep.cmu_unipro_gate);

    // Working-vs-failed PMA/PA snapshot pairs.
    let pma_labels: [&[u8]; 12] = [b"P000", b"P140", b"P150", b"P19C", b"P1A0", b"PC74", b"P9F0", b"P9F4", b"PA00", b"PACS", b"PATX", b"MXGR"];
    for (prefix, snap) in [(b"UFS_W_", &pmaw), (b"UFS_F_", &pmaf)] {
        for (i, l) in pma_labels.iter().enumerate() {
            let mut label = Vec::with_capacity(12);
            label.extend_from_slice(prefix);
            label.extend_from_slice(l);
            label.push(b'=');
            append_hex(&mut resp, &label, snap[i]);
        }
    }

    // ferros-side FWTRACE: every MMIO access full_init issued, in order, to diff against Linux's captured working trace. Addr bit31 set = READ, clear = WRITE.
    let hex = b"0123456789ABCDEF";
    append_hex(&mut resp, b"UFS_TRACE_N=", ferros_hal::ufs_cal::trace_len() as u32);
    for i in 0..ferros_hal::ufs_cal::trace_len().min(600) {
        let (a, v) = ferros_hal::ufs_cal::trace_get(i);
        let mut label = Vec::with_capacity(16);
        label.extend_from_slice(b"FTRACE_");
        for shift in [28, 24, 20, 16, 12, 8, 4, 0] {
            label.push(hex[((a >> shift) & 0xF) as usize]);
        }
        label.push(b'=');
        append_hex(&mut resp, &label, v);
    }
    // Full PMA analog capture at the failed link startup: [0..128] COMN G#000-G#1FC, [128..384] TRSV G#800 onward.
    for i in 0..384usize {
        let v = unsafe { ferros_hal::ufs_cal::PMA_FULL[i] };
        let (tag, off) = if i < 128 { (b'C', (i * 4) as u32) } else { (b'T', (0x800 + (i - 128) * 4) as u32) };
        let mut label = Vec::with_capacity(12);
        label.extend_from_slice(b"PMA_");
        label.push(tag);
        for shift in [12, 8, 4, 0] {
            label.push(hex[((off >> shift) & 0xF) as usize]);
        }
        label.push(b'=');
        append_hex(&mut resp, &label, v);
    }
    resp.extend_from_slice(b"END\n");
    resp
}
