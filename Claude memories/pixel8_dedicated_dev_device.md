---
name: pixel8-dedicated-dev-device
description: Pixel 8 Pro (husky) is the owner's WIFE's phone used as the ferros dev device — free to unlock/wipe for ferros, but it is HER device: never put the owner's identity (or anyone's) on it without asking; graphene.md is dead scaffolding
metadata: 
  node_type: memory
  type: project
  originSessionId: 7c0559c4-1684-4522-9dd3-bb5f5f140193
---

The Pixel 8 Pro (husky) has no daily-driver or banking constraint — it is free to unlock/wipe/dedicate to ferros. The Chase-2FA / Google-Wallet angle was an abandoned tangent (reverse-engineering Google Wallet turned out to be illegal; the user carries physical cards and does not want Google controlling the device).

**Why:** removes the wipe/banking blocker that would otherwise make unlocking the Pixel bootloader for ferros a hard decision — there is nothing on it worth preserving.

**Whose device (owner, 2026-10-27):** the husky is the owner's wife's phone. "Dedicated dev device" means free to flash and wipe for ferros work, not that it is the owner's personal device. On 2026-10-26 a session pushed the OWNER's Photon identity roots onto it (taking the desktop's session) and the next day's attest screen locked it to his handle; the owner: "So you copied my identity to my wife's phone why?" See [[feedback_identity_belongs_to_a_person]].

**How to apply:** don't re-raise banking/2FA as a reason to preserve the Pixel; treat it as a dev device. `dumpster/graphene.md` (self-signed GrapheneOS to suppress voicemail/call-forward nags) is dead scaffolding from that tangent — ferros is bare-metal and *replaces* Android, so it needs ~none of GrapheneOS: only bootloader unlock + `fastboot oem pkvm disable` (EL2) + `fastboot boot` (RAM dev loop). Bring-up reality: no `ferros_hal_pixel8` crate yet (template off `ferros_hal_m1`), BOOT.md still describes the dropped FP5/QCM6490, and a prior attempt hit an entry-code crash. See [[pixel8-boot-findings]].
