---
name: dpu_power_secure_registers
description: "ferros switches the DPU domains off/on itself (dpu_power.rs, diag dpu); rules learned 2026-11-03: D_TZPC save before off, muxes before any access, CONTROLLER_OPTION last, never touch zero-reading (secure) words, DSIM in reset after power-up"
metadata:
  type: project
---

The display power-up from off (DISPLAY-DESIGN.md §8) layers 1-2 work: `diag dpu off|on` cycles the three DPU domains with the CMU restored from the cold-boot capture (oracle/display/husky-hk3/cold-power).

**Why:** jumps that find Android's DPU hibernated, ferros's own screen off, boots without ABL's splash.

**How to apply (each one cost a hang):**
- SMC G#8200_0410 (0 save / 1 restore, need_smc, 2) always as a pair: restore without a save never returns.
- PMU CONFIGURATION G#1546_1D00/1D80/1E00 is the on request; STATUS is +4. Check both.
- CMU user muxes (+G#600/+G#610) before any DPU register access.
- CONTROLLER_OPTION (+G#800) last, after all domains are up.
- **Secure registers read 0 non-secure** (D_TZPC): the DPUF sysregs, the zero Q-channel slots. Touching one poisons EL3 like a dead domain: the next EL3 call hangs. Touch only words that read non-zero on a cold boot.
- DSIM held in reset after power-up (SYSREG_DPUB +G#1008 bit 0): its init sets it before any DSIM access.

Related: [[hv_jump_to_ferros]], [[display_driver_design_first]].
