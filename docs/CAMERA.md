# The camera as a service (Lumis on ferros)

The bring-up record, register by register, is docs/oracle/CAMERA.md.
This is the other half: what a kist needs from the cameras, the endpoint it calls, and how far ferros is from Lumis showing a live preview.
Started 2026-11-12 (the owner: "would be nice to have ... lumis fully working in the morning").

## Built (2026-11-14): the driver, not yet run on a device

The owner, 2026-11-14: "Can we get Lumis wired up?", taken as the go on "The driver" below. Everything here is built and host-tested; nothing has run on the Pro yet (it needs a kernel flash, which the owner's tag attends).

- **The `camera` switch** (kernel camera_power.rs): both camera buses, the main module's power with the rails behind the overvoltage guard, the front end's domain thru EL3 only (pd.rs `on`: its configuration and the secure restore, the pair of the boot's gating save; never a PMU read, the EL3 poison pattern), `fe_clocks` (VDD_CAM, ISPFE_NOC, CMU_ISPFE, SYSREG_ISPFE), the D-PHY isolation bypass once a boot (thru the PMU's atomic set alias, as Linux sets it, with no read), the SYSMMUs in bypass, the S2MPUs fenced. Off: the sensor's stream stopped if it still runs, the engine stopped and Linux's close replayed when a job was given (camera_close.rs), the S2MPUs shut, the module down. The domain stays on once a boot has opened the camera (its off/on cycle is not trusted yet, pd.rs). The torch's switch and the `cam` tools refuse while it is on, and it refuses while the torch's is.
- **Frame memory from the pool** (process.rs `cam_ask`, `cam_free`; frames.rs `take_run`): the driver asks for one run of frames side by side (SYS_DMA x1 = 3), taken from the top of the pool (under 4 GiB if asked), zeroed and cleaned out of the caches, mapped non-cacheable at cachemap `CAM_VA`; the three ISPFE S2MPUs admit exactly those frames for every VID (s2mpu.rs `fence_cam`) and are shut before the frames go back wiped when the driver ends (both end paths). Their faults are in the plain DIAG (`S2MPU_CAM_FENCE`, a `S2MPU_FAULT_*` group each) while the domain is on.
- **The outputs, measured** (October's per-slice report, target/cam/run-rate.txt, every slice patterned and sampled every 256 bytes after a ten-frame run): the main camera's job writes **all 22** of its outputs (October slots 0 to 21) and none past them. Slot 2 is the image body (15 MiB), slot 0 the PD path (3.75 MiB), slots 1, 3, 7 and 20 a 16-byte header each, slots 15 and 16 written to the end of their 16 MiB and to 8 MiB (data of one kind: 15 may run on into 16), the rest statistics of 6 KiB to 1.05 MiB. The region gives each the power of two at or past twice what was seen (the frames exactly theirs), in slot order, 15 and 16 side by side: **78 MiB, 39 frames** (ferros_cam job `MAIN`). The driver patterns the outputs at each open and says on its console after the eighth frame which were written and how far, so a run re-measures it.
- **The job, moved in Rust** (ferros_cam job, replacing what tools/camera-pdma-image.py did for fixed addresses): the October 'CPR3' from the vault (`cam.boitata.pdma`) with the ring's list addresses, the twelve block loads' sources and the 25 address registers moved into the region. Every address register's +4 is its high half, written as zero by the program (read from the main camera's list: 24 of the 25; G#1C35_4ECC's +4 holds G#C000_0000, so not there), so the region may lie past 4 GiB; whether the front end's masters drive those bits in bypass is unmeasured, and the fence is what makes the first try safe (a write that loses them lands outside the frames and faults). The driver asks for a run under 4 GiB first (below).
- **`payloads/camera`**: started on demand by a call on `camera`, ended on CLOSE, when its holder's frame reply fails, or after 64 frames nobody asked for. An open is `cam pdma`'s order (region, switch, identity, init, job placed and patterned, the CPU's writes before, engine, receiver last, the writes after, lens, stream). A frame thread answers the front end on SPI 479 (zuma.h IRQ_ISPFE_0, which carries the CSIS core's events) instead of the kernel's spin; if no interrupt comes within the front end's own deadline it answers on a 2^15 µs tick and says so with the status words. FRAME bins by copy at 2, 4 or 8 (ferros_cam raw, matched sample for sample against std/camreplay's binning of the recorded frame); SET takes exposure (lines inside the hold), the captured gain only, and focus G#0800 to G#7800.
- **Two kernel changes the endpoint needed**: a plain reply was capped at a store transfer (256 KiB, process.rs `SYS_REPLY`), so the preview frame (6 MiB at bin 2) could never have crossed; a reply past it now goes from the server's memory straight into the caller's, up to 8 MiB (`REPLY_MAX`). And a live call on an on-demand endpoint (`torch`, `audio`, `camera`) now waits for the server the kernel is starting instead of ERR (KERNEL.md "CALL_LIVE"): Lumis calls live, and its first call would otherwise always find the driver not yet started.
- **Lumis's side**: the camera right (code 9) is on forge in the kernel (process.rs), the stower and the map (ferros_kist RIGHTS, the map's right names); Lumis's own parser reads the driver's list (its `main` camera with the Absolute IDT). Nothing in Lumis changed.
- **`payloads/camtest`**: the endpoint driven headless (LIST, OPEN, frames at one bin, CLOSE), for the time to the first frame and the frames' spacing without anything on the screen.

**To run it on the Pro** (each step after the owner's go; back up first, /mnt/Harbor/tmp/camera/back/):
```
B=target/debug/ferros-bridge
$B store put cam.boitata.pdma @target/cam/cam.pdma      # the main camera's job (local, the vendor's settings)
$B store put cam.frame @target/cam/cam.frame            # the init and the CPU's writes, if the vault lacks them
$B capsule <build>/camera.bin target/camera.capsule.vsf && $B store put camera @target/camera.capsule.vsf
FERROS_PORT=1-2.2 FERROS_HV_CORE=1 FERROS_HV_UFS=1 FERROS_BOTH_AT_ONCE=1 tools/husky-cold.sh   # the kernel
$B spawn camtest.bin 4008 --name camtest --use camera   # 64 frames at bin 8
$B diag console; $B diag                                # the driver's lines; S2MPU_CAM_FENCE and its faults
```
**On the Pro, 2026-11-14: frames flow** (this branch's kernel RAM-reloaded, boot b874; the driver and camtest stored; nothing on the screen):
- The first try refused at the switch: it asked whether the sensor's bus was ready before the module was powered, and its lines are held low until then (gpp2 DAT G#0, the master busy). It now asks after the power, as `diag cam up` did.
- The region: 40 frames under 4 GiB at G#CD00_0000 (the first run G#CD20_0000), taken and cleaned in 3.4 to 5.6 ms, with 9.1 GiB of the pool free.
- The open: switch on in 43 ms; the sensor's init (1347 writes, 2 reads) in 2095 ms, the whole of the wait; streaming 2173 ms after the open, the first SHADOW_UPDATE at once, the **first frame 2278 ms after the open** (105 ms after stream on).
- **SPI 479 fires**: every answer of every run came on the line (2555 of 2555 in the longest), none on the tick.
- **All 22 outputs written** in the new region, each with October's extent (the image body to +G#EFFF00, the PD path to +G#3BFF00, slots 15 and 16 to 16 MiB and 8 MiB).
- **S2MPU faults 0** on all three, the camera's fence 1, in every run.
- **The rate**: the front end takes a frame every 67.3 to 67.7 ms (14.8 a second), October's rate. Served at bin 2 (6 MiB) every frame, 67.3 ms apart, 1530 in one open; binning 36 ms a frame (14.4 ms copying out of the non-cacheable region with 64-byte loads, the rest at opt-level 3). It first took 121 ms (8-byte loads) and 66.8 ms (opt-level "s").
- **The ring**: the job's 1024 descriptors, with the index written back to 0 after the last, stopped the front end for good after frame 1025. The engine now gets its own ring of 2^14 descriptors (about 18 minutes) in a frame past the job's region; the count register holds it.
- **Power**: 1383 mW idle (the clocks at rest), 2836 mW streaming at bin 2 with every frame taken (the clocks at their active profile throughout), over the same 8 s window.

**On the 8a, 2026-11-14, against std/camreplay** (this branch's kernel RAM-reloaded, nothing stored): with no server and no capsule, a call on `camera` started the on-demand path, which found no capsule and answered the waiting call ERR in 1 ms (no hang). With camreplay serving, camtest took 32 frames of 6266944 bytes at bin 2, 68.0 ms apart (camreplay's own 67.8 ms), mean sample 70: the 8 MiB reply crosses whole. Lumis, launched by hand with a kist's slots, listed the camera, opened it at bin 2 and showed the recorded frame developed through the main camera's Absolute IDT, upright, its controls under it (shutter 1/15 s, gain 1.00x, focus G#3800, display -4 stops); camreplay served 110 frames per 8 s (13.75 a second) and the display showed Lumis's window about 9 times a second. Not measured: the kernel lock's hold for a 6 MiB reply. The waiting half of the first-call change (a live call held until a capsule's server starts) needs a stored capsule and was not run.
The region is asked under 4 GiB first by default (coordinator, 2026-11-14: what October's runs proved; `cam.region` = `high` asks higher first). The Pixel source does not settle the high halves: Linux's ISP DMA always goes thru the SYSMMU, whose window samsung-iommu-v9.c clamps to 32 bits, so the halves are always zero under Linux. On a running Pro the low ranges are likely full of arcs (the 8a's were), and the driver then takes a run higher and says so.

**A kist, end to end** (the Pro, the same day): the stand-in Lumis front, stowed by the owner thru `ferros send` and granted the camera, listed the camera, and a tap on "main (back)" brought its first frame (2040 x 1536 at bin 2) 2276 ms after the driver's open, 15 ms after the front end's. The front then ended on its own memory (its arc refused growth past its ceiling, "memory allocation of 6266880 bytes failed": Lumis's, not the driver's; the real Lumis is being ported onto fluor instead). The driver saw 64 frames go unasked, closed, and gave its frames back, the S2MPUs still at 0 faults.
**The controls, mid-stream** (payloads/camtest `60080001`, bin 8): exposure to a quarter (17.2 ms) took the mean sample from 134 to 81 on the very next frame (70 to 17 above black: 0.24 for 0.25) and back to 133 the frame after the restore; gain 2048 answered 1 (outside the captured code), 1024 answered 0; focus G#2000 taken; 96 frames 68.1 ms apart, the stream never stalled. Not measured: the lens's movement (sharpness, or the actuator's position at 84).
**The binning on four threads** (one for each A715): 17 to 19 ms a frame; the clock governor at rest for 7.3 of 10 s streaming (was none), the A715s at a mean 639 MHz (was 1062).
**The harness went** the same day (KERNEL-OUT.md decision 3): every `diag cam` tool, the probes, the PDMA runs and the viewfinder, camera_events.rs, camera_snap.rs, the ultrawide's and front camera's receiver tables and the FERROS_CAM_* blobs. camera.rs keeps what the switch runs; the record of how it was found is docs/oracle/CAMERA.md, the code forge's history before that commit.

## Where it stands (2026-11-12, measured)

**Lumis's side is built.**
- Lumis's ferros front: `rust/lumis-ferros` on Lumis's `ferros` branch (a worktree at /mnt/Harbor/Code/lumis-ferros), built by tools/lumis-build.sh, 3888368 bytes.
- It asks the `camera` endpoint for the list, opens a camera, takes frames, develops each into the panel's pixels through the sensor's own Absolute IDT (linear VSF RGB, then fluor's 16-bit g2t pixel; no sRGB, no white balance, no auto exposure), and sends the manual controls (shutter, gain, focus; display gain is its own).
- Each way there is no camera is its own sentence: the right not given, no driver serving the endpoint, a list that does not read, an empty list.
- Host tests: the list, the frame header, the development (white lands at diffuse white, below black is black, the mosaic goes to its channels, a quarter turn) and a real 4080 x 3072 RAW10 frame from the Pro (target/cam/img.bin) developing to a lit picture.
- **Run on the 8a** (2026-11-12, spawned by hand with a kist's slots, the endpoint unserved): Lumis's page, "No camera driver runs on this device yet.", seen in a screen capture; a tap asks again and it keeps running. The preview and the controls have run on no device: there are no frames to give them.
- tools/lumis-kist.sh signs it with the owner's developer key, declaring the camera; the capsule signs (3898347 bytes).

**The camera right is built** (code 9, asked; KISTS.md "The permission list").

**Nobody serves the `camera` endpoint.** There is no camera driver process, on either device.

**husky: the cameras work from the kernel's diagnostics only, and those cannot run beside today's shell.**
- What the harness does (docs/oracle/CAMERA.md): the main camera at 4080 x 3072 RAW10, about 14.5 frames a second, frames in DRAM, focus, exposure; the ultrawide at 4000 x 3000, about 30 a second; a viewfinder the kernel paints on the panel at 0.8 frames a second; the front camera and the tele NACK.
- Its inputs: the main sensor's init from the vault (`cam.boitata.init`), the front end's CPU program from the vault (`cam.ispfe.prog`), the PDMA job built into the kernel from a local file (FERROS_CAM_PDMA=target/cam/cam.pdma; the vendor's settings, never in the repository).
- **Its output memory is now other things' memory** (read from the code, camera.rs and cachemap.rs; nothing was run): the PDMA job's 32 output slices are 16 MiB each from G#A000_0000, so G#A000_0000-G#C000_0000.
  That is the frame pool's second range (`POOL_BIG`, G#A000_0000-G#B400_0000: the processes' own memory since 2026-11-11), the GPU's memory (G#B400_0000-G#B600_0000) and the secure carveout from G#B600_0000 (the slices there "read as zeros", CAMERA.md 2026-10-21).
  The run patterns every slice before it starts, and the focus sweep copies to G#9800_0000, the layer arena.
  So `diag cam pdma` on a kernel whose shell is running would write over live processes, the GPU's rings and the arena.
  It was never run on a running shell, and since 2026-11-14 it is gone with the harness.
- It runs inside the kernel's service loop and spins there for the whole run, answering the front end every 15 to 50 ms: nothing else in the system is served meanwhile.
- It adopts the panel itself (`display::adopt`), which the display server owns now.
- Whether the Pro's running kernel even carries the PDMA job is unchecked tonight (a build without FERROS_CAM_PDMA refuses with "no PDMA job built in"): no camera command was run on the Pro.

**akita: the three cameras answer from ferros; none streams yet** (2026-11-15, "akita's cameras" below).

## akita's cameras (2026-11-15)

The owner, 2026-11-15: "Might not be a bad idea to work on the 8a cams whilst it's plugged in." Branch akita-cams.

**From its device tree** (the factory image's MP overlay, dtbo 18, the same as /mnt/Harbor/ferros-ref/kdev-akita/dts/zuma-akita-camera.dtsi and -camera-pmic.dtsi), in ferros_cam cams (`AKITA`, chosen by fact 84 in the driver and by the board in the kernel) and camera.rs (`AKITA_MAIN`, `AKITA_ULTRAWIDE`, `AKITA_FRONT`):

| id | name | module | sensor bus | power-up (device tree order and delays) | reset | master clock |
|---|---|---|---|---|---|---|
| 0 | main | oksoko | hsi2c_1 G#1A | sub LDO12, LDO5, LDO4, LDO1, LDO3, sub GPIO2 (1 ms), reset, clock (2 ms) | gpp2-2 | gpp9-0, CIS_CLK3 |
| 1 | ultrawide | leshen-uw | hsi2c_3 G#1A | sub LDO12, LDO2, LDO6 (1 ms), reset, clock (1 ms) | gpp6-3 | gpp3-0, CIS_CLK0 |
| 2 | front | leshen | hsi2c_2 G#1A | sub LDO12, LDO2, LDO7 (1 ms), reset, clock (+1 ms of ours) | gpp4-2 | gpp5-0, CIS_CLK1 |

- **akita's rails are not husky's**: LDO1 2.9 V, LDO2, 4 and 5 2.8 V, LDO3 1.8 V, LDO6 and 7 1.2 V, LDO8 1.1 V (`RAILS_AKITA`). husky's table would have put 2.25 V on akita main's 1.8 V LDO3. Android's /sys/class/regulator reads exactly akita's table.
- Sub LDO12 (L12S_CAMIO, the modules' I/O and nothing else) is read before its enable and held to a tenth over 1.8 V: cold it reads G#AE, 1.85 V (Linux sets 1.8). The camera PMIC goes on only as REVISION_AB (pattern 03 31 B1, as husky's); akita names no sub GPIO3, so it is never driven there; the RT6160 buck-boost (G#77, hsi2c_9) is not registered under Linux at all and is left as the boot has it.
- Every master clock pin at drive X2 (1). The clocks stay on the 24.576 MHz oscillator (CMU_TOP's CIS muxes and dividers read 0 cold): the device tree asks 26 MHz of CIS_CLK3 and 24 MHz of the others, but every init Android sends tells its sensor 24.574 MHz (EXCK_FREQ G#1893), so Linux runs them on the oscillator too.

**PROBE on the 8a** (b232, the akita-cams kernel RAM-reloaded, the driver stored as `camera`, camtest `00000007<id>`):
- main: G#0787 at G#0016 (Sony IMX787), only G#1A acknowledged (its "nessie" lens/OIS/EEPROM at G#24 did not);
- ultrawide: G#0712 (Sony IMX712), its EEPROM at G#50;
- front: G#0712 (Sony IMX712), its EEPROM at G#50.

**Android's Camera2 characteristics** (`dumpsys media.camera`, local: /mnt/Harbor/tmp/akitacams/android-dumpsys-camera.txt, the colour matrices in android-matrices.txt; model-tier seeds for each IDT until a spectral calibration):

| camera | pixel array | CFA | black / white | orientation | exposure | focal, aperture |
|---|---|---|---|---|---|---|
| main (physical 2) | 4624 x 3472 | RGGB | 64 / 1023 | 90 | 59.9 µs to 16 s, ISO 50 to 9846 (analogue to 615) | 5.43 mm f/1.89 |
| main 2x (physical 4) | 4624 x 3472, half the main's sensor size | RGGB | 64 / 1023 | 90 | from 84.7 µs | the same lens: the unbinned centre crop |
| ultrawide (physical 3) | 4208 x 3120 | RGGB | 64 / 1023 | 90 | 12.4 µs to 16 s, ISO 49 to 4729 | 1.835 mm f/2.2 |
| front | 4208 x 3072 | BGGR | 64 / 1023 | 270 | 12.4 µs to 1 s, ISO 48 to 6095 | 2.74 mm f/2.2 |

**The captures** (Android under oracle hv builds on slot b, tools/akita-hv-camall.sh, decoded with tools/cam-oracle; all local in /mnt/Harbor/tmp/akitacams, the vendor's settings):
- The sensor inits, one bus a capture, each clean inside its init (the only length mismatches were the main bus's G#24 controller and each session's last stream off): main 277 transfers to stream on (4624 x 3472, three lanes, a 9568-clock line, a 4826-line frame: 7.036 µs a line, 29.5 frames a second), ultrawide 140 (4208 x 3120, four lanes), front 119 (4208 x 3072, four lanes, flipped both ways, G#0101 = 3). `inputs/oksoko.init`, `leshen-uw.init`, `leshen.init`.
- The receivers and jobs (the light build: the PHY, the CSI links, SYSREG_ISPFE and the PDMA trapped, the PDMA dumped): the main runs CSI link 1 and the csis1 lanes, its receiver husky's main's but for link 1's sizes (`rx::RX_START_AKITA_MAIN`), SYSREG select 2, the line G#16C0; the front runs link 6 and the csis3 lanes (`RX_START_AKITA_FRONT`, its captured stop `RX_STOP_AKITA_FRONT`), SYSREG G#60, the line G#14C0. PDMA jobs: the main's twice (lists of 6656 and 6672 bytes), the front's (6220).
- The main's job puts the PD output at the start of a 16 MiB IOVA granule and the image 5 MiB into it, and the image is 20.2 MB: October's one-granule-a-slot mapping (cam-oracle pdma-image, as camera-pdma-image.py) does not fit it as it stands.
- **The jobs, laid out** (desk work after the captures): `cam-oracle pdma-image --own-image` gives the image body slot 2 alone, at its start, so the main's PD output (G#1C32_21C0) moves to slot 1, and drops a last record that runs past the buffer (the main's list 1 ends in four stale words under a header G#0004_0000, which ferros_cam job's walker refused as truncated; the engine stops at the buffer's length, so it never ran them).
  Each slot then runs from the lowest offset the job names to its granule's end in whole frames, until a first run measures the outputs (ferros_cam job `AKITA_MAIN_SLOT_BYTES` / `_SKIP`, `AKITA_FRONT_SLOT_BYTES` / `_SKIP`); the local jobs (target/cam/akita-main.pdma from the main's list 1, akita-front.pdma from list 2) move into their regions in the host test, 98 MiB with a ring of one for the main (158 with four), 94 for the front (142).
  Unsettled: which of the main's two lists to run (list 1 at the first open, 6640 bytes after the cut; list 3 at the reopen, 6672; on husky one list reset the phone and the other ran), and the front's tail (its last nine words read as a burst header with no body and a record past the buffer: cut, but they may be a half-written list the dump caught, frame 16 being mid-update).
- The ultrawide's session ran link 0 with the csis0 lanes, SYSREG select 3 then 1, and its first writes stopped the PDMA engine (control 0) and reset the main's link; the engine did not start again until the front camera opened. So its frames reach memory some other way than the PDMA program (the front end's core pages, not trapped in the light build, or another block): its path needs the late-arm capture of an ultrawide session.
- **IDTs**: the Camera2 matrices (ColorMatrix1/2 as colorTransform1/2 under StandardA and D65, ForwardMatrix1/2, and the unit's own calibrationTransform) give a model-tier Absolute matrix (camera to XYZ by inverting ColorMatrix2, then vsf's XYZ2VSF_RGB), but no sensitivity, and the endpoint's list wants the Absolute entry's scale (ferros_cam list): none was written, so as not to invent a sensitivity. The owner's calibration (as the Pro's 2026-10-21 captures) gives both.
- The full trap of the front end's 42 pages (husky's recipe) hard-locked a core 24 s into Android's boot ("Watchdog detected hard LOCKUP on cpu 4"): the camera stack's boot probe, trapped access by access. FERROS_HV_ORACLE_LATE (ferros_hv) now traps the pages only from the arm time on; its first capture was flashed when the host's USB controller died (below).

**Left, in order**: the front end's CPU programs with the late-arm build (main, then front, then the ultrawide's path); the main's job laid out for its 20.2 MB image (the image granule shared with the PD output); per camera slot sizes, acknowledgement and tick (the first run's report measures the outputs); `streams: true`; the vault inputs (`cam.oksoko.*`, `cam.leshen.*`); camtest, the view; IDTs (model tier from the matrices above, then the owner's calibration).

**The 8a as left (2026-11-15 afternoon)**: the host's USB card (xhci 0000:04:00.0, the ASM107x card both devices sat on) died at 11:48:24 right after the late-arm build was flashed, so that capture never ran; the owner reset the card, and slot b was flashed back from `/mnt/Harbor/tmp/akitacams/back/boot_b.img` and read back from Android (sha256 fc572ff3, the same), ferros through the gate, attested. Slot a is the bare ferros (BOARDS.md "akita's slots"); nothing camera is in the vault; Android's settings changed for the captures (stay on while plugged in, Pixel Camera's location answer) are put back. The next capture session waits for the 8a on a motherboard port, so a card crash cannot take the Pro with it.

### akita: the captures of 2026-11-18 (Android 17 on slot a, rooted, under the hv)

Android updated itself into slot a that day (CP3A.261005.005, kernel 6.1.162), so the captures moved there: slot a's boot carries the oracle hv in front of its own kernel, and ferros's gate on slot b is never written (BOARDS.md "akita's slots").
`tools/akita-hv-camcap.sh` (SLOT=a, AUTO=1) ran every session hands-free over adb (the 8a has no lock screen): the image flashed to boot_a, a capture boot, the session's marks, a sysrq crash, the next boot's ring pulled twice and compared.
All local: /mnt/Harbor/Code/keys/akita/captures-2026-11-18/ (B3SUMS over every file; the vendor's settings, never the repository).

What Lumis 0.2.8 lists on the 8a: the ultrawide (4208 x 3120, to 16 s), "multiple Back" opening the main at 80 degrees (4624 x 3472, to 16 s) and the main's in-sensor 2x crop at 46 degrees (4624 x 3472 of 0.8 µm pixels, to 16 s), and the front (4208 x 3072, to 1 s).
Camera2 offers no maximum-resolution mode on any 8a camera (`dumpsys media.camera`: the main's whole array is 4624 x 3472), so the 46 degree crop is the only other main mode.

**The I2C sessions** (one bus a capture, the sensor at G#1A alone, nine set-up words quiet; one capture boot per session, since the main's binned and crop opens alone filled the 2^15-line cap with their frames' holds):

| session | transfers | integrity | inits (transfers before stream on) |
|---|---|---|---|
| i2c-main (Lumis binned, crop) | 4257 | 0 | binned 278, crop 289 |
| i2c-main-16s (Lumis, 16 s) | 1398 | 0 | 278 (the binned one, byte for byte) |
| i2c-main-gcam (Pixel Camera 1x, 2x) | 2231 | 1 | 1x 278, 2x 206 (a mode switch, no power cycle) |
| i2c-uw (Lumis binned) | 2234 | 1 | 140 |
| i2c-uw-16s | 1305 | 1 | 140 (the same) |
| i2c-uw-gcam (Pixel Camera .5, video) | 2385 | 1 | .5 140, video 143 |
| i2c-front (Lumis binned) | 3416 | 1 | 119 |
| i2c-front-1s | 1921 | 1 | 119 (the same) |
| i2c-front-gcam (photo, video) | 2782 | 1 | photo 119, video 155 |

Every length mismatch is outside every init: each session's last stream off (G#0100 written with two of its three bytes), and once a per-frame hold word in the main's Pixel Camera 2x stream.
The geometry and timing words, from the inits (CCS):
- main binned: the whole 9248 x 6944 array read to 4624 x 3472, a line of 9568 clocks, a frame of 4826 lines, PLL multiplier 166 (as 2026-11-15's);
- main crop (Lumis 46 degrees and Pixel Camera's 2x alike): x the whole width, y G#6C0 to G#145F (3488 rows), out 4624 x 3472, a line of 10320 clocks, frame 6011 lines, PLL 223;
- ultrawide: 4208 x 3120 of the whole array, a line of 5192 clocks, PLL 122; its video y G#178 to G#AB7, 4208 x 2368, PLL 102;
- front: 4208 x 3072 (y G#18 to G#C17), a line of 5192, PLL 119; its video 4208 x 2368 as the ultrawide's.

**Long exposures, the holds** (`<session>.holds`): the main (Sony IMX787) shifts its integration and frame by G#3100 (2, 4, then 6 at 16 s: integration G#8ACA and frame G#8B0A lines, each times 2^6, at its 7.036 µs line is 16.0 s), mid-stream, frame by frame as the Pro's Samsung main does with G#0702 and G#0704; the ultrawide and the front (Sony IMX712) shift by G#3004 (the ultrawide 2 then 5 at 16 s; the front 1 at its 1 s).
Gain G#0204 (analogue) and G#020E (digital, 1) ride in the same holds.

**The ISP sessions** (the late-arm build, FERROS_HV_ORACLE_LATE: no lock at boot this time):

| session | oracle lines | PDMA lists dumped | link, CSIS select |
|---|---|---|---|
| isp-main | 28696 | 2 | link 1 (G#1C05), 2 |
| isp-front | 24147 | 3 | link 6 (G#1C0A), G#60 |
| isp-main-crop | 20797 | 2 | link 1, 2 |
| isp-uw (the whole front-end span trapped) | 3029 | 1 | link 0 (G#1C04), 1 |

The main's engine is started (control +0 = 1) in isp-main; in isp-main-crop and isp-uw Android starts it another way (+G#28 = 1, +G#C = 1000), not yet read.
The ultrawide's frames do go thru a PDMA program this time (list 1 dumped), unlike 2026-11-15's session.
`cam-oracle cpu` (inputs/akita-main.cpr2: 134 CPU writes before the engine, 54 after; inputs/akita-front.cpr2: 132 and 36) gives the receiver starts too: the main's matches `rx::RX_START_AKITA_MAIN` word for word (2026-11-15's), the front's differs from `RX_START_AKITA_FRONT` in its lane resets and five csis1 base words.

**Streaming from a hand-spawned driver (2026-11-18, nothing stored):** the akita-cams kernel RAM-reloaded (b262; the flashed kernel has no akita camera switch and would run husky's rails), the driver spawned by hand with its inputs in its spawn (`cam-oracle cinp`, payloads/camera `spawn_input`: a 'CINP' set after the 16 knob bytes, read in place of the vault's, with `<key>.slots` in place of the compiled job layout), camtest `1008` (16 frames at bin 8).
- **main**: PROBE answers G#0787; the open runs to stream on in 610 ms (switch 41 ms, init 276 writes and 2 reads in 544 ms, job placed, engine started, receiver on, 188 CPU writes, none refused); CSI link 1 latches frame start and frame end on channels 0 to 2 (+G#24 = +G#2C = G#7), but the front end never answers: no interrupt on SPI 479, loch0's image status 0, not even SHADOW_UPDATE, in 2.1 s; the same with 2026-11-15's job, today's list 1 and today's list 2. So the link takes frames and nothing reaches loch0: where husky's main stood on 2026-10-20, but one step before (husky's raised SHADOW_UPDATE).
- **front**: the init runs (117 writes, 2 reads, 153 ms), then the job does not move into its region (`ImageWrites(0, 0)`: none of the lists, 2026-11-15's included, writes the image address register the relocation looks for), so the open is refused (3) before the engine.
- **ultrawide**: not tried (no CPU program cut from isp-uw yet, no receiver table).
Next: read Android's engine start of isp-main-crop and isp-uw; the hv snapshot (FERROS_HV_SNAPSHOT, docs/oracle/CAMERA.md) of the front end mid-stream under Android against `diag cam pdma`-style cold reads, as husky's ISPFE_NOC clock was found; which register the front's jobs name its image by.
(Answered on 2026-11-19, below: "Android starts it another way" and "the front's jobs name no image address" were both misreadings.)

### The 8a's main streams (2026-11-19, branch akita-stream)

akita-stream's kernel RAM-reloaded (b264 to b266), then flashed (b267); the driver hand-spawned with its inputs (`cam-oracle cinp`) until stored; camtest and cambench.
The driver now says why a stream stopped (`readback_say`, `words_say`, `copies_say`, the engine's log at the first answers): every register the program's first copy and the CPU wrote, read back; the PDMA's five contexts, loch0's words, the link's and its PHY lanes' words; the program's copies against the first; the engine's place and progress (+G#58, +G#5C) at each of the first answers.
A stream that stopped says them at its close; a device test asks for `words_say` at every close with the spawn's twelfth byte 1 (the eleventh on akita-stream; forge's eleventh names the spawn's inputs, so it moved at the merge, 2026-11-19).
Since the merge with forge the link and PHY pages it reads are the open readout's receiver's (`rx_of`, a captured readout's own on the Pro).

**The cause, two of them, both measured:**
- **Every 8a ISP capture lost oracle lines.** Read and write pairs with one half missing, each session different ones (the main lost G#1C32_0444, 0458, 0418, 00B0 = 1, 0478, 0470, 048C, 0488, 2018 and 8800; its crop session kept most of those and lost others; the PDMA's own `+0 = 1` start is missing from isp-uw and isp-main-crop, which is all "Android starts it another way" was). The PDMA dumps are whole (their lines carry offsets: no gap in any). The main's CPU program cut from isp-main therefore left loch0's image path unconfigured: the engine ran its first program (the program read back 1446 of 1447), and loch0 raised nothing. Merged by hand (keys/akita/cam-inputs-2026-11-19/main-after.txt: the crop session's order, the binned main's values, the pairs the 8a lost both times from husky's list; `cam-oracle prog-text` and `prog-build`), the first frame landed at once: SHADOW_UPDATE at stream on, SOF 10.9 ms later, EOF and DMA_DONE at 35.4 ms, a whole frame.
- **Slot 6 was laid out 14 MiB; its output is about 32.4 MiB.** G#1C35_51C8's output starts 2 MiB into its granule and ran on past the region's slots into the engine's area, over the program's copies (`copies_say`: copies 1 to 3 differed from copy 0 in 1100 to 1482 words from word 0). The engine then stopped 68 bytes into the next program it was given (+G#5C stuck at G#44 while the index ran on) and every later frame began and never ended (LATE_CONFIG on both paths). With slot 6 at 40 MiB the stream runs; measured, it is written to +G#206_BE00. The front's and the ultrawide's jobs name the same register (their slot 3 and slot 4): 40 MiB each there too.
- Not the cause, for the record: Android on the 8a enables SOF in loch0's mask (G#1B) and writes the next index after each SOF, where the Pro's release is at DMA_DONE. Both were run on the 8a once the copies were safe: both clean (69 of 69 frames each), so the driver keeps one path, the Pro's.

**Android's image format is not ferros's.** The 8a's jobs write loch0's image in 16-bit samples (G#1C32_0550 = 2, a line of G#2420 for the main), the PD path RAW10 packed (4, G#16A0). Every path of the driver (binning, the view, Lumis) reads RAW10 packed, so the stored jobs have the image's format word 4 and the line G#16C0 (`cam-oracle job-set`, `keys/akita/cam-inputs-2026-11-19/main-packed.pdma`), as the Pro's jobs do.

**Measured on the main** (IMX787, 4624 x 3472 binned, C-PHY on link 1):
- 29.5 frames a second (the sensor's counter 68 frames in 2.30 s; frames 33.96 ms apart), every descriptor released by the kernel at DMA_DONE, no error word, 64 of 64 asked frames clean, the image's mean 64.05 in the dark (black 64) and a scene at 16 s, 64x;
- the 2 x 2 cell R G / G B: the greens equal on the anti-diagonal (268 against 325 and 396 per sample over black, unsaturated cells, 16 s at 64x under the room's coloured light); red at (0, 0) from the IMX787's mosaic as the Pro's ultrawide measured it (B G / G R flipped both ways there, unflipped here: the 8a's init writes no G#0101), and Camera2's RGGB;
- long exposures thru Sony's shift G#3100 (ccs `AKITA_ARRAYS`): the ladder 64 ms to 2 s ran (frames 64.5, 128.4, 256.5, 511.8, 1026.3, 2052.9 ms apart). **The shift's ceiling** (camtest's fourth byte 10): the field is 3 bits wide: shifts 6 and 7 at 4800 lines gave frames 2.19 s and 4.38 s apart, 8, 9 and 10 frames at 34.2, 68.4 and 136.9 ms (read as 0, 1 and 2); 65471 lines of 2^7 ran, frames 59.02 s apart. The longest exposure is 65471 lines of 2^7, 58.96 s, now LIST's `exposure_max`. A forced shift whose frame is under the mode's own in shifted lines (76 lines of 2^6) stops the sensor; the plan never makes one;
- slits: the default 64-row binned slit (detached, slim) at 745 a second at the sensor, 740 at the front end; 65280 slits in 88.2 s with none lost (cambench); one earlier run died after 783 slits on an 11.2 ms gap in the interrupts (LATE_CONFIG), the Pro's slit death, not seen again in 69368 slits since.

**Stored and flashed** (the 8a's kernel vault, each read back alike): `camera` (this branch's driver), `cam.oksoko.init` (the binned init, i2c-main), `cam.oksoko.prog` (the merged program), `cam.oksoko.pdma` (list 1, RAW10 packed), and `cam.oksoko.idt`: the Pro's ultrawide's IDT (`cam3-ultrawide.idt.vsf`, the same IMX787 module family), a stand-in so the main is listed, until the owner's calibration of this unit (its lens is f/1.89 to the Pro ultrawide's f/2.2, so its absolute scale is off).
boot_b was saved first (keys/akita/images/boot_b-before-2026-11-19, blake3 8b9de8e1, read twice), then flashed with the same hv and Linux Image and this branch's kernel (`ferros-mkimg gate` over the old gate's own hv and Image, `avb-footer` with the old gate as template; blake3 808edcea, kept beside it). The 8a booted it cold (b267), the identity unlocked at boot, and camtest against the vault's driver (started on demand) took 64 frames clean.

### The 8a's front and ultrawide (2026-11-19): wired, no line arrives

Both are Sony IMX712s on D-PHY (four lanes): the front on link 6 and the csis5 and csis6 PHYs, the ultrawide on link 0 and csis0.
Wired the main's way: their programs merged from their lossy sessions (keys/akita/cam-inputs-2026-11-19/front and uw), the front's receiver merged from both captures and the ultrawide's from isp-uw (`rx::RX_START_AKITA_FRONT`, `RX_START_AKITA_UW`), their jobs RAW10 packed, the 40 MiB slot, their timing from their inits (lines of 5192 clocks at PLLs 119 and 122). The front's open had been refused because the driver asked a PD path of every captured-mode job and the front's has none (fixed: a PD path only where the program writes one).
Both now run to stream on, SHADOW_UPDATE applies their programs (the go bit G#1C32_041C read back consumed), and their links latch frame start and end on two channels (+G#24 = +G#2C = 3), with no error word. But no line reaches the link: its per-channel packet words (+G#6A0 on) stay 0 where the main's read G#1694_2B00 (5780 bytes of data type G#2B), and loch0 never sees a frame start.
Not it: the 8800 block (G#1C32_8800 = 1, lost by the front's session, written by the others), G#1C32_048C (the Pro's front writes G#200: no change), the lanes' G#x30 word (Android's G#137 on both; G#8, the Pro front's, gives the same; G#37 gives a storm of errors and frame starts on every channel, so that word does govern the HS reception).
Next: the hv snapshot of Android's front stream against ferros's reads of the same list (`tools/akita-hv-camcap.sh` image `snap`, sessions `snap-front` and `snap-uw`, SNAP_LIST keys/akita/captures-2026-11-19/snap-front.list: CMU_TOP's ISPFE_NOC mux and divider, CMU_ISPFE, the links, the csis0, 5 and 6 lanes, loch0). Built (target/agent-akitastream/camcap/snap.img); not flashed: the host's USB card died (xhci 0000:04:00.0, 00:54:25) while the script waited for the 8a's fastboot, with Android up on slot a and boot_a untouched.
The suspects it would settle: the ISPFE_NOC clock Android's DVFS runs these faster D-PHY sensors at (ferros sets 465 MHz, the Pro main's level), and any PHY or link word set before the late arm.
(Answered the same day, below: neither. Lines did arrive; loch0 listened to the main's link.)

### The 8a's front and ultrawide stream (2026-11-19, branch akita-dphy)

**The cause: loch0 listened to link 1.** G#1C32_048C is loch0's input word, and its bits 24 to 27 are the CSI link its frames come from. Every capture that kept the write says so: the 8a's main crop G#0100_0200 (link 1), the Pro's ultrawide G#0200_0200 (link 2), the Pro's tele and the 8a's front G#0600_0200 (link 6), the Pro's front and the 8a's ultrawide G#200 (link 0).
The 8a's front and ultrawide sessions both lost that write (as they lost others, "The 8a's main streams"), and the merged programs carried the main's G#0100_0200. So loch0 waited on link 1 while the front sent on link 6 and the ultrawide on link 0.
The links were never the problem. The PHY lanes' last-header words read RAW10 long packets on ferros (lane 0 G#2B, the data type; lanes 1 and 2 G#8C and G#14, the word count G#148C = 5260 bytes = 4208 pixels; lane 3 the ECC). Android's link words read the same as ferros's, and +G#6A0 is 0 under Android too, so the "no line" reading was wrong.
The fix is G#0600_0200 for the front and G#200 for the ultrawide, Android's own live values. The front then took 16 of 16 frames at once.
- **The ultrawide had a second fault: its job ran one word past Android's.** isp-uw's list 1 ends in a stray word G#1 after the last record. With it, the program is G#182C bytes where Android gives the engine G#1828 (its live PDMA +G#50). The ultrawide took one frame, then LATE_CONFIG (G#10 at G#1C32_0438): every later frame began and never ended. `cam-oracle job-len <in> <out> G#1828` cuts the program and every descriptor's length, and then it streamed.
- **Not it, measured:**
  - the ISPFE_NOC clock: Android streams both D-PHY cameras at the cold PLL_SHARED3_D1 over six (CMU_TOP G#2604_10BC = 1, G#2604_18B4 = 5), a quarter of ferros's 465 MHz. A kernel that set Android's clock from the switch on (RAM-reloaded, b271) changed nothing, so it was reverted; the front streams at 465 MHz.
  - every link, PHY, SYSREG and CMU_ISPFE word in the snapshots: equal to ferros's, but for status.
  - loch0's G#478 and G#47C (also lost, the main's values carried): Android's front G#7CA00 and G#15F0, ultrawide G#C44A0 and G#14F8. The front streams with the main's 478 and its own 48C. The stored programs carry Android's values all the same.
- **How it was found** (the hv snapshot, `tools/akita-hv-camcap.sh` image `snap`, sessions `snap-front` and `snap-uw`, all local in keys/akita/captures-2026-11-19):
  - The first list (whole spans, link 0 and the csis0 lanes read during the front's stream) hung the 8a mid-snapshot ("No kernel logs", nothing usable; snap-front-hang).
  - Lists cut to registers read safely before then ran whole: husky's 2026-10-21 snapshot list, plus the spans the driver's `words_say` reads while streaming. The second cut had 1288 and 1224 registers, the third 1450 and 1386 with loch0's further words and the blocks at G#1C34 and G#1C35. Every address came back in the list's order.
  - Ferros's side: the driver's `words_say` at the close, widened by its twelfth spawn byte (2: the frame converter's words at G#1C33, 3: loch0's, G#1C34's and G#1C35's), joined against the snapshot by address.
  - The diff left loch0's and the frame converter's status (FC ctx3 G#1C33_700C G#8F against G#10: requests pending, as on husky in October) and the LMP's counters (G#1C35_0014, G#1C35_001C zero on ferros). So frames reached neither block. The programs' writes against Android's live values then left exactly G#478, G#47C and G#48C.
- **The driver now says it**: an open whose loch0 input is not the receiver's link prints "loch0 takes link A#x ..., the receiver started link A#y: no frame can reach it".
  geom `value` no longer rewrites G#1C32_048C: a front slit turned G#0600 (half of 3072 rows) into a slit's 32 and sent loch0 to link 0 (`LOCH0_INPUT`, host-tested).
- **Facts found on the way, don't re-derive:**
  - Both IMX712s count their line in two clocks. The real line is 10.652 us on the front and 10.390 us on the ultrawide (frames 33.88 to 34.04 ms apart at 3187 and 3269 lines), twice what cams had. Every exposure before read half its time; `line_ps` is corrected in cams and camtest.
  - Both stream their whole 4208-wide array unbinned (G#0900 = 0) and write no digital crop. So ccs `crop` is false for them, and a captured mode's `bin` is 1 where a Sony init's binning mode is 0 (a slit refused "Missing(1032)" before).
  - Their margin is 10 lines: every Android hold sets the frame 10 past the integration. Their CCS G#1004 words read 0 and 0, so the driver takes the list's margin when the sensor states none.
  - The stored front's job is front-packed.pdma, so job `AKITA_FRONT_SLOT_*` is its layout: 12, 16 and 40 MiB, from offsets G#40_0000, G#8_0000 and G#E0_0000. The table was another list's, and the stored front would not move into its region (`Unplaced`).
  - About one open in four answers first with loch0's status 0. Twice that was the stream's only answer: the paths never came on and the line went quiet. Now, when the line is quiet with the paths off but the engine's place is at the index and its progress past the start, the driver enables them anyway. Six of six front view opens ran, one of them saved this way.
  - Lanes: csis5 (G#1C10_3100, clock lane, data lanes 3200 to 3500) and csis6 (G#1C10_3600) are released by SYSREG G#1C02_0500 bits 5 and 6 (the device tree's `sysreg_csis_reset`, one bit a PHY: the "select" word is the PHYs' reset). The ultrawide's csis0 is bit 0.

**Measured** (the flashed kernel b267's build, no reload; the driver hand-spawned, then stored; camtest and cambench; every stream ended by its close):

| | front (leshen, link 6) | ultrawide (leshen-uw, link 0) |
|---|---|---|
| frames | 255 of 255 at bin 8, no error word | 255 of 255 at bin 8, no error word |
| rate | 29.5 a second (DMA_DONEs 33.88 to 34.02 ms apart) | 29.4 a second (33.89 to 34.04 ms) |
| cell (2 x 2, thru the view) | 151 232 / 234 211: greens on the anti-diagonal, the dimmer corner (0, 0) blue under the room's warm light: B G / G R (Camera2 BGGR) | 796 729 / 725 630 at 4 s and 16x: R G / G B (Camera2 RGGB) |
| the shift G#3004 | 3 bits: shifts 5, 6, 7 at 4800 lines gave frames 1.640, 3.279, 6.558 s apart; 8 read as 0 (51.2 ms); a forced 9 stopped the front end | the same: 1.599, 3.198, 6.397 s; 8 read as 0 (49.98 ms) |
| longest | 65525 lines of 2^7: frames 89.353 s apart; LIST's exposure_max 89.34 s | 65525 lines of 2^7: 87.156 s apart; exposure_max 87.14 s |
| the plan's ladder | 64 ms to 16.384 s, each frame as asked (16.388 s) | 64 ms to 16.384 s (16.392 s) |
| slits (64 rows, detached, slim, 192-line frames) | 4099 in 8.38 s, 489 a second (sensor 492), none lost, no error word | 4096 in 8.17 s, 501 a second (sensor 505), none lost |

A forced shift with a frame under the mode's own in shifted lines (1429 of 2^6) stopped the front, as the main's 76 of 2^6 did; the plan never makes one.

**Stored** (the 8a's vault, each read back alike): `camera` (this branch's driver, unmasked capsule, blake3 20ef0f61; the one before saved in keys/akita/cam-inputs-2026-11-19/final/camera-before.capsule.vsf). Also `cam.leshen.{init,prog,pdma,idt}` and `cam.leshen-uw.{init,prog,pdma,idt}`: the inits as before, the programs with Android's G#478, G#47C and G#48C, the ultrawide's job cut to G#1828. The IDTs are stand-ins so both are listed: the Pro's front's for the front, the Pro's ultrawide's for the ultrawide, until the owner's calibration.
The files are local in keys/akita/cam-inputs-2026-11-19/final (B3SUMS).
LIST from the vault's driver lists the main, the ultrawide (4208 x 3120, 115.95 degrees) and the front (4208 x 3072, 93.60 degrees). camtest from the vault took 16 frames clean on each of the three.
Nothing was flashed: no kernel change was needed, and boot_b is untouched. boot_a holds the hv-alone image again (blake3 e6ee3eea, restored after the snapshot images).

## The gap to "Lumis opens and shows a live preview"

| piece | state | left |
|---|---|---|
| Lumis's front on ferros | built, host-tested | run on a device (akita, without a camera: its "no driver" page) |
| the camera right | built (code 9) | a kernel, stower and map with it on each device |
| the endpoint | served by payloads/camera (built 2026-11-14) | a run on the Pro |
| frame memory | a run of 39 frames from the pool, fenced (built 2026-11-14) | a run on the Pro |
| the driver | payloads/camera behind the `camera` switch (built 2026-11-14) | streams on the Pro (2026-11-14); the harness out the same day |
| the front end's answer | its interrupt, SPI 479, in the driver's frame thread (built 2026-11-14) | whether the line fires (else the thread's own tick, said on the console) |
| the PDMA job | from the vault (`cam.boitata.pdma`), moved into the region in Rust (ferros_cam job, built 2026-11-14) | the vault entry on the Pro |
| akita's cameras | the main streams and is stored, its kernel flashed (2026-11-19, "The 8a's main streams"); the front and the ultrawide wired | the front's and the ultrawide's D-PHY lines ("The 8a's front and ultrawide"); the main's own IDT |

## The endpoint

`camera`, served by the camera driver, called by a kist holding the camera right (slot 7 for a kist that declares only it).
A request is an op byte and its arguments; numbers are little-endian.
The list is a VSF document (the house format for anything structured); a frame is a fixed header and the samples, as the GPS fix is.

| op | request | reply |
|---|---|---|
| 0 LIST | `[0]` | a VSF document, a section `camera` per camera the driver can stream whose inputs are in the vault (none: an empty document; "The other cameras"); a camera whose 1:1 init is in the vault has a second section, the same `id` and `name`, `mode` 2 ("1:1 on the main") |
| 1 OPEN | `[1][id]`, or `[1][id][1]` for the view, then optionally the mode block `[kind][rows: 2][fll: 2][exposure: 2][flags][ring][line length: 2][cols: 2]` ("Modes, the bench and the slit"; kind 2 the 1:1 readout, "1:1 on the main"), then optionally the view flags (bit 0: write-back, "A cacheable view") | `[0]` open, `[1]` no such camera (or one that cannot stream: the tele), `[2]` another holds it, `[3]` it would not start; with the view given, `[0]` then the view's token (32 bytes) and the ring's shape ("The view"; 64 bytes with a mode block: also the ring's count as a u32 at 48, 1 at 52 for a slit ring, 1 at 53 for a write-back view, a slit's record offset at 56) |
| 2 FRAME | `[2][bin]` | waits for the next frame: a 64-byte header, then the mosaic, `bin` x `bin` same-colour samples averaged, 16 bits a sample, row by row |
| 3 SET | `[3][what][value: 8 bytes]` | `[0]` taken, `[1]` outside the range; what: 0 exposure (ns), 1 gain (1024 unity: the sensor's analogue gain alone, by its own CCS model read at the open, digital gain kept at one; past the analogue's most refused, "Gain"), 2 focus (the actuator's position), 4 the frame length (lines, 0 the OPEN's own: a rate change on the running stream, "A rate change without a reopen") |
| 4 CLOSE | `[4]` | the view taken back, the camera stops (a held FRAME, MAP, SNAP or SLITS answered empty first); it also stops when its holder ends, at once on a kernel that watches the holder's end ("Serving while a frame waits") |
| 5 MAP | `[5][buffer to give back, G#FF none]` | with the view: waits for a frame newer than the last mapped and holds its buffer; its 64-byte header (bin 1; the buffer at +40, the format at +41, a line's bytes at +44, the buffer's place in the view at +48, its PD rows' place in the buffer at +56 and their rows at +60, 0 none), the samples read in place |
| 6 RELEASE | `[6][buffer]` | the held buffer given back: `[0]`, `[1]` when it was not held |
| 7 PROBE | `[7][id]`, development, no camera open | that camera's module powered, its identity read, the addresses that answer on its bus mapped, the module down: `[code][id: 2][error]` and a 16-byte map (bit a of byte a / 8); code 0 it answered as itself, 1 another identity, 2 no answer (the bus's error), 3 the switch refused, 4 no such camera, 5 a camera is open |
| 8 STATS | `[8]` | 26 u64s: frames written, answers, on the line, the frame thread's ticks and most, slit frames late, ring frames unseen, the engine ring's count (bytes) and its wraps, the counter and its frequency, refused, status bits, rows, width, line, buffer, slit ring, fll, line length, integration, bits, slim, PD, ticking, open time |
| 9 SLITS | `[9][after: u32]`, a slit ring open | waits for a frame past `after`: 8 u64s, the newest count, the ring's count, a buffer's bytes, rows, line, width, late, the ring's wraps; frame n is in buffer (n - 1) mod the ring, its record (count, counter, Eagle ns, probe moved) after its rows |
| 10 RATE | `[10][windows]` | the sensor's frame counter (CCS G#0005) read every 2^16 us for `windows` (16) windows beside the front end's count: 5 u64s, sensor frames, front-end frames, ticks, frequency, failed reads |
| 11 CAPS | `[11]`, a camera open | 73 bytes: the sensor's data formats (G#00C0 and 16 descriptors), integration least and margin (G#1004), the mode registers read back, a bit per failed read |
| 12 SNAP | `[12]`, the PD rows in the ring | the next frame held as MAP holds it: its header, then its image's and PD rows' physical addresses and the PD shape ("The slit for real") |

The ids: 0 the main (boitata), 1 the ultrawide (oksoko), 2 the front (dokkaebi), 3 the tele (kraken); each is also the `camera` switch's module (ferros_cam cams).
The calls that wait for a frame (FRAME, MAP, SNAP, SLITS) are held by the driver while every other call is answered (since 2026-11-18, "Serving while a frame waits"): one held at a time, a second answered empty; SET takes effect while one is held.

A `camera` section's fields: `id` (u), `name` (a: main, ultrawide, front), `facing` (u: 0 away from the person, 1 toward), `width` and `height` (u: the readout), `cfa` (u: the 2 x 2 cell's channels, 2 bits each, top left first, 0 R 1 G 2 B: husky's main G B / R G is G#49), `black` and `white` (u), `rotation` (u: quarter turns clockwise to stand it upright; husky's main 1), `idt` (nine f5: the Absolute IDT's matrix, row-major, camera to VSF RGB), `sensitivity` (three f5), `exposure_min`, `exposure_max` (u, ns), `gain_min`, `gain_max` (u, 1024 unity), `focus_min`, `focus_max` (u; both 0 where focus is not offered).
Since 2026-11-18 ("1:1 on the main") a section is one readout of a camera, and a camera may have two: `mode` (u: the OPEN mode block's kind that opens it, 0 the binned init's own, the same as no block; 2 the 1:1), `cfa_period` (u: 2 a Bayer mosaic; 4 the quad array at 1:1, `cfa` then naming its 2 x 2 blocks), `frame_ns` (u: the readout's own frame period, the frame rate while an exposure fits it), with `width`, `height`, `exposure_min`, `exposure_max` and `readout` the readout's own; `id` and `name` are the lens's, the same in both.
Since 2026-11-21 (RAW video on every camera) also `line_ps` (u: the readout's line at its own line length, picoseconds) and `fll_least` (u: the shortest whole frame the driver streams it at, its rows and `BLANK_LEAST`, 128 lines), from which a client paces any rate (ferros_clip `Line::of_ps`, `Line::of_list`, `Pacing::of_line`, RAW-VIDEO.md "Pacing"): the Pro's main 3148916 ps, the ultrawide 6561622, the front 10437753, the tele 6106805.
Since 2026-11-17 (owner: "Missing lens stats on the Lumis page") also the optics, each Android's Camera2 value as Lumis recorded it on the Pro (ferros_cam optics): `sensor_width_mm`, `sensor_height_mm` (f5: SENSOR_INFO_PHYSICAL_SIZE from Lumis's enumeration, Lumis docs/google-bug-tele-maxres-fov.md; the front's derived from its recorded 35 mm equivalent), `focal_mm` and `f_number` (f5: LENS_FOCAL_LENGTH and LENS_APERTURE from each camera's calibration capture of 2026-10-21). Main 9.792 x 7.3728 mm, 6.9 mm, f/1.68; ultrawide 6.4 x 4.8, 2.23, f/1.95; front 4.6846 x 3.3378 (derived), 2.74, f/2.2; tele 5.6448 x 4.2336, 18.0, f/2.8. Not recorded anywhere found, so not listed: the nearest focus distance (Lumis keeps its own 8 dioptres for the main's slider) and stabilisation (the main's OIS part is on its bus, ferros does not drive it). Lumis 0.2.12 (std/lumis) reads them, a driver without them giving zeros (shown as unknown); its page then draws each sensor, its field of view, pixel size, focal length and aperture.
The IDT is the device's calibration (husky's main: `pixel 8 pro calibration data/derived/cam2-main.idt.vsf`), kept in the vault beside the sensor's init, never in a kist: `cam.<key>.idt` (the `.idt.vsf` file as it is: `cam.boitata.idt`, `cam.oksoko.idt`, `cam.dokkaebi.idt`, `cam.kraken.idt` from cam2-main, cam3-ultrawide, cam7-front, cam4-tele).
Since 2026-11-15 the driver reads it there at every LIST and carries its Absolute entry's matrix and sensitivity in that camera's section (ferros_cam list); a camera whose IDT is absent is not listed.
Until then the main's was not read from the vault at all: the driver's build.rs read the repository's file and built the list in.

The frame header: count (u32, +0), width and height after binning (u16, +4, +6), bin (+8), cfa (+9), black and white (u16, +10, +12), exposure (u64 ns, +16), gain (u32, +24), focus (u32, +28), when the exposure ended (Eagle time, i64 ns, +32), zeros to +64; since 2026-11-18 the mosaic's period at +14 (u8: 2 Bayer, 4 the quad array of a 1:1 frame; 0 from a driver before, read as 2).
The client's reader and the header's writer are side by side in rust/lumis-ferros/src/camera.rs, tested together.

A frame by copy is for the preview: husky's main at bin 2 is 2040 x 1536 samples, 6 MiB a frame, 90 MB/s at 14.5 frames a second.
A full raw frame for a capture (25 MB at 16 bits) wants a mapping instead, a ring of frame buffers the kist reads in place (KISTS.md "Lumis"): the view, since 2026-11-15 ("The view" below).
How large a reply the kernel copies between two processes in one call (read from the code, 2026-11-14): a plain reply was capped at a store transfer, 256 KiB, so the preview frame could not cross; since 2026-11-14 up to 8 MiB (`REPLY_MAX`), copied server to caller under the kernel lock (about a millisecond for 6 MiB, unmeasured).

## The view (2026-11-15): whole frames to the kist, read in place

The owner, 2026-11-15, seeing Lumis's saves: "why the ridiculously low camera resolution?"
A frame by copy is capped at a reply (8 MiB, `REPLY_MAX`), so Lumis only ever had bin 2: 2040 x 1536, a quarter of the sensor, and its saves were 3 MP of a 12 MP sensor.
The view gives the kist that holds the camera the driver's frames themselves, read-only, in place.

**The ring** (payloads/camera, ferros_cam job `main_ring`): the image body's slot of the job's region becomes a ring of buffers of 16 MiB (eight frames of the pool each; a 4080 x 3072 RAW10 frame at G#1400 a line is 15 MiB), four of them (128 MiB with the rest of the region and the long ring's frame: the camera's window whole), else three, two, or one (the region before the view, 40 frames), under 4 GiB first as before; one buffer gives no view (frames by copy only), so a pool with little room (a jumped session's) opens the camera with no more memory than before (the driver's arc is unchanged too, four frames).
The job's program writes the image body's address once (`image_words`: words 30 and 32 of the main camera's 6620-byte program), so the driver puts a copy of the program per buffer in the long ring's frame, each naming its own buffer, and at every DMA_DONE writes the descriptor it is about to release to name a buffer that is neither held (by the kist, or by the binning of a FRAME by copy), nor still with the engine, nor the newest written one (`ring_turn`).
Which buffer a frame lands in is measured, not assumed: each buffer's probe (the first 64 bytes of its last line, written last of the frame) is patterned as the buffer is given to the engine, and the frame's DMA_DONE finds the buffer whose probe moved; the console says, every 64 MAPs, how many releases back each frame's buffer was given (0: the descriptor released at the frame before), frames whose buffer was not found, and turns that found no free buffer.
A held buffer's probe is kept when it is handed out and compared when it is given back: any write while held is counted and said (the ring's promise is none).
FRAME by copy stays for anything that wants it, binned from the newest buffer, held while it bins.

**The kernel's view** (process.rs `cam_view_give`, `cam_view_drop`; cachemap `CAM_VIEW_VA`): at an OPEN that asks for it, the driver calls SYS_DMA x1 = 4 on its region with the OPEN's reply token, and the kernel maps the ring's frames of the driver's run, read-only for EL0 (and for EL2: AP[2] binds both in the VHE regime) and non-cacheable (the front end does not snoop), at G#50_3800_0000, past the driver's own window (since the bench, G#51_2000_0000 in the camera's own GiB: "Modes, the bench and the slit"); it gives the process waiting on that reply token a capability for the `camera-view` region and writes its token into the driver's arc for the OPEN's reply to carry.
The kist presents the token to SYS_DMA for the view's base (x1 = 1 its size), as any region.
Isolation, as the system has it for surfaces and GPU blocks: only the holder has the token; the view is never a syscall buffer (`dma_span` leaves it out, so the kernel never writes thru it), never granted by name, never passed on in a spawn; the GPU never sees it (its S2MPU admits surfaces and blocks only), and the front end's S2MPUs admit the driver's run and nothing else, as before.
Native processes share one translation table (KERNEL.md, execution model), so a native process that guessed the address could read the view while it is mapped, as it could a surface; a masked one cannot.
Taken back (every block mapped to the blank frame, every TLB told, the holder's capability gone so its token answers ERR): at the driver's close (SYS_DMA x1 = 5, the first thing `close` does), when either the holder or the driver ends (`cam_free`), or when a new view is given.
The frames go back to the pool wiped when the driver ends, as before.

**Lumis** (std/lumis): opens with the view, and per frame MAPs (giving back the buffer it held), copies each line out of the view in 64-byte loads, unpacks RAW10 to 16 bits across the cores, and integrates the whole 4080 x 3072 frame; its saves are the sensor's whole frame (VSF RAW, and VSF 8B through vsf's AV1 on Lumis's `vsf8b-av1`). An open answered without the view (a driver or kernel from before) falls back to frames by copy at bin 2. Its heap's ceiling is 2 GiB (the integrator's memory at full size is about 350 MB, a save's on top).

**On the Pro, 2026-11-15** (cold boot b881, this kernel RAM-reloaded as b882; the driver and camtest stored and read back exact):
- camtest `4001`: a ring of four in 64 frames under 4 GiB (G#CA00_0000); 64 whole 4080 x 3072 frames thru the view 66.3 ms apart (the full 15 a second), none skipped; DMA_DONE to the MAP's answer 20 us on average, 52 us at most; every frame in the buffer released at the frame before (64 of 64), none unseen, none crowded, held buffers written over 0; the token refused after the close. A whole frame read out of the view on one thread: 60 to 66 ms.
- camtest `08010002`: the store into the view ended camtest on a permission fault (ESR G#9200004E, a write, level 2; FAR G#50_3800_0000), and the kernel took the view back as its holder ended.
- **Above 4 GiB, measured**: with Lumis running no run was free under 4 GiB, and the driver took 64 frames at G#A_E5E0_0000; frames landed there with S2MPU faults 0 on all three, so the front end drives the high address halves with its SYSMMUs in bypass.
- Lumis 0.2.10 (stowed): frames of 4080 x 3072; the display showed its window 144 times in 10 s (14.4 a second; about 9.6 at bin 2). A turn of the camera thread 68.7 ms over 64 frames: 15.4 waiting for the driver, 25.8 unpacking out of the view across the cores, 29.6 in the integrator (the next 64: 78 ms; 110 ms with a save running). 192 MAPs, every frame in the buffer released at the frame before, held buffers written over 0.
- A VSF RAW save: 15669179 bytes, four times the bin-2 save's 3918758 (the whole sensor), 3.09 s from the request to kept, 2641 ms of it the write into the vault (not read back: it is in the kist's own domain).
- A VSF 8B save (AV1): Lumis ended 2.4 s after the request (exit 0: lumis_core `platform::exit`, whose callers are the camera thread's UI-heartbeat timeout of 1.6 s, the Exit button and the calibration's exits), with no save kept. Which one is not yet measured.
- S2MPU faults 0 in every run, the camera's fence 1.

**Lumis 0.2.11, built the same day, not yet run** (std/lumis; Lumis branch `ferros-fullres`, kete branch `ferros-write-pieces`):
- The lost save, read from the code (not measured): lumis_core's exits are the camera thread's UI-heartbeat timeout (1.6 s), the Exit buttons and the calibration's; the heartbeat's line was printed only in Lumis's debug build. On ferros the UI and the camera service are threads of one process sharing rayon's pool, and a VSF 8B save's demosaic and AV1 tiles (rav1e on rayon) fill it: the UI's draw asks the pool for work from outside it and waits behind the whole encode, past 1.6 s. On Android they are two processes, two pools. Now a save encodes in a pool of its own (half the cores), every exit says why on the console and names a save it loses, and the camera thread keeps what is encoded before it ends.
- Each save is written as a bulk value (out of kete's read cache), read back from the vault and compared (blake3), one line: size, encode ms, write ms, read ms, match.
- The 2.6 s write, read from the code: kete wrote every value in 4 KiB pieces, a 15.7 MB still in 3826 store calls (0.69 ms each), where the kernel takes a set of 256 KiB (STORE_MAX) and the vault process streams pieces of any size. kete's branch writes 256 KiB pieces (60 calls), stepping down on a refusal.
- The first run should show: a VSF 8B save's line with its encode time, Lumis still running after it, and the write time of both saves.

**payloads/camtest** drives it headless: bin 1 (`4001`: 64 frames) opens with the view, MAPs, reads each frame in place (a whole frame's copy timed), closes and tries the token again (it must be refused); `40010002` also stores into the view at the end, which must end camtest on the fault.

## The other cameras (2026-11-15): built, not yet run

The owner, 2026-11-15: "would be nice to get the other cams online."
Built on branch worktree-agent-a072c735fcd747099 while the Pro was away; nothing here has run on a device.
Capsules and the kernel: /mnt/Harbor/tmp/othercams/out (camera.capsule.vsf and camtest.capsule.vsf native, ferros_kernel.elf).

**What changed, in one place each:**
- **ferros_cam cams**: the four cameras by id (0 main, 1 ultrawide, 2 front, 3 tele): sensor bus and address, identity, stream and hold writes, lens actuator, receiver, the front end's acknowledgement and tick, geometry, the job's region, line time and exposure range, and the list's facts. Each field says whether it is measured, read from an init, or a guess.
- **The `camera` switch takes a module**: SYS_POWER's bits 8-15 for this switch only (ferros_sys `power_arg`; plain `power` is the main, so the old driver still works on the new kernel). One module at a time: another asked for while one is held is refused. The main's power-up is untouched (`power_up`); the other three run their device-tree sequence (camera.rs `Module`, `power_module`, `power_down_module`) after `program_rails`, the same step the main's now starts with: every one of the eight VSELs programmed, read back and checked against the overvoltage guard and the chip's own range while the matrix reads 0, before any enable bit. Off stops the held camera's stream with its own write, stops its own link (`rx_stop`), replays Linux's close as before, and powers that module down in its own order.
- **Grants**: hsi2c_3, hsi2c_2 and hsi2c_4 (`hsi2c3`, `hsi2c2`, `hsi2c4`) after the driver's nine, slots 10 to 12, so slots 1 to 9 keep their places.
- **The camera window** (cachemap `CAM_VA`) is 224 MiB, from the surfaces' last slot (it was 128 MiB): the front camera's region keeps whole 16 MiB granules until its outputs are measured (178 MiB with a ring of four).
- **The driver**: OPEN takes ids 0 to 2; LIST builds the document of the cameras whose inputs and IDT are in the vault, each section with its own IDT read from `cam.<key>.idt` (ferros_cam list, in a 1 MiB heap); PROBE (op 7) powers any module, reads its identity, maps the addresses that acknowledge on its bus and powers it down; frames, the view and SET follow the open camera's table entry. camtest's fifth input byte picks the camera, a fourth of 7 probes it.

**The ultrawide (oksoko, Sony IMX787, id 1)**: everything the harness streamed it with on 2026-10-22 (4000 x 3000 RAW10, about 30 frames a second) is back.
- Measured then: its power from the camera PMIC (LDO1, 2, 4, 6, 8, sub LDO12, the gpio2 load switch G#1711, reset gpp6-3), identity G#0787 at G#0016 from a cold boot with sub GPIO2 low, stream on G#0100 = 1 alone, receiver on link 2 (G#1C06, csis2 lanes, SYSREG 500 = 4, ferros_cam rx `RX_START_UW`), the acknowledgement with G#1C35_0148 first (`FRAME_ACK_UW`), a 15 ms answer where 50 ms stopped it (the driver's tick 2^13 µs), the image body at G#1C32_05C0 with stride G#13C0, and every output's extent, identical in four runs (run-uw1..4.txt).
- Its job keeps each output's offset inside its 16 MiB IOVA granule (the later image builder), so ferros_cam job gained a per-slot skip; the region is sized to what was written, not twice it (`UW_SLOT_BYTES`): 118 MiB with a ring of four, 70 MiB with one. The driver's report after the eighth frame re-measures it.
- The sequence order and delays are now the device tree's (`sensor@2`); the harness used the same rails in table order with the switch before the clock, and answered either way. The clock pin's drive is the device tree's X4 (3; the harness used 2).
- From the init, not measured: line time 6.5616 µs (9568 clocks at 24.576 MHz / 3 x 178; its 5081-line frame is then 33.3 ms, the rate seen), integration 4124 lines; the longest exposure offered is the frame less 2^6 lines (Sony's margin unread). Its analogue gain code G#3C0 is kept (if it follows Sony's 1024 / (1024 - code) it is 16x).
- From its calibration capture (2026-10-21, 4000 x 3000, the mode ferros's init sets; "Mosaic, levels and orientation" below): mosaic B G / G R, black 64, white 1023, orientation 6, so rotation 1 as the main. Derived from the capture, not yet seen on a ferros frame.
- Its lens actuator (G#0F) never acknowledged ferros, so focus is not offered (0 and 0 in the list) and the lens stays at rest.
- Vault inputs to store (local files, the vendor's settings, never in the repository): `cam.oksoko.init` from target/cam/uw.init, `cam.oksoko.prog` from target/cam/uw.cpr2, `cam.oksoko.pdma` from target/cam/uw.pdma; and its calibration `cam.oksoko.idt` from the repository's `derived/cam3-ultrawide.idt.vsf`.

**The front camera (dokkaebi, Samsung G#30A1, id 2) and the tele (kraken, Samsung G#08D5, id 3): why they NACK.**
Every register their power-up touches was compared with Android's on 2026-11-06 and matched, and both still NACKed while the main and the ultrawide answer (docs/oracle/CAMERA.md "The front camera and the tele").
The device tree (/mnt/Harbor/ferros-ref/kdev-shusky/dts/zuma-husky-camera.dtsi) names one supply that comparison could not have covered, because their power-up never touches it:
- The main's `enable-gpios` is the sub-PMIC's GPIO2 with the comment "CAMS_LVBUCK_EN for ldo8": a low-voltage buck that feeds the camera PMIC's LDO8 (0.97 V, the LV group's input).
- The front and the tele both list LDO8 as a supply and have no enable of their own; the ultrawide lists LDO8 too, but its 1.1 V core is LDO6.
- Under Android the main is `lwis,power-up-to-suspend` (powered at boot, kept there), which fits the ACPM trap seeing no sub-PMIC command at all during a camera open or the flip: the line is already high whenever the front or tele runs there.
- From a cold ferros boot every reading of it was G#40 (output, low): the main raises it at its own power-up and lowers it at its power-down, and nothing else touches it.
- So the front's and the tele's sequences now raise it where the main's does (after LDO1, 10 µs before LDO8) and lower it last. A guess from the device tree; the switch logs GPIO2's value before and after, and PROBE shows whether the sensor then answers.
- Other candidates, if it does not: the sensor's master clock on its pad (gpp5-0 for the front, CIS_CLK1; gpp7-0 for the tele, CIS_CLK2), whose pad reads low even when running, so no register says; the rails at the module; the reset's polarity. PROBE's address map separates them: the front's EEPROM (G#51, on sub LDO12 alone) and lens actuator (G#0C, on the shared enable) answering while the sensor does not would leave its core supply, clock or reset.
- Android can answer the LV-buck question without the hv: with the front camera open, `adb shell su -c 'cat /sys/kernel/debug/gpio'` lists s2mpg15's GPIO2.

**The front camera, built as far as its captures go**: its power (`sensor@1`: LDO1, LDO8, sub LDO26 and LDO12, the gpio3 switch G#1712, the shared enable sub GPIO4, the clock on gpp5-0 at drive X3, reset gpp4-2, 8 ms; plus the LV buck), receiver (link 0, `RX_START_FRONT`, captured 2026-11-06; its stop derived), job (front.pdma, whole granules), geometry from its init (3840 x 2736, stride G#12C0).
From its two calibration captures (3840 x 2736, the size its init sets): mosaic G R / B G, black 64, white 1023, orientation 8 (rotation 3) in the second, none recorded in the first.
Guesses until it streams: the ultrawide's acknowledgement and tick (as the harness had them), the line time (8.35 µs, 30 frames a second; the PLL allows 60 at 4.18), exposure offered only up to the init's 1712 lines, focus not offered.
Vault inputs, only once PROBE shows it answering: `cam.dokkaebi.init` from target/cam/front.init, `cam.dokkaebi.prog` from target/cam/front.cpr2, `cam.dokkaebi.pdma` from target/cam/front.pdma, `cam.dokkaebi.idt` from `derived/cam7-front.idt.vsf`. With them stored it is listed.

**The tele, built as far as its capture goes**: its power (`sensor@3`: LDO1, LDO8, sub LDO12, LDO7, LDO3, LDO5, the gpio1 switch G#1710, the clock on gpp7-0, reset gpp6-2, 10 ms; plus the LV buck) and PROBE.
It cannot stream: its receiver, CPU setup and job were never captured (in dim light the app's 5x is a crop of the main), so it is never listed and OPEN answers `[1]`. Its init alone is local (target/cam/tele.init, 4032 x 3024 RAW10, no stream-on in it).
Its calibration capture (4032 x 3024) gives mosaic B G / G R, black 64, white 1023, orientation 6 (rotation 1), in cams already; its IDT, `cam.kraken.idt` from `derived/cam4-tele.idt.vsf`, is for when it streams.

### Mosaic, levels and orientation, from the calibration captures (2026-10-21)

The five Android captures the IDTs were solved from (`pixel 8 pro calibration data/*average.vsf`, gitignored, local only: each carries the unit's pixel-response pattern) were read with vsf on the host (a scratch reader printing the capture and plane fields, never the samples or the location); only the numbers below come from them.

| camera | capture | size | cfa (2 x 2, 0 R 1 G 2 B) | black / white | orientation (EXIF) | ferros rotation |
|---|---|---|---|---|---|---|
| main (cam 2) | 13:33:48 | 4080 x 3072 | 1 2 / 0 1, G B / R G | 64 / 1023 | 6 | 1 |
| ultrawide (cam 3) | 13:34:16 | 4000 x 3000 | 2 1 / 1 0, B G / G R | 64 / 1023 | 6 | 1 |
| front (cam 7) | 13:34:43, 13:54:07 | 3840 x 2736 | 1 0 / 2 1, G R / B G | 64 / 1023 | none, then 8 | 3 |
| tele (cam 4) | 13:33:15 | 4032 x 3024 | 2 1 / 1 0, B G / G R | 64 / 1023 | 6 | 1 |

Whether ferros reads out as Android did, compared from the inits ferros replays (target/cam, Samsung parts read on the CCS page G#4000 only):

| camera | init's output size | x_addr_start, y_addr_start | image_orientation G#0101 as ferros writes it |
|---|---|---|---|
| main | 4080 x 3072 | 16, 16 | 0 in the init, 3 in the stream-on write (Linux's `01 00 01 03`) |
| ultrawide | 4000 x 3000 (binned) | 0, 464 | 3 in the init; stream-on G#0100 alone |
| front | 3840 x 2736 | 136, 0 | 0 in the captured stream-on (`01 00 01 00`) |
| tele | 4032 x 3024 | 8, 0 | in no captured write (its stream-on was never captured) |

- Every capture's size is exactly the size the init ferros replays sets, and each init is Android's own, captured from the HAL opening that camera: the same mode, so the same crop and flip.
- Every crop start is even, so none moves the 2 x 2 phase.
- The flip ferros writes is the HAL's own write, in the init or in the stream-on Linux sent.
- What was not captured: the sensor's registers while Android took the RAW capture itself (a different session from the one the init came from). That the RAW stream ran the same mode rests on the matching size; a HAL that flipped or cropped by an odd line per stream would break it, and nothing seen says it does.
- Measured on ferros: the main only (its frames developed in true colour with G B / R G on 2026-10-21, and stand upright in Lumis at rotation 1). Derived from the capture: the ultrawide, the front and the tele. Still unconfirmed: the front's orientation rests on one capture of two (the first records none); the tele's flip is not in any captured write.
- Lumis's 2026-10-18 captures of the ultrawide (8000 x 6000 quad Bayer, orientation 3) were its full-resolution mode with the phone held sideways: not the mode ferros runs.

### The tele's capture (needs the owner; not run)

What is missing is a session where Android streams the tele: its CSI link and lanes, its CPU setup writes and its PDMA job, and its stream-on.
The owner: the Pro on USB, Android under the hv (the gate's volume down), adb with root, a bright scene (daylight thru a window; the Pixel Camera app picks the tele for 5x only with enough light), the phone on a stand not touched while the taps run, and three boots of Android under an oracle hv.
One bus per capture (feedback_oracle_capture_integrity), each armed 60 s after the reset, the console quieted to level 4 (husky-hv-camall.sh does), the zone kept by the crash it ends with:
```
# 0. the ISP front end (the 42 pages a camera open uses): the tele's link, lanes and CPU setup
FERROS_HV_STAGE2=1 FERROS_HV_ORACLE=1 FERROS_HV_NO_PMU_READ=1 FERROS_HV_ORACLE_PAGES="$(cat target/hv/ispfe-pages.txt)" \
FERROS_HV_ORACLE_CAP=32768 FERROS_HV_ORACLE_ARM_AFTER_MS=60000 tools/husky-hv.sh --no-ferros --flash
tools/husky-hv-camall.sh target/hv/tele-isp.log 8 tele:615:1437
# 1. the PDMA dump: the tele's front-end program (its list 2 is the one the job is built from)
FERROS_HV_STAGE2=1 FERROS_HV_ORACLE=1 FERROS_HV_NO_PMU_READ=1 FERROS_HV_PDMA_DUMP=1 FERROS_HV_ORACLE_PAGES="G#1C39_0000" \
FERROS_HV_ORACLE_CAP=4096 FERROS_HV_ORACLE_ARM_AFTER_MS=60000 tools/husky-hv.sh --no-ferros --flash
tools/husky-hv-camall.sh target/hv/tele-pdma.log 8 tele:615:1437
# 2. the tele's sensor bus (hsi2c_4), the controller's polling quieted: its init with the stream-on
FERROS_HV_STAGE2=1 FERROS_HV_ORACLE=1 FERROS_HV_NO_PMU_READ=1 FERROS_HV_ORACLE_PAGES="G#108B_0000" FERROS_HV_ORACLE_CAP=32768 \
FERROS_HV_ORACLE_QUIET="G#108B_0000,G#108B_0004,G#108B_0008,G#108B_0020,G#108B_0024,G#108B_0030,G#108B_0040,G#108B_0048,G#108B_0050" \
FERROS_HV_ORACLE_ARM_AFTER_MS=60000 tools/husky-hv.sh --no-ferros --flash
tools/husky-hv-camall.sh target/hv/tele-i2c.log 8 tele:615:1437
```
Before the first flash, save what slot a runs now (its boot image, or the build and FERROS_HV_* flags it came from), and flash that back after the third capture: `husky-hv.sh` without the oracle flags is not the same build (on 2026-11-06 a saved image was flashed back, /mnt/Harbor/tmp/husky-cam-2026-11-06/boot_a_restore_normal.img).
Check before using any of it: the tele really streamed in each (the I2C capture holds a G#0100 stream-on to G#2D; the front-end capture holds link writes past the main's; a screenshot of the app showing 5x on the tele, not "crop"), and `tools/oracle-i2c` reports no LENGTH MISMATCH inside the init.
From them, as for the front on 2026-11-06: `tools/oracle-isp cpu` (the CPU setup, a 'CPR2', and the receiver table), `tools/oracle-isp pdma` then `tools/oracle-isp image --list 2` (the job, a 'CPR3'), `tools/oracle-i2c --init` (the init), all local files (target/cam/tele.*, the vendor's settings). (Done 2026-11-18 night: "Every readout, captured" below.)
Then in code: its receiver table into ferros_cam rx, its outputs' slots into a `TELE_SLOT_BYTES`, `streams: true` and its geometry in cams, a list bit; the first run measures the outputs as the ultrawide's did.
The same three captures with `front:780:1683` instead give the front camera's whole session again, should its 2026-11-06 capture turn out wrong once it answers.

### Device test plan (for the owner's return)

Risks first:
- **Overvoltage**: no enable bit is set before all eight rails' codes are programmed, read back and passed the guard (at most a tenth over the Android regulator readout of 2026-10-21) while the matrix reads 0; a rail already on refuses the whole power-up. The tele's LDO3 and LDO7 and the ultrawide's LDO2 and LDO6 were enabled from ferros before (the harness, October and November), with these codes.
- **The LV buck** for the front and the tele is the line the main raises at every open; nothing new electrically.
- **A sensor stuck until a cold boot**: the driver writes no mode or window register mid-stream, only exposure inside the hold. The ultrawide's exposure SET is new (8-bit hold, the longest 5017 lines a guess): try it last, and if its frames stop after a SET, a cold boot (a reload does not power-cycle the sensor). camtest's controls mode (fourth byte 1) carries the main's numbers: never with the ultrawide.
- **One module at a time**: a PROBE answers 5 while a camera is open; the torch still refuses while the switch is on.
- **The EL3 poison pattern**: the switch's path is the main's (no PMU reads); nothing new there.

The steps (FERROS_PORT the Pro's port, 1-2.2 on 2026-11-14; each step after the owner's go; `B=/mnt/Harbor/Code/ferros/target/debug/ferros-bridge`, `O=/mnt/Harbor/tmp/othercams/out`, `C=/mnt/Harbor/Code/ferros/target/cam`):
0. **Back up** what will change: `$B store get <k> /mnt/Harbor/tmp/othercams/back/<k>` for `camera`, `camtest`, `cam.boitata.idt`, `cam.oksoko.init`, `cam.oksoko.prog`, `cam.oksoko.pdma`, `cam.oksoko.idt` (absent ones say so).
1. **The kernel**, a minute after a boot: `$B reload $O/ferros_kernel.elf` (RAM only; a reboot returns to the flashed kernel), then attest. A new driver on an old kernel cannot pick a module (the old kernel takes x1 = G#101 as off); an old driver on this kernel works as before.
2. **Store** the driver, camtest, the two IDTs and the ultrawide's inputs (`P="/mnt/Harbor/Code/ferros/pixel 8 pro calibration data/derived"`): `$B store put camera @$O/camera.capsule.vsf`, `$B store put camtest @$O/camtest.capsule.vsf`, `$B store put cam.boitata.idt "@$P/cam2-main.idt.vsf"`, `$B store put cam.oksoko.idt "@$P/cam3-ultrawide.idt.vsf"`, `$B store put cam.oksoko.init @$C/uw.init`, `$B store put cam.oksoko.prog @$C/uw.cpr2`, `$B store put cam.oksoko.pdma @$C/uw.pdma`; then `store get` each and `cmp` with its source (the `@`, capsule_arg_order). Both capsules are native: no seal. **The main is not listed without `cam.boitata.idt`** (this driver reads every IDT from the vault): Lumis would show no camera.
3. **The main as before** (the regression): `$B spawn $O/camtest.bin 4008 --name camtest --use camera` (every camtest below the same, its input changed), then `$B diag "console kept 64"` and `$B diag | grep S2MPU`. Expect LIST of about 870 bytes (main and ultrawide; under 500 means one is missing, the console names an IDT that did not read), the first frame about 2.3 s after the open, frames 67 ms apart, `camera: main: 22 of its 22 outputs written`, S2MPU faults 0.
4. **The ultrawide's frames**: camtest `4008000001`. Expect `camera: ultrawide: A#60 frames at ...` (or more frames for a ring of four: 60 is a ring of four's 118 MiB plus the long ring's frame), `switch on`, `sensor G#0787`, init 231 writes in about a quarter second, `job placed: A#25 address registers ... A#12 loads`, the engine started, the first frame, then frames about 33 ms apart, `of its A#10 outputs written` with no output past its slot, S2MPU faults 0. If no frame comes: the console's `no interrupt on SPI 479` line, `image status bits seen`, and the flash log's `camera: ultrawide:` power lines.
5. **The ultrawide's view**: camtest `4001000001`: the view of a ring, 4000 x 3000, 5056 bytes a line; the token refused after the close.
6. **Lumis, a check**: the menu lists main and ultrawide; open the ultrawide on a scene with something red, green and blue, held upright. Its mosaic and rotation come from its calibration capture (above), so this confirms them: red, green and blue in their places and the picture upright. If either is off, Android's RAW stream ran another mode than the init's (an odd crop or another flip): tell the coordinator what it shows.
7. **The front camera's probe**: camtest `0000000702`, then the console and `$B diag log | grep "camera:"`. The flash log shows each step of its sequence, `sub GPIO2 high: G#40 -> G#60` among them. Answered G#30A1: the LV buck was it; store the front's inputs (`cam.dokkaebi.init` from `$C/front.init`, `.prog` from `$C/front.cpr2`, `.pdma` from `$C/front.pdma`, `.idt` from `"$P/cam7-front.idt.vsf"`, cmp each) and try its frames, camtest `4008000002` (everything about its stream is first-time: a stall needs a cold boot). Still NACK: the address map says what lives on its bus (G#51 its EEPROM, G#0C its actuator), and the next measure is Android's debugfs line for GPIO2 with the front open, then a capture (above).
8. **The tele's probe**: camtest `0000000703`: answered G#08D5 means its power is right and only its capture is missing.
9. **Restore** if anything misbehaves: put back what step 0 saved (`store rm` the keys that were absent), and reboot to the flashed kernel.

## Modes, the bench and the slit (2026-11-15): built, not run

The owner, 2026-11-15: "I want straight as fast as possible full frame 1:1 and 2x binned for all back cams integrating into the buffer. straight add/accumulate and see what numbers we can hit. Then I want the slitscan wired up, same idea, we'll only use the center four pixels (two binned) so as narrow as we want to run that, same idea, binned and 1:1, how fast can we capture? Get it ready so when the pro is available we can run it."
Built on branch `camtrials` (from worktree-agent-a072c735fcd747099 and forge) while the Pro was away: nothing here has run. Capsules and the kernel: /mnt/Harbor/tmp/camtrials/out (camera, camtest, cambench native; ferros_kernel.elf).

**What changed, one place each:**
- **ferros_cam ccs**: each init's mode read back out of it (direct writes on the CCS page, or thru Samsung's indirect window G#6028/G#602A/G#6F12), and the modes built from it: the binned mode with its frame and integration cut (`timed`), a 1:1 readout (`unbinned`, a guess), slits (`slit`), RAW12 (`raw12`); `writes` gives the I2C writes inside the camera's own hold (`Cam::hold`), refusing an integration that does not fit the frame with a 2^5-line margin.
- **ferros_cam geom**: the front end's geometry rewritten from the captured frame to the mode's (`value`, `rewrite`): the image and PD paths' strides and sizes, the CSI link's channel sizes and data types; `Scope::Core` (the frame's way into memory) or `Scope::All` (every register, the October tool's reach); and the slim per-frame program (`slim`).
- **ferros_cam raw**: the NEON unpack-and-add (`add10_u16`, `add10_u32`, `unpack10`, `fold`, `add12_u32`): 40 bytes to 32 samples with three table lookups and a lane-wise shift, no branch per sample; host-tested lane by lane against the plain unpack (the build has no aarch64 emulator: cambench checks the NEON path against the plain one on the device, on the first frame's middle line).
- **ferros_cam job, cams**: a buffer's bytes per camera and mode (`buffer_bytes`, `slit_bytes`, `with_image`, `Cam::buffer_bytes`), no longer one 16 MiB constant (the 8a's 4624 x 3472 main needs 22 MiB).
- **The kernel**: the camera's window and view in a GiB of their own (cachemap `CAM_GIB` G#51_0000_0000, 512 MiB each), so four 60 MiB 1:1 frames and their region fit (the main's: 302 MiB).
- **payloads/camera**: OPEN's mode block (below); the mode written after the init and before stream on; buffers sized to the mode; a slit ring; STATS, SLITS, RATE and CAPS (the endpoint table); the holder taken for gone after 2^22 us of silence instead of 64 frames.
- **payloads/cambench**: the bench (below).

### The modes, per camera: known and not

From each init (target/cam, the decode in ferros_cam ccs's tests):

| camera | Android's binned mode (the init) | addresses | 1:1 (Android's maximum resolution) | what the 1:1 needs |
|---|---|---|---|---|
| main (Samsung G#08EF) | 4080 x 3072, x G#10 to G#3FCF, y G#10 to G#300F, increments 4, binning G#0900 = G#32, G#0901 = G#44, frame 21362 lines (10584 in its first block) of 6320 clocks, integration 21342, RAW10, 4 lanes | 2 a pixel: 16320 x 12288 for 8160 x 6144 | 8160 x 6144 RAW10 | a capture: Samsung's G#0900 is not CCS's (G#32 G#44 here, G#22 G#22 on the tele), and its setfile writes on pages G#2000 to G#2021 may carry mode timing |
| ultrawide (Sony IMX787) | 4000 x 3000 from a 9248 x 6016 window binned (G#0900 = 1, G#0901 = G#22, G#0902 = 8), digital crop 312, 4, frame 5081 lines of 9568 clocks, integration 4124 | 1 a pixel | 8000 x 6000 RAW10 (Lumis took these on 2026-10-18) | a capture: Sony's quad Bayer full readout normally also sets its remosaic registers, which no CCS register names |
| tele (Samsung G#08D5) | 4032 x 3024, x 8 to G#1F87, y 0 to G#17BF, increments 2, binning G#22 G#22, frame 5457 lines of 5568 clocks, integration 2232, 2 lanes | 1 a pixel: 8064 x 6080 | 8064 x 6048 (Lumis's bug report: the HAL's log says "sensor output size: 8064 x 6048") | it has never streamed on ferros (no receiver, CPU setup or job captured: "The tele's capture"), and then the 1:1 capture |

- **2x binned** is known for the main and the ultrawide (it is what they stream); what is new is cutting the frame (`fll`) and the integration, both inside the hold, timing only, nothing in the front end changes.
- **1:1** is built as a guess from the CCS registers alone (binning G#00 G#11, the increments one pixel, the output Android's maximum, the line length doubled since a 1:1 line carries twice the pixels at the same link rate, the frame 256 lines past the rows), with the front end's geometry rewritten (strides G#2800, sizes 8160 x 6144 or 8000 x 6000) and the PD path off. It may not stream, or stream garbage; the capture below gives the real one.
- **RAW12**: whether the sensor offers it is CAPS's first line (the CCS data format descriptors, G#0C0C). The attempt writes G#0112 = G#0C0C, the link's data type G#2C and stride G#1800; the front end's own format word is not known (G#1C32_0478 = G#014C1E38 and G#1C32_048C = G#01000200 are the candidates in the CPU's writes), and Android's maximum-resolution RAW is RAW10 only, so no capture can show it: an experiment, nothing more.
- **How the slit is centred**: the captured window's middle (the optical axis as Android crops it: main y G#1810, ultrawide y 3472), the slit's start rounded to the nearest colour filter period from the window's top (8 addresses on the main, 4 on the ultrawide and tele: a 2 x 2 Bayer cell binned, a 4 x 4 quad cell 1:1), so the mosaic keeps the captured phase; a slit of an odd number of periods sits half a period off the true middle (the binned 2-row slit of the main: y G#1810 to G#1817 against the middle G#1810).

### The October slit, read again

`diag cam slitsweep` (2026-10-21, git 7cae2561) measured 4 rows as the floor and 1212 frames a second as the ceiling (825 us, however few rows), 2 and 1 rows refused, a frame of 68 lines stopping the stream and 96 running, and "exactly one geometry change per stream session".
Its writes went without the hold: `w16(G#0104, 1)` writes G#0104 = 0 and G#0105 = 1, which is the release (G#0104 is one byte, found the same night); and with the init's integration of 21342 lines left in frames of 68 to 576 lines.
So each change landed register by register (y_end before y_output_size, a frame shorter than its integration), which is the likeliest cause of the stuck sensor and of the "never starts before stream on".
The driver now writes every mode inside the real hold with an integration inside the frame; whether the 825 us floor and the 4-row floor stand is the first thing the bench measures.

### The bench (payloads/cambench)

`ferros-bridge spawn cambench.bin <16 hex bytes> --name cambench --use camera`, the bytes: +0 camera, +1 kind (0 the init's, 1 binned cut, 2 1:1, 3 binned slit, 4 1:1 slit), +2 rows (u16 LE), +4 fll (u16 LE; 0 the default, FFFF the mode's own), +6 integration lines (u16 LE; 0 the driver's 16), +8 flags (1 RAW12, 2 mid-stream, 4 PD on, 8 geometry everywhere, G#10 slim program, G#20 slim level 2, G#40 front end left as captured), +9 slit ring log2 (0: 2^8), +10 line length (u16 LE), +12 work (0 map only, 1 read only, 2 read + u16 add folded into u32 every 64 frames, 3 read + u32 add), +13 frames in 16s (0: 64), +14 sweep (1 slit rows 64 down, 2 frame length from the mode's own down), +15 threads (0: four).

Per open it says: the OPEN and the view's shape; CAPS (the data formats, with "RAW12 G#0C0C offered" or not; the mode registers as the sensor holds them, so a mode that did not land shows); RATE (the sensor's own counter against the front end's DMA_DONEs over a second); per frame the read and add time per thread, frames skipped, cores used and left; the link's and the bus's MB a second; the sums' mean a frame (the adds landed); for slits, slits a second, lost to the ring, read over, the cost a slit; STATS (answers, the frame thread's time an answer and its most, late slit frames, the long ring).

**Why u16 sums by default:** 64 frames of 1023 fit 16 bits (65472), so the add reads and writes half the bytes of u32 sums and NEON adds eight lanes, not four; every 64th frame each row folds into the u32 total (once a row per 64 frames, not a branch per sample). Work 3 measures plain u32 for comparison.

**Why reading is separate (work 1) and map only (work 0):** the view is non-cacheable (the front end does not snoop), and reading it is the measured cost: a whole binned frame took 60 to 66 ms on one thread in camtest, 14.4 ms in the driver's binning copy, 25.8 ms in Lumis across the cores (2026-11-14/15). Work 0 shows what the front end delivers with no CPU on the frames; work 1 the read's ceiling; 2 and 3 the add on top.

### Expected numbers (arithmetic, before any run)

- **Main, binned, cut**: the line is 3.2237 us (measured: 21362 lines a 68.9 ms frame). The default cut, 3072 + 256 = 3328 lines, is 10.7 ms, **93 frames a second** if the sensor takes it (the sweep finds its least). The link carries 4080 RAW10 samples a line either way (about 12.7 Gbit/s while lines run; the cut shortens only the blanking): 1.46 GB/s of image averaged at 93 a second, and the front end writes the same. The read out of the view decides the bench: 15.7 MB a frame at 0.25 to 1.1 GB/s a thread is 3.6 to 16 ms on four threads, so between about 60 and 270 frames a second of reading; the add is about 1.5 ms of a thread a frame (12.5 M samples, 32 a block). Expect the read, not the sensor or the add, to be the ceiling at work 1 to 3.
- **Main, 1:1 (if the guess streams)**: lines of 12640 clocks, 6.45 us; 6400 lines, 41 ms, **24 a second**; 63 MB a frame (60 MiB buffers), 1.5 GB/s; reading 4 times a binned frame, so 15 to 60 frames a second of reading on four cores, near the sensor's.
- **Ultrawide, binned, cut**: lines 6.5616 us (from its init: 9568 clocks at 1458.2 MHz); 3256 lines, 21.4 ms, **47 a second** (30 now); 15.2 MB a frame.
- **Ultrawide, 1:1 (guess)**: 13.1 us lines, 6256 lines, 82 ms, about 12 a second.
- **Main, binned slit**: October's floor 825 us, **1212 slits a second** at 4 rows (24.8 MB a second, 20 KiB a slit), 954 a second at 64 rows on its model (about 443 us past 3.15 us a frame line). If the floor was the missing hold's, the default 4-row frame (132 lines) is 425 us of lines, up to about 2350 a second.
- **Per slit, the front end**: the frame thread's answer is about 57 register accesses (each a syscall: the driver's DMA_DONE to MAP measured 20 us on average), so about 4 to 7 percent of a core at 1212 a second. The engine runs the whole job's program every frame: 1447 writes and 3048 loaded words for the main; at 100 to 200 ns a write that is 150 to 300 us of an 825 us slit, the likeliest front-end ceiling; the slim program (flag G#10) re-arms only loch0's paths, 21 writes (176 bytes; G#20 adds the processing blocks' request words, 680 bytes).
- **The slit ring**: descriptors written once at the open (descriptor e names buffer e mod the ring), so a frame costs the CPU the answer and one index write; 2^8 buffers of 24 KiB cover 211 ms at 1212 a second, and the bench's SLITS calls need to come at least that often; a long ring of 2^16 descriptors (if the count register holds it; else 2^14) is 54 s at 1212 a second, after which the stream ends.
- **What only the device can tell**: whether a mode written before stream on starts at all (October's never did, without the hold); the sensor's least frame and blanking; whether the front end takes a 4-row frame (with the frame's sizes rewritten; October's matched slit predates the G#81 enable, so it never had a chance); how long the PDMA program takes; the view's read rate on four cores at once; RAW12 support; anything 1:1.

**Where the ceiling is, read off a run:** RATE's sensor rate under the asked frame means the sensor (its floor); the front end's rate under the sensor's means the front end (late descriptors: STATS late, SLITS lost; or the engine's program: compare slim); MAP's taken rate under the front end's with cores used near four means the CPU (work 1 against 0 separates the read from the add); the link is never near its rate at these sizes except 1:1.

### The 1:1 captures (need the owner; not run)

What the 1:1 guess lacks is Android's own maximum-resolution mode: the sensor's writes when Lumis opens a camera in SENSOR_PIXEL_MODE_MAXIMUM_RESOLUTION (Lumis's second entry per camera, "50 MP" on the main), and the front end's program for that frame.
One camera, one bus a capture (feedback_oracle_capture_integrity), as the tele's recipe above: the Pro on USB, Android under the hv (the gate's volume down), adb with root, Lumis installed on Android, the phone still on a stand.
Save what slot a runs first and flash it back after (the tele's recipe says how).
The sensor's bus, the essential one (the main's hsi2c_1 below; the ultrawide's G#108A_0000, the tele's G#108B_0000 with their own quiet lists, the same nine offsets):
```
FERROS_HV_STAGE2=1 FERROS_HV_ORACLE=1 FERROS_HV_NO_PMU_READ=1 FERROS_HV_ORACLE_PAGES="G#1088_0000" FERROS_HV_ORACLE_CAP=32768 \
FERROS_HV_ORACLE_QUIET="G#1088_0000,G#1088_0004,G#1088_0008,G#1088_0020,G#1088_0024,G#1088_0030,G#1088_0040,G#1088_0048,G#1088_0050" \
FERROS_HV_ORACLE_ARM_AFTER_MS=60000 tools/husky-hv.sh --no-ferros --flash
# past 62 s of uptime: quiet the console and mark
adb shell 'su -c "echo 4 > /proc/sys/kernel/printk; echo \"<3>ferros-mark: lumis maxres open\" > /dev/kmsg"'
# by hand: open Lumis, pick the main camera's maximum-resolution entry, let it run 10 s, then
adb shell 'su -c "echo \"<3>ferros-mark: closed\" > /dev/kmsg; echo c > /proc/sysrq-trigger"'
# after the reboot
adb shell 'su -c "grep -aE \"ferros hv|ferros-mark\" /sys/fs/pstore/console-ramoops-0"' > target/hv/main-maxres-i2c.log
```
Then the PDMA dump of the same session (`FERROS_HV_PDMA_DUMP=1`, pages G#1C39_0000, cap 4096, as the tele's step 1) for the front end's program at 8160 x 6144, and optionally the 42 front-end pages (the tele's step 0) for the link's channel words.
Check: the I2C log decodes with no LENGTH MISMATCH (`tools/oracle-i2c`), holds a G#0100 stream on, and its G#034C/G#034E read 8160 x 6144 (G#1FE0, G#1800) in the decode.
From it: `tools/oracle-i2c --init` gives a 1:1 init (target/cam/main-full.init, local, the vendor's settings), whose mode `ccs::Mode::captured` reads; the driver then wants that init as a second input per camera (`cam.<key>.init.full`: built 2026-11-18, made by tools/oracle-i2c `--init` from the 16 s capture of 2026-11-17, Lumis on Android in this very mode; kind 2 replays it instead of writing the guess: "1:1 on the main").
The tele's 1:1 waits on its first capture (it has never streamed); its Lumis entry is the tele-virtual camera (physical id 6) and, per the bug report, the HAL may serve it from another sensor: check the I2C log is the tele's bus.

### Run plan (for the owner's return)

Risks first:
- **Nothing electrical is new**: the modes are sensor registers inside its hold and front-end registers the driver already writes; no rail, voltage or PMU access changed; the overvoltage guard is untouched.
- **A stuck sensor**: every mode is written before stream on (the default), and each open powers the module up from off (the switch's off at every close powers it down), which October's stuck runs never had between tries. If an open's RATE shows 0 sensor frames, the next open is a cold module; if that also shows 0, a cold boot of the Pro (the owner) clears it. The sweeps stop at the first open that does not stream.
- **A frame too short stops the stream** (October: 68 lines at 4 rows): the frame sweep goes from long to short and stops there.
- **Mid-stream (flag 2)** is October's order and October's trap: last, and only if before-stream-on never starts.
- **1:1 and RAW12 are guesses**: last; a 1:1 attempt that hangs the front end needs a cold boot.
- **Memory**: a 1:1 ring of four is 302 MiB in one run; the driver tries rings of 4, 3, 2, 1 and goes above 4 GiB (measured to work), and says when the pool has no run.

The steps (FERROS_PORT the Pro's; `B=/mnt/Harbor/Code/ferros/target/debug/ferros-bridge`, `O=/mnt/Harbor/tmp/camtrials/out`; every cambench line the same but its 16 bytes, `$B spawn $O/cambench.bin <bytes> --name cambench --use camera`, then `$B diag "console kept 64"`):
0. **Back up** `camera` and `camtest` (`$B store get <k> /mnt/Harbor/tmp/camtrials/back/<k>`).
1. **The kernel**, a minute after a boot: `$B reload $O/ferros_kernel.elf` (RAM only), then attest. An old driver works on it unchanged; this driver on an old kernel cannot take a 1:1 region (its window is 224 MiB) but streams the binned modes.
2. **Store** `$B store put camera @$O/camera.capsule.vsf`, `$B store put camtest @$O/camtest.capsule.vsf`, `store get` each and `cmp` (native: no seal). cambench is spawned from the host, not stored.
3. **Regression**: camtest `4001` and `4008` as on 2026-11-15 (the view at 15 a second, every frame where the ring put it; Lumis the same). The view's base now reads G#51_2000_0000 or past.
4. **The capture's ceiling at the init's mode**: `00000000000000000000000000040000` (map only), `...0104...` (read), `...0204...` (u16 add), `...0304...` (u32 add), i.e. `00000000000000000000000001040000`, `00000000000000000000000002040000`, `00000000000000000000000003040000`. Expect RATE about 14.7 and 14.7, no skips, CAPS showing frame 21362 and the formats, the NEON check "matches", per-frame read and add times and cores used: these are the cost a frame at any rate.
5. **Binned, the frame cut**: first the mode's own frame thru the new path, `00010000FFFF00000000000002040000` (integration 16 lines, so a dark frame: the sums' mean near 64); then the frame sweep `00010000000000000000000002040200`. Expect the rate to rise as the frame falls until the sensor's floor (stops) or the read's ceiling (skips).
6. **The binned slit**: one 64-row slit `00034000000000000000000002080000`, then the rows sweep `00030000000000000000000002080100` (64 down to 2), then the frame sweep at the narrowest that streamed, e.g. 4 rows `00030400000000000000000002080200`; then the slim program at the best: `00030400000000001000000002080000` and level 2 `00030400000000003000000002080000`. Expect October's 1212 a second at 4 rows or more; compare STATS late and SLITS lost with and without slim.
7. **The ultrawide**: the same with +0 = 01 (`01000000000000000000000002040000`, `01010000FFFF00000000000002040000`, `01010000000000000000000002040200`, `01034000000000000000000002080000`, `01030000000000000000000002080100`).
8. **RAW12**, only where CAPS said G#0C0C: `00010000FFFF00000100000003040000`; the console's CAPS line shows G#0112 as written; frames that land read as RAW12 (the sums' mean about 4 times RAW10's), or the front end writes nothing (no frames: then its format word is the missing piece).
9. **1:1 (guess)**: `00020000000000000000000002040000` and the 1:1 slit `00040400000000000000000002080000`. Expect either frames (8160 x 6144, the sums' mean sensible) or no frames and a cold boot; either way, then the captures above.
10. **Mid-stream**, only if step 5 or 6 never started before stream on: `00034000000000000200000002080000`, then a cold boot.
11. **Restore** if anything misbehaves: put back what step 0 saved and reboot to the flashed kernel.

### On the Pro, 2026-11-16 (b897 the flashed forge kernel, b898 this branch's kernel RAM-reloaded)

The owner's go; the run plan above in order, the bench on three A715 threads unless said. Logs: /mnt/Harbor/tmp/camtrials/runs.

**Before the reload: the jump counters.** b895 was a cold boot right after a jump that reset (b894), and the hv's counters at G#FD60_0D00 still read the magic "HVCORE01", jumps begun 1, last stage 6: what `reload_in_jumped_session` takes for a jumped session, so a reload would have stood the drivers down. Word 0 was cleared with two pokes and read back 0 (the coordinator's call); the reload then read GATE_ANSWER 0, cold. The real fix (a cold boot marking the counters stale, or the check tying them to this boot) is for the next jump work.
**The first reload died** (b896): about 16 s after it, as its pd gating pass logged every domain "already off", camtest's open came in and the Pro reset with nothing more in the flash log (no "camera: on" line). The second reload (b898), left idle 79 s past its pass, ran the camera for an hour. Not reproduced: likely a camera open meeting the reload's gating pass; noted for whoever owns pd.rs.

**The main, binned, the init's own mode (15 a second), 64 frames, four threads** (b897): map only 0 us a frame; read only (64-byte loads out of the view) 5.2 ms of wall work, 18.2 ms of thread time (0.86 GB/s a thread, about 3 GB/s on four); read and u16 add 10.4 ms (19.3 reading, 19.5 adding); read and u32 add 11.7 ms (21.9, 21.2). The NEON unpack matches the plain one on the device. The frame thread's answer: 189 us on average, 295 us at most.
**CAPS**: the main offers G#0A0A, G#0A08, G#0808 (RAW10, 10 to 8 compressed, RAW8); the ultrawide G#0808, G#0A0A, G#0E0E (RAW8, RAW10, RAW14). Neither offers RAW12, so the RAW12 step was not run. Integration least and margin: main 2 and 2, ultrawide 1 and 64.
**The mode before stream on, inside the real hold, works**: the main's integration of 16 lines landed (the frames' mean 64, black) and every open after streamed. October's "never starts before stream on" was the missing hold.

**The main, binned, the frame cut** (the sweep, b897; the sensor's own counter): 21362 lines 15 a second, 7168 44, 4096 77, 3328 95, 3200 and anything shorter **96**: a floor of about 10.4 ms (about 3230 lines of 3.17 us) the sensor keeps itself; it never stopped, down to the rows and one. The front end delivers 94 to 97.
**Integrating at 96 a second** (256 frames, three threads; b897 and b898 alike): u16 97 a second, 0 skipped, 7.1 to 7.3 ms of wall work a frame, 2.0 of the 3 cores; u32 96, 1 skipped, 8.8 ms, 2.5 cores; read only 6.0 ms, 1.65 cores.
**Four threads at 96 a second lose the stream** (five of eleven sweep steps, b897): the bench took 3.5 to 3.8 cores and the driver's frame thread, on the same A715s, missed the front end's deadline; it stopped for good after about a second. **Pinning the frame thread made it worse** (b898): on the X3 (the owner's call) the main stopped after 10 frames at 96 a second, the ultrawide above about 38; on one A715 of its own (core 7) the main stopped too. Unpinned, with the consumer leaving one A715 free, every run was clean. The driver stays unpinned; a consumer at these rates leaves one A715 free.

**The ultrawide** (b898; its first frames thru the driver, the open 571 ms, the first frame 612 ms after it): its own mode 30 a second, read and u16 add 12 ms of wall work a frame, 1.04 of 3 cores. The frame cut: sensor 7096 lines 20, 4024 37, 3256 46, about 50 at its floor (3004 lines); the front end **48 a second at 3128 lines** (128 blank, 256 frames, 0 skipped, 1.5 cores integrating), nothing at 3032 (32 lines, 210 us of blanking). The slim program kills it (one frame).

**Slits: the sensor runs them, the front end takes one frame.**
- The main's binned slit written before stream on: **1169 a second** on its own counter at 64 rows (October's 1184), 1126 written mid-stream. One frame lands per open, then none: the same at 64 and 1024 rows, at 1170 and 16 a second (the frame left long), with the geometry rewritten (Core, All), the slim program, or the front end left at the captured geometry. The link says why: +G#14 = G#20, ERR_CRC_PH (packet header CRC, isp_fe.h), frame starts on all three channels and no frame end. A y window changed thru CCS alone corrupts this Samsung sensor's packets; its frame length alone does not.
- The ultrawide's binned slit (Sony): its link is clean (frame starts and ends on both channels, no error), and the front end takes two frames and stops (100 answers, no DMA_DONE), with the geometry rewritten Core or All.
- No sensor stuck: every next open streamed (the switch powers the module down at each close), mid-stream included. October's "stuck until a cold boot" was a sensor never power-cycled between tries.

**1:1 (the guesses)**: the main takes the registers (CAPS reads 8160 x 6144, increments 2, binning G#00 G#11) and runs about 2 frames a second on its counter, its link with ERR_CRC_PH, no frame. The ultrawide streams 11 a second (its predicted 82 ms frame) but sends lines of its binned width: the link reports both resolution mismatches (+G#1C = G#60). Neither is a mode the sensor runs whole from CCS alone; both need the captures ("The 1:1 captures").

**The coordinator's ideas for the main's slit, not yet tried**: the window with the PD channel and embedded data off (Samsung's PD enable, CCS embedded-data control) and the link set for channel 0 alone, since a window that does not match the sensor's PD layout may be what corrupts the headers; and a capture of Android's 2x on the main, an in-sensor crop of the full readout, which would show Samsung's own windowed writes (the recipe above, with the camera app at 2x instead of Lumis at its maximum resolution).

### The kernel lock and the frame thread (2026-11-16, kernlock)

The owner: "Measure the kernel lock, see if we can dedicate it something, would be nice to truly max out the sensor readout." KERNEL.md "Order" item 7 has the kernel side (`diag lock`, `diag dev`, the `dev first|follow|core` switches); this is the camera's. Logs: /mnt/Harbor/tmp/kernlock/runs.

- **The deadline is the blanking.** At the main's 96 a second (fll 3200: 128 lines past the rows, about 400 us) the next descriptor must be released before the next frame begins, and the PDMA engine then runs the frame's program. A stream that died read, on the driver's new quiet check: image status G#1 (a frame begun), aggregate 0, the engine's index the driver's own; the frame never finished and no answer revived it. With every answer in time no run died.
- **The driver writes the index first now**: at DMA_DONE the ring's turn and the index write come before the 56 acknowledgements, 12 us into the answer on average (it was about 170). An answer still takes 100 to 170 us in all. The close line says both.
- **What it needs from the kernel**: a core within tens of microseconds. With nothing switched on, every one of 10 runs with the u16 add died in its first 45 to 163 frames (read without the add, 3 of 3 ran). With `dev first` (3 or 4 threads) or `dev core 7` + `dev first` (4 threads), 6 of 6 runs of 2048 frames ran clean on the second kernel, interrupt to run 24-30 us on average, 70-90 us at most. Pinning instead kills it: on the X3 in 11 to 19 frames, on core 7 alone in 5.
- **Use**: `diag "dev first on"` and `diag "dev core 7"` before a heavy consumer; the bench then has three cores, the frame thread and the other device threads the fourth.
- **The readout's ceiling** (core 7 + first, integration 2 lines, fll 3200, 1024 frames each; the sensor's own counter in RATE): line length 6320 (the init's) 96 a second, the front end 94, clean; 5888 102 and 101, clean; 5632 107, 5376 112, 4864 and shorter 116 (the sensor's least line, about 5370 clocks): the front end takes one frame and stops, with a longer frame too (5376 at fll 3328 and 3456). So the main's front end (or its link setting) needs lines of about 5888 clocks or more: **101 frames a second is the main's binned top on ferros today**, the sensor's own 116. Between 5632 and 5888 untried. Integrating at 5888 (u16 add, 3 threads): 1024 frames mapped at 103 a second, 4 skipped between them (as at 6320: 5), 7.4 ms of wall work a frame, 2.2 of the 3 cores.
- **The Pro reset** during that sweep (the second kernel, about the 20th open on it, an open at 5632 x 3328 after three opens whose front end took one frame): cause not yet read (the flash log after the next cold boot).
- **The reset, read** (b901): the last flash-log line was the switch's "on in A#42 ms"; everything later in an open went to the console only, lost with the DRAM (the session power-cycled: every PREV note FF with flipped bits, ABL's own record overwritten by the Android boot in between). The driver now writes a flash-log line before each step of an open (`step!`, ferros_sys `console_flash`: switch on, init done, job placed, engine starting, engine started, receiver on and lens set, stream on, first frame), so the next open that kills the SoC names its step. The close is unchanged: Linux's front end is driven by the closed camera HAL thru LWIS, so nothing beyond the captured close already replayed is grounded.
- **Kernel #3 and #4 (dev first on by default) hung twice right after the person attested**: not the camera. Two cores sat wedged in PCIe BAR accesses left by the RAM reload (WIFI.md, last section); the first spawn of fgtw or the map then hung with the lock held. With WiFi and cellular off by the radios preference the attested reload ran.
- **In that attested session (kernel #4, the person's processes running) the 96 fps stream still died**: 2048-frame runs with 4 threads, off 0 of 3 (100 frames), `first` 0 of 3 (361 to 793 frames), `core 7` + `first` 0 of 3 (100 to 1155 frames), `first` with 3 threads 0 of 1 (606). The answers were in time (interrupt to run 23 to 41 us on average, the index written 11 us into the answer, 59 us at most), and the death has the same signature (image status G#1, the engine's index ours). So with the shell and the person's processes up, something other than the frame thread holds the front end past the blanking: unmeasured (the engine's program fetch and its writes against the DRAM traffic of four reading threads and the person's display is the first candidate; the PDMA's own status words during a death are the next thing to read). The un-attested session (kernel #2) ran 6 of 6 clean with the same switches. The torch held the camera PMIC's bus once (the switch refused, as designed).
- **Not yet run**: the ultrawide's line-length sweep (its line 9568 clocks, its front end needing 128 blank lines), and the kernel fast path (a checked list of register writes run in the interrupt handler) that would take the frame thread off the deadline entirely.

## Gain (2026-11-17): the sensor's own model, measured on the main

The owner, using Lumis on the Pro: "the ISO doesn't do anything in lumis, everything else looks to be wired."
Lumis sends ISO as gain in 1024ths of one (ISO 100 = 1024) and holds it inside the list's `gain_min` and `gain_max`, which were 1024 and 1024; the driver took 1024 alone ("the captured gain only"). So ISO was pinned at 100.

**Built** (camtrials): the driver reads each sensor's gain model from its own CCS words at the open (G#0080 to G#0093: the capability, codes least and most and step, gain = (m0 code + c0) / (m1 code + c1); G#1084 to G#1089: the digital gain's least, most and step, 8.8 fixed point) and says the words on its console (ferros_cam ccs `Gain`). SET gain is split into the analogue code up to the analogue's most and digital gain past it, both written inside the hold as exposure is (since the owner's word the same day, analogue only: below); a frame's header carries the gain the codes make, and the init's own gain is reported as it is (the main's init leaves code G#400: 32x, not one). A sensor whose words describe no model keeps the captured gain alone. The list's range comes from ferros_cam ccs `ARRAYS` (filled from a measurement: the list is built with no sensor powered). camtest's fourth byte 3 measures the curve (1x to 640x in half stops at a fixed exposure).

**The main's words** (the Pro, b913, the flashed kernel, 2026-11-17): G#0080 = 2 (CCS's "alternate" capability; its init drives G#0204 regardless), codes G#20 to G#1400 step 1, m0 1, c0 0, m1 0, c1 G#20: gain = code / 32, **1x to 160x analogue**; digital G#100 to G#8000 step G#100: 1x to 128x in whole steps. The list offered 1024 to 20971520 at first (ISO 100 to 2048000, analogue and digital); the owner, the same day: "digital gain is after the ADC, so analogue only". Now the list offers the analogue range, **1024 to 163840 (ISO 100 to 16000)**, SET writes digital gain as one always, and a gain past the analogue's most is refused (`[1]`, outside the range), never capped; the ultrawide follows the same rule once its model is read.

**The curve** (camtest `1008000300`: bin 8, 16 frames a step, the init's 68.8 ms exposure, a dim room; the mean sample in 16ths, black 1022):

| gain | 1x | 2x | 4x | 8x | 16x | 32x | 64x | 128x | 160x (analogue most) | 320x (digital 2x) | 640x (digital 4x) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| mean above black (16ths) | 0 | 2 | 7 | 20 | 47 | 99 | 206 | 419 | 476 | 949 | 1917 |
| per doubling | | | | 2.9 | 2.35 | 2.1 | 2.08 | 2.03 | (x1.25: 1.14) | 1.99 | 2.02 |

- Linear: twice the signal per doubling from 8x up, within the noise of a dim scene; below 8x the signal is under a code above black, so the ratios there say little.
- The analogue ends at 160x as the words say: asking 181x or 256x gives 160x (digital steps are whole, so the next is 320x); 160x over 128x gave 1.14 for 1.25, the one step that falls short (unexplained: clipping at black, or the top of the analogue range).
- Digital doubles cleanly: 1.99 and 2.02.
- The ultrawide, measured later the same day (b915, forge with this branch: the module switch): its words read capability 0, codes 0 to G#3F0 step 1, m0 0, c0 G#400, m1 -1, c1 G#400: Sony's gain = 1024 / (1024 - code), **1x to 64x analogue**; digital G#100 to G#FFFF step 1 (never used).

**Where the analogue really ends** (the owner: "Was that a typo?" of 160x; camtest `0400000400` lit and `040000040007` dark on the main, `0400000401` and `040000040107` on the ultrawide (the fourth byte 4): four frame pairs a step, the middle 64 rows raw thru the view, the temporal noise from each pair's difference, the codes near the mean; the Pro on a tripod, steady light, b915):

| main, dark (128 us) | 1x | 2x | 4x | 8x | 16x | 32x | 64x | 128x | 160x |
|---|---|---|---|---|---|---|---|---|---|
| noise, 16ths of a code | 13 | 13 | 19 | 22 | 20 | 22 | 30 | 50 | 55 |
| input-referred (noise / gain), codes | 0.81 | 0.41 | 0.30 | 0.17 | 0.078 | 0.043 | 0.029 | 0.024 | 0.021 |
| codes near the mean holding samples | 8 of 11 | 7 of 11 | 9 of 13 | 12 of 15 | 12 of 15 | 15 of 15 | 19 of 19 | 29 of 29 | 31 of 31 |

| main, lit (68.8 ms) | 1x | 4x | 16x | 64x | 128x | 160x |
|---|---|---|---|---|---|---|
| noise, 16ths | 13 | 21 | 44 | 155 | 286 | 315 |
| codes near the mean holding samples | 10 of 11 | 15 of 15 | 27 of 27 | 81 of 81 | 147 of 147 | 161 of 161 |

- **No empty codes at any step**, dark or lit, and the residues mod 4 even (63 to 66 of 256 each) wherever the noise spans more than a few codes: a gain after a 10-bit ADC would leave every 2nd code empty at 2x past it. (The dark low steps' uneven residues are a distribution narrower than a code around black 64, not gaps.)
- **The input-referred noise keeps falling thru 160x**: a gain after the ADC multiplies everything already quantised, so its noise grows exactly with it (x2 a doubling, x1.25 from 128x to 160x); measured x1.67 from 64x to 128x and x1.10 from 128x to 160x. So the gain is still applied before the noise floor there: analogue to the stated 160x, and **the main's ceiling stays 1024 to 163840** (ISO 100 to 16000).
- The caveat the test cannot close: a sensor whose ADC has more bits than its RAW10 output, with digital gain applied before the cut to 10 bits, shows no empty codes until that gain passes the extra bits; the noise growth above still argues against it up to 160x.
- **The ultrawide** (dark, 128 us): noise 7, 8, 11, 16 sixteenths at 1x, 2x, 4x, 8x and 77 at 64x (x4.8 from 8x to 64x, against x8 for a gain after the ADC); every code near the mean holding samples at 64x (43 of 43); lit (its init's exposure) the same, no empty codes to 64x. **Its ceiling: 1024 to 65536** (ISO 100 to 6400), now in `ARRAYS`, so Lumis's ISO works on it too. (The console ring dropped the 11x to 45x lines of its dark run; the runs are in /mnt/Harbor/tmp/camtrials/runs.)

## The preview's split (2026-11-17), measured on the Pro

The owner: Lumis's preview about 14 frames a second.
Lumis (std/lumis, branch preview-split) hand-spawned on the Pro (forge 9eaa7afa, attested), no ira or vault domain (nothing stored), the main camera, about 50 s a run; its probe says every 2^2 s the UI's passes, the passes with a new image, the images never drawn, Lumis's draw and the convert; its camera line every 64 frames the sensor's period by the driver's clock and the frames never taken; fluor's line the bake (present) and op 22; `diag clock` the A715s' clock.

**The path**: sensor, the driver's ring (the view; the 6 MiB reply is no longer on the path), Lumis's camera thread (MAP, copy out of the view and unpack to 16 bits, the integrator's whole finalize every frame at exposure 0), then the UI thread (Lumis's CPU draw of the camera image into an RGB888 frame of the panel's size, the convert to G2t16, fluor's clear and bake, op 22, the display's publish).
The two threads overlap, but share one rayon pool of four on the four A715s with the display's publish workers and the driver's frame thread.

**The init's mode** (67.27 ms, 14.87 a second):
- The camera thread takes every frame: waiting 14 to 41 ms, unpacking 10 to 26, the integrator 15 to 30.
- Lumis's draw of a new image: 37 to 85 ms, in two states. Passes 35 in 4.2 s, every one a new image, draw 82 to 85 ms: 8.4 new images a second, 27 of 61 never drawn. Or passes 115, 58 new, draw 37 to 39 ms: 14 a second, 2 to 3 never drawn.
- The two states are the kernel's A715 lift (service.rs `interactive`): 2367 MHz while shown frames come within 2^-4 s of each other, else 1065 MHz. At 85 ms a draw the frames come too far apart, the clock stays calm and the draw stays slow; the ratio of the draws is the ratio of the clocks.
- Every pass whole (before redraw on change): fluor draw 44 to 68 ms a frame, present (bake) 11.3 to 15.8 ms, clear 4.7 to 5.9, Lumis's convert 4.2 to 6.6; the display's publish 9 to 10 ms a show.
- Redraw on change: present 5.0 to 7.7 ms and convert 0.8 to 3.3 ms a pass on average; new images drawn the same (34 to 55 of 60 in 4.1 s): the draw decides them.

**The binned cut** (kind 1):
- fll 4096 (77 a second) and fll 3328 (94): the stream dies in 2 to 3 s under Lumis (`no frame in A#2097 ms`, image status G#1), three times of three, as the 96-a-second deaths in an attested session.
- fll 7168 (22.57 ms, 44.3 a second), redraw on change: the camera thread is the wall, 52.6 to 57 ms a frame (waiting under 1 ms, unpacking 15 to 27, the integrator 28 to 41): 17.5 images a second, 80 to 97 of every 160 of the driver's frames never taken.
- Lumis draws 33 to 39 of those 70 to 77 images in 4.1 s (8.5 a second), every pass a new image, the draw 58 to 70 ms, fluor's present 24 to 27 ms; the A715s at 1065 MHz, busy 256 of 256 (1588 saturated windows), never lifted.

**So, for 120**: the sensor's 15 is a wall only with nothing else in the way; above it, at the A715s' calm clock, the camera thread caps at about 18 and Lumis's draw at 8 to 14. Ranked: Lumis's draw (a develop at panel size straight to the panel's codes, or the GPU), the camera thread's intake (cambench's NEON read-and-add from the view, 7 to 9 ms a frame at 96 measured), the lift's trap (a camera stream or a busy cluster is an animation too), the stream's death above 44 a second in an attested session, then the bake and the publish.

## Long exposures (2026-11-17): built, not run

The owner, relayed by the Lumis session: "this needs done", Lumis on ferros exposing as long as on Android, up to 16 s on the main.
Until now the longest exposure was the init's own frame: the driver wrote only the integration (CCS coarse_integration_time, G#0202) inside the hold, never the frame, so the main stopped at 21342 lines of 3.2237 us, 68.8 ms (the ultrawide at 33 ms, the front at 14 ms).

**What changes** (ferros_cam ccs `Timing`, payloads/camera `exposure_set`), in the order that moves the least, each inside the camera's own hold so line, frame and integration land at one frame's start:
0. Inside the mode's own frame: the integration alone, as before; the frame rate stays the mode's.
1. Past it: the frame length (CCS frame_length_lines, G#0340) lengthened to the integration plus the sensor's margin, to 16 bits. The margin is the sensor's own word (CCS coarse_integration_time_max_margin, G#1006), read at every open: the main answers 2, the ultrawide 64 (CAPS, 2026-11-16). The main reaches 65533 lines, **0.206 s** (0.211 s at the old line time); the ultrawide 65471 lines of 6.56 us, 0.430 s.
2. Past that: the line length (CCS line_length_pck, G#0342) lengthened on a step of 2^4 clocks (every line this sensor has run on ferros is on it) to the longest line time the front end has taken (`LINE_PS_MOST`, 15.27 us: 30640 clocks on the main, 22256 on the ultrawide; the Pro, b932, below), the frame at its 16 bits. The main reaches 65533 lines of 15.27 us, **1.000 s**; the ultrawide 65471, **0.999 s**. Built first to 65520 clocks (2.19 s and 2.94 s): the main's stream stopped at 61280 (2 s).
3. Past that, to 16 s: the vendor's long-exposure shift. Android's own hold on the main writes two words every frame, G#0702 and G#0704, both 0 at ordinary exposures (the 2026-10-19 open capture, target/hv/cam-open3, local): Samsung's frame and integration shifts. How a long exposure writes them (which byte carries the shift, its most, whether the stream stops around the change) is not guessed: the capture below shows it, and then `ARRAYS[0].lte` gets it (ferros_cam ccs `Lte`, built and host-tested with an example encoding). Captured the same evening (below): built.

The list offers what the camera takes (`exposure_max`, ferros_cam list from `Cam::listed_timing`): the main 64.0 s with its shift (below), the ultrawide about 1.0 s; the front and the tele, whose margins were never read, their init's integration as before. SET refuses past it, never caps (as gain does). Lumis reads `exposure_max` from the list, so nothing changes on its side.
A camera without a margin in the table, or whose margin read fails or makes no sense at an open (a word not under the mode's frame), keeps to its mode's frame less 2^5 lines, as before. The console says the margin it read and the longest exposure it takes at each open, and each change of the frame (`camera: exposure A#.. us: frame A#.. lines of A#.. clocks, shift A#.., integration A#.. lines; a frame every A#.. ms`).

**Mid-stream: the frame in the hold with the integration.** From the docs and the code, not yet from a run:
- The frame length written inside the real hold never hurt the main: the 2026-11-16 sweep cut it from 21362 to 3200 lines and every open streamed; "a y window changed thru CCS alone corrupts this Samsung sensor's packets; its frame length alone does not".
- Android does it every frame: its hold on the main is the shift words, the gains, the integration and the frame length (target/hv/cam-open3, the per-frame blocks), so a frame that moves with the exposure inside the hold is the HAL's own way, not a new one.
- October's "stuck until a cold boot" was writes outside a hold (the harness wrote the release for the hold) on a sensor never power-cycled; since 2026-11-16 every close powers the module down, and every next open streamed, mid-stream writes included.
- The line length moving mid-stream is in no capture. It is step 2's risk, and the first thing the device test measures. If it fails, the fallback is OPEN's mode block, whose line length is written before stream on (run at 4864 to 6320 clocks on 2026-11-16): a long exposure would then reopen the stream (about 2.3 s on the main).
- What a wedge costs: a camera-process restart. A stream that stops closes itself ("no frame in A#.. ms"), and its close, or the process's end, turns the switch off and powers the module down; the next open is a cold sensor. No kernel reload. Only a front end that takes no frame at two opens running would want a cold boot of the Pro (the owner's), and on 2026-11-16 the Pro reset once after three stuck-front-end opens in a row: the test stops at the first open whose frames stop.

**The frame wait and the holder** (payloads/camera):
- A FRAME, MAP or SLITS waits `late_us`: 2^21 us past two frame periods, the period the sensor runs now plus, for two frames after a change, the one before (a frame begun under the old timing may still come). It was 2^21 us flat, so `no frame in A#2097 ms` closed any frame past about two seconds.
- The holder's quiet time (2^22 us with nobody asking) counts from the driver's last answer, not from its call: a MAP answered after a 16 s frame found its holder "gone" at once before.
- The frame thread still looks every 2^17 us (the close waits on it, and nothing may touch the front end after the switch is off), but says "the stream stopped by itself" only past two frame periods of quiet.
- A frame's header carries its exposure in ns as set (line time at the line length it ran, the shift), no longer lines at the init's line.

**Lumis's preview while a long frame integrates** (std/lumis): its camera thread waits in MAP for the frame; its UI thread draws on from the integrator's memory, and lumis_core's own elapsed and remaining labels (the exposure's start in the shared header) count the exposure down, as on Android. Not built: Lumis on ferros sends a setting after each frame it gets, so a change during a long frame lands after the frame in flight (Android's Lumis pushes from a 30 Hz poll). At 2 s that is one frame; for 16 s the next step is the driver answering SET while a MAP waits (its reply held, the server back in its loop) and Lumis pushing from its UI thread. Built 2026-11-18: "Serving while a frame waits" below.

**Device test** (the coordinator's go; no kernel reload, nothing stored: the driver and camtest hand-spawned, the module power-cycled at every close):
```
B=target/debug/ferros-bridge; R=target/el0/aarch64-unknown-none/release
cargo el0 -p ferros-payload-camera && cargo el0 -p ferros-payload-camtest
aarch64-linux-gnu-objcopy -O binary $R/camera camera.bin && aarch64-linux-gnu-objcopy -O binary $R/camtest camtest.bin
$B spawn camera.bin --name camera --serve camera --hw hsi2c1 --hw ispfe --hw csis --hw ispfe-sysreg --store cam --power camera --dma camera --irq ispfe0 --hw hsi2c3 --hw hsi2c2 --hw hsi2c4
$B spawn camtest.bin 0408000500 --name camtest --use camera      # the main: 64 ms doubling to 2048 ms, back to 64 ms
$B diag console
```
camtest's fourth byte 5 is the ladder: each step's SET answer, the frames' spacing, the mean sample, the header's exposure and the sensor's frame, line and integration read back (CAPS). Expect: the open's `integration least A#2 and margin A#2`, `exposures to A#1024 ms` (A#2190 before the cap); frames 68.9 ms apart up to 64 ms; at 128 ms the frame line and frames about 128 ms apart (the frame lengthened); from 256 ms the line lengthened and frames about the exposure apart; the mean doubling a step until it saturates; back at 64 ms, 68.9 ms again. The first step that writes the line (256 ms) is the mid-stream measure: frames that stop there say the line must go before stream on.
Then the ultrawide (`04080005010B`, to 2^11 ms), then Lumis hand-spawned (above, "For the morning") with the shutter at 2 s: a frame every 2.2 s, the UI live between.

**On the Pro, 2026-11-17** (b932, forge 9eaa7afa flashed, the owner attested; the driver and camtest hand-spawned, nothing stored, no reload; logs in the session's scratchpad). Two traps first: a driver spawned by hand ends after a LIST that leaves nothing open, so camtest's LIST took it and the OPEN behind started the vault's driver (the ladder now opens first, without LIST); and the first driver binary was a stale build. Then the main's ladder, its own driver:
- The open: `integration least A#2 and margin A#2 (the list's margin A#2); frame A#21362 lines of A#6320 clocks; exposures to A#2190 ms`.
- 64 ms: frames 68.2 ms apart, the init's frame. Mean 1400 sixteenths (black 1024).
- 128 ms: the frame to 39707 lines mid-stream in the hold, taken: header 128.0 ms, the sensor reads 39707 / 6320 / 39705, mean 1779 (376 to 755 above black: twice).
- 256 ms: **the line moved mid-stream and the stream ran on**: 65487 lines of 7664 clocks, frames 247.5 ms apart, mean 2490 (1466 above black: twice again).
- 512 ms: 15328 clocks, frames 500.8 ms apart, mean 3703. 1024 ms: 65521 lines of 30640 clocks, frames 999.6 ms apart, mean 5779.
- **2048 ms: the stream stopped.** 65521 lines of 61280 clocks; 131 ms after the change the front end sat with image status G#1 (a frame begun, never ended: the 2026-11-16 death's signature), aggregate 0, the engine's index the driver's own; the driver answered it, no frame came, and at 8.2 s it closed (`no frame in A#8241 ms`, 30 frames in all). The link had seen frame starts and ends (+24 and +2C G#7, no error bits). The switch powered the module down; nothing else was run (the rule: stop at the first open whose frames stop).
- So the sensor takes the frame and the line mid-stream in the hold, and the front end takes lines to 30640 clocks (15.6 us) but not 61280 (31.3 us). Where between is not measured; the shift (the capture) keeps the line at 6320 and would not need either. So the most offered is now the 1 s step's line time (`LINE_PS_MOST`), about 1.0 s without the shift, until a bisect between 15.6 and 31.3 us or the shift moves it.

**The capture for step 3** (tools/husky-hv-longexp.sh; the owner's Android session):
- The camera: the main (boitata, Samsung G#08EF), sensor at G#3D on hsi2c_1 (G#1088_0000). One controller, and on it the sensor's transfers alone: the hv's new knob `FERROS_HV_ORACLE_I2C_TARGET` logs a controller's ADDR, AUTO_CONF and TX writes and RX reads only while that target is selected, so the lens (G#0C), OIS (G#24) and ToF (G#29) polling on the same bus (seven of every eight transfers after stream on) and the controller's status polls stay out, and the whole session fits the console ring. Logging is not capped: the ring keeps the newest, which is the 16 s change and after.
- The image: the oracle hv in front of Android's kernel, no ferros, the UFS firewall on, console at level 4 from boot, armed 60 s into the hv's counter (about 40 s of Android's uptime), footered with the owner's key on this morning's signed boot image (as b929's flash). `build` makes it on the host.
- Ours: `build`; `FERROS_PORT=1-2.2 IMG=<capture image> tools/husky-hv-longexp.sh flash <good gate>` writes the good gate (signed, forge aa6d8ec3: /mnt/Harbor/Code/keys/husky/images/boot_forge_aa6d8ec3.img, with its .b3) to boot_b first, so a capture boot that fails falls back to a good slot, then the capture image to boot_a, never a set-active (ferros goes down: the owner's identity with it); `start` once Android is up (it checks /proc/cmdline for the capture's loglevel=4, quiets the console and marks).
- **The owner**: unlock the phone; open Lumis; choose the main camera; set the shutter to 16 s; take one shot; wait until it has landed (about 35 s); say so. Nothing else on the phone.
- Ours: `end capture.log` (marks, panics Android so the crash reset keeps the ring, the next boot traps nothing, pulls the hv's lines and decodes them with tools/oracle-i2c: `.holds` has every hold on one line, `0702=.. 0704=.. 0202=.. 0340=..`); then `restore <good gate>` to boot_a, and the owner attests on ferros again.
- What to read: the hold where G#0702 and G#0704 leave 0, the frame and integration beside them, any line length (G#0342) and any stream off and on (G#0100) around it; the transfers' integrity line (a length mismatch inside that hold means recapture).
- Hazards: if Android does not boot from the capture image, ABL falls back to slot b, which `flash` has just given the good gate (it was older than the capsules stored on 2026-11-17, Claude memories/stored_capsules_need_flashed_kernel.md). The flash goes thru fastboot's own words (a failed 64 MiB send is the cable).

**The capture, 2026-11-17 evening** (the owner at the Pro; boot_b given forge aa6d8ec3's gate first, boot_a the capture image, then restored to that gate; the files local only, /mnt/Harbor/Code/keys/husky/captures/longexp-16s-2026-11-17.*, with B3SUMS): 1404 transfers to G#3D, integrity 0. Lumis opened the main in its 8160 x 6144 mode (x_output G#1FE0, y_output G#1800, increments 2, binning G#2222, line 11600 clocks, the PLL multiplier G#D7), so the capture also holds the sensor's whole init for Android's 1:1 mode (the same 6528-byte vendor upload, blake3 2fc6c49d1d6ece07), the sensor half of "The 1:1 captures" above; its front end's half was not trapped.
- Every hold, as before: G#0702, G#0704, G#020E, G#0204, G#0202, G#0340. The frame is always the integration plus 36 (G#24) in its units.
- At ordinary exposures: 0702 = 0704 = 0, integration G#278A in a G#27AE frame.
- The change to 16 s, mid-stream, a plain hold, **no stream off or on, no line length**: two holds of `0702=0300 0704=0300 0202=D38C 0340=D3B0`, then `0702=0600 0704=0600 0202=8A60 0340=8A84`, then `0202=9446 0340=946A` at shift 6 for the rest (gain G#039F throughout).
- So the shift is the high byte of both words (`at: 8`), the integration and the frame both count in 2^shift lines, and Android takes the least shift that holds the integration in 16 bits (54156 << 3; 37958 << 6, about 16 s at this mode's line). 6 is the most seen.
- In ferros: `ARRAYS[0].lte` = G#0702, G#0704, at 8, most 6. At the binned mode's 3.15 us line shift 6 reaches 13.2 s with the line untouched; 16 s is shift 6 with the line at about 7700 clocks (3.8 us, well under the front end's 15.3 us); the list offers to shift 6 at the longest line, about 64.0 s (65.5 s at the old line time). The ultrawide (Sony) has no shift: its own capture would be on hsi2c_3.
- **Not known yet, and the next device test:** whether the 2 s stop of b932 was the line (31.3 us) or the frame (2 s). Android's front end takes a 16 s frame, but its HAL may set something ferros does not. camtest `0408000500` and then the sixth byte 14 (to 16 s) on the main: 2 s now runs at shift 1 on the 3.22 us line, so a stop there says the frame period, not the line.

**The shift on the Pro, 2026-11-17 night** (b933, forge aa6d8ec3's gate, the owner attested; the driver and camtest hand-spawned, nothing stored). camtest `0408000500`, then `04080005000E` (the sixth byte is the top; `040800050E` names camera G#0E and is refused), every step running, the module powered down at the close:

| asked | plan | frames apart | mean (16ths, black 1024) | the sensor reads (frame / line / integration) |
|---|---|---|---|---|
| 64 ms | own frame | 67.3 ms | 1371 | 21362 / 6320 / 19852 |
| 128 ms | frame lengthened | 102.6 ms (the change inside the 4) | 1718 | 39707 / 6320 / 39705 |
| 256 ms | shift 1 | 250.1 ms | 2445 | 39707 / 6320 / 39705 |
| 512 ms | shift 2 | 500.2 ms | 3625 | same |
| 1 s | shift 3 | 998.3 ms | 5632 | same |
| 2 s | shift 4 | 2003 ms | 8158 | same |
| 4 s | shift 5 | 4003 ms | 11864 | same |
| 8 s | shift 6 | 8004 ms | 14366 | same |
| 16 s | shift 6, line 7664 | 16004 ms | 15480 | 65487 / 7664 / 65485 |
| 64 ms again | own frame | 69.3 ms | 1338 | 21362 / 6320 / 19852 |

- **16 s runs.** The shift moves mid-stream in the hold, the sensor reads its frame and integration back in shifted units, and the front end takes 16 s frames: so b932's 2 s stop was the 31.3 us line, not the frame period. The ladder before it (`0408000500`) ran the same to 2 s.
- The mean doubles a step to 1 s and then bends toward the ceiling (16368, 1023 a sample): the scene saturates, not the exposure.
- Once, at 8 s, the frame thread's quiet look found image status G#1 for 131 ms and answered it; the stream ran on, so a begun frame is ordinary in a long period and not by itself the death of b932.
- **The main's line time is 2.3 % short of what ferros uses.** Every long step's spacing is 0.977 of the period asked (2003/2048, 4003/4096, 8004/8192, 16004/16384), and the 16 s step's 16.004 s over 65487 lines of 2^6 at 7664 clocks gives 3.1489 us at 6320 clocks: exactly 6320 clocks of the PLL at 24.576 MHz / 3 x 245 (2007.04 MHz), the clock the ultrawide's line was already derived from, not the 24 MHz the October reading assumed (`MAIN_LINE_PS` 3.2237 us). The 64 ms frames, 67.3 ms apart, agree (21362 x 3.1489 us = 67.27 ms). So every exposure ferros has reported for the main is 2.3 % long. **Corrected the same night** (coordinator: "It's measured, not a theory"): `MAIN_LINE_PS` 3_148_916 ps, the front end's line cap rederived (`LINE_PS_MOST` 15.27 us, still the 30640-clock line of b932), the main's offer 64.0 s with the shift, 1.000 s without, the ultrawide's 0.999 s (its own line was already from 24.576 MHz, unchanged).
- **What read 2.3 % long before** (each the true value times 1.0237): every main frame header's exposure since the driver ran (2026-11-14), so every exposure Lumis on ferros showed or wrote into a VSF RAW save of the main; the list's main `exposure_max` (68.8 ms, really 67.2); "Focus, exposure" in docs/oracle/CAMERA.md (68.8 ms of integration, the "about 71 ms" estimate camera-raw2vsf was given); the controls test's "a quarter (17.2 ms)" (16.8); the gain tests' exposures in time (the main's 2^16 us and 128 us steps were set in time and so integrated 2.3 % fewer lines than said: their gain ratios hold, their absolute times do not); the bench and kernlock arithmetic from the line time ("about 400 us" of 128 blank lines, the slit's line-time sums), not the rates, which were counted. The Android-derived IDTs are untouched (their exposures are Android's); a comparison of a ferros capture against them read the ferros exposure 2.3 % long.

**The ultrawide's ladder** (b933, the corrected driver, camtest `04080005010A`): the open read `integration least A#1 and margin A#64; frame A#5081 lines of A#9568 clocks; exposures to A#999 ms`. 64 ms: the frame to 9817 lines, frames 66.4 ms apart, mean 1257 sixteenths; 128 ms: 19571 lines, 103.9 ms (the change inside the 4); 256 ms: 39078 lines, 258.4 ms, mean 1982; 512 ms: the line to 11408 clocks mid-stream (Sony's), 65508 lines, 512.5 ms apart, mean 2910; 1024 ms refused (past its 0.999 s); back at 64 ms, 62.5 ms. The mean doubles each step (233, 474, 958, 1886 above black), the sensor reads back what was written, and the spacings match its line time (from 24.576 MHz already) to the 0.1 %.

**The default route, the owner's steer (2026-11-17 night): the readout stays the mode's.** A longer line is a longer readout and more rolling-shutter skew, so it is never the silent default. Past the mode's frame: the frame length alone in vertical blanking (to about 0.206 s on the main), then the vendor shift with the line at the mode's 6320 clocks (the readout 3072 rows of 3.15 us, 9.67 ms, at every exposure). The line stretch stays in the code only behind `Timing::stretch`, off for every camera; the list now carries `readout` (ns) beside `exposure_max`, the readout at the longest exposure it offers. So the ultrawide, with no shift, offers 0.430 s (its 16-bit frame at its own line); the main offers its shift's most.
- **Shifts past 6, on the Pro (b933)**, the driver spawned with a test override of the most shift (second input byte 8), camtest `02080005000F0D` (8 s to 32 s, two frames a step): 16 s at shift 7 and 32 s at shift 8 both ran at the mode's line, frames 16.43 s and 32.77 s apart, the sensor reading back 40650 / 6320 / 40648, the mean 15617 and 16175 sixteenths (saturating). So `ARRAYS[0].lte.most` is 8: the main offers 52.8 s until the owner picks a cap. The 8 s step, straight from the init's 67 ms frames, still gave its first four frames about 129 ms apart at the old exposure: the change took more frames to land than the two the ladder lets pass.
- **The readout, measured directly: not yet.** The front end cannot time it: its line fires once a frame at DMA_DONE with SOF and EOF already latched (image status G#13), and again for SHADOW_UPDATE (G#8) about 150 us later (the driver logs the first 2^5 answers at its close). So the driver watches rows in memory instead (`row_timing`, the hand spawn's third input byte 1, a test only): the next frame's buffer gets the pattern in its first row, its last row's probe is patterned already, and the frame thread times the first row's landing against the last's. Its first run (camtest `02080005000E0A`) stopped the stream after two frames: the watch began at DMA_DONE and slept thru the SHADOW_UPDATE answer, and the next frame began and never ended (image status G#1). It now begins after the SHADOW_UPDATE answer. Not rerun: the coordinator's rule (stop at the first open whose frames stop). The module powered down at the close as always.
- **Its rerun stopped the stream too** (the coordinator's go, one rerun; camtest `02080005000E0A`, the driver's input `000001`): the first frame at 1 s landed, its SHADOW_UPDATE was answered, the watch ran, the second frame's DMA_DONE was answered (G#13 at 1035 ms), and then no SHADOW_UPDATE came: the next frame began (image status G#1) and never ended, as at b932's 2 s line and the first watch. The ladder without the watch ran every step on the same driver, so the watch itself upsets the front end: a pattern word written into a buffer already with the engine, or a core spinning on the non-cacheable region while the PDMA writes it (the kernlock deaths had the same signature under heavy reading). Stopped there. The direct readout stays unmeasured; what stands for it is the sensor reading back its 6320-clock line at every shift (CAPS), the frame spacings matching that line times the shifted frame, and CCS's readout of rows times line. A measure that does not touch the region while the engine writes it (the optical one: a flickering light's bands, whose spacing in rows is the line time, at a short and a long exposure) is the next to try; `row_timing` stays in the driver, off, for whoever changes it.
- **The shift's whole reach (the Pro, 2026-11-17 night).** The long-exposure sweep (camtest's fourth byte 6, SET 3 forcing each shift, frame 4 lines and integration 2 at the 6320-clock line) ran shifts 21, 22 and 23: frames 26.4 s, 52.8 s and 105.66 s apart, the sensor reading back what was written. Then the field's width, without waiting out a long frame:
  - Written before stream on, inside the hold, both words read back exactly as written: G#1800, G#1F00, G#2000, G#3F00, G#4000, G#7F00, G#FF00, even G#FFFF (the driver's third input byte 2, `shift_width`). The registers are plain 16-bit words, so a read-back bounds nothing.
  - Shift 32 forced mid-stream (camtest `01080006002020`: 16 lines of 2^32, a frame every 2.4 x 10^5 s, 2.8 days, if the field held it): frames came 10.3 ms apart, the sensor's own floor, the mean black (1015 sixteenths). So bit 5 of the high byte is ignored: 32 reads as 0.
  - Bit 4 counts (16 to 23 ran), bit 5 does not: **the field is 5 bits, shifts 0 to 31.** `ARRAYS[0].lte.most` is 31 and nothing else caps it (the owner: no artificial cap anywhere): the main offers 65533 lines of 2^31 at 3.1489 us, **443150242 s, about 14.0 years** (`exposure_max` A#443150242145049247 ns); the open says "exposures to A#443150242145 ms".
  - The arithmetic moved to 128 bits where it passed 64 (65535 lines of 2^31 is 4.4 x 10^20 ps): ccs `Timing::ns`, `most_ns`, `plan`, `plan_shift`, and the frame period is now ns (`frame_ns`; the driver's `PERIOD_NS`, its waits' deadline `late_tick`, past about 8.7 days of us the product with the counter's 24.576 MHz). Host tests: 105 s at shift 9, the most at 31, its frame 443157004 s.
  - Lumis's calibration's dark seed takes `exposure_max` as its shutter (std/lumis main.rs, `calibration` Some(true)): with the whole reach that asks a 14-year frame. Lumis's to decide; not changed here.
  - **Corrected 2026-11-18** ("The shift's ceiling" below): the field is 5 bits, but the sensor counts the frame in 32 bits of lines (frame_length_lines << shift, modulo 2^32). Shift 32's floor here was that wrap (18 << 32), and the 14-year offer was a frame the sensor runs wrapped; the main now offers 65533 lines of 2^16, 13524 s binned and 28286 s at 1:1.
To test with Lumis, the driver is spawned by hand with the input `01`: it then stays between calls (a hand-spawned driver ended after a LIST, and the OPEN behind it started the vault's).

## The slit for real (2026-11-17): built, device runs pending

The owner, via the Lumis session: slit-scan a racecar (Android Lumis tops out near 31 lines a second); phase detect off at the sensor for slits; the slit ring drained so a run lasts as long as wanted; where the 825 us floor comes from; then "stupid fast", a million lines a second, planned around the link.

**The side channels, found by diffing the inits** (tools/oracle-i2c `--table --flat`: every write at its resolved address, the page G#FCFC selects or the indirect window; the binned init `cam.frame` against the 1:1 init in the 16 s capture, keys/husky/captures, and the tele's, front's and ultrawide's inits):
- **Embedded data is G#0118**: an enable byte and the line count. The main and the tele write G#0104 and their links carry 4 embedded lines (the main's link +G#64 G#40FF0); the front writes G#0102 and its link carries 2 (G#20F00). Off is G#0000.
- **Phase detect is the main's G#0720 block**: G#0720 = 1 beside G#0728/G#072A = 2040 x 768 and G#072C/G#072E = 508 x 768, the 768 the link's channel 1 height (G#3000FF0, the PD rows a quarter of the frame's). The binned and the 1:1 inits write it the same, so the 1:1 mode keeps PD too; off is G#0720 = 0 (from the block's shape and its sole enable-looking word, not yet measured). The tele has no G#0720 block but writes G#0D00/G#0D02 = G#0101, likely its own PD control; not wired.
- Why it matters: the PD block says 768 rows whatever the window, so a slit's window and the sensor's PD layout disagree, the leading suspect for the slits' ERR_CRC_PH (one frame an open, 2026-11-16).
- **Built**: ferros_cam ccs `Side` (per camera), `Mode::quiet` (both 0 in the mode's own hold, after its registers), `Mode::cols` (an x window across the middle, on the period, 2^4 columns); OPEN's mode block takes `cols` (u16 at +14), a windowed mode (a slit, or `cols`) is quiet unless flag G#80 (`F_LOUD`); the console's mode line says which. Every close says the link's words (+G#14 errors, +G#24 frame starts per channel, the counters), so a run shows channels 1 and 2 gone and the CRC error with them, or not.

**The ring drained** (the slit's 54 s end, and a bug in every long stream):
- The engine's count register and its index are **bytes** of the descriptor ring, not descriptors, and the engine's own place wraps at the count. Android's HAL shows it (the ultrawide's oracle capture, uw-pdma-raw, 2026-10-22): count G#3E8, the index stepping G#10 a frame and going G#3E0, then G#8, G#18, ...: `(index + 16) mod count`.
- ferros read the count as descriptors and never wrapped the index: the engine went round its count/16 slots unreleased, past the index (which never again equalled its place), reading slots ferros had stopped writing. **Measured on the Pro, 2026-11-17** (Lumis, the preview-split run, count 2^14 so 1024 slots): held buffers written over 0 in the first 938 frames, 21 by frame 1091, 386 by 2194, with unseen and crowded turns climbing from the 1024th frame. And the 2026-11-14 stop "after the 1025th frame" was the index written as 0 when the engine's place was 0 (count 1024: 64 slots), the same model.
- **Built**: the index goes round the ring as the HAL's does, `(index + 16) & (count - 1)`, the descriptors are count/16 (whole frames 1024, slits 4096), a slit ring must divide them (the open refuses otherwise), and STATS word 8 and SLITS word 7 now count the wraps. So the stream runs as long as it is answered, and every frame lands where `ring_turn` put it in any round. A hand-spawned driver's fourth input byte asks a small count (2^5 to 2^16) to see wraps quickly.
- **The drain, for Lumis (its integrator)**: nothing new to learn. OPEN with a slit mode and the view (`[1][0][1]` and the mode block); then loop `SLITS(after)`: it blocks until a slit newer than `after` lands and answers the newest count n, the ring's buffers m, a buffer's bytes, rows, line bytes, width, late, wraps. Slit k (after < k <= n, and k > n + 2 - m) is in buffer (k - 1) mod m of the view, its record (count, counter, Eagle ns, landed) right after its rows; copy the rows, then check the record still reads k (else the engine wrote it over while read: lost). Set after = n and call again. The consumer has m - 2 slits of slack (the two oldest buffers are the engine's next and the probe patterned for the one after): 2^8 buffers at 1212 a second is 210 ms, 2^12 is 3.4 s; past it slits are lost, never the stream. cambench's slit loop is this drain (its +18 byte runs it long: `<< 6` is 3.6 minutes at 1212 a second).

**Phase detect as data** (owner, via the Lumis session: split-image and microprism looks, lens rendering, a focus map; PD off at the sensor only for slits):
- Every ring buffer carries its frame's PD rows: with loch0's PD path on (the init's mode, or `F_PD`) a buffer is 20 MiB, the image's 3072 lines then the PD path's 768 at the same G#1400 line, and each program copy names its buffer's PD place where the program writes G#1C32_21C0 (ferros_cam job `pd_words`: words 10 and 12 of the main's program, once each). They lie in the image slot the view maps, inside the fenced run; MAP's header gives the PD rows' place in the buffer at +56 (u32, 0 none) and their rows at +60 (u16). The job's own PD slot (slot 0) is no longer written: the open's output report says so.
- SNAP `[12]` holds the next frame's buffer as MAP holds it and answers its header with both physical addresses (+64 image, +72 PD, +80 rows, +84 width, +88 line, +92 status: 1 the PD rows still the open's pattern). camtest's fourth byte 8 takes pairs (`1000000800` one at about a metre; `1000000800FF06` a rack, G#1000 to G#7000, each held about 64 s) and prints a `PAIR` line per pair; `tools/camera-pd-pair.sh <prefix> "<that line>"` pulls both by `ferros-bridge read` and packs them (tools/camera-pd-pair) into one VSF visual: the sensor plane and one plane of role `phase` (vsf 0.11.0 at 1255a07, path dependency, not yet on crates.io), raw counts in the sensor's order. Pairs stay local, in keys/husky/captures.

**The PD layout, measured (2026-11-20, host only, no device; branch pd-layout, `tools/camera-pd-layout <prefix>`)** on the three pairs in keys/husky/captures: s1-pair-0 and s6-g3800-0 (the dark room, focus G#3800, minutes apart) and s8-rack-0 (morning light, focus G#1000, everything far out of focus). The tool prints every number below and writes the visuals.
- **A PD line is 4080 RAW10 samples, 2040 pairs interleaved**: 5100 bytes of the G#1400 stride carry samples, the last 20 are zero in every line, no line is all zero. Samples 2k and 2k+1 are the two halves of one place. It is not two halves of 2040 side by side: across 16 bands the line's brightness follows the image's over the whole width, for the even and the odd samples alike. That is the G#0728 block's 2040 (pairs) at two samples each, and the link's channel 1 at 4080 x 768 (G#3000FF0).
- **The G#072C block's 508 x 768 is not in the rows**: nothing follows sample 4079. It is a second window the sensor does not send with this init, or one loch0's PD path drops; not known (508 x 8 = 4064 = 4080 - 16 suggests an 8-column grid, a guess).
- **The two pupil halves are the even and the odd samples**: each alone correlates with the image (0.86 and 0.84 on s1), and they are the scene shifted against each other along the line.
- **Left/right only, no top/bottom pair (not a quad)**: even rows against odd rows, and row pairs against row pairs, differ by under 2% (s8) with no trend down the frame, where the halves' ratio moves 10% across it. The vertical part of the halves' disparity is noise: block matching gives |dy| a spread of 0.28 PD rows and no slope against dx (0.003 on s8, 0.09 on s1); fitting the difference to the sum's gradients gives |dy| a median of 0.11 to 0.18 against |dx| 1.95 to 4.1. So the disparity is one signed axis, not a complex one.
- **Each pair's place in the image**: in the image's 2 x 2 Bayer cells (2040 x 1536), pair k of PD row r sits at cell x = 1.0000 k (offset within 0.04) and cell y = 2.0000 r + 0.5 (0.46 to 0.56 over the three pairs), fit over 46 to 48 blocks with residuals of 0.16 to 0.38 cells rms; every other scale tried (0.5 and 2 across, 0.5, 1 and 4 down) correlates under 0.03. So a pair covers binned columns 2k and 2k+1 and binned rows 4r to 4r+3 (4 x 8 full-resolution pixels), over the full field both ways.
- **So the PD is not full resolution**: one pair per Bayer cell across (2040, half the binned width) and one row per four binned rows down (768, a quarter). The line has the binned width's 4080 samples only because each place sends two.
- The halves' sum lines up with the image; each half sits about half the disparity to either side (on s1, the even half +0.53 cells right of the image, the odd 0.56 left).
- **Brightness**: the halves follow green (the sum's correlation with the cell's greens 0.85, red 0.81, blue 0.74 on s1). A half carries 0.93 of a green sample's light over black (the halves' mean is 0.932 to 0.938 of the image's green over the three pairs).
- **Blacks**: the even half's is A#64.0 to A#64.4, the odd half's A#62.4 to A#62.6, the same in the dark and the bright pairs, from each half's line against the image's green over 192 blocks.
- **Noise**: a half's sigma is 2.8 counts at 54 over black where the image's green is 4.35 at 59 (s8), and 1.37 against 2.10 in the dark. About two-thirds of a green sample's at the same light: a half averages about two green samples' worth. The halves' difference has sigma 4.26 (s8) and 2.35 (s1), independent halves.
- **Shading of the halves** (even over odd, each less its black, s8): 0.90 to 1.02 across, 0.94 to 0.99 down, and not monotonic either way, so a gain map rather than a slope.
- **Disparity** (the odd half's offset that matches the even, in PD steps, one step a cell = 2 binned columns; 16 x 12 blocks of 120 x 56 pairs, the textured ones): on s1 and s6, -0.6 to -3.5, smooth from block to block and the same in both pairs (one column of blocks going down the sensor's rows: -1.4, -1.2, -0.9, -0.7, -0.8, -1.6, -1.7, -2.3, -2.4), so it follows the scene's depth. On s8, -0.4 to -6.5, one block +0.3. Negative means the even half's content lies toward higher k than the odd half's. Both scenes sit on one side of focus; which half is the left pupil, and which sign is near, needs a scene with a known near and far (the rack's other pairs).
- **Visuals** (local, beside each pair: s1-pair-0 and s8-rack-0 `.pd-a.vsf`, `.pd-b.vsf`, `.pd-sum.vsf`, `.pd-phase.vsf`): display planes on the image's cell grid (2040 x 1536, each PD row over two cell rows by the fit), VSF RGB, gamma2tanh at 16 bits, orientation 6 as the sensor plane's, about 18 MB each. All four share one scale, the brightest half sample (less its black) white. The phase rendering is the owner's: brightness the brighter half, colour (A - gB) / (A + gB) on one blue-yellow axis with luminance kept (VSF YCbCr Y unchanged), where g is the halves' shading over a 129 x 65 box, through tanh at the median |t| of the most-textured eighth (0.062 on s8, 0.103 on s1); yellow where the even half is brighter.

**What a Lumis phase view needs** (from the above; not built):
- The PD rows from MAP (+56 their place in the buffer, +60 their rows, 768) at the image's line, unpacked as RAW10, 4080 a line: the even samples are one half and the odd the other, each 2040 x 768.
- Each half less its own black (A#64 and A#62.5, measured here, no register known), and the halves' shading divided out: a smooth gain map from the frame itself (as the tool does), or better a flat-field capture taken once.
- The place: pair k of row r at cell (k, 2r + 0.5), so the halves go x 2 across and x 4 down onto the binned image (x 1 and x 2 onto its cell grid).
- The axis: the disparity runs along the sensor's lines, which orientation 6 turns vertical in the upright picture; a split-image line or microprism look works across that axis.
- The rendering: brightness the brighter half, colour one signed axis (no hue wheel: there is no second axis), the scale about 0.06 in good light and wider in the dark, where the difference's noise grows.
- For a focus map: block matching of the halves (precision about 0.1 to 0.3 PD steps for blocks of 120 x 56 pairs), signed by the rack (near against far) once its pairs are taken.
- camera-pd-pair still writes the PD rows as one `phase` plane in the sensor's order; vsf's `phase` role wants each half a plane of its own, so it can now split them (even and odd) when it is next touched.

**Device plan** (the coordinator's go; hand-spawned driver and camtest or cambench, nothing stored, no reload; every close powers the module down, so a stall is a driver restart; stop at the first open whose frames stop unless said):
0. The ring's wrap: driver input `01000008` (stay, then the fourth byte: a 2^8 count, 16 slots), camtest `4001`: 64 frames thru the view, 4 wraps; expect every frame in the buffer released at the frame before, held buffers written over 0, wraps 4. Then the normal count past its first round: driver input `01`, camtest `400120` (2048 frames, two wraps of 1024; the multiplier is the third byte).
1. The PD rows in the ring: camtest `4001` (normal driver input `01`): MAP's header +56 G#F00000, +60 768; the output report shows slot 0 unwritten. Then one SNAP pair, `1000000800`, pulled.
2. The slit, quiet: cambench `00034000000000000000000002080000` (64 rows, PD and embedded off): the close's link line should show frame starts on channel 0 only and +G#14 without G#20; frames flowing at the sensor's 1169 a second. Then the rows sweep (`...02080100`), then 4 rows. If frames still stop: `F_LOUD` (flag G#80) beside it says whether the side channels changed anything.
3. The drain: the best slit with the +18 byte 6 (261120 slits): wraps about 64, lost 0.
4. The floor: the frame sweep at 4 rows (`00030400...02080200`): RATE's sensor rate against the front end's at each frame length, with PD and embedded off (the 825 us of 2026-10-21 was with them on).
5. The line, toward a million: binned with a 1280-column window and the line sweep, cambench `000100000000000000000000020403000005`: RATE gives the sensor's real line at each asked, the front end's beside it. RAW8 (G#0808, offered by the sensor) would cut the link's bytes by a fifth more; not built (the link's data type and the front end's format word for it are unknown).
6. The scenes (the owner): near and far both out of focus, one pair at G#3800; then the focus rack.

### On the Pro, 2026-11-17 night (b933 flashed, the coordinator's go; the driver hand-spawned, camtest and cambench, nothing stored, no reload)

Logs: the session's scratchpad (runs-slit2). Every open powered the module up from off; every stall was a driver restart.

**The ring's wrap: fixed, measured.**
- Count 2^8 (16 descriptors; driver input `01000208`), camtest `4001`: 64 frames, 4 times round, every frame in the buffer released at the frame before, held buffers written over 0.
- The normal count (2^14, 1024 descriptors), camtest `400120`: 2048 frames, 0 skipped, 67.2 ms apart (the longest 67.5), held buffers written over 0, unseen 0, crowded 0; 4097 answers at 136 us, the index written 11 us into the answer. The 2026-11-17 corruption past the 1024th frame is gone.

**PD rows in the ring: as built.** MAP's header +56 G#F00000, +60 768 at every frame; SNAP's status 0 (the PD rows written). A pair at G#3800 pulled in about 10 s (two 64 KiB-read pulls, 15 MiB and 3.75 MiB), packed into a 19586051-byte VSF: keys/husky/captures/s1-pair-0.*. The room was dark but not black: the image's mean sample about 70 of 1023 at the init's 67.2 ms and unity gain, the PD rows near black.

**The slit: PD off at the sensor removes the CRC error, and the front end then stops on its own geometry.**
- The link, quiet (64 rows, PD and embedded off): +14 G#0 (loud, `F_LOUD`, the same open: G#20, ERR_CRC_PH, and +1C G#1), frame starts and ends on channels 0 and 2 (+24 = +2C = G#5): the PD channel is gone and the packets are good. **Goal 0's suspect is confirmed for the CRC.**
- Embedded data is not G#0118: written 0 it leaves channel 2 on, in slits and in whole frames (the side test below: +24 G#7 with it off). Channel 2's real switch is still unknown; it does no harm here.
- But the front end takes one or two slit frames and stops: every later frame begins (image status G#1, about every 812 us at 64 rows) and never ends. A new close line (`ack_say`: each acknowledged word's bits since the open and in the last answer, and the PDMA engine's first 16 words) names it: loch0's image error word G#1C32_0438 holds **G#2000, SHORT_VB_FATAL_ERROR** (bit 13, isp_fe.h `ISP_FE_CSIS_CORE_IMG_ERR_LOCH0_*`; with 256 or 512 rows also bit 1 SHORT_VB_ERROR and bit 3 EBUF_OVERFLOW). The PD path's error word stays 0.
- It is not the blanking: at 64 rows the frame of 192, 576 and 21362 lines (the last 67 ms) all die after two frames, the 21362 one with each program's SHADOW_UPDATE 0.1 to 0.7 ms after its DMA_DONE and 66 ms to spare. Not the rows: 64, 256 and 512 the same. Not the program: geometry everywhere (`F_ALL`) the same, slim level 2 the same, slim level 1 stops differently (no error bits, the image path at G#1, answered on the tick).
- It is the window, not PD off: a whole binned frame with only PD off at the sensor (the driver's fifth input byte, `SIDE_TEST` 1: `0100000001`, fll 3200) runs clean, 97 a second, no error bits; and an x window of 1280 columns at full height (cambench kind 1, cols 1280) dies the same way (G#200A, one frame) at every line length tried (6320, 4096, 3072, 2560, 2048, 1536 clocks). Whatever the front end keeps of the frame's shape beyond loch0's size (G#1C32_0474, rewritten) and the link's channel words still says 4080 x 3072. Candidates, the CPU 'after' writes beside the size that no rewrite touches: G#1C32_0478 = G#014C1E38, G#1C32_047C = G#180 and G#1C32_048C = G#01000200 (and their PD twins at G#1C32_2078 to G#1C32_208C). The other road is Android's own window: a capture of the camera app at 2x on the main (the recipe under "The 1:1 captures", the app at 2x).
- 4-row slits keep ERR_CRC_PH even quiet (+14 G#20, no frame): 4 rows is under what this sensor sends cleanly.

**The 825 us floor is the sensor's.** RATE (its own frame counter) at 4 rows, the frame from the mode's own down: 16 a second at 21362 lines, 77 at 4100, 306 at 1028 (each the frame asked, at 3.149 us a line), then **1199 a second (834 us) at 260 lines and at every shorter frame** (132, 68, 36). At 64 rows: 1218 a second (821 us) at a 192-line frame, 547 (exactly 576 lines) at 576. So the sensor will not run a frame shorter than about 262 lines of 3.149 us, whatever the rows: the October 825 us. With 256 and 512 rows at 128 lines of blanking the counter read 1172 and 698 a second, faster than the frame asked (826 and 496): not understood, seen once each.

**The PD pairs and the rack: one more pair, then a stop.** The rack's example input was a byte off (`100000080000FF06` put G#FF in the hold's byte: one pair at G#3800 held for 2^10 s); that pair is kept (keys/husky/captures/s6-g3800-0.*), and camtest and the driver were killed mid-stream to end it, so the driver's close never ran. The rerun with the right input (`1000000800FF06`) then got no frame at all: stream on, SHADOW_UPDATE, the link taking whole frames (+24 = +2C = G#7), and the front end never starting one (image status 0 for 2 s). The same driver had run whole frames clean minutes before, so the likeliest cause is the front end left unclosed by the kill (its power domain is not cycled by the switch). Stopped there (the rule); the next boot clears it. Lesson: end a held pair with camtest's own close, never a kill of the driver. The rack is still to take.

**After the reflash (2026-11-18 morning, forge 0cada073, the camera stored fresh, the owner attested; the driver hand-spawned).** camtest `4001` ran clean: 64 frames 66.6 ms apart, 0 skipped, PD rows at +G#F00000, the link clean, the image mean about 120 of 1023 (morning light), so the front end left unclosed the night before was cleared by the boot. The rack (`1000000800FF06`): pair 0 at G#1000 taken and pulled (keys/husky/captures/s8-rack-0.*, status 0); then, 40 s into its hold (frame 600, the host's pull long done, camtest calling STATS every 2 s), the stream died: a frame begun and never ended (image status G#1, the engine's index the driver's own, G#2590), the line quiet 131 ms and answered, no frame after, loch0's error word having shown bit 4 (LATE_CONFIG_ERROR) once. That is the kernlock death's signature (an attested session, the person's processes up), not a slit or PD effect; the driver closed itself and powered the module down. Stopped there (the rule): pairs 1 to 6 of the rack are still to take.

**Not run, blocked by the front end**: the drain (a slit run of minutes) and the line sweep's rates (step 5 stopped at the first frame at every line; the batch ran its six lines though the first had stopped, against the stop rule; each was a driver restart). What the arithmetic already says about "a million lines a second": full-width binned frames run today at 97 a second, 298k lines a second, the link at about 1.5 GB/s; the sensor's least line at full width (about 5370 clocks, 2.7 us) caps it near 370k; a million needs a 1 us line (about 2007 clocks), only possible with a narrow window (1280 columns of RAW10 at 1 us is 1.6 GB/s, the link's ceiling), which is exactly what the front end refuses until its window is understood.

### The slit's second blocker, read from the deaths (2026-11-18, no device)

The coordinator's lead was the front end's frame-shape words that geom's rewrite never touches: G#1C32_0478, G#1C32_047C and G#1C32_048C (with their PD twins at G#1C32_2078 to G#1C32_208C). Read across the three captured opens (target/cam: the main's cam.frame, the ultrawide's uw.cpr2, the front's front.cpr2; the CPU's 'after' writes), they are not the frame's shape:

| | image size (+G#474) | +G#478 | +G#47C | +G#48C | PD size (+G#2074) | PD +G#2078 / 207C / 208C |
|---|---|---|---|---|---|---|
| main (4080 x 3072, link 1) | G#0C000FF0 | G#014C1E38 | G#180 | G#01000200 | G#03000FF0 | the same / the same / G#01001200 |
| ultrawide (4000 x 3000, link 2) | G#0BB80FA0 | G#00BE1078 | G#818 | G#02000200 | G#02EE0FA0 | the same / the same / G#02001200 |
| front (3840 x 2736, link 0) | G#0AB00F00 | G#000DC710 | G#1B80 | G#00000200 | G#02AC0F00 | the same / the same / G#00001200 |

- +G#48C is the input's select: its high byte the CSI link (main 1, ultrawide 2, front 0), bit 12 the link's channel (0 the image, 1 the PD rows; docs/oracle/CAMERA.md had bit 12), G#200 in all. A window keeps both.
- +G#478 and +G#47C are written the same for the image path and the PD path, whose sizes differ by four times in height: so neither is a size. Neither scales with any camera's width, height, stride or frame (no ratio holds across the three); they read as per-camera timing or format words (G#478 was the RAW12 note's candidate format word). Nothing says a window should change them, and nothing was found that names them (the LWIS tables name the interrupt words only).
- What the deaths do say, from the close's `ack_say` lines (2026-11-17 night), against the same lines for whole frames:
  - the bayer front-end context 3, FC ctx3 (G#1C33_7300; isp_fe.h `ISP_FE_FC_BAYER_CTX3_*`): slit and x window G#84, **FC_ERROR** (bit 7) beside REG_UPDATED; whole frames G#4, REG_UPDATED alone;
  - the LMP's bayer context 3 (G#1C35_00E8; `ISP_FE_LMP_BAYER_CTX3_*`): slit G#9, **FE_LMP_ERROR** (bit 3) and no EOF; whole frames G#13, EOF and CDAF_PRIMARY;
  - loch0 then SHORT_VB_FATAL, at any blanking.
- So loch0's frame also feeds the bayer statistics pipeline (FC ctx3 and the LMP), which the program sets up for 4080 x 3072 (FC ctx3's size G#1C33_7904 = G#0C000FF0, its windows from G#1C33_7500, the LMP's 1280 words at G#1C35_4000 and G#1C35_5000); a window makes it err, and loch0 stalls behind it. `Scope::All` rewrote the sizes it recognises and still died: the statistics grids are laid out for the whole frame and a 64-row frame is no frame they take.
- **The fix to try: take loch0's frame to memory alone**, the statistics pipeline switched off as Linux's close switches it off (kernel camera_close.rs): ferros_cam geom `DETACH` and `detach`, the hand spawn's eighth byte (`DETACH`): bit 0 FC ctx3's enable G#1C33_7028 to 0 (1 in the program), bit 1 the context at G#1C33_B028 to 0, bit 2 the crossbar word G#1C33_0C08 to 2 (G#7C at the open; G#1C33_0D08 is 2 already), bit 3 G#1C34_0000 to 0 (the block sized for the PD rows). Host-tested against the local job: each bit changes its own words only.
- The slit ring and the kernel's release: since camera-full-rate the kernel releases the next descriptor in the interrupt, before the driver runs, so the next frame's buffer may be under the engine when `slit_turn` runs. Its probe is now patterned two frames ahead ((n + 1) mod m at frame n), and a reader keeps to the newest m - 2 (cambench's drain and SLITS' doc changed with it).

**The run plan** (the coordinator's go; the kernel with the interrupt release RAM-reloaded by the full-rate agent, the Pro handed over with no reboot; the driver hand-spawned, cambench and camtest; nothing stored, no reload, no reboot; the scripts end a client and the driver only after the driver's own close line, never a streaming camera; stop at the first open whose frames stop unless said):
0. Regression: driver `01`, camtest `4001`: 64 frames, the close's "released by the kernel's interrupt" near 64.
1. Detach on whole frames, the control: driver `0100000000000001` (FC ctx3 off), cambench `00010000800C00000000000000040000` (fll 3200, 64 frames): loch0 should run at 97 a second with FC ctx3's word 0. If frames stop, detaching breaks loch0 itself: then `...02`, `...04`, `...08` each alone to find the switch that does.
2. The slit, detached: the same driver input, cambench `00034000000000000000000002080000` (64 rows, PD and embedded off); then driver `010000000000000F` (every switch). Expect frames past the second, SLITS at the sensor's 1218 a second, loch0's error word 0.
3. If it streams: the rows sweep (`00030000000000000000000002080100`, 64 down to 2, may stall by design), the frame sweep at the narrowest that streamed (`...02080200`): the front end's rate against the sensor's at each step.
4. The drain: the best slit with the run's +18 byte 6, `00034000000000000000000002FF0000000006` (261120 slits, 3.6 minutes at 1218 a second, 2^8 buffers): lost 0, read over 0, the engine ring round about 64 times.
5. The line: the 1280-column window, detached, with the line sweep, `000100000000000000000000020403000005`; then 640 columns (`...0280`): RATE gives the sensor's line at each step, the front end's beside it; the link at 1280 columns of RAW10 and a 1 us line is about 1.6 GB/s, the ceiling planned around.
6. If any detached slit streams, a 4-row slit (`00030400...02080000`) once: 4 rows kept ERR_CRC_PH even quiet, so expected not to.

### The slit runs (the Pro, 2026-11-18, camera-full-rate 82deb020's kernel RAM-reloaded, armed actions on; this branch's driver at f4263c9a hand-spawned, cambench, nothing stored)

Logs: the session's scratchpad (runs-slit2/r*). Every DMA_DONE of every run was released by the kernel's armed action, before the frame thread (the close's count equal to the DMA_DONEs).

**The slit streams.** Two fixes together:
- **Detached** (`DETACH` bit 0, FC ctx3's enable G#1C33_7028 to 0): SHORT_VB_FATAL is gone. On whole frames the same switch is harmless (fll 3200: 98 a second, 64 frames, FC ctx3's and the LMP's words 0: the statistics pipeline idle).
- **The slim program** (flag G#10, loch0's paths alone each frame): detached alone, a 64-row slit ran 1270 frames (about 1 s at 1230 a second) and died on LATE_CONFIG (G#10): the full program (6620 bytes, 1447 writes, 3048 LUT words) does not fit a slit's 403 us blanking every time. Slim, or a longer frame (320 lines, 1004 a second), both ran clean.
- So the slit recipe is detached, slim and quiet (PD and embedded off), and since this commit it is the default for every windowed mode on the main (a slit, or an x window; ferros_cam geom `shape`, host-tested): whole frames and the other cameras are unchanged. The driver's eighth hand-spawn byte overrides it for an A/B (G#80 with the switches in its low four bits; G#80 alone is neither detached nor slim unless OPEN asks G#10).

**Rows** (each its own driver; detached, slim, fll rows + 128): 64, 56, 48, 32 and 16 rows all stream at the sensor's own floor, 1218 to 1229 a second on its counter, the front end 1208 to 1218 beside it, 0 lost; 64 rows keeps the link clean (+14 G#0), 56 rows and fewer set ERR_CRC_PH (G#20) on the link while frames still land; 8 and 4 rows take no frame (loch0 G#404: SENSOR_RESOL_MISMATCH and STREAM_ERROR, the sensor not sending what it was asked). The slit's floor is the sensor's frame of about 262 lines (825 us), whatever the rows, and the front end keeps up with it.
- A trap found on the way: a hand-spawned driver still ends at a CLOSE, so a cambench sweep's second and later opens went to the vault's stored driver (no detach, no quiet, an older close line). The rows sweep of this morning past its first step, and every sweep step of 2026-11-17 night past its first, ran the stored driver; the sensor's counter numbers stand, the front end's verdicts for those steps do not. The rows above were rerun one driver an open. Fixed since: a driver spawned to stay (input 01) stays after its CLOSE and refuses a second open (answer 4, "spawn another"), so nothing falls thru to the vault's driver; a sweep of a hand-spawned driver is tools/cambench-sweep.sh (a driver and a cambench each step, the field written into the input, nothing ended before the driver's own close line). **Run on the Pro (2026-11-18, 813db641, the armed-write kernel):** camtest `4001` clean (64 frames, every release the kernel's, no detach on a whole frame); the default 64-row slit with no override detached and slim by itself, 1244 a second, 0 lost, the link clean; tools/cambench-sweep.sh rows 64, 32, 16, each step its own hand-spawned driver (detached, armed, one open, one close, none the vault's), 1221 to 1255 a second; and cambench's in-process sweep on a hand-spawned driver: the 64-row step ran, the 32-row OPEN answered G#4 ("spawn another"), the sweep stopped there.

**The drain: done.** 64 rows, detached, slim, u16 add on one thread: **261120 slits in 212 s, 1230 a second, lost to the ring 0, read over 0, the engine's ring round 64 times**, 262391 DMA_DONEs all released by the kernel, the frame thread 82 us an answer (1062 the most). Reading and unpacking a slit 292 us, adding 213 us. A slit run lasts as long as it is drained.

**The line, toward a million** (whole frames, detached, slim, 3328-line frames, one driver an open):
| window | line asked (clocks) | sensor / front end a second | rows a second | link |
|---|---|---|---|---|
| 4080 | 5888 | 102 / 100 (103 mapped) | 340k | 1598 MB/s |
| 4080 | 5376 | no frame, ERR_CRC_PH | | the link's limit |
| 1280 | 5888 | 102 / 100 | 340k | 501 MB/s |
| 1280 | 5376 | 111 / 109 | 369k | 548 MB/s |
| 1280 | 4096 and 2048 | 115 / 113 (117 mapped) | 383k | 566 MB/s |
- **The ceiling is the sensor's row time**: asked 2048 or 4096 clocks it reads them back but runs about 2.61 us a binned row (about 5240 clocks), so this mode tops out near 383k rows a second, at any width. The full width meets the link first (5376 clocks: 1.6 GB/s of RAW10 and the packets fail); a narrow window frees the link but the sensor's row does not get shorter.
- So a million lines a second is not in the binned mode: its row costs 2.6 us however few columns are read.

**A million lines a second, for the owner: which modes could get there** (from the captured inits and the runs; the numbers in one place):
- **Measured today:** the slit floor is **1230 slits a second** (the sensor's least frame, about 262 lines of 3.15 us, 825 us), for slits of **16 to 64 rows** (8 and fewer take no frame); a drain of 261120 slits lost none. The binned mode's ceiling is **383k rows a second** (a 2.61 us row, about 5240 clocks of its 2007 MHz pixel clock, 115 frames a second of 3328 lines, at any width). At the full 4080 width the **link** stops it first: 5888 clocks (102 a second, 1.6 GB/s of RAW10) runs, 5376 fails its packets (ERR_CRC_PH).
- **The binned mode (Android's init, the one ferros streams):** vt PLL G#F5 (245) at 24.576 MHz / 3, 2007 MHz; its own line 6320 clocks (3.15 us), its floor 2.61 us. **383k rows a second, not a million.** Narrowing the window only frees the link.
- **1:1 full resolution (Android's 8160 x 6144, captured with the 16 s exposure, keys/husky/captures/longexp-16s):** vt PLL G#D7 (215), 1761 MHz; line G#2D50 = 11600 clocks = **6.59 us a row, 152k rows a second** (24 frames a second of 6326 lines). Its 8160 RAW10 samples a row are 1.55 GB/s at that line, about the link's rate, so its line is the link's, not obviously the sensor's: a 1:1 row's own floor is unmeasured.
- **A 2x crop (1:1 rows, the middle 4080 columns, as the camera app's 2x):** not captured. If the 1:1 row's floor is the binned one (2.6 us), a 4080-wide 1:1 crop reaches about the same 383k rows a second, and its link (4080 RAW10 a 2.6 us row, 2.0 GB/s) wants RAW8 to fit. Nothing suggests a 1:1 row is faster than a binned one; if anything a binned row does more work in the same ADC time. Not a million either way.
- **RAW8 (G#0808) or 10 to 8 (G#0A08), both offered (CAPS):** a fifth fewer bytes on the link. That moves the full width's link limit (5376 clocks) to below the sensor's floor, so the full 4080 width reaches the same 383k rows a second; it does not shorten the row. Not built: the front end's data type and format word for RAW8 are unknown (a capture of Android in a RAW8 mode, if it has one, or a trial of the link's data type G#2A).
- **Fewer rows a frame (slits, vertical skipping or more binning):** the frame rate rises but each row still costs the row time, and below about 262 lines the sensor holds the frame anyway; rows a second do not rise.
- **So no mode in the captured inits reaches a million rows a second**: the sensor's row time is the ceiling, about 2.6 us (383k) binned and unmeasured but not likely shorter at 1:1. A million wants a 1 us row, 2.6 times the sensor's floor. What remains to look for: a vendor mode with a faster ADC (fewer bits converted; not in the CCS capabilities, the vendor's pages if anywhere), measured with tools/cambench-sweep.sh's line sweep in that mode.
- **For slit-scan, another way to count:** within any frame each row is read one row time after the one above (rolling shutter), so a frame of a moving subject already samples time at 2.6 to 3.15 us a row, 320k to 383k samples a second, each at the next row down. A slit-scan that takes its time axis from the rows of whole frames (one row a time step, the subject crossing the rows) gets that rate today; the 1230-a-second slit is the rate when every time step must be the same rows.
- With the front end detached and slim it is no longer a ceiling anywhere measured: it took every frame and slit the sensor sent.

## Full rate (2026-11-17 night, branch camera-full-rate): built, device runs pending

The owner: "If I wake up to 120fps full spec camera feed and it all works flawless, that would be nice."
The main's binned top is about 96 to 102 a second (the sensor pads itself at about 3230 lines; the front end takes lines of 5888 clocks or longer); the panel's 120 repeats frames.
The stream dies above about 44 a second under load: fll 4096 (77) and 3328 (94) under Lumis in 2 to 3 s, fll 3200 (96) in an attested session in 9 runs of 9 with every answer in time; whole frames ran clean at 96 to 97 with the person's processes down (kernlock #2) and on slit-for-real's driver (PD off, 2026-11-17 night).
Every death reads the same: image status G#1 (a frame begun, never ended), the engine's index the driver's own.
Not one of them recorded loch0's error words: the acknowledgement read and cleared them every answer, unseen, until slit-for-real's `ack_say` and this branch's timeline.

**Built, the driver (measurement only, the answer's order unchanged):** per answer the counter, image status, index written, the answer's time and loch0's image and PD error words (low and high halves), the last 2^8 kept; DMA_DONE to SHADOW_UPDATE lags (a histogram in 2^k us) and DMA_DONE gaps; the first answer whose error words were not 0 and its frame; the image and error overflow words (G#1C32_0428, 043C: an event latched again while pending, a missed interrupt). Said at the first quiet line with the front end waiting and at every close (`fe_say`, then `ack_say`).

**Built, the kernel (two switches, not run; one RAM reload):**
- `diag clock cammem on|off` (on): while the camera's switch is on, the tier stays active (never rest or dark: INT stays 356 MHz, the workers stay up with the screen off) and MIF and the DSU at the lift's 3744 and 1800 MHz, from the switch's on (2 s of sensor init before stream on) to its off, thru heat. Today MIF changes under a running stream at the lift's start (the first answers), at every tier switch the lift does not cover, and whenever any controlled domain is capped (thermal.rs `throttled`, the step policy's 90 C trip on the X3 and A715s drops the lift: MIF 3744 to 2028, DSU 1800 to 1197), then back at the release (80 C). With the screen off the dark tier puts MIF at 421 MHz.
- `diag clock cambts on|off` (off): the switch's on sets the front end's four bus traffic shapers (zuma-bts.dtsi bts_ispfe0 to 3 at G#26C4_8180, G#80 apart) to Linux's boot values (AxQoS 12 read and write overriding the master's, outstanding G#26/G#FFFF x3 reads and G#40 x3/G#8 writes, urgency on at G#20) and says the words before and after in the flash log. ferros never programs them; Linux's BTS driver does at boot, the front end being one of its four real-time masters with the displays.

**The SHORT_VB lead, against the binned cut (from the captures, target/cam cam.frame):** loch0's error interrupt mask is G#3FFF (G#1C32_0444: all 14 errors on, and the PD path's G#1C32_2044 the same), so a SHORT_VB (bit 1) or SHORT_VB_FATAL (bit 13) at a death will be in the timeline. The blanking argues against it for whole frames: fll 4096 leaves 1024 lines (3.2 ms) past the 3072 rows, eight times the 128 lines (403 us) at fll 3200 that ran clean at 97 a second, yet 4096 died under Lumis. The slit and x-window deaths keep the frame's blanking (the 1280-column window at full height dies at every line length), so their SHORT_VB is the front end's idea of the frame's shape, not real blanking.
Where the shape may live: the CPU's 'after' writes to loch0's image path, in order, none touched by `geom::rewrite` but the size: +G#410 = 7, +G#430 = G#12 (later G#1A), +G#444 = G#3FFF, +G#458 = 3, +G#418 = 1, (G#1C32_00B0 = 1), **+G#478 = G#014C_1E38** (halves 332 and 7736), **+G#47C = G#180** (384), +G#460, 464, 46C, 470 = 1, **+G#48C = G#0100_0200** (the PD path's G#0100_1200), +G#488 = 0, +G#474 = G#0C00_0FF0 (the size), +G#42C = G#1B; the PD path the same at +G#2000. Neither half of G#478 nor G#47C is a line count that the 128-line cut would trip (fll 3200 runs clean); none scales with the frame's 4080 x 3072 in an obvious way (7736 is not 2 x 4080 nor a line's bytes, 5100). Unproven: a whole-frame open that writes G#47C or G#478 differently (with the slit agent's x window, where the error is certain) is the short test.

**The morning runs** (the coordinator's go; the flashed kernel first, no reload; driver hand-spawned with input `01`; cambench never killed, every stream ended by its CLOSE or the driver's own close; FERROS_PORT=1-2.2, the debug bridge, stop at the first -71):
0. `diag clock`, `diag thermal`, and `bridge read` of the shapers' defined words one at a time (G#26C4_8180 +0, +8, +G#10, +G#20, +G#24, +G#40, +G#44, and the same at +G#80, +G#100, +G#180): are they Linux's values, reset values, or zero.
1. The reproduction, minutes at 96 with a consumer: cambench `00010000800C00000000000002FF0003000002` (fll 3200, u16 add on three threads, 16320 frames, about 170 s); `diag clock` before and after (the lifts and tier switches counted during it), `diag dev`, `diag lock`, `diag thermal`. Then fll 3328 (`000D`) and 4096 (`0010`) the same.
2. At a death: the close's `fe_say` names the error bits and the frame they first came; the timeline shows whether the last answers were late (a gap of a period or more), the lag whether the engine's program ran long. EBUF_OVERFLOW (bit 3) or WDMA (bit 5) points at the bus (the clock change or the shapers); LATE_CONFIG (bit 4) at the program; SHORT_VB (1, 13) at the shape; none at all, with answers late, at the kernel.
3. The memory clock by hand, at 44 a second (fll 7168, `001C`), mid-stream: `diag dvfs set 0 2028000`, then `3744000`. A death at the change proves the clock suspect at once.
4. Only then the reload (tell the coordinator first): cammem on (the default) and the step 1 runs again; cambts on if step 2 said the bus.
5. Heat: `diag thermal` every 2^5 s thru the longest clean run.

### The cause, measured (the Pro, 2026-11-18, forge 0cada073, the owner attested, 36 C)

- **Reproduced** at 96 a second (fll 3200, cambench u16 add on three threads): every answer on time for 2493 frames, then one DMA_DONE answered 3.3 ms late (13487 us after the SHADOW_UPDATE before it, against 10.2 ms); that answer read loch0's error word G#10 (LATE_CONFIG_ERROR), and the next frame began and never ended. `diag dev`: ispfe0 interrupt to run longest 3311 us, to the wake 3297 us (the `.Lirq` stamp on time, the wake late). `diag lock`: map's set_pref (x1 G#D, after a tap opening settings) held the kernel lock 3322 us at that moment. A second run died at frame 46 the same way, the wake 976 us behind a 1019 us `with_lock` hold.
- **Injected** (the driver's test bytes, no kernel change): an index write 2 ms late at the 256th frame kills the stream exactly so, with a lead of one or two (the engine runs every released program at once: its place equals the index at every DMA_DONE, so a lead buys nothing); the index on time with the 56 acknowledgements 2 ms late runs 1024 frames clean. The release of the next descriptor inside the frame's blanking (403 us at 96 a second, 57 ms at the init's 15) is the whole deadline, and the frame thread's wake goes thru the kernel lock (run_slice, wake_sleepers), which this boot held 36 to 148 ms at times (`with_lock` around spawns, the sensors' and the camera's own power switch 42 to 43 ms, dma 4.8 ms). The 15 a second death (the slit agent's, frame 600, LATE_CONFIG) fits the same.
- **Not the cause**: heat (36 C, no cap), the memory clock (2194 lifts that boot, deaths unrelated to them), the bus shapers (all four ISPFE and the display's read at reset: CON 0, AxQoS 4 not overriding, no urgency; noted, unchanged).

### The fix, two shapes (the owner's choice)

**(a) Built, then replaced by (b)** (camera-full-rate 10944891, never run): `.Lirq_cam` (INTID 511 only, registers saved as `.Lirq_prof` saves) calls camera_power.rs `ferros_cam_fast` before the line is masked and the driver woken, never the lock: on DMA_DONE (G#1C32_0424 bit 4) with the engine's place (+G#14) at its index (+G#10) it writes `(index + 16) & (count - 1)`. The driver names each descriptor a frame ahead and writes the index only while the engine still reads its own, so it runs unchanged on a kernel without the path. Slack: about a frame period (10 ms at 96 a second, 66 ms at 15) instead of the blanking. 121 lines (kernel 103, driver 18).

**(b) A generic armed action, the owner's choice ("Go for B!", 2026-11-18), built** (docs/KERNEL.md "Armed actions"; disarmed at the process's end and its power switch's off, not at a regrant: init holds every grant): a driver arms a small bounded program on its own granted line; the kernel runs it at interrupt time, lock-free; no camera logic in the kernel.
- **The call**: `SYS_IRQ_ARM` (a new number): x0 the line's token, x1 a hardware grant's token, x2 and x3 the program (address, bytes; 0 bytes disarms). ferros_sys `irq_arm(line, hw, &[Op])`.
- **The ops** (16 bytes each: op, offset, a, b; at most 2^2 of them, run in order, the first failed test ends the run):
  - `TEST_BITS off mask`: go on only if `read(off) & mask != 0`;
  - `TEST_EQ off_a off_b`: go on only if `read(off_a) == read(off_b)`;
  - `ADVANCE off step mask`: `write(off, (read(off) + step) & mask)`, mask of the form 2^k - 1;
  - `WRITE off value`.
- **Checked once, at the arm** (under the lock, as SYS_MMIO is): both tokens the caller's (`Irq`, `Hardware`); every offset 4-aligned and inside the grant; the grant not through EL3 (`via_el3`), not a DMA fence (`s2mpu::is_fence`), not a PCIe window (`pcie_guard`: a read never answered at interrupt time would wedge the core); the mask a power of two less one; at most 2^2 ops. Stored resolved (absolute addresses, the op words) in a table by `IRQS` index (`IRQ_GUARDED`, 2^5 lines), published with its count last (release order), so the interrupt does no lookup and no check. Disarmed at the process's end (`end_process`, beside `cam_free`), at the line's next grant (`build_from`), and by its own power switch going off (the camera's `down`: no action may touch a block whose clocks are off).
- **The interrupt**: `.Lirq_live` tests a byte table `__irq_armed[intid]` (one load and a branch, as `__irq_pending` is indexed) and only for an armed line saves the caller-saved registers and calls `ferros_irq_armed(intid)`, then the line's own path as now.
- **Cost at interrupt time**: the camera's program is 2 + 2 + 1 register accesses (status; place and index; the write: 4 reads and 1 write, as (a)'s, the count's read replaced by the armed mask), plus the save and restore (about 70 pairs, a few hundred ns) and a decode of three ops: within a microsecond or two of (a). Unarmed lines pay one load and a branch.
- **The camera arms**, after stream on's first index write: `TEST_BITS G#20424 G#10` (DMA_DONE), `TEST_EQ G#90014 G#90010` (every released program run), `ADVANCE G#90010 16 count - 1`; disarms (or the switch's off does) before its close. Its "write only if the engine still reads ours" stays as in (a).
- **More than (a)**: the syscall, its validator (host-testable as a pure function over the op words and the grant), the table and its three disarm paths, the byte table in `.Lirq`, ferros_sys's wrapper and a KERNEL.md entry: about 250 lines more, half a day with host tests, and one more surface to audit (a process's register write at interrupt time, inside its own grant: what SYS_MMIO_WRITE already lets it do, only earlier). Other drivers with a blanking-sized deadline (the display's DECON trigger, audio's DMA ring) could use it as is.

### On the Pro with (b), 2026-11-18 (camera-full-rate 82deb020 RAM-reloaded over forge 0cada073, not attested; the driver hand-spawned, cambench, nothing stored)

Every open below armed its release (`the descriptor's release armed on the front end's line`); the close counts the DMA_DONEs the kernel released before the frame thread answered. The consumer is cambench reading every frame out of the view and adding it into u16 sums on three A715 threads.

| mode (main, binned 4080 x 3072) | frames | rate (sensor / taken) | released by the armed action | errors | consumer |
|---|---|---|---|---|---|
| init's (camtest `4001`) | 64 | 15 (66.5 ms) | 65 of 65 | none | camtest MAP |
| fll 3200, every 256th answer 2 ms late (injected) | 1124 | 96 | 1124 of 1124 | none (dies at 256 with `camfast off`: LATE_CONFIG) | map only |
| fll 3200 | 16424 (170 s) | 96 / 96 | 16424 of 16424 | none | 6.2 ms of work a frame, 2 skipped |
| fll 3328 | 8260 | 95 / 95 | 8260 of 8260 | none | 0 skipped |
| fll 4096 | 8245 | 77 / 77 | 8245 of 8245 | none | 3 skipped |
| fll 3200, line 5888 clocks | 8271 | 102 / 103 | 8271 of 8271 | none | 1 skipped |
| fll 3200, line 5760 | 16436 (155 s) | 105 / 106 | 16436 of 16436 | none | 6.0 ms a frame, 3 skipped |
| fll 3200, line 5696 | 1138 | 106 / 107 | all | none | 1 skipped |
| fll 3200, line 5632 | 1 | 107 / 0 | | the link's ERR_CRC_PH (+G#14 = G#20): the sensor's line too short for the link | |

- **The top**: about 106 frames a second at full binned size (line 5696 to 5760 clocks), the link's own limit at 5632; 105 a second held 155 s with a consumer reading and adding every frame.
- **Heat** (`diag thermal` every 2^5 s, the room cool, no cap anywhere): at 96 a second the hottest zone 37 C to 53 at 32 s, 55, 57, 58, 58 at 160 s; at 105 a second 46 to 61, 64, 65, 65 C at 128 s, then falling to 48 C a minute after the close. The trips are 90 C (X3, A715s) and 95 (A510s).
- **The one death on this kernel** (fll 3200, frame 4358): the frame thread woken 133 ms late, past a whole frame (the line stays masked until the thread's acknowledgement, so the armed action releases once), behind kernel lock holds of 130, 80, 61, 50 and 43 ms: the AoC firmware's pieces (`aoc.fw.N`) read from the vault under `with_lock` by the service loop's `sensor_step`, each time init's keeper restarted `sensors` (on a RAM-reloaded kernel the AoC never comes online, so every 64 s). With that loop stopped every run was clean. A hold past a frame period still stops the stream: those holds are the lock-holds work (branch lock-holds); the armed action moved the camera's limit from the blanking (0.4 ms at 96 a second) to a frame period (10 ms).

## A cacheable view (2026-11-18, branch cam-cacheable; Lumis on branch lumis-cam-cacheable)

The owner: "if we can actually get 60fps on ferros with the cam I'd be stupidly happy".
The Lumis session measured its intake at 61.4 a second (kind 1, fll 5170): 53.5 ms a frame on the four A510s, 14.7 to 18.8 on the A715s (which starved the driver's frame thread), 42 to 51 on the X3, every row copied out of the non-cacheable view in 64-byte loads, each its own bus transaction.

**Can the front end snoop instead?** Not as ferros runs it, and Linux never asks it to (the reference tree, /mnt/Harbor/ferros-ref/soc-gs):
- On zuma only UFS (`zuma-ufs.dtsi:64`) and the JPEG block (`smfc`, `zuma.dtsi:1679`) are `dma-coherent`; no LWIS node is (`zuma-isp.dtsi`, `lwis_isp_fe` at line 164), and the ISPFE SysMMUs (`zuma-sysmmu.dtsi:475-547`) carry no shareability property.
- The SysMMU v9 has the means: a shareable bit in its page entries (`drivers/iommu/samsung-iommu-v9.c:36` FLPD_SHAREABLE_FLAG, `:40` SLPD_SHAREABLE_FLAG), set only for IOMMU_CACHE mappings (`:606`, `:651`), which the DMA layer asks only for a dma-coherent device, and dropped for IOMMU_PRIV (`:693`); the heaps map uncached buffers non-shareable on purpose (`drivers/dma-buf/heaps/samsung/heap_dma_buf.c:107-112`). PCIe, UFS and the modem's DIT switch theirs in a SYSREG shareability word (`pcie-exynos-rc.h:227-238`); no such word is named for SYSREG_ISPFE (G#1C02_0000).
- ferros runs the three ISPFE SysMMUs in bypass ("The view": frames land above 4 GiB with S2MPU faults 0), so the bit would need them translating, with page tables, on a path no Pixel kernel ever ran coherent. Not tried; cache maintenance instead.

**What is built:**
- The kernel maps the view Normal write-back, inner shareable, read-only for EL0 and EL2, never executable, when the driver asks (SYS_DMA x1 = 4 with x3 bit 63; cachemap `cam_view_map`'s `cached`); a kernel from before refuses the bit (it reads as frames past the run) and the driver asks again without it.
- SYS_DMA x1 = 6 (process.rs `cam_lines`): bytes x2 to x2 + x3 of the driver's run (on `camera`) or of the view (on `camera-view`, by its holder) cleaned and invalidated to the point of coherency, a `dc civac` a line, by the kernel's own alias of the frames (read-write at EL2), with the kernel lock let go (`unlocked`); answered the counter's ticks it took.
- The driver (payloads/camera): an OPEN asks for it in the byte after the whole mode block (`VIEW_FLAGS`, the seventeenth, bit 0), so a holder that asks a driver from before still gets the non-cacheable view; the 64-byte reply carries 1 at 53 when it was given (never for a slit ring, which has no hand-over). Each MAP cleans and invalidates the buffer it hands over (16 MiB, or 20 with the PD rows) before it answers; each release cleans and invalidates the buffer's probe line before reading it. Its ninth hand-spawn byte (1) skips the MAP's maintenance, a test only.
- cambench +20: bit 0 the view write-back, bit 1 the frame threads on the A510s, bit 2 a check (16 lines a frame read again from DRAM after the kernel drops them, and compared).
- Lumis 0.2.18 (std/lumis, branch lumis-cam-cacheable from lumis-damage 810a3fda): asks for the view write-back and unpacks its rows in place (no copy); a second knobs byte after the whole mode block (the input's fifteenth), bit 0 `DEV_UNCACHED`, asks for the non-cacheable view for the comparison.

**Why it is right:** a buffer is handed over only after its frame's DMA_DONE (`ring_turn` makes it the newest only then), and the front end writes it again only after the holder gives it back (`ring_turn` never names a held buffer, nor the newest while another is free).
So once its lines are dropped at MAP, whatever a core kept from the last time round the ring, or fetched by speculation while the front end wrote it, is gone, and any line fetched after (by the holder, or by speculation) is the frame's.
The lines are clean: nothing writes a camera frame thru a write-back alias after the region's take cleaned it (the view is read-only, the kernel's alias untouched until the frames go back), so the clean writes nothing back and the invalidate loses nothing.
Clean and invalidate, not `dc ivac`: the range is found under the lock and maintained with it let go, and a run given back meanwhile may be another process's frames, whose dirty lines a clean writes out where an invalidate would drop them.
The probe line at a release: the driver reads its probe non-cacheable, and the architecture does not promise a non-cacheable read sees DRAM while a core holds the line write-back (mismatched attributes), so a write while held could read as none.

**Where the maintenance belongs, and why there:**
- In the kernel, called by the driver at MAP: correctness stays with the driver and the kernel (a holder that skips nothing can read stale lines), and it is one call a frame.
- Not EL0 `dc civac` thru SCTLR_EL2.UCI: the bit is global, every EL0 process would get `dc civac`, `dc cvac`, `dc cvau` and `ic ivau` on anything it can read (a flush-and-reload timer, help for row hammering), for no gain: the call is a few microseconds against 0.4 to 1.8 ms of lines.
- Not the reader a call per row: cambench's check made one call a 5120-byte line at 680 ns, against about 2.8 ns a line in one call over a buffer (715 us for 16 MiB), so per row would cost about 2 ms of calls a frame. Per band (four calls, one a reading thread) would spread the lines over the readers but leave correctness to the kist; not done.
- Tried and dropped: a driver thread making each new frame's buffer right as it landed, ahead of MAP. 36 to 50 of every 64 were done ahead, MAP still did 9 to 33 itself (the holder asked as the frame landed), the thread's own took 2.5 to 3.7 ms each with Lumis drawing, and the turns were no shorter.

**Measured on the Pro** (cam-cacheable 2db55058 RAM-reloaded over a510-workers 40d28cee, b947, not attested; the main binned, kind 1, fll 5170, exposure 3000 lines: 16.28 ms, 61.4 a second by the driver's clock; the driver hand-spawned with input 01, every stream ended by its own close).

cambench, 128 frames, four threads on the A510s, PD off (16 MiB buffers):

| view | work | a frame | taken | the maintenance at MAP |
|---|---|---|---|---|
| non-cacheable | read every line | 41.7 ms | 23 a second, 199 skipped | none |
| write-back | read every line | 2.8 ms | 61 a second, none skipped | 715 to 778 us on average, 799 at most |
| write-back | read and u16 add | 9.4 ms (reading 1.4 ms a thread) | 61 a second, 1 skipped | 718 to 785 us, 818 at most |

The check: 0 of 2048 sampled lines differed from DRAM with the maintenance; 0 of 2048 without it too (input `010000000000000001`): with four buffers and 48 MB read between a buffer's turns the old lines are gone from every cache by then, so this ring cannot show the hazard, which the architecture still allows (speculative fills, a smaller ring, a slower reader).

Lumis 0.2.18, hand-spawned (`--use display --use touch --dma surface --ira --domain --entropy --use camera`), 32 s (`DEV_OPEN`), inputs `280100003214B80B0000000000` (the intake on the A510s) and `680100003214B80B0000000000` (bit 6: on the A715s), the non-cacheable runs with `0001` after them; every run about 1968 frames written by the driver, no error word, held buffers written over 0, crowded 0:

| intake | view | the integrator a frame (first 64, last 64) | a turn (first, last) | taken (frames integrated of about 1968) | preview: new images drawn a second (second 4 s window, last) and Lumis's draw | the maintenance at MAP |
|---|---|---|---|---|---|---|
| A510s | non-cacheable | 53.3, 53.8 ms | 53.1, 53.9 ms | 18.6 a second (576) | 18.5, 18.5 (13.5 to 15.5 ms) | none |
| A510s | write-back | 13.2, 17.2 ms | 16.2, 19.4 ms | 61 a second for the first 1300 frames, 48 to 52 at the end (1856) | 56, 26 (13.7 to 27 ms) | 1.1 to 1.7 ms on average, 7.4 at most |
| A715s | non-cacheable | 7.5, 17.4 ms | 16.0, 18.8 ms | 61 a second, 50 to 53 at the end (1792) | 43, 21 (17 to 35 ms) | none |
| A715s | write-back (three runs, one with `diag clock cammem on`) | 4.3 to 4.8, 10.4 to 11.2 ms | 16.0, 16.5 to 17.8 ms | 61 a second, 57 to 61 at the end (1920 each) | 45 to 51, 21 to 25 (14.6 to 33.5 ms) | 420 us for the first 200 frames, then 0.7 to 1.8 ms, 8.5 at most |

- **The intake is no longer the wall**: write-back on the A715s takes 1920 of 1968 frames, the integrator 4.3 ms a frame where non-cacheable it was 7.5 to 19; on the A510s it keeps the sensor's 61 a second for about 20 s.
- **Everything on the CPU slows as a run goes on**, by the same factor in every run: the A715 integrator 4.4 to 10.4 ms, the A510 integrator 13 to 17 (to 25 in one run), Lumis's draw 14.6 to 33 ms, the maintenance 0.42 to 1.6 ms, from about 3 to 8 s into the stream. Not ferros's governor (`diag clock`: the A715s asked 2367 MHz and the A510s 1704 the whole run), not the memory clocks (the same with `cammem on`, MIF 3744 and DSU 1800 held), no thermal cap set (the zones 62 to 66 C after a run, peaks of 89 to 93 C kept since the boot); `diag thermal`'s divider line reads CLKDIVSTEP bits seen on all three clusters (G#80000901 every bit seen). Unmeasured: the cores' real clock during a run (the hardware's divider steps, or the firmware's power limit). With it the preview falls from about 50 to about 25 new images a second.
- **The non-cacheable A715 run did not starve the driver this time** (no "written while the kist held it", no late frame), the integrator 7.5 ms at first; it slowed to 17 ms with the rest.

**If the front end writes a held buffer** (the Lumis session's "buffer 1 was written while the kist held it", then no frame for 2097 ms, with the A715s starved): `ring_turn` names a buffer neither held nor the newest while one is free (crowded 0 and none free 0 in every run above), so the ring's choice never names a held buffer; read from the code, the write happens only once the stream is already dying, a release missed (the frame thread late past a whole frame, the line masked so the armed action releases once, then LATE_CONFIG), where the engine writes outside the ring's choice. Not reproduced here. What a reader sees then:
- non-cacheable: each line as DRAM holds it when read, old or new: a torn frame;
- write-back: lines read before the write as they were (they stay in the caches), lines read after it from DRAM, new: the same torn frame; a line read twice reads the same both times. No worse: the view is read-only, so no dirty line can carry a reader's copy back over the front end's, and the release's probe check (its line dropped first) still counts the write.

**To repeat:** `$B reload` the kernel; `tools/cambench-sweep.sh <out> 01 000100003214B80B00000000020800040000000007 fll 5170` (BIN the directory holding this branch's camera.bin and cambench.bin; the last byte 07: write-back, A510s, check); Lumis with the inputs above after the driver (`--name camera --serve camera --hw hsi2c1 --hw ispfe --hw csis --hw ispfe-sysreg --store cam --power camera --dma camera --irq ispfe0 --hw hsi2c3 --hw hsi2c2 --hw hsi2c4`, input 01).

## The slowdown, measured (2026-11-18, branch slowdown-probe)

"Everything on the CPU slows as a run goes on" (the section above) is two things, neither of them the camera's.
Measured on the Pro, not attested, the driver hand-spawned with input 01, every stream ended by its own close; the kernel cam-cacheable plus the trace below, RAM-reloaded (b949, then b952).

**The tools added:**
- corebench's watch (input byte 0 = 3): one probe pinned to each core asked (u16 mask), every 2^-4 s its real clock (PMU cycles over the counter), adds and NEON flops a cycle, and dependent-load chases in its own L2, the shared L3 (4 MiB) and DRAM (64 MiB); a line a measure each second. An optional no-camera load (a u16 mask: each core streams 16 MiB into u16 sums).
- cambench's window line every 64 frames: the work a frame, and per frame thread its PMU clock over its own busy time, instructions and backend stalls a cycle, L2 and L3 refills.
- `diag thermal trace [n]` (thermal.rs): each second the zones, the PLL_CON3 rate of CPUCL0, the DSU, CPUCL1 and CPUCL2 (24.576 MHz x M / (P x 2^S)), the three CLKDIVSTEP_STAT words, SYSREG_CPUCL0's MPMM words, the PMIC warning pins (GPA9, GPA10), and how many service-loop passes that second read CPUCL0's CLKDIVSTEP_STAT with bit 31 set (about 2000 passes a second).

**One: ferros's governor, flat, not a slowdown.** cambench, write-back, three A715 threads, u16 add, 2048 frames (33.5 s):

| clocks | work a frame | the threads' real clock | the MAP's maintenance |
|---|---|---|---|
| held at the tops (`diag dvfs policy off`, A715 2367, A510 1704, X3 2914, MIF 3744, DSU 1800) | 2.16 ms, first window to last | 2413 MHz | 394 us |
| the governor | 6.3 to 7.7 ms from the first window | 440 to 780 MHz (the PLL agrees) | 620 to 740 us |
| the governor, the view non-cacheable | 7.1 to 9.6 ms | 540 to 780 MHz | none |
| held, non-cacheable | 4.26 ms | 2390 MHz | none |

- The governor aims each cluster's busiest core at 4/5 busy (service.rs `Gov`). Three threads busy about 7 ms of a 16.4 ms frame never reach that, so the A715s sit at or near their floor, 402 MHz. The camera's lift raises their ceiling, not their floor. cambench still took all 61 frames a second, none skipped.
- The "the A715s asked 2367 MHz" of the section above was the USB diag's own boost: every diag's transfer puts both clusters at their top, and `diag clock`'s "asked" shows it. Its residency showed a time-weighted mean of 524 MHz for the run.
- With every clock held, nothing slows: 2.16 ms a frame for 2048 frames, the A510 run below 3.69 ms for 2048. The die reached only 49 C and 61 C.

**Two: a hardware clock divider on CPUCL0 (the A510s and the DSU) as the die heats.** Lumis 0.2.18, the intake on the A510s (input `280100003214B80B0000000000`), governed, 32 s (twice, the second with the watch's probes):

| seconds into the stream | 1-4 | 5-8 | 9-12 | 13-16 | 17-20 | 21-24 | 25-28 | 29-32 |
|---|---|---|---|---|---|---|---|---|
| new images drawn a second (run 2) | 22 (start) | 55 | 41 | 34 | 31 | 29 | 28 | 27 |
| Lumis's draw a new image (ms) | 14.0 | 13.8 | 19.2 | 23.6 | 25.1 | 27.1 | 29.0 | 30.2 |
| CPUCL0 passes stepped (of about 2000 a second) | 0 | 0 to 22 % | 26 to 37 % | 32 to 40 % | 35 to 47 % | 36 to 54 % | 44 to 53 % | 50 to 52 % |
| LITTLE zone (C) | 66 to 83 | 85 to 87 | 85 to 90 | 85 to 90 | 85 to 90 | 86 to 90 | 86 to 90 | 89 to 91 |

- The steps begin within a second or two of the LITTLE zone reaching 85 to 87 C (the stream's fifth second), grow while it stays there, and stop with the stream (18 % the second it closed at 72 C, 0 at 66 C). Stepping, CPUCL0's CLKDIVSTEP_STAT reads G#80000801; at rest, G#101. CPUCL1's read G#80000801 too in five of the hot seconds (1 Hz reads); CPUCL2's (the X3, at 880 MHz, BIG 79 C) never did.
- The PLLs held their rates thru the steps: A715 2367, DSU 1800 (and in run 1 the firmware's rates agreed, MIF 3744). A divider downstream of the PLL is invisible to both. One coincidence to untangle: in both runs the governor lifted the A510s from 1098 to 1704 MHz in the same second the steps began (their intake saturating as it slowed, or the lift adding the current that trips it); four A510s held at 1704 MHz on a cool die (61 C, below) never stepped.
- The cores' clocks barely moved (the probes' PMU cycles: the A715s 2400 to 2000-2300 MHz, the A510s about 1100-1300 as governed). The memory side slowed: dependent-load latency on the A715s L3 88 to 250-330 ns, DRAM 166 to 400-550 ns; on the A510s DRAM 275 to 700 ns. The DSU (the L3 and the cores' way to DRAM) is clocked from CMU_CPUCL0, so its divider slows every cluster at once, the X3's display included. That is why everything slowed by the same factor.
- The heat: MID and LITTLE went from 48 C to 85-90 C in about 6 s, with the A715s at 2367 MHz (Lumis's UI threads, 1.5 W a core) and the A510s at 1704.
- Not the camera's traffic: cambench on four A510s at 1719 MHz with everything held at the top, 33 s, 61 C at most: not one stepped pass, 3.69 ms a frame from the first window to the last.
- Not MPMM: SYSREG_CPUCL0's MPMM words read 0 in every second (MPMMEN 0: Linux sends ACPM the levels at probe, ferros never does). The warning pins read GPA9 G#11 and GPA10 G#0 in every second read (a 1 Hz read can miss a pulse).


**What drives CPUCL0's divider, from the reference tree** (/mnt/Harbor/ferros-ref; nothing in it gives the status word's bits):
- The power firmware's init writes three words to CMU_CPUCL0 (flexpmu_cal_system_zuma.h:15-17): CLKDIVSTEP_SMPL_FLT at +G#834 = G#8000FFFF, CLKDIVSTEP_CON at +G#838 = G#00F041C3, CLKDIVSTEP at +G#830 = G#00210007. Linux's BCL driver then sets CLKDIVSTEP to 1 (zuma-shusky-bcl.dtsi `cpu0_clkdivstep`).
- CPUCL1, CPUCL2, the GPU and the TPU each get OCP, VDROOP and SMPL filters. CPUCL0, the A510s with the DSU, gets the SMPL filter alone.
- So the one source wired to CPUCL0's divider is SMPL_WARN: the main PMIC's warning that the battery's voltage has sagged under its level. It is the same line every cluster's divider hears, which fits CPUCL1's status stepping in the same seconds.
- No TMU trip, ACPM thermal path or DSU source is named for it. ACPM's thermal governor caps through DVFS, and the PLLs never moved.

**Measured instead (b954, 81-82 % battery, the host's port charging): heat, a cluster at a time, at about 88 C; not the battery.**
- SMPL_WARN_CTRL reads G#68: 2.9 V, below Linux's 3.1 V. OCP_WARN is off on B2M, B3M and B7M (G#0).
- GPA9's CON word is G#2: pin 4 is an input, so the data bit shows SMPL_WARN_R_L. It never read low.
- Under 2.4 A of load the cell never fell under 3.93 V, so neither SMPL level was ever near. Setting Linux's 3.1 V could change nothing, and was not tried.
- **No camera, the watch's load on all eight workers, every clock at its top, 40 s:**
  - CPUCL0 began stepping the second LITTLE reached 88 C (it read 86 the second before, with nothing stepped). It climbed over 15 s to 50-77 % of passes, with LITTLE at 84-91.
  - CPUCL2, the X3's, stepped in exactly the five seconds BIG read 88-90 C. The X3 ran only the kernel, and measured 365 MHz (2914 / 8) in those seconds.
  - CPUCL1 never stepped there (MID at most 87). In the Lumis runs it stepped whenever MID read 88-90.
  - Each cluster's divider follows its own zone, which a shared electrical warning would not do.
  - The load's end stopped the steps within the second, but the zones fell under 85 C within that second too.
  - ACPM's trip counters stayed 0 (ferros never sends TMU_IPC_INIT), and the PLLs never moved. A thermal path into CLKDIVSTEP that the reference tree does not name.
- **Lumis with the intake on the A715s** (input bit 6), governed, 32 s:
  - New images 53 a second falling to 27, the draw 14.5 to 28-30 ms, the integrator 4.3 to 9.9 ms.
  - CPUCL0 stepped from LITTLE's 87 C, CPUCL1 from MID's 88.
  - The stream died at frame 1783, 29 s in ("no frame in 2129 ms", the A715s starved the frame thread, as the section above saw once); the driver closed itself.

**The governor: what it sees, and a proposal (not built).**
- **What it sees:** each 64th of a second it takes the busiest core's awake share and moves the level toward the running rate x the averaged share x 5/4, aiming at 4/5 busy (service.rs `Gov`).
  - The camera lift raises only the ceiling (to 2367 MHz).
  - Under cambench, three threads busy about 7 ms of a 16.4 ms frame read under 4/5, so the A715s settle at or near their floor, 402 MHz, with a saturated window now and then (time-weighted mean 524 MHz).
  - Under Lumis the UI threads' load reads saturated and climbs to the ceiling, 2367 MHz, where it stays: that load is what heats the die.
- **The two loads want opposite things.** cambench keeps 61 a second at the floor, at 3x its held-clock latency (7 ms against 2.16), for little power. Lumis at the ceiling pulls about 6 W from the A715s alone and walks into the divider within 6 s.
- **Proposal:** a ceiling for the A715s while a camera streams (`CAM_HOLD`'s lift capped), instead of their whole table.
  - At 1572 MHz an A715 draws 449 mW against 1514 at 2367 (`pixel_em`, THERMAL.md): the four cores go from about 6.1 W to 1.8 W for two thirds of the clock.
  - cambench's measured points bracket the cost: 2.16 ms a frame at 2413 MHz, 7 ms at about 480. The work is memory-bound, so at 1572 it should land nearer 3 ms than 3.2 (cycles a frame fall with the clock: 5.2 M at 2413, 3.4 M at 480).
  - The test before any build: `diag clock lift 1572000` and the A510-intake Lumis run again, read for the stepped share and the preview's new images a second (55 falling to 27 today).
- **No floor is proposed:** a frame-paced consumer that keeps up at the floor (cambench) loses only latency. Lumis's intake keeps up on the A510s.
- **The test, run (b954): `diag clock lift 1572000`, Lumis with the intake on the A510s, governed, 32 s.**
  - Not one CPUCL0 step. The zones peaked at 79-80 C, not 90.
  - New images held at 39 a second from the second window to the last (55 falling to 27 at 2367).
  - Lumis's draw held at 19.8 ms (13.8 rising to 30). The integrator held 13.0-13.3 ms, with 1920 of 1968 frames integrated (1856 and 1728 at 2367).
  - The MAP's maintenance held at 427-433 us. The A510s stayed at 1097 MHz.
  - The ceiling buys a steady preview at about 70 % of the cold start's rate, for about 4 W less on the A715s. The lift went back to 2367000 after the run; the ceiling during a stream is the owner's choice to build.
- **Saves under the stream, measured 2026-11-19** (THERMAL.md "The busy lift"): a 12.5 MP save at 1572 MHz took 5.76 s against 5.63 s at 2367, the zones ending at 75 C against 90; a 50 MP save took 31.3 s at 1572 (the A510s calm) against 48.0 s at 2367 (the A510s at 1704, every cluster's divider stepping). The cap would make a stream's saves faster as well as cooler.

A reload with this kernel's first trace build (every divider word read every pass, six ACPM rate reads a second) found ACPM answering nothing and the device reset 35 s later (b950, the kernel's core between the clock policy and the heat step, crumb G#D7). The build committed reads only CPUCL0's word every pass and asks ACPM nothing extra.

## Slits under Lumis (2026-11-18, branch slit-robust)

The Lumis session's Slitscan (Lumis lumis-slit d2b67747, later a274d0b9; the driver hand-spawned, input 01; binned 4080 x 64 slits, frame 192 lines, 812 us apart, detached and slim, a ring of 256) died within seconds once Lumis drew and baked on the A715s: run 0b at slit 1296, run 0c at 3235 (with `cammem on`), each on LATE_CONFIG (G#10), though the same driver had drained 261120 slits with nothing drawing.
Measured on the Pro, not attested, the identity unlocked at boot, RAM reloads b955 to b969 over slowdown-probe, the driver and Lumis hand-spawned (Lumis input `18`, 13 zero bytes, `02`: DEV_OPEN, 16 s, DEV_SLIT), every stream ended by its own close; logs in target/agent-slit-robust/runs of the main checkout.

**What the logs said, read again:**
- The one "stopped by a test" of every run is the SHADOW_UPDATE-only interrupt at stream on (status 8, before any DMA_DONE). No release was ever refused at a death.
- Run 0c's `acted A#3237` is its 3236 DMA_DONEs plus one: the program ran twice on one DMA_DONE at the third slit (the place log G#20, G#30, G#50), so the index stood one ahead for the rest of the stream (harmless to the engine, but every slit's record then sat one buffer from its rows).
- The six lock alarms of 7.8 ms or more were all at Lumis's spawns, none in a stream: process.rs's `build_from` after the gate (the lock taken again at line 7356 for a 22 MiB capsule: 20.6 ms at each spawn) and the host spawn's arc staging (service.rs 5434, 12.2 ms).
- Every death had the same shape: one slit's DMA_DONE came while the line was still masked by the slit before (the line stays masked from the interrupt to the frame thread's ack), so the armed action never ran for it, its descriptor went unreleased, and the next slit hit LATE_CONFIG. The armed action covers a frame thread late by up to one period; a slit's period is 812 us.

**Why the frame thread was late past a period, each measured** (tools below):
| cause | measured | fix |
|---|---|---|
| the line on the X3: its waiter noticed only when the service loop's pass came round to `run_slice`, no A715 being idle to notice | 865 us behind the battery gauge's poll (crumbs D6 to F4), 6.0 ms once | an armed line follows its waiter's core (process.rs `irq_route`); interrupt to run then at most 39 to 253 us |
| preempted mid-answer by another line's interrupt on its core, sent to the back of the queue | 4.4 ms behind Lumis's threads (the sensors' line on c6) | a device thread parked unasked in its interrupt's run keeps its place, urgent, and a core is asked for it (`ferros_el0_preempt`, `make_room`) |
| a slice inherited nearly spent, ended mid-answer | parked 2 ms between two syscalls of one answer | the same |
| the kernel lock, test-and-set, under Lumis's pool (its threads yielding every 10 us, 56k holds a second) | waits of 852 to 1029 us with no hold that long | a ticket lock (KERNEL.md Order item 8) |
| its front credit spent (an answer of 60 to 100 us every 812 us, now and then 323) | woken to the back, not urgent, run 2.8 to 9.7 ms late | a device thread's credit leaks a quarter, not an eighth |
| the frame thread's own cost: 56 register syscalls an answer | 91 us an answer | words that read 0 not written back: 60 us |
| the acknowledgement clearing a DMA_DONE it never saw (a slit landed between the first read and the ack loop) | run 0b's death (an answer 786 us late, then LATE_CONFIG) | an unseen DMA_DONE left latched, so the line fires again at the ack |
| SYS_IRQ_WAIT unmasking the line between the interrupt and its mask (the armed program runs first, a few us) | the double release of every run's start, 8 us apart; SHADOW_UPDATE interrupts refired 7 to 10 us later on whole frames | irqarm `TAKEN`: a wait does not unmask a line taken and not yet pending |

**And then the release taken off the frame thread altogether** (ferros_layout irqarm `UNMASK`, KERNEL.md "Armed actions"): a slit ring arms TEST_BITS DMA_DONE, TEST_EQ place index, ADVANCE, WRITE G#1B to the image status (SOF, EOF, SHADOW_UPDATE, DMA_DONE cleared), UNMASK; the driver sets loch0's image interrupt mask (G#1C32_0430, the dtsi's irq-mask-reg) to DMA_DONE alone once its path is on, and counts slits from the engine's index past its own. Each slit's DMA_DONE then fires its own interrupt and its own release whatever the frame thread is doing; a late frame thread takes several slits in one answer. Whole frames keep the three operations: their descriptors are named a frame ahead by the frame thread, which a release it did not see would skip. The hand spawn's tenth byte 1 asks the three for an A/B; a kernel without UNMASK refuses the program and the driver arms the three.

**The runs:**
| kernel (b) and what it had | runs reaching the 16 s (19678 slits) | the others died at slit |
|---|---|---|
| the Lumis session's (slowdown-probe) | 0 of 2 | 1296, 3235 |
| 955 to 958: the record only, then `dev follow on` by hand | 0 of 6 | 58, 195, 358, 430, 2047, 5481 |
| 959 to 963: TAKEN, the ack fixes, armed lines following, preempted kept | 1 of 13 | 1283, 2456, 6662, 7348, 7774, 8656, 10245, 11288, 15249, 15853, 16048, 17608 |
| 965: and the device credit | 0 of 4 | 5603, 6830, 9358, 13442 |
| 966 to 968: and the ticket lock | 5 of 11 | 1641 to 14304 |
| 969: and the open program | **7 of 7** | |
- On b969 each run: every slit released by the kernel and left open (19678 to 19689), one stop (stream on's SHADOW_UPDATE), the gap between acted runs at most 915 to 1044 us, interrupt to run at most 162 us; 0 to 6 answers a run took several slits (5 the most), each a death before; Lumis's own line "A#0 written over while read, A#0 too old to read" every 4096 slits; the hottest zone 62 to 66 C.
- Whole frames on b969 (Lumis, the init's mode): 239 frames, none written over, no error word, one stop a frame (its SHADOW_UPDATE; 379 stops for 239 frames before TAKEN).
- Lumis a274d0b9's fallback ("the slits stopped (the driver ended); whole frames again, and Slitscan takes its band from them") appeared after every death up to b968 and never on b969. After it Lumis opened whole frames thru the vault's stored driver once and failed once ("the camera driver did not answer the open"); a hand-spawned driver that exits after its close leaves the stored one to answer.

**A slit's exposure (the Lumis session's SET_EXPOSURE 68800205 ns, refused)**: that refusal was the vault's stored driver (run 1, from before the detached slit). This driver plans a slit's exposure as any mode's (ferros_cam ccs `Timing::plan`): inside the slit's frame the integration alone, past it the frame lengthened to the integration and the margin. cambench's new +21 (an exposure in ns SET halfway thru a slit run), measured on b969: 68.8 ms taken mid-stream (frame 21863 lines, integration 21861, a slit every 68 ms), the slits after it 15 a second, none lost; 1/2000 s taken (159 lines, inside the 192-line frame, nothing else moved), 1232 slits a second after it. Lumis's own first ask in Slitscan, 3148 ns (one line), is refused rightly: the main's least integration is 2 lines.

**A mode switch in place, without the power cycle and the init** (not built; the open's steps as the console times them): switch on 52 to 61 ms, the sensor's init 2064 to 2098 ms (1347 writes: 1.55 ms a write, every register access of the bus controller a syscall and its polls more), the job, engine and receiver about 17 ms, stream on to the first slit about 28 ms. A switch that kept the sensor powered and initialised would stop the stream, close the front end as the close does (its replay, CPU writes), write the new mode's registers in one hold (about 20 writes, some 30 ms at today's bus rate), place the job and start the engine again (17 ms), and take a new region when the ring's size differs (the slit ring 74 frames against the image ring's 64 to 73: about 5 ms each way, the frames given back wiped): about 0.1 to 0.15 s against today's 2.28 s. The front end's side is what every reopen already does with no power cycle of its domain. The sensor's side is the open question: October's mid-stream geometry change left the sensor stuck until a cold boot, but with no real hold (`w16(G#0104, 1)` wrote the release) and its PD channel on; every change since has been inside a real hold with PD and embedded data off for slits, and has never been tried on a streaming sensor without its power cycle. Separately the init itself could be faster: consecutive registers in one auto-increment transfer, the bus's status polled less.

**A rate change without a reopen: SET's frame length (built and measured 2026-11-20, the Pro b1069, the driver spawned by hand).** cambench's slit (192 lines, 16 lines of integration, 1232 a second) with a SET of 384 lines after 512 slits: taken, the integration still 16 lines, frames 1209 us apart after it, no error word; a SET of 100 lines refused (`[1]`) and the stream ran on at 192 lines. cambench's "lost to the ring" in those runs is its own reader a ring behind while it made the SET and read CAPS; the driver wrote and released every frame. The owner, on Lumis going from stills to RAW video: "why the mode switch? it takes like 5 seconds". Stills and RAW video are the same binned readout; only the frame length differs, and until now it was fixed at the OPEN, so every rate change was a close, a power-up and the init's 1347 writes.
- SET what 4 (`[3][4][fll: u64]`, lines; 0 the OPEN's own): the frame every exposure is planned against (`Timing::fll`) becomes `fll`, and the exposure in force is planned again against it at its own lines (`Timing::ns_ceil`: from `ns`, rounded down, 16 lines came back 15), then written in the hold as an exposure SET is (`plan_apply`, shared with SET's exposure). The frame is the new length, or the integration plus the sensor's margin when the shutter is longer (max(1 / fps, shutter), host test ccs `a_frame_length_set_keeps_the_shutter`); it lands at the next frame's start, the frame in flight finishing at the old length.
- Refused (`[1]`, the timing as it was): past 16 bits, under the readout's rows plus the least blanking the driver asks (a slit's 128 lines), or an exposure no plan holds against the new frame (a camera without long exposures).
- ferros_clip: `Pacing::set_msg()` (the pacing's frame length), `set_open_fll_msg()` (back to the OPEN's), `set_fll_msg(fll)`, `SET_SETTLES` (the frames at the new length are those 2 or more past the last frame before the SET). A clip's rate is its recorder's (`Mode::pacing`, written in the clip's header), so a rate change goes between clips: SET, then a recorder at that pacing.
- The in-place switch to another readout (the paragraph above) is the step after: the same SET path can carry the frame, but a different readout also needs the stream stopped, the front end's geometry, job and engine replaced and, when the ring's size differs, a new region; the sensor's side, a new mode written to a streaming sensor without its power cycle, is the part never tried.

**The tools built for this:**
- `diag "irqs runs"`: the last 64 armed runs (the counter, the core, how each ended, the words each operation read), the stopped runs kept apart, and at a freeze each core's last 8 crumbs; `diag "irqs freeze <us>"` / `"irqs freeze short <us>"` freeze it, and devsched's trace, at the first gap between acted runs longer or shorter; `diag irqs` adds the acted-to-acted gaps by 2^k us and the runs left open. A `dev freeze` at a late run snapshots the crumbs too.
- `diag crumbs` and `diag "crumbs ring"`: the X3's longest stretch ending at each service-loop crumb (F0 to F6 added inside the pass) and the last 256 stretches of 2^7 us or more in time order.
- `diag dev`: woken behind the queue for a spent credit, and the most credit then; `diag sched`: device threads kept at the front when parked mid-run.
- The driver: the first answers with their lengths and counter times, the timeline's start on the counter (to set beside the kernel's runs), answers that took several slits.

**Open:**
- r6 (b958, `dev follow on`) died at slit 58 with every answer and every acted run on time and the index one ahead from the double release; not seen again, not understood.
- A late interrupt on a worker with interrupts open (b965 v4: c6 at EL0 for 886 us before it took the line) was seen once; the open program does not cover an interrupt itself taken late past the blanking.
- **Lumis's slit death of 2026-11-20 (b1069, t5215206), not reproduced.** Lumis's slit (192 lines, 16 of integration) after RAW video at 60/s and a one-frame whole open: the first slit 27.9 ms after stream on, its answer 2257 us (about 1 ms in every healthy open), the second slit 2321 us later with LATE_CONFIG (G#10 at G#20438), then the line quiet; both descriptors had been released by the kernel's armed action.
  cambench on the vault's driver, the same day: the slit alone, three times right after a 60/s paced whole-frame run, and once after 60/s plus a 16-frame whole open with its PD path: every run 1024 slits at 1232 a second, no error word, the first answer 975 to 1009 us.
  So the death needs something Lumis does around the open (its threads, the GPU, a recording just ended) that cambench does not; the long first answer is the one measured difference. The other "failure" Lumis reported (t5185653) was a healthy stream Lumis itself closed after its first slit. `diag "irqs freeze 1500"` is left armed so the next one keeps each core's crumbs.
- The ticket lock, the device credit and the kept preemption change every process's scheduling, measured here only under Lumis and the camera: worth a Photon scroll and the 96-a-second whole-frame bench before forge.
- `build_from` holds the lock 20.6 ms for a 22 MiB capsule (each Lumis spawn).

## Serving while a frame waits (2026-11-18, branch cam-serve-while-waiting): built, host-tested, not run

**The bug** (the owner's Pro, 2026-11-18): the owner dragged Lumis's shutter to its end, a frame of about 13.6 years (the 5-bit shift's reach), and left Lumis. Tapping Lumis again showed only the deck: the new Lumis was blocked in the driver's LIST. The driver has one server thread, and it sat in MAP's wait (`wait_hold`) with a deadline of the frame's late tick, about 14 years out, for a holder that had already ended. Nothing but killing a streaming camera (never: "Never kill a streaming camera") or a reboot could end it; the device was rebooted.

**Two faults, two fixes:**
- **(a) A waiting call held, not waited in.** The kernel's reply token is a capability in the server's table, single use and kept until it is spent (process.rs SYS_REPLY), so a server may hold a call and answer others meanwhile; the display and the GPS driver's WATCH already do. The driver now keeps a call that waits for a frame (MAP, FRAME, SNAP, SLITS) as `Pending` (its token, what it waits for, the count its frame must be past, its deadline) and goes back to its loop. Each pass looks at the held call first (answered when its frame has landed, the same compare-and-swap hold as before, `hold_newest`), then takes the next call: LIST, SET, CLOSE, STATS, RELEASE, PROBE, RATE and CAPS are answered while a frame waits. One held call at a time: a second call that would wait is answered empty (the holder reads frames from one thread). Considered and not taken: a second server thread (every call would then share the open camera's state across two threads, CLOSE racing a MAP that holds the region and the ring), and the kernel cutting a wait short on a call (a call does that already: `wait_or_call`, below).
- **(b) The holder's end, told by the kernel.** A reply token tells its server nothing until the reply fails, and a 14-year frame's reply comes in 14 years. New: SYS_CALLER x1 (KERNEL.md "A caller's end watched"): with the OPEN's reply token, before replying, the driver names a word, and when the process behind that OPEN ends, any way, the kernel sets bit 63 of it (`CALLER_ENDED`) and wakes the driver's threads waiting on it. The stream then closes at once thru its own close, the held call let go. No poll: the driver learns it in the wait it already makes.
- **One wait for all three.** The word is the driver's doorbell, `DOOR`: the frame thread counts every frame in its low bits (`door_ring`, an atomic add), and wakes the server only while it holds a call for a frame (`WANT`), so a server holding none sleeps thru a slit stream's thousand frames a second (as `QUIET` did, which it replaces). The kernel's end mark is in the same word, so neither a frame nor the end can land between the server's look and its wait unseen: the wait returns at once when the word moved since the look. Calls end the same wait (ferros_sys `wait_or_call`, SYS_WAIT x3 bit 1; with nothing held, `wait_world_or_call` as before).
- **On a kernel without the watch** (the answer lacks `CALLER_WATCHED`; the console says so at the open): a held call's reply failing closes as before; another process's OPEN finding the camera held asks whether the held call's caller still lives (ferros_sys `caller_alive`: SYS_CALLER answers NOTHING once the token went with its process) and closes the dead holder's stream, answering that OPEN 3 to be asked again of a fresh driver; else 2^22 us with nothing asked and nothing held, as before. So the driver alone, on the flashed kernel, already unblocks a new Lumis (LIST answered; its OPEN closes the dead stream and is answered 3; an OPEN asked again starts a fresh driver, whether Lumis asks again by itself is Lumis's), and the kernel adds the stream closing the moment the holder ends, the module powered down with it.

**CLOSE with a frame in flight**: the stream's own close as always (the view taken back, the sensor's stream off, 2^16 us, the frame thread stopped, the switch off: the engine stopped, Linux's close replayed, the S2MPUs shut, the module down), whatever the frame's length, then the held call answered empty (Lumis reads it as closed) and the CLOSE `[0]`. The front end sees no frame start until a frame's readout, at its end, so a long frame's close is a close between frames.

**SET while a frame waits, and the cut** (the owner, earlier: "let's say I start a 64s exposure and 5s in I decide, well maybe I want it 12s"; the coordinator for Lumis, 2026-11-18: "A SET whose new exposure would end before the frame in flight should drop that frame and start the new one at once ... The cost is one dropped frame"):
- Every change measured mid-stream lands at a frame's start, in the hold ("Long exposures": the 16 s change took the frames in flight first, the 8 s step four of them), so nothing in the hold cuts a frame short.
- So when the new timing's frame, begun now, would end more than 2^20 us (`CUT_US`) before the frame in flight (begun at the last DMA_DONE, at the period written before it: `FLIGHT_AT`, `FLIGHT_NS`), the SET cuts it (`exposure_cut`): CCS frame truncation asked (fast_standby_ctrl G#0106 = 1, no local init writes it), the stream off, the new timing in the hold while the sensor stands by (the open's own order), the stream on. The console says how much of the frame in flight was left and, at the next answered frame, how long after the stream came back on it landed.
- Under 2^20 us of difference, a longer exposure, a slit ring (its armed open program and records), or before a stream's start: the change lands at the next frame's start, as before.
- The arithmetic is ferros_cam ccs `Timing::cut_due`, in 128 bits (a 14-year frame's ns times the counter's 24.576 MHz passes 64 bits), host-tested (`the_cut`: the 14-year frame cut a second in and 13 years in; 64 s cut 5 s in for 12 s; not at 59 s in; not inside the margin; never for a longer frame).
- **Unmeasured, the device test's first question**: whether this Samsung sensor truncates (CCS lets a sensor finish the frame regardless: then the stream on finds it streaming and the change lands at the next frame's start, the same as no cut, and the held call keeps its long deadline, so nothing closes wrongly), and whether the front end takes a stream off and on with no close between (the link goes quiet and comes back as at an open's stream on). If the sensor will not truncate, the other road is a close and reopen of the stream inside the driver (about 2.3 s, the init's writes), not built.
- A held call's deadline moves later with an exposure that lengthens the frame (`late_tick`, which counts the frame before and the new one for two frames), never earlier: the frame in flight may still be the old one.

**What did not change** (c): the frame thread's answer is the same but for its last step, where `wake(&FRAMES, 4)` (a syscall every frame) became the doorbell's add and a wake only when the server waits for a frame, plus two stores of the frame in flight's start; the armed release (`arm`), the open program for slits, the descriptor releases, `ring_turn` and `slit_turn`, the close and its order, the hand spawn's inputs. Still a timer, flagged (feedback "no polling"): the frame thread's look every 2^17 us (`QUIET_US`) with the line quiet, which the close waits on and which answers a front end left waiting without an interrupt (2026-11-16); across a 14-year frame it wakes eight times a second. A sibling's doorbell cannot end an `irq_wait` today; taking it off the clock wants that (a word that ends an interrupt wait, as a call ends a word wait).

**Built:** the kernel's watch (process.rs `end_watch`, `end_watch_ring`, rung in `end_process` and `kill_locked`), ferros_sys `caller_watch`, `caller_alive`, `wait_or_call`, `CALLER_WATCHED`, `CALLER_ENDED`; the driver (above); ferros_cam ccs `Timing::cut_due` with its test; camtest's fourth byte 9 (below). Lumis's side (branch lumis-leave-long, std/lumis 0.2.21, the coordinator's): on leaving it sends CLOSE once and its window thread never calls the driver.

**Device test plan** (needs the coordinator's slot; the Pro, FERROS_PORT=1-2.2, the debug bridge; nothing stored, nothing flashed; the driver hand-spawned with input `01` each run, one open each; every stream ended by its own close or by its holder's end, never by killing the driver; `B=target/debug/ferros-bridge`, `O=target/agent-camserve/bin`):
0. **The kernel** (for the watch): RAM reload of this branch's kernel (`target/agent-camserve/kernel/aarch64-unknown-none/release/ferros_kernel`), a minute after a boot; the identity unlocks at boot. Without the reload, steps 1, 2 and 5 still run (the driver says the kernel does not watch) and steps 3 and 4 test the fallback instead.
1. **LIST, STATS, a cut, CLOSE while a 17-minute frame waits**: `$B spawn $O/camera.bin 01 --name camera --serve camera --hw hsi2c1 --hw ispfe --hw csis --hw ispfe-sysreg --store cam --power camera --dma camera --irq ispfe0 --hw hsi2c3 --hw hsi2c2 --hw hsi2c4`, then `$B spawn $O/camtest.bin 00000009 --name camtest --use camera`, then `$B diag console`. Expect: LIST and STATS while the MAP waits in milliseconds (LIST about 870 bytes); the SET to 67 ms answered in tens of ms, the driver's "the frame in flight cut with about A#1022000 ms of it left" (a 1024 s frame, 2 s in), the waiter's MAP answered about 70 to 140 ms after the SET (camtest's line) and the driver's "the first frame after the cut landed A#.. ms after the stream came back on"; the second long SET; CLOSE answered in under a second with the waiter's MAP answered empty; the close's usual lines (link clean, S2MPU faults 0). If the cut does not land ("NOT answered 4 s after the SET"), the sensor finished the frame: stop there, the stream closes with camtest's CLOSE.
2. **The 8.5-year frame and the holder's own end**: a new driver, camtest `00000009001C01`: camtest ends with the MAP held and no CLOSE; the driver's "its holder's process has ended (the kernel's watch); closing" within milliseconds of camtest's exit line, then its close lines and its exit.
3. **A holder killed from outside**: a new driver, camtest `00000009001C02`, then `$B diag "kill camtest"` once camtest says it holds: the same closing line at once.
4. **Lumis, the owner's case** (with the Lumis session; Lumis 0.2.21 from lumis-leave-long hand-spawned with the driver, or both as the coordinator stores them): the shutter to its last stop; send Lumis behind: it exits in under a second with "the camera closed for leaving", the driver closes; a fresh Lumis lists and opens normally. Then the shutter to its last stop and back to 1/60 s without leaving: the preview back at once (the cut).
5. **Regressions**: camtest `4001` (64 frames thru the view, every release the kernel's); a slit run, cambench `00034000000000000000000002080000` (64 rows, detached, slim, quiet: about 1230 a second, 0 lost, the drain's SLITS held and answered each); 96 a second with a consumer, cambench `00010000800C00000000000002FF0003000002` (fll 3200, u16 add on three threads, 16320 frames, every release the kernel's, no death).

### On the Pro, 2026-11-18 evening (this branch's kernel RAM-reloaded over the flashed forge bc1bef30 as b974, the identity unlocked at boot even so; the driver hand-spawned from target/agent-camserve/bin, one open each, input `01`)

- **Step 1 passes, the cut lands.** camtest `00000009`: LIST 1105 bytes in 14 ms and STATS in 44 us while the 1024 s frame's MAP waits; the SET to 67 ms answered in 2.5 ms, "the frame in flight cut with about A#1022094 ms of it left", the first frame after the cut 92 ms after stream on, the waiter's MAP answered 95 ms after the SET; CLOSE answered in 168 ms, the waiter's MAP empty; link clean, no error word. So this Samsung sensor truncates on G#0106 and the front end takes a stream off and on with no close.
- **Steps 2 and 3 pass**: "its holder's process has ended (the kernel's watch); closing" in the same millisecond as camtest's exit (`00000009001C01`), and at once after `diag "kill camtest"` (`00000009001C02`), each closing thru its own close.
- **But 2^28 s is not a long frame on the sensor.** At `…1C…` the driver plans shift 31 ("a frame every A#268447626580 ms") and the sensor ignores it: frames kept coming 10.2 ms apart (about 96 a second, the sensor's floor), 188 MAPs answered in the 2 s before the hold, so in steps 2 and 3 the MAP was held only between frames. The same two ends with a really held MAP (`00000009000A01`, `00000009000A02`, 1024 s at shift 13, no frame for 2 s until the cut) closed at once too. Shift 13 holds; somewhere between 13 and 31 the sensor stops honouring the shift, unmeasured (b933 ran 1 to 8).
  - Measured since ("The shift's ceiling" below): not a shift ceiling but the frame counter's 32 bits of lines; 39700 << 31 wraps to 0, the sensor's floor.
- **Regressions pass**: camtest `4001` 64 frames 66.7 ms apart; the slit sensor 1218/s, front end 1209/s, 1238 slits a second read, 0 lost; 96 a second 18184 frames, 16320 mapped (1757 skipped, 7.2 ms of work a frame), no error word, every release the kernel's, 55 C to 75 C.
- Rebooted after (b975, cold, the flashed kernel): "identity unlocked at boot", fgtw "a fleet member with the fleet key".

## 1:1 on the main (2026-11-18, branch cam-full-res): built, host-tested, not run

The owner: "why don't we have the high rez 1:1 options in Lumis?"
On Android Lumis lists a second readout per lens ("50 MP" on the main, 8160 x 6144, a 4 x 4 quad CFA); on ferros the driver listed one binned readout per camera, and its 1:1 was a guess from the CCS registers that never streamed (2026-11-16: the main's link ERR_CRC_PH, about 2 sensor frames a second).

**The capture** (keys/husky/captures/longexp-16s-2026-11-17.*, local; "The capture, 2026-11-17 evening" above, Lumis on Android opening the main in its maximum-resolution mode, then 16 s):
- Integrity: the files' blake3 match B3SUMS; tools/oracle-i2c reads 1404 transfers to G#3D and **0 whose byte count differs from its programmed length**, one controller, one target. One stream on (`01 00 01 03`, transfer 749) and no stream off in it.
- Before the stream on: the identity read, then 747 writes, the same 6528-byte vendor upload as the binned init (the same blake3), then Android's first hold.
- **What it changes against the binned init** (both read flat, `--flat`, each address's last value compared): 193 addresses. On the CCS page: the PLL multiplier G#0306 G#D7 (215, the binned G#F5, 245), the line G#0342 G#2D50 (11600 clocks, the binned 6320), the output G#034C/G#034E G#1FE0 x G#1800 (8160 x 6144), the increments G#0380 to G#0386 2 (one pixel: the main's addresses run 2 a pixel), the binning G#0900 G#2222 (the binned G#3244; the guess's G#0011 was not it), G#0B08, the frame G#0340 G#18B7 (6327) and integration G#0202 G#11DA (4570) in its hold. The window (G#10 to G#3FCF, G#10 to G#300F) is the binned one's. About 170 vendor words on pages G#2000 to G#2002, among them G#2001_7C2E/7C30 = 8160 x 6144.
- **What it does not change**: every op (link) clock word, G#0300 to G#0312 but the multiplier. So the CSI lanes run at the binned rate and the receiver's captured table holds (`RX_START`), only its channel sizes rewritten. A 1:1 line of 8160 RAW10 samples is 10200 bytes in 6.586 us, 1.55 GB/s, under the 1.80 GB/s the binned link carried at 5696 clocks ("On the Pro with (b)").
- Phase detect stays on in it (G#0720 = 1) with the same 2040 x 768 block as the binned init; what that puts on the link's channel 1 at 1:1 is not known (the front end's half of this session was never trapped).
- From the clocks, not measured: a line 11600 clocks at 24.576 MHz / 3 x 215 (1761.28 MHz), **6.586 us**; a frame of 6327 lines **41.67 ms, 24.0 a second**; the readout 6144 rows, 40.5 ms. The binned line's 3.1489 us is exactly its own clock's (b933), so the same reading is expected to hold.
- Android's 16 s in this mode was shift 6 with integration G#9446 (37958 lines of 2^6): ferros's plan for 16 s at the 1:1 line gives the same to a line (host test).

**Built:**
- `tools/oracle-i2c --init <out> --holds G#3D`: a target's transfers up to its first stream on (G#0100 on the CCS page) as an init table (W, R, D records, the delays the main's first init had: 10 ms after the first three, 5 ms after the upload, 1 ms after each other write), refused if the capture has no stream on or any transfer before it lost bytes. Run on the capture: 748 transfers, 15499 bytes, target/cam/main-full.init (local, the vendor's settings; blake3 81359be0b2d8a75e), which read back `--table --flat` equals the capture's own writes line for line.
- ferros_cam ccs `Mode::captured_full` (the 1:1 mode out of that init, bin 1; refused unless its output is one pixel an array pixel over its window and `Array::full`: the binned init under the name is refused), `line_ps_for` (the camera's measured line time scaled by the init's PLL, `Cam::pll`), `Cam::listed_mode_timing`; cams `pll` and `fll`; list `Readout` (a section per readout) with `mode`, `cfa_period` and `frame_ns`.
- payloads/camera: kinds 2 and 4 replay `cam.<key>.init.full` in place of the binned init (the CPU's writes and the job stay the binned session's, their geometry rewritten to 8160 x 6144 at G#2800 a line, `Scope::Core`); kind 2 with zeros is the 1:1 init's own frame and integration, written again in the hold with phase detect and embedded data off (`F_LOUD` keeps them); the statistics pipeline detached and the slim program by default (ferros_cam geom `shape`: what a window needed, and what whole binned frames ran clean in); the timing at the 1:1 line (the open's console says the line, the PLL and the frame); LIST's second section; the header's +14. Buffers of 60 MiB (6144 lines of G#2800), a ring of four 302 MiB with the region, rings of three, two, one as the pool allows; FRAME by copy refused at 1:1 (it bins the captured shape only), so frames go thru the view.
- A device test with nothing stored: a hand-spawned driver takes the 1:1 init in its input, after 16 knob bytes, as `--input-file` frames it, in place of the vault's.
- **A hand-spawned driver takes the 1:1 init in its input only after the 16 knob bytes; without them it ignores the init** (input `01` + 15 zero bytes, `01000000000000000000000000000000`, then `--input-file target/cam/main-full.init`).
- camtest's eighth byte, the OPEN's kind (`4001000000000002`: the 1:1 thru the view); its view run says the mosaic's 4 x 4 cell (`cells`: each place's mean, so a quad array shows 2 x 2 blocks alike and a Bayer one a period of 2); its long-exposure ladder runs thru the view at bin 1 (`0401000500000002`).
- Host tests: the 1:1 mode, its clock, its timing and the shift (ccs `the_main_full_mode`), the two sections as Lumis's parser reads them (list `the_mains_two_readouts`), the table's writer (oracle-i2c `an_init_table`).

**Unknown until the device says** (the risks, in order):
- Whether the front end takes an 8160-wide line into loch0 at all with the binned session's program (Android's front-end program for this mode was not trapped). No frame, or SHORT_VB or a resolution mismatch on loch0, would say so in the close's `fe_say` and `ack_say` lines; then the capture to take is the PDMA dump and the 42 front-end pages of a Lumis 1:1 session ("The 1:1 captures").
- Whether detaching FC ctx3 alone quiets the statistics pipeline for a frame larger than it was set up for (the windows were smaller); the outputs' report after the eighth frame says which outputs were written and how far.
- The quad CFA: Lumis on Android reads its maximum-resolution RAW as a quad array, and this is that session's init; nothing in the sensor's words names a remosaic. camtest's cell table on a lit coloured scene settles it.
- Nothing electrical is new: the same module, rails and sequence, the same vendor upload; the sensor's PLL runs slower than the binned one.

**What Lumis needs** (std/lumis; not changed here):
- LIST: sections are readouts now; group them by `id` (and `name`), offer one entry per section, and open an entry by its `mode` (absent: 0). A Lumis from before lists "main" twice and opens the second as the binned readout while its section says 8160 x 6144.
- OPEN: the view with the mode block's kind 2 and every other field zero (`[1][0][1][2]`, then 12 zero bytes for the rest of the block, then the view flags byte as now): Android's 1:1 mode, its own frame and integration, then SET as ever (the exposure's plan at the 1:1 line, gain and focus the same).
- Frames: MAP only (FRAME by copy answers empty at 1:1); 8160 x 6144 RAW10 at 10240 bytes a line, 60 MiB a buffer, read write-back as now.
- The mosaic: `cfa` G B / R G as 2 x 2 blocks of one colour (`cfa_period` 4, header +14): each 2 x 2 block averaged is the binned readout's pixel, the phase as the binned frame's (the window's start, 8 pixels, is a whole quad cell).
- The rate: `frame_ns` 41670368 (24 a second) against the binned 67267143 (14.9); `readout` 40.5 ms (the rolling shutter's skew, four times the binned 9.67 ms); `exposure_min` 6586 ns.

**The device plan** (the coordinator's slot; the Pro on its flashed kernel, no reload, nothing stored, every driver hand-spawned with the init in its input, one open a driver, every stream ended by its own close, stop at the first open whose frames stop):
```
B=target/debug/ferros-bridge; O=target/agent-camfull/bin
D="--name camera --serve camera --hw hsi2c1 --hw ispfe --hw csis --hw ispfe-sysreg --store cam --power camera --dma camera --irq ispfe0 --hw hsi2c3 --hw hsi2c2 --hw hsi2c4"
$B spawn $O/camera.bin 01000000000000000000000000000000 --input-file target/cam/main-full.init $D
$B spawn $O/camtest.bin <input> --name camtest --use camera      # or cambench.bin <input> --name cambench --use camera
$B diag "console kept 64"
```
0. Regression on this driver: camtest `4001` (binned, 64 frames thru the view, about 67 ms apart; LIST about 560 bytes longer than before: two sections for the main, 1210 bytes for the main alone against 651).
1. The 1:1, map only: cambench `00020000000000000000000000040000`. The driver's lines: the init from the input (15499 bytes), 747 writes and 1 read, mode 2 8160 x 6144 binning G#2222 frame 6327 lines of 11600 clocks, detach G#1, a line 6586 ns at PLL 215, a frame every 41670 us; RATE sensor and front end both about 24.
2. The frames and the mosaic: camtest `4001000000000002` (64 frames thru the view, about 41.7 ms apart; the cell table, on a lit scene with colour in it; a whole frame read).
3. The rate with reading: cambench `00020000000000000000000001040000` (every line read out of the view) and `00020000000000000000000002040000` (u16 add).
4. Long exposure at 1:1: camtest `0401000500000002` (64 ms to 2 s and back), then `04010005000E0A02` (1 s to 16 s): the frames the exposure apart, the sensor reading back frame, line 11600 and integration.
5. Regressions: the slit (cambench `00034000000000000000000002080000`) and 96 a second (cambench `00010000800C00000000000002FF0003000002`), each its own driver (input `01`, no file).

### On the Pro, 2026-11-18 evening (b973, the flashed forge bc1bef30, no reload, nothing stored; every driver hand-spawned from target/agent-camfull/bin, one open each, every stream ended by its own close)

| step | input | result |
|---|---|---|
| 0 binned | camtest `4001` | pass: 64 frames 66.5 ms apart, link clean, no error word |
| 1 the 1:1 opens | cambench `…00040000` | pass: init 747 writes 1 read in 1.3 s, mode 2 8160 x 6144 G#2222, line 6586 ns at PLL 215, a frame every 41670 us; RATE sensor 23-24/s, front end 21-22/s over the first second; 64 mapped at 24/s; DMA_DONE gaps 41525 to 41802 us; the link at 1492 MB/s |
| 2 frames, mosaic | camtest `4001000000000002` | pass: 64 frames, DMA_DONE gaps 41550 to 41759 us; a whole frame read thru the non-cacheable view in 205 ms |
| 3 reading | cambench `…01040000`, `…02040000` | pass: 24/s, 0 skipped both; read 16.2 ms a frame (4 threads), u16 add 29.3 ms |
| 4 long exposures | camtest `0401000500000002`, `04010005000E0A02` | pass: 64 ms to 16384 ms, frames the exposure apart (16384079 us at 16 s), the sensor reading back line 11600, shift 0 to 6, integration 38869; the step back to 64 ms cut the frame in flight (2030 ms and 16362 ms of it left) and the next frame landed 120 ms after stream on |
| 5 slit | cambench `00034000000000000000000002080000` | pass: sensor 1226/s, front end 1214/s, 0 lost |
| 5 96 a second | cambench `00010000800C00000000000002FF0003000002` | first run died, a rerun cool passed (below) |

**The CFA is quad.** The cell table (4 x 4 means, a dim indoor scene, black 64) read the same in the first and the 64th frame: rows 0 and 1 `80 80 73 73`, rows 2 and 3 `83 83 80 80`. That is 2 x 2 blocks of one colour, G B / R G as blocks (G 80, B 73, R 83, G 80), the binned readout's phase. The binned table (step 0) is a period of 2: `102 86 102 86` / `109 102 109 101`.

**The 96 a second death was heat, not the driver.** Photon was in front rendering every frame (76 a second, 12.5 ms a draw), which kept the A715s lifted at 2367 MHz; after the 1:1 runs the zones read BIG peak 92 C, MID and LITTLE 90 C, CPUCL0's divider stepping (G#80000801). In that state cambench read only 36 frames a second (27 ms of work a frame, 1522 MHz) and the stream died at frame 26718 (about 278 s): one DMA_DONE gap of 10939 us, then LATE_CONFIG (G#10) and image overflow G#1, the driver's own close after 2117 ms with no frame. Every DMA_DONE had been released by the kernel's armed action; `diag dev` showed ispfe0 interrupt to run at most 3061 us. Photon sent behind (the Deck tap), the Pro fell from 86 C to 54 C in about 3 minutes, and the same run passed: 17201 frames at 96 a second, 16320 mapped (775 skipped, 6.2 ms of work a frame), no error word, every release the kernel's, 72 C at the end.

## The shift's ceiling (2026-11-18, branch cam-exposure-ceiling)

The question ("Serving while a frame waits", its Pro run): 2^28 s at shift 31 gave frames at the sensor's floor while 1024 s at shift 13 held. Where does the main stop honouring a long exposure, binned and 1:1?
**Answer: not at a shift.** The sensor counts the frame in lines, frame_length_lines << shift, in 32 bits, and wraps past them: a frame of 2^32 lines or more runs as its remainder modulo 2^32 (at the sensor's floor when that is 0). So the longest exposure is the longest integration whose frame stays under 2^32 lines: 65533 lines of 2^16 (the frame 65535 << 16). **Binned 13523.9 s (3 h 45 min); 1:1 28285.9 s (7 h 51 min).**

**The tool**: camtest's fourth byte 10 (`ceiling`): OPEN with the view, a second thread MAPping, then for each shift (seventh byte to sixth, 14 to 31 by default) the shift forced (SET 3) with an integration of the ninth and tenth bytes' lines (u16 LE, 2^15 by default) of 2^shift; 2^20 us for the old timing's frames, then a window (first byte, 8 s) counting the frames that land and their spacing, with CAPS before and after (now with the shift words, +103); then 67 ms again, which cuts the long frame, and the next frame within 4 s or the run closes. Third byte 1: no shift forced, the driver's own plan. Nothing waits out a long frame: a held frame is no frame in the window.
The driver hand-spawned (`01`; at 1:1 `01000000000000000000000000000000` with `--input-file target/cam/main-full.init`), nothing stored, nothing flashed, every stream ended by its own close.

**Binned** (b975, the flashed forge bc1bef30; camtest `0800000A`, one open, 32768 lines of integration, so a 32770-line frame, at each shift):

| shift | asked | frames in 8 s | spacing | (32770 << shift) mod 2^32 |
|---|---|---|---|---|
| 14 | 1690 s | 0 | held | 2^29 + 2^15, held |
| 15 | 3381 s | 0 | held | 2^30 + 2^16 |
| 16 | 6762 s | 0 | held | 2^31 + 2^17 |
| 17 | 13524 s | 9 | 825.461 ms | 2^18 lines: 825.47 ms |
| 18 | 27049 s | 5 | 1650.924 ms | 2^19: 1650.94 ms |
| 19 | 54098 s | 2 | 3301.827 ms | 2^20: 3301.9 ms |
| 20 | 108197 s | 1, 6.6 s after the SET | | 2^21: 6.60 s |
| 21 to 30 | 216395 s to 1.1 x 10^8 s | 0 | | 2^22 to 2^31: 13 s to 1.9 h |
| 31 | 2.2 x 10^8 s | 776 | 10.310 ms, the floor | 0 |

- Every step read back as written (frame 32770, line 6320, integration 32768, both shift words G#0E00 to G#1F00), and the sensor's own frame count (G#0005) stood still in the held steps and moved in the others: the words are taken, the frame is what wraps.
- Every spacing is 2^(shift + 1) lines of the measured 3.148916 us line to 0.01 %.

**The model's checks** (one open each, binned):
- A 4-line frame (integration 2) at 29, 30, 31: 29 held (2^31 lines), 30 and 31 at the floor (4 << 30 is 2^32: 0), 10.31 ms.
- A 5-line frame (integration 3) at 29, 30, 31: all held (5 << 30 wraps to 2^30, 5 << 31 to 2^31). So shifts 30 and 31 are taken as written; on 2026-11-17 and on the evening run they only looked ignored because their frames wrapped to 0 (shift 32's 18-line frame, the 39700-line frame at 31: both even).
- The edge: 65535 << 16 (2^32 - 2^16) held; 32767 << 17 (2^32 - 2^17) held; 32768 << 17 (2^32) at the floor.

**1:1** (b976, the 1:1 init in the spawn's input; one open each): 32768 lines at 17 gave frames 1726.501 ms apart, 2^18 lines of 6.586118 us (1726.51 ms); 32768 << 17 (2^32) ran at the 1:1 floor, 41.6 ms; 65535 << 16 held, and 32770 at 14, 15 and 16 held. The same 32 bits of lines: the counter is lines, not time, so the 1:1's longer line doubles its ceiling in seconds.

**What is inferred, not seen**: no frame past b933's 105.66 s (2^25 lines) was waited out. That 2^32 - 2^16 lines really ends after 3.76 hours rests on the wrap, which every spacing above matches exactly; a frame at the ceiling has not been watched land.

**A reset, not reproduced**: the first 1:1 sweep (one open, shifts from 14, b975) ended with the Pro resetting about 10 s into shift 15's window, a held frame with nothing moving, after shift 14 had held 8 s and been cut cleanly. Cold boot b976, no panic record (the record's words all ones), nothing in the flash log after the open's first frame. The same sequence again in one open (14, 15, 16, each cut) and each step on its own ran clean. Cause unknown; noted in case it returns.

**Built** (ferros_cam ccs, payloads/camera, payloads/camtest):
- `Lte::frame_bits` (32 on the main) and `Lte::holds`; `Timing::shift_most` (16 on the main), the least shift that holds the longest integration; `plan` stops its least-shift search there and refuses an exposure whose frame passes the counter (never caps, as past any range); the stretched-line path is checked too.
- `Timing::most_ns`: the longest integration the counter holds at the readout's line, so the list's `exposure_max` per readout is the longest honoured exposure: binned A#13523872135774 ns, 1:1 A#28285866534109 ns (they were 443150242 s and about 926871274 s, 14 and 29 years, at shift 31). Lumis's slider and its LONG EXPOSURES go by the list; nothing changes on its side.
- `plan_shift` (SET 3, a device test) is not held to the counter: forcing a wrap is how it was measured.
- The driver's open says `the frame counter A#32 bits (the longest exposure at shift A#16)` and `exposures to A#13523872 ms`; the first LIST of a run says each readout's `exposure_max`; CAPS is 107 bytes (+103 the shift words G#0702 and G#0704).
- Host tests: ccs `the_frame_counter` (every measured held and wrapped frame, the plan at 3 hours taken and 4 refused, 65534 lines of 2^16 refused, the 1:1 readout, the stretch), `the_mains_shift`, `the_cut`, `the_main_full_mode`; list `the_mains_two_readouts` prints both ranges.

## The other cameras on the Pro (2026-11-18 night, branch pro-cam-captures)

The owner: "all four cams up with all modes on the pro ... slitscan and all".
The Pro on the flashed forge bc1bef30 (b976, both slots the signed gate blake3 d4494f92, the identity unlocked at boot); every driver hand-spawned from target/agent-procaps/bin (forge 7451dd26), input `01`, one open each, every stream ended by its own close; logs in target/agent-procaps/runs.

**The front answers: the LV buck was it.** camtest `0000000702` ("Device test plan" step 7): the switch raised sub GPIO2 (G#40 to G#60) after LDO1, then LDO8, and the sensor answered G#30A1 at G#0000, with G#0C (its actuator) and G#51 (its EEPROM) acknowledging beside G#10. The NACK of 2026-11-06 was CAMS_LVBUCK_EN low, as the device tree said.
- Its inputs stored as step 7 says (absent before): `cam.dokkaebi.init` (front.init), `.prog` (front.cpr2), `.pdma` (front.pdma), `.idt` (cam7-front.idt.vsf), each read back and compared exact. So LIST now carries the front (1761 bytes with main and ultrawide).
- **Its first frames** (camtest `4008000002`): init 503 writes and 1 read in 663 ms, stream on 742 ms after the open, the first frame at 801 ms; 64 frames at bin 8 (480 x 342), mean sample 78; no error word, the overflow words 0, every DMA_DONE released by the kernel's armed action; closed cleanly.
- **Its view** (camtest `4001000002`): a ring of four 16 MiB buffers, 3840 x 2736 at 4800 bytes a line; 64 whole frames 41.1 ms apart on average (41.9 the longest); a frame read thru the non-cacheable view in 33 ms. The PD rows land in the buffer too (+G#C86400, 684 rows).
- **Its mosaic, measured**: the cell table (4 x 4 means of the whole frame, the first and the 64th the same) reads rows `79 97 79 97` / `75 78 75 79`: a period-2 Bayer cell with G at (0,0) and (1,1) (79, 78), R 97, B 75, so **G R / B G**, as its calibration capture said.
- **Its rate is 24 a second, not the 30 guessed**: DMA_DONE gaps 41657 to 41849 us. The driver's timing still takes the guessed line (8.35 us, 4001 lines, "a frame every A#33409 us"), so every front exposure it reports (the header's 14295 us, `exposure_max` 33 ms) is off by the same factor. If the sensor runs the init's 4001 lines the line is about 10.43 us; not fixed here: the sensor's frame and line read back (CAPS) and a frame-length step are the measure, then `FRONT.line_ps`.
- The driver's open reads its words: integration least 2, margin 5 (the table has none, so exposures stay inside the mode's frame), gain = code / 32 with codes 32 to 512 (analogue 1x to 16x, Samsung's CCS model as the main's), the init leaving code G#20 (1x).

**The tele answers too** (camtest `0000000703`): G#08D5 at G#0000, with G#2D (itself) and G#35 acknowledging; its power sequence and the LV buck are right. Only its capture is missing (receiver, CPU setup, job: "The tele's capture").

**The ultrawide's slit streams, detached and slim** (the driver's eighth byte `81`: FC ctx3 off, the program as the open asks; cambench `01034000000000001000000002080000`: 64 rows, the slim flag G#10). The open: mode 3, 4000 x 64 (y G#D50 to G#DCF, binning G#0122), frame 192 lines of 9568 clocks (1259 us), phase detect and embedded data off; detach switched off 1 program word; the slit ring of 256 buffers of 320 KiB; the release armed open.
- RATE: the sensor's counter 798 a second, the front end 793; the link clean (+14 G#0, two channels starting and ending, +24 = +2C = G#3).
- The first run died at slit 838 on LATE_CONFIG (G#10): one DMA_DONE's interrupt ran 8997 us after the one before (`diag "irqs runs"`, on core 4), so its release came seven slits late. The late-interrupt class of "Slits under Lumis" ("Open"), not the ultrawide's shape: the rerun took 969 slits clean, and a drain (`…02FF0000000004`) took **65280 slits in 82.2 s (793 a second), lost to the ring 0, read over 0, the engine ring round 16 times**, no error word, every release the kernel's, 54 C.
- On 2026-11-16 the same slit took two frames and stopped; the switch is the detach (with the slim program), as on the main. So the ultrawide's windowed modes want the same defaults as the main's (ferros_cam geom `shape` keeps them to the main: not changed here, an override per open for now).

## The I2C side without a flash (2026-11-18 night, branch pro-cam-captures)

The hv oracle is a build, not a switch: `FERROS_HV_ORACLE*` are compile-time knobs (ferros_hv/build.rs, `cfg(hv_oracle)`, the pages and the I2C target baked in), the gate both slots carry is built without them, so every oracle capture flashes a capture image to boot_a. But the sensors' half of a capture needs no hv at all: Android's kernel traces every I2C message at its i2c core (`events/i2c`: i2c_write with the whole buffer, i2c_read, i2c_reply with the bytes read, i2c_result), and root can turn that on in the gate's own Android. LWIS sends every sensor transfer thru i2c_transfer (its bus threads, `lwis_I2C_Bus_N`), so the trace holds them.

**The session** (one Android boot via `ferros-bridge diag android`, no flash, no hv change; Lumis on Android opening each readout from its menu; the owner's go for Android and Lumis, 2026-11-18):
- Armed under root: `buffer_size_kb` 32768 (a CPU), `trace_clock` boot, a filter on i2c_write, i2c_read and i2c_reply of `addr == 0x3d || addr == 0x1a || addr == 0x10 || addr == 0x2d` (the four sensors), i2c_result unfiltered, console at level 4, marks written to `trace_marker`.
- Lumis's menu by taps from screenshots (1344 x 2992; the cards at y 1620, 1872, 2122, 2370 of the screen; a lens's card opens its readouts in the same places below BACK; the shutter slider's knob dragged from x 660 to 1060 at y 2442 goes to its longest; EXIT at 1050, 1582 leaves Lumis).
- Every readout Lumis lists besides the main's: the ultrawide's 12.0 MP (4000 x 3000) and 48.0 MP (8000 x 6000); the front's 10.5 MP (3840 x 2736) and 8.42 MP (3440 x 2448); the tele's 12.2 MP at 22 degrees (4032 x 3024, 1.40 um), 12.2 MP at 11 degrees (4032 x 3024, 0.70 um) and 48.8 MP (8064 x 6048); each opened, 10 s of preview, the shutter to its longest (16 s, the front 1 s) for two or three frames, then EXIT.
- Stopped, then the text (`trace`) and each CPU's ring pages (`per_cpu/cpuN/trace_pipe_raw`) pulled with the stats and the event formats: keys/husky/captures/ftrace-2026-11-18 (local only: Google's vendor settings), sha256 checked against the device's, B3SUMS.
- Two traps: Android's "System update needed" dialog was up on the home screen (dismissed with Back: an update writes slot b); the host's hub (2-2, an ASM107x) dropped and came back every few tens of seconds while Android ran at SuperSpeed, and adb's server lost the phone each time (restarting the server finds it: target/agent-procaps's helpers wait for it). Then `adb reboot`: b977, the gate chose ferros, "identity unlocked at boot".

**Integrity**: every CPU's stats `overrun: 0`, `dropped events: 0`, `commit overrun: 0`; 2039 pages, none flagged with missed events; tools/oracle-i2c `--ftrace` (below): 0 writes whose bytes are not their length, 0 reads unanswered or short, 0 transfers on the sensors' buses not completed whole; 31504 transfers, one sensor a bus (i2c-1 the main G#3D, i2c-2 the front G#10, i2c-3 the ultrawide G#1A, i2c-4 the tele G#2D).
**Against the hv**: the inits cut from this trace equal the hv's captures (uw.init 2026-10-22, front.init 2026-11-06, tele.init 2026-11-06) word for word but for each session's own integration and frame length (2, 2 and 1 words), the tele's 21599-byte init included. So the trace is as exact as the oracle for the I2C side, every message whole (its text output cuts at 64 bytes; the ring pages do not).

**The tool**: `tools/oracle-i2c <dir> --ftrace [--adapter N]` reads the ring pages (kernel/trace/ring_buffer.c's layout: page header time and commit, 5-bit type and 27-bit delta, time extends, length words, `__data_loc` arrays) by the saved `format` files, merges the CPUs by time, and gives the same transfers as an hv capture, so `--holds`, `--flat` and `--init` work as before; `--from <mark text>` starts at a mark (each open of one session its own init); `--stream N` takes the init between stream ons N - 1 and N (the ultrawide's close streams on for a moment, so its next open's init is the second). Host-tested (`ftrace_pages`).

**What the inits say** (keys/husky/captures/ftrace-2026-11-18/init: `.init` tables, `.flat`, `.holds`):
- **The ultrawide, 48 MP** (init 256 transfers, 6823 bytes): output G#034C/E = 8000 x 6000, binning off (G#0900 = 0, G#0901 = G#11, G#0902 = G#0A), the line G#0342 = G#2850 (10320 clocks), its crop and scaler words G#0408 to G#040F at 8000 x 6000, Sony's G#3200/3201 = 1, G#0808 and G#084E to G#0859 (MIPI timing), **and another op PLL** (G#030B 4 to 1, G#030F G#A7 to G#28, G#0307): its link runs at another rate than the binned mode's, so ferros's receiver table for the ultrawide (`RX_START_UW`, the binned session's) may not settle on it; that is the front end's half, below.
- **The ultrawide's long exposure is Sony's shift G#3100**: at 16 s each hold writes the integration (G#0202/3), the frame (G#0340/1, the integration plus 64) and G#3100 = 6 (lines counted in 2^6), every byte a single-byte write; 12 MP: integration G#94D4, frame G#9514 (38164 x 64 lines of 6.56 us = 16.0 s); 48 MP: G#B49E and G#B4DE.
- **The front's 1 s is the main's mechanism**: its hold `0104=0101 0702=0100 0704=0100 020E=0100 0204=.. 0202=BB1E 0340=BB24 0104=0001` (shift 1 in the high byte of G#0702/0704, the frame the integration plus 6), frames 1.0 s apart. 47908 x 2 lines in 1.0 s is a **10.44 us line**, the 24 a second ferros measured (4001 lines x 10.44 us = 41.8 ms). Its 8.42 MP readout is the same init with the output set to 3440 x 2448 (G#034C/E written twice: G#0F00/G#0AB0, then G#0D70/G#0990).
- **The tele** (init 1142 transfers, 21599 bytes; the 48.8 MP 1137): long exposure as the main's (16 s: `0702=0600 0704=0600`, integration G#9FE8, frame G#A000, margin 24); the 11-degree and 48.8 MP readouts write G#0B30 = G#0101 (the 22-degree G#0100), and the 48.8 MP uploads other vendor tables (two 966-byte blocks at G#2002_AC00 and G#2002_B000, pointed to by G#2000_151A/151C, where the binned has 950-byte ones at G#5A00/5E00); its CCS size words still say 4032 x 3024 and binning G#2222, so the 48.8 MP output size lives in those tables, and what reaches the link is the front end's half.
- Every close streams on and off once more after its stream off (the tele `0100=0000`, `0100=0102`, `0100=0000`; the ultrawide `0100=00`, `0100=01`, `0100=00`).

**On ferros with them** (the Pro b977, the flashed forge bc1bef30, drivers hand-spawned from target/agent-procaps/bin, nothing stored; logs target/agent-procaps/runs/p2-*):
- **The front's long exposures run**: its line measured (ferros_cam cams `FRONT.line_ps` 10.437753 us: 8448 clocks at 2023.424 MHz over vt_pix_clk_div 5, two pixels a clock; the guess was 8.35), its margin 5, its shift the main's words (`ARRAYS[2].lte`, most 1, Android's). camtest `04080005020A`: 64, 128, 256, 512 and 1024 ms (shift 1), frames 63.9, 128.1, 256.2, 512.3 and 1023.5 ms apart, the mean above black doubling, the sensor reading back each frame and integration, then 64 ms again. LIST offers 1.37 s. Trap: a SET before the front's first frame left it with no frame at all (twice, even an integration-only SET), while every SET mid-stream was taken; camtest's ladder now takes the first frame first, as Lumis does.
- **The ultrawide's long exposures run to 16 s**: Sony's shift as one byte at G#3100 (ccs `Lte::byte`: a 16-bit write would touch G#3101), most 6. camtest `02080005010E`: shifts 1 to 6 from 512 ms, frames 516 ms, 1.02, 2.05, 4.10, 8.20 and 16.41 s apart, the mean rising each step, the sensor reading back, back to 64 ms, no error word. LIST offers 27.5 s.
- **The ultrawide's 48 MP, the sensor's half runs**: ccs `captured_full` now takes a cropping part's init (binning off, the crop inside the window the array's full size); the driver hand-spawned with the traced init in its input (`01` + 15 zero bytes, `--input-file target/cam/uw-full.init`), cambench `01020000000000000000000000040000`: init 252 writes, CAPS reads 8000 x 6000, binning off, frame 7707 lines of 10320 clocks, the sensor's counter 23 a second; one frame landed (the NEON check read its line 3000, mean 66), then the link reported errors (+G#14 G#7700C0) and resolution mismatches (+G#1C G#78) and loch0 G#404 (SENSOR_RESOL_MISMATCH, STREAM_ERROR); the driver closed itself. Its op PLL differs from the binned mode's (the link rate) and the front end's channel sizes stayed the binned session's: the front end's half, below. Stopped there.

**What still needs the hv** (and so a boot_a flash, the owner's go): the front end's half of every readout ferros has not run: the tele's receiver (its link and lanes), CPU setup and PDMA job, for any of its readouts; the ultrawide's 48 MP and the tele's 48.8 MP front-end programs and link words; the front's 8.42 MP if its window does not run on the 10.5 MP's program. Those are the 42 front-end pages and the PDMA dump ("The tele's capture", steps 0 and 1); the I2C step (2) is done here.

**The front-end captures, built and waiting for the go** (2026-11-18 night): tools/husky-hv-longexp.sh takes `KIND=isp` (the 42 pages of target/hv/ispfe-pages.txt, cap 32768) and `KIND=pdma` (G#1C39_0000 with the mid-stream dump, cap 4096), the same stage 2, UFS firewall, no PMU reads, arming at 60 s, loglevel=4 and owner-key footer as the 2026-11-17 capture image. Built: target/agent-procaps/images/boot_a_isp.img (blake3 ad573dd8...) and boot_a_pdma.img (f3d810ce...), B3SUMS beside them. Both slots hold the signed gate boot_pro-flash-1118_718df3bb_vaultcarried.img today (blake3 d4494f92, read back by `bridge part dump`), so boot_b stays the fallback. The steps, each after the owner's go, from a tree with this commit:
```
G=/mnt/Harbor/Code/keys/husky/images/boot_pro-flash-1118_718df3bb_vaultcarried.img
I=/mnt/Harbor/Code/ferros/target/agent-procaps/images
FERROS_PORT=1-2.2 IMG=$I/boot_a_isp.img tools/husky-hv-longexp.sh flash $G   # bridge reboot fastboot; boot_b <- G (the same bytes), boot_a <- isp; never a set-active
tools/husky-hv-longexp.sh start                                                # Android past 62 s, console quiet, marked
# Lumis: one readout, 10 s of preview (the tele 22 degrees first), EXIT
tools/husky-hv-longexp.sh end target/hv/tele22-isp.log                         # crash, the next boot traps nothing, the lines pulled
# reboot once more for the next capture boot (the trap is one shot per cold start), then start / one readout / end again: tele 48.8 MP, ultrawide 48 MP
FERROS_PORT=1-2.2 IMG=$I/boot_a_pdma.img tools/husky-hv-longexp.sh flash $G   # the same three readouts with the PDMA dump
tools/husky-hv-longexp.sh restore $G                                           # boot_a back to the gate; the Pro boots ferros, the identity unlocks at boot
```
Hazards: the whole 64 MiB send failed twice on 2026-11-18 behind the hub at 2-2.2 (husky_flash_paths: the cable), and the prefix route cannot carry a new footer, so a failed send means another port or cable before anything else; the hub (2-2, ASM107x) also dropped every few tens of seconds under Android tonight. If Android does not boot from a capture image (2026-10-29), ABL falls back to boot_b's gate: stop after the first failed boot. Decoding: tools/oracle-isp (the Rust port of camera-cpu.py, oracle-decode-pdma.py and camera-pdma-image.py; "Every readout, captured" below).

## Every readout, captured (2026-11-18 night, branch pro-oracle-captures)

The owner wants every camera in every mode on the Pro. Missing after "The I2C side without a flash": the tele (its receiver, CPU setup and job never captured), the ultrawide's 48 MP (its link errored after one frame), the front's 8.42 MP (untried).

**The decoders in Rust** (tools/oracle-isp; the three Python tools removed):
- `cpu <capture> <a> <b> <c> <out> --link G#1C0A [--name X] [--sysreg]` (camera-cpu.py): the 'CPR2' and the receiver table. The lines: a the line after `oracle armed` (or the previous session's PDMA stop), b the PDMA start (G#1C39_0000 = 1), c the first index advance after it (G#1C39_0010 = G#10). `--sysreg` also takes G#1C02_0504 (the tele's SYSREG pairing word, which camera-cpu.py dropped).
- `pdma <dump> --list N` (oracle-decode-pdma.py), `image <dump> <out> --list N [--height N] [--set G#reg=G#val]` (camera-pdma-image.py), `cpr2-set` (one CPU word changed, a device test).
- Each prints an integrity line: oracle lines not whole, `rep` runs, a dump's offsets that skip and its words against the length its `PDMA buffer` line names.
- Checked on the captures that still exist (the main's own ISP and PDMA dumps went with Harbor tmp): the ultrawide's 2026-10-22 dump gives uw-pdma-writes.txt (`pdma --list 3`) and uw.pdma (= the vault's cam.oksoko.pdma, `image --list 3`) byte for byte; its 42-page capture gives uw.cpr2 (= cam.oksoko.prog) and uw.rx.rs (= ferros_cam rx `RX_START_UW`) byte for byte with `cpu 19264 19862 20036 --link G#1C06`.

**The captures** (keys/husky/captures/oracle-2026-11-18, local only, B3SUMS): the previous agent's two signed images (boot_a_isp.img ad573dd8, boot_a_pdma.img f3d810ce) flashed to boot_a with tools/husky-hv-longexp.sh (boot_b rewritten with the gate's same bytes first; the 64 MiB sends went thru this time, 1.5 s each), Lumis on Android driven by taps from screenshots, one readout a capture boot, 10 to 12 s of preview, EXIT, the crash flush, the zone pulled whole (`end` now checks its sha256 against the device's).
| capture | lines / lists | integrity |
|---|---|---|
| tele22-isp, tele11-isp, tele48-isp | 32789, 23630, 17157 lines | 0 oracle lines not whole; the PDMA start and the close's stop both inside (the whole session fit under the cap) |
| uw12-isp, uw48-isp, front8-isp | 32789 lines each | 0 not whole; the cap reached mid-stream (the open and many frames inside) |
| tele22/11/48-pdma | 2 lists each (18B8 / 188C, 1838 / 1800 bytes) | every list whole: offsets contiguous, every word parsed, every block load captured |
| uw48-pdma, front8-pdma | 3 and 4 lists | the same |
Every pulled zone's sha256 equalled the device's. Traps: the script took the first adb device for the Pro, with the 8a on Android beside it (`restore` would have sent the 8a to its bootloader; fixed: the Pro's serial alone, ferros by FERROS_PORT); with two readouts a lens's menu puts them at 1872 and 2122 below BACK, with three at 1620, 1872 and 2122 (the first ultrawide capture took the 12 MP: kept, as uw12). After the last, both boot slots back to the gate (read back by `bridge part dump`: d4494f92 both), b983, "identity unlocked at boot".

**What they say:**
- **The tele** is CSI link 6 (G#1C0A), the csis3 D-PHY's two lanes (G#1C10_3200, 3300) with the common block, SYSREG G#1C02_0500 = G#20 and G#1C02_0504 = 4; its link start word G#205D01; the main's frame acknowledgement (no G#1C35_0148). Its CPU setup 133 writes before the engine and 65 after (48 for the 48.8 MP). Android's job writes its image as 16-bit samples (loch0 format G#1C32_0550 = 2, stride G#1F80); the stored job is made RAW10 packed (format 4, stride G#13C0, `image --set`), as every other camera's, and streams so. Its close stops lanes, link and both SYSREG words before the PDMA (`RX_STOP_TELE`, captured; the driver writes it, the flashed kernel has none for the tele).
- **The tele's 48.8 MP** writes RAW10 packed itself (stride G#2760), no PD channel, two outputs (a header and the image); its 11 degrees is the 22-degree job with a 248 x 1488 PD channel.
- **The ultrawide's 48 MP** differs from its 12 MP in the lanes' G#08, G#0C and G#30 words (G#33, G#1662, 1 against G#1F3, G#F6AB, 2): the slower op PLL, which is why the 12 MP's table never settled on it.
- **The front's 8.42 MP** has the 10.5 MP's receiver with smaller channels; Android's job writes it 16-bit (packed here, stride G#1100).

**Built** (ferros_cam, payloads/camera, camtest):
- ferros_cam rx: `RX_START_TELE`, `RX_START_TELE11`, `RX_START_TELE48`, `RX_STOP_TELE`, `RX_START_UW48`, `RX_START_FRONT842`. cams: the tele streams; `Alt`, a captured readout replayed whole (its own init, CPU program, job and receiver, nothing rewritten, its measured line and switches), opened by the mode block's kind 8 + k: the ultrawide's `w48` (kind 8), the front's `f8` (kind 8), the tele's `t11` (kind 8) and `t48` (kind 9). Their vault inputs `cam.<key>.<what>.<tag>`.
- payloads/camera: kind 8 + k; LIST a section per captured readout whose inputs are stored; the spawn's eleventh byte frames any of the 1:1 init, init, CPU program, job and IDT (bits 0 to 4, `--input-file` in that order) for a device test with nothing stored.
  With the eleventh byte 0, one frame after the knobs is a 'CINP' set (akita-stream's `spawn_input`, by name) or the 1:1 init alone; akita-stream's `words_say` at every close moved from the eleventh byte to the twelfth at the merge (2026-11-19).
- camtest: at bin 1 a sixth byte sets the exposure to 2^n ms after the first frame (a dim scene's cell table).

**On the Pro** (b983, the flashed gate's kernel, no reload; every driver hand-spawned from target/agent-oracle/bin, one open each, every stream ended by its own close; logs target/agent-oracle/runs):
| readout | frames | rate | line | mosaic (cell table, the owner's red-lit room) |
|---|---|---|---|---|
| tele 22 degrees, 4032 x 3024 | 68 at bin 8, 64 thru the view, all 7 outputs written, link clean, no error word | 30 a second (DMA_DONE 33.24 to 33.40 ms) | 6.1068 us (two pixels a clock) | G B / R G (`104 79` / `164 104` at 2 s) |
| tele 11 degrees (kind 8) | 64 thru the view, clean | 30 a second | 9.8712 us | G B / R G, Bayer (the sensor remosaics) |
| tele 48.8 MP (kind 9), 8064 x 6048 | clean | 12 a second (83.3 ms) | 13.2008 us | the quad array, blocks G B / R G |
| ultrawide 48 MP (kind 8), 8000 x 6000 | 66 thru the view, link clean | 24 a second (41.55 to 41.74 ms) | 5.4067 us | the quad array, blocks R G / G B |
| front 8.42 MP (kind 8), 3440 x 2448 | 64 thru the view, clean | 30 a second | the 10.5 MP's, 10.4378 us | G R / B G |
- The ultrawide's 12 MP read R G / G B too (`261 195` / `195 131` at 256 ms): the half turn of its calibration capture's B G / G R, which the init's flip (G#0101 = 3) makes; and the tele's G B / R G is a column from its capture's B G / G R. ferros_cam cams now carries the mosaics measured; the rotations (from the captures) are unchecked on a ferros frame.
- **Corrected 2026-11-22** ("Focus calibration and the rear mosaics"): this table's mosaics took each scene's brightest place for red, and the rear cameras saw the room's blue light; the ultrawide's and the tele's are B G / G R as their calibration captures and the tele's own pixel_order say (the tele G R / B G unflipped), the front's G R / B G stands.
- **The front's 8.42 MP needs FC ctx3 off**: with the processing blocks as captured the engine took the first frame, then stalled inside the second program (its place G#14 against the index G#20), no SHADOW_UPDATE, LATE_CONFIG on both paths; neither the packing (Android's own 16-bit job did the same), nor loch0's timing words (G#1C32_0478, 047C), nor the slim program changed it; detach 1 alone did (`Alt::detach`).
- Regressions on the same driver: the main (`4008`, 67.2 ms frames) and the front's 10.5 MP (`4001000002`, 41.7 ms) clean; LIST 4560 bytes, eight readouts.

**Long exposures to each sensor's ceiling** (camtest's ladder and ceiling, "The shift's ceiling"'s method):
- **The tele**: 64 ms to 65.5 s, frames the exposure apart (16.39 s at 16 s, 65.54 s at 65.5 s), the mean doubling each step, the shift words read back. Its own words: integration least 4, margin 4 (Android used 24), gain code / 32 over codes 32 to 2048 (1x to 64x). The ceiling: shifts 14 to 29 held every frame tried (8 << 29 = 2^32 lines and 32770 << 29 among them), 30 and 31 ran at the sensor's floor (19.9 ms) whatever the frame: so no 32-bit wrap as the main's (32770 << 17, which wraps to 2^18 lines on the main, held here), and the field ends at 29. Offered: frames held to 2^32 lines (65531 lines of 2^16, 7.3 hours) as on the main; none past 65.5 s watched land.
- **The front**: 16386 lines at shifts 14 to 19: 14 to 17 held, 18 frames 5.4724 s apart ((16386 << 18) mod 2^32 = 2^19 lines of 10.4378 us), 19 one at 10 s: the main's 32-bit line counter. Its most 19, its longest 65530 lines of 2^16 (12.45 hours); it was 1.37 s.
- **The ultrawide**: a 3164-line frame at shift 6 gave frames 1.3287 s apart; shift 7 stopped the stream (no frame for 5.5 s, the driver closed it), and a 555-line frame stopped at 6 (the frame must hold the readout in its own units). Its ceiling stays 6, 27.5 s.

**Slits on the tele**: the binned 64-row slit opens (the window and a 192-line frame read back), but the tele runs no faster than 30 frames a second in this mode whatever the frame length (a 3280-line whole frame, 20 ms, also ran at 33.3 ms), and every slit sets the image path's G#404 with the link's +1C G#40 (a size the link did not expect). With FC ctx3 off and the slim program, 128 slits at 30 a second, 0 lost, each with those words; without, one slit and the stream stopped. Not clean: the tele's frame floor is a vendor word not found, and its window is not only the CCS words.

**Stored** (the Pro's vault, each read back equal): `cam.kraken.init`, `.prog`, `.pdma`, `.idt` (the tele 22 degrees and its IDT, cam4-tele.idt.vsf), `cam.kraken.{init,prog,pdma}.t11` and `.t48`, `cam.oksoko.{init,prog,pdma}.w48`, `cam.dokkaebi.{init,prog,pdma}.f8`. A hand-spawned driver with no input lists all eight readouts and opens the tele from them (68 frames clean). The stored `camera` driver is still the old one: it ignores these (the tele not listed by it) until this branch's driver is stored.

**Left**: this driver into the vault (and Lumis's mode numbers 8 and 9 tried from its menu); the tele's frame floor and slit window; the rotations of the ultrawide and the tele on a ferros frame; noise tests of the tele's and the front's gain ranges; the main's own front-end captures were lost with Harbor tmp (its 1:1 still runs on the binned job rewritten).

## Slits on every camera (2026-11-18 night, branch cam-slits-all)

The Lumis session ran every Pro readout in Lumis: the slits on the ultrawide, the front and the tele each took 2 frames, then "no frame in 2099 ms" (the link +14 G#0 on the ultrawide, +1C G#40 on the front and the tele).
The Pro b983 (the flashed gate's kernel, no reload, the identity unlocked at boot); every driver hand-spawned from target/agent-slitsall/bin, one open each, every stream ended by its own close; logs target/agent-slitsall/runs.
Every slit below is a 64-row binned slit as Lumis opens it (kind 3, rows 64, every other byte 0: no slim flag, no override), cambench `0X034000000000000000000002080000000005`: 4096 slits added into u16 on the A715s.

**The cause, the same on three cameras: the statistics pipeline.** The defaults that let the main's slit run (FC ctx3 detached, the slim program: "The slit runs") applied to the main alone (geom `shape`, `main`), so the others opened their slits with FC ctx3 on and the full program, as the main's slits died in 2026-11-17's runs. The front, with the override (driver byte 8 `81`, cambench's slim flag), took 128 slits at 503 a second at once; the ultrawide had run so earlier in the day. `shape` now gives every camera's windowed (and 1:1) mode the defaults; whole frames are as before.

**The tele's two faults, measured:**
- **Its 30 a second: the sensor drops a mode written before stream on.** Frames of 8000, 6000, 5000 and 4000 lines written in the hold before stream on (kind 1, whole frames) all ran 30 a second, and a 64-row slit's window and size were dropped too: opened with the front end left at the whole frame (`F_KEEP_FE`), the slit's sensor sent clean whole 4032 x 3024 frames at 30 a second (link +1C G#0, no error word). That is the size mismatch of every tele slit (image path G#404, SENSOR_RESOL_MISMATCH and STREAM_ERROR; link +1C G#40, VRESOL_MISMATCH: the front end set for 64 rows, the sensor sending 3024) and its 30 a second (the init's 5457-line frame, 33.3 ms, whatever was asked). Written after the first frame (`F_MID`), the same words take: 8000 lines 20 a second, 4000 lines 40, the slit 559. The CCS words read back as written either way (CAPS), so the sensor holds them and its stream on runs its own mode over them; long exposures always worked because they are SETs mid-stream. Built: ferros_cam cams `Cam::mode_live` (the tele's true), the driver writes such a camera's mode after its first frame; that first frame arrives at the init's shape and loch0 counts it once (G#404 at frame 0, none after).
- **Its slit floor**: the frame asked against the sensor's counter, 64 rows: 192 and 256 lines gave 558 a second (1.79 ms, about 293 lines of 6.1068 us), 320 lines 513, 384 lines 427, 512 lines 322 (each the frame asked). So the tele's least frame is about 293 lines, as the main's is about 262.
- After a cut (a SET shortening the exposure by over 2^20 us: stream off and on) the tele's frame word reads its init's 5456 again (its stream on), while the integration written in standby holds and lengthens the frame: camtest's ladder 64 ms to 2 s and back cut the 2 s frame, and the 64 ms frames after it ran 64.04 ms apart, the mean as before. So exposures survive a cut; a slit ring never cuts.

**Measured, 4096 slits each, none lost to the ring, none read over:**
| camera | slits a second (sensor / front end / drained) | frame | link at the close | error words |
|---|---|---|---|---|
| main | 1219 / 1209 / 1231 | 192 lines, its floor about 262 | +14 G#0, +1C G#0 | none |
| ultrawide | 798 / 793 / 794 | 192 lines of 6.5616 us, 1.26 ms | +14 G#0, +1C G#0 | none |
| front | 503 / 497 / 499 | 192 lines of 10.4378 us, 2.00 ms (as asked) | +14 G#0, +1C G#40 | image 0; PD path G#414 |
| tele | 559 / 554 / 557 | its floor, about 293 lines of 6.1068 us | +14 G#0, +1C G#40 (frame 0) | image G#404 at frame 0 alone |
- The front's +1C G#40 and its PD path's G#414 (SENSOR_RESOL_MISMATCH, LATE_CONFIG, STREAM_ERROR) read as its phase detect (the image path is clean; inferred, not isolated): no PD switch is known for it (ccs `Side::pd` None), so its sensor keeps sending PD rows the link's channel 1 was resized for (`geom::value`: a quarter of the slit's rows); the image path is clean and every slit lands. Its PD control word, or the channel 1 size its sensor sends in a slit, would quiet it; neither is known.
- Regressions with this driver: the tele's whole frames (kind 0) 64 at 30 a second, clean; the main's 1:1 (kind 2) from the vault's stored init, 64 at 23 a second, clean.

### A readout's field of view

The Lumis session: a field per LIST readout that tells modes apart in a menu (the tele's two 4032 x 3024: 22 degrees and the 11-degree 2x crop).
Each section now carries `field_width_mm`, `field_height_mm` (the sensor's part it reads), `fov_width_deg`, `fov_height_deg`, `fov_diagonal_deg` and `focal_equiv_mm` (its 35 mm equivalent), all f5, from ferros_cam optics `Optics::field`: the readout's array pixels (`Readout::covered`, its size times the array pixels each output pixel gathers: 2 binned, 1 at 1:1) over the array the sensor size measures (`Optics::array_px`: the rear sensors' maximum resolution, the front's 10.5 MP), times the sensor's size (Camera2 SENSOR_INFO_PHYSICAL_SIZE), and the lens's focal length (the calibration captures' `focal_mm`). The angles need an arctangent and a square root core has none of: `optics::atan`, `optics::sqrt`, host-tested against std's to 1e-14.
The captured readouts' gather is a field of each (`Alt::gather`, 1 for all four): the tele's 11 degrees cannot be read off its init, whose CCS words say the whole array binned as the 22 degrees' do; its crop is the vendor's G#0B30 = G#0101, and Android's virtual camera 6 reports half the tele's sensor (Lumis docs/google-bug-tele-maxres-fov.md).
| readout | size | field (mm) | angles across width x height, diagonal (degrees) | 35 mm equivalent |
|---|---|---|---|---|
| main, mode 0 and 2 | 4080 x 3072, 8160 x 6144 | 9.792 x 7.373 | 70.72 x 56.23, 83.22 | 24.36 |
| ultrawide, mode 0 and 8 (w48) | 4000 x 3000, 8000 x 6000 | 6.400 x 4.800 | 110.26 x 94.21, 121.72 | 12.06 |
| front, mode 0 | 3840 x 2736 | 4.685 x 3.338 | 81.05 x 62.69, 92.78 | 20.61 |
| front, mode 8 (f8) | 3440 x 2448 | 4.197 x 2.987 | 74.89 x 57.18, 86.45 | 23.02 |
| tele, mode 0 and 9 (t48) | 4032 x 3024, 8064 x 6048 | 5.645 x 4.234 | 17.82 x 13.41, 22.18 | 110.37 |
| tele, mode 8 (t11) | 4032 x 3024 | 2.822 x 2.117 | 8.97 x 6.73, 11.19 | 220.75 |
The diagonals equal Android's computed ones (83.2, 121.7, 22.2, 11.2), the 35 mm equivalents the captures' recorded ones; host tests optics `the_diagonal_angles_are_androids`, list `every_readouts_field`. On the Pro the driver's first LIST says each readout's size and diagonal (6541 bytes, nine readouts with the 1:1 stored). What Lumis does with them is Lumis's.

**Each board's own optics (the merge of akita-stream into forge, 2026-11-19).**
The optics are the camera's (`Cam::optics`, as its CCS facts are `Cam::array`): the Pro's `OPTICS`, the 8a's `AKITA_OPTICS`; until then the list took the Pro's by camera id, so the 8a's main would have listed the Pro main's sensor and lens.
The 8a's are its Camera2 report (`dumpsys media.camera`, keys/akita/captures-2026-11-18/dumpsys-media-camera.txt): SENSOR_INFO_PHYSICAL_SIZE over SENSOR_INFO_PIXEL_ARRAY_SIZE, the focal length and aperture.
The 8a lists no maximum resolution, so each array is the readout ferros streams and gathers one array pixel a pixel (the main's 1.6 um pixels, the IMX712s' 1.12 um); the main's 2x (physical camera 4, 3.6992 x 2.7776 mm) is its middle quarter, not a readout of ferros's.
| 8a readout | size | sensor (mm) | focal, aperture | angles across width x height, diagonal (degrees) | 35 mm equivalent |
|---|---|---|---|---|---|
| main (streams) | 4624 x 3472 | 7.3984 x 5.5552 | 5.43 mm f/1.89 | 68.53 x 54.18, 80.86 | 25.39 |
| ultrawide | 4208 x 3120 | 4.7130 x 3.4944 | 1.835 mm f/2.2 | 104.18 x 87.19, 115.95 | 13.53 |
| front | 4208 x 3072 | 4.7130 x 3.4406 | 2.74 mm f/2.2 | 81.39 x 64.25, 93.60 | 20.32 |
Host tests optics `the_8a_sizes_agree` (the 2x's size, the pixel pitches), list `the_8as_main_lists_its_own_optics`.

### The main's 1:1 init stored

`cam.boitata.init.full` stored in the Pro's vault from target/cam/main-full.init (15499 bytes, blake3 81359be0b2d8a75e; local only, the vendor's settings) and read back equal (`cmp`). A driver with nothing in its input lists the main's 8160 x 6144 readout (mode 2) and opens it from the vault: init 747 writes and 1 read in 1.3 s, 64 frames at 23 a second, the link clean.

**Open:**
- This driver is not stored: Lumis's on-demand `camera` is still the vault's older one until it is (`$B capsule` then `store put camera`), so Lumis's slits on the other cameras stay as they were until then.
- The front's phase-detect switch (its +1C G#40, harmless to the slits).
- Why the tele's sensor runs its own mode over the CCS words at stream on: a vendor word in its init (the G#0B30 writes, G#00FF then G#01FF then G#0100, look like a mode select and apply) may say; written mid-stream is enough for now.
- The front's slit floor (192 lines ran as asked; shorter not tried).

## The ultrawide's least gain (2026-11-19, branch uw-gain-slit)

The Lumis session on the Pro: the ultrawide's 64-row slit ran 794 a second until the first gain SET, even at unity (1024), then 397 a second (2517.5 us, 384 lines' worth); exposure and focus SETs left it alone.
The Pro on its flashed forge kernel; every driver hand-spawned from target/agent-uwgain/bin, cambench's SETs in a slit run (its +29 what, +30 how many, +31 the step), each followed by CAPS and RATE; the driver's thirteenth spawn byte 6 says every gain SET with the sensor's words read back at once and 2^14 us later; logs target/agent-uwgain/runs.

**Not the hold, not the frame.** After the gain SET that halved the slits, the sensor read back frame G#C0 (192 lines), integration G#10, the code written, digital G#100, the hold G#0 (released), G#3100 G#0 and G#0350 G#1, at once and 2^14 us later, the same as before the SET.
**The sensor sends every frame; the front end takes every other one.** RATE over a second after the SET: the sensor's own counter (G#0005) 799 to 801 frames a second, the front end 396 a second.
Every link and error word stayed clear (+14 G#0, +1C G#0, no error word, no overflow).

**The analogue code, not the write.** In the same run, set one after another from the init's G#3C0:
| codes asked, in order | slits a second after each |
|---|---|
| 0, G#200, G#300, G#380 | 397, 794, 794, 794 |
| 0, G#72, G#CD, ... G#1DE (a ladder of 128ths) | 397, then 794 at every step |
| 0, 3, 7, B, F, G#13, G#17, G#1B | 397 to F, 794 from G#13 |
| F, G#10, G#11, G#12, G#13 | 794 at every step |
| 1, 9 | 397, 397 |
| 7 | 794 |
- So codes 0 and 1 set from the init's code put the sensor in a state where the front end drops every other slit; once in it, codes to F keep it there and G#13 (19) or more leave it. Codes 2 to 6 were not tried.
- Whole frames are not touched: camtest's gain curve (1x first) on the ultrawide ran every step's frames 33.25 to 33.43 ms apart, the mean rising with the gain from 1x.
- What the state is inside the sensor is not known (a readout that runs longer at the lowest gains, leaving the 192-line frame too little blank for the front end, would fit, unmeasured; a 256-line slit did not stream at all on this kernel, from its first frame, before any SET).

**The fix: the list offers the ultrawide from code 7.** ccs `ULTRAWIDE_LEAST` = 1031 (1024 x 1024 / 1017, 0.01 stop over one) is the least of `ARRAYS[1].gain`, and the driver's model starts at the least code that gives it (ccs `Gain::from_least`), so SET refuses 1024 (`[1]`, outside the range) as the list no longer offers it: Lumis's least ISO there becomes 100.7. Refused, never capped. Asked of the owner, as any floor is.
`Gain::split` also found a code a step low: the solution cut to a whole code and `analogue` cutting its gain made 1031 solve to code 6 (1030); it walks up now (host test `gain_models`).
A gain SET equal to the gain already set writes nothing (`Open::gain_code`, the init's code and digital gain first).

**Measured with the fix** (the slit, map only):
| gain asked | answer | slits a second, the sensor / the front end |
|---|---|---|
| 1024 | `[1]` refused | 801 / 794 (the init's G#3C0 kept) |
| 1031 (code 7) | `[0]` | 798 / 793, a slit every 1232 us |
| 2048, 2062 | `[0]` | 799 / 794, every 1258 us |
| 4096 | `[0]` | 799 / 794, every 1259 us |
| 8192 | `[0]` | 799 / 794, every 1259 us |
Regressions with this driver: the main's 64-row slit 1231 a second, clean; the tele's 557 a second, G#404 at frame 0 alone, as "Slits on every camera" measured.
**Stored** on the Pro: `camera` (capsule 555697 bytes, object 913e2b19b31cd716; the vault's one before, 530633 bytes, the cam-slits-all build, kept in target/agent-uwgain/camera-before.capsule.vsf), read back and compared equal; started on demand it said codes 7 to 1008, refused 1024, and ran the slit at 794 a second.

**The 8a.** Its main is the same IMX787 (`AKITA_ARRAYS[0]`, its gain words read alike), so its slits (740 a second) likely do the same at codes 0 and 1: unmeasured, its least left at 1024. Its ultrawide and front (IMX712) have no gain model in the list yet; when they stream, a cambench gain ladder from code 0 in a slit (`...010400...` with +31 `80`-steps) says whether they need a least of their own.

**Traps met on the way:**
- A hand-spawned driver without the stay byte (input `00`) ends at camtest's LIST, and camtest's OPEN then runs the vault's driver: every camtest run needs `01` and the driver killed after its own CLOSE.
- A slit opened within 3 minutes of a boot, or while rawbench writes, stopped at its first or 840th frame (LATE_CONFIG); cambench's 512 MiB slit ring, grown for the first SLITS, is the suspect, so a map-only run now keeps one slit.

### The tele's orientation (2026-11-19; settled below, "The tele flipped, stored")

The Lumis session: every tele readout stands mirrored left to right against the main (NCC 0.935 mirrored, 0.40 straight).
Android's HAL writes the tele's stream on as `01 00 01 02`, CCS image_orientation G#0101 = 2 (the vertical flip), at all three readouts (keys/husky/captures/ftrace-2026-11-18); ferros's is `01 00 01 00`, and no tele init writes G#0101.
The driver's fourteenth spawn byte writes the stream on with G#0101 0 (3) or 2 (4); camtest's cell table now also says the frame's mean in eight bands of rows and of columns.
Measured back to back on the 22-degree readout (one scene, minutes apart):
| G#0101 | cell rows 0 and 1 | bands of rows, top first | bands of columns, left first | mean |
|---|---|---|---|---|
| 0 | `156 114` / `400 155` | 266 370 270 75 67 66 65 65 | 150 166 174 176 170 157 138 116 | 156 |
| 2 | `395 156` / `156 114` | 75 78 82 85 131 692 1019 1004 | 445 441 426 409 388 367 345 319 | 387 |
- The sensor takes the bit (CAPS reads G#0101 = 2) and the rows turn over: the bright bands move to the bottom and the cell's rows swap, so the mosaic as ferros reads it goes from G B / R G to R G / G B, not the calibration's B G / G R.
- But the frame is not the same scene turned over: the column bands change shape and the bright part reads 2.5 times as bright at the same header exposure and gain. A vertical flip alone explains neither.
- So nothing is changed in the stored driver: Lumis's NCC on a frame with byte 14 = 4 (the driver hand-spawned) says whether that readout matches the main, and the 11-degree and 48.8 MP readouts are untried. Writing G#0101 = 2 before the init, or mid-stream in the hold, is untried (those runs went to the vault's driver, the trap above).
- (Later the same day, below: both "A vertical flip alone explains neither" points were camtest's, not the sensor's. Its bands and its mean read one place of the 2 x 2 cell, the place the flip turns from green to red.)

### The tele flipped, stored (2026-11-19, branch tele-orientation)

The Lumis session measured on the Pro with byte 14 = 4 (logs in the lumis-slit worktree's target/runs/tele): with G#0101 = 2 all three tele readouts match the main straight (22 degrees NCC 0.947, 48.8 MP 0.947, 11 degrees against 22 degrees 0.997); with 0 they read mirrored.
The tele's quarter turn puts the sensor's rows across the screen, so the sensor's vertical flip is the screen's left-right mirror.
The Pro b1003 (the flashed forge kernel), every driver hand-spawned from target/agent-teleorient/bin with the stay byte, one open each, every stream ended by its own close; logs in target/agent-teleorient/runs.

**The 2.5 times was camtest's sampling, not the sensor.**
camtest's bands of rows and columns read every 16th sample of every 16th line, and its "mean sample" every 65th byte of every 16th line: x and y always even, so always the cell's top left place, one colour.
Unflipped (G B / R G) that place is green; flipped (R G / G B) it is red.
The run of the table above read R 400, G 155, B 114 in its cell table: red over green is 2.5, the "2.5 times as bright".
The column profile changed because it was the red channel's profile against the green channel's.
camtest now reads whole 2 x 2 cells at all ten bits: its bands, an 8 x 8 grid of them and the frame's mean (also the view's MAP mean and the long-exposure ladder's view mean).

Measured with it, back to back, the 22-degree readout, the same scene (whatever was in view), the init's 32.56 ms and 16x on the first frame, then SET to 16 ms and 8x (camtest `08010000030403`, the driver `01` and 12 zero bytes then `0B` (0) or `0C` (2)):
| run | G#0101 | cell rows 0 and 1, first frame | whole cells: frame mean, first / at 16 ms 8x | bands of columns, first frame |
|---|---|---|---|---|
| a0 | 0 | `199 141` / `498 197` | 260 / 135 | 265 280 285 284 273 257 234 205 |
| a1 | 2 | `494 198` / `197 139` | 259 / 134 | 264 279 284 282 272 255 232 204 |
| a2 | 0 | `196 138` / `498 195` | 259 / 134 | |
| a3 | 2 | `494 195` / `194 137` | 257 / 134 | |
- The grid's rows turn over and nothing else: a0's top grid row `336 402 436 450 454 449 427 380` is a1's bottom `337 403 436 450 454 449 428 381`, and so on down; the columns keep their order and their values.
- Each colour, flipped against not: red 494 against 498, green 197 against 198, blue 139 against 140 (a0 to a3, the first frames): within 1 % of black-subtracted signal, as one or two rows of the window at a steep vertical gradient would make. No brightness difference to explain.
- **The sensor's words differ in two places only.** The driver's fourteenth byte with bit 3 reads G#0000 to G#10FF back after the first frame (`regs_say`); of its 32-byte rows, those that differ between two runs of the same flip (G#0440, 0580, 0680, 0760 to 07A0, 08C0, 09A0 to 09E0, 0BE0, 0D40, 0F80) are read noise of unimplemented space; between the flips only G#0101 (0 to 2) and G#0006 (0 to 2) differ. G#0006 is CCS pixel_order: the sensor names its order GRBG unflipped and BGGR flipped, the transpose of what ferros's cell tables read (G B / R G and R G / G B: red is the place a red-lit scene lights most), so the flip moves its own report the same way and no other register.

**Built:** ferros_cam cams `TELE.stream_on` = `01 00 01 02` (every tele readout: the 22 degrees, the 11 degrees and the 48.8 MP replay the camera's stream on, as does a slit and the exposure cut's stream on), `TELE.cfa` R G / G B; the driver's fourteenth byte keeps 3 (G#0101 = 0) and 4 (2) as an A/B, and bit 3 adds `regs_say`.
The cell tables with the stream on as built (the driver `01`, camtest `08010000030403` with kind 0, 8, 9 in the eighth byte; at 16 ms 8x):
| readout | cell table, rows 0 to 3 | mosaic |
|---|---|---|
| 22 degrees (the stored driver, on demand) | `257 94 257 93` / `93 79 93 79` / `257 93 257 93` / `93 79 93 79` | R G / G B |
| 11 degrees (kind 8) | `309 103 308 102` / `102 83 102 85` / `306 102 309 102` / `102 84 103 82` | R G / G B (remosaiced Bayer) |
| 48.8 MP (kind 9) | `258 255 95 93` / `254 253 93 94` / `92 93 80 78` / `93 94 79 78` | the quad, blocks R G / G B |

**Corrected 2026-11-22** ("Focus calibration and the rear mosaics"): read with blue as each table's brightest place, all three are B G / G R (the quad's blocks too), as the sensor's own pixel_order (G#0006 = 2, BGGR) said here; the list carried R G / G B until then.
Each link clean (+14 G#0, +1C G#0), no error word.
- An aside, not chased: at the same header exposure and gain the 48.8 MP reads about 2.8 times the 22 degrees' signal over black (whole-cell means 257 and 133 at 16 ms 8x). Two readouts with their own inits, so their own analogue settings; Lumis's NCC does not see it.

**The slit** (cambench `03034000000000000000000002800000`, the 64-row binned slit, 2048 slits added into u16): 557 a second (the sensor 559, the front end 555), none lost to the ring, none read over; G#404 at frame 0 alone and the link's +1C G#40, as "Slits on every camera" measured. CAPS: y G#BA0 to G#C1F, the window symmetric about the array's middle, so the flip reads the same rows.

**Stored** on the Pro: `camera` (capsule 560345 bytes, blake3 32829448fc96da9c; the vault's one before, 555697 bytes, the uw-gain-slit build, kept in target/agent-teleorient/camera-before.capsule.vsf), read back and compared equal; started on demand (camtest alone) it read the 22 degrees R G / G B, the link clean.

## The four cameras for the promo (2026-11-21 night)

The owner: Lumis on the Pro shoots RAW video on all four cameras with no dropped frames, focus and ISO adjustable on each ("function over form").
Measured on the Pro (flashed 82b908c4, then that kernel with a9bdb503 RAM-reloaded), Photon and Lumis stopped (`diag kill`), the stored driver from 117fadb5 on; tools: rawrec job 8 (the stream alone, +22 the camera), job 0 (a recording on the GPU coder, straight to the flash), job 9 (the list), camtest 3 (gain), 13 (focus), 7 (PROBE), 11 (the ring on the GPU).

| | main | ultrawide | front | tele |
|---|---|---|---|---|
| binned readout | 4080 x 3072 | 4000 x 3000 | 3840 x 2736 | 4032 x 3024 |
| line (list `line_ps`) | 3.149 us | 6.562 us | 10.438 us | 6.107 us |
| fastest whole rate | 60 | 48 | 33 | 50 (its sensor pads past 128 blank lines) |
| stream alone, 64 s at 30/s | 1920, 0 skipped | 1921, 0 | 1920, 0 | 1922, 0 |
| stream alone, 64 s at its fastest | 3838 at 60, 0 | 3073 at 48, 0 | 2112 at 33, 0 | 3210 asked 51 (50.0 a second), 0 |
| recording, 32 s at 30/s | 960, 0 dropped, 64 C (the screen-off run) | 960, 0 dropped, 72 C | 960, 0, 76 C | 960, 0, 73 C |
| recording at its fastest | 60: 1741, 0 dropped, the 85 C guard at 29 s | 48: 1105, 0, 85 C at 23 s | 33: 1056, 0, 78 C | 50: 1600, 0, 66 C |
| GPU coder a frame | 6.2 ms | 7.0 | 6.1 | 6.7 |
| gain (SET 1, 1024 one) | 1x to 160x, the mean rising each half stop | 1.007x to 64x (1024 refused: the stored least, code 7, not approved by the owner) | 1x to 16x | 1x to 64x |
| focus (SET 2) | G#0800 to G#7800 (as before) | G#0800 to G#7800 since its LV buck: actuator G#0F and EEPROM G#50 answer, the lens moves | G#0800 to G#7800: its actuator G#0C reads back every target within G#64 | none: the lens is the OIS controller's (humbaba, G#61, 16-bit registers), undecoded |

- **Source digests at 60/s**: rawrec's default hashes every frame as captured; with them the main at 60/s dropped 12 frames a second (the hand-over 20 ms, past 16.7). Without (Lumis's own path), none. A 60/s clip must be recorded without them.
- **Heat** is what ends a 48 to 60 a second recording: 85 C in 23 to 29 s from 54 C, the charger in. 24 and 30 a second ran their 32 s under 78 C.
- **The screen off** put the clock policy in its dark tier (the worker cores stopped, the GPU at its 150 MHz floor) under a recording: the 30/s main fell to 5 to 23 frames a second (the stream itself slowed, so none counted as dropped). a9bdb503: no dark tier while the camera's switch is held or a clip's extent is open. RAM-reloaded: the tier stayed active, 960 of 960, 0 dropped thru the screen off and on.
- **A SET before the first frame** (the front's trap, "The other cameras on the Pro") is now held by the driver until a frame has landed, then served: a client may set its shutter, gain or focus right after its OPEN on any camera. The front with a 262 ms SET before its first frame then streamed, gain 1x to 16x.
- **The tele's floor**: asked 3211 lines (51 a second) its frames came 19.95 ms apart (about 3267 lines); 3275 (50 a second) ran exact. `Cam::blank_least` 250 makes the list's `fll_least` 3274, so its fastest is 50.
- **camtest 11 after END_FLUSH** (1786ed7b removed the camera's own per-job invalidate): every word of every frame equal to the CPU's view, non-cacheable and cacheable; a frame's copy 949 us non-cacheable, 1189 us cacheable (849 before); a store into the ring a permission fault; a CLOSE with a job in flight ended its queue; unmapped after; the GPU's fence clean.
- **The clips window** had 16 GB free of 48 (other domains' clips): a minute at 60/s is about 17 GB coded.
- **Open**: the tele's focus (a capture of Android's camera focusing the tele, `diag android` and the i2c tracepoints on i2c-4 at every address, as "The I2C side without a flash"); each lens's infinity and nearest targets (the EEPROMs' calibration, undecoded), so a slider in dioptres maps by measure, not by guess.
- **The Pro went silent** after a RAM reload of forge 8f04f065 (23:43:56: no USB, no fastboot, not back on its own); the same night's reload of 82b908c4 with a9bdb503 alone came back in a second and ran every test above. Between the two, the kernel's changes were GPU-COMPOSE step 1 (727a23db; e4a30393 since). Both slots still hold the flashed boot_restarts_82b908c4, and the stored `camera` (65348f7a, blake3 2e34495d...) runs on it, so a power-button reset brings back a healthy Pro, but without a9bdb503: a RAW recording with the screen off still falls to the dark tier until a kernel with it is flashed.

### For the owner (the morning of 2026-11-22)

- **Promo-ready on the ferros side**: all four cameras stream and record RAW on the GPU coder with no dropped frame at 24 and 30 a second; gain (ISO) works on all four; focus on the main, the ultrawide and the front.
- **Shoot settings**: 30 a second on all four (the front's fastest whole frame is 33, the ultrawide's 48, the tele's 50, the main's 60); the top rates for short takes only, from a cool start (85 C in 23 to 29 s at 48 to 60 a second with the charger in).
- **Needs your hand**: a power-button reset of the Pro, then the flash of the night's kernel (a9bdb503's screen-off fix) once GPU-COMPOSE step 1 is fixed or left out.
- **Needs your call**: the ultrawide's least gain at code 7 (1031, 1024 refused): a floor stored since uw-gain-slit that you never approved (left as it is tonight).
- **Not done**: the tele's focus (its lens is driven by its OIS controller, humbaba at G#61; a capture of Android's camera focusing the tele is the way in); each lens's infinity and nearest targets, so Lumis's dioptre slider maps by measure.
- **Front focus**: confirmed by the actuator's own position readback (every target G#0800 to G#7800 read back within G#64); the frames of the night's scene were too featureless to show the sharpness move.

## Focus calibration and the rear mosaics (2026-11-22, no device)

The owner, shooting Lumis on the Pro: the front's focus slider ran from about infinity to "way past infinity" with nothing near in focus, real infinity at its far end; the ultrawide's colour looked like "a Bayer pattern whoopsie, b-r maybe" and the tele "def appears to be an r-b swap".
Settled at the desk from what already existed; the device checks below confirm it.

### The factory's focus calibration

The Pro's persist partition holds each module's factory AF calibration, written at manufacture: `/camera/AF_calibration.cal` (read from the 2026-11-18 partition dump in keys/husky with tools/f2fs-cat; one unit, as the IDTs are).
Its infinity and near ("macro") codes, the HAL's own, and as targets for the actuator (ferros_cam cams `FocusCal`, the list's `focus_infinity` and `focus_near`):

| camera | infinity | near | target at infinity | target at the near point | past infinity (to G#0000) |
|---|---|---|---|---|---|
| main ("rear wide") | 509 | 3274 | G#1FD0 | G#CCA0 | 509 codes, 0.18 times the near point's dioptres |
| ultrawide ("rear uw") | 883 | 3224 | G#3730 | G#C980 | 883 codes, 0.38 times |
| front ("front normal") | 1642 | 3072 | G#66A0 | G#C000 | 1642 codes, 1.15 times |
| tele ("rear tele") | 7050 | 13538 | (humbaba's units) | | not offered: its lens is the OIS controller's (G#61), undecoded |

Larger is nearer on all four.
The near point's distance is not in the file and the Pro's LENS_INFO_MINIMUM_FOCUS_DISTANCE was never recorded (the 8a's main reports 10.204 dioptres, 98 mm), so the list carries no `focus_near_mm` yet; at 10 dioptres the main runs 1.8 dioptres past infinity, the ultrawide 3.8, the front 11.5.

**The target is the code times 2^4** on the three cornerfolk actuators (the main's G#0C, the ultrawide's G#0F, the front's G#0C): a 12-bit code left-aligned in the 16-bit target (00/01).
Two observations say so, and none says otherwise:
- The main's sharpest over a floor about a metre away was G#3800 (docs/oracle/CAMERA.md, 2026-10-21): code 896, a seventh of the way from 509 to 3274, about 1.4 dioptres at a 10 cm near point. At 2^3 it would be code 1792, about 4.7 dioptres (21 cm).
- The owner's front slider covered G#0800 to G#7800, codes 128 to 1920 at 2^4: from far past the front's infinity (1642) to just past it, nothing near. At 2^3 the front's infinity (G#3350) and near point (G#6000) would both lie inside that span and the slider would have focused near.
- Android's two captured main targets (G#5D10 at the open, G#5120 at the boot probe) both end in a zero nibble.

**The list now offers each lens's whole code range** (`focus_min` G#0000 to `focus_max` G#FFF0, ferros_cam cams `CORNERFOLK_TRAVEL`): the old G#0800 to G#7800 (the main's sweep span, copied to the others) held no camera's near point, and SET refuses a target outside the list's range.
- **Since 2026-11-22 night the travel offered is the span tracked on the Pro, G#0100 to G#FF00** (`CORNERFOLK_TRAVEL`): every target from G#0100 to G#FF00 read back within G#24 on all three ("Focus calibration and the rear mosaics" travel check, b1134); G#0000 and past G#FF00 were never sent, and a lens driven past its stop can hold its coil at full current, so they are not offered.
Past G#7800 is unmeasured: the front's position (G#84) read back every target of G#0800 to G#7800 within G#64 (2026-11-21) and the main's within 30 ms (2026-10-21).
**Each open now starts at the infinity target** where the calibration is known (payloads/camera; it was G#3800 on every camera, about 750 codes past the front's infinity, so the front's first frames were blurred until a client set its focus).

### The ultrawide and the tele are B G / G R

The list said R G / G B for both since 2026-11-18 ("Every readout, captured", "The tele flipped, stored"), from cell tables that took a scene's brightest place for red in "the owner's red-lit room".
The rear cameras saw that room's blue light, so the brightest place was blue:
- The main, whose G B / R G is measured in true colour, read its blue place brightest there the same day (`162 181` / `104 162`, the main's cell table of "The shift's ceiling"); the front, facing the other way, read its red place brightest in the same session as the ultrawide's and tele's tables (`72 83` / `69 72`, G R / B G as its list).
- The tele's own CCS pixel_order (G#0006), read back on ferros after the first frame (`regs_say`, "The tele flipped, stored"), says BGGR with G#0101 = 2 and GRBG with 0: the red-blue swap of each cell table as it was read, in both flips.
- Both calibration captures say B G / G R (the tele's with Android writing the same G#0101 = 2), and in each the channel labelled red peaks at 604 nm in the response solved from it (ultrawide 604, 544, 468 nm; tele 604, 556, 464 nm): Android's frames were B G / G R as labelled.
- The ultrawide's IMX787 is R G / G B itself (Camera2's RGGB on the 8a, whose init writes no flip); the Pro's init flips it both ways (G#0101 = 3) on Android and on ferros alike: B G / G R.
- ferros's own ultrawide frames of 2026-10-22 (target/cam/uw1.raw10, uw4.raw10, the same init and job) develop the wooden floor tan with B G / G R and blue-grey with R G / G B.
- The owner saw red and blue swapped on exactly these two cameras.

Every cell table above reads consistently once its brightest place is blue: the tele's 22 degrees G R / B G unflipped and B G / G R flipped, its 11 degrees the same, its 48.8 MP quad's blocks the same; the ultrawide's 12 MP and 48 MP B G / G R.
**Changed:** ferros_cam cams `ULTRAWIDE.cfa` and `TELE.cfa` B G / G R (every readout of each takes its camera's).
The IDTs are each camera's own (their matrices differ, their channels peak in order).
**Lesson for the method:** a cell table names a colour only under light of a known colour and with a control camera beside it; the brightest place in a room is whatever its lights are.

### Device checks (the Pro, for the next slot)

- **Travel** (headless): on the main, the ultrawide and the front, SET 2 to G#0000, G#7800, the near target, G#FFF0, then G#0000 again; the position (G#84) read back 30 ms after each should be within G#64 of the target.
- **Infinity** (the owner aiming at a target 50 m or more away): sweep the infinity target plus and minus G#800 in G#80 steps, the camtest focus job's sharpness (byte 13); its peak should sit within about G#100 of the factory's infinity.
- **Near** (a high-contrast target at a measured 100 mm from the lens): sweep the near target plus and minus G#1000 in G#100 steps for the sharpest target P; the near point is then 10 (near - infinity) / (P - infinity) dioptres, which gives `focus_near_mm`.
- **The mosaics** (Lumis DEV_CELLS, knobs4 bit 0, a saturated red card under white light, all four cameras, the main and the front as the controls): red brightest at (1, 0) on the main, (0, 1) on the front, (1, 1) on the ultrawide and the tele; a blue card at (0, 1), (1, 0), (0, 0) and (0, 0).
- **The tele's focus**: an Android i2c trace on its bus with G#61 in the address filter (the 2026-11-18 trace kept the sensors' addresses alone), tapping focus near and far; its writes should run between the factory's 7050 and 13538 in some scale.

## Two-row slits, 2:1 and 1:1 (2026-11-22, desk)

The owner, via the Lumis session: "We should be able to pull 10k lines per second, if we can do 4k full frame video at 60fps"; "ideally we read out those two rows only, in 1:1 and 2:1 mode"; at 1:1, of the quad's R R G G / R R G G / G G B B / G G B B "we only pull" the second and the third rows; "So if the sensor can let us do 2 rows, that's ideal. SSPPEEDD".
Measured the same day with Lumis 0.2.57 on the flashed kernel: the tele's slit about 63 a second, the main's about 745; Lumis keeps 2 rows a slit.

### The tele's 63 a second was Lumis's shutter

The driver's console of that run (target/farm/lumis-target/slit1.txt):
- `mode A#3: A#4032 x A#64 (... y G#BA0 to G#C1F ...), frame A#192 lines ... written after the first frame`, then `the mode written mid-stream after A#1 frames`: the slit's window and 192-line frame reach the tele after its first frame (`Cam::mode_live`), as on 2026-11-18 night when the same slit ran 557 a second.
- Then `exposure: exposure A#15621 us: frame A#2562 lines`: Lumis carried its stills' 15.6 ms shutter into the slit, and the exposure's plan (ferros_cam ccs `Timing::plan`) lengthened every slit to hold it: 2562 lines of 6.1068 us, 64 a second.
- A slit runs at its own frame only while the integration fits it, the frame less the margin (tele 4 lines, main 2, ultrawide 64). The main's 745 a second (1.34 ms) reads the same way, unconfirmed (its log was not kept).
- For Lumis: open a slit at an integration inside its frame (a few hundred microseconds), or show that the shutter sets the slit rate.

### What caps a slit: the sensor's least frame

- **The main runs no frame shorter than about 262 lines (825 us), whatever the rows**: 4 rows asked 260, 132, 68 and 36 lines all ran 1199 a second; 64 rows asked 192 ran 1218, drained 1230 ("The slit for real", "The slit runs").
- **The tele's least frame is about 294 lines (1.79 ms)**: 192 and 256 asked both ran 558 ("Slits on every camera").
- In vt clocks the two are nearly equal (main 262 x 6320 = 1.656 million, tele 294 x 5568 = 1.637 million, both Samsung): a line count or a clock count, the device check below says which. Nothing captured names it: Android never runs such short frames, and the main's CCS limit block (G#1100) aliases.
- **The ultrawide and the front ran the 192 lines asked** (794 and 499 a second): their floors are under 192 lines, unmeasured.
- **The front end and the PDMA are not the cap.** Every slit the sensor sent landed: 261120 drained at 1230 a second, none lost or read over, every DMA_DONE released by the kernel's armed action at the interrupt, the frame thread 82 us an answer. A descriptor must be released inside the blanking: at 2 rows the main's blanking is about 260 lines, 819 us, where the slim program fits the 403 us of a 64-row slit at 1230 a second. A buffer of 2 rows is 12 KiB (`job::slit_bytes`, 2 lines of G#1400 and the 64-byte record on a page); the PDMA takes it as any other.
- **So packing several slits into one DMA frame would buy nothing**: each slit is one sensor frame, and the sensor's frame is the floor, not the per-frame cost. It would matter only if the floor were lifted below about 100 us.

### Two rows at 2:1

- A binned slit of 2 rows is one Bayer pair, period-aligned, the readout's own colours (ferros_cam ccs `Mode::slit`, host-tested).
- **The main refused 8 and 4 rows** (loch0 G#404, SENSOR_RESOL_MISMATCH and STREAM_ERROR: the sensor sent other than asked); 16 to 56 streamed with ERR_CRC_PH on the link, 64 clean. So its least window is between 9 and 16 rows, and 2 is expected to fail; the other three are unmeasured.
- **If a sensor will not send 2 rows, the fastest way to deliver exactly the 2** is its least window with the 2 in it: the frame floor is the same, so the rate is the same (16 rows of the main's 3.149 us lines are 50 us of its 825), and the client keeps 2 rows of each slit.

### Two rows at 1:1

- **The 1:1 readouts are quad on the main, the ultrawide (48 MP) and the tele (48.8 MP)**; the front's own readout is plain Bayer at 1:1, so 2 rows there are a Bayer pair.
- **The owner's slit straddles the quad's halves**: rows 1 and 2 of each 4-row period, one row of the top half's colours over one of the bottom half's, all three colours in 2 rows, the mosaic 4 across and 2 down. Rows 1 and 2 of a period read back as rows 2 and 1 under a vertical flip, and each captured window holds whole periods, so the same window gives the period's rows 1 and 2 in the frame as read, flipped or not (the ultrawide's G#0101 = 3, the tele's 2, the main's 3).
- **Its colours as read** (`Cam::cfa`'s blocks, each one row tall): the main `G G B B` over `R R G G`; the ultrawide and the tele `B B G G` over `G G R R`. The list carries them as `cfa` (the blocks, as now) with `slit_cfa_period` 4 across and `slit_cfa_period_y` 2 down; each frame's header says its periods at +14 (across) and +15 (down).
- **Remosaic** (plain Bayer at 1:1): the tele has one, its 11-degree readout (the array's middle 4032 x 3024 at 1:1, a Bayer mosaic, its line 9.8712 us against the 22 degrees' 6.1068 for the same 4032 columns); a remosaic of the tele's whole width is not captured (its 48.8 MP readout is quad). The main's 1:1 init is quad and no remosaic word is known; the ultrawide's IMX787 likely has Sony's on-chip remosaic, but no captured readout uses it. So a full-width 1:1 slit is the straddle on all three.
- **The 1:1 row comes as one line**: the main's 8160 RAW10 a line, 24 a second clean thru loch0 with the link at 1.49 GB/s (2026-11-18 evening). Its line (11600 clocks at 1761 MHz, 6.586 us) is the link's at full width; a slit's few rows do not load the link, but each line still crosses it at the lane rate, so the line stays near 6.5 us.
- The straddle needs the sensor to start its window one row into a quad: on the main that is 2 addresses (its addresses run 2 a pixel), on the tele and the ultrawide 1, an odd address, which a sensor may refuse or round. The device check below reads the window back.

### What each camera reaches with 2-row slits

Lines a second is 2 times slits a second. Measured where marked; the rest from each line time and the floor as noted.

| camera, mode | line | frame of the slit | slits a second | lines a second |
|---|---|---|---|---|
| main 2:1 | 3.1489 us | its floor, about 262 lines | 1212 to 1230, measured | 2.4k to 2.5k |
| main 2:1, line at 5888 clocks (the least the link takes at full width) | 2.934 us | 262 lines, if the floor is lines | 1301 | 2.6k |
| main 1:1, straddle | 6.586 us | 262 lines (lines), or 1.656 million clocks at 1761 MHz (clocks) | 579 (lines), 1064 (clocks) | 1.2k to 2.1k |
| ultrawide 2:1 | 6.5616 us | 192 asked, measured; its floor under it | 794 measured; 1588 at 96 lines | 1.6k (3.2k at 96) |
| ultrawide 1:1, straddle | 5.4067 us | 192 lines, if its floor in lines holds | 963 | 1.9k |
| front (its readout is its 1:1) | 10.4378 us | 192 asked, measured; its floor under it | 499 measured; 998 at 96 lines | 1.0k (2.0k at 96) |
| tele 2:1 | 6.1068 us | its floor, about 294 lines | 557, measured | 1.1k |
| tele 1:1, 48.8 MP straddle | 13.2008 us | 294 lines (lines) | 258 | 0.5k |
| tele 1:1, 11 degrees (remosaiced Bayer pair) | 9.8712 us | 294 lines (lines) | 344 | 0.7k |

- **10k lines a second from 2 rows needs 5000 frames a second, 200 us a frame**: four times shorter than the main's 825 us floor, the shortest measured. Not reachable on any of the four unless the floor is a register that can be set.
- 4k at 60 a second is 130k rows a second, but each row is read once a frame; the sensors' readout (the main's 383k rows a second binned, "The line, toward a million") is not their frame rate, and a slit is one frame a sample.
- Rows kept from each slit raise the lines a second at the same frame rate: the main's 1230 slits of 10 rows are 12.3k lines a second, neighbouring lines of the scene read a few microseconds apart (a strip stitched at the subject's speed, Lumis's to do).

### Staged (not committed; needs the device): target/farm/camera-work/slit-2row.diff

- ferros_cam ccs: `Mode::slit` takes 2 rows at 1:1 on a quad array, one row into its period (`Mode::straddles`), and `Mode::periods` gives (2, 2) binned, (4, 4) quad, (4, 2) the straddle; host tests on the main's binned and 1:1 inits.
- ferros_cam cams: `Cam::slit_least`, the measured floors (main 262, tele 294; the ultrawide and the front 0, unmeasured).
- ferros_cam list: every section's `cfa_period_y`; the binned and 1:1 sections' `slit_kind` (3, 4), `slit_rows_least` 2, `slit_cfa_period` and `slit_cfa_period_y`; `slit_lines_least` where measured.
- payloads/camera: the header's +15, the period down.

### Device checks (no owner needed; hand-spawned driver and cambench, nothing stored)

- **Least rows, each camera**: binned slits of 2, 4, 8, 12 and 16 rows; at each, the link's last packet header (PHY lane +B4 to +D0: data type and word count) and its line-end word, so a refused height says what the sensor sent.
- **Least frame, each camera**: at its least rows, the frame asked 192, 160, 128, 96, 64 lines and down to rows plus 2^4; the sensor's counter against the front end's rate (cambench's frame sweep). Each floor goes into `Cam::slit_least`.
- **Lines or clocks**: the main's slit with its line at 5888 clocks, the frame 192 asked: about 769 us (1301 a second) if the floor is 262 lines, 825 us (1212) if it is about 1.65 million clocks.
- **The 1:1 straddle**: kind 4, 2 rows, on the main, the ultrawide and the tele; CAPS reads the window back (y start one row into a period), and Lumis's DEV_CELLS on the red card names the two rows' colours (expected above).
- **The front end at the floor**: every run above says its DMA_DONEs released by the kernel and its error words; any LATE_CONFIG at 2 rows would name the per-frame cost.

### On the Pro, 2026-11-22 evening (b1134, the flashed e5f4da21, nothing stored, no reload; every driver hand-spawned from target/farm/camera-work/kit, forge 325e9e14 plus slit-2row.diff, one open each, every stream ended by its own close; logs in kit/runs/all)

**Least rows** (binned slits, the frame rows plus 2^7 asked; sensor's counter / front end, a second):
| rows | main | ultrawide | front | tele |
|---|---|---|---|---|
| 16 | 1241 / 1229 | 1017 / 1058 | 668 / 665 | 669 / 0 |
| 12 | 1231 / 0 | 1013 / 1087 | 689 / 683 | 678 / 0 |
| 8 | 1231 / 0 | 1007 / 1120 | 708 / 704 | 690 / 0 |
| 4 | 1231 / 0 | 1001 / 1154 | 730 / 725 | 690 / 0 |
| 2 | 1231 / 0 | 1000 / 0 | 742 / 0 | 708 / 0 |
- **Every sensor sends 2-row frames**: its own counter runs at every height. What stops them is the receive side.
- The Samsung parts (main, tele): the link reports ERR_CRC_PH (+14 G#20) and loch0 SENSOR_RESOL_MISMATCH (G#404) at the first frame; the main's least is 16 rows (12 and under no frame), the tele's over 16 (64 runs, 559 a second).
- The ultrawide streams 4 rows (2 rows: link +1C G#40, loch0 G#414); the front streams 4 rows (2 rows: loch0 G#4040000).
- The ultrawide's frame counter reads about 1000 a second wherever its front end runs faster; its front end's rate is the one that matches the frame asked (128 lines of 6.5616 us: 1190).

**Least frame** (the least rows that streamed; the frame asked 192 down to 32):
- **Main, 16 rows**: 1231 to 1242 a second at every frame asked (812 us): its floor.
- **Ultrawide, 4 rows**: 794 at 192 lines, 952 at 160, **1190 at 128** (840 us); at 96 the link takes clean frame starts and ends (+24 = +2C = G#3, no error) but loch0 starts none, and at 80 and under nothing streams. So its least is between 96 and 128 lines: the front end's or the stream's start, not isolated.
- **Front, 4 rows**: 504 at 192, 598 at 160, 747 at 128, **791 at 96 and every frame under it**: its sensor's floor, about 120 lines of 10.4378 us (1.25 ms).
- **Tele, 64 rows**: 555 to 559 a second at every frame asked: its floor, about 293 lines (1.79 ms). With fewer rows its sensor runs faster (669 at 16 rows, 708 at 2: about rows plus 229 lines) but no frame lands.

**Lines or clocks** (the main, 64 rows, frame 192 asked): line 6320 clocks 1222 a second (818 us), line 5888 clocks (read back as 5888) 1244 (804 us), where a floor of lines would give 1321. **The main's floor is a time, about 1.6 million clocks (800 to 820 us), not a line count**: a shorter line does not raise the slit rate. At 5888 the link also reports ERR_CRC_PH (+14 G#20) while frames land.

**The 1:1 straddle** (the main, kind 4): the sensor took the window as built, y G#1812 to G#1815 (2 addresses, one row, into the period that G#1810 starts), 8160 x 2, its frame 130 lines of 11600 clocks (CAPS and the sensor's words read back), and ran 832 a second on its own counter. No frame reached memory: the link ERR_CRC_PH (+14 G#20), as at 4 rows (y G#1810 to G#1817, the same). The straddle's addresses are right; the 1:1 slit's receive side is the main's binned one's.

**What reaches memory today, exactly the 2 wanted rows kept of the least window that streams:**
| camera | window | slits a second | lines a second |
|---|---|---|---|
| main 2:1 | 16 rows | 1229 to 1241 | 2.5k |
| ultrawide 2:1 | 4 rows, 128-line frame | 1190 | 2.4k |
| front | 4 rows, its floor | 791 | 1.6k |
| tele 2:1 | 64 rows (16 and under refused) | 559 | 1.1k |
| main 1:1 | none (4 and 2 rows refused) | | |

**The blocker for 2 rows on the Samsung parts is the receive side** (ERR_CRC_PH from 56 rows down on the main, every short slit on the tele, both 1:1 slits), not the sensors, the per-frame cost or the PDMA. Next, at the desk: the link's channel sizes (+44/+54/+64) and the embedded-data channel 2, which stays on in every slit ("The slit for real"), against what a 2-row frame carries; then the uw's 96-line start.

### The receive side of short slits, read from the runs (2026-11-22, desk)

The CSI link's interrupt words, named from the LWIS tables (zuma-isp-event-info.dtsi, isp_fe.h `ISP_FE_CSIS_INT0_*`, `INT1_*`): +G#14 bit 5 ERR_CRC_PH, bits 12 to 23 the lanes' invalid codes and start-of-transmission errors (none seen); +G#1C bit 0 LINE_END, 1 FRAME_END, 2 FRAME_START, 3 ERR_LOST_FE, **4 ERR_LOST_FS (G#10)**, 5 HRESOL_MISMATCH, **6 VRESOL_MISMATCH (G#40)**. The channel words: +G#40/+G#44 channel 0 (the image: G#30AC, then height << 16 | width), +G#50/+G#54 channel 1 (the PD rows), +G#60/+G#64 channel 2 (embedded data, G#B048, 4 lines).

- **The ultrawide and the front at 2 rows: the PD channel programmed for 0 lines.** Neither sensor's phase detect is switched off in a slit (ccs `Side` none for the Sony; none known for the front), so channel 1 carries PD rows (+24 G#3 on the ultrawide: channels 0 and 1 start). geom sized it a quarter of the rows: 4 rows gave 1 line (G#10FA0) and streamed, 2 rows gave **0 lines (G#0FA0, G#0F00)** and the link reported VRESOL_MISMATCH (+1C G#40), loch0 G#414 or G#4040000, nothing landed. Staged: the quarter rounded up (geom `Geom::of`, target/farm/camera-work/receive-side.diff), every captured height a multiple of 4 and so unchanged.
- **The tele: its PD channel stays at the captured 2992 x 504 in every slit** (+G#54 G#BB001F8: its PD rows are not a quarter of its 3024, so geom never matches them), no PD switch is known for it, and the link reports VRESOL_MISMATCH at every slit; at 64 rows frames land anyway (the mismatch at frame 0 alone), at 16 and under loch0 G#404 and nothing lands. Its PD control is likely G#0D00/G#0D02 (= G#0101 in its init: "The slit for real"), unwired. Staged: a device knob, the driver's fifteenth spawn byte, writes channel 1's height after the receiver starts.
- **The main at 12 rows and under: no frame end ever reaches the link.** Frame starts on channels 0 and 2 (+24 G#5) and none ending (+2C G#0), with ERR_CRC_PH (+14 G#20); at 16 rows the ends arrive (+2C G#5) beside ERR_CRC_PH and ERR_LOST_FS (+1C G#10), and frames land; at 64 the link is clean. Its PD is off at the sensor (G#0720 = 0) and channel 1 starts nothing; its embedded channel 2 (4 lines of 4080) stays on whatever G#0118 is written. A header in error is a frame end lost, so the CRC errors and the missing ends read as one fault, the short frame's packets themselves; not settled at the desk.
- **The ultrawide's 96-line frame**: the link took clean starts and ends on both channels (+14 G#0, +1C G#0, +24 = +2C = G#3) but counted one frame (its per-channel words G#14100 against G#8B4100 at 128 lines), and loch0 started none, no error word anywhere: the link stopped counting after the first frame, the receiver's own minimum blanking or the PD rows overrunning a short frame, not separated.

**Device checks for the next slot** (headless; target/farm/camera-work/receive-side.diff hand-spawned, nothing stored):
- The ultrawide and the front at 2 rows with the PD channel at 1 line (the staged geom): expected to stream as 4 rows did.
- The tele at 16, 8, 4 and 2 rows with channel 1's height asked 2, 4, 8, 16 and the rows themselves (the fifteenth byte), and once with G#0D00 = 0 in its hold: which height its sensor sends, or whether its PD turns off.
- The main at 8 and 2 rows with embedded data's other candidates off (its init's G#0118 alone does not stop channel 2), and with the frame left at the captured size (`F_KEEP_FE`, the front end whole) to see in memory how many lines a short frame carries.
- The ultrawide at fll 96, 104, 112, 120: where the link stops counting.

### The receive-side checks on the Pro (2026-11-22 evening, b1134, the flashed e5f4da21; camera-receive.bin hand-spawned, then the camera stored)

- **The front's 2-row slit lands** with its PD channel at 1 line (geom's quarter rounded up, 7acde33f): 128 slits at 747 a second, none lost, 12 KiB buffers (3840 x 2 at G#12C0). Before, 0 lines and nothing.
- **The ultrawide at 2 rows still refuses**: its PD channel now 1 line, but no PD rows start (+24 G#1, channel 0 alone) and the image channel reports VRESOL_MISMATCH (+1C G#40), loch0 G#414: the Sony sends another line count than its 2-row window, not found.
- **The ultrawide's least frame at 4 rows is 104 lines**: 1269 a second at 120, 1360 at 112, **1465 at 104 (1483 drained, none lost, no error word)**, nothing at 96. In the list since 7acde33f.
- **The tele at 16, 8, 4 and 2 rows refuses at every PD channel height tried** (1, 2, 4, 8, 16 lines, the driver's fifteenth spawn byte, read back as written): ERR_CRC_PH and VRESOL_MISMATCH each time, so its PD channel is not the only fault; its PD switch (G#0D00) is the next try.
- **The main at 8 and 2 rows with its PD and embedded data left on** (`F_LOUD`) refuses as quiet does: the side channels are not what stops its short frames.
- **Stored**: `camera` from 7acde33f (566689 bytes, blake3 663d5577...), read back equal, restarted idle; the one before (564929 bytes, blake3 9ddd2682..., 7617f691) kept in target/farm/camera-work/out/camera-before-7acde33f.capsule.vsf. The camera's interface with the kernel is e5f4da21's (ferros_sys since adds only calls it does not make).

### Lumis's slits ran the driver's default frame, not the floor (2026-11-22, desk)

The Lumis session's runs on the 7acde33f driver (Lumis 0.2.60, DEV_SLIT_LATER, 16 s): the ultrawide's 4000 x 4 slits every 866.128 us (1155 a second), the front's 3840 x 2 at 737 a second.
- **Both are the driver's default frame, rows plus 2^7** (payloads/camera `mode_of`: an OPEN's `fll` 0 gives `rows + BLANK`): the ultrawide's 132 lines of 6.5616 us are 866.13 us, the sensor's 866.128; the front's 130 lines of 10.4378 us are 1.357 ms, 737 a second. Neither the exposure (the driver's 16 lines fit both frames with their margins) nor a rounding.
- The list's `slit_lines_least` (the ultrawide 104, the front 122) is the frame to ask: an OPEN with `fll` = `slit_lines_least` and `rows` = `slit_rows_least` runs at the floor (the ultrawide 104 lines measured at 4 rows, 1465 a second; the front's 122 from 791 a second at 4 rows and 96-line frames). No driver change is needed for that.
- The driver's default stays rows plus 2^7: the floors were measured at one height each, and the ultrawide's 104 may be a blanking (rows plus 100) or a whole frame; the main's and the tele's floors are times the sensor pads to whatever is asked. A default from the floor waits for that answer.
- **Device check**: the ultrawide at 64 rows with frames of 164, 150, 132 and 120 lines, and the front at 2 rows with 122, 110 and 96: whether a floor is a blanking past the rows or a whole frame, so the list can say the least frame for any rows.

### The tele's phase detect off (staged)

The tele's init writes G#0D00 = G#0101 and G#0D02 = G#0101 and nothing else PD-shaped (its 22-degree and 48.8 MP inits differ only in the vendor's G#0B30, the frame, the integration and two vendor tables). Staged as target/farm/camera-work/tele-pd-off.diff: its `Side::pd` G#0D00, written 0 in a windowed mode's hold as the main's G#0720 is. Device check: the tele at 64, 16, 8, 4 and 2 rows with it; its link should then start no channel 1 (+24 without bit 1).

### The main's short frames (desk, not settled)

At 12 rows and under the link takes frame starts on channels 0 and 2 and never a frame end, with ERR_CRC_PH; its PD is off and channel 1 silent; its side channels on or off change nothing (F_LOUD, 2026-11-22). Channel 2 (data type G#12, embedded) stays on whatever G#0118 is: its 4 lines and the image's few may be what the link reads wrong. Device check: F_KEEP_FE (the front end left at the whole frame) at 8 rows, to see in memory how many lines and of what a short frame carries, and the link's PHY header words at the stop.

### The floors' shape, the tele's PD off, the main's short frame (the Pro, 2026-11-22 night, the flashed 07f922ca kernel)

Hand-spawned from forge (7acde33f's driver, and tele-pd-off.diff for the tele), headless, every stream closed by the driver; logs in target/farm/camera-work/kit/runs/shape.
- **The ultrawide's floor is a whole frame, not a blanking**: 64 rows at frames of 164, 150, 132 and 120 lines ran 929, 1015, 1154 and 1269 a second (drained 1021 to 1278), each the frame asked; 120 lines is 56 past the rows. With 104 at 4 rows (and 96 refused) its least frame is about 104 lines whatever the rows up to 64.
- **The front's floor is its sensor's frame**: 2 rows asked 122, 110 and 96 lines ran 784, 791 and 791 a second (797 to 803 drained): about 121 lines of 10.4378 us, however short the frame asked.
- **The tele's slits land down to 2 rows with its phase detect off** (G#0D00 = 0 in the hold): 64, 16, 8, 4 and 2 rows, none lost, the link's ERR_CRC_PH gone (+1C G#40 remains from the first whole frame before the mode lands mid-stream). 2 rows 713 a second drained, 16 rows 670, 64 rows 559: its frame the rows plus about 229 lines of 6.1068 us. Committed in deca6b4f.
- **The main at 8 rows takes no frame end even with the front end left at the whole frame** (`F_KEEP_FE`: loch0 and the link at 3072 rows): +24 G#5, +2C G#0, ERR_CRC_PH; the PHY's last header a RAW10 line of 5100 bytes (G#13EC, G#2B). So neither loch0's nor the link's geometry stops it: the short frame's packets fail at the link itself, and its least stays 16 rows.
- **Stored**: `camera` from deca6b4f (566689 bytes, blake3 b4efb01b...), read back equal; started on demand it ran the tele's 2-row slit at 715 a second, none lost. The one before (7acde33f, blake3 663d5577...) kept in target/farm/camera-work/out/camera-before-deca6b4f.capsule.vsf.

Exactly 2 rows to memory today: the front (791 a second, 1.6k lines a second) and the tele (713, 1.4k); the ultrawide keeps 2 of 4 (1465, 2.9k lines a second of the 2 kept), the main 2 of 16 (1231, 2.5k).

### The main's short frames: the link's rate, not the rows (2026-11-22, desk)

What the runs say together:
- The main's link fails on rate as well as on rows: at 64 rows a 6320-clock line is clean and a 5888-clock line sets ERR_CRC_PH; at the full 4080 width 5376 clocks takes no frame ("The line, toward a million"). At 6320 clocks it carries 5100 bytes a 3.149 us line, 1.62 GB/s, near what the link takes: the main runs its link closest to its limit of the four.
- The tele (4032 a 6.107 us line, 0.83 GB/s), the front (3840 a 10.44 us line, 0.46 GB/s) and the ultrawide (4000 a 6.56 us line, 0.76 GB/s) carry half or less, and all three take short frames.
- The main's errors grow as its frame shortens: 64 rows clean, 56 to 16 ERR_CRC_PH with frames landing, 12 and under no frame end at all. A binned sensor's line buffer starts a frame by sending its first lines as fast as they come, not a line time apart; in a long frame that is a few lines among thousands, in a short one it is the whole frame, and on a link already near its rate the frame end is lost in it.
- The fix, if so, costs nothing in rate: the main's floor is a time (about 812 us), so a slit's line can be two or four times longer (16 rows of 12.6 us are 200 us of the 812) and its bytes a second on the link half or a quarter.
- The 1:1 straddle (8160 a line, 6.586 us, 1.55 GB/s, ERR_CRC_PH at 2 and 4 rows) reads the same.

**Device test (staged: target/farm/camera-work/kit/burst.sh, the stored deca6b4f driver hand-spawned, no code change):** the main's 8-row slit at lines of 6320, 9480, 12640 and 25280 clocks; 12, 8, 4 and 2 rows at 12640; 8 rows narrowed to 2048 and 1280 columns at the mode's line (fewer bytes a line, the same line); the 1:1 straddle at 11600, 23200 and 46400 clocks. If the longer line or the narrower window lands them, the main's slits take the line that keeps the link under its rate (a mode's `llp`, already in OPEN's mode block), and its least rows drop from 16.

### The ultrawide's 2-row refusal (desk)

At 2 rows its image channel reports VRESOL_MISMATCH (+1C G#40, channel 0 alone starting) and loch0 G#414: the Sony sends a line count other than its 2-row window's. Its slit sets the digital crop's height to the rows (ccs `Mode::slit`, `crop[3]`) over a 4-address window (2 rows binned 2 x 2); a crop or output height under its pipeline's least (4 rows streams) is the likely cause. Its floor is a whole frame of about 104 lines whatever the rows, so 2 rows would gain nothing over 4 with 2 kept: left until the main's line test is done. A device knob writing the link's channel 0 height (as the fifteenth byte writes channel 1's) would read the count it sends.

### The main's short frames on the Pro (2026-11-22 night, the stored deca6b4f driver hand-spawned, kit/burst.sh; logs kit/runs/burst)

| slit | line (clocks) | slits a second (sensor / drained) | link |
|---|---|---|---|
| 8 rows, 4080 | 6320 | nothing lands | ERR_CRC_PH, no frame end |
| 8 rows, 4080 | 9480 | nothing lands | the same |
| 8 rows, 4080 | 12640 | 1050 / 1005 | **clean** (+14 G#0) |
| 8 rows, 4080 | 25280 | 581 / 576 | clean |
| 12 rows, 4080 | 12640 | 1044 / 1035 | lands |
| 4 and 2 rows, 4080 | 12640 | nothing lands | ERR_CRC_PH, no frame end |
| 8 rows, 2048 wide | 6320 | nothing lands | |
| 8 rows, 1280 wide | 6320 | 1237 / 1242 | lands, ERR_CRC_PH and ERR_LOST_FS beside |
| 1:1 straddle, 8160 x 2 | 11600, 23200, 46400 | nothing lands (sensor 818, 512, 284) | ERR_CRC_PH, no frame end, at every line |

- **The link's rate is the cause for 8 to 12 rows**: halving the bytes a second (the line doubled) or cutting them to a third (1280 columns) lands 8 rows, the doubled line with a clean link. 4 and 2 rows still fail at the doubled line, and the 1:1 straddle at every line to four times its own: under 8 rows something else holds, not settled.
- **No faster for it**: at the doubled line the main's floor is about 952 us (1050 a second), against 812 us (1231) for 16 rows at its own line. Two rows kept a slit: 16 rows at the mode's line 2.46k lines a second, 8 rows at the doubled line 2.10k. So the list keeps 16 rows at the mode's line (`slit_rows_least` 16), the fastest full-width slit; 8 rows full width (a doubled line) or 8 rows 1280 wide (1237 a second, a third of the field) are there for a client that wants fewer bytes, not more speed.
- **The 1:1 slit fields stay held back**: no 1:1 slit lands at any line tried.

### Next: the main's frames of 4 rows and under (lead, not yet tested)

What separates them from 8 rows and up: at a doubled line 8 and 12 rows land with a clean link and 4 and 2 rows do not, the same "frame starts on channels 0 and 2, no frame end, ERR_CRC_PH" whatever the rate; the 1:1 straddle (2 rows) and the 1:1 4-row control fail the same way at every line to four times their own. So under about 8 rows the cause is the frame's shape, not the link's rate.
- **The lead: the embedded-data channel.** Channel 2 (data type G#12) carries 4 lines a frame and stays on whatever G#0118 is written ("The slit for real"); the link is told 4 lines of 4080 (+G#64 G#40FF0). A frame of 4 image lines or fewer is the one where the embedded lines are as many as the image's, and the tele and front, which take 2 rows, write their embedded control the same way but run their links at half the main's rate or less. Either the sensor's embedded lines interleave with so few image lines that the link misreads the packets, or the link's channel 2 expectation breaks the frame's end.
- **Device test** (a driver knob, as the fifteenth byte writes channel 1's height): the main's 4 and 2-row slits at the doubled line with the link's channel 2 switched off (+G#60 = 0) and, separately, with its height set to 0; then the same on the 1:1 straddle. If either lands them, the slit's receiver drops channel 2 and the main's least rows go to 2 at a doubled line (1050 a second, the same as 8 rows there; worth it for the 1:1 straddle, the owner's quad rows).
- If neither does: the sensor's own least output height (the main's binning G#3244 may need 8 output rows), read by its frame counter against a window of 8 rows with its output size 2, if its digital crop takes it.
- **Tested on the Pro (2026-11-22 night, kit/emb.sh, slit-knobs.diff hand-spawned): the embedded-data channel is not it.** With the link's channel 2 config word written 0 (+G#60 G#B048 to 0) or its height 0 (+G#64 G#40FF0 to G#0FF0), read back as written, the main's 4 and 2-row slits at the doubled line and the 1:1 straddle at 11600 and 23200 clocks fail exactly as before (frame starts on +24 bits 0 and 2, no frame end, ERR_CRC_PH); 8 rows lands as before (1042 a second). The link still reports channel 2's frame starts with its config cleared, so +24's bit 2 is not what the link's channel 2 word gates. Next: the sensor's own least output height (its binning G#3244), not the receiver.

### The tele's focus from Android, and every lens's nearest focus (the Pro, 2026-11-22 night)

One Android boot (`diag android`, no flash), the kernel's i2c tracepoints under root filtered to humbaba (G#61) and the tele's sensor (G#2D) on i2c-4, Lumis on Android racking the tele's 22-degree readout by taps: its focus slider from its far end (infinity) to its near end (80 cm), back, then eight even steps; `adb reboot` back to ferros (b1137, the identity unlocked at boot). The capture: target/farm/camera-work/kit/runs/tele-af (local; B3SUMS), every CPU's overrun and dropped events 0, tools/oracle-i2c `--ftrace`: 0 writes whose bytes are not their length, 0 reads short, 0 transfers not whole; 165385 humbaba transfers, 25499 the sensor's.

**Humbaba's focus, decoded** (16-bit register addresses, 8-bit values):
- At each open: G#0201 read (01, ready), G#0200 = 01, G#0B00 = 01, G#0200 = 00, G#0200 = 01 (servo on); later G#0278 = G#3B00, G#0B00 = 01, G#0000 = 01, and reads of G#085C/085E and G#0414/0416 (calibration words, read only).
- **The target: a 16-bit little-endian value, its low byte at G#0204 and its high byte at G#0205**, each written as its own one-byte write (`02 04 lo`, `02 05 hi`).
- **The position: G#0208 (low) and G#0209 (high)**, polled between targets (G#1A0A for a target of G#1A0C: within 2). G#0B10/G#0B12 are polled as well (the OIS's, not chased).
- **Android's targets**: the slider's far end (0 dioptres) **6668 (G#1A0C)**, its near end (1.25 dioptres, 80 cm) **13155 (G#3363)**, the eight steps 7456, 8270, 8983, 9848, 10662, 11400, 12316, 13130, about 810 apart: linear in dioptres, larger nearer.
- Against the factory's codes (persist AF_calibration.cal, "rear tele"): infinity 7050 and near 13538, 6488 apart; Android's span is 6487 and both ends lie 382 lower. So the factory's codes are humbaba's own units, its near point is the HAL's minimum focus distance (80 cm), and the HAL drives them less 382 (an offset applied to both ends, likely its temperature or posture compensation; the factory's were measured at 33 C).
- So for the tele: `focus_infinity` 6668 and `focus_near` 13155 as Android drives it (or the factory's 7050 and 13538), `focus_near_mm` 800. Its whole travel is unmeasured; Android drove nothing outside 6668 to 13155.

**Every camera's nearest focus, from the HAL** (`dumpsys media.camera`, read only; each camera's LENS_INFO_MINIMUM_FOCUS_DISTANCE, its calibration APPROXIMATE):
| camera (Camera2 physical id) | minimum focus distance | so `focus_near_mm` | hyperfocal |
|---|---|---|---|
| main (2) | 9.5238 dioptres | 105 | 0.169 dioptres |
| ultrawide (3) | 50 dioptres | 20 | 1.255 dioptres |
| front (7) | 10 dioptres | 100 | (none listed) |
| tele (4) | 1.25 dioptres | 800 | 0.0242 dioptres |
The tele's capture puts the factory's near code at the HAL's minimum focus distance; the three cornerfolk lenses' near codes are taken the same way, unmeasured.

**Next** (built on the owner's go): the tele's focus in the driver, humbaba's protocol beside the cornerfolk's (servo on, the target at G#0204/G#0205, the position at G#0208/G#0209), the list's `focus_infinity` 6668 and `focus_near` 13155; `focus_near_mm` on all four (105, 20, 100, 800). A device check: the tele's target written and its position read back across 6668 to 13155, and a sharpness rack on the owner's near and far targets.
**Built and run (b39efa9d, the Pro b1137, the flashed 07f922ca kernel; the driver hand-spawned, then stored):**
- The tele opens at infinity (6668), humbaba ready at its first read; camtest's sweep `1100000D031B33` (17 targets G#1B00 to G#3300): every target taken, the position read back within 4 of it (G#1AFF for G#1B00, G#2704 for G#2700, G#317F for G#3180), the open's 6668 read back exact.
- Sharpness over the owner's near and far targets (the frame's middle half): 7 at the infinity end, rising to 10 at G#2280 to G#2400 (8832 to 9216, about 0.45 dioptres, 2.2 m), 7 again past G#2580: the lens racks and the scene comes into and out of focus.
- The main's cornerfolk path unchanged (its open's G#1FD0 read back G#1FE4, G#5555 G#555C, G#8AAA G#8A98).
- Stored: `camera` from b39efa9d (567473 bytes, blake3 b3468dc5...), read back equal; the one before (deca6b4f) kept in target/farm/camera-work/out/camera-before-b39efa9d.capsule.vsf.
- **Its travel** (a test build with the range open, every target read back): G#0100 to G#1900 within 7 (G#100 apart), G#3300 to G#3B00 within 2, G#3F00 to G#3EEC; past G#3F00 the position reads G#3FF2 to G#3FFF whatever the target, its 14-bit end. Offered since d49a0433: 256 to 16128, about 1.24 dioptres past infinity and to about 55 cm. Stored from d49a0433 (blake3 2963178d...), read back equal; it took G#0100 (read G#0103) and refused G#4000 (answer 1); the one before (b39efa9d) kept in target/farm/camera-work/out/camera-before-d49a0433.capsule.vsf.

## The driver (proposed, for the owner; built 2026-11-14, "Built" above)

KERNEL-OUT.md decision 3 (the camera harness) is still open: its proposal was to leave the harness until the pipeline is understood, then write the userspace driver fresh.
The pipeline is understood now for the main camera and the ultrawide, so this is that driver, in the pattern every move followed (grants, a power switch for the privileged part, the protocol in a process, started on demand):
- **A `camera` power switch** in the kernel for what a process cannot do: the camera PMIC's rails (ACPM channel 2, with the overvoltage guard), the ISP front end's power domain (the EL3 prepare, once per boot), the D-PHY isolation bypass (PMU G#3EBC, once per boot), the front end's NOC clock (CMU_TOP G#2604_10BC / G#2604_18B4).
  Each step is code camera.rs has and has run.
- **`payloads/camera`**, started on demand by a call on `camera` (process.rs `ON_DEMAND`, as torch and audio are), ending when its last holder closes:
  - grants: the sensor buses (hsi2c_1 and hsi2c_3 on husky, polled, as hsi2c.rs is), the front end (G#1C30_0000, 1 MiB), the PDMA (G#1C39_0000, G#6000), the CSI receiver and PHY lanes (G#1C04_0000.., G#1C10_1000..), its interrupt lines, its own frame region, the vault scope `cam.` for the inits, programs and IDTs;
  - the sensor init replayed, the PDMA job laid out in its region with every address register remapped there, the receiver last (as a camera open does it), then each frame answered on the interrupt (`FRAME_ACK`, G#81 to G#1C32_00A0 after the first SHADOW_UPDATE) and served to the holder;
  - the close replayed (`camera_close.rs`, 223 writes) and the sensor powered down at the end.
- **Frame memory**: the image body (15 MiB), the PD path (3.75 MiB) and whatever small outputs the job writes, in a region nothing else uses.
  How many of the 22 outputs the main camera's job really writes is measured by a run's own per-slice report ("buffer A#k at ...: A#n probe points written"); October's runs were read for the image body only.
- **Then the harness goes**, its diagnostics with it (decision 4's bucket for `cam`).

## Lumis itself (2026-11-14)

The front above is retired (owner, 2026-11-14: "Lumis, that is a camera app for Android, most is in Rust so it should be a minimal transplant").
Lumis runs on ferros as itself: `std/lumis` builds Lumis's own core (lumis_core, Lumis branch `ferros-port`) under fluor's ferros host, and only the platform layer is new (lumis_core `app` and `platform`).
- **The seam**: every JNI entry point is a thin wrapper over `lumis_core::app` (menu, camera setup, UI frame, calibration, save done); the window is a `platform::Surface` (Android's ANativeWindow, or an RGB888 `PixelSurface` elsewhere); exit, descriptors and the config store are `platform` too. Android's build is the same code thru JNI.
- **The ferros arm** (std/lumis): the menu from the camera list (each camera at bin 2, the finest a call carries), the camera service as a thread (open, the first frame seeds the integrator, then a frame a call into `process_frame`, the settings it answers sent with SET), the UI as a second thread over the integrator's memory, Lumis's frame shown as fluor's 16-bit g2t pixels (the rows that changed), its settings and saves in the kist's vault (kete, device scope).
- **Colour**: the display matrix is the list's Absolute IDT with its sensitivity, white at green 1, ending in VSF RGB (`platform::DISPLAY_VSF_RGB`); Android's ends in Rec.2020 as before.
- **Saves**: VSF RAW and VSF 8B only (the house formats); the JPEG, JPEG XL, TIFF and DNG writers are not in the ferros build.
- **Run on the Pro** (hand-spawned with a kist's slots, the camera kernel b874): the menu, the lens's modes, the live preview at about 9.6 frames a second (Lumis's draw 90 ms a frame on the CPU), the camera served 115 ms apart, the shutter slider taken by the driver, a VSF RAW save kept (3918758 bytes).

## For the morning

On akita (nothing to decide, nothing at risk): Lumis's front, spawned by hand with the slots a kist gets, shows its "no camera driver" page.
```
LUMIS=/mnt/Harbor/Code/lumis-ferros tools/lumis-build.sh
FERROS_PORT=1-2.4 target/debug/ferros-bridge spawn /mnt/Harbor/Code/lumis-ferros/rust/lumis-ferros/target/ferros/aarch64-unknown-ferros/release/lumis.bin --name lumis --use display --use touch --dma surface --ira --domain --entropy --use camera
```
Lumis's preview and controls on a device before the driver exists: `std/camreplay`, a development stand-in that serves the endpoint with one recorded ferros frame of the Pro's main camera (binned at build time from a local file, never in the repository; the list names it a recording, the controls change nothing).
It is built (6654640 bytes) and has not run: the 8a went silent on USB during another session's kernel flash at 22:25 that night, before the test could take the screen.
```
cd std/camreplay && CAMREPLAY_FRAME=/mnt/Harbor/Code/ferros/target/cam/img.bin ../../tools/std-build.sh . && cd ../..
FERROS_PORT=1-2.4 target/debug/ferros-bridge spawn std/camreplay/target/aarch64-unknown-ferros/release/camreplay.bin --name camreplay --serve camera
```
then Lumis as above, and tap the camera's row: the recording, developed through the main camera's Absolute IDT, rotated upright, should fill the width; the display gain row brightens it.
Unchecked until then: a 6 MiB reply between two processes, and how many frames a second Lumis develops.

As a kist (after a kernel, stower and map with the camera right are on the device): `LUMIS=/mnt/Harbor/Code/lumis-ferros tools/lumis-kist.sh`, then Stow on its screen.

On the Pro: nothing camera runs beside the shell today (above), and the endpoint has no server.
The decisions that start the driver: KERNEL-OUT.md 3 (the driver above, and the harness out after it), where the frame region goes, and whether akita's cameras get an oracle capture session (Android under the hv, the Pixel Camera app driven over adb).
