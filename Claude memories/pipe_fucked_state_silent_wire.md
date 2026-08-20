---
name: pipe-ngaro-state-silent-wire
description: "PIPE NGARO state (formerly FUCKED then DARK): chip gone silent. Trigger conditions: ≤3 of 8 healthy rings OR byzantine OTP fail OR interrupted burn → wire goes silent, no heartbeat."
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

**State renamed 2026-06-07: FUCKED -> DARK (interim) -> NGARO (final).** See [[pipe-kore-state]] for the full te reo Maori state set. NGARO = lost / vanished / silenced.

A PIPE chip in the NGARO state does not drive the wire at all — no heartbeat, no slot transitions, nothing. The host's pull-down resistor holds the line at 0 forever. From the host's perspective, "NGARO PIPE" looks identical to "no PIPE connected"; both mean "no functioning PIPE here."

**Why:** Stated by the user during the trng.v expansion. Heartbeat means "I can authenticate." If the chip can't authenticate (rings broken, OTP corrupted, burn interrupted), broadcasting heartbeat would lie about its capability and would give an attacker a side-channel they could exploit. Silent is the honest answer.

This preserves the Claim 6 "indestructible-from-outside" invariant because going silent is a *silicon-level truth condition* describing pre-existing reality, not a protocol-triggered state transition. No host command, no incoming wire pattern, no debug message causes a chip to go silent — only intrinsic broken-ness does. OTP and state remain readable for forensic analysis; the chip is silently-bricked, not destroyed.

**How to apply:**
- NGARO triggers (all gate `tx_drive_enable` to 0 in the codec wrapper):
  - `trng.health_fail` latched: <8 of 16 rings passed the dual popcount + toggle health check during the last KOHI
  - OTP read returns fewer than Byzantine-majority valid pairs (the 2-of-4 minimum)
  - OTP burn ceremony interrupted partway (the bricked-with-preserved-fuses state)
- HARA / WHARA chips (4–7 healthy rings, 2–3 of 4 OTP pairs) DO heartbeat normally — they can still authenticate. The degradation level is reported only via attestation-request response per [[pipe-host-asks-enclave-answers]].
- One-line implementation: `assign tx_drive_enable = !chip_ngaro;` in the codec wrapper, where `chip_ngaro = trng.health_fail || otp_byzantine_fail || burn_interrupted_latched`.

Related: [[pipe-host-asks-enclave-answers]] (no proactive state reporting), [[pipe-ira-is-meaningless-alone]] (why no killswitch needed — silence is enough).
