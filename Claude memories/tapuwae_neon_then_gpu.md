---
name: tapuwae_neon_then_gpu
description: tapuwae (attest mark / Photon avatars) renders on NEON for now; owner 2026-10-29 — it MUST be converted to the GPU once ferros drives the Mali
metadata:
  node_type: memory
  type: project
  originSessionId: 73becc5a-a246-4afa-9e54-76a7ff3d89c6
  modified: 2026-10-29T16:37:42.634Z
---

The attest mark renderer (tapuwae, /mnt/Harbor/Code/tapuwae) goes NEON first: stratified 4×4 samples a pixel, four lanes in lockstep, the shading's transcendentals as our own branch-free polynomials (no libm). Owner, 2026-10-29: "NEON for now, but make sure we note it needs converted to GPU when we get that shit hot."

**Why:** measured on the desktop the shading is ~70% of the cost (half of it libm) and the orbit only 14–89 iterations a sample; a pixel's 4×4 grid is exactly one 16-lane Mali Valhall warp (lockstep efficiency 63–91% at 16 lanes, 82–96% at 4). The GPU is blocked on the CSF firmware and a shader compiler (KERNEL.md: after toka).

**How to apply:** keep the render code branch-free and libm-free so it ports to a shader unchanged; when GPU work starts on ferros, the attest mark is a first customer. Noted in tapuwae's README ("Speed: NEON now, the GPU later"). Related: [[ferros-attest-os-level]], [[feedback-hot-paths-branchless-bits]].
