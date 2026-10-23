---
name: kernel-on-x3
description: since 2026-10-22 the husky kernel (single-core) runs on the Cortex-X3 (CPU8, G#800) after a boot-time hop from CPU0; why, how, the per-core register trap, and the escape (FERROS_X3=0)
metadata:
  type: project
---

The ferros kernel on husky runs on the Cortex-X3 big core, not the A510 the bootloader starts on (x3.rs, KERNEL.md "Where it runs on husky"). CPU0 starts CPU8 with PSCI CPU_ON right after the cache map, the X3 parks with CPU0's translation, and at service start CPU0 hands over the boot state and parks itself in WFI (no CPU_OFF: cluster and watchdogs untouched).

**Why:** the shell's frames were 135–305 ms on the A510 (its NEON is weak and shared); on the X3 at 2.91 GHz they are 20–29 ms — the owner asked why rotate/zoom was slow (2026-10-22 night).

**How to apply:**
- Every per-core EL2 register CPU0 set must be copied into the hand-over block. CNTHCTL_EL2 was missed first: EL0 counter reads trapped (ESR class G#18) in the display server.
- A reload issued from the X3 starts the new kernel on the X3 (launch skips; stale "parked" flag must be cleared first — it hung once).
- Test kernel changes that touch the hop by hot reload: a hang watchdog-resets into the flashed kernel within ~2.5 min, no hands needed.
- FERROS_X3=0 builds keep CPU0.

Related: [[husky-cold-slot-b]], [[feedback-measure-before-fixing]].
