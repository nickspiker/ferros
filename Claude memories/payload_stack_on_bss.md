---
name: payload-stack-on-bss
description: "FIXED in the kernel 2026-11-09 (70ad26a): every payload carries a size header via ferros_sys::entry! and gets a guard page under its stack; images without the header are refused. Was: a no_std arc sized without .bss, stack on the last statics"
metadata:
  type: project
---

**The bug (2026-11-08, the 8a's display):** `process.rs build` sized a no_std payload's arc from its flat image, which has no .bss, so the arc could end inside .bss and the first thread's 64 KiB stack silently overwrote the last statics. Crashes moved with every build (band table, glyph caches, repeated rows), so feature bisects misled. A capsule over the vault's put limit also never stored, so later tests ran the old build: check for "stored" after every store put.

**The fix (70ad26a, flashed on the 8a and the Pro 2026-11-09):** every payload carries a header thru `ferros_sys::entry!` with `__ferros_tail = ADDR(.bss) + SIZEOF(.bss) - __file_end + 16`; the kernel refuses an image without it ("the image has no header") and puts a kernel-only guard page under every first-thread stack (payloads/stackprobe faults in it). `tools/payload-check.sh` runs before storing.

**How to apply:** a new payload must use `ferros_sys::entry!` and pass payload-check. Before flashing a kernel that changes the capsule ABI on any device, re-store every process capsule from a current build (`store ls`, then put each), or old capsules will not start. Related: [[sandbox_state_2026_11_03]], [[masked-capsule-needs-seal]].
