---
name: pipe-trng-silicon-primitives
description: "Silicon TRNG primitive options for PIPE's on-die TRNG — design intent before any layout work"
metadata: 
  node_type: memory
  type: project
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

PIPE TRNG candidate primitives (from user notes, for ASIC implementation — not the current FPGA bringup phase).

**Four physics sources worth designing in, in increasing distance from "ring oscillator":**

1. **Metastability cell** — cross-coupled inverters with matched layout, deliberate setup violation on input. Simplest and smallest; well-characterized physics. *Layout symmetry is everything* — any asymmetry biases resolution direction. Treat as the primary entropy source.

2. **TERO (Transient Effect Ring Oscillator) cell** — two inverter chains, enable pulse fires both simultaneously, count transitions until they stabilize. Transition-count variance = entropy. Easier to analyze than free-running-ring phase noise, gives a clean NIST SP 800-90B story.

3. **Current-starved subthreshold inverter chain** — narrow PMOS in series to starve current, push inverters into subthreshold operation. Slow but thermal noise *dominates* deterministic delay there. Weird and noisy in the good way; very different statistical profile from a normal ring.

4. **Shot noise from reverse-biased junction** — almost any process gives you a parasitic diode for free. Reverse-bias it, amplify, digitize. *Different physics* (electron arrival statistics vs thermal fluctuation) — provides the independence argument across the entropy ensemble.

**FPGA validation limits**: ring oscillators on FPGA can validate architecture and post-processing chain, but cannot faithfully simulate metastability or subthreshold behavior. Both must be SPICE'd with process-corner Monte Carlo before layout commit.

**Why:** User has not yet decided final TRNG topology for PIPE. These four cover the physics tree (deterministic-but-noise-dominated → genuinely thermal → genuinely quantum), so ensemble entropy from independent sources is the goal — not a single primitive scaled up.

**How to apply:** When the user is ready to tackle TRNG (deferred from 2026-05-13 in favor of USB/PT work and FPGA wire bringup), surface these four sources as the design starting point and ask which to prototype first. Default to metastability + TERO on FPGA for architecture validation, plan shot-noise diode + subthreshold for SPICE before tape-out.

Related: [[pipe-slow-is-security]] — same conservatism-over-speed principle applies; entropy quality dominates throughput when picking which sources to commit silicon area to.
