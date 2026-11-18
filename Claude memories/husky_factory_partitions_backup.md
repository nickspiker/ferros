---
name: husky-factory-partitions-backup
description: "husky's per-unit factory partitions (persist = calibration, efs/efs_backup = modem data incl. IMEIs): the 2026-10-19 images were lost with Harbor tmp on 2026-11-17; re-dump into keys/husky/, local only, never commit"
metadata:
  type: project
---

`persist` (sda1, 128 MiB), `efs` (sda5, 64 MiB) and `efs_backup` (sda6, 64 MiB) from husky were imaged with `dd` over adb root on 2026-10-19 into `/mnt/Harbor/tmp/husky-backup-2026-10-19/`, with device-side and host-side sha256 lists that matched.
**Those images, and every other husky and akita partition backup under /mnt/Harbor/tmp, were deleted on 2026-11-17 when the owner cleared tmp (and the snapshots) to free a full disk.** The originals are still on the devices; only the safety net is gone. Only keys/husky-devinfo-2026-10-14.bin survived.

**Why:** written once at the factory (2023-08) and unrecoverable if anything formats them. `persist` holds per-unit calibration for camera (OTP/PDAF, AF, AWB, laser AF), display, haptics, sensors, audio and battery; `efs` holds the modem's per-unit data including the IMEIs.

**How to apply:** re-dump persist, efs, efs_backup, modem_userdata, devinfo, pinfo, misc and the GPT of each device into `/mnt/Harbor/Code/keys/<device>/` with sha256 lists, never into Harbor tmp ([[feedback_harbor_tmp_is_scratch]]). Restore with `fastboot flash persist|efs|efs_backup <img>` only if those partitions are damaged. Owner's rule: dumping identity data locally is fine, even into context; committing it publicly is not. Calibration contents also feed the ira breadth catalog (IRA-ENTROPY-SOURCES.md). Related: [[ira_entropy_sources_husky]], [[secrets_canonical_keys_dir]], [[akita_pixel8a]].
