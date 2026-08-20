---
name: pixel8-dedicated-dev-device
description: Pixel 8 Pro is a dedicated ferros dev device — no banking/daily-driver constraint; graphene.md build plan is dead scaffolding
metadata: 
  node_type: memory
  type: project
  originSessionId: 7c0559c4-1684-4522-9dd3-bb5f5f140193
---

The Pixel 8 Pro (husky) has no daily-driver or banking constraint — it is free to unlock/wipe/dedicate to ferros. The Chase-2FA / Google-Wallet angle was an abandoned tangent (reverse-engineering Google Wallet turned out to be illegal; the user carries physical cards and does not want Google controlling the device).

**Why:** removes the wipe/banking blocker that would otherwise make unlocking the Pixel bootloader for ferros a hard decision — there is nothing on it worth preserving.

**How to apply:** don't re-raise banking/2FA as a reason to preserve the Pixel; treat it as a dev device. `dumpster/graphene.md` (self-signed GrapheneOS to suppress voicemail/call-forward nags) is dead scaffolding from that tangent — ferros is bare-metal and *replaces* Android, so it needs ~none of GrapheneOS: only bootloader unlock + `fastboot oem pkvm disable` (EL2) + `fastboot boot` (RAM dev loop). Bring-up reality: no `ferros_hal_pixel8` crate yet (template off `ferros_hal_m1`), BOOT.md still describes the dropped FP5/QCM6490, and a prior attempt hit an entry-code crash. See [[pixel8-boot-findings]].
