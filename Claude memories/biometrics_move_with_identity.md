---
name: biometrics-move-with-identity
description: ferros biometrics must move with the identity; husky's fingerprint reader is marked never (device-bound in Google's TEE) unless Google gives keys or the raw sensor proves reachable
metadata:
  type: project
---

Owner rule (2026-10-24): ferros supports biometrics only if the material moves with the identity, never device-bound.

**Why:** husky's Goodix under-display fingerprint reader appears to keep capture, templates and matching inside a Google Trusty app. Linux's device tree gives it only IRQ, reset and power, and no data bus. Templates sealed to Google's TEE can't follow an identity across devices.

**How to apply:** fingerprint is marked **never** unless Google provides keys, or the sensor's SPI controller proves reachable from the normal world (then ferros captures raw images and keeps its own templates in the vault). The secure-world ownership is inferred from the device tree, not measured. Don't propose the Trusty-app yes/no route: it is device-bound by construction. Details in ferros SENSORS.md "Fingerprint: never". Related: [[aoc_start_needs_trusty]], [[ira_network_owns_assignment]].
