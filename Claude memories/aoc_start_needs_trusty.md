---
name: aoc-start-needs-trusty
description: husky sensors are all behind the AoC; no-TEE exhausted — AP cannot reach the AoC's buses (firewalled), but GSA's non-secure mailbox loads AND starts the AoC without Trusty; not yet online — Linux also enables the AoC SYSMMU first
metadata:
  type: project
---

Every husky sensor (IMU, prox/ALS, IR thermometer, mag, pressure, spectral) sits on the AoC's own I2C/I3C buses and is served by Google's AoC firmware through USF (FlatBuffers on /dev/acd-com.google.usf). Owner chose "no TEE, direct if possible" on 2026-10-23.

**Decisive 2026-10-23: both no-TEE routes are exhausted.** (1) Direct buses: inside the AoC block the AP reads only the DesignWare GPIO's pin levels; controllers, SRAM, PCU, pin mux abort, EL3's privileged read refuses — firewalled. (2) Firmware route: GSA's NS mailbox (G#1649_0000) loads+authenticates the vault's aoc.bin (cmd 90) in 30 ms and ferros enables the AoC SYSMMU from the firmware's own IOMMU table (no fault), but the firmware never comes online. The oracle proves why: Linux sends mailbox cmd 90 once and NEVER cmd 91 — it starts the AoC over Trusty (com.android.trusty.gsa.hwmgr.aoc), not the mailbox. ferros's mailbox cmd 91 is accepted (state loaded→4, release→2) but does not run the firmware. So starting the AoC genuinely needs a Trusty client (FF-A + trusty-virtio + tipc).

**How to apply:** full detail in ferros SENSORS.md ("The direct route, measured", "Starting the AoC through GSA's mailbox"); oracle log local at /mnt/Harbor/tmp/aoc-oracle-2026-10-23. Root on husky = Magisk in slot b's init_boot: never leave slot a active after a capture. A second phone (Pixel 3a) shares the dev host's adb — address the husky by serial ([[husky_cold_slot_b]]).
