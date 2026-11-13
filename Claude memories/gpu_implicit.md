---
name: gpu-implicit
description: "GPU access is implicit for every kist, like CPU time: no permission, never asked (owner 2026-11-11 and 2026-11-12); isolation between GPU clients is the protection"
metadata:
  type: feedback
---

Owner, 2026-11-11: "No GPU prompt, that's implicit." Again 2026-11-12, when I proposed a `gpu` right: "I feel like GPU should be implicit. If I'm running it on my machine..."

**Why:** the GPU is compute, like the CPU; a kist the person chose to run gets both. A prompt protects nothing the hardware isolation doesn't.

**How to apply:** never propose a GPU permission or want. Every launched kist gets its own GPU arena region and surfaces from the kernel; the protection is per-client isolation in the GPU MMU/kernel plus fair sharing, and (until the checked GPU kernel format exists) a development-build-only gate for unchecked GPU programs. Related: [[permissions-asked-once]].
