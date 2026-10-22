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

**Receive side (2026-10-19):** `diag cam fe`/`cam stream` bring up the ISP front end (pd_ispfe on cold; PMU status read direct, config/prepare via EL3), replay the sensor init from the vault (cam.boitata.init, 744 writes), bypass the D-PHY's PMU isolation (G#3EBC bit 0 — a PMU write the oracle can't see), and configure CSI link 1 (version reads G#5000, PHY lanes reach G#3). **Streaming works (2026-10-19):** with a byte-exact init (the first was corrupt, see [[feedback_oracle_capture_integrity]]) the sensor's frame counter runs ~14 fps and CSI link 1 latches its interrupt sources and fills its counter words at G#634-G#6C4. The master clock was never the problem (its pad reads low even while running). Census mode (FERROS_HV_ORACLE_CENSUS=1) found the 42 ISPFE pages a camera open uses. DMA (PDMA G#1C39_0000 behind 3 ISPFE S2MPUs) comes after frames flow.

**Next:** PDMA + the three ISPFE S2MPUs/SYSMMUs for a first raw frame in memory. Related: [[ira_entropy_sources_husky]] (camera calibration EEPROMs on these buses).

## 2026-10-21: the clock no capture could show

**The hv snapshots Linux's live registers.** `FERROS_HV_SNAPSHOT=1` + `FERROS_HV_SNAP_LIST` reads 2914 known-to-exist registers sixteen frames into a Linux stream (`tools/camera-snap-list.py` builds the list and turns the log into `camera_snap.rs`); `diag cam pdma` reads the same list cold and prints every difference. This is the tool for "everything we capture matches, it still fails": it sees state set by firmware, which no trap does. Recipe in CAMERA.md.

**Found with it:** CMU_TOP `ISPFE_NOC` (mux G#2604_10BC, div G#2604_18B4) is 465 MHz under Linux (PLL_SHARED1_D4 ÷1) and 111 MHz cold (SHARED3_D1 ÷6). Linux sets it through ACPM DVFS (INT domain), never a CPU write. `fe_up` now sets it, mux first, and the link's deskew overflow vanished. **Not sufficient:** the core still sees no frame start. The front-end diffs left over (G#8F where Linux has G#10, `+G#1C`/`+G#20` = 1 where Linux has 0) are pending requests the hardware consumes at frame start: symptoms.

**Also ruled out:** the wide census (whole ISP span plus CMU_TOP plus every EL3 call) shows no unseen front-end page (only its SYSMMUs/S2MPUs) and no EL3 calls during a camera open. The PDMA builder's IOVA window missed four address registers (fixed, `ADDR_REGS`), and that wasn't it either.

**Process trap:** with several adb devices attached (other phones on the LAN), `tools/husky-hv.sh --flash` dies at `adb reboot bootloader`. Use `ANDROID_SERIAL` from Android, or go `bridge reboot fastboot` + fastboot by hand from ferros.
