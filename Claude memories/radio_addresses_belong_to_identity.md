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

**Cellular identity is MVNO-gated (2026-10-24).** The MVNO is **data-only** ([[mvno_data_only]]): no phone numbers, so the subscriber identity is just the IMSI/eSIM profile. It moves with the user only as far as the carrier allows (physical SIM by hand; carrier eSIM transfer is their button; eSIM profiles are locked to one eUICC) — until the owner's own MVNO provisions it itself (own HSS + SM-DP+). Owner calling Verizon, AT&T, Dish, T-Mobile and others (late Oct 2026). Budget: owner guessed $10M for the "peoples democratic utility" launch; industry figures put a lite MVNO at ~$100k-500k and a full MVNO core at ~$1-5M; the rest is host-carrier wholesale minimum commitments (the big unknown), staff and support. Compliance for data-only: no E911, no number porting, no voice CPNI, no USF (broadband not assessed); CALEA likely applies to a full MVNO running its own packet core — just comply; not worth a lawyer (owner: "they can look at all the traffic all they want"; Photon is end-to-end encrypted, so an intercept yields ciphertext). Interim: ferros cellular is blocked on the modem (boot channel stalls), so data = WiFi.

**eSIM provisioning facts (checked 2026-10-24).** The husky now carries a provisioned Verizon eSIM. Adding the MVNO's own profile is additive: the eUICC holds several profiles and a Pixel 8 Pro can run two eSIMs at once (carrier permitting); check the phone isn't Verizon carrier-locked first. The hard part is upstream: production eUICCs only accept an SM-DP+ whose DPtls/DPauth/DPpb certificates are signed by the GSMA CI, which requires GSMA SAS-SM accreditation (audited site). Osmocom's open-source osmo-smdpp works only with test eUICCs. Launch route: rent an accredited SM-DP+, accredit our own later if wanted. ferros needs a working modem plus an SGP.22 LPA before it can use any eSIM; until then Android's LPA can install test profiles.

**WiFi today (owner, 2026-10-31):** until an identity wears the address, the WiFi MAC is per network: BLAKE3 keyed by the ira-derived key `wifi mac per network v1` over the SSID (payloads/pcie1, WIFI.md). Never the chip's own `00:90:4c:50:10:01`, the calibration file's Broadcom placeholder, which is identical on every chip using that file. The router throttles quick rejoins from one address; the keeper's 64 s retry gets past it.
