---
name: husky-camera-bringup
description: ferros powers husky's main camera cold and DRIVES the sensor — its own window and rate, 4080x64 at ~1184 fps (2026-10-20) — but no frame reaches memory; the ISP's register names come from LWIS's device tree, and the break is link 1 -> front-end channel
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


## 2026-10-20: the sensor is ours, the ISP front end is not

**The ISP's register map is in the Pixel tree, in the open.** `zuma-isp-event-info.dtsi` (7847 lines) + `dt-bindings/lwis/platform/zuma/*.h` name every interrupt source, mask, overflow and reset register of every ISP block and every bit in them. LWIS's *logic* is a passthrough, which is why it got written off, but its device tree carries the semantics the closed HAL otherwise keeps. `tools/gen-isp-events.py` generates `ferros_kernel/src/camera_events.rs` (203 named sources); `diag cam events` prints the ones reporting, counting an armed mask as interesting too. **Use this before diffing any ISP register by hand.**

It immediately re-read words the driver had printed for days: `CSIS_LINK1 + G#14 = G#100` is **ERR_DESKEW_OVER** (a D-PHY lane deskew overflow), not an "interrupt latch"; `+G#1C` LINE_END, `+G#24` FRAME_START ch0-2, `+G#2C` FRAME_END ch0-2.

**Where it breaks:** link 1 takes frames (frame start and end, three channels); the front end's local channel 0 is armed with Linux's own mask G#1A and raises only SHADOW_UPDATE, never SOF; the PDMA engine reports no errors. Nothing carries frames from the link into the channel. That one hop is the whole remaining problem.

**Ruled out — do not redo:** the PDMA-before-link ordering; the 28 per-frame interrupt acknowledgements (real, implemented as `FRAME_ACK`, necessary but not sufficient); missing boot-time init (`diag cam preexist` compares all 45 registers Linux read non-zero before ever writing — 36 match, 9 are stale status); a DMA on the link to bypass the front end (the csis-link-phy space has only int0/int1/fs/fe per context, no DMA); VOTF (serves downstream rgbp/yuvp/mcsc/gdc only).

**The sensor speaks MIPI CCS (SMIA++), so it is fully ours.** Only 20 of the 1347 init writes define the mode; the rest is vendor tuning. `diag cam slit` proves it: four writes in a grouped-parameter hold (`y_addr_start`, `y_addr_end`, `y_output_size`, `frame_length_lines`) take 4080x3072 at ~16 fps to **4080x64 at ~1184 fps**, measured on the sensor's own frame counter. Traps: set before stream-on the sensor never starts; set mid-stream it works but leaves the sensor stuck until a **cold boot** (a reload does not power-cycle it).
The CCS *limit* block at G#1100 does not exist on this part — G#10F4, G#1104 and G#1140 all return G#41400000, address aliasing. Verified as the sensor's answer, not ours (`diag cam probe`). So the envelope comes from experiment.

**The front end's geometry is legible:** stride at +G#20554/+G#22154 = G#1400 (4080 RAW10 on a 256-byte boundary), frame size as (height<<16)|width = G#0C000FF0 at ~12 registers, once reversed as G#0FF00C00, height halved at G#600 and quartered in +G#40014's high half. `camera-pdma-image.py --height N` rewrites it. Necessary, not sufficient — a matched slit still writes nothing.

**Traps in our own tooling:** `camera-pdma-image.py` needs **`--list 2`** (the default list 1 is a different program and resets the phone); output slices must be **16 MiB** (a 4080x3072 RAW10 frame is 15 MiB and they were 2 MiB, so no correct frame could ever have landed) but only **two** of them (patterning as far as G#8F10_0000 resets the phone; G#8810_0000-G#8AA0_0000 is safe); patterning them needs a `pet_watchdogs()` in the loop. And the write test used to count a probe point only if it read neither pattern nor zero — a dark frame is mostly zeroes, so it was blind to the result it existed to find.

Full detail: ferros/docs/oracle/CAMERA.md.

## 2026-10-21: the clock no capture could show

**The hv snapshots Linux's live registers.** `FERROS_HV_SNAPSHOT=1` + `FERROS_HV_SNAP_LIST` reads 2914 known-to-exist registers sixteen frames into a Linux stream (`tools/camera-snap-list.py` builds the list and turns the log into `camera_snap.rs`); `diag cam pdma` reads the same list cold and prints every difference. This is the tool for "everything we capture matches, it still fails": it sees state set by firmware, which no trap does. Recipe in CAMERA.md.

**Found with it:** CMU_TOP `ISPFE_NOC` (mux G#2604_10BC, div G#2604_18B4) is 465 MHz under Linux (PLL_SHARED1_D4 ÷1) and 111 MHz cold (SHARED3_D1 ÷6). Linux sets it through ACPM DVFS (INT domain), never a CPU write. `fe_up` now sets it, mux first, and the link's deskew overflow vanished. **Not sufficient:** the core still sees no frame start. The front-end diffs left over (G#8F where Linux has G#10, `+G#1C`/`+G#20` = 1 where Linux has 0) are pending requests the hardware consumes at frame start: symptoms.

**Also ruled out:** the wide census (whole ISP span plus CMU_TOP plus every EL3 call) shows no unseen front-end page (only its SYSMMUs/S2MPUs) and no EL3 calls during a camera open. The PDMA builder's IOVA window missed four address registers (fixed, `ADDR_REGS`), and that wasn't it either.

**Process trap:** with several adb devices attached (other phones on the LAN), `tools/husky-hv.sh --flash` dies at `adb reboot bootloader`. Use `ANDROID_SERIAL` from Android, or go `bridge reboot fastboot` + fastboot by hand from ferros.

## 2026-10-21 night: FRAMES, focus, colour, viewfinder

**The missing write was G#1C32_00A0 = G#81** (enable loch0 image + PD paths), which Linux writes right after the first SHADOW_UPDATE ack. It was missed because it's a plain write, not a read-back ack, and the setup extraction stopped before per-frame handling. With it: SOF/EOF/DMA_DONE every frame, and a 15 MiB RAW10 frame in DRAM.

**Facts to not re-derive:**
- **Where the frame lands:** the image body goes to the G#1C32_05C0 address (an `ADDR_REGS` entry), not to G#1C32_0510 (a 16-byte header).
- **Output slices:** 32 × 16 MiB at G#A000_0000, one per output. Slices from G#B600_0000 up read as zeros.
- **Format:** MIPI RAW10 packed, stride G#1400, mosaic **G B / R G** (Lumis cfa [1,2,0,1] over R,G,B), black 64.
- **Exposure is already near max** in the init (21342 of 21362 lines). The hold is `01 04 01 01` / `01 04 00 01` (G#0104 is one byte). Copying Linux's older block (frame length G#2959) stalls the core.
- **The lens actuator** is at G#0C. Reset 02←G#40, **wait 5 ms**, then 02←0, F1←0, target in 00/01, position read at 84. Focus around 1 m is G#3800.
- **Service the front end at least every ~50 ms** (index advance + FRAME_ACK) or it stops for good. Chunk long copies.
- **Adopt the display before the camera** (EL3 poison hang otherwise).
- **IDT matrices are row-major** (`ProfileEntry.matrix`); `apply_matrix_3x3_f32` is column-major.
- **A second `cam pdma` needs a cold boot.** `scratchpad coldflash.sh` / `bridge reboot` + fastboot flash boot_b.
- **`diag android`** leaves a ramoops marker so slot a's gate picks Android unattended.

**Tools:** `tools/camera-pull.sh`, `tools/camera-raw10.py`, `tools/camera-raw2vsf`; `cam pdma af= exp= gain= sweep=<a>:<step> view=<s> snap fast`.

**Next:** the other three cameras; a re-runnable capture without a cold boot; the exact exposure time (pixel clock) for absolute-IDT comparison with the Android captures; screen calibration with the owner.

## 2026-11-06 night: front captured and wired, front + tele still silent from ferros

Front (dokkaebi, hsi2c_2 G#10, ID G#30A1) fully captured hands-free (I2C init, ISP front end, PDMA list 3) and wired as `diag cam front up` / `cam pdma front` (FERROS_CAM_FRONT=target/cam/front; CSI link 0, csis0 lanes, SYSREG 500 = 1). It NACKs from ferros; camera-PMIC writes, voltages, pins and USI clocks all measured equal to Android's, so the sub-PMIC (ACPM) side is the suspect. Tele's init captured; it needs a bright scene to stream (5x is a main-sensor crop in dim light). Capture traps: ARM_AFTER_MS counts from the last reset; a clean reboot wipes the console zone (crash reset keeps it); read the zone from ferros (`bridge read FD3FF000 200000`, order from the write pointer at +4); the app always reopens the rear camera. Detail in docs/oracle/CAMERA.md. Flashing that night: [[husky-flash-paths]].

## 2026-11-14: the driver built (payloads/camera), not yet run

Owner: "Can we get Lumis wired up?" Built on branch worktree-agent-abba9f7999aaaa0a6 (CAMERA.md "Built"): the kernel's `camera` switch (camera_power.rs), the camera's frames as one run from the pool (SYS_DMA x1 = 3) with the three ISPFE S2MPUs fenced to exactly them, payloads/camera on demand, payloads/camtest headless, ferros_cam shared (job relocation, RAW10 binning, both host-tested on target/cam). Facts found on the way, don't re-derive:
- **A plain reply was capped at 256 KiB** (STORE_MAX): the 6 MiB preview frame could never cross. Now 8 MiB (REPLY_MAX), server to caller directly.
- **Lumis calls live**: a CALL_LIVE on an on-demand endpoint now waits for the server the kernel starts (it got ERR before).
- **All 22 outputs are written** (run-rate.txt), slots 15/16 to 16 and 8 MiB: the region is 78 MiB / 39 frames.
- Every address register's +4 is its high half (written 0) except G#1C35_4ECC's; whether the front end drives high bits in SYSMMU bypass is unmeasured (vault `cam.region` = `low` asks for frames under 4 GiB first).
- The EL3 poison suspect in the camera path is the direct read of PMU G#3EBC (pd.rs and fe_up read STATUS plainly before EL3 calls without harm); the switch sets the bit thru the PMU's atomic alias (G#FEBC, value 0), no read.
- ispfe is gated at boot (pd.rs); the switch brings it on with pd.rs `on` (the restore = the prepare call) and marks it prepared, and the gating skips it once the switch has.
Deploy: /mnt/Harbor/tmp/camera/deploy.sh backup|store|reload|test|restore (FERROS_PORT=1-2.2).

**Same day, on the Pro: frames flow thru payloads/camera** (boot b874, RAM-reloaded kernel 5c295fc8-era): first frame 2278 ms after OPEN (the init's 2.1 s of I2C is the wait), 67.3 ms a frame served at bin 2, SPI 479 fires for every frame, all 22 outputs written, S2MPU faults 0, 2.84 W streaming vs 1.38 W idle. Traps found: the sensor bus reads not-ready until the module is powered (lines held low); the PDMA index must never go back to 0 (the front end stops for good after the 1024th descriptor), so the driver gives the engine its own 2^14-descriptor ring; reading the NC region needs 64-byte loads (8-byte: 121 ms a frame).

**Later the same day:** a stowed kist (the stand-in Lumis front) took frames through the endpoint (first frame 2276 ms after the open); the controls work mid-stream (exposure a quarter: the mean 134 to 81 the next frame); binning on four threads keeps the governor mostly at rest; and the `diag cam` harness is gone from the kernel (c5f24d02): to experiment with the camera now, change payloads/camera and restart it, never a kernel tool.

## 2026-11-15: the other cameras in the driver (built, not run; the Pro away)

Owner: "would be nice to get the other cams online." Branch worktree-agent-a072c735fcd747099, docs/CAMERA.md "The other cameras" (the device test plan and the tele's capture recipe are there). ferros_cam cams holds each camera's facts by id (0 main, 1 ultrawide, 2 front, 3 tele); the `camera` switch takes the id as its module in SYS_POWER bits 8-15 (ferros_sys `power_arg`); grants hsi2c3/2/4 are the driver's slots 10-12; CAM_VA widened to 224 MiB; PROBE (op 7) powers a module, reads its identity, maps the acknowledging addresses.
Facts found, don't re-derive:
- **The ultrawide's job keeps each output's offset in its 16 MiB granule** (the later image builder), unlike the main's: ferros_cam job has a per-slot skip; slot 8 runs on into 9 (they stay adjacent). Its outputs were identical in four runs (target/cam/run-uw1..4.txt).
- The ultrawide's line time from its init: 9568 clocks at 24.576 MHz / 3 x 178 = 6.5616 us, 5081 lines = 33.3 ms (the 30/s seen). Lumis's UW averages are 43 frames: black 2752 = 64 x 43.
- **Front/tele NACK suspect: sub GPIO2 = CAMS_LVBUCK_EN ("for ldo8")**, raised only by the main's sequence; cold ferros reads G#40 (low); their power-up never touched it, so the 11-06 register comparison could not see it. Their sequences now raise it. Unmeasured.
- The slg51002 here is REVISION_AB (pattern ID 03 31 B1): its GPIO switches need no SW test mode.
- The worktree does not build in place (valhall crate collision with another worktree): build in /mnt/Harbor/tmp/othercams/ferros (siblings symlinked, plus the gitignored oracle/display/husky-hk3/cold-power/*.bin copied in).
- **Each camera's mosaic, levels and orientation are in its 2026-10-21 calibration capture** (`pixel 8 pro calibration data/*average.vsf`, local, a fingerprint: read only the numbers): main G B/R G, ultrawide and tele B G/G R, front G R/B G; all 64/1023; EXIF 6 rear, 8 front. Their sizes match ferros's inits and every crop start is even. Read these before guessing any camera fact (the owner had to point at them).
- **The list's IDTs come from the vault** (`cam.<key>.idt`, the repo's .idt.vsf as is) since 2026-11-15: without `cam.boitata.idt` stored the main is not listed.

## 2026-11-15: modes, the bench and the slit (built on branch camtrials, not run; the Pro away)

Owner: "straight as fast as possible full frame 1:1 and 2x binned for all back cams integrating into the buffer ... Then I want the slitscan wired up". docs/CAMERA.md "Modes, the bench and the slit" has the per-camera table, the arithmetic and the run plan; capsules in /mnt/Harbor/tmp/camtrials/out (build copy /mnt/Harbor/tmp/camtrials/ferros, `build.sh`).
Facts found, don't re-derive:
- **October's slit wrote no hold**: `w16(G#0104, 1)` writes G#0104 = 0, G#0105 = 1 (the release), and left the init's 21342-line integration in 68-576-line frames. "One geometry change a session" and "never starts before stream on" may be that, not the sensor; the driver now writes modes inside the real hold, integration inside the frame.
- **The main's CCS addresses are 2 a pixel**: 16320 x 12288 for 8160 x 6144; binned increments 4; Samsung's G#0900/G#0901 are G#32 G#44 (the tele's G#22 G#22), not CCS's meaning. The ultrawide (Sony) bins in G#0900-2 (1, G#22, 8) with a digital crop (312, 4). The tele's addresses are 1 a pixel (8064 x 6080), 2 lanes.
- **The front end's sizes for loch0 and the link come from the CPU's 'after' writes** (G#1C32_0474, G#1C32_2074, link +G#44/54/64), not the PDMA program; the program holds the strides (G#1C32_0554, 2154). The program's first 40 words and last 11 are loch0's DMA setup and the go bits: the slim per-frame program (176 bytes of 6620).
- **The camera window and view moved** to their own GiB, G#51_0000_0000 (512 MiB each).
- **cargo el0 of camera and cambench together fails** (feature unification: ferros_cam's `list` brings vsf's alloc into cambench): build each package on its own.
- No aarch64 emulator on the host: NEON code is tested by a lane model on the host and checked against the plain path on the device (cambench does it on its first frame).
**Run on the Pro 2026-11-16 (b897 forge, b898 camtrials reload; CAMERA.md "On the Pro, 2026-11-16"):** main binned sensor floor 96 fps (about 3230 lines, it pads itself), integrate u16 at 97 fps on 3 A715s (7.2 ms a frame); ultrawide front end 48 fps (needs about 128 blank lines; 32 gives nothing). Never pin the camera's frame thread (X3 or one A715 both killed 96 fps streams); consumers leave one A715 free, or (kernlock) switch `dev first` and `dev core 7` on: [[kernel-lock-device-threads]]. Slits: sensor 1169/s but the main's link gets ERR_CRC_PH (+G#14 = G#20) whenever the y window changes thru CCS alone; the ultrawide's link is clean but its front end stops after 2 frames. 1:1 by CCS alone fails on both (main CRC_PH; Sony keeps its binned line width). No RAW12 on either (main 0A0A/0A08/0808, ultrawide adds 0E0E). The switch's power cycle at close unsticks the sensor: no cold boot needed after any of it. Reload trap: the hv's counters at G#FD60_0D00 survive a jumped session's reset; clear word 0 before a reload or the kernel stands down as jumped.
**Gain wired 2026-11-17 (CAMERA.md "Gain"):** the main's own CCS words: gain = code / 32, codes 32 to 5120 (1x to 160x analogue), digital 1x to 128x whole steps; its init leaves 32x (code G#400). Linear from 8x (2x signal a doubling) and thru digital. Its capability word reads 2, not 0. The list's gain range is a table (ferros_cam ccs ARRAYS), measured, since LIST runs with no sensor powered; the ultrawide's is still unmeasured (the flashed kernel can't power it).
**Gain noise test 2026-11-17 (b915):** main analogue really to 160x (no empty codes, dark noise x1.67 64x->128x and x1.10 128x->160x, not x2/x1.25), ceiling 1024-163840; ultrawide Sony 1024/(1024-code), 1x-64x, analogue to 64x (dark noise x4.8 from 8x to 64x), ceiling 1024-65536. Digital gain never written above one (owner: after the ADC). The bridge console drains on read and can drop lines under load: gather it often.

## 2026-11-17: long exposures (branch camera-long-exposure, built, not run)
Owner: Lumis on ferros to expose as long as on Android (16 s on the main). CAMERA.md "Long exposures". SET exposure now plans the timing (ferros_cam ccs `Timing::plan`): inside the mode's frame the integration alone; past it the frame length in the same hold (main to 0.211 s); past that the line length on a 2^4-clock step, capped at the line time the front end took (15.6 us: 1.02 s on main and ultrawide; on b932 the main ran 1 s with the line moved mid-stream and stopped at 2 s, 31.3 us lines); past that the vendor shift, `Lte`, empty until the capture. The margin is the sensor's own G#1006, read at each open (main 2, ultrawide 64). Facts, don't re-derive:
- **Android's per-frame hold on the main writes G#0702 and G#0704 (both 0) beside G#0202 and G#0340** (cam-open3): the shift words' addresses come from a capture; their long-exposure values need the 16 s capture (tools/husky-hv-longexp.sh).
- **The driver's frame wait was 2^21 us flat** and its holder deadline counted from the call: either would have closed any frame past about 2 s. Both scale with the frame period now.
- **Hand-spawn for tests** (no store): the driver serves `camera` while it runs, and each close powers the module down, so a wedge costs a driver restart; stop at the first open whose frames stop (2026-11-16: three stuck opens, then a Pro reset).
- The hv oracle can log one I2C target alone (`FERROS_HV_ORACLE_I2C_TARGET`); `tools/oracle-i2c` is the Rust decoder (`--holds`).
- **The main's shift, captured (2026-11-17 evening, Lumis on Android at 16 s, integrity 0):** G#0702 = G#0704 = shift << 8 in every hold, integration and frame counted in 2^shift lines (frame = integration + 36 there), least shift that fits (3, then 6 at 16 s), mid-stream, no stream off/on, no line change. Built as `ARRAYS[0].lte`. The capture also holds the sensor init of Android's 8160 x 6144 mode (keys/husky/captures, local). Open: whether b932's 2 s stop was the 31.3 us line or the 2 s frame.
- **16 s runs on ferros (b933):** the main's ladder to 16 s, shift 1-6 mid-stream, frames the period apart, the sensor reading back shifted units; b932's 2 s stop was the 31.3 us line, not the frame. The main's real line is 3.1489 us (24.576 MHz / 3 x 245 at 6320 clocks), not 3.2237: every main exposure reported before was 2.3 % long; MAIN_LINE_PS corrected (CAMERA.md lists what read long). camtest's sixth byte is the ladder's top: `04080005000E` for 16 s.
- **Owner: the readout stays the mode's** (line never stretched by default; `Timing::stretch` off). Main: frame to 0.206 s, then shift at line 6320; shifts 7 (16 s) and 8 (32 s) ran (b933), `lte.most` 8, 52.8 s offered. The front end's line fires at DMA_DONE (SOF+EOF latched, G#13) and SHADOW_UPDATE (G#8) 150 us later: no SOF time; a frame-thread watch must answer SHADOW_UPDATE before it sleeps or the next frame never ends.
