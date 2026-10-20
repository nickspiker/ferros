---
name: husky-factory-partitions-backup
description: husky's per-unit factory partitions (persist = all factory calibration, efs/efs_backup = modem per-unit data incl. IMEIs) imaged and hash-verified 2026-10-19 to /mnt/Harbor/tmp/husky-backup-2026-10-19; local only, never commit
metadata:
  type: project
---

`persist` (sda1, 128 MiB), `efs` (sda5, 64 MiB) and `efs_backup` (sda6, 64 MiB) from husky, imaged with `dd` over adb root on 2026-10-19 into `/mnt/Harbor/tmp/husky-backup-2026-10-19/`, with device-side and host-side sha256 lists that match.

**Why:** written once at the factory (2023-08) and unrecoverable if anything formats them; they were missing from the 2026-10-13/14 slot backups. `persist` holds per-unit calibration for camera (OTP/PDAF, AF, AWB, laser AF), display, haptics, sensors, audio and battery; `efs` holds the modem's per-unit data including the IMEIs.

**How to apply:** restore with `fastboot flash persist|efs|efs_backup <img>` only if those partitions are damaged. Owner's rule: dumping identity data locally is fine, even into context; committing it publicly is not. Calibration contents also feed the ira breadth catalog (IRA-ENTROPY-SOURCES.md). Related: [[ira_entropy_sources_husky]], [[secrets_canonical_keys_dir]].
