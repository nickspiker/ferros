//! Post-handoff entry — ferros chainloaded from a running Linux shim.
//!
//! The shim kernel (Google GKI + exynos UFS module, built from source) brings
//! the machine up far enough that the UFS link is alive, then jumps here via
//! the kexec primitive `cpu_soft_restart(is_hyp_nvhe(), 0x92400000, ...)`.
//! That re-enters at EL2 with the MMU off — exactly the state ferros's `_entry`
//! fast path expects (it detects MMU-off and skips the cache/MMU teardown).
//!
//! The whole point: whatever off-chip / analog / device-firmware condition the
//! working Linux bring-up satisfies and ferros's own link startup does not, we
//! stop needing to know — Linux satisfies it, and ferros adopts the live link.
//!
//! M0 (this file) proves only the jump: stamp a result marker into the reserved
//! scratch and spin. The next Linux boot reads the scratch back (ferros_scratch
//! initcall in the shim kernel) — definitive even if no display is attached.
//! M1 layers the UFS adoption + SCSI read on top, writing block data to the same
//! scratch.

use core::ptr::write_volatile;

/// Result scratch — last 1 MB of the ferros_handoff carveout (zuma-rmem.dtsi:
/// ferros_handoff@92400000, 12 MB, no-map). Placed well past ferros's image +
/// 64 KB stack (all under ~3 MB from base), so writing it never touches code.
pub const SCRATCH: usize = 0x92F0_0000;

/// Proof ferros executed after the handoff. ASCII "FeRoShIm", little-endian.
pub const MAGIC: u64 = 0x6D_49_68_53_6F_52_65_46;

/// Milestone tag written at SCRATCH+16 so the readback distinguishes builds.
pub const STAGE_M0: u64 = 0x4D30; // "M0"
pub const STAGE_M1: u64 = 0x4D31; // "M1"

/// pixel8 / Tensor G3 UFS host base (UFSHCI). Same value the cold-boot path uses.
const UFS_BASE: usize = 0x1320_0000;
/// HSI2 S2MPU gating UFS DMA. Bypassed so the UFS master DMAs to physical
/// carveout addresses (zuma UFS has no stage-1 SysMMU — S2MPU is the only gate).
const S2MPU_HSI2: usize = 0x131F_0000;

/// Result drop inside the ramoops pmsg zone (zuma-rmem ramoops_mem@fd3ff000:
/// console [0xfd3ff000,0xfd5ff000), pmsg [0xfd5ff000,0xfd7ff000)). Unlike the
/// low carveout scratch, this DRAM survives a reset (bootloader preserves
/// ramoops), so the next boot's early initcall can read what ferros left here.
const RAMOOPS_RESULT: usize = 0xFD60_0000;
/// Marks a valid ferros result block at RAMOOPS_RESULT. "FeRoUfS1" LE.
const RESULT_MAGIC: u64 = 0x3153_6655_6F52_6546;

/// Diagnostic breadcrumb slots (survive a crash for the kernel readback):
///   +0x100 = 0xBEE1 written by _entry asm  (jump landed, ferros started)
///   +0x108 = 0xBEE2 written here in Rust   (BSS zero + stack + kernel_main OK)
pub const CRUMB_ENTRY: usize = SCRATCH + 0x100;
pub const CRUMB_RUST: usize = SCRATCH + 0x108;

/// M0 entry: stamp the scratch, prove the exception level, spin.
pub fn entry(x0: u64) -> ! {
    // Rust reached: BSS was zeroed, stack is set, kernel_main dispatched here.
    unsafe {
        write_volatile(CRUMB_RUST as *mut u64, 0xBEE2);
        ferros_hal::mmio::cache_clean(CRUMB_RUST, 8);
    }

    let el: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack));
    }

    unsafe {
        write_volatile(SCRATCH as *mut u64, MAGIC);
        write_volatile((SCRATCH + 8) as *mut u64, x0); // the arg Linux handed us (dtb phys or 0)
        write_volatile((SCRATCH + 16) as *mut u64, STAGE_M0);
        write_volatile((SCRATCH + 24) as *mut u64, el >> 2); // landed EL — must read 2
        // MMU is off here, so these stores go straight to DRAM; clean to PoC
        // anyway so the value survives a warm reset.
        ferros_hal::mmio::cache_clean(SCRATCH, 64);
    }

    // Success signal that does not depend on DRAM persistence: trigger a fast,
    // clean PSCI SYSTEM_RESET. If the handoff worked, the phone reboots in a few
    // seconds (vs the ~168s hang-then-fallback of a crash). A warm PSCI reset
    // may also preserve the scratch, so the recovery boot can read the magic.
    // PSCI SYSTEM_RESET = 0x8400_0009; SMC from EL2 traps to EL3 (TF-A).
    unsafe {
        core::arch::asm!(
            "mov w0, #0x0009",
            "movk w0, #0x8400, lsl #16",
            "smc #0",
            options(nomem, nostack, noreturn),
        );
    }
}

/// M1 entry: adopt the live UFS link Linux brought up and read LBA 0.
///
/// Wired once M0 proves the jump: the shim kernel's trigger moves from the M0
/// late_initcall to a hook in ufs-exynos.c's link-up POST path, so the handoff
/// fires only after Linux has the UFS link fully up (HS-G4, device initialized).
/// linkstartup and fDeviceInit are skipped entirely here — so whatever off-chip
/// condition made ferros's own link startup fail never gets exercised.
///
/// Scratch layout (all little-endian, read back by the M1 initcall):
///   [0]=MAGIC [1]=x0 [2]=STAGE_M1 [3]=landed_EL
///   [4]=link_up [5]=nop_ocs|nop_rsp<<8|read_ocs<<16|status<<24
///   +64: first 64 bytes of the block read from LBA 0
pub fn entry_m1(x0: u64) -> ! {
    use ferros_hal::ufs::UfsController;
    use core::ptr::write_volatile;

    let el: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack));
    }

    // Bypass the HSI2 S2MPU so the UFS master DMAs to our physical carveout
    // buffers (UFS_BUF is inside the reserved region). Disabling the DMA gate
    // does not disturb the M-PHY/UniPro link. zuma UFS has no stage-1 SysMMU.
    unsafe {
        write_volatile((S2MPU_HSI2 + 0x54) as *mut u32, 0xFF); // clear VID protection
        write_volatile((S2MPU_HSI2 + 0x00) as *mut u32, 0x00); // disable
        core::arch::asm!("dsb sy");
    }

    // Force the UFS AXI master to NON-coherent DMA. ABL/Linux set sysreg_ufs
    // iocc=3 (coherent: the master snoops CPU caches). ferros runs MMU-off so
    // SCTLR.C=0 — the CPU is out of the coherency domain and never answers the
    // snoop, so the master's coherent descriptor-fetch stalls forever = the dead
    // doorbell (DBR stuck, OCS=0xF). Clearing iocc makes it read DRAM directly,
    // where ferros's cache-cleaned descriptors already live.
    const SYSREG_UFS_IOCC: usize = 0x1302_0710;
    let iocc_before = unsafe { core::ptr::read_volatile(SYSREG_UFS_IOCC as *const u32) };
    unsafe {
        core::ptr::write_volatile(SYSREG_UFS_IOCC as *mut u32, 0);
        core::arch::asm!("dsb sy");
    }
    let iocc_after = unsafe { core::ptr::read_volatile(SYSREG_UFS_IOCC as *const u32) };

    let rd = |off: usize| unsafe { core::ptr::read_volatile((UFS_BASE + off) as *const u32) };
    let wr = |off: usize, v: u32| unsafe { core::ptr::write_volatile((UFS_BASE + off) as *mut u32, v) };

    // Adopt the controller. Manual rebase (NOT init_transfer_list, which clears
    // IS with 0xFFFFFFFF): on this Exynos controller IS bit 12 is a vendor RW
    // bit, not write-1-to-clear, so blanket-writing 1s SETS it (Linux never has
    // it set) and wedges the transfer engine. Clear IS the way Linux does — write
    // back only the bits that were actually set (W1C), never touching bit 12.
    let ufs = UfsController::new(UFS_BASE);
    let ferros_utrd = ufs.utrd_phys();
    let ie_before = rd(0x24);
    let is_pending = rd(0x20);
    wr(0x60, 0); // UTRLRSR = 0 (stop list so UTRLBA is sampled)
    wr(0x50, ferros_utrd as u32); // UTRLBA
    wr(0x54, (ferros_utrd >> 32) as u32); // UTRLBAU
    wr(0x20, is_pending); // clear pending IS via write-back (W1C) — no 0xFFFFFFFF
    wr(0x60, 1); // UTRLRSR = 1 (start)
    let is_after_rebase = rd(0x20);
    let utrlba_rb = rd(0x50);

    // Exynos vendor HCI register block (reg_hci @ 0x13201100) — SEPARATE from the
    // standard HCI base. The vendor DMA-engine / nexus registers live here.
    const HCI: usize = 0x1320_1100;
    let hci_r = |off: usize| unsafe { core::ptr::read_volatile((HCI + off) as *const u32) };
    let hci_w = |off: usize, v: u32| unsafe { core::ptr::write_volatile((HCI + off) as *mut u32, v) };

    // THE ROOT-CAUSE FIX: Exynos "INVALID_UPIU" address-window protection.
    // Linux programs HCI_INVALID_UPIU_BADDR (0x14/0x18) + UTR/DIN offset bounds
    // (0x20=0x8000, 0x24=0x10000) around ITS descriptor region, then the
    // controller BLOCKS any transfer whose UTR/data address falls outside that
    // window and raises vendor_IS bits 19,20 (invalid UTR/DIN offset). ferros's
    // carveout (0x924xxxxx) is outside Linux's window, so every doorbell is
    // rejected — THE dead doorbell. Disable the check so our physical addresses
    // are accepted. (Also nexus-type, already 0x7fffffff, harmless to re-set.)
    let upiu_ctrl_before = hci_r(0x10); // HCI_INVALID_UPIU_CTRL
    let upiu_baddr = hci_r(0x14); // HCI_INVALID_UPIU_BADDR (window base)
    hci_w(0x10, 0); // disable INVALID_UPIU window enforcement
    hci_w(0x38, 0x0018_0000); // clear vendor_IS invalid-offset error bits (W1C)
    let nexus_before = hci_r(0x40);
    hci_w(0x40, 0xFFFF_FFFF);
    unsafe { core::arch::asm!("dsb sy") };
    let nexus_after = hci_r(0x40);

    let link_up = ufs.link_is_up();
    let hcs = rd(0x30);

    let (nop_ocs, nop_rsp) = ufs.live_nop();
    let dbr_after_nop = rd(0x58);
    let is_after_nop = rd(0x20);

    let read_ocs = ufs.read_block(0);
    let dbr_after_read = rd(0x58);
    let is_after_read = rd(0x20);
    let rsr = rd(0x60);
    let status = ufs.last_response_status();
    let data = ufs.data_buffer();

    // Exynos vendor DMA-engine state (CORRECT reg_hci base now):
    let vs_fsm = hci_r(0xC0); // HCI_FSM_MONITOR — where the transfer FSM is parked
    let vs_dma_state = hci_r(0xC8); // HCI_DMA0_MONITOR_STATE
    let vs_dma_dbell = hci_r(0xD8); // HCI_DMA0_DOORBELL_DEBUG
    let vs_vendor_is = hci_r(0x38); // HCI_VENDOR_SPECIFIC_IS — bits 19,20 = invalid offset
    let vs_axi_ctrl = hci_r(0xF8); // HCI_UFS_AXI_DMA_IF_CTRL
    // The controller latches the address it rejected as "invalid offset" here.
    // If these equal ferros's carveout UTRD/data addr, the controller enforces a
    // valid-address WINDOW that the carveout (0x924xxxxx) falls outside of.
    let inv_utr = hci_r(0x20); // HCI_INVALID_UTR_OFFSET_ADDR
    let inv_din = hci_r(0x24); // HCI_INVALID_DIN_OFFSET_ADDR
    let inv_utmr = hci_r(0x1C); // HCI_INVALID_UTMR_OFFSET_ADDR
    let _ = (nexus_before, nexus_after);
    // S2MPU control readback — did our disable actually take?
    let s2mpu_ctrl = unsafe { core::ptr::read_volatile(S2MPU_HSI2 as *const u32) };
    // UFS Protector (UFSP @ 0x13208000) — the last unexamined DMA gate. If it
    // protects a region covering ferros's carveout (0x924xxxxx), the master's
    // descriptor DMA is denied. rsec/wsec = read/write secure masks; region 0
    // begin/end/ctrl. Compare against Linux (all-zero regions = not gating).
    const UFSP: usize = 0x1320_8000;
    let up = |off: usize| unsafe { core::ptr::read_volatile((UFSP + off) as *const u32) };
    let ufsp_rsec = up(0x10);
    let ufsp_wsec = up(0x110);
    let ufsp_sbegin0 = up(0x200);
    let ufsp_send0 = up(0x204);
    let ufsp_sctrl0 = up(0x20C);

    // Ramoops diagnostic (survives reset). u64 slots:
    //   +0x00 magic  +0x08 link_up  +0x10 ocs/status word
    //   +0x18 hcs|rsr  +0x20 is_nop|is_read  +0x28 dbr_nop|dbr_read
    //   +0x30 utrlba_rb|iocc_after  +0x38 vs_fsm|vs_dma_state
    //   +0x40 vs_dma_dbell|vs_vendor_is  +0x48 vs_axi_ctrl|s2mpu_ctrl
    //   +0x80..0xA0 first 32 bytes of LBA 0
    unsafe {
        let w = |off: usize, v: u64| write_volatile((RAMOOPS_RESULT + off) as *mut u64, v);
        w(0x00, RESULT_MAGIC);
        w(0x08, link_up as u64);
        w(0x10, (nop_ocs as u64) | ((nop_rsp as u64) << 8)
            | ((read_ocs as u64) << 16) | ((status as u64) << 24));
        w(0x18, (hcs as u64) | ((rsr as u64) << 32));
        w(0x20, (is_after_nop as u64) | ((is_after_read as u64) << 32));
        w(0x28, (dbr_after_nop as u64) | ((dbr_after_read as u64) << 32));
        w(0x30, (utrlba_rb as u64) | ((iocc_after as u64) << 32));
        w(0x38, (vs_fsm as u64) | ((vs_dma_state as u64) << 32));
        w(0x40, (vs_dma_dbell as u64) | ((vs_vendor_is as u64) << 32));
        w(0x48, (vs_axi_ctrl as u64) | ((s2mpu_ctrl as u64) << 32));
        w(0x50, (ie_before as u64) | ((is_after_rebase as u64) << 32));
        // invalid-offset bounds + vendor_IS (should now be clear if fix worked) +
        // the INVALID_UPIU window base/ctrl we disabled.
        w(0x58, (inv_utr as u64) | ((inv_din as u64) << 32));
        w(0x60, (vs_vendor_is as u64) | ((upiu_ctrl_before as u64) << 32));
        w(0x68, upiu_baddr as u64);
        let _ = (iocc_before, ufsp_wsec, ufsp_send0, ufsp_rsec, ufsp_sbegin0,
                 ufsp_sctrl0, nexus_before, nexus_after, inv_utmr, ferros_utrd);
        core::ptr::copy_nonoverlapping(data.as_ptr(), (RAMOOPS_RESULT + 128) as *mut u8, 32);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 192);
    }

    // PSCI signal: SYSTEM_OFF (power off, stays off) if the doorbell WORKED (NOP
    // completed: nop_ocs==0), else SYSTEM_RESET (reboot loop -> fastboot).
    let _ = (x0, el, SCRATCH, MAGIC, STAGE_M1);
    let success = link_up && nop_ocs == 0;
    let psci_fn: u32 = if success { 0x8400_0008 } else { 0x8400_0009 };
    unsafe {
        core::arch::asm!(
            "mov w0, {f:w}",
            "smc #0", // SYSTEM_OFF on success, SYSTEM_RESET on failure
            f = in(reg) psci_fn,
            options(nomem, nostack, noreturn),
        );
    }
}
