---
name: token-recovery-shamir-not-oprf
description: "TOKEN custodian recovery uses hardened Shamir, NOT threshold-OPRF — OPRF is mathematically broken for this case"
metadata: 
  node_type: memory
  type: project
  originSessionId: b8f6088b-ca98-4de9-8420-6c13c0f0ddc1
---

TOKEN credential recovery (reconstitute after total device loss via custodians) uses **hardened Shamir threshold secret sharing** (the existing `custodes` GF(2^8) crate), NOT threshold-OPRF.

**Why threshold-OPRF was rejected (the non-obvious part — don't re-propose it):** the appealing pitch was "each custodian holds only their own independent key, zero per-designator state, so no correlation surface." That is mathematically incoherent with threshold recovery. Lagrange combination requires the custodian keys to be evaluations of ONE shared polynomial; n *independent* keys lie on a degree-(n−1) curve, so every different k-subset interpolates a DIFFERENT secret → honest recovery fails with zero malicious custodians. The only fix is a per-user DKG/VSS, which re-introduces per-designator share state — exactly the correlation surface OPRF was chosen to avoid. So OPRF cannot deliver "zero correlation surface" and "threshold-combinable" at once. (Found via the recovery-protocol workflow, 2026-06-24.)

**Decisive secondary reasons:** Shamir is already in patent Claim 2 + implemented + re-shareable (dead custodians → re-split same secret; OPRF keys are rotation-forbidden, so a dead custodian permanently erases a polynomial point). And every dangerous attack (deepfake, handle-hijack, MITM, gunpoint coercion) is **mechanism-independent** — it hits the human-recognition boundary, not the secret math — so mechanism choice never fixed the real attacks.

**The recovery design (now disclosed in TOKEN patent recovery + custodian sections):** redundancy margin N≥2K · parallel private sealed solicitation (not broadcast, not relay-through-one-custodian) · four-factor recognition gate for the new-device case (custodian-initiated callback on a pre-held channel + enrollment shared-secret + live improvised question + channel-pin SAS) · recognition-gated SIGNED shard release bound to the new device (seized vault = inert fragments) · per-shard commitments for robust/located reconstruction · correlation-resistant at-rest storage (random tag, Alice-supplied blob key) · mandatory non-compressible time-lock window for high-value (duress defense).

**Unresolved implementation fork (patent discloses BOTH ends, claims the property not the allocation):** the deepfake fix needs minimal per-(user,custodian) recognition-secret state on the custodian device, which trades against maximal correlation-resistance. Can't have both for free. See [[pipe_host_asks_enclave_answers]] for the related minimal-enclave-state philosophy.
