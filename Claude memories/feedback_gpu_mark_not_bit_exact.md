---
name: feedback-gpu-mark-not-bit-exact
description: "The GPU attest mark does NOT have to be bit-identical to the CPU; speed wins, \"looks the same\" is the bar"
metadata:
  node_type: memory
  type: feedback
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-05T23:54:55.097Z
---

The owner never asked for the GPU attest mark (tapuwae on valhall) to match the CPU's pixels bit for bit; I invented that constraint and kept repeating it (owner, 2026-11-05: "Why do you keep saying it MUST STAY IDENTICAL!!!! I literally NEVER said that.").

**Why:** the goal is the mark under a 120 Hz frame; holding exactness blocked FMA contraction, raw FRCP/FRSQ and the hardware transcendentals.

**How to apply:** use any GPU shortcut that keeps the mark visually the same (contraction, unrefined SFU ops, hardware exp/log); report the pixel difference as a measure, not as a gate. Don't invent invariants and attribute them to the owner. The CPU's own pinned hash (`attest_mark_holds_still`) is a separate CPU regression test. Related: [[feedback-hot-paths-branchless-bits]].
