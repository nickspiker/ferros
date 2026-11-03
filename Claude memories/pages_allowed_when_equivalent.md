---
name: pages_allowed_when_equivalent
description: "Owner 2026-11-03 — compiled masking is the default, but hardware page isolation is allowed where it is equivalent and faster; on Glyph we design the hardware, so use pages (or ISOMEM bounds) to our advantage for things designed isolated"
metadata:
  node_type: memory
  type: project
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-11-03T16:32:05.363Z
---

Owner, 2026-11-03, after the sandbox's CPU half landed: masking is "only for speed when warranted", and "on glyph we can design our own hardware so if it's equivalent to use pages to our advantage for things we know will be designed isolated, we can."

**Why:** KERNEL.md's execution model chose compiled masking over per-process page tables; this relaxes it: isolation must be equivalent, the mechanism is the faster one.

**How to apply:**
- Small processes stay masked (cheap, no tables).
- Big std processes (Photon) are the case for a per-process translation table with an ASID: same isolation by the MMU, no per-access cost, no emitter (no tpidr_el0 exception, no gcc asm through the rewriter, no 2S block). Proposed, not built.
- Guard pages (64 KiB unmapped above an arc) are equivalent to the 2S block and cheaper.
- DMA masters and the GPU are confined by IOMMUs (S2MPU, SysMMU, Mali MMU), built from the driver's handle table.
- Glyph: ISOMEM base-limit beats pages for the CPU; put ISOMEM bounds on every bus master too, so devices need no IOMMU.
Related: [[sandbox_state_2026_11_03]].
