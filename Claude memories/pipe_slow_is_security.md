---
name: PIPE — slow ragu/spaghettify is the security property, not the cost
description: Throughput-bounding is a deliberate security property in PIPE. Do not optimize chaos_amp_v2/spaghettify for speed. Pick the slowest clock that meets usability, not the fastest that closes timing.
type: project
originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---
When tuning PIPE clock parameters (RING_STAGES, RING_DIV, any successor structure that sets enclave_clk), the heuristic is **slowest configuration consistent with usability**, NOT fastest configuration that closes timing.

**Why:** Fast crypto is vulnerable to fast attacks. If a primitive can produce N attestations/sec, an attacker can request N attestations/sec and do statistical correlation, differential cryptanalysis, timing analysis across the response set. A physically-bounded throughput (ring osc, 88 MHz silicon Fmax for chaos_amp_v2, 514 cycles per ragu call) hard-caps the attacker's sample rate. This is not "we rate-limited it in software" — it's "you cannot exceed the ring oscillator's natural pace, ever." Same mechanism that makes spaghettify (the tortoise) the structurally-orthogonal primitive in the system: slowness IS the protection.

This is an *independently sufficient* second reason for the no-PLL rule in production. The original justification was unforgeability (host cannot synchronize to a clock it never sees stably). Now there's a second: PLLs can be cranked far past silicon physics; ring oscillators cannot. Both reasons must hold.

The corollary: **chaos_amp_v2 being 0.68× slower than blake3 (88 MHz vs 129 MHz silicon Fmax) is a feature, not a deficit.** Spaghettify being the narrow part of the throughput pipe is correct — that puts the structurally-orthogonal primitive on the attacker's sample-rate critical path, rather than the ARX-family hash.

**How to apply:**
- When picking RING_STAGES / RING_DIV for production `top.v`: prefer higher stage counts and larger divisors (slower) over the minimum that closes timing.
- Reject any "let's add a PLL to make ragu faster" proposal even if it would close timing. Bounded throughput is load-bearing.
- Do not propose optimizations to chaos_amp_v2 that would raise its Fmax above blake3's. The slower primitive must remain on the critical path.
- When sizing the production clock floor, the constraint is usability (responsiveness of user-facing PIPE operations), NOT throughput maximization. If a 30 MHz enclave_clk is fast enough for the planned use, do not bump to 60 MHz.
- "Spec-sheet attractive" numbers (high MHz, high Gbps) are anti-features here. The differentiating product claim is "deliberately slow, structurally orthogonal, mathematically independent" — speed undermines it.
