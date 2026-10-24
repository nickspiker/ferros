---
name: radio-addresses-belong-to-identity
description: ferros devices wear the identity's WiFi and Bluetooth addresses so observers see the identity, not hardware; platform support table; M1 BCM4378 needs a host-set BT address anyway
metadata:
  type: project
---

Owner direction (2026-10-24): ferros maintains the user's radio identity — WiFi MAC and Bluetooth address belong to the identity, not the device, so "anyone looking just sees their identity." Owner runs ferros full-time and is **not interested in other platforms** (noted for completeness only).

**Why:** pairings and network presence then follow the user across devices with no re-pairing (see [[bluetooth_pairing_moves_with_identity]]).

**How to apply:**
- Use locally-administered WiFi addresses (software-chosen bit set) so an identity address never collides with a vendor range. For BLE use a static random address (host-chosen, stable by design); classic BR/EDR needs a cloned public-format address. Option for later: derive a stable per-network address from the identity key.
- Cellular: the subscriber identity (SIM/eSIM) can move with the user; the IMEI stays with the device — changing it is illegal in many countries, off the list.
- Platform support (checked 2026-10-24): ferros and Linux (root, driver-dependent BT via `btmgmt public-addr`) can set both; Windows WiFi yes, BT only fragile third-party tools; macOS only Apple's own Private Wi-Fi Address, no BT; stock Android and iOS no.
- M1 MacBook Air: BCM4378 (same Broadcom family as husky's BCM4398), WiFi and BT both on PCIe; the BT controller *requires* the host to configure its address (Asahi `hci_bcm4377`, `local-bd-address`), so the identity address is native there. Needs Apple PCIe bring-up, macOS-extracted firmware, and m1n1/ADT board data first.
