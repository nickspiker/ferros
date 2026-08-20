---
name: pipe-popcount-alone-insufficient
description: PIPE TRNG health check uses layered RCT + APT + iterated GF(2) differencing — superseded earlier popcount+toggle dual check which still admitted balanced structured patterns.
metadata: 
  node_type: memory
  type: project
  originSessionId: 3623524f-ac39-42d0-b4d5-31df8cb753bf
---

PIPE TRNG ring health is checked by three concurrently-evaluated layers (Claim 3c, revised 2026-05-16):

1. **NIST SP 800-90B Repetition Count Test (RCT)** — instantaneous long-run detection, catches stuck-at faults.
2. **NIST SP 800-90B Adaptive Proportion Test (APT)** — windowed bias detection, standards-mandated.
3. **Iterated GF(2) differencing, K ≥ 4 levels** — at each level k, popcount must lie within W/2 ± J·σ (z-score window with J = 4). A periodic pattern of period 2^k collapses to all-zeros at level k, so the level-k popcount drops far below W/2 and the check trips. Catches any periodic pattern with period ≤ 2^K.

Ring healthy iff all three pass.

**Why the upgrade from the original popcount+toggle dual check:** popcount alone misses "stuck-then-stuck" (50% balance, 1 toggle). Adding toggle catches that, but the dual check still admits balanced structured patterns like the periodic `00000111` (period 8, 37.5% popcount inside the 1/8–7/8 band, ~25% toggle rate well above any reasonable minimum threshold) which carries near-zero per-sample entropy. Standards-mandated RCT+APT plus iterated GF(2) differencing closes that gap.

**Implementation: round-robin Variant A** — one shared `ring_health.v` instance + `ring_scheduler.v` FSM that drives 16 sequential per-ring gathers. Each ring gets the full W=65,536 sample gather (full statistical power), one at a time. Per-ring `ring_enable` is one-hot. KOHI total = 16W = 1.75 s at enclave_clk = 600 kHz; fits per-session handshake budget (sessions are long-lived) and beneath Claim 8 TATARI for OTP burn.

**Measured synth on ECP5-25F (yosys + nextpnr-ecp5)**: 689 LUT4 + 474 FF + 151 carry cells total for the 16-ring health subsystem. 13× LUT reduction vs parallel-instance equivalent (9,280 LUT4 measured). Fmax 100–106 MHz across placement seeds. At 28 nm ASIC, ~3,500 standard cells ≈ ~0.01 mm² (~2% of SOT-23 die budget).

**SOT-23 package context:** PIPE's target package is SOT-23 (3-pin surface-mount, ~3 × 1.4 mm body, smallest standard 3-pin package). Silicon area at a premium — Variant A's small footprint is what makes the layered health check affordable on this package envelope.

**Side benefit:** the layered check also defends against physical extraction attacks. An adversary with device custody who attempts to coerce predictable output via voltage glitching, clock injection, thermal forcing, or EMI cannot succeed without producing a stuck/biased/periodic ring state — all of which the three layers detect and convert into the silent-wire FUCKED state of [[pipe-fucked-state-silent-wire]].

**How to apply:**
- The earlier popcount+toggle implementation in `trng.v` is superseded; the new health stack is RCT + APT + iterated GF(2) differencing.
- Tight z-score check (W/2 ± J·σ where σ = √W/2) at each iterated-diff level — much tighter than the old "toggle ≥ W/256" threshold.
- Same architectural defense layers still apply downstream: per-ring health → Byzantine threshold (8-of-16, per [[pipe-byzantine-ring-threshold]]) → silent-wire on failure → BLAKE3 conditioner on accumulated output.

Related: [[pipe-slow-is-security]], [[pipe-byzantine-ring-threshold]], [[pipe-fucked-state-silent-wire]], [[pipe-pt-unified-stack]].
