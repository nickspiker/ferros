---
name: sensor-mailboxes
description: Owner 2026-11-19 - sensor drivers (GPS, accelerometer, mic, ...) publish into a capability-gated, read-only, public mailbox any number of readers map; no per-reader calls into the driver
metadata:
  type: project
---

Owner (2026-11-19), after the GPS driver ran out of WATCH slots with six watchers: "the driver for GPS should have a capability based public read only mailbox so you could have an unlimited number of items accessing it. same with accelerometer, mic, etc."

**The shape:**
- One writer: the driver.
- Any number of readers, each mapping the mailbox read-only thru a capability.
- Readers wait on its sequence word (a futex or doorbell, never polling) and read without calling the driver.
- The driver never knows or limits how many read.

**Why:** a call-per-reader endpoint caps readers (the GPS driver's 4, then 16 WATCH slots) and turns a busy reader set into driver load. A published mailbox makes reading free for the driver and unlimited by design.

**How to apply:**
- New sensor or measurement drivers publish this way. Existing ones (GPS fixes and tracks, sensorlink's accelerometer, gyro and others, the mic) move to it.
- The capability is the permission: implicit rights stay implicit ([[rights_implicit_set]]), while location and the mic are asked once ([[permissions_asked_once]]).
- Related: [[gps_onchip_fix]], [[kernel_out_moves]].

**Built 2026-11-19 (branch mailboxes; KERNEL.md "Mailboxes", SENSORS.md "The motion mailbox"), not yet run on a device:**
- A mailbox is a DMA region pair over one pool frame kept for the boot: `<name>-mailbox` read-write (the writer's), `<name>-view` read-only (EL0 and EL2).
- SYS_DMA maps it (the writer's region makes the one writer); SYS_WAIT on the sequence word waits; the writer's SYS_WAKE on it wakes every process's waiters; SYS_MAILBOX (47) op 0 opens a view with a proof (GPS: Settings, the `gps` right, a GPS view; motion: none yet, as public as SYS_FACTS).
- The kernel marks it dead and zeroes it once the writer's process is gone (where the arc is freed, all three paths).
- ferros_mailbox: the seqlock and the layouts, host-tested; ferros_sys::mailbox: Writer and Reader; payloads/mbtest measures (64 readers).
- Sensors publish `motion` (gravity, the field, the accelerometer every 2^4 ms).
- The GPS move (driver into slot 10, map, GPS puck and ferros_puck `gps_fix` reading, WATCH and SYS_CLAIM op 3 removed) was written in commit 93e7d55e and taken back out on the coordinator's word: it is staged after gps-indoor-lock merges. The kernel grants the driver nothing new yet: `gps-mailbox` joins `gps_grants` with the move.
- Open for the owner: whether motion is an asked right; the mic (a PCM ring, the `mic` right) once the audio service captures.
