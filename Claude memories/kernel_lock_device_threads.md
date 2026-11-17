---
name: kernel-lock-device-threads
description: "2026-11-16 kernlock: diag lock / diag dev measure the kernel lock and each device line's interrupt to run; dev first/follow/core switches; what kills the camera's stream at 96 fps; the lock's offenders and what was fixed"
metadata:
  type: project
---

Branch worktree-agent-a2ab777f01ad0b437 (camtrials + forge 25a6c877), devsched.rs, KERNEL.md "Order" item 7, CAMERA.md "The kernel lock and the frame thread".

**Tools:** `diag lock` (every hold by length, by syscall with its x1 byte, by site, the last 2^8 holds past the long mark with the thread; `lock reset`, `lock long <us>`); `diag dev` (per granted line: interrupt to run mean/longest/histogram, landing/noticing/running cores, late waits, yields; lock waits inside interrupt runs and behind whom; `dev reset`, `dev freeze <intid> <us>` stops `diag sched`'s trace at the first late run). `sched reset` resets both. Holds of 2^-7 s or more go to the flash log ("lock held long").

**Switches (run time, off by default):** `dev first on` (an IRQ-woken thread with front credit is picked first; with no idle core a compute core gets SGI 3 and its thread keeps its place at the front), `dev follow on` (a line routed to its waiter's core; lines go home before workers CPU_OFF), `dev core <k>` (core k runs device threads only, i.e. threads that ever irq_waited).

**What killed the main's stream at 96 fps (fll 3200, 128 blank lines, about 400 us):** the next PDMA descriptor must be released inside the blanking, and the engine's program runs after the release. (1) The frame thread waited for a core (all switches off: 10/10 runs with the u16 add died, interrupt to run up to 5 ms behind the display's publish workers and the bench). (2) The driver wrote the index after its 56 acknowledgements (about 170 us); now first (12 us). (3) Kernel lock holds mid-stream: a keeper's pcie1 respawn (capsule read 24 ms + gate/build 7 ms), pcie1's power switch 19.5 ms x2. A dead stream reads image status G#1 (a frame begun), aggregate 0, the engine's index ours.

**Measured outcome (kernel #2, 2048 frames, u16 add):** off 0/2; first 3 thr 1/1, 4 thr 1/1 (it died 3/3 before the lock fixes); core 7 + first 4 thr 3/3; first + follow 4 thr 2/3 (a 7 ms spawn gate/build). Interrupt to run mean 24-30 us, longest 70-90 us with first/core. Pinned to the X3: dies in 11-19 frames (the X3 runs the service loop, render, ahau); pinned to core 7 with no switch dies in 5, with first+follow 1024 clean.

**Lock fixes made:** kvault caches the bound mark (init's 64/s identity poll held 300 us each); SYS_POWER and power_offs_apply run the audited switches (gps nfc bt touch amp torch) with the lock released and IRQs open (every switch, ACPM, the PMIC bus and the pd gating pass run on the kernel's core only); the camera's switch keeps the lock (its region fence writes the same S2MPUs from any core); spawn capsules cached (KVault get_capsule, 2^22 B) on kernels #2-#3, REMOVED on #4: #3 hung in SYS_SPAWN (core 6, lock held, IRQs masked, PREV_EXIT G#9A) right after attest, cause unproven; spawn_step crumbs (G#9A, step in bits 24-31) name the step now. Still under the lock: spawn's gate+build (about 7 ms), cam_ask (4.9 ms), a process end with the camera region (5.8 ms), SYS_BLOCK (to 1.2 ms), flash log lines (to 0.66 ms), a cold capsule read (18-60 ms).

**Readout:** the main's sensor reaches 116/s at its least line (about 5370 clocks), but the front end takes only lines of 5888 clocks or longer (101/s clean); at 5632 and below it takes one frame whatever the blanking. The Pro reset to Android during that sweep (b2 reload, an open after three stuck-front-end opens): cause not yet read.

**How to apply:** for a camera stream use `dev core 7` + `dev first` (or `first` alone, 3 consumer threads); don't pin the frame thread; read `diag dev` before blaming the driver. Related: [[smp-first-cut]], [[sched-trace-and-past-deadline-spin]], [[husky-camera-bringup]].
