# Camera, from the register oracle (2026-10-19)

Captured with the hv oracle (HV.md): the camera's control buses unmapped from the stage-2 map, every access trapped at EL2, emulated against the real register and logged. Decoded back to I2C transactions with `tools/oracle-decode-hsi2c.py`.

## The buses (husky, from the live device tree)

| sensor | role | I2C controller | addr | lens | EEPROM |
|---|---|---|---|---|---|
| boitata | main (wide) | hsi2c@10880000 | G#3D | act-cornerfolk G#0C, ois-djinn G#24 | djinn G#24 |
| dokkaebi | tele | hsi2c@10890000 | G#10 | G#0C | G#51 |
| oksoko | ultrawide | hsi2c@108A0000 | G#1A | G#0F | smaug G#50 |
| kraken | front | hsi2c@108B0000 | G#2D | — | humbaba G#61 |
| tof-tarasque | depth | hsi2c@10880000 | G#29 | — | — |
| flash-lm3644 | torch | hsi2c@10980000 | G#63 | — | — |

MIPI receiver (CSI) D-PHY: G#1C10_1000..G#1C10_3FFF, shared (`dcphy_m0s4s4s4s4s4_csi0`). The ISP/processing blocks (`lwis_*`, G#1C00_0000..G#1D8F_FFFF) are separate ioreg devices, unmapped the same way when that layer is captured.

## What the captures show

- **Main camera open, start to stop** (`camera-main-open-transfers.txt`, 1062 transfers between the `camera open` and `closed` markers; `camera-csi-dphy-writes.txt`, 54 PHY accesses). In order:
  1. **CSI D-PHY bring-up first** (index 36 of the combined stream): lane settings at G#…31C/41C/51C/81C across the lane blocks, the common block at G#1C10_1000..G#1C10_1014, then the clock and data lanes at G#1C10_1800 / G#1900 / G#1A00, ending with all three read back as 3 (ready).
  2. **Sensor setup over Samsung's indirect protocol**: page select by writing G#FCFC, an internal address through G#6028 (high) / G#602A (low), then data through G#6F12 (auto-increment).
  3. **The settings-file upload** (index 43): one 6521-byte G#6F12 burst into the sensor's RAM. Vendor firmware, so the committed file has its length and a blake2b-64 hash only; ferros loads it from the phone's own vendor partition at runtime, as Linux does.
  4. **~750 configuration writes** (mode, timing, output format, readout geometry), then a grouped-parameter hold (G#0104 = 1), the first exposure (coarse integration G#0202, analog gain G#0204, frame length G#0340), hold released (G#0104 = 0).
  5. **Stream on**: G#0100 = G#0103 (index 795).
  6. **Per-frame updates** while streaming: exposure writes bracketed by the G#0104 hold, with the lens actuator (G#0C) and OIS (G#24) polled alongside.
  7. **Stream off** at close: G#0100 = 0.
- **Boot-time probe** (`camera-main-boot-probe-transfers.txt`, 668 transfers at ~30 s uptime): the camera service touching the main module at boot — the OIS/EEPROM at G#24, the ToF at G#29, the lens, and a short burst to the sensor. Earlier mislabelled as the camera open; it is the probe, before any app runs.
- **Ultrawide calibration** (`camera-ultrawide-108a-transfers.txt`, 101 transfers): the module's calibration EEPROM at G#50 read in full at boot. Long reads are summarized (length + hash): module calibration can carry identifying data.

## The controller

Exynos USI HS-I2C in auto mode, the same block `ferros_kernel/src/hsi2c.rs` already drives for the PMIC (the oracle confirmed the register map on a second instance): ADDR G#70 holds the 7-bit target at [16:10], AUTO_CONF G#44 carries length [15:0] + READ (16) + STOP (17) + RUN (31), TX_DATA G#34 / RX_DATA G#38 are the FIFOs, and the driver polls STATUS/FIFO in a tight loop (most of the raw trap volume is that poll, filtered out by the decoder).

## Oracle tooling notes

- Targets, log cap, quiet addresses, and arming (idle gap or after a time) are build settings: `FERROS_HV_ORACLE_PAGES`, `_CAP`, `_QUIET`, `_ARM_GAP`, `_ARM_AFTER_MS`. A capture is a rebuild, not a code edit.
- **Most I2C trap volume is controller setup, not data.** About 26 register accesses per transfer, of which four carry the transaction (ADDR G#70, AUTO_CONF G#44, TX G#34, RX G#38). Quieting the other nine registers (G#00, 04, 08, 20, 24, 30, 40, 48, 50) took the whole open-to-close session under the cap.
- **Arm after boot, mark from Android.** The hv's counter tracks the kernel's printk clock closely, so `_ARM_AFTER_MS=60000` skips the boot probe. `echo "<3>ferros-mark: ..." > /dev/kmsg` lands in the same console ring as the hv's lines, so the decoder shows exactly where the app opened and closed.
- One chatty bus still starves a shared cap: capture one bus at a time.
- Identical consecutive accesses collapse into a `rep N` line (the decoder replays them). That did not help on this controller, whose poll alternates between registers; the quiet list is what worked.

## Capture recipe (main camera)

```
FERROS_HV_STAGE2=1 FERROS_HV_ORACLE=1 FERROS_HV_NO_PMU_READ=1 \
FERROS_HV_ORACLE_PAGES="G#1088_0000,G#1C10_1000-G#1C10_4000" FERROS_HV_ORACLE_CAP=16384 \
FERROS_HV_ORACLE_QUIET="G#1088_0000,G#1088_0004,G#1088_0008,G#1088_0020,G#1088_0024,G#1088_0030,G#1088_0040,G#1088_0048,G#1088_0050" \
FERROS_HV_ORACLE_ARM_AFTER_MS=60000 tools/husky-hv.sh --flash
# past 75 s uptime: mark, open the camera, mark, close, then flush the console with a panic:
adb shell 'su -c "echo \"<3>ferros-mark: camera open\" > /dev/kmsg"; am start -a android.media.action.STILL_IMAGE_CAMERA; sleep 8; input keyevent KEYCODE_HOME; su -c "echo \"<3>ferros-mark: closed\" > /dev/kmsg; echo c > /proc/sysrq-trigger"'
# after the reboot:
adb shell 'su -c "grep -aE \"ferros hv|ferros-mark\" /sys/fs/pstore/console-ramoops-0"' > capture.log
tools/oracle-decode-hsi2c.py capture.log --max-bytes 32
```
