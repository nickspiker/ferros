---
name: pipe-ira-is-meaningless-alone
description: "PIPE's `ira` (boot secret) is just 256 bits without TOKEN binding — the user's TOKEN is what gives device attestations meaning. Architectural consequence: no self-destruct needed."
metadata: 
  node_type: memory
  type: project
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

**The principle**: PIPE's `ira` (the 256-bit boot secret burned at provisioning) is, alone, just entropy. Useful attestations come from binding `ira` to a user's **TOKEN** (the user's ferros identity). Without that binding, raw `ira` bits cannot impersonate anyone or assert anything — there is no relying party in the system that recognizes naked `ira` as authoritative.

**Why this matters**: This makes PIPE inherit the FIDO-security-key safety property — extracted credentials are useless without the relying-party context. Ownership transfer happens by unbinding the old TOKEN and binding a new one; no chip-level destruction is required.

**Architectural consequence**: PIPE has *no killswitch, no self-destruct, no permanent-halt command*. Removed from design entirely. The chip's only permanent-failure path is physical/electrical interruption of provisioning (which brick-preserves the OTP for diagnosis), or terminal aging that defeats the 3-of-4 Byzantine majority across slot pairs.

**Why this is the right call**:
- Every use case for self-destruct (resale, end-of-life, recall, seizure) is solved at the TOKEN/registry layer, not at silicon
- Removes attack surface — no command sequence can permanently kill a working chip
- Removes denial-of-service attack vector — no retry-exhaustion path
- Preserves forensic evidence in failed chips for RMA learning
- Chip is *indestructible from outside*: only silicon physics can change its state

**The three lifelong states**:
1. **Virgin** (A=B=0 across all pairs) → can be provisioned
2. **Provisioned** (3-of-4 pairs valid + secrets agree) → operates, optionally DEGRADED-flagged
3. **Failed** (interrupted burn or terminal aging) → refuses, OTP preserved

No host, key, command, manufacturer, or attacker can drive the chip between these states. Only `KOHI` → `TATARI` → `TAHU` on a virgin chip, or aging on a provisioned chip, or interruption during `TAHU`.

**How to apply**: Whenever someone proposes adding chip-level destruction (whether for security, privacy, or operational reasons), check whether the use case can be solved at the TOKEN/registry layer instead. It almost always can. The chip stays simple.

Related: [[pipe-slow-is-security]] — same conservatism principle (the chip does the minimum and trusts the layers above to handle policy); [[pipe-trng-silicon-primitives]] — the entropy that produces `ira` is meaningless on its own, only meaningful within the binding.
