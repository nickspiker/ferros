//! The cold boot's UFS: the device powered back on and the link brought up for the vault.
//!
//! On a cold boot ABL leaves the UFS device POWERED OFF (see `power_on_device`), which is why every register-identical bring-up attempt met a silent device (UFS.md). With VCC restored, full_init's link startup succeeds. The on-demand link-startup probe (`bridge diag ufs`: the pristine register file, PMA/PA snapshots, the full_init report and MMIO trace) left on 2026-11-01 with the wall long solved (KERNEL-OUT.md order 10); git history keeps it.

use core::ptr::{read_volatile, write_volatile};

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

