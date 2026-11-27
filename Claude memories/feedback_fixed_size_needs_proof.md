---
name: feedback-fixed-size-needs-proof
description: No-ceilings means a fixed size needs a proof it is the right size, not that nothing is fixed; u16 hit ids and the u16 TCP handle are the examples
metadata:
  type: feedback
---

The owner, 2026-11-27: "no fixed sizes isn't quite right. it's more of a proof for the right size if it's fixed. our hitmap for example is 16-bit, and we just made the TCP 16 bit too."

A fixed width or count is fine when it comes with a proof that it can never be the limit that binds, the way AGENT.md Rule 0 asks a bounds check for its proof. A preset with no proof is the ceiling the founding rule forbids.

**Why:** founding rule 3 (no fixed ceilings, 2026-11-23) was being read as "nothing fixed"; the owner means "nothing fixed without a proof". The same day he said "Def grow on demand. Not sure why we have so many hardcoded limits around" about ip's sixteen connections, which had no proof.

**How to apply:** for every fixed size, write the proof beside it (a comment or the doc), e.g. ip's handle: 2^16 connections would hold 2^16 blocks of 1.25 MiB, 80 GiB, past any device's memory, so memory refuses first. With no proof, make it grow on demand, or bring the size to the owner. See [[owner-decisions-2026-11-23]], [[feedback-hot-paths-branchless-bits]].
