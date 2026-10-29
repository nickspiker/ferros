---
name: android-boot-experiments-rescue-party
description: "failed Android boots on husky (fastboot boot of a stock image, hv oracle builds on slot a) trip Android's rescue screen and can escalate to settings resets on the owner's wife's phone — ask before any experiment that boots Android under a changed image"
metadata:
  node_type: memory
  type: feedback
  originSessionId: a305f35f-299d-4e22-bd4c-7f99024cbeea
  modified: 2026-10-29T21:35:09.371Z
---

2026-10-29: a `fastboot boot` of the factory boot.img (to read cbd's handover block from DRAM) did not stay up, and then an hv oracle capture build on slot a (`husky-hv.sh --no-ferros --flash`, IPC page trapped, the FMT ring logger added) failed to boot Android repeatedly. The phone landed on "Cannot load Android system" (rescue), the owner chose "Try again", ABL fell back to slot b's gate, and the console zone held only uninitialised DRAM, so the cause was never seen. Slot a was restored with `husky-cold.sh`.

**Why:** husky is the owner's wife's phone ([[pixel8_dedicated_dev_device]]); Android's rescue escalation can reset settings by itself, and the last rung offers a factory reset. Captures under the hv worked on 2026-10-19, before the owner-signed AVB chain ([[avb_signed_unlocked]]); a hand-built slot-a boot image no longer matches the signed vbmeta, a likely suspect, not proven.

**How to apply:** ask the owner before any boot of Android under a modified or unsigned image. Prefer ferros-side measurement. If an oracle capture is needed, first find out why the hv build no longer boots Android (sign the capture image, or check the gate's own Android path), and stop after the first failed boot instead of letting retries pile up. Never choose or suggest "Factory data reset".
