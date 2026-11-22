---
name: gpu-role-layers
description: "REVERSED 2026-11-21: the GPU composites full frames, scrolling and damage above the measured crossover; NEON keeps small damage. (2026-11-05 had kept under + present on NEON.)"
metadata:
  node_type: memory
  type: project
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-05T17:45:45.378Z
---

**Owner (2026-11-05):** "not sure 'switching to' is what I was thinking. I was thinking a carefully segmented pipe where we keep the buffers in RAM organized proper and split tasks accordingly. We may only run the mandelbrot compute thru the GPU, and possibly some other glow/edge/effects but I'd suspect under is already stupid fast if we layer it out right."

**Measurements behind it** (GPU.md):
- The present stage on the GPU is bit-exact but about 3.5x slower in wall time even at 890 MHz (memory-light, branchy table work: NEON wins).
- The Mandelbrot on the GPU: bit-exact, 0.47 ms for 4096 pixels × 64 iterations.
- CPU↔GPU sharing needs no copy: the GPU snoops the CPU's caches when its pages are coherent (MEMATTR 6 + SH both, COHERENCY_ENABLE none).

**How to apply:**
- GPU work = layers with many operations per pixel, written into a shared arena (CPU-cacheable, GPU-coherent, inside the GPU's fence), consumed by the CPU's `under` with no clean or copy.
- Don't move the present stage or `under` to the GPU, and don't widen the fence to the display's canvas or phases; give the GPU its own layer arena instead.
- Open decisions: how kernels are produced (Mesa's compiler on the host vs our own), and the GPU as a resident service.
- Related: [[gpu-online]], [[tapuwae-neon-then-gpu]].

**Reversed 2026-11-21** (owner, on GPU-KISTS open question 13, relayed by the Photon session: "Reverse, yes."). With workgroup memory and the parallel noise (forge 715525d4, be536647) the GPU makes the whole frame's phases in 8.8 ms at 43 mJ a frame above idle against NEON's 147 (36 vs 109 under the policy), no CPU a frame. So:
- The GPU composites full frames, scrolling and any damage above the measured crossover (about 2^14 to 2^15 px at held clocks, 2^11 to 2^12 under the policy); NEON keeps smaller damage. The line comes from measurement, never a picked constant.
- The noise needs no match between the sides: "If it's close, literally nobody will notice. if it flickers, I'll probably like it." fluor's CPU noise_under stays for NEON patches; the GPU uses the parallel form.
- Isolation must not weaken (the owner's condition, "not sacrifice security"): a display-owned GPU context reading app surfaces read-only, an L2 invalidate per job (END_FLUSH, done), surfaces held from reuse while read (GPU-KISTS, the compbench security analysis).
