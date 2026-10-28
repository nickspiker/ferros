---
name: keys-in-registers
description: "since 2026-10-28 the ira and wairua live only in CPU registers (pointer-auth keys XOR dead EL1 regs), code lifted from safe Rust; those registers are taken, never write them"
metadata:
  node_type: memory
  type: project
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-28T19:23:38.881Z
---

Since 2026-10-28 the ira and wairua have no RAM copy: each is two shares, APIA/APIB/APDA/APDB (pointer-auth keys, switched off) XOR TPIDR_EL1, SP_EL1, ELR_EL12, FAR_EL12, MAIR_EL12, APGAKey, TPIDR_EL2. Code: keyreg/keys.rs (safe Rust) → tools/keyreg-lift.sh → keyreg/keys.s, linked by ferros_kernel/src/keys.rs. `diag keys`, `diag keys sleep`, payloads/keytest.

**Why:** owner asked for the ira and wairua out of DRAM ("crypto registers"), fast (split fetch 16-24 cycles), surviving deep sleep with the same wairua; owner's workflow is "write it in safe Rust, then chainlift the assembly" (derive → audit → freeze, KEY_REGISTERS.md).

**How to apply:**
- Those registers are taken: ferros never uses pointer authentication, never uses TPIDR_EL2 as a per-cpu pointer, and `diag keyreg` (the probe that writes patterns) refuses once keys are held.
- Edit keyreg/keys.rs, never keys.s; re-run the lift, which refuses stack/v8-v15/calls. -O2 not -O3 (unrolled rounds spill); outputs must be 16-byte aligned (strict-align target); helpers must be macros (target_feature fns won't force-inline).
- The firmware TRNG costs ~1.3 ms a call: keep it off any wake path (masks come from the seal, then the wairua stream).
- Core power-down/deep sleep: call keys::leave before, keys::serve/ensure after (not wired yet; ferros does no c2). No always-on storage dies on reset (PMU CPU_INFORM cells are live firmware state), so the reset-and-dump gap waits for PIPE. Related: [[ira-is-brand-not-credential]], [[identity-not-on-disk-default]].
