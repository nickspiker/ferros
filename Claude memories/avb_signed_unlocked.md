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
- Slot a's init_boot (root), dtbo, vendor_boot, vendor_kernel_boot are patched with stale footers, so slot a fails verification; they are the companions of the Android kernel packed in the gate (slot a's own boot_a, not factory). Since 2026-10-28 slot b carries copies of them (backup in /mnt/Harbor/tmp/husky-backup-2026-10-28/) and Android runs from either slot; the gate rewrites ABL's `androidboot.slot_suffix:=_b` to _a (it used to match only `= "_b"`, and its checksum store was an unaligned word with the MMU off). tools/husky-mirror-slot-b.sh copies slot a -> b from Android with root (sha256-checked; --factory is the old behaviour). Slots compared 2026-10-28: all 22 partition pairs identical except dram_train (the bootloader's own); Android's dynamic partitions exist only for slot a (system_b is a 147 MB leftover). Since 2026-10-28 night all four are vouched for under the owner's key (top hashes as installed, init_boot chained + re-signed): avb_ret=OK on BOTH slots, Android boots from each. husky-cold.sh signs with --installed (AVB_INSTALLED = the 2026-10-28 backup); a new Magisk patch or Android update means refreshing those copies. `diag part avb <partition>` reads a partition's own AVB metadata over USB; Android's console ring survives a reset at G#FD3F_F000 (2 MiB, DBGC header, start, size).
- The base is the factory zip for CP1A.260505.005; a new Android build needs a new base (or slot a fails).
- A/B fallback proven 2026-10-28 (BOOT.md): a dead slot a (FERROS_TEST_HANG=1 gate) ran out of retries and ABL booted ferros from slot b, ~1.5-2 min per failed attempt (~12-15 min at 7 retries). Owner wants slot a the default with b a spare; ferros's slot marking re-arms the spare (clears its de-activated flag) every boot. Redundancy covers a bad slot, not a bad build flashed to both. `diag slot [word ...]`.
