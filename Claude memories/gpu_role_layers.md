---
name: gpu-role-layers
description: "Owner 2026-11-05 — the GPU is a producer of compute-dense layers (Mandelbrot/tapuwae, glows, edge effects) into shared RAM; the CPU's front-to-back `under` and the present stage stay on NEON; no wholesale \"switching\" of fluor to the GPU"
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
