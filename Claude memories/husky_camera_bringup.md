---
name: husky-camera-bringup
description: ferros powers husky's main camera from a cold boot and the sensor answers (model ID G#08EF, 2026-10-19) — diag cam bus/up/off; the five traps (ACPM, HS-I2C timeout + error IRQ enables, camera-PMIC chip select, voltages before enables); next is the CSI receive side
metadata:
  type: project
---

Working since 2026-10-19 on slot b, no Linux: `bridge diag "cam bus"`, `"cam up"`, `"cam off"`. Recipe and register detail in ferros/docs/oracle/CAMERA.md; code in ferros_kernel/src/camera.rs, acpm.rs, hsi2c.rs.

**Traps that cost the day (don't re-derive):**
- Sub-PMIC (S2MPG15) GPIOs/LDOs only via ACPM channel 2 (acpm.rs); camera PMIC chip select = sub GPIO0, must be HIGH (Android idle state) or the SLG51002 NACKs everything.
- HS-I2C from cold: TIMEOUT G#48 bit 31 is on out of reset and must be cleared; enable the error interrupt sources too or a NACK looks like a hang. The bootloader had pre-cleared both on hsi2c_11, which is why the battery bus never showed it.
- Camera-PMIC LDO cold voltages are WRONG for the camera (LDO1 2.85 V vs 2.25 V needed): program and verify VSEL before setting any enable bit. Linux does it at boot, so camera-open captures never show it.
- The "engine stuck in START" reading was wrong: TRANS_STATUS G#80001 is normal idle; diff against a working controller before theorizing ([[feedback_measure_before_fixing]]).

**Receive side (2026-10-19):** `diag cam fe`/`cam stream` bring up the ISP front end (pd_ispfe on cold; PMU status read direct, config/prepare via EL3), replay the sensor init from the vault (cam.boitata.init, 744 writes), bypass the D-PHY's PMU isolation (G#3EBC bit 0 — a PMU write the oracle can't see), and configure CSI link 1 (version reads G#5000, PHY lanes reach G#3). **Open:** sensor latches stream-on but no frames — master clock suspected (gpp9-0 func 2 reads steady low, CIS_CLK3 not reaching sensor XCLK). Census mode (FERROS_HV_ORACLE_CENSUS=1) found the 42 ISPFE pages a camera open uses. DMA (PDMA G#1C39_0000 behind 3 ISPFE S2MPUs) comes after frames flow.

**Next:** fix the sensor master clock, then the PDMA + S2MPUs for a first raw frame. Related: [[ira_entropy_sources_husky]] (camera calibration EEPROMs on these buses).
