---
name: hv-chords-decided
description: "Under Android (owner, 2026-11-01): power+vol up = to ferros (steal Android's prevent-ringing shortcut), power+vol down stays Android's screenshot, vol up+down = kill; interrupt-driven via GIC group 0 FIQ to EL2, never polled"
metadata:
  node_type: memory
  type: project
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-11-01T22:38:24.871Z
---

Chords while Android runs under the hv:
- **Power + volume up → ferros.** Android's "prevent ringing" shortcut is taken (owner: "That shit can be done with a tile. Steal that shit."); the hv swallows the chord so Android never sees it.
- **Power + volume down** stays Android's screenshot (owner uses it; "you need a button" — the screen must be in the state you want).
- **Volume up + volume down** = kill chord (SYSTEM_OFF), unchanged.

**Interrupt-driven, not polled.** Owner rejected the hv's "two consecutive trap-time checks" (`chord::held`). Design (HV.md "The chords under Android"): the keys' SPIs to GIC group 0 → FIQ → EL2 (HCR_EL2.FMO); Linux keeps group 1 and gets the keys through proxy SPIs the hv's DTB patch points at; the distributor page trapped so Linux can't regroup them. First probe: does EL3 let group-0 FIQs reach EL2 (SCR_EL3.FIQ)? The kill chord keeps the trap-time check until that lands.

**Why:** owner wants a button way back from Android that keeps Android's RAM ("steal whatever Android had going on") and works when Linux is wedged. Related: [[android-behind-attestation]], [[ufs-firewall-decided]], [[feedback-killswitch-irq-mask]].
