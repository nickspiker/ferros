---
name: link-measures-no-bars
description: Corner circles show each link's own measures (tagged words, SYS_STATUS 27, facts 33-80), never bars or one number; indicators up top, navigation bottom; radio flavours; owner gave design latitude 2026-11-03
metadata:
  type: project
---

Owner 2026-11-03: "DO NOT copy signal bars", "many dimensions of signal, frequency, bandwidth, speed, latency, flavor, packet loss ... RSSI, VSWR", "all indicators up top and navigation bottom", and "whatever feels good bub ... throw some of that in the design" (latitude on corners and flavours).

Built 2026-11-03 (WINDOWS.md "Link measures", "Flavours"):
- `ferros_sys::measure`: a word = kind (bits 48-63) | signed value in its unit (milli-dBm, milli-dB, kHz, bit/s, ns, ppm, VSWR thousandths); all ones empty. A new measure is a new kind, not a new layout. 8 slots per link: modem, pcie1, bt, nfc, gnss, usb (by process name, no capability). Kernel stores, reads none. FACT_COUNT 88.
- Battery circle from facts 1-4 (kernel's gauge poll): %, V, mA, W, °C.
- Modem signal: G#0906 is 202 bytes, LteSignalStrength at word 15 from offset 2 (ASU, RSRP, RSRQ, RS-SNR tenths, CQI, TA). **Inferred, not checked against Android yet**; range-checked per HAL bounds.
- VSWR has a kind; no driver measures it.

**Why:** a link is not linear, not five levels; the owner wants users never "behind bars".
**How to apply:** new link info = a new measure kind from the driver; never a level, never a bar glyph.

Related: [[windows-design-pending]], [[feedback-vsf-rgb-only]].
