---
name: smp-first-cut
description: ferros runs on 5 cores since 2026-10-23 (X3 kernel + render, 4× A715 workers); one kernel lock, global run queue, sev not SGIs, per-thread core masks (pin); DIAG SMP_UP/SLICES_C*/LOCK_SPINS, diag threads shows masks; FERROS_SMP=0 escape; the TLBI-is and per-core kctx lessons
metadata:
  type: project
---

Since 2026-10-23 the kernel brings up the four Cortex-A715s (MPIDR Aff1 4-7) as workers from the X3 (`ferros_kernel/src/smp.rs`): hand-over blocks in the result page at RESULT+G#200, idle loop = run_slice or wfe, every enqueue does sev. One kernel lock (process.rs `lock()`), one global run queue, per-thread core masks: processes default to the workers (G#F0), the display's render thread pins to the X3 (G#100) via `ferros_sys::pin`. A RELOAD CPU_OFFs the workers first (`smp::stop_all`). `FERROS_SMP=0` builds stay single-core.

**Why:** the drag at 114 fps needs the render thread on the X3 (unpinned it wandered to A715s: 70-81 fps); everything else on the workers so a call hands off on the caller's own core (4 µs wait, 7 µs server).

**How to apply:** DIAG `SMP_UP` (bitmask of workers), `SMP_ON_k` (CPU_ON results), `SLICES_C4..C8`, `LOCK_SPINS`; `diag threads` shows each thread's core mask; `diag sched` has a core column (cN). Any pool page-table change must use `tlbi vae1is` (inner shareable) — a core-local invalidate left a worker fetching through a stale non-exec entry (map faulted at its entry). Anything saved per core in asm (`__el0_kctx`) must be indexed by MPIDR Aff1. Next: tiled rendering across workers; the A510s for the waiting servers; per-core queues only if the trace shows lock contention mattering.

**Later 2026-10-23:** the 7,000 spins/s were workers spinning on a fruitless look (RUNNABLE_N counted the X3-pinned render thread); fixed (sleep after a fruitless look; `kick(mask)`). WFI idle is written behind `FERROS_IDLE_WFI=1` but SHELVED: non-deterministic reload hangs, post-mortem notes kept getting overwritten (ramoops page: gate; X3 result page: a log; G#80F0_0000: the 17 MB gate image; G#C1FF_0000: modem firmware — APP_SURFACE overlaps cp_rmem, move it before cellular). Notes at G#B000_0000 now. `bridge reboot fastboot` stopped taking late in the day (3 failures in a row) — go via `diag android` + `adb reboot bootloader`, or hands.
