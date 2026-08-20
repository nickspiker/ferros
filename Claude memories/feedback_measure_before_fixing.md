---
name: measure-before-fixing
description: "When a system is failing intermittently, surface diagnostics FIRST. Don't write code fixes from theories."
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 8a47d2cf-0516-4c75-ab36-e6a43da45e08
---

When a system fails intermittently or has unclear behavior, the first move is to add instrumentation that tells you WHY — not to write a fix based on a theory of what's wrong.

**Why:** In the PIPE comms session 2026-05-26, I attacked a "slot-phase tracker fix" derived from the prior session's summary's wrong theory. Two firmware changes; both broke CDR lock entirely (regression from ~50% lock rate to ~10%, plus a full BOOTSEL+UF2 cycle to recover from each). The actual diagnosis came from a 30-line change that surfaced already-existing `_edges_*` debug counters in fast_sync to the host. The data immediately showed the real failure mode (67% mid-slot rejection from CDR starvation + 28% wrong-polarity rejection from a Claim-12-incompatible filter) — neither matched my theory. Hours wasted, two flash cycles, code that got reverted.

**How to apply:** If a system has counters tracked locally but not surfaced (look for `_unused_*` Rust vars, `(void)` C casts, debug-only fields), wire them up before changing any logic. If counters don't exist, add them; that's a smaller and safer change than fixing the actual bug, and it gives you the data to fix the right thing on the next pass. Especially important when iteration is expensive (every flash cycle in PIPE costs a BOOTSEL+UF2 reflash because of the chronic load_dev race — see [[pipe-load-dev-race]] if I ever write that one). Related: [[pipe-comms-acceptance-test]] — without distinguishable test data (DEADBEEF vs symmetric AAAAAAAA), even instrumentation can lie.
