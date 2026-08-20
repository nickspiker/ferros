---
name: pmic-glink and battery status
description: GLINK to ADSP is dead (ABL boot path doesn't start GLINK), but battery voltage readable via direct SPMI QG register
type: project
---

## GLINK to ADSP: DEAD END (2026-03-20)

ADSP never starts GLINK transport after ABL handoff. Confirmed across multiple boots:
- SMEM items 478/479 exist but ADSP tx_head stays 0 — never writes VERSION
- SMP2P item 443 allocated, stop bit cleared, SMP2P doorbell (signal 2) sent — no effect
- ADSP never creates SMP2P item 429 (inbound) — doesn't know about SMP2P
- (7,2) APSS↔ADSP partition is empty
- IPCC RECV registers (0x10, 0x14, 0x1C) are TZ-protected, crash from EL1

**Root cause:** ABL uses a different ADSP boot flow than Linux remoteproc. The ADSP
firmware doesn't start GLINK without the full Linux PIL/remoteproc boot sequence
(firmware load, MBA auth, PIL handshake). ABL's charger_pd runs only during ABL phase.

**How to apply:** Don't spend more time on GLINK/SMP2P. Charging works autonomously
(ADSP coprocessor handles it). For battery monitoring, use direct SPMI reads.

## Battery Voltage: WORKING via Direct SPMI

QG fuel gauge at SID 8, PID G#C8, registers G#50-G#51:
- u16 LE raw ADC code, conversion: `raw * 625 / 256` → millivolts
- Example: raw G#63E (1598) → G#F3D (3901) mV ≈ 3.9V
- Consistent across boots, plausible for Li-ion at ~75-80%
- `probe_battery_qg()` in pmic_glink.rs reads voltage + SDAM + charger status
- Conversion factor (2.44 mV/code) NOT yet verified against Android

SDAM at SID 8, PID G#70, offset G#40: calibration data only (ESR deltas),
not live battery state. Valid byte = 2.

Charger status bits (SID 8): C9[0x09]=USB RT, C7[0x09]=charger RT, CB[0x07]=MISC.
