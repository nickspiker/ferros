# ferros — release checklist

What must be true before a release build ships. An item comes off this list only when it is done and verified on the device, or when the owner strikes it. Gates recorded in other documents are linked from here, so this file is the one place to look.

## Open

- **The process watchdog enforces (owner, 2026-11-05: "keep in measure mode but make sure it's on the release checklist").**
  - **Today:** drivers pet once a pass of their main loop (`ferros_sys::pet`), and the kernel records each process's longest stretch between pets (`diag pets`). Nothing is enforced.
  - **What counts:** the process's own running, its waits on hardware it drives, and its timed sleeps.
  - **Paused:** waits on the world (traffic, credentials, a person, the network), calls blocked on another process (that process's own watchdog answers for it), and the time before the first pet (startup).
  - **Before release:**
    1. Build the pauses.
    2. Measure a day of normal use.
    3. The owner sets the window from those numbers (one for every process, like the hardware's 1 s).
    4. A miss kills the process, init's keeper restarts it, and the console says who missed and by how much.
    5. The display's own quarter-second frame guard and its half-millisecond wake go once this is enforcing.
- **Counsel before release** ([APK-LEGAL.md](APK-LEGAL.md) O7): before ferros runs banking or government identity apps.
- **APK support** (owner): no ship without APKs, compiled on the device at install and sealed to the hardware.
- **A production build** (`FERROS_PRODUCTION=1`):
  - It compiles every development door out ([KERNEL-OUT.md](KERNEL-OUT.md), [BOOT.md](BOOT.md) "The USB development doors close in a production build").
  - It refuses native capsules ([SANDBOX.md](SANDBOX.md)).
  - Its "Ready?" gate waits for the press with no timer. Whether it boots at once instead is undecided (owner, 2026-11-05).
- **The device lock** (owner): prepared freely, run only for the release, never before.

## Done

- **The hardware watchdog across the jump** (2026-11-05): measured 323 ms and 291 ms from the hv's re-arm to ferros's own arm. The hv now re-arms at the kernel's 1 s (was ~87 s), verified on a chord ([POWER.md](POWER.md)).
