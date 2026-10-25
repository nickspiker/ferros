---
name: husky-hardware-night-2026-10-25
description: The overnight bring-up of 2026-10-24/25 — sensors (39/41), haptics, GNSS, modem boot channel, GPU, TPU firmware — where each stands and its doc; the traps found
metadata:
  type: project
---

Overnight 2026-10-24/25 the owner gave the phone for "all the hardware". Where each stands (docs in the repo hold the detail):
- Sensors: 39/41 AoC sensors stream (SENSORS.md); front TMD3719 needs panel TE2 + a DisplayInfo (type 1001) message; prox wake built, off (`device.prox_wake`), to tune with the owner.
- Haptics CS40L26 answers (hsi2c_0 G#43, rev A1; hibernates until a reset); `diag haptic play <n>` not yet felt by the owner.
- GNSS BCM4776 alive on spi_21 (GNSS.md); next = strace capture of lhd for the patch download.
- Modem: boot channel through TOC/MAIN/VSS/APM (the missing A11B TOC frame); deliberately stops before NV + A400 — never start the CP without its own NV (no IMEI/calibration on air).
- GPU Mali-G715 answers (GPU.md); CSF plan written; project order puts GPU after toka.
- TPU firmware runs (TPU.md): GSA cmd 50 load, Trusty START, KCI EXCHANGE_INFO answered — but only after the TPU LPM bring-up (ABL leaves pd_tpu on, LPM idle).

Traps worth remembering: ABL leaves sensor rails AND GNSS rails programmed-but-off; flashlog writes need the kernel lock (raced vault reads → touch spawn failed); kvault pets watchdogs so `reboot fastboot` needs RESET_ORDERED; Trusty connect must remember GO_ONLINE for a second port; the AoC log (`diag aoc log`) names what a sensor waits for. Related: [[aoc-start-needs-trusty]], [[husky-cold-slot-b]].
