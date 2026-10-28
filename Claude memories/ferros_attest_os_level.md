---
name: ferros_attest_os_level
description: "Owner 2026-10-27: ferros attest is at the OS level, not per-app, and how to wire it is open. The ira unlocks drivers and key apps on disk; once the user attests, the identity (Photon's keys from the handle) unlocks Photon messages, Lumis settings and the rest — a lock screen before anything works. Design before building; the display driver meets identity-keyed data here"
metadata:
  type: project
---

The attest/lock screen is a ferros OS concern, not something each app wires for itself (owner, 2026-10-27, while Photon was first running on the phone). How to wire it is open — flagged, not designed.

The scheme the owner laid out earlier (same day):
- Drivers and key-app capsules are encrypted on disk by the **ira** (the device key, kernel-derived, survives factory reset). The device brings itself up from them.
- Once the user **attests**, the **identity** (Photon's keys, derived from the handle) unlocks the identity-keyed data: Photon messages, Lumis settings, the rest. Nothing works before attesting — a lock screen.
- Two key classes meet here: the ira (device, boot) and the identity (user, post-attest). The display driver and the identity-keyed data cross at the attest screen, so the owner wants it designed before it is built ([[display_driver_design_first]], [[owner_directions_2026_10_27]]).

My read for when it is designed (not built yet, not agreed):
- The attest screen is a ferros process the shell starts before any identity-keyed app; it holds the display and touch, takes the handle (the input device the owner is still deciding, [[owner_directions_2026_10_27]]), derives the identity, and hands the unlocked roots to the apps that need them (Photon's vault scope, Lumis settings) over the capability IPC — the apps never see the handle, as the phone's Photon client already never does ([[photon_on_ferros_no_bootstrap]]).
- The kernel vault scope is the natural place for the identity-keyed data: a scope sealed under an identity-derived key the attest process installs, not the ira. The kernel's store seals under the ira today (kvault); an identity-sealed scope is the new piece.

**The capsule is `ahau`** (owner, 2026-10-27; Māori "I, me"): the integrated, userspace ferros capsule for attest, the You page, the Fleet page, and every TOKEN-identity setting. Photon keeps only the messenger; the Photon session is gating Fleet, You and identity settings out of Photon on the ferros target (gate, don't delete, logic kept reachable). Encryption boundary, refined: the identity encrypts every capsule on disk, drivers excepted (they stay under the ira because they must run before attest), and the attest front end (display, touch, input) sits in the ira set for the same reason. Icon: grey at rest, so it must read by shape; leading idea is the ferros logo's broken iron ring closed around a single centre point. Not yet built.

**Built 2026-10-27 (owner: "build it however you want, I can critique later"):** `payloads/ahau`, started by init before the map at the system window level, so it covers everything until attest. Derivation without std in `ahau_core`: vsf 0.6.42's x encoding over only the keyboard's characters (a 102-entry code table; host tests byte-exact against vsf and ihi::handle_to_hash), identity seed = BLAKE3 of it. Device keeps `ahau.verifier` = keyed_hash(ira key "ahau verifier", seed); first match against the paired `photon.identity_seed` writes it. No identity yet: typed, typed again, then ihi::handle_proof (ihi feature `proof`, split from `handle` so no vsf) pinned to the X3 (6.5 s; 22.6 s on an A715), roots written to `photon.`. Scratch = ahau's own 25 MiB .bss via the ferrosRT header. The handle shows as typed (masking removed the same day: "this is NOT a password", [[feedback_handle_not_password]]); the left key clears; minting a new identity asks for the handle twice, visibly (owner: "you only do that once"). Dev escape: `ferros-bridge diag "kill ahau"`. Open for critique: the mark reads like a power symbol; identity_seed still sits raw in the ira-sealed vault (no identity-sealed scope yet, so attest gates the screen, not the data); no rate limit on wrong handles; fail-open if ahau cannot start.

**How to apply:** treat attest as a kernel/shell feature with its own process, not an app feature; design the attest screen and the identity-sealed vault scope with the owner before building; it gates Photon and Lumis shipping, so it comes before them on the phone.
