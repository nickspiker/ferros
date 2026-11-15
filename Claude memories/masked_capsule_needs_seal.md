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

**How to apply:** after storing any masked payload (tools/ferros-masked-build.sh output, e.g. ahau), on every device: `ferros-bridge seal target/masked/<name>/<name>.bin --text-len N`, where N is the masked build's ".text: N words" count x 4 (the same N given to `ferros-bridge capsule ... --masked --text-len N`); NOT the `.textlen` file, which is 52 bytes larger and doesn't match what the devices store (found 2026-11-14 deploying the Pro), then `diag "restart <name>"` to check it runs before any reboot. Kists installed thru the installer are sealed by it.

**Correction 2026-11-15 (pcie1 on the Pro):** words x 4 is too short. The scan's count leaves out the 13-word image header, so it is the `.textlen` (the whole .text) less 52 bytes, and the last 52 bytes of .text are the mem* stubs (memcmp, memcpy, memmove, memset). Only the declared text is executable: when the stubs cross a 4 KiB page past the declared end, the first memset call faults (esr G#8200000F, elr = memset's offset + the arc's G#1000). It worked before only while the page rounding happened to cover them. Use the `.textlen` value as N, the same N for `capsule --text-len` and `seal --text-len` (pcie1, modem and init stored that way 2026-11-15; pcie1 and modem ran, init runs at the next boot). The 2026-11-14 mismatch was most likely a capsule and seal made with different N.
