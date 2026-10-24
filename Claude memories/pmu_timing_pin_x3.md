---
name: pmu-timing-pin-x3
description: "On husky, time EL0 code pinned to the core it runs on (pin, then sleep 1 ms to migrate); unpinned runs land on the A715s at 1.07 GHz and mislead by 3-4x"
metadata:
  node_type: memory
  type: project
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-24T20:53:55.015Z
---

2026-10-24: every display timing taken before pinning ran on the A715 workers (processes default there; the display pins itself to the X3 only later in startup), and the A715s sit at 1.07 GHz — the X3 runs at 2.85 GHz. The f16 ODT read 11.4 ns/px unpinned and 2.76 on the X3; the integer-vs-f32 ranking even changed. `ferros_sys::pin(mask)` only takes effect at the next reschedule: sleep ~1 ms after it. The PMU is open to EL0 on every core (ferros_kernel/src/pmu.rs, `ferros_sys::pmu`), but its counts are per core, so a migration mid-run gives nonsense (zeros, or a few cycles per 16 pixels). Cycles ÷ elapsed µs = the clock, which tells which core the run was on.

**Why:** a wrong core makes every comparison wrong.

**How to apply:** pin to the core the real code runs on (the present stage and scene: the X3, `CORES_BIG`; publish bands: the A715s too), sleep a millisecond, read cycles and instructions, report the clock alongside. Synthetic pans for frame pacing: FERROS_MAP_START=drag|crowd (the map logs passes, draws and frames on the panel). Related: [[feedback-hot-paths-branchless-bits]], [[smp-first-cut]].
