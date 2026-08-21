---
name: husky_ferros_probe_boot
description: "How to actually run a ferros shim/probe on husky (Pixel 8): inject /ferros.bin into the vendor_kernel_boot ramdisk, disable pKVM, flash, reboot, read dmesg. The run procedure, reconstructed from the 873bc7fe transcript."
metadata: 
  node_type: memory
  type: reference
  originSessionId: 35f77793-aafd-4fba-bd97-4cd4e60013b9
---

The shim/probe (entry_vault, the ira instrumentation) runs on husky via a CUSTOM Android kernel handoff, NOT a standalone boot. Reconstructed 2026-08-20 from transcript 873bc7fe (it was never scripted — assembled ad-hoc).

**Mechanism (baked into the custom `-dirty` android14 GKI kernel at `/mnt/Harbor/husky-kernel`):**
- Symbols in kallsyms: `ferros_handoff`, `ferros_do_jump`, `ferros_soft_restart`, `ferros_ramoops_read`. Fired from inside `ufshcd_async_scan` (`ferros_fired` one-shot) so ferros inherits the LIVE UFS link.
- Reserved mem `ferros_handoff@92400000` (12 MiB at phys G#9240_0000) = where the bin is staged/jumped. ramoops result region at phys G#FD60_0000.
- Flow: boot fires ferros -> ferros runs -> `ferros_soft_restart` (WARM reset, DRAM preserved) -> next boot reports ramoops + skips -> Android. One-shot.

**Arm = presence of `/ferros.bin` in the vendor_kernel_boot ramdisk.** Self-disarms if absent (initcall just logs "cannot read /ferros.bin: -2").

**pKVM MUST be off** or `kvm-arm.mode=protected` owns EL2 and swallows the handoff: `fastboot oem pkvm disable`. Check `/proc/cmdline` for `kvm-arm.mode=protected` (present = still armed pKVM = handoff blocked).

**Procedure:**
1. Build + flat-binary: `FERROS_SHIM_HANDOFF=1 cargo build -p ferros_kernel --target aarch64-unknown-none --release` then objcopy `-O binary` (bin ~197 KB).
2. Unpack `/mnt/Harbor/husky-kernel/out/shusky/dist/vendor_kernel_boot.img` with `tools/mkbootimg/unpack_bootimg.py --format=mkbootimg` (save mkargs); `lz4 -dc vendor_ramdisk00 | cpio -id`.
3. Copy the bin to `./ferros.bin` in the ramdisk root. ALSO strip power-domain/pkvm modules from `lib/modules/*/modules.load` (sed out: `exynos-pd*`, `ect_parser`, `gs_acpm`, `acpm_flexpmu_dbg`, `power_stats`, `pkvm-s2mpu-v9`, `pmic_class`, `s2mpg14/15-*`, `slg51002-*`, `s2mpg1415-gpio`) so the EL2 handoff isn't blocked.
4. Repack: `find . | cpio -o -H newc | lz4 -l -9 > vendor_ramdisk00.new`, then `mkbootimg.py $(cat mkargs) --vendor_boot out.img` (or `repack_bootimg.py`). lz4/cpio/mkbootimg live under `/mnt/Harbor/husky-kernel/prebuilts/kernel-build-tools/linux-x86/bin` and `tools/mkbootimg`.
5. `adb reboot bootloader` -> `fastboot oem pkvm disable` (once) -> `fastboot flash vendor_kernel_boot out.img` -> `fastboot reboot`.
6. Read: `adb shell su -c "dmesg" | grep -i ferros` -> the `ferros: scan magic=... stage=... landed_EL=2 ...` line; also `/sys/fs/pstore/console-ramoops-0`. Result offsets: see ferros/IRA-ENTROPY-SOURCES.md "ramoops offset map".

**GAPS to confirm before flashing (not fully captured in transcript):** the exact `/ferros.bin` inject line, and whether vendor_kernel_boot needs AVB re-sign or vbmeta is already disabled. Prior bins live at `/data/local/tmp/{fvault,fvault2,fv3}.bin`. Reboots are recoverable (Android boots normally if disarmed) but budget them. See [[ufs_linux_handoff]], [[ira_entropy_sources_husky]]. THIS SHOULD BECOME A COMMITTED SCRIPT (tools/pixel8) so it never lives only in context again.
