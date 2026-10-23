---
name: eret-race-irqs-masked
description: any code that loads ELR_EL2/SPSR_EL2 for an eret must run with IRQs masked — ferros_el0_resume didn't, and an SGI kick in the window parked cores on a self-returning eret (the 2026-10-23 freezes); diag core <k> dumps a core's PC/PSTATE via SGI 2
metadata:
  type: feedback
---

The 2026-10-23 shell freezes ("one drag then frozen", killswitch dead when the X3 was hit) were cores parked on `.Lel0_restore`'s `eret`: `ferros_el0_resume` ran with IRQs enabled, an interrupt between `msr elr_el2/spsr_el2` and the `eret` overwrote them with the eret's address and EL2h, and the eret returned to itself forever. Masking IRQs at the top of `ferros_el0_resume`/`ferros_el0_enter` fixed it (the eret restores the thread's PSTATE).

**Why:** exception entry clobbers ELR/SPSR; the trap-return path was safe only because entry had masked IRQs. SMP's SGI kicks made the window hit within seconds.

**How to apply:** every `eret` sequence runs with IRQs masked, and a core "running" a thread whose slice counter is frozen with no crumbs is diagnosed with `diag core <k>` (SGI 2 → the handler dumps ELR/SPSR/SP/x0-x3 into __core_dump; `diag threads` shows CURRENT per core, `IRQ_STAGE_Ck` the handler's last stage, `NOTE_k` the core's last syscall crumb). A core parked that way cannot CPU_OFF for a reload (comes back ALREADY_ON) — a cold boot restores it.
