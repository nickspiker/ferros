# Camera, from the register oracle (2026-10-19)

Captured with the hv oracle (HV.md): the camera's control buses unmapped from the stage-2 map, every access trapped at EL2, emulated against the real register and logged. Decoded back to I2C transactions with `tools/oracle-decode-hsi2c.py`.

## The buses (husky, from the live device tree)

| sensor | role | I2C controller | addr | lens | EEPROM |
|---|---|---|---|---|---|
| boitata | main (wide) | hsi2c@10880000 | G#3D | act-cornerfolk G#0C, ois-djinn G#24 | djinn G#24 |
| dokkaebi | **front** | hsi2c@10890000 (hsi2c_2) | G#10 | G#0C | G#51 |
| oksoko | ultrawide | hsi2c@108A0000 | G#1A | G#0F | smaug G#50 |
| kraken | **tele** (folded) | hsi2c@108B0000 (hsi2c_4) | G#2D | — | humbaba G#61 |
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

## One frame reaches the front end, and the write test was blind to dark frames (2026-10-20, late)

Three results, all from the existing capture: **the oracle traps reads as well as writes**, so `target/hv/ispfe-full-raw.log` already holds Linux's live value for every front-end register it reads. No new capture was needed, and none of this required Android.

**The receiver is not the problem.** `diag cam pdma` snapshots every CSI link 1 word the camera stack touches, before the stream and again once the engine has stopped but the receiver still holds its counters. The captured CPU setup writes land in the same G#1C04_0000 range the receiver lives in, so they could have been undoing `camera_rx.rs`; they do not. Over a run 21 of 192 words move, including `+G#14 = G#100`, `+G#1C = 1` and the per-channel counters `+G#634 = G#3`, `+G#668 = G#7010011`, `+G#674 = G#140`, `+G#678 = G#4824`, `+G#6A0 = G#20000`, with the sensor's own counter running alongside.

**The output test could not have seen a dark frame.** Its sampled-page check counted a probe point only when it read neither the pattern nor zero. Every point is patterned before the run, so a point that reads zero *was written*, and a frame of a dark scene is mostly zeroes — precisely the result the test exists to detect. It now counts any departure from the pattern, samples every 256 bytes rather than every 4096, and reports how many points went to zero and the span between the first and last. The re-run still shows nothing written, so the conclusion survives, but it now rests on a test that could have contradicted it. `PDMA_BUFS` is also 18 while the program carries 21 output addresses (all correctly remapped into ferros memory), so the last three slices are neither patterned nor probed.

**The per-frame interrupt acknowledgement is load-bearing.** `tools/camera-prog.py` stops extracting CPU writes "before the per-frame interrupt handling", so 21 registers Linux reads and writes straight back on every frame appear in neither the replayed CPU writes nor the PDMA program. They look like bookkeeping and are not: unacknowledged, `G#1C32_008C` sticks at G#81 where Linux alternates it with G#1. Acknowledging all 28 in the captured order (`FRAME_ACK` in camera.rs: three words in G#1C35_01xx, the two G#1C32 channel blocks, six more in G#1C35, and the nine `G#1C33_x300`) clears it — it reads G#81 on the frame it fires and G#0 after.

**What is left: the front end takes one frame and stops asking for another.** With the acknowledgement in, the status words fire once and go quiet. Linux, per frame, steps the buffer index `+G#49C` through 0, 1, 2, 3 and the write pointer `+G#124` through G#74000, G#74800, G#75000, G#75800 — four buffers rotating, G#800 apart. ferros holds `+G#49C` at 0 and `+G#124` at G#75800 forever, while CSI link 1 keeps counting and the sensor's counter runs to 06.

Linux's descriptor ring is count G#3E8 and the HAL builds a **fresh list per frame**, each with new buffer addresses; ferros hands the engine 16 descriptors that all point at one list and advances `PDMA + G#10` by G#10 a frame. So the likely missing piece is not an enable bit but the re-arm itself: in Linux the front end does not rotate buffers on its own, the HAL re-programs it every frame through the PDMA. The ordering around a frame boundary is in the capture (one frame's block is lines 1596-1699 of the full log); what to compare is what Linux does between one frame's acknowledgement and the next frame's arrival.

## The sensor is ours; the front end's routing is not (2026-10-20, night)

### The register map was in the tree all along

`zuma-isp-event-info.dtsi` (7847 lines) names every interrupt source, mask, overflow and reset register of every block in the ISP, and `dt-bindings/lwis/platform/zuma/*.h` names every bit. LWIS's *logic* is a passthrough, which is why it was written off — but its device tree is where the front end's register semantics are written down in the open. `tools/gen-isp-events.py` generates `ferros_kernel/src/camera_events.rs` from it: 203 sources with named bits. `diag cam events` reads them all and prints the ones reporting, counting a source as interesting when its status is live **or** its mask is armed — an armed mask with no status is a channel that was set up and never fired.

That immediately re-read words this driver had been printing for days. `CSIS_LINK1 + G#14 = G#100`, recorded as the link "latching its interrupt sources", is **ERR_DESKEW_OVER** — a D-PHY lane deskew overflow. The rest of the snapshot decodes as `+G#1C` LINE_END, `+G#24` FRAME_START on channels 0-2, `+G#2C` FRAME_END on channels 0-2.

### Where the chain actually stops

Read three frames into a PDMA run:

```
isp_fe_ctx1_csis_fs_int          +G#10024 = G#7 FRAME_START_CH0|CH1|CH2   mask G#0
isp_fe_ctx1_csis_fe_int          +G#1002C = G#7 FRAME_END_CH0|CH1|CH2     mask G#0
isp_fe_loch0_csis_core_img_proc  +G#20424 = G#0                           mask G#1A
isp_fe_ctx0_pdma_err             +G#34    = G#0                           mask G#1F
```

"ctx1" is CSI link 1 (the csis-link-phy space plus one times G#10000). So link 1 takes frames, three channels of them; the front end's local channel 0 is armed with Linux's own mask G#1A (EOF, SHADOW_UPDATE, DMA_DONE) and raises nothing but SHADOW_UPDATE, which only means "your configuration was latched"; the engine reports no errors. **Nothing carries frames from the link into the front end's channel.**

### Ruled out, each cheaply and each worth not repeating

- **Ordering.** Linux starts the PDMA before enabling the link (capture lines 1421 then 1573) where ferros did the reverse. Reordered to match: identical result.
- **The per-frame interrupt acknowledgement.** `tools/camera-prog.py` stops extracting "before the per-frame interrupt handling", so 21 registers Linux reads and writes straight back each frame were in neither the replay nor the program. They are real — unacknowledged, G#1C32_008C sticks at G#81 instead of alternating with G#1 — and `FRAME_ACK` in camera.rs now does all 28 in captured order, which clears it. It did not produce a frame.
- **Missing boot-time initialisation.** The oracle was armed sixty seconds in, on purpose, to skip the boot probe, so any one-time setup before a camera open was never captured. The capture exposes it anyway: a register Linux **reads as non-zero before ever writing it** was set up earlier. `diag cam preexist` compares all 45 such registers on a cold ferros — 36 match, and the nine that differ are all interrupt status from Linux's prior session. There is no missing boot init.
- **A DMA on the link, to bypass the front end.** The csis-link-phy space has only `int0/int1/fs_int/fe_int` per context across all 12 contexts, no DMA. The front end's `csis_core` *is* the minimal DMA; there is nothing to route around.
- **VOTF.** `lwis_votf` serves rgbp/yuvp/tnr/mcsc/gdc/gse — downstream blocks, not the CSIS-to-front-end path.

### Two measurement bugs of ours, both of which could have hidden success

- The output test counted a probe point only when it read neither the pattern nor zero. Every point is patterned first, so a point reading zero **was written**, and a frame of a dark scene is mostly zeroes — exactly the result the test exists to find. It now counts any departure from the pattern and samples every 256 bytes.
- The slices were 2 MiB. A 4080 x 3072 RAW10 frame is 15 MiB, so no correctly-programmed frame could ever have landed in one. Linux's buffers were spread over about 100 MB; the builder had compressed them to 2 MiB apart. `tools/camera-pdma-image.py` and camera.rs now agree on 16 MiB slices over the seven that fit free DRAM.

**A wrong conclusion, corrected (2026-10-21).** Widening the slices reset the phone three times and that was written up as "something above G#8AA0_0000 is not ours". It was not: all three of those runs were built from PDMA list 1, and the single run that worked was the only one built from list 2. `zuma-rmem.dtsi` reserves nothing below G#9000_0000 and the hv's own addresses sit below G#8400_0000. Retested with list 2, seven 16 MiB slices patterned to G#8F10_0000 run clean — sensor streaming, link 1 counting its 21 words, channels holding G#8A10_0000 and G#8910_0000. **`--list 2` was the whole cause**, which makes it the single most important flag in that tool.

### The sensor, on the other hand, is fully ours

It speaks MIPI CCS (SMIA++), a public standard, and of the 1347 writes in the captured init only **twenty** define the mode — the rest is vendor tuning. The captured mode decodes completely: RAW10, 4080 x 3072 output, the full 16320 x 12288 array binned four to one, `line_length_pck` 6320, `frame_length_lines` 10584, `pll_multiplier` 245, `extclk` 24.0 MHz, exposure 20 lines.

**`diag cam slit` drives it instead of echoing it.** Four writes inside a grouped-parameter hold — `y_addr_start`, `y_addr_end`, `y_output_size`, `frame_length_lines` — and, with the sensor's own frame counter as the clock so nothing downstream can flatter the result:

| mode | frames in 250 ms | rate |
|---|---|---|
| captured, 4080 x 3072 | 4 | about 16 a second |
| ours, 4080 x 64 | 296 | **about 1184 a second** |

Seventy-four times the rate, read back as `y_output_size G#40`, `frame_length_lines G#80`, `x_output_size G#FF0`, `mode_select G#103`. That is slit-scan's foundation and it stands clear of the front end entirely. A slit frame is also 319 KiB rather than 15 MiB.

With the slit the link's own state tracks the new geometry (`+G#678` moves to G#954A, `+G#624` appears) and new error bits arrive in `+G#14` and `+G#1C` — the captured program hard-codes the full-frame geometry, so the front end is being handed 64 lines where it expects 3072. Matching the front end to our own window needs its configuration registers, which is the one map LWIS does not carry.

**`diag cam caps`** reads the CCS capability block (addresses from the mainline driver's generated `ccs-regs.h`). Identity, gain, integration-time and ADC capability answer properly; the clock and frame limit block at G#1100 does not exist on this part — G#10F4, G#1104 and G#1140 all return the same G#41400000, which is address aliasing. `diag cam probe` establishes that this is the sensor's answer and not ours: G#1140 reads G#4140 whatever preceded it, and a four-byte read of G#0000 gives G#08EFA100, which is the model ID and revision exactly. So the envelope has to come from experiment, and the slit result is the first measurement in it.

### What to try next

1. **The front end's geometry.** Everything we can compare against Linux now matches, so the gap is a register whose meaning we do not have rather than a value we failed to copy. The remaining untried source is the PDMA's **first** list of a session — `camera-ispfe-pdma-writes.txt` is list 2 of a rotating three, and one-time setup would live in the first.
2. **A frame at the captured geometry** now that a slice can hold one, which is the test the 2 MiB slices made impossible.
3. **Our own pipeline from raw Bayer**, in the pinned gamma2tanh profile, once a frame lands. The vendor ISP is 4495 register writes whose meaning lives only in a closed HAL; reproducing its tuning is neither possible nor wanted.

## Lead 1: the front end's live state against Linux's (2026-10-21)

**The physical layer is clean.** The link's error word has 22 named bits and reads exactly G#100 — `ERR_DESKEW_OVER` alone. `ERR_CRC` (payload), `ERR_ECC` (header), `ERR_CRC_PH`, `MAL_CRC`, `ERR_SOT_HS` and `ERR_SOT_SYNC` on all four lanes, `RX_INVALID_CODE` on all four lanes, `PHY_TIMING_ERR` and `ERR_SKEW` are all clear. The receiver CRC-checks every long packet, so if deskew had failed the pixel payloads would fail CRC; they do not. Deskew is a calibration timer overflowing, not corruption, and it is not the blocker it was briefly taken for. (The slit run with mismatched geometry *did* add `ERR_CRC_PH`, which is what real damage looks like.)

**The read-back diff has little reach.** Linux reads only 66 front-end and link registers while streaming; 57 hold one value, and only five of those are non-zero. A routing map written once at setup and never read back is invisible to this method, so a clean result here does not clear the front end's configuration — it only clears what Linux happened to look at.

**Three interrupt masks run one bit lighter under Linux.** Written identically at setup by the program, then each with one bit cleared by the HAL at runtime:

| mask | Linux | ferros | extra bit in ferros |
|---|---|---|---|
| G#1C32_2030, loch0 pd_proc | G#12 | G#1A | 3, SHADOW_UPDATE |
| G#1C35_0150, ctx0 lmp_pdaf | G#3EF | G#3FF | 4 |
| G#1C35_00F0, ctx3 lmp_bayer | G#103FFFFE | G#103FFFFF | 0 |

They are interrupt masks, which gate delivery rather than data, so they are unlikely to be the routing — but they are the only configuration-class difference this method found, and they cost one run to rule out.

**Reading the front end mid-stream is timing-dependent.** `live_compare` stalled the bus and reset the phone reading G#1C32_0430, the loch0 image mask — a word the event dump reads without trouble moments earlier in the same run. It is left in camera.rs but not called from the frame loop. Treat any front-end read during streaming as able to hang the core, and name each one in the flash log first.

## From the ground up: the link's output never reaches the core (2026-10-21)

**ferros's own raw path, no vendor program.** `diag cam raw` configures only the link and the front end's CSIS core — the 64 writes Linux makes into G#1C32, in its order, with RX_START between — and skips the PDMA and every processing block. It behaves exactly as the full 4495-write program does: frame one the core latches our configuration (G#81 / G#8 / G#8, SHADOW_UPDATE), and every frame after the link reports frame start and end on all three channels while the core reports nothing. **The processing pipeline is exonerated.**

The configuration is legible enough to say it is right. Link 1's channels: CH0 RAW10 4080 x 3072 (image), CH1 RAW10 4080 x 768 (phase-detect), CH2 embedded data 4080 x 4. The core sizes its image path to CH0 (+G#474 = G#0C000FF0) and its PD path to CH1 (+G#2074 = G#03000FF0), and +G#48C / +G#208C differ only in bit 12, which selects the link channel.

**No local channel sees anything.** `diag cam lochs` arms all fourteen paths — image and PD for loch0 to loch6, G#400 apart from +G#20424 and +G#22024 — clears them, streams without acknowledging, and reads once after the stop. All fourteen read G#0, the aggregate reads G#0, while the link reported frame start and end throughout. The data is not landing on a channel we were not watching; it is not reaching the core. (The aggregate's long-standing G#81 is loch0's image path at bit 0 and its PD path at bit 7: image paths occupy bits 0-6 and PD paths start at 7.)

**Closed along the way:**
- **The skipped Q-channel.** `fe_up` requests every CMU_ISPFE Q-channel from G#301C to G#30D0 except G#30C8. So does Linux — the capture writes the same range with the same gap.
- **A link-side DMA.** The link's interrupt words carry frame start and end for 32 channels, line end, lost-frame and resolution-mismatch errors, and lane errors. Nothing DMA-shaped. The core is the only write path.
- **PDMA list 1 as one-time setup.** It writes the same 1447 registers as list 2, none unique to either; 51 values differ, 42 of them in G#1C35 — buffer addresses and per-frame 3A tables. Its resets are most likely a buffer address escaping the builder's remap, not a clue.

**What that leaves.** Every CPU write Linux makes to the link and the core, ferros makes, in order; the pre-capture state matches; the PDMA's lists hold no extra setup. So the link-to-core connection is set by a write we have never seen. The census only logs pages in its target list, which covered the ISP front end — a write to a page outside it would be invisible, and a camera open has already been seen reaching into the TPU domain, outside that list. **Next: a wider census**, arming the whole ISP span (G#1C00_0000 to G#1D8F_FFFF) and the system controllers around it for one camera open, to find any CPU-written page that nothing ferros does touches.

## The wide census, and a live snapshot that found the clock (2026-10-21, afternoon)

**The wide census closes the "unseen page" lead.** `FERROS_HV_ORACLE_CENSUS=1` now logs each page's first read and first write separately and hands a page back to the guest after its first write, so the census can span thousands of pages without trapping a streaming camera. The bitmap is sized from the target list (it was fixed at 2048 pages). Armed over the whole ISP span G#1C00_0000-G#1D8F_FFFF plus CMU_TOP (G#2604_0000-G#2604_FFFF), plus every EL3 call (`FERROS_HV_ORACLE_SMC=1`), through boot and one marked camera open, it found 182 pages (`target/hv/isp-wide-census-raw.log`). Inside the front end, the only pages missing from the 42-page capture are its SYSMMUs and S2MPUs. Every other ISP block (G#1C40 to G#1D8F) gets its CMU control word, sysreg, S2MPU and SYSMMU set at boot, and during the open a handful of enables, none of them upstream of the front end. The open makes **no EL3 calls at all**. So no CPU write was being missed.

**The hv can now snapshot Linux's live registers.** `FERROS_HV_SNAPSHOT=1` with `FERROS_HV_SNAP_LIST=<file>` reads a list of registers when the camera stack advances the PDMA's descriptor index to G#100, sixteen frames into a stream, and logs every value. The list comes from `tools/camera-snap-list.py`: 2914 registers known to exist, drawn from these sources:
- every CMU_TOP and CMU_ISPFE register in cmucal-sfr.c;
- every address in the front-end capture;
- every register the PDMA program writes, minus the G#1C36 block-load region;
- every LWIS event status and mask register;
- CSI links 0 and 1.

The same script turns the log into `ferros_kernel/src/camera_snap.rs`. `diag cam pdma` reads the same list three frames into its own stream (`snap_compare`) and reports every word that differs.

Capture:
```
tools/camera-snap-list.py > target/hv/snap-list.txt
FERROS_HV_STAGE2=1 FERROS_HV_ORACLE=1 FERROS_HV_NO_PMU_READ=1 FERROS_HV_SNAPSHOT=1 FERROS_HV_SNAP_LIST=$PWD/target/hv/snap-list.txt \
FERROS_HV_ORACLE_PAGES=G#1C39_0000 FERROS_HV_ORACLE_CAP=256 FERROS_HV_ORACLE_ARM_AFTER_MS=60000 HV_CMDLINE=loglevel=4 tools/husky-hv.sh --no-ferros --flash
# past 62 s: mark, open the camera 10 s, close, mark, sysrq c; then pull "ferros hv|ferros-mark" from console-ramoops-0
tools/camera-snap-list.py --rust target/hv/cam-snapshot-linux.log
```

**The front end's clock was a quarter of Linux's.** CMU_TOP's `MUX_CLKCMU_ISPFE_NOC` / `DIV_CLKCMU_ISPFE_NOC` (G#2604_10BC / G#2604_18B4) read 4 / 0 under Linux: PLL_SHARED1_D4 undivided, 465 MHz (SHARED1 = 24.576 MHz × 227 / 3). A cold ferros has 1 / 5: PLL_SHARED3_D1 ÷ 6, 111 MHz (SHARED3 = × 108 / 4). That clock feeds the front end's bus, the CSI links' ACLK and the D-PHY clock. Linux never writes it from the CPU: the camera's DVFS request goes to the ACPM firmware, which sets it, so no register capture could have shown it. `fe_up` now sets it, mux first so the clock only slows on the way.

With it, the link behaves as under Linux:
- ERR_DESKEW_OVER (`+G#14` = G#100) is gone, along with LINE_END in `+G#1C` and the `+G#634` / `+G#66C` words;
- `+G#668` reads G#11, Linux's value, where it read G#7010011.

The rest of CMU_TOP differs only in other domains' clocks (AUR, BW, DPU, G3D, MFC, MIF_NOCP, NOCL0) and in the CIS_CLK Q-channel requests ferros leaves at 2.

**The core still reports nothing.** What the snapshot still shows in the front end looks like the core never seeing a frame start, not like a missing setting:
- G#1C32_0508 / 2108, G#1C33_700C and G#1C34_0004 hold the program's G#8F where Linux reads G#10;
- G#1C32_041C / 201C and G#1C33_7020 / B020 hold the program's 1 where Linux reads 0.

Those are requests the hardware consumes when a frame arrives, and ours are still pending. The remaining differences are buffer addresses, counters and per-frame 3A values. CMU_ISPFE's Q-channel status words (G#1C00_7xxx: PPMU and SYSMMU channels) differ because ferros forces those clocks on.

## First frame (2026-10-21, night)

**The front end's local channels were never enabled.** Linux writes G#81 to G#1C32_00A0 right after acknowledging the first SHADOW_UPDATE (capture access 531, after the PDMA index write at 458), and the image path's SOF follows. G#81 is loch0's image (bit 0) and PD (bit 7) paths, the same layout as the aggregate word. It is a plain write, not a read-and-write-back acknowledgement, so neither the setup extraction (`camera-prog.py` stops "before the per-frame interrupt handling") nor `FRAME_ACK` (read-back pairs only) carried it. The live snapshot did show it (Linux G#81, ferros 0), and it was first misread as status. Its partners: G#1C32_00B0, written 1 then G#80 during setup, is the request; G#1C32_00A4, written 1 then G#80 at stop, is the disable.

With it, `diag cam pdma` gets, every frame:
- G#1B (SOF, EOF, SHADOW_UPDATE, DMA_DONE) on both of loch0's paths;
- a buffer index counting 0, 1, 2 and on;
- the frame in memory.

**Where the frame goes.** The image body is written to the address in **G#1C32_05C0**. That is one of the four registers the builder's IOVA window had missed (`ADDR_REGS`): it was pointing the frame at physical G#0200_0000. G#1C32_0510 holds only a 16-byte header (G#8F, 0, 1, 0). The PD path (G#1C32_21C0) gets 3.75 MiB, 4080 × 768 RAW10. The output slices now sit at G#A000_0000, 32 × 16 MiB, one per output address. The first frame, taken while the 22 outputs still shared seven slices below G#9000_0000, came back with other channels written over its top half. Slices from G#B600_0000 up read back as zeros after patterning, so keep the outputs below that.

**The format is MIPI RAW10, packed.** Four pixels in five bytes (four MSB bytes, then the 2-bit LSBs), stride G#1400 = 5120 bytes, 3072 lines: exactly 15 MiB. Adjacent same-colour rows correlate at 0.94. The first frame: black level about 64, median 68, 99th percentile 108. That is a dim scene at the captured exposure of 20 lines, out of focus because nothing drives the focus actuator. Readback: `bridge read A2000000+n*10000 10000` × 240 (64 KiB per read).

Also ruled out tonight, worth not repeating:
- **Response timing** (`cam pdma fast` answers the core within microseconds and sees no more than the 50 ms loop did). The single G#81 seen without the enable comes from the PDMA executing the program, not from a frame.
- **VDD_CAM.** Linux raises S1S_VDD_CAM from 537.5 to 600 mV while streaming. Cold ferros has 575 mV, and ACPM refuses a direct write to that buck (error G#103).
- **The hv transplant** (`FERROS_HV_XPLANT`). It works mechanically, but Linux's HAL restarts the whole session within 25 ms of any disturbance, so it cannot isolate a breaking write.

## Focus, exposure, and keeping the stream alive (2026-10-21, night)

**The lens actuator** ("cornerfolk", G#0C on the sensor bus) runs from ferros. The sequence:
- 02 ← G#40 (reset);
- **at least 5 ms** of quiet (straight after the reset every write NACKs);
- 02 ← 0, F1 ← 0;
- the 16-bit target into 00/01.

84 reads the position back; it tracks the target within 30 ms across G#0800 to G#7800.

`diag cam pdma sweep=<start>:<step>` steps eight positions while streaming and copies a frame at each to G#9800_0000 + k × 16 MiB. `tools/camera-pull.sh <prefix> <base>` and `tools/camera-raw10.py` fetch and decode. Over a floor about a metre away, sharpness on 8 × 8-binned frames peaks at G#3800 and falls off on both sides. The crop resolves cable braid and dust at half resolution.

**Exposure is already long.** The captured init's last per-frame block sets integration G#535E (21342 lines) in a G#5372-line frame at analogue gain G#0400. The "20 lines" seen earlier is an intermediate value. Copying Linux's block from the older open capture (integration G#0A56, frame length G#2959) doubled the frame rate and stalled the core. So `exp=`/`gain=` now write only what is asked, inside the hold. The hold is the four-byte write `01 04 01 01` and its release `01 04 00 01`: G#0104 is one byte, G#0105 stays 1.

**The front end has to be answered.** A single 15 MiB copy of uncached memory (about 600 ms) left it unacknowledged long enough to stop taking frames for good (buffer index frozen). With the copy cut into 256 KiB pieces and the index advance plus acknowledgement every 50 ms between them, it streams through a whole sweep (index G#7 to G#83). The descriptor ring is now 1024 entries, matching the count register.

## First colour (2026-10-21, night)

`tools/camera-raw2vsf` turns a pulled frame into a VSF visual characterised by the main camera's Absolute IDT (`pixel 8 pro calibration data/derived/cam2-main.idt.vsf`, derived from the Android/Lumis captures), plus a preview PNG. The Lumis main-camera capture is the template: same sensor, same 4080 × 3072 mosaic, black 64, white 1023.

- **The mosaic is G B / R G** in the channel order the IDT was fitted in (Lumis records `cfa` [1, 2, 0, 1] over channels R, G, B). ferros's readout uses the same mode and orientation, so the same layout applies.
- **`ProfileEntry.matrix` is row-major** (vsf `spectral_image.rs`). `apply_matrix_3x3_f32` reads vsf's own column-major constants, so an IDT matrix must be applied by rows. The first render went through the helper and came out transposed.
- **Preview:** black-subtract; scale each channel by the IDT's sensitivity, so channel ratios become radiance ratios; absolute matrix to linear VSF RGB; vsf's `VSF_RGB2SRGB` and the sRGB curve for a screen. One exposure scale and no white balance: an Absolute IDT keeps the room light's cast.

```
tools/camera-pull.sh target/cam/shot [base]
tools/camera-raw2vsf/target/release/camera-raw2vsf target/cam/shot.raw10 "<lumis main capture>.vsf" "pixel 8 pro calibration data/derived/cam2-main.idt.vsf" shot.vsf shot.png [exposure_s]
```

The exposure is about 71 ms, estimated from frame timing (G#535E of G#5372 lines at roughly 14 frames a second); the sensor's pixel clock is not yet read, so it is marked as estimated in the file. That figure, and the f-number, are what the IDT's absolute scale needs. Comparing ferros's own calibration capture against the Android-derived IDT needs both exact.

## Viewfinder (2026-10-21, night)

`diag cam pdma af=<pos> view=<seconds>` paints the live main camera onto the phone's panel:
- one 2 × 2 Bayer cell per output pixel, 10-bit, at half the panel's resolution;
- rotated into portrait and centred;
- coloured through the main camera's Absolute IDT folded with vsf's VSF_RGB2SRGB (`VIEW_M`, Q12, no floating point in the kernel), gamma 2 encoded;
- exposure from a 64 × 64 pre-sample.

About 0.8 frames a second, limited by uncached reads of the frame (six byte reads a cell). The front end streams throughout (index G#135 after 20 s). A framebuffer readback (`bridge read FAC00000…`) shows the floor scene, in the room's cast.

Two traps:
- **Adopt the panel before the camera.** `display::adopt` after the camera bring-up hung in its first EL3 call every time (the flash log ended at "adopting the panel"), the EL3 poison pattern after the camera path's direct PMU reads. Adopted at the start of the run, it works, and `present()` makes no EL3 calls.
- **The display side is a stand-in** (sRGB primaries, gamma 2) until the panel itself is calibrated; the camera side is the measured IDT.

## Many runs in one boot (2026-10-21, night)

`cam pdma` used to work once per boot, and the second one hung. Measured with flash-log marks at every stage:
- **The domain prepare call (SMC G#8200_0410) never returns the second time.** It is now made once per boot.
- **The D-PHY isolation bypass's EL3 write (PMU G#3EBC) hung the same way.** Also once per boot now: the bit stays set until reset.
- **The front end needs Linux's teardown.** With both fixes the second run completes, but its front end takes no frames. `tools/camera-close.py` extracts every CPU write from the capture's "camera close" mark onward: the PDMA's last index, PHY lanes off, the local channels disabled (G#1C32_00A4 = G#80 then 1), processing blocks cleared, PDMA stopped, links and core reset. That is 223 writes in `camera_close.rs`, replayed at the end of every run.
- **The sensor needs a power cycle** (`diag cam off`, `diag cam up`) or it does not stream again.

Sequence: `cam bus`, `cam up`, `cam pdma …`, then for each further run `cam off`, `cam up`, `cam pdma …`. Two consecutive runs both take frames (buffer index 0 to 6).

Avoid `bridge read priv:15463EBC` after a camera run: it hung ferros until the watchdog.

**Exposure timing.** CCS from the init: EXTCLK 24 MHz, pre-PLL ÷3, ×245, vt_sys ÷1, vt_pix ÷8, line_length_pck 6320, frame_length 21362, integration 21342. Read literally that is a 551 ms frame; measured, the sensor runs about 14 to 15 frames a second. vt_pix_clk taken as 1960 MHz (the ÷8 not applied, or eight pixel pipes) gives 3.22 µs lines, a 68.9 ms frame and **68.8 ms integration**, consistent with the measured rate. Confirmed by a timed count (`cam pdma rate`): 30 frames in 2.034 s on both the core's buffer index and the sensor's own counter, a period of 67.8 ms ± 2.3 ms (one-frame resolution), against 68.9 ms predicted. Still open for an absolute comparison: how analogue gain G#0400 maps onto the ISO the IDT's `gain_ref` was measured at.

## The other cameras: captures (2026-10-22, early)

Driven from adb through the Pixel Camera app (`tools/husky-hv-camall.sh`): open the camera, then marked taps at measured screen positions (1008 × 2244). The positions are UW (396, 1437), T (615, 1437) and flip (780, 1683). The front-camera intent extras are ignored by the app.

- **One bus per capture.** Two controllers trapped together (camera PMIC + ultrawide) gave length mismatches in the same place twice. The cause is the hv's repeat-collapsing state, shared across cores without a lock (memory: feedback_oracle_capture_integrity).
- **Ultrawide ("oksoko", hsi2c@108A0000): replay-grade capture taken.** 3978 transfers, integrity 0 (`target/hv/uw1-i2c-raw.log`, decoded `uw1-108A.txt`; local only, it carries the vendor settings).
  - The sensor at G#1A identifies as **G#0787 (Sony IMX787)** at G#0016/0017.
  - Plain CCS writes, one 4034-byte settings burst at G#CC00, stream-on at transfer 255.
  - Per-frame exposure inside the G#0104 hold.
  - The lens actuator at G#0F takes the same protocol as the main camera's (02 ← G#40, 02 ← 0, target in 00/01).
  - Its lens polling fills a 36000-line cap in about 8 s, after the init.
- **Roles corrected (2026-10-22):** the device tree's own comments (`zuma-husky-camera.dtsi`) name dokkaebi the FRONT camera (G#10, hsi2c_2) and kraken the REAR folded TELE (G#2D, hsi2c_4); this table had them swapped. So the "front" capture below trapped the tele's bus, and in the four-bus run the T tap did reach the tele (kraken traffic in the "tele" segment).
- **Tele ("kraken", G#2D on hsi2c@108B0000):** its ID reads **G#08D5**. Those single-bus captures flipped to the front camera, so the tele only answered its boot-time ID probe; nothing was wrong with the trap.
- **Front ("dokkaebi", G#10 on hsi2c@10890000):** not captured yet on its own bus; the flip tap reaches it.

Still to capture for each: its CSI link and PHY lanes (an ISP front-end capture of that session) and its PDMA program (`FERROS_HV_PDMA_DUMP`). The power-up comes from each module's `module_power_seq` in the device tree, as for the main camera.

## The ultrawide takes pictures (2026-10-22)

`diag cam uw up` then `diag cam pdma uw`, built with `FERROS_CAM_UW=target/cam/uw` (local bundle). Image 4000 × 3000 RAW10 at stride G#13C0, pulled with `tools/camera-pull.sh <prefix> A2000000 4000 3000 5056`.

What it took, all from captures of a Linux switch from the main camera to the ultrawide:
- **Power** from the existing module table (`power_module`): camera-PMIC LDO1, 8, sub-PMIC LDO12, LDO2, 4, 6 through the overvoltage guard, load switch G#1711, reset gpp6-3. It answers **G#0787 (Sony IMX787)** at G#0016.
- **Sensor init:** 231 writes including a 4034-byte settings upload, from the single-bus I2C capture (`tools/camera-init.py`, integrity 0). Stream on is G#0100 = 1 alone (the main's write also carries orientation).
- **Receiver:** CSI **link 2** (G#1C06_0000) and the **csis2 D-PHY lanes** at G#1C10_2300/2400/2500, with the same lane values as the main's csis1 lanes, SYSREG G#1C02_0500 = 4 (`camera_rx_uw.rs`, from `tools/camera-cpu.py`).
- **Front end:** the same 4495-register PDMA program with its own values: geometry G#FA0 × G#BB8, stride G#13C0, request words G#8D where the main has G#8F. Its CPU setup comes from the capture with the 25 address registers dropped; the program sets those.
- **The image builder** now remaps the 25 address registers by name and maps each 16 MiB IOVA granule to one slot, offset kept (`tools/camera-pdma-image.py`). The guessed IOVA window failed on this session's addresses, and two registers hold a base and an offset into the same buffer. The image body still pins to slot 2, so it lands at G#A200_0000 for either camera.

The sensor runs at about 30 frames a second and the whole frame lands (G#E77100 bytes). The front end's buffer index stays at 0 and its image path reports SOF without EOF after the first frame, so the per-frame acknowledgement may differ from the main's. Next: its lens actuator (G#0F, the main's protocol), then its own IDT (`derived/cam3-ultrawide.idt.vsf`).

**Continuous frames and colour (same day).** The ultrawide runs at about 30 frames a second, so the front end is now answered every 15 ms for it (50 ms for the main, which runs at 14.5). At 50 ms it took one frame and stopped. With that and its frame handler's extra acknowledgement (G#1C35_0148), the buffer index runs through a whole sweep (5 to G#AB). `tools/camera-raw2vsf` now takes the frame geometry from the template, so the ultrawide renders through `derived/cam3-ultrawide.idt.vsf` with the Lumis ultrawide capture as template (its mosaic reads B G / G R). Colour order not yet confirmed against a known scene.

**Open: the ultrawide's lens actuator (G#0F) and EEPROM (G#50) never acknowledge.** `diag cam uw scan` finds only the sensor at G#1A, before and after its init, though both parts' DT supplies are on: sub LDO12 at 1.85 V (ctrl G#AE, the enable bit 7 set), camera-PMIC LDO1, 4 and 8. The main camera's actuator, on the same LDO12, works. In Linux's capture the actuator's first write comes during the sensor's init, right after the sensor writes `47 0C 01` and `47 0D 07`, which may be a sensor-driven enable; but our replay includes those writes. Until this is found the ultrawide stays at its rest focus.
