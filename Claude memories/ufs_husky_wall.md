---
name: ufs-husky-wall
description: "SOLVED 2026-10-14: the husky cold UFS wall was ABL powering the flash OFF (gpp0-1 VCC low) right before the kernel jump; turn VCC back on, then full_init with per-command nexus. Lesson: check what the previous stage does on its way out"
metadata:
  type: project
---

**Solved 2026-10-14 by reverse-engineering ABL.** ABL's UFS shutdown (`0x2cdbc`) runs on the kernel-boot path: it hibernates, then vendor hook stage 10 sets `HCI_GPIO_OUT=0`, repurposes gph5-0, and drives **gpp0-1 low = UFS VCC off** (`zuma-ufs.dtsi ufs_fixed_vcc`). ferros drives gpp0-1 high (`cold_ufs::power_on_device`, 4 ms) before `full_init`, and sets the Exynos per-command nexus bit before each doorbell (SCSI set, NOP/query clear). Cold result: LS_RES 0, HS-G4, link + first read in 44 ms; the first READ gets UNIT ATTENTION, the second returns LBA0. Recipe and history in docs/UFS.md.

**The lesson (the reason this memory stays):** for two months ferros's `full_init` was byte-identical to Linux's MMIO trace (FWTRACE diff, mclk 178 MHz, FORCE_HCS, ERROR_EN, refclk drive) and the device stayed silent at DME_LINKSTARTUP. VCC was "ruled out on spec grounds" because ABL had just read the device; that reasoning was wrong. When the registers match and the device is silent, look at what the previous stage does on its way out (and measure it, [[feedback_measure_before_fixing]]).

Ruled out on the way, still true: secure-DMA/GSA (Linux's descriptors live in low non-secure DRAM with WSECURITY=0), S2MPU/UFSP/iocc coherency, hibernate-exit resume (Linux never resumes ABL's hibern8; re-link from scratch is the only path). Related: [[husky_cold_slot_b]], [[husky_linux_handoff_oracle]].
