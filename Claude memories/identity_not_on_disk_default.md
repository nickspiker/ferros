---
name: identity_not_on_disk_default
description: "Owner rule 2026-10-28: no form of the handle/seed stored on a device by default — typed each boot, seed+proof handed to the Photon client in RAM (op 4), fleet key recovered from the published fan-out; opt-in device.unlock_at_boot stores the seed to unlock without typing, with a theft warning"
metadata:
  type: project
---

The identity seed IS the identity's root key (its signing key derives straight from it, and the handle proof is computed from it), and it is a fast guessing target: measured on the dev box (Ryzen 9 5950X, 2026-10-28) `x`-encode + BLAKE3 (ahau_core::identity_seed) runs 3.1 M guesses/s on one thread, 45 M across 32; raw BLAKE3 on a short input 16.9 M / 350 M. A memorable two-word handle (~400 M candidates) falls in ~9 s against a stolen seed. The handle proof is the only memory-hard step (~1 s/guess desktop, 4.7 s husky X3) — it does not protect a stored seed. The vault is sealed under the ira and ferros unseals it itself at boot, so a stored seed on a stolen phone simply IS the identity.

**Default (owner: "sand on the beach with no keys anywhere and it's all derived from the handle and the ira"):** nothing of the identity on disk. The handle is typed each boot; ahau derives seed+proof and hands them to the Photon client in RAM via the `photon` endpoint op 4 (`[4][seed 32][proof 32]`); the client looks up, holds the chain/key in RAM, and recovers the fleet key from the published fan-out (`recover_fleet_key`). ahau no longer keeps a verifier; a device is "its identity's" only once the client folds the public chain and finds this device a member.

**Opt-in:** `device.unlock_at_boot` (ferros_sys::PREF_UNLOCK_AT_BOOT = 13; a field in the settings VSF doc) stores seed+proof+chain+fleet_key so the phone unlocks without typing. ahau's entry has an "unlock at boot: off/on" toggle; on, it warns "if this phone is lost, whoever holds it becomes you." `Client::persist_set` writes only when on; `Client::forget_disk` wipes every identity value whenever off (at client start and on op 4).

**How to apply:** never store identity_seed/handle_proof/fleet_key on a device unless unlock_at_boot is on; default is RAM-only, re-derived from the typed handle each boot ([[feedback_identity_belongs_to_a_person]], [[feedback_handle_not_password]]). The handle text itself is never on disk or the wire in any mode.
