---
name: hv-chords-decided
description: "Under Android (owner, 2026-11-01): power+vol up = to ferros, power+vol down = Android's screenshot, vol up+down = kill; interrupt-driven through an hv-owned core (A510 core 3), working on slot b"
metadata:
  type: project
---

Chords while Android runs under the hv:
- **Power + volume up → ferros.** This takes over Android's "prevent ringing" shortcut (owner: "That shit can be done with a tile. Steal that shit.").
- **Power + volume down** stays Android's screenshot.
- **Volume up + volume down** = kill chord (SYSTEM_OFF).

**How (built 2026-11-01, FERROS_HV_CORE, HV.md "The chords under Android"):**
- Group-0 FIQ was impossible, because IGROUPR is RAZ/WI from the non-secure side. So the hv owns A510 core 3 instead, and Linux's CPU_ON for it is refused. The owner accepted losing the core ("Totally fine by me!").
- The key SPIs (28/29/32) are routed to that core, which wakes on an interrupt and does nothing between key presses.
- Android gets its key presses through proxy SPIs 950-952, via a DTB patch.
- The ferros chord resets with FERROS_ONCE (G#FD60_0FE8) left in RAM, so the gate goes straight to ferros.
- **Verified on slot b:** the keys work in Android, and the chord lands in ferros (GATE_ANSWER 5).

**Open:**
- Superseded 2026-11-02: the chord no longer resets thru ABL; it freezes Android and jumps into a fresh ferros on the hv core ([[hv_jump_to_ferros]]). Both slots carry the same hv (husky-cold.sh). Deep sleep under the hv was fixed 2026-11-03 ([[sandbox_state_2026_11_03]], HV.md).
- The owner's later idea: patch Linux so it knows about the hv-owned core.

**Why:** the owner wants a button that gets back from Android, keeps Android's RAM ("steal whatever Android had going on"), and works even when Linux is wedged.

Related: [[android-behind-attestation]], [[ufs-firewall-decided]], [[feedback-killswitch-irq-mask]].
