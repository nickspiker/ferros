---
name: pipe-host-asks-enclave-answers
description: "PIPE protocol rule — the enclave never proactively reports state, only answers when queried. Heartbeat carries no information."
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

The PIPE enclave never volunteers state on the wire. It heartbeats (the wire idle pattern) or it goes silent. Beyond that, all information transfer is host-initiated request / enclave-computed response.

**Why:** Stated by the user during the trng.v expansion when discussing how DEGRADED ring-health should be communicated. The principle prevents the enclave from leaking timing or side-channel information through proactive status pushes, keeps the wire-level protocol stateless from the enclave's perspective, and matches the "indestructible-from-outside" / minimal-protocol philosophy (Claim 6, Claim 7).

**How to apply:**
- No "I am degraded" or "OTP just got corrupted" status broadcasts.
- No periodic state pings or wakeup advertisements.
- The heartbeat is *purely* an alive/dead signal — present + alternating means "alive enough to drive," absent means either FUCKED or unpowered/disconnected (indistinguishable, which is correct).
- Internal state (ring_health_cnt, otp_pair_count, 5-state classifier, wairua/ira presence) lives as registers inside the chip. The codec does NOT reach in to slip these into the heartbeat or anything else.
- The only path from chip-state to wire-bits is via an attestation request. Even then, the response is a hash/derivation that obscures raw fields.
- When designing new FSMs (ira_fsm.v, codec.v wrappers, attest handlers), default to: "what does the host need to ask for to get this information?" not "when should I send this information?"

Related: [[pipe-fucked-state-silent-wire]] (silent-wire policy on FUCKED state), [[pipe-ira-is-meaningless-alone]] (ira-needs-TOKEN-binding).
