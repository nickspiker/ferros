---
name: radios-at-attest-gps-locked
description: "Owner 2026-11-04 — all radios up at attest (locked), GPS never used unattested except a write-only locate; conditions that make radios-while-locked safe"
metadata:
  node_type: memory
  type: project
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-05T04:10:38.844Z
---

Owner decisions 2026-11-04:
- **All radios up at attest** (LTE, WiFi chip, BLE, NFC run while locked; today only LTE carries data while locked: WiFi needs the identity's `wifi.cred`, BLE/NFC users are fgtw which waits for a person). Owner asked whether this is a security issue; my answer: fine provided (1) every radio bus master is DMA-fenced, (2) no stable identifiers on air while locked (random per-boot address, passive scan, no BLE adverts but a rotating-key locate beacon), (3) nothing acts on radio input while locked without a fleet signature (no pairing, no NFC actions, no listening sockets, no network time).
- Found 2026-11-04: on a cold boot the modem's S2MPU (HSI1 G#1209_0000) and misc (G#101F_0000) have protection 0 — the modem can DMA anywhere. hv denies them only at a jump. Fence HSI1 to the modem's carveouts + shm (measured, on a cold boot); identify misc.
- **GPS: never while unattested**, one exception: **locate device**. Design proposed (not built): write-only locate — on a fleet-signed, Eagle-time-stamped request, a locked bound device takes a fix, seals it to the fleet's public key (X25519 + XChaCha20-Poly1305), sends over LTE (only `ip` path while locked; ip should allow only the locate responder to fgtw.org), never stores it readable (may queue the ciphertext). Later: BLE crowd relay (Find-My style). Unbound device: no locate.
- Done since (AUTH.md): `gps.last` is identity class (kvault.rs IDENTITY); the capture's position messages in `gps.replay`/`gps.replay.sky` are zeroed (the gps driver's scrub). The kernel refuses to start `gps` until a person attests.

**Why:** location is the person's; a locked/stolen device must not reveal where its person lives. Radios at attest serve locate and a usable device.
**How to apply:** don't start GPS pre-attest; don't let anything but a signed locate act on radio input while locked; fence before widening radio use. Related: [[auth-flow-decisions]], [[no-baked-apps]], [[radio-addresses-belong-to-identity]].
