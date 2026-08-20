---
name: pipe-byzantine-ring-threshold
description: PIPE uses N rings with K-of-N byzantine threshold; the chip operates if any ≥K rings pass health. Math + fleet-safety derivation.
metadata: 
  node_type: memory
  type: project
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

PIPE's TRNG uses 16 ring oscillators with an 8-of-16 byzantine threshold for fleet-safety. The chip operates if ≥8 rings pass their health checks; ≤7 healthy means FUCKED and silent wire.

**Why:** Per-ring failure rate must be derived from the fleet operating budget. For target `P_chip < 10⁻¹⁵` per KOHI (so a fleet of 10¹¹ devices × 10 KOHIs/day × 100 years has < 1 expected false-FUCKED event total), the math:

```
P(≥9 fail of 16) ≈ C(16, 9) · p⁹ = 11,440 · p⁹  < 10⁻¹⁵
⇒ per-ring p < 7.9 × 10⁻³
```

Per-ring failure budget = 0.79%. With our actual TOG_MIN = N/256, per-ring toggle-fail rate under worst-case stable-frequency analysis is 0.39% — ~2× margin under the ceiling. Popcount (1/8–7/8 band, 12σ from BLAKE3-output mean) adds negligibly to per-ring failure rate.

**How to apply:**
- When picking ring count N and byzantine threshold K, compute per-ring failure budget from target fleet `P_chip` and verify it against the actual TOG_MIN/N rate. The relation is `P_chip ≈ C(N, N−K) · p^(N−K)`.
- XOR mixing means a single healthy ring is theoretically sufficient for cryptographic uniformity (XOR of any set including one uniform input is uniform). The byzantine threshold is defense-in-depth against catastrophic mass failure, not strict cryptographic necessity.
- The threshold applies to ring health, NOT to slot health. Slot byzantine (4 inverted pairs, 2-of-4 valid required) is a separate check governed by `inverted_pair_verify.v` per [[pipe-ira-is-meaningless-alone]].
- Going from 8 rings to 16 rings + byzantine threshold is the difference between "all must pass" (8× failure-rate multiplier into per-chip rate) and "≥half must pass" (combinatorial small power of per-ring p). 16 rings cost ~270 LUTs + ~290 FFs more than 8 rings — modest.
- At silicon tapeout: per-ring physics differ (TERO, metastability cell, shot-noise diode); per-ring failure rates and ideal threshold may need re-derivation from process-corner Monte Carlo. Architecture stays.

Related: [[pipe-popcount-alone-insufficient]] (the toggle counter is what brings per-ring failure rate down to fleet-safe), [[pipe-fucked-state-silent-wire]] (what happens when threshold isn't met).
