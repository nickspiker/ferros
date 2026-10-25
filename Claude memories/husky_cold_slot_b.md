---
name: husky_cold_slot_b
description: Slot b on husky boots ferros straight from ABL (no Linux) since 2026-10-14 — tools/husky-cold.sh, link base, exits to fastboot, delayed watchdog reset, cold display on ABL's splash, cold UFS + vault (44 ms + 84 ms)
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
- **USB enumeration is first-try** (13/13 cold boots, about 23 s from `fastboot reboot`) since the 2026-10-14 EP0 fix. The long-standing "coin flip" was SET_ADDRESS's status ZLP being started before XferNotReady(Status) (the race is in ferros_hal usb.rs `StatusInWait`), not the repeater or PHY. `bridge diag` FAIL_ATT blocks dump per-attempt DWC3 event rings if it ever regresses.
- **Display:** ABL leaves DECON on with both DPU SYSMMUs in translation-off, so the splash buffer G#FAC0_0000 (1344x2992 linear XRGB8888) is physical and `display::show` draws on it.
- **Exits:** every cold exit except a plain `bridge reboot` goes to fastboot. The report block survives fastboot (Android dmesg `ferros: r[..]`, next cold boot `PREV_*`).
- **Watchdog-triggered reset is DELAYED:** about 15-20 s after the cluster watchdogs start expiring (about 1 min for a single expiry). `reboot_to_bootloader` waits up to 64 s. Measured with `payloads/wdtfire`, a RUN payload that owns the CPU so nothing pets. The earlier "fastboot works" test only passed because the bridge waited 60 s.
- **EL3 poison extends beyond the PMU:** reading the watchdog block's undefined offsets (+0x14..+0x1C) also hung the next EL3 call. Read only defined registers.

**Disk works cold (2026-10-14):**
- ABL powers the flash off on its way out. `cold_ufs::bring_up` turns VCC (gpp0-1) back on and runs full_init with the per-command nexus. Link + first read take 44 ms; the vault opens and verifies in 84 ms.
- It is the same ira-rooted vault as the handoff path: the `note` written via the handoff reads back on the cold boot.
- **Slot b marks itself successful** (slot.rs, 2026-10-14): devinfo = UFS LUN 3 LBA 6, word per slot at G#30 (retries, flags b1 successful b2 active b3 fastboot-ok). ABL confirms `slot-successful:b:yes`; 5 plain reboots stayed in ferros. devinfo ALSO holds serials/MACs/IMEIs: never alter beyond the flag bit; full backup in keys/husky-devinfo-2026-10-14.bin. Details in BOOT.md.
- See [[ufs_husky_wall]].


**2026-10-23 corrections:** ABL boots the UFS boot LUN's slot (a on this phone) whatever `fastboot --set-active` says; `--set-active=b` marks a "de-activated" (devinfo bit 3) and ABL then sits in fastboot in 12 s every boot until `--set-active=a` (or a ferros boot) rewrites the word. Never switch slots from fastboot on husky; both slots carry the same gate image and slot b's boot-chain partitions are mirrored from the factory build. A PT stall no longer exits to fastboot (PT_STALLS in DIAG). The report block (RAMOOPS_RESULT) is rewritten by the hv gate every boot: post-mortem notes live in the X3 result page instead (`bridge read 819FF980 64` = the previous boot's; platform::note).

**2026-10-24: `bridge reboot fastboot` works again** (~40 s to fastboot). It had silently stopped resetting: kvault.rs pets the watchdogs on every vault op and the map's constant `map.place.*` writes on the worker cores kept petting during the 64 s wait (the X3 frozen, USB dead, ferros running on). platform.rs RESET_ORDERED now stops all petting once the reset is ordered. If the phone ever sits in ferros with no USB after a flash attempt, that is this path; the fallback is `diag android` → adb reboot bootloader.
