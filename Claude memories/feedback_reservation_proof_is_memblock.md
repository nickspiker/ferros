---
name: feedback-reservation-proof-is-memblock
description: A reserved-memory node's presence in the device tree proves nothing; only memblock / /proc/iomem "reserved" proves Linux kept its hands off. The hv's 64 MiB sat in the page allocator for two days (2026-10-17 to 19) because reg had a two-cell size under a one-cell parent.
metadata:
  type: feedback
---

When ferros hands Linux a DTB with a carved-out region, verify the carve-out on the running kernel with `grep reserved /proc/iomem` (the range must read `: reserved`) or a split in `/sys/kernel/debug/memblock/memory` — never with `ls /sys/firmware/devicetree/base/reserved-memory/`.

**Why:** the hv's `hv@afc000000` node was visible in sysfs from the first boot on 2026-10-17, and was taken as proof. Its `reg` carried the size as two cells while husky's `/reserved-memory` has `#size-cells = 1`, so Linux read size 0 and reserved nothing. Two days of "random" freezes, an SError chase and a direct-I/O hang followed, all Linux and the hv writing over each other. The pstore line that gave it away was the hv's own per-core stats turned to random bytes.

**How to apply:** any new reserved node (hv, shared pages, ferros's own carve-outs) gets the iomem check in the boot script's verdict, next to the EL1 line. Match the parent's cell counts from the DTB, not from habit. See [[husky_ferros_probe_boot]] and HV.md "the reservation that never was".
