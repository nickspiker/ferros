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

## Power-up (captured 2026-10-19)

Order, rails, voltages and delays come from the device tree (`module_power_seq@0`); register traffic from the oracle during a marked camera open:
- `camera-main-powerup-pmic-transfers.txt`: the camera PMIC (G#75) and flash driver (G#63) on hsi2c@10980000, 108 transfers.
- `camera-main-powerup-cmu.txt`: CMU_TOP accesses for the master clock CIS_CLK3 (mux G#2604_101C, divider G#2604_181C, gate G#2604_2044, clock-gating control G#2604_3010).
- The sensor enable line is sub-PMIC GPIO2 over ACPM channel 2 (GPIO sub-block G#0C, register G#0A); LDO12 and LDO26 are sub-PMIC rails on the same channel.

## ferros brings the main camera up cold (2026-10-19)

`diag cam bus`, `diag cam up`, `diag cam off` (ferros_kernel/src/camera.rs, acpm.rs, hsi2c.rs), on slot b with no Linux: the main sensor answers with model ID G#08EF, the value Linux reads. What it took, in order:

1. **ACPM client** for the sub-PMIC (acpm.rs): its GPIOs and LDOs are reached only through the power-management firmware's channel 2. Worked first try.
2. **Both camera HS-I2C controllers from cold**: pins muxed (PMIC bus gpp16-0/1 function 2; sensor bus gpp2-0/1 function 3 with pull-ups), USI in I2C mode, controller reset and timed, on the 24.576 MHz oscillator. Two traps: the transfer **timeout enable comes out of reset on** (G#48 bit 31; the kernel clears it every message), and the **error interrupt sources must be enabled** or a no-acknowledge never shows and looks like a hang.
3. **Camera PMIC chip select (sub GPIO0) high** — Android keeps it high whenever the camera is closed; without it the chip does not acknowledge. Then buck-boost (sub GPIO1) and buck (sub GPIO3) enables with 2 ms each. The driver's tuning step is empty on this chip revision (enter, then exit, nothing between).
4. **Program the rail voltages before enabling anything** — the chip's cold defaults are not the camera's: LDO1 G#A5 = 2.85 V where the camera needs 2.25 V. Codes, V = base + code × step: LDO1 G#69 (2.25 V), LDO4 G#AF (2.95 V), LDO5 G#AA (2.90 V), LDO8 G#72 (0.97 V); each written and read back while every rail is off. Linux sets these at boot, which is why a camera-open capture never shows them.
5. **The device tree's power sequence** (`module_power_seq@0`): LDO1, 500 µs; sensor enable (sub GPIO2); LDO8, 750 µs; sub LDO26 and LDO12 (bit 7 of their control registers; voltages already programmed); LDO4, 1 ms; master clock pin gpp9-0 function 2 (CIS_CLK3 already runs cold); ToF reset high; LDO5, 1 ms; the GPIO4 load switch (bit 0 of G#1713), 1 ms; then the sensor reset (gpp2-2) released and 9 ms. Camera-PMIC LDO enables are one bit each in G#110D (LDO1 bit 0 .. LDO8 bit 7).
6. **Sensor bus up again, read G#0000** → G#08EF.

`diag cam off` reverses the sequence and drops the supply enables; chip select stays high as Android leaves it.

Next: the receive side (the CSI receiver and its DMA in the ISP front end, behind three S2MPUs), then the sensor's init sequence and a first raw frame.

## The receive side, cold (2026-10-19)

`diag cam fe`, `diag cam stream` (camera.rs, camera_rx.rs). Built from a census capture: a camera open touches 42 of the ISP front end's pages (`FERROS_HV_ORACLE_CENSUS=1` logs the first access to each). What works cold:

- **ISP front-end power domain** (pd_ispfe): already on at cold boot. PMU status is a plain read (EL3 refuses a read request for it, returns -22); the config write and the domain prepare call (SMC G#82000410) go through EL3, as Linux's CAL engine does. CMU_ISPFE dividers, bus-clock mux, control word and every Q-channel request restored from the capture; SYSREG_ISPFE set. **CSI link 1 then reads version G#5000, its value under Linux.**
- **Sensor init replayed from the vault** (`cam.boitata.init`, 744 writes incl. the 6.5 KB settings upload, generated from the capture, kept off the repo): the sensor latches stream-on, `mode_select` G#0100 reads back G#0103.
- **The combo D-PHY's PMU isolation must be bypassed** (PMU G#3EBC bit 0, `__set_phy_isolation`): a PMU write the oracle never sees. Without it the PHY's per-lane registers read 0 after being written; with it the lane status words reach G#3 (Linux's ready value).
- **Receiver configured** (camera_rx.rs: the D-PHY common/lane settings, CSI link 1's virtual-channel/data-type/lane setup and start G#205E01, the CSIS reset that releases link 1), all from the capture.

**Correction (same evening): the first replayed init was corrupt.** The hv and the kernel both append to the pstore console ring and cannot lock against each other; under the kernel's level-7 console traffic (and the hv's stats line from another core) captured lines were occasionally clobbered. The sensor upload was programmed as 6528 bytes and captured as 6521, and five short writes came out with wrong lengths. Captures are now taken with `echo 4 > /proc/sys/kernel/printk` (markers at level 3 still print) and the stats line off in oracle builds, and `tools/oracle-decode-hsi2c.py` checks every transfer against its programmed length. The recapture is byte-exact up to stream-on (1347 writes, 2 reads). The no-frames result below was taken with the corrupt init and has to be re-run.

**Streaming works (2026-10-19, with the clean init).** `diag cam stream` from a cold boot: the sensor's frame counter (G#0005) runs 01, 02, 03 … 0E over one second (about 14 frames per second), with G#0006 = G#03 as under Linux, and CSI link 1 latches its interrupt sources (G#14 = G#100, G#1C = 1) and fills its per-channel status and counter words at G#634–G#6C4. The master-clock pad samples low even while the clock runs, so pad readback in a clock-output function says nothing; the clock was never the problem.

**Superseded (corrupt init):** The sensor accepts stream-on but its frame counter (G#0005) does not advance and link 1's status registers stay 0. The master-clock pin is the suspect: gpp9-0 set to function 2 reads a steady low (sampled high 0/128), so the 26 MHz CIS_CLK3 is not reaching the sensor's XCLK even though the clock node runs. (Superseded.) Was: confirm CIS_CLK3's gate/divider actually output (Android's clk_summary shows its real rate and enable count), and whether the sensor's XCLK is that pin or a PMIC clock-buffer output. Only once the sensor streams does the DMA (PDMA at G#1C39_0000, behind the three ISPFE S2MPUs) become worth wiring.

**Note:** a camera open run *under the 42-page oracle hv* ended in an SError to the TPU power domain (`BOOKER CPU8 to TPU`), unrelated to the cold camera code; the camera stack reaches into the TPU/other domains in ways the ISP-only page list did not cover. Cold ferros touches only what it powers.

**Next:** the DMA. Frames reach link 1; the ISP front end's PDMA (G#1C39_0000, behind the three ISPFE S2MPUs at G#1C14/18/1C_0000 and SYSMMUs at G#1C11/15/19_0000) writes them to memory. In ferros: open the S2MPUs and put the SYSMMUs in bypass (as `open_usb_dma_path` does for USB), point the PDMA at a physical buffer, and replay the front end's DMA configuration from the 42-page capture (re-taken with the integrity rules). First target: one raw frame, read back over PT.

## Where the DMA programming lives (2026-10-19, late)

The front end's frame-writer configuration (buffer addresses, formats, strides) is not written by the CPU. The camera HAL builds lists of register settings in memory and starts the PDMA (G#1C39_0000) on them: the CPU only writes the PDMA's base (an IO virtual address, different every session: G#1D53_A000, G#1D7C_F000, G#1CC2_9000), count (G#3E8) and start bit. A bus master's writes are invisible to stage 2, so the register oracle cannot see that configuration. Tried and stopped: reading the command lists from EL2 through the front end's SYSMMU (`FERROS_HV_PDMA_DUMP`, left in behind its knob, not working): context 0 of s0 is enabled but reports a page-table base of 0, and reading s1's per-context registers hangs the core, so the walk never completes. LWIS is open source but only a passthrough; the values come from the proprietary Pixel camera HAL, for which there is no source. Next route to decide: read back the front end's DMA registers while streaming (after the PDMA has applied them), limited to registers known to exist, or reverse the frame-writer register layout from the CSI link side.

**ISPFE SYSMMU page table found (2026-10-20).** Trapping the three front-end SYSMMUs (G#1C11/15/19_0000, 16 pages each) and the PDMA page through a camera open shows Linux writing the same table base to all three, VM 0 only: REG_MMU_CONTEXT0_CFG_FLPT_BASE_VM at G#8404 = G#98480 (table at PA G#9848_0000, base >> 12, samsung-iommu-v9.h), then CTRL_VM G#8000 = G#25. The parked dump read the base from G#800C, which is why it saw 0; it now reads G#8404. Each session the PDMA gets a fresh base IOVA (G#1D69_4000, G#1C97_D000), count G#3E8, start = 1; the full register order is PDMA +G#2C=1, +G#40=G#1F, +G#20=0, +4 base, +8=0, +C count, +G#10=0, +0=1 (start), +G#34=0, +G#28=1, +G#10=G#10/G#20.
Next: the dump capture (`FERROS_HV_PDMA_DUMP=1`, only the PDMA page trapped, armed after 60 s) to read the command lists through that table.

**The front end's register-write list, decoded (2026-10-20).** With the FLPT fix the hv walk works, and the PDMA turns out to be a descriptor engine: its base IOVA points at a ring of 16-byte descriptors {flags G#C003, list IOVA, 0, length}, filled by the time the session stops (the dump at start reads zeros), rotating over three lists. The first list (G#19DC bytes) is a register-write program in records of one header word (count of following words = w >> 14, opcode = w & G#3FFF): opcode 2 = (address, value) pairs, opcode 1 = a burst from a start address, opcode 9 = {IOVA, 0, length, register} block load from memory, opcode G#B = a marker. All 1655 words parse with no unknown records: 1447 register writes and 12 block loads (LUTs into G#1C36_9xxx..G#1C36_Cxxx).
Addresses are offsets into the ISPFE block at G#1C30_0000 (zuma-isp.dtsi `lwis_isp_fe`: ISPFE 1 MB, CSIS link + PHY G#1C04_0000, SYSREG_ISPFE G#1C02_0000, PDMA G#1C39_0000). Pages written: ISPFE +G#2_0000/+G#2_2000 (two channels carrying output buffer IOVAs at +G#2_0510 / +G#2_2110), +G#3_7000, +G#3_B000, +G#4_0000, +G#5_0000/+G#5_1000, and +G#5_4000/+G#5_5000 (1280 writes, with more buffer IOVAs at +G#5_48A0..+G#5_5200).
Decoded list: `camera-ispfe-pdma-writes.txt` (`tools/oracle-decode-pdma.py`). Capture: `FERROS_HV_PDMA_DUMP=1`, only the PDMA page trapped, armed after 60 s, camera opened from the launcher intent; the dump runs at the PDMA's start and stop writes, cleans each line to DRAM first, and continues past an unmapped list page.
**Block loads captured too (same day):** the hv walks the list and dumps each load's source; triggering mid-stream (descriptor index PDMA + G#10 reaching G#100) avoids a race at stop, where the HAL had sometimes already unmapped the list. `camera-ispfe-pdma-writes.txt` is now list 2 of that capture with its 12 loads expanded: 4495 register writes, all 1655 words parsed. Successive lists differ in buffer addresses/sizes and in the LUT contents (per-frame 3A updates), not in structure.
Next: in ferros, after `cam fe` and `cam stream`, replay the list with CPU writes into G#1C30_0000 with the output IOVAs replaced by physical buffers (SYSMMUs in bypass, S2MPUs open), and read a frame back.

**The CPU cannot stand in for the PDMA (2026-10-20).** `diag cam frame` now reads back every register it writes and reports, per page, how many hold their value. Everything in the ISPFE control pages holds (G#1C35_4000 598/598, G#1C35_5000 682/682, G#1C34_0000 54/54, G#1C33_0000 14/14, G#1C32_0000 28/33), and one region holds **nothing at all**: G#1C36_1000 / G#1C36_9000 / G#1C36_A000 / G#1C36_B000 / G#1C36_C000, 0 of 3048 writes. That region is exactly where the PDMA program's twelve block-load records (opcode 9) point.

Probed directly, it is not writable memory: G#1C36_95FC, G#1C36_A02C and G#1C36_C8B8 read constant values that CPU writes do not change, and G#1C36_1D48 changes by itself between two reads 20 ms apart while the sensor streams. So part of it is live hardware and the rest is read-only to the CPU — and the opcode-9 records are the PDMA moving data between memory and hardware the CPU has no write path to (which also means the decoder's "block load into consecutive registers" reading may be backwards: the register could be the source and the IOVA the destination, i.e. statistics being written out).

So the replay approach cannot be completed with CPU writes, and `cam frame` now drops that range instead of scattering writes into it. **The next step is to run the PDMA itself**: build a descriptor ring (16-byte entries {flags G#C003, list IOVA, 0, length}) in ferros memory pointing at the decoded program, point the PDMA at it (base, count G#3E8, start) and let the engine execute it, exactly as the HAL does. The format is fully decoded and `camera-ispfe-pdma-writes.txt` is the content.

**The PDMA engine executes our program (2026-10-20).** `diag cam pdma` (tools/camera-pdma-image.py + camera.rs) lays the whole job out at fixed physical addresses — descriptor ring at G#8800_0000, the register-write list at G#8801_0000, the twelve block-load sources at G#8802_0000, and one 2 MiB output slice per distinct buffer address at G#8810_0000 — with every bus address inside the captured list rewritten to point at ferros memory. The front end's SysMMUs are in bypass on a cold boot, so a bus address is a physical address and no mapping is needed.

Handed that ring (base, count 16, start, then the descriptor index advanced per frame), **the engine runs the program**: after the run the front end's two channel registers read G#1C32_0510 = G#8850_0000 and G#1C32_2110 = G#8830_0000, which are slices this builder invented and no CPU write ever touched. That is the step the readback test said was needed, and it works.

The CPU's own setup writes are applied around it in the capture's order (137 before the start, 66 after), the sensor streams (frame counter 01..06), and **no output slice is written** — not in its first 16 KiB, and not at any 4 KiB sample across its 2 MiB. So the front end has our addresses and still does not write.

Two hazards worth knowing: once the engine is stopped, reading anything in G#1C34_xxxx or G#1C35_xxxx stalls the bus until the watchdog (the flash log names the address, which is how both were found), and a second `cam pdma` on an already-configured front end hangs, so each attempt wants a cold boot.

What is left is the original question in a sharper form: the frame writer holds the right destination and is idle, so something that *enables* the write path is missing from the captured CPU writes and the program. The next thing to compare is the ISPFE input routing — which CSI link feeds the front end — against Linux's live registers, which the Linux handoff path (the register oracle) can dump while the camera streams.
