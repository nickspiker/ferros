---
name: android-boot-experiments-rescue-party
description: "failed Android boots on husky (fastboot boot of a stock image, hv oracle builds on slot a) trip Android's rescue screen and can escalate to settings resets on the owner's wife's phone — ask before any experiment that boots Android under a changed image"
metadata:
  node_type: memory
  type: feedback
  originSessionId: a305f35f-299d-4e22-bd4c-7f99024cbeea
  modified: 2026-10-29T21:35:09.371Z
---

2026-10-29: a `fastboot boot` of the factory boot.img (to read cbd's handover block from DRAM) did not stay up, and then an hv oracle capture build on slot a (`husky-hv.sh --no-ferros --flash`, IPC page trapped, the FMT ring logger added) failed to boot Android repeatedly. The phone landed on "Cannot load Android system" (rescue), the owner chose "Try again", ABL fell back to slot b's gate, and the console zone held only uninitialised DRAM, so the cause was never seen. Slot a was restored with `husky-cold.sh`. Retried the same evening on the owner's go (one slot only): the capture build again landed on "Cannot load Android system" at its first boot, so it is the build itself (not a stale recovery request in misc); restored again with `husky-cold.sh` from fastboot (`adb reboot bootloader` works from the rescue screen). Suspects, unproven: the oracle path (FERROS_HV_STAGE2 / ORACLE trap of the IPC page) has rotted since 2026-10-19, the unsigned boot_a against the signed vbmeta, or the uncommitted FMT ring logger in ferros_hv. The gate's own volume-down Android path booted fine on 2026-10-28.

**Why:** husky is the owner's wife's phone ([[pixel8_dedicated_dev_device]]); Android's rescue escalation can reset settings by itself, and the last rung offers a factory reset. Captures under the hv worked on 2026-10-19, before the owner-signed AVB chain ([[avb_signed_unlocked]]); a hand-built slot-a boot image no longer matches the signed vbmeta, a likely suspect, not proven.

**How to apply:** (owner 2026-10-29: "if you're only borking one slot, flashey thingey away") booting Android under a changed image is fine when only one slot is touched and the other keeps the known-good gate; ask first if both slots would change. Prefer ferros-side measurement. If an oracle capture is needed, first find out why the hv build no longer boots Android (sign the capture image, or check the gate's own Android path), and stop after the first failed boot instead of letting retries pile up. Never choose or suggest "Factory data reset".
