---
name: bluetooth-pairing-moves-with-identity
description: Bluetooth bonds move with the identity, one radio active at a time; clone the identity's BT address + host-held keys across devices (dual-boot trick generalised); handoff lifecycle still to design
metadata:
  type: project
---

Owner direction (2026-10-24): Bluetooth pairing must move with the user's identity, with **one radio connected at a time** (touch the MacBook, or lock to the Pixel, and the earbuds follow). The handoff lifecycle is deliberately undecided ("we'll work out the lifecycle once we distill").

**Why:** legacy earbuds bond to a phone's *address*, not a person, and the bonding keys are host-held, exportable data (not TEE-locked, unlike the fingerprint reader — see [[biometrics_move_with_identity]]).

**How to apply:** the identity carries its Bluetooth address (set on Broadcom radios via vendor HCI Write_BD_ADDR), the link keys / LE LTK+IRK, and the peer list, sealed under the identity in the vault (data, not a third secret class — see [[ira_network_owns_assignment]]). Prior art: dual-boot key sharing (bt-dualboot) proves same-address+same-key works with any earbuds; Apple MagicPairing (iCloud, Apple-only) and Google Fast Pair (public spec, account key written to the earbud, certified earbuds only) are the vendor-cloud versions. Fast Pair support is an option; whether it needs Google's servers is unchecked. Husky's BT path (likely BCM4398 over UART) not yet looked at.
