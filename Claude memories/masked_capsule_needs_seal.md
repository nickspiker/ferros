---
name: masked-capsule-needs-seal
description: "A masked payload (ahau, kists' installs) stored with store put is refused at the next boot unless sealed: run ferros-bridge seal <masked.bin> --text-len N on each device after storing"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-11T05:11:29.157Z
---

2026-11-10: I stored a new masked ahau with `bridge capsule --masked` + `store put` and flashed a kernel; the Pro rebooted to a black screen: "ahau: gate: masked capsule not installed (no seal)", so no attest screen at all. A masked capsule runs only once the device has checked it and added its seal to the vault's `sealed` list.

**Why:** the seal is per device (each device's hash differs); a plain store keeps the bytes but not the seal, and the old seal names the old bytes.

**How to apply:** after storing any masked payload (tools/ferros-masked-build.sh output, e.g. ahau), on every device: `ferros-bridge seal target/masked/<name>/<name>.bin --text-len $(cat target/masked/<name>/<name>.textlen)`, then `diag "restart <name>"` to check it runs before any reboot. Kists installed thru the installer are sealed by it.
