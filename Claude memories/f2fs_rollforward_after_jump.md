---
name: f2fs-rollforward-after-jump
description: After a jump Android never checkpoints f2fs; ferros_f2fs rolls forward read-only (2026-11-03) or files read stale (modem replay_region.bin read 0 bytes, modem dead every boot after a jump)
metadata:
  type: project
---

Found 2026-11-03: the jump freezes Android without sync, so f2fs data fsync'd since the last checkpoint lives only in the warm node chain. ferros read the checkpoint: modem_userdata/replay_region.bin was 0 bytes and the modem's bring-up failed every boot. ferros_f2fs now walks the chain (recovery.c rule: nodes with the checkpoint's version [+CRC], each fsync'd inode's up to its last fsync mark, overlay on the NAT), writing nothing. `diag "part f2fs <part> <path>"` shows chain length and whether an inode came from it.

**Why:** any Android file ferros reads after a jump may be newer only in the chain.
**How to apply:** if an Android-side file looks truncated or old after a jump, check `diag part f2fs` first; ext4 partitions (vendor) have the analogous journal problem and no replay yet.

Related: [[hv-jump-to-ferros]], [[modem-uecap-registration]].
