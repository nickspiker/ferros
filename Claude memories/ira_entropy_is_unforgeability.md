---
name: ira_entropy_is_unforgeability
description: "ira entropy matters for UNFORGEABILITY, not confidentiality; a forgeable ira registered+unlocked = an unevictable, permanently-rekeyed fleet member; best-ira = physical PUF core + vendor breadth"
metadata: 
  node_type: memory
  type: project
  originSessionId: 35f77793-aafd-4fba-bd97-4cd4e60013b9
---

Settled 2026-08-20. The ira is public (see [[ira_is_brand_not_credential]]), so its entropy is not about hiding it — it is about whether an attacker can FORGE a device (real or emulated) that derives an ira the fleet already trusts.

**Why:** if a forgeable ira is registered AND unlocked in the fleet, the attacker becomes an unevictable member — every SHRINK re-wraps the fleet key to that ira, and you cannot lock it without locking your own device because you cannot distinguish the brands.
So a serial-only ira is unsafe: a structured serial is settable in emulation and readable with brief possession.
The ira secret must root in material that is high-entropy AND physically bound AND not attacker-settable.

**How to apply:** build the ira in two layers — an unforgeable physical PUF core (fuzzy-extracted: secure sketch + ECC, public helper data) plus vendor-issued identifiers for breadth/collision-resistance (Google's opacity on those is a plus, not a source of secrecy).
Tier sources so a drifting bit never bricks the vault: Tier 0 deterministic serials key now; Tier 1 chipid trims graduate via a two-boot + temp-cycle probe; Tier 2 PUFs (UFS bad-block, camera hot-pixel, SRAM, MCT clock-ratio) ride fuzzy extraction.
Grounded husky source catalog + register addresses: [[ira_entropy_sources_husky]] and ferros/IRA-ENTROPY-SOURCES.md. Measure before keying (see [[feedback_measure_before_fixing]]).
