# Camera, from the register oracle (2026-10-19)

Captured with the hv oracle (HV.md): the camera's control buses unmapped from the stage-2 map, every access trapped at EL2, emulated against the real register and logged. Decoded back to I2C transactions with `tools/oracle-decode-hsi2c.py`.

## The buses (husky, from the live device tree)

| sensor | role | I2C controller | addr | lens | EEPROM |
|---|---|---|---|---|---|
| boitata | main (wide) | hsi2c@10880000 | G#3D | act-cornerfolk G#0C, ois-djinn G#24 | djinn G#24 |
| dokkaebi | tele | hsi2c@10890000 | G#10 | G#0C | G#51 |
| oksoko | ultrawide | hsi2c@108A0000 | G#1A | G#0F | smaug G#50 |
| kraken | — | hsi2c@108B0000 | G#2D | — | humbaba G#61 |
| tof-tarasque | depth | hsi2c@10880000 | G#29 | — | — |
| flash-lm3644 | torch | hsi2c@10980000 | G#63 | — | — |

MIPI receiver (CSI) D-PHY: G#1C10_1000..G#1C10_3FFF, shared (`dcphy_m0s4s4s4s4s4_csi0`). The ISP/processing blocks (`lwis_*`, G#1C00_0000..G#1D8F_FFFF) are separate ioreg devices, unmapped the same way when that layer is captured.

## What the capture shows

- **Main camera open** (`camera-i2c-transfers.txt`, 668 transfers): the HAL brings up the whole wide-camera module on one bus — sensor G#3D (register writes to stream config, 2-byte reg + value), lens actuator G#0C (focus steps), OIS G#24, and a ToF read at G#29. The sensor's register protocol is 16-bit register address, big-endian, then the value; reads write the register then read back. That is the init sequence the ferros sensor driver replays.
- **Ultrawide** (`camera-ultrawide-108a-transfers.txt`, 101 transfers): the calibration EEPROM at G#50 read in full — the per-module tuning blob the ISP needs.

## The controller

Exynos USI HS-I2C in auto mode, the same block `ferros_kernel/src/hsi2c.rs` already drives for the PMIC (the oracle confirmed the register map on a second instance): ADDR G#70 holds the 7-bit target at [16:10], AUTO_CONF G#44 carries length [15:0] + READ (16) + STOP (17) + RUN (31), TX_DATA G#34 / RX_DATA G#38 are the FIFOs, and the driver polls STATUS/FIFO in a tight loop (most of the raw trap volume is that poll, filtered out by the decoder).

## Oracle tooling notes

- Targets, log cap and an idle-arming gap are build settings (`FERROS_HV_ORACLE_PAGES`, `_CAP`, `_ARM_GAP`), so a capture is a rebuild, not a code edit.
- **One chatty bus starves a shared cap.** Capturing four buses at once, the ultrawide's EEPROM poll filled all 16384 log slots before the others were touched. Capture one bus (plus the CSI PHY) at a time, or add a per-page cap. The idle-arm gap also fired on a boot-time burst, not the camera-open; arming is better driven by the event than by a timer.
- The CSI PHY pages saw no traffic in these windows: the PHY is touched at stream-on, which needs a capture armed at that moment (future refinement).
