---
name: husky-factory-partitions-backup
description: "husky's per-unit factory partitions (persist = calibration, efs/efs_backup = modem data incl. IMEIs): the 2026-10-19 images were lost with Harbor tmp on 2026-11-17; re-dumped 2026-11-18 into keys/husky/partitions-2026-11-18/ (akita: keys/akita/partitions-2026-11-18/), local only, never commit"
metadata:
  type: project
---

`persist` (sda1, 128 MiB), `efs` (sda5, 64 MiB) and `efs_backup` (sda6, 64 MiB) from husky were imaged with `dd` over adb root on 2026-10-19 into `/mnt/Harbor/tmp/husky-backup-2026-10-19/`, with device-side and host-side sha256 lists that matched.
**Those images, and every other husky and akita partition backup under /mnt/Harbor/tmp, were deleted on 2026-11-17 when the owner cleared tmp (and the snapshots) to free a full disk.** Only keys/husky-devinfo-2026-10-14.bin survived. **Re-dumped 2026-11-18 by ferros itself** (`ferros-bridge part dump|gpt`: the kernel's read-only development `diag lun`, no Android, no root, no flash; each image read twice and alike, sizes equal to the GPT extents): persist, efs, efs_backup, modem_userdata, devinfo, pinfo, blenv, misc, fips, trusty_persist, mfg_data (all zeros) and the GPTs of sda-sdd (primary + backup), into `/mnt/Harbor/Code/keys/husky/partitions-2026-11-18/` with SHA256SUMS and B3SUMS; the 8a's alike into `keys/akita/partitions-2026-11-18/`.

**Why:** written once at the factory (2023-08) and unrecoverable if anything formats them. `persist` holds per-unit calibration for camera (OTP/PDAF, AF, AWB, laser AF), display, haptics, sensors, audio and battery; `efs` holds the modem's per-unit data including the IMEIs.

**How to apply:** the backups are `/mnt/Harbor/Code/keys/<device>/partitions-2026-11-18/`; a new dump (`FERROS_PORT=<port> ferros-bridge part dump <name> <out>`, `part gpt <lu> <primary> <backup>`) goes into a dated folder beside it with sha256 and blake3 lists, never into Harbor tmp ([[feedback_harbor_tmp_is_scratch]]). Restore with `fastboot flash persist|efs|efs_backup <img>` only if those partitions are damaged. Owner's rule: dumping identity data locally is fine, even into context; committing it publicly is not. Calibration contents also feed the ira breadth catalog (IRA-ENTROPY-SOURCES.md). Related: [[ira_entropy_sources_husky]], [[secrets_canonical_keys_dir]], [[akita_pixel8a]].
