# The camera as a service (Lumis on ferros)

The bring-up record, register by register, is docs/oracle/CAMERA.md.
This is the other half: what a kist needs from the cameras, the endpoint it calls, and how far ferros is from Lumis showing a live preview.
Started 2026-11-12 (the owner: "would be nice to have ... lumis fully working in the morning").

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
  **Do not run `diag cam pdma` on a running shell.** It is safe only on a boot where nothing has started (as all of October's runs were).
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
| the endpoint | specified below | a server |
| frame memory | none | a region of its own, out of the frame pool; the PDMA job rebuilt for it |
| the driver | the kernel's diagnostics only | a process (below) |
| the front end's answer | the CPU, every 15 to 50 ms, in a spin | its interrupt (ISPFE SPIs A#479 to A#483, zuma.h; zuma-isp.dtsi `isp-fe`) |
| the PDMA job | a local file built into the kernel by tools/camera-pdma-image.py | from the vault at run time, its address registers remapped to the new region (in Rust: no new Python) |
| akita's cameras | nothing | power, ID, init at 26 MHz, the receiver and front end from a capture |

## The endpoint

`camera`, served by the camera driver, called by a kist holding the camera right (slot 7 for a kist that declares only it).
A request is an op byte and its arguments; numbers are little-endian.
The list is a VSF document (the house format for anything structured); a frame is a fixed header and the samples, as the GPS fix is.

| op | request | reply |
|---|---|---|
| 0 LIST | `[0]` | a VSF document, a section `camera` per camera the driver can stream (none: an empty document) |
| 1 OPEN | `[1][id]` | `[0]` open, `[1]` no such camera, `[2]` another holds it |
| 2 FRAME | `[2][bin]` | waits for the next frame: a 64-byte header, then the mosaic, `bin` x `bin` same-colour samples averaged, 16 bits a sample, row by row |
| 3 SET | `[3][what][value: 8 bytes]` | `[0]` taken, `[1]` outside the range; what: 0 exposure (ns), 1 analogue gain (1024 unity), 2 focus (the actuator's position) |
| 4 CLOSE | `[4]` | the camera stops; it also stops when its holder ends |

A `camera` section's fields: `id` (u), `name` (a: main, ultrawide, front), `facing` (u: 0 away from the person, 1 toward), `width` and `height` (u: the readout), `cfa` (u: the 2 x 2 cell's channels, 2 bits each, top left first, 0 R 1 G 2 B: husky's main G B / R G is G#49), `black` and `white` (u), `rotation` (u: quarter turns clockwise to stand it upright; husky's main 1), `idt` (nine f5: the Absolute IDT's matrix, row-major, camera to VSF RGB), `sensitivity` (three f5), `exposure_min`, `exposure_max` (u, ns), `gain_min`, `gain_max` (u, 1024 unity), `focus_min`, `focus_max` (u).
The IDT is the device's calibration (husky's main: `pixel 8 pro calibration data/derived/cam2-main.idt.vsf`), kept in the vault beside the sensor's init, never in a kist.

The frame header: count (u32, +0), width and height after binning (u16, +4, +6), bin (+8), cfa (+9), black and white (u16, +10, +12), exposure (u64 ns, +16), gain (u32, +24), focus (u32, +28), when the exposure ended (Eagle time, i64 ns, +32), zeros to +64.
The client's reader and the header's writer are side by side in rust/lumis-ferros/src/camera.rs, tested together.

A frame by copy is for the preview: husky's main at bin 2 is 2040 x 1536 samples, 6 MiB a frame, 90 MB/s at 14.5 frames a second.
A full raw frame for a capture (25 MB at 16 bits) wants a mapping instead, a ring of frame buffers the kist reads in place (KISTS.md "Lumis"); that is an object the kernel does not have yet.
Unchecked: how large a reply the kernel copies between two processes in one call (the store's are capped at STORE_MAX; an endpoint's reply is copied whole as far as the code shows).

## The driver (proposed, for the owner)

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
