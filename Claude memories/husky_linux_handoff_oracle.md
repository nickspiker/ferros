---
name: husky-linux-handoff-oracle
description: "The Linux→ferros handoff on husky (tools/husky-hotload.sh, shim build) is a REGISTER ORACLE ONLY since 2026-10-14, never a boot path; how it works, its traps, and the hardware facts it measured"
metadata:
  type: reference
---

Merged 2026-11-12 from husky_shim_usb_service, husky_ferros_probe_boot and ufs_linux_handoff (2026-08..10 logs). ferros runs from the cold boot ([[husky_cold_slot_b]]) and the hv gate; this path only boots Linux, lets it set hardware up, hands off, and dumps/diffs what it left. Don't port features to it.

**Loop:** `tools/husky-hotload.sh` → push + `/sys/kernel/ferros/stage` (copies the image into the `ferros_handoff@92400000` 12 MiB carveout) + `adb reboot ferros` → Android shuts down cleanly, the ufshcd.c reboot notifier drains the UFS host, `ferros_handoff_now()` soft-restarts into ferros at EL2 with the UFS link live → ferros on USB as 1209:4665 in ~35 s. Build: `FERROS_SHIM_HANDOFF=1` (cfg `shim_handoff`, `aarch64-shim.ld`, `ferros_kernel/src/shim.rs`). Nothing is armed at boot, so every exit lands back in Android with adb. Kernel side (ferros_handoff.c, ferros_reset.S, ufshcd.c hooks) lives in /mnt/Harbor/husky-kernel/aosp, not this repo; build `./build_shusky.sh --config=use_source_tree_aosp`. Results go to ramoops (G#FD60_0000, magic "FeRoUfS1"), printed by the next Linux boot as `ferros: r[..]`.

**Traps:**
- **pKVM must be off** (`fastboot oem pkvm disable`; check `/proc/cmdline` for `kvm-arm.mode=protected`): protected KVM owns EL2 and swallows the handoff ([[pkvm_resets_on_wipe]]).
- The old arm-by-`/ferros.bin`-in-vendor_kernel_boot path: never strip modules from the ramdisk (stripping PMIC/regulator modules breaks Android boot even with ferros absent: a whole session's red herring). The cmdline lives in vendor_boot, not vendor_kernel_boot.
- Handoff stack is 256 KiB (`aarch64-shim.ld`; the service loop needs ~77 KiB); don't kick the USB port (`usb-kick.sh`) while a handoff ferros enumerates.
- PSCI SYSTEM_OFF reboots on this Pixel; DRAM scratch outside ramoops does not survive a reset; a crash → cold reset wipes ramoops.

**Hardware facts it measured (don't re-derive):**
- Linux leaves USB DMA translating thru the USB SYSMMU (G#1104_0000): clear bit 0 of every VM ctrl (G#8000 + vm·G#1000).
- DWC3: arm bulk OUT on XferNotReady only when not already armed (re-arming ENDTRANSFERs the live TRB → "no resource" storm).
- Exynos UFS vendor registers are the separate `reg_hci` block G#1320_1100, not std_base+off; the per-command nexus (HCI_UTRL_NEXUS_TYPE, reg_hci+G#40) must be set for SCSI and clear for NOP/query before every doorbell. That was the "dead doorbell" (2026-08-19).
- Fastboot from ferros: stamp PMU SYSIP_DAT0 (G#1546_0810) = G#FC via EL3 SMC G#8200_0504 (x1 addr, x2 1=write, x3 val), then fire the cluster watchdog; PSCI SYSTEM_RESET loses the mode.
- **EL3 poison rule:** a direct EL2 read of the PMU (G#1546_xxxx), of a powered-off DPU, or of undefined watchdog offsets "works" but hangs the NEXT EL3 PRIV_REG call. Read PMU state via EL3; touch only defined registers ([[dpu_power_secure_registers]]).
- Bootloader log of the current boot: G#FD80_0000, 1 MiB; DSS header G#D810_0000 (reason @+G#300).
- pKVM aside, the handoff proved the ira-rooted vault on the ferros partition (sda35) end to end before the cold boot could reach UFS.

Related: [[ufs_husky_wall]], [[feedback_reservation_proof_is_memblock]].
