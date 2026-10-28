---
name: avb-signed-unlocked
description: "since 2026-10-28 husky boots a gate signed with the owner's RSA-4096 key (keys/avb.vsf), ABL avb_ret=OK on slot b while unlocked; husky-cold.sh signs every flash; slot a's patched images fail"
metadata:
  node_type: memory
  type: project
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-28T23:18:45.342Z
---

Since 2026-10-28: `avb_custom_key` holds the owner's key (sha256 f066329b…), both slots carry the signed gate and an owner-signed top-level vbmeta, and ABL logs `Last AVB: avb_ret=OK`, verity enforcing, no error parts, on slot b. `tools/husky-cold.sh` signs automatically when `keys/avb.vsf` exists (FERROS_UNSIGNED=1 skips it). Tooling: `ferros-mkimg avb-keygen|avb-pubkey|vbmeta-info|avb-sign` (tools/mkimg/src/avb.rs, Rust, no avbtool); the key is VSF `kgr`+`kvr`, types added to vsf for this.

**Why:** step 1 of the owner's locked-bootloader path (BOOT.md "Signed and unlocked"); lock only after the UFS firewall ([[ufs-firewall-decided]]), EL2-when-locked proven, dev USB doors closed, on a second Pixel 8 Pro first.

**How to apply:**
- Google's top vbmeta *chains* boot/init_boot/vbmeta_system/vbmeta_vendor (it doesn't hash boot); avb-sign footers the gate and re-points the boot chain; the other 22 descriptors stay Google's.
- Read ABL's verdict from its RAM log: `bridge read FD800000 10000 <file>`, grep "Last AVB" (it describes the PREVIOUS boot).
- Slot a's init_boot (root), dtbo, vendor_boot, vendor_kernel_boot are patched with stale footers, so slot a fails verification; they are the companions of the Android kernel packed in the gate (slot a's own boot_a, not factory). Android runs ONLY from slot a: from slot b (factory companions, husky-mirror-slot-b.sh) it hangs on missing vh_mm.ko. Keep slot a active. Making both lockable = sign those four (or rebuild on stock), owner's call, open. `diag part avb <partition>` reads a partition's own AVB metadata over USB; Android's console ring survives a reset at G#FD3F_F000 (2 MiB, DBGC header, start, size).
- The base is the factory zip for CP1A.260505.005; a new Android build needs a new base (or slot a fails).
