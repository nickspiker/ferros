---
name: pro-unlocks-at-boot
description: "since 2026-11-17 the Pro (husky) and the 8a (akita) both unlock their identity at boot (the owner's opt-in, for each device's person), so reloads, reboots and flashes need no handle typed; the owner approved unattended flashes on the Pro (no slot switch)"
metadata:
  type: project
---

Owner, 2026-11-17 night: "attest on boot is now checked so let's test it?", then yes to rebooting the Pro to test it and "yuppers" to flashing it overnight (fastboot from the bridge, boot partition only, never a slot switch on husky).

**Why:** overnight iteration without him at the device; every reload used to drop the identity until he typed his handle.

**How to apply:** on the Pro, reloads, reboots and flashes may run unattended; the identity comes back by itself (check "identity unlocked at boot" on the console after each). It stores the identity on the device (identity_not_on_disk_default: the opt-in, "if this device is lost, whoever holds it becomes you"), his choice; never turn it off or on for him. The 8a also unlocks at boot since 2026-11-17 night (the owner enabled it on his wife's identity); her handle is never written in any repo or memory. Related: [[identity_not_on_disk_default]], [[feedback_pro_reload_freely]], [[stored_capsules_need_flashed_kernel]].
