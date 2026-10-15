---
name: feedback_killswitch_irq_mask
description: Any path that returns into the kernel from an exception must unmask IRQs, or the Vol Up+Down killswitch silently dies; check DIAG DAIF (I bit) after processes run
metadata:
  type: feedback
---

The killswitch (Vol Up + Vol Down → SYSTEM_OFF) is IRQ-driven; it only works while PSTATE.I is clear. On 2026-10-15 the owner found it dead: `.Lel0_leave` returned from EL0 exception context into the kernel with `ret`, leaving IRQs masked from the first process (boot's `init`) onward — DIAG showed DAIF G#3C0 and only 7 IRQs per boot.

**Why:** the owner treats the killswitch as a hard guarantee ("as kernel-ey as we can"); a silent disarm is worse than no killswitch.

**How to apply:** when adding any new exception-return or context-switch path (EL0 trap/preempt, fault recovery, new process models, toka VM), make sure the kernel resumes with IRQs unmasked (`msr daifclr, #2`); the service loop re-asserts it every pass as a backstop. After hardware changes, verify `bridge diag | grep -E "DAIF|KILL_IRQS"` — DAIF must read G#340 and KILL_IRQS must climb. Related: [[ferros_process_model_husky]].
