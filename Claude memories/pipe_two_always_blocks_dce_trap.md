---
name: pipe-two-always-blocks-dce-trap
description: Two always blocks driving the same registers cause yosys to silently dead-code-eliminate one of them; outputs stay at reset value and the entire downstream logic gets pruned.
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

When writing Verilog modules with both sequential FSM logic AND output latching, **do not split them across two `always @(posedge clk)` blocks driving the same registers**. yosys treats this as a multi-driver conflict and silently drops one of the always blocks. The dropped block's outputs hold at their reset value forever. Downstream consumers see all-zeros (or whatever the reset value is) and the synthesizer correctly prunes them as dead code, leading to massive (10×+) reductions in reported LUT/FF count that mask the bug.

**Why:** During an `inverted_pair_verify.v` refactor I split the popcount sweep FSM and the classifier-output latching into two separate `always @(posedge clk or negedge reset_n)` blocks, both targeting `pair_virgin`/`pair_valid`/etc. Synth reported 439 LUT4 instead of the expected ~10K — the output-latching block had been dropped and the outputs stayed at reset-zero, causing yosys to eliminate the entire downstream classifier+ira_fsm chain as dead-code-driven-by-constants. The bug was caught when stat numbers looked impossibly good.

**How to apply:**
- Combine sequential FSM + output latching into a SINGLE `always` block. Use intermediate latch enables (e.g., `latch_outputs <= end_of_sweep;`) and conditional updates within the one block.
- When a synthesis report shows suspiciously few cells, suspect dead-code elimination from a broken assignment chain. Sanity check: does the LUT count match the order-of-magnitude of the design's input/output complexity? If a 10K-LUT module synths to 400 LUTs, something has been silently optimized away.
- yosys warnings about multi-driver conflicts may not be loud — scan the full log for "Removed unused" or "memory \\X with list of registers" patterns when in doubt.
- The same pitfall applies to mixing `assign` continuous assignment with `always`-driven assignment to the same register — pick one.

Confirmed harmless when always blocks drive DISJOINT register sets, even with overlapping triggers.
