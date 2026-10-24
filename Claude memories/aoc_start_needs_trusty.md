---
name: aoc-start-needs-trusty
description: husky sensors are all behind the AoC; GSA mailbox loads aoc.bin but only Trusty (TIPC port com.android.trusty.gsa.hwmgr.aoc) takes the AoC out of reset; USF traffic capturable by kprobes from Android
metadata:
  type: project
---

Every husky sensor (IMU, prox/ALS, IR thermometer, mag, pressure) is served by the AoC (always-on hub) through USF, Google's FlatBuffers protocol on /dev/acd-com.google.usf. Found 2026-10-23, written up in ferros SENSORS.md.

**Why it matters:** starting the AoC from cold ferros needs a Trusty client in the kernel. GSA's non-secure mailbox loads/authenticates aoc.bin, but GSA_AOC_START goes over Trusty IPC (FF-A + trusty-virtio + tipc; Linux's driver ~7000 lines in /mnt/Harbor/husky-kernel/private/google-modules/trusty). `diag ffa` shows the secure world answers ferros at EL2 (FF-A 1.1, partition 0). The alternative (AoC in reset, AP drives the AoC's own buses) is unmeasured.

**How to apply:** the route choice (Trusty client vs direct buses) was put to the owner on 2026-10-23; check SENSORS.md for the decision before building either. USF capture = two kprobes on the handoff kernel (SENSORS.md "Capturing USF"), decode with tools/usf-decode.py; captures stay in /mnt/Harbor/tmp/usf-captures-*. Another phone (a Pixel 3a) shares the dev host's adb: always address the husky by serial ([[husky_cold_slot_b]]).
