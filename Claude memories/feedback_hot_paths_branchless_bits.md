---
name: feedback-hot-paths-branchless-bits
description: "Owner wants per-pixel hot paths branch-free, with bit tricks (clz, exponent/mantissa, shifts, power-of-two scales) over divides and lookup tables; a compare that forks two maths paths is not a \"shortcut\""
metadata:
  node_type: memory
  type: feedback
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-24T20:53:48.937Z
---

On per-pixel / hot paths the owner wants one branchless path, built from bit tricks rather than divides or tables: `clz` and the bits under the top one as a log2 (Mitchell), a float's exponent/mantissa as a table index, power-of-two spans so scaling is a shift, f16 where its precision is enough, NEON lanes. A test that forks two equivalent maths paths (the grey test, 2026-10-24) was rejected outright: "a compare on the hot path is not a speed shortcut … locking up gates on the CPU for no reason". Skipping work that produces a constant (16 black pixels → black words) was accepted with the reasoning stated.

**Why:** the owner thinks in silicon cost (gates, pipelines), and prefers designs that are uniformly fast over ones that are fast on the common case.

**How to apply:** present the bit-trick options first and let the owner pick (they often push further, e.g. "f16?", "skip the clamp and let the tables do it"); then build it and time it with the PMU on the right core ([[pmu-timing-pin-x3]]). Look at the emitted assembly when numbers look off. Visual choices (glow brightness, linear vs gamma) are the owner's eyes: build a switch, show both. Related: [[colourspace-pinned-vsf-gamma2tanh]].
