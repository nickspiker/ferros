---
name: pkvm-resets-on-wipe
description: "fastboot -w (and likely recovery wipes) puts pKVM back to auto/enabled, which breaks Android under the ferros gate (animation, then a hardware reset, then the rescue screen); check `fastboot oem pkvm status` before blaming anything else"
metadata:
  type: feedback
---

2026-10-30: Android would not boot through the gate after the 2026-10-29 factory reset. Five attempts failed: the boot animation, then a pin reset, or "Cannot load Android system". Stock boot (no hv) worked all along. The cause was pKVM, back to enabled after the wipe. With pKVM on, Android's kernel tries to own EL2, which the hv holds.

Reading `fastboot oem pkvm status`:
- `enabled (fastboot=auto misc=auto)`: armed. This is what a wipe leaves.
- `disabled (fastboot=kvm misc=auto)`: off. A ferros boot keeps it off (tested). The `fastboot -w` right after it set it back to auto.

After `fastboot oem pkvm disable` and a fresh wipe, Android booted through the gate in 72 s, to the setup wizard.

**Why:** the hv runs Android at EL1 under it. pKVM is a hypervisor of its own and dies trying to take EL2. The 2026-10-19 handoff hit the same wall ([[husky_linux_handoff_oracle]]).

**How to apply:**
- After any wipe (`fastboot -w`, recovery's format, a factory reset), run `fastboot oem pkvm disable` and confirm with `status` before booting Android through the gate.
- When gate-Android fails while stock boots, check pKVM first.
- Don't trust a disable read back in the same session as a wipe.

**Recovery wipes (2026-11-22, for the recovery question):** the record says a recovery wipe does NOT re-arm pKVM: husky's 2026-11-02 Settings factory reset (let thru by `diag recovery`) ran its wipe and pKVM stayed disabled. `fastboot=` is the bootloader's own setting, reset by its fastboot erase handler, out of Linux's reach; misc's vendor space (G#800, where `theme-dark` sits) has no pKVM flag on either device (`misc=auto`). Unchecked on the 8a after its 2026-11-22 factory reset: run `fastboot oem pkvm status` next time it is in fastboot. ferros logs "pKVM signs from the bootloader" (KVM words in /chosen/bootargs and bootconfig) at every boot, to find whether the two states differ there.
