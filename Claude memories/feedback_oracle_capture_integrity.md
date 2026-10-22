---
name: feedback-oracle-capture-integrity
description: hv oracle captures through the pstore console ring can silently lose lines (hv and kernel append without a shared lock); quiet the kernel console during captures and verify every I2C transfer against its programmed AUTO_CONF length before replaying anything
metadata:
  type: feedback
---

Never replay an oracle capture without an integrity check. The hv's `pstore::log` appends to the console ring with a plain read-modify-write; the kernel appends under its own lock, which the hv cannot take. Under the kernel's normal level-7 console traffic, and the hv's per-second stats line from another core, lines collide and bytes vanish.

**Why:** on 2026-10-19 the main camera's sensor init was replayed with 7 bytes missing from its 6528-byte settings upload and 5 short writes garbled. The sensor latched stream-on but never produced frames, and a whole evening went to chasing the master clock for what was a corrupt capture.

**How to apply:** before a capture, `echo 4 > /proc/sys/kernel/printk` (markers written at `<3>` still print); oracle builds keep the stats line off; run `tools/oracle-decode-hsi2c.py` and read its integrity line. Any transfer flagged LENGTH MISMATCH inside the window being replayed means recapture. The I2C controller's AUTO_CONF length is the ground truth. Related: [[husky_camera_bringup]], [[feedback_measure_before_fixing]].

**One bus per capture, measured (2026-10-22).** Trapping two I2C controllers at once (camera PMIC bus + ultrawide sensor bus, both busy on different cores) gave LENGTH MISMATCH transfers twice in the same place, with no ring loss involved. The hv's repeat-collapsing state (`ORACLE_PREV`/`ORACLE_REP`) is shared across cores without a lock, so a `rep N` can land on the wrong access or vanish. Capture one controller per run. Also: the ultrawide's lens polling plus per-frame exposure fills a 36000-line cap in about 8 s; the sensor init is in the first ~1500 transfers.
