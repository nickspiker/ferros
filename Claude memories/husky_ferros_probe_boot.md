---
name: husky_ferros_probe_boot
description: "How to run a ferros shim/probe on husky: tools/husky-hotload.sh (runtime trigger, no flash, ~1 min) since 2026-10-13; the older inject-/ferros.bin-into-vendor_kernel_boot + flash path; report decoding; disarm/backup facts"
metadata: 
  node_type: memory
  type: reference
  originSessionId: 35f77793-aafd-4fba-bd97-4cd4e60013b9
---

**USE THIS FIRST (2026-10-13): `tools/husky-hotload.sh` — hot-load with no flashing, ~52 s round trip including the cargo build.** The kernel (husky-kernel aosp, flashed to slot a) has `/sys/kernel/ferros/stage` (root writes an image path → copied into the 0x92400000 carveout) and a top-priority reboot notifier in ufshcd.c: `adb reboot ferros` → Android shuts down cleanly (sync+unmount) → notifier resumes the host, ungates clocks, turns auto-hibern8 off, pins clock scaling at the top gear, drains doorbells → `ferros_handoff_now()`. Nothing is armed at boot (the flashed vendor_kernel_boot has no /ferros.bin), so every reset lands back in Android with adb and `adb reboot bootloader` is always one command away — no buttons needed unless ferros wedges with the watchdog dead. First run: link idle at gear 4, AHIT=0, vault opened+verified, root hash `ca8171f6 18d243f3 ecccc2a18 3cea25f6` identical to the boot-time-trigger run the same day (ira-rooted root stable across boots and trigger paths). The "~10 min cycle" below was the watchdog-recovery era; a healthy boot-time cycle measured 65 s. Kernel-side changes (ferros_handoff.c, ferros_reset.S, the ufshcd.c hooks) are UNCOMMITTED in the aosp tree (detached HEAD) — not in the ferros repo. Pre-flash backups of boot_a/vendor_kernel_boot_a: `/mnt/Harbor/tmp/husky-backup-2026-10-13/`.

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
3. Copy the bin to `./ferros.bin` in the ramdisk root. **DO NOT strip ANY modules.** The canonical `husky-kernel/repack-ferros-vkb.sh` strips nothing; stripping the power/PMIC/regulator modules BREAKS Android boot (sticks cool at the Google logo) even with ferros absent, which poisons every result. Proven 2026-08-21 — this was the whole-session red herring.
4. Repack: `find . | cpio -o -H newc | lz4 -l -9 > vendor_ramdisk00.new`, then `mkbootimg.py $(cat mkargs) --vendor_boot out.img`. Simplest: just run `bash /mnt/Harbor/husky-kernel/repack-ferros-vkb.sh <ferros.bin>` (repacks in place, no strip). The committed `ferros/tools/husky-probe-boot.sh` now also does no strip.
5. `adb reboot bootloader` -> `fastboot oem pkvm disable` (ONCE) -> `fastboot flash vendor_kernel_boot out.img` -> `fastboot reboot`. Bootloader is UNLOCKED (flash.locked=0), no AVB re-sign.
6. Read: `adb shell su -c "dmesg" | grep -i ferros`. Recovery from a hang: force Vol-Down+Power to fastboot, flash STOCK `dist/vendor_kernel_boot.img` to disarm, reboot.

**THE GATE + READBACK (kernel patch `husky-kernel/aosp/arch/arm64/kernel/ferros_handoff.c`):** ferros fires from the ufshcd hook, writes a result to ramoops `0xFD60_0000`, and PSCI SYSTEM_RESETs (warm). Next boot `ferros_ramoops_read` (postcore initcall) checks `r[0]==0x315366556F526546` ("FeRoUfS1"): if present it PRINTS then zeroes `r[0]` (auto-consume -> next boot re-arms) and sets `ferros_reported` so the handoff SKIPS this boot -> Android. So it's a clean fire->read->refire loop; reboot to re-run, no manual clear.
**The print format is HARDCODED — lay ferros's result to match to see anything:** `r[0]`(0x00)=magic; `r[1]`(0x08)=`link_up` u64 (use for a full-64-bit value e.g. wairua sample — differs each boot if TRNG works); `r[2]`(0x10) bytes=nop_ocs/nop_rsp/read_ocs/scsi_status (use for flags); `r[3]`(0x18)&0xffff=`mbr_sig` (use for stage — this is where hello-world's 0xA0 showed as `mbr_sig=0xa0`); `r[4]`(0x20)=5 bytes; `r[5]`(0x28)=wrote_ok/restored_ok; `r[20..24]`(0xA0)=16-byte `pattern-readback` hex dump (BEST slot for raw trim bytes, e.g. ap_hw_tune[0..16]).

See [[ufs_linux_handoff]], [[ira_entropy_sources_husky]]. Committed as `ferros/tools/husky-probe-boot.sh` (now strip-free).

**2026-09-12 update — the loop is now self-reporting, no button needed:**
- **Cycle time is ~10 min** (fire → run → reset → Android). `adb wait-for-device shell ...` often drops with exit 255 or "no devices" mid-cycle even though the phone comes back; just re-poll `adb devices` + `getprop sys.boot_completed` and read again. Do not assume a hang before ~10 min.
- **vendor_boot_a was repacked once with `log_buf_len=16M`** (stock had 1024K, which wrapped past the 0.06 s report before adb could read it). The cmdline lives in vendor_boot, NOT vendor_kernel_boot (that one's cmdline is ignored). Stock copy: `/mnt/Harbor/tmp/vendor_boot/vendor_boot_a.stock.img` (64 MiB, has AVB footer; the footerless repack boots fine unlocked). Restore with `fastboot flash vendor_boot <stock>` if ever needed.
- **pstore console-ramoops does NOT survive a trip through fastboot** — the report boot's console log was gone after `adb reboot bootloader` + flash. Read dmesg in the report boot itself.
- **Hang localization is built into the shim now** (ferros_kernel/src/shim.rs + main.rs): sync-exception sink (first fault's ELR/FAR/ESR → ramoops + PSCI warm reset), panic reporter (`mbr_sig=0xEExx` = panic with last crumb xx; `link_up` = first number in the panic message; r[2] bytes = heap free bytes + block count; r[4] = line+col; hexdump = file tail + message head), and the cluster watchdogs armed at entry (pure spin → `bootreason=watchdog,apc` in ~9 min). Boot reason `reboot,pin` = ferros's own PSCI reset (normal completion OR panic/fault report).
- **Root cause of the 2026-08-21 vault hang:** ferros_heap rounded sizes to 8 but its header is 16, so after 4216 live bytes the free region started 8 mod 16 and a 512-byte 16-aligned request was refused → null → `handle_alloc_error` → silent `wfe` panic loop. Fixed (sizes rounded to header size; regression test in ferros_heap/tests/open_sequence.rs). ira-rooted vault genesis + persistence across reboot PROVEN on hardware the same day (root hash stable across boots, opened=1 verified=1).
- **Disarm** = flash stock `dist/vendor_kernel_boot.img`; while armed, EVERY reboot fires ferros and costs the ~10 min cycle.
