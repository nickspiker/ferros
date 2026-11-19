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
