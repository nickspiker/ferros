---
name: ufs-firewall-decided
description: "owner decided 2026-10-28 the hv gets a UFS command firewall (Android can't write the boot chain) and blocks Android's UFS device reset; after the signed vbmeta; a prerequisite for locking"
metadata:
  node_type: memory
  type: project
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-28T22:38:58.926Z
---

Owner, 2026-10-28: "as long as it gets built. I'd block reset too." The hv (EL2, Android at EL1) traps the UFS doorbell, shadows the command list, and refuses Android's writes/UNMAP to boot_*, vbmeta_*, init_boot_*, vendor_boot_*, vendor_kernel_boot_*, dtbo_*, pvmfw_*, devinfo, the ferros partition, LU1/LU2, WRITE BUFFER, descriptor writes and protection flags. It also blocks Android's UFS device reset, so the fPowerOnWPEn the gate arms before Android stays set all session. Full list in BOOT.md ("Can Android be kept from writing the boot chain?", Decided).

**Why:** the flash's own write protection can't cover LU0 on husky, and Android's driver resets clear it (measured 2026-10-17); locking without the firewall is the one thing BOOT.md says not to do.

**How to apply:** order is signed vbmeta (unlocked) → fix the hv direct-I/O hang/freeze (HV.md) → firewall → only then think about locking (on a second Pixel 8 Pro first). Related: [[keys-in-registers]], [[pixel8-dedicated-dev-device]].
