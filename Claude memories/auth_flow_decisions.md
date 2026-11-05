---
name: auth-flow-decisions
description: "Owner decisions 2026-10-31 for the ferros auth flow — device vs identity vault classes, WiFi creds + BT pairings move with the identity, key bound to phone, lock only on power loss or OS switch, auth mostly in kernel; instant unlock from seed + phone-only secret; one identity per phone (built, AUTH.md)"
metadata:
  node_type: memory
  type: project
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-10-31T21:00:40.344Z
---

Owner decisions, 2026-10-31. Built the same day (AUTH.md, commits 9daf32c..c6f8bcc); the flowchart page tracks the state (https://claude.ai/artifact/7sxcUNi5owYvcB426AESLB).

**The two classes are agreed:**
- **Device class**, sealed by the ira. It holds everything that must work before a person is present:
  - drivers and firmware;
  - the attest front end (display, touch, ahau);
  - network and radios;
  - the hardware-owned settings: language, base, tz, keyboard layout, brightness, dither, prox wake, unlock_at_boot, and theme (my call, since the attest screen draws with it).
- **Identity class**, sealed under a key bound to this phone AND the person:
  - the map arrangement and view;
  - installed apps and their data;
  - Photon chain and key;
  - identity prefs;
  - WiFi credentials and device pairings (car, headphones).

**WiFi credentials and device pairings move with the identity.** The owner wants every device in the fleet to get the identity's networks and pairings instantly. MAC addresses stay the device's (per-network, from the ira).

**The key is bound to the phone:** KDF over (ira, identity input).

**Locking happens only when power goes, or on a switch to another OS.** The owner may add jumping to and from Android. There are no timers.

**Auth mostly in the kernel:**
- The kernel takes key custody, enforces the scope classes, and wipes the key on an OS switch.
- The UI and the heavy derivation stay in ahau (EL0).

**Decided: instant unlock from the seed** (owner, 2026-10-31). key = BLAKE3-keyed(derive_key(ira), phone-only secret ‖ seed). The phone-only secret is 32 bytes of wairua, kept in the device class; Google never sees it. A hypervisor net still to build keeps Android from reading the ferros partition; the owner wants nets around everything ferros uses that Android does not need. The memory-hard proof stays Photon's (registry, squatting).

**One identity per phone** (owner, 2026-10-31: "when did we start issuing multiple identities per device?"). The kernel binds the phone when its fleet binds it, and refuses any other handle. Release on a completed departure wipes the identity class. I had shipped per-handle spaces without asking; don't reintroduce them. Spec: ferros AUTH.md.

Related: [[identity-not-on-disk-default]], [[ferros-attest-os-level]], [[radio-addresses-belong-to-identity]], [[bluetooth-pairing-moves-with-identity]].

**NFC tag unlock (owner, 2026-11-05).**
- **What:** the owner keeps a plain NTAG-type tag (NFC-A Type 2, SAK 00, 7-byte UID, no crypto) in his glasses. Tapping it at attest unlocks without typing the handle, and nothing is remembered without the tag.
- **How:** the identity seed is kept in the vault, sealed under a KDF of the ira and the tag's number. Device plus tag unlocks.
- **The owner's threat view:** acceptable, since it needs his device and his glasses. The handle isn't secret anyway (a coworker knows it; a handle is a name, not a password).
- **Later:** "would be nice to have PIPE on tag".
- **Order:** build it after the GPU attest mark.
