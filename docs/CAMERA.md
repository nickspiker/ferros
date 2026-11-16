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

**akita: no camera has been touched from ferros.** From its device tree (/mnt/Harbor/ferros-ref/kdev-akita/dts/zuma-akita-camera.dtsi, -camera-pmic.dtsi):
- The main camera, "oksoko", is on hsi2c_1 at G#1A: the same sensor model as husky's ultrawide (Sony IMX787, G#0787 at G#0016), which ferros streams. Its init exists locally (target/cam/uw.init), but husky's ultrawide runs a 24 MHz master clock and akita's main 26 MHz (CIS_CLK3, mclk4): the captured PLL settings would run it about 8 % fast. Recapture on akita, or rescale the PLL.
- Its power: sub-PMIC LDO12, camera-PMIC LDO5, 4, 1, 3, then the sub-PMIC's GPIO2 (CAMS_LVBUCK_EN), reset gpp2-2, 1 ms after the enable, the clock 2 ms after.
- The camera PMIC is an SLG51002 at G#75 on hsi2c_14 (as husky's), with an RT6160 buck-boost at G#77 on hsi2c_9 that husky does not have.
- The front ("leshen") and the ultrawide ("leshen-uw") are on hsi2c_2 and hsi2c_3, both at G#1A; their part is not named in the tree.
- Its CSI link, PHY lanes and the front end's program are unknown: they come from an hv oracle capture of Android's camera on akita, as husky's did (`tools/akita-hv.sh` runs Android under the hv).

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
| akita's cameras | nothing | power, ID, init at 26 MHz, the receiver and front end from a capture |

## The endpoint

`camera`, served by the camera driver, called by a kist holding the camera right (slot 7 for a kist that declares only it).
A request is an op byte and its arguments; numbers are little-endian.
The list is a VSF document (the house format for anything structured); a frame is a fixed header and the samples, as the GPS fix is.

| op | request | reply |
|---|---|---|
| 0 LIST | `[0]` | a VSF document, a section `camera` per camera the driver can stream whose inputs are in the vault (none: an empty document; "The other cameras") |
| 1 OPEN | `[1][id]`, or `[1][id][1]` for the view, then optionally the mode block `[kind][rows: 2][fll: 2][exposure: 2][flags][ring][line length: 2]` ("Modes, the bench and the slit") | `[0]` open, `[1]` no such camera (or one that cannot stream: the tele), `[2]` another holds it, `[3]` it would not start; with the view given, `[0]` then the view's token (32 bytes) and the ring's shape ("The view"; 64 bytes with a mode block: also the ring's count as a u32 at 48, 1 at 52 for a slit ring, a slit's record offset at 56) |
| 2 FRAME | `[2][bin]` | waits for the next frame: a 64-byte header, then the mosaic, `bin` x `bin` same-colour samples averaged, 16 bits a sample, row by row |
| 3 SET | `[3][what][value: 8 bytes]` | `[0]` taken, `[1]` outside the range; what: 0 exposure (ns), 1 analogue gain (1024 unity), 2 focus (the actuator's position) |
| 4 CLOSE | `[4]` | the view taken back, the camera stops; it also stops when its holder ends |
| 5 MAP | `[5][buffer to give back, G#FF none]` | with the view: waits for a frame newer than the last mapped and holds its buffer; its 64-byte header (bin 1; the buffer at +40, the format at +41, a line's bytes at +44, the buffer's place in the view at +48), the samples read in place |
| 6 RELEASE | `[6][buffer]` | the held buffer given back: `[0]`, `[1]` when it was not held |
| 7 PROBE | `[7][id]`, development, no camera open | that camera's module powered, its identity read, the addresses that answer on its bus mapped, the module down: `[code][id: 2][error]` and a 16-byte map (bit a of byte a / 8); code 0 it answered as itself, 1 another identity, 2 no answer (the bus's error), 3 the switch refused, 4 no such camera, 5 a camera is open |
| 8 STATS | `[8]` | 26 u64s: frames written, answers, on the line, the frame thread's ticks and most, slit frames late, ring frames unseen, the long ring's count and end, the counter and its frequency, refused, status bits, rows, width, line, buffer, slit ring, fll, line length, integration, bits, slim, PD, ticking, open time |
| 9 SLITS | `[9][after: u32]`, a slit ring open | waits for a frame past `after`: 8 u64s, the newest count, the ring's count, a buffer's bytes, rows, line, width, late, ring ended; frame n is in buffer (n - 1) mod the ring, its record (count, counter, Eagle ns, probe moved) after its rows |
| 10 RATE | `[10][windows]` | the sensor's frame counter (CCS G#0005) read every 2^16 us for `windows` (16) windows beside the front end's count: 5 u64s, sensor frames, front-end frames, ticks, frequency, failed reads |
| 11 CAPS | `[11]`, a camera open | 73 bytes: the sensor's data formats (G#00C0 and 16 descriptors), integration least and margin (G#1004), the mode registers read back, a bit per failed read |

The ids: 0 the main (boitata), 1 the ultrawide (oksoko), 2 the front (dokkaebi), 3 the tele (kraken); each is also the `camera` switch's module (ferros_cam cams).

A `camera` section's fields: `id` (u), `name` (a: main, ultrawide, front), `facing` (u: 0 away from the person, 1 toward), `width` and `height` (u: the readout), `cfa` (u: the 2 x 2 cell's channels, 2 bits each, top left first, 0 R 1 G 2 B: husky's main G B / R G is G#49), `black` and `white` (u), `rotation` (u: quarter turns clockwise to stand it upright; husky's main 1), `idt` (nine f5: the Absolute IDT's matrix, row-major, camera to VSF RGB), `sensitivity` (three f5), `exposure_min`, `exposure_max` (u, ns), `gain_min`, `gain_max` (u, 1024 unity), `focus_min`, `focus_max` (u; both 0 where focus is not offered).
The IDT is the device's calibration (husky's main: `pixel 8 pro calibration data/derived/cam2-main.idt.vsf`), kept in the vault beside the sensor's init, never in a kist: `cam.<key>.idt` (the `.idt.vsf` file as it is: `cam.boitata.idt`, `cam.oksoko.idt`, `cam.dokkaebi.idt`, `cam.kraken.idt` from cam2-main, cam3-ultrawide, cam7-front, cam4-tele).
Since 2026-11-15 the driver reads it there at every LIST and carries its Absolute entry's matrix and sensitivity in that camera's section (ferros_cam list); a camera whose IDT is absent is not listed.
Until then the main's was not read from the vault at all: the driver's build.rs read the repository's file and built the list in.

The frame header: count (u32, +0), width and height after binning (u16, +4, +6), bin (+8), cfa (+9), black and white (u16, +10, +12), exposure (u64 ns, +16), gain (u32, +24), focus (u32, +28), when the exposure ended (Eagle time, i64 ns, +32), zeros to +64.
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
Check before using any of it: the tele really streamed in each (the I2C capture holds a G#0100 stream-on to G#2D; the front-end capture holds link writes past the main's; a screenshot of the app showing 5x on the tele, not "crop"), and `tools/oracle-decode-hsi2c.py` reports no LENGTH MISMATCH inside the init.
From them, as for the front on 2026-11-06: `tools/camera-cpu.py` (the CPU setup, a 'CPR2', and the receiver table), `tools/oracle-decode-pdma.py` then `tools/camera-pdma-image.py --list 2` (the job, a 'CPR3'), `tools/camera-init.py` (the init), all local files (target/cam/tele.*, the vendor's settings).
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
Check: the I2C log decodes with no LENGTH MISMATCH (`tools/oracle-decode-hsi2c.py`), holds a G#0100 stream on, and its G#034C/G#034E read 8160 x 6144 (G#1FE0, G#1800) in the decode.
From it: `tools/camera-init.py` gives a 1:1 init (target/cam/main-full.init, local, the vendor's settings), whose mode `ccs::Mode::captured` reads; the driver then wants that init as a second input per camera (`cam.<key>.init.full`, not built yet: one key and a kind that replays it instead of writing the guess).
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
