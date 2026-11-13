---
name: identity-restores-the-device
description: owner 2026-11-08: attest on any ferros device (or after a factory reset) syncs the person's identity space from the fleet: kist roster, permission answers, home-screen layout (synced across all their ferros devices unless unchecked in settings); capsules fetched from fleet or fgtw.org, compiled/sealed, placed in the exact same spot and rotation
metadata:
  type: project
---

Owner's goal (2026-11-08), after a vault wipe on the Pro: "when I attest, the key matches, it syncs with my fleet, pulls a kist roster and then fetches the respective capsules from fgtw.org or my fleet ideally in the future and then builds/optimizes/signs and throws it on my home screen in the same exact spot and rotation it was before I factory reset this thing." Also: "on all my ferros devices the homescreen layout should sync, unless of course I go into settings and uncheck that."

**What lives in the person's identity space** (sealed with the fleet key, synced across their fleet):
- the kist roster: capsule hashes, gate identities, permission answers;
- the map arrangement: spot, size and rotation per circle, and the view;
- their settings: language, permission defaults, WiFi networks.

**Where capsules come from:** content-addressed by blake3. A fleet device is tried first (LAN or PT), then fgtw.org. The gate checks the signature, then the device compiles and seals. Device firmware (GPU, AoC, sensor scripts, WiFi blobs) is re-extracted from the device's own vendor partitions, never synced.

**Layout sync** is on by default across all of the person's ferros devices, with a settings switch to turn it off per device.

**Why:** an identity owns the device's state; a reset or a new device comes back to the same home screen. On 2026-11-08 the Pro's vault process lost everything because nothing was synced off it.

**How to apply:** any new per-person state goes in the identity space, so it syncs. Never keep the person's arrangement or roster only on one device.

Related: [[settings-identity-to-device-only]], [[permissions-asked-once]], [[ferros_vocabulary]], [[no-baked-apps]].

**Built 2026-11-08 (docs/IDENTITY-SPACE.md):** fgtw::space (facts, total-order merge, baseline reconcile, XChaCha seal under the fleet key, frames space_push/space_want, seed push_space/pull_space); fgtw-bootstrap `space_put`/`space_get` slot (committed, NOT deployed: needs the owner's deploy); ferros_space (kinds KIST/ANSWER/DEFAULT/PLACE/VIEW/HOME_SYNC over kists.list, map.place.*, map.view); payloads/fgtw space.rs (slots 9 map., 10 kists.; ops 17/18); map reloads on the generation, Fleet page switch. Capsule fetch and firmware refill are designed only (sources: own capsules., fleet device, fgtw.org if it ever serves capsules, bridge `kist install`).
