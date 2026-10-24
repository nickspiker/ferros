---
name: aoc-start-needs-trusty
description: husky sensors are all behind the AoC; the AP cannot reach the AoC's buses (firewalled), but GSA's non-secure mailbox loads AND starts the AoC without Trusty; not yet online — Linux also enables the AoC SYSMMU first
metadata:
  type: project
---

Every husky sensor (IMU, prox/ALS, IR thermometer, mag, pressure, spectral) sits on the AoC's own I2C/I3C buses and is served by Google's AoC firmware through USF (FlatBuffers on /dev/acd-com.google.usf). Owner chose "no TEE, direct if possible" on 2026-10-23.

**Measured 2026-10-23:** the direct route is closed — inside the AoC block the AP reads only the DesignWare GPIO's pin levels; controllers, SRAM, PCU and pin mux abort, EL3's privileged read refuses. But GSA's NS mailbox (G#1649_0000) takes AoC command 91 on this firmware, so no Trusty client is needed: load (90) authenticates the vault's aoc.bin in 30 ms, start → state 4, release A32 (5) → state 2 running. The firmware still doesn't come online (parameter block never replaced).

**Why:** the hv oracle of Android's bring-up shows Linux enabling the AoC SYSMMU (19090000, v9) with its own page table before the load; ferros leaves it off. That is the next step; what the NS table must map is open.

**How to apply:** full detail in ferros SENSORS.md ("The direct route, measured", "Starting the AoC through GSA's mailbox"); oracle log local at /mnt/Harbor/tmp/aoc-oracle-2026-10-23. Root on husky = Magisk in slot b's init_boot: never leave slot a active after a capture. A second phone (Pixel 3a) shares the dev host's adb — address the husky by serial ([[husky_cold_slot_b]]).
