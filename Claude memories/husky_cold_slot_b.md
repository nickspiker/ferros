---
name: husky_cold_slot_b
description: Slot b on husky boots ferros straight from ABL (no Linux) since 2026-10-14 — tools/husky-cold.sh, link base, exits to fastboot, delayed watchdog reset, cold display on ABL's splash; disk still blocked
metadata:
  type: project
---

**Layout:** slot b = ferros cold boot (only `boot_b` replaced; other b partitions stock, `product_b` empty), slot a = Android + handoff kernel (dev host, escape hatch). Pre-flash slot-b backups: `/mnt/Harbor/tmp/husky-backup-2026-10-14/`. See [[husky_shim_usb_service]], [[husky_ferros_probe_boot]].

**Commands:**
- `tools/husky-cold.sh`: build, reach fastboot from anywhere (adb / `bridge reboot fastboot`), flash boot_b, set b active, boot, wait for 1209:4665, DIAG.
- `tools/husky-cold.sh --android`: back to slot a in about 40 s.
- Iterate with `bridge reload <objcopy'd ferros_cold.bin>`: copy stub, same link base, about 3 s.
- `bridge diag ufs` runs the old cold UFS link-startup probe on demand.

**Facts measured (don't re-derive):**
- **Link base:** ABL loads the kernel at G#8008_0000, so the cold image links there (`aarch64-pixel8.ld`). The old 0x80000 link only worked because nothing used absolute addresses.
- **USB enumeration:** cold takes 2-7 attempts (the handoff's is first-try).
- **Display:** ABL leaves DECON on with both DPU SYSMMUs in translation-off, so the splash buffer G#FAC0_0000 (1344x2992 linear XRGB8888) is physical and `display::show` draws on it.
- **Exits:** every cold exit except a plain `bridge reboot` goes to fastboot. The report block survives fastboot (Android dmesg `ferros: r[..]`, next cold boot `PREV_*`).
- **Watchdog-triggered reset is DELAYED:** about 15-20 s after the cluster watchdogs start expiring (about 1 min for a single expiry). `reboot_to_bootloader` waits up to 64 s. Measured with `payloads/wdtfire`, a RUN payload that owns the CPU so nothing pets. The earlier "fastboot works" test only passed because the bridge waited 60 s.
- **EL3 poison extends beyond the PMU:** reading the watchdog block's undefined offsets (+0x14..+0x1C) also hung the next EL3 call. Read only defined registers.

**Still blocked:** the cold UFS link (UFS.md, [[ufs_husky_wall]]), so there is no vault or devinfo slot marking on slot b. ABL's retry counter for b decrements per cold boot and is reset by `--set-active=b`.
