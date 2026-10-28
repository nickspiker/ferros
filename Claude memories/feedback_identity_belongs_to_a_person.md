---
name: feedback_identity_belongs_to_a_person
description: Never put an identity (identity seed, handle proof, verifier, any TOKEN/Photon roots) on a device without asking whose device it is and which identity it should carry; a dev device is not the owner's identity carrier by default
metadata:
  type: feedback
---

On 2026-10-26 a session pushed the owner's Photon identity roots (from the desktop's own Photon session, via photon-interop `roots`) onto the husky so the ferros Photon client could ask to join the fleet; the husky is the owner's wife's phone. The attest screen built the next day then locked her phone to his handle. The owner: "So you copied my identity to my wife's phone why?" (2026-10-27).

**Why:** an identity is a person (TOKEN: the handle is who you are). Putting one person's roots on another person's device is impersonation infrastructure, whatever the dev convenience; "dedicated dev device" ([[pixel8-dedicated-dev-device]]) licenses flashing and wiping, not identity.

**How to apply:** before any step that places identity material on a device (store put of identity_seed/handle_proof, a roots push, a minted identity, a fleet bind), ask the owner whose device it is and whose identity it should hold, every time. Prefer the device minting its own identity through its attest screen (ahau Fresh path) over pushing roots. Never derive roots from the desktop's session for another device. Removing misplaced identity material is also the owner's call (it deletes vault entries) — propose, then wait.
