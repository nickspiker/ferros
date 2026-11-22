# RAW video on ferros

Status: measured and prototyped, 2026-11-19 (branch raw-video).
Nothing is built into a shooter yet, and nothing was stored on a device in a new format.
The format decision at the end is the owner's (AGENT.md Rule -1).

The owner (2026-11-19): a RAW video shooter on ferros, graded later, lossless if at all possible; the main camera at 24, 30 and 60 a second binned and at 24 a second 1:1; clips of about a minute.

## The answer first

| mode | raw | fits the flash for a minute? | lossless (measured 2 to 3.5x) | CPU today |
|---|---|---|---|---|
| binned 24/s | 376 MB/s | yes, uncompressed, sustained (the flash's floor is 555 MB/s) | 110 to 190 MB/s | the codec at 24/s on four A715s at 1065 MHz has room (26.6 ms of 41.7) |
| binned 30/s | 470 MB/s | yes, uncompressed (85 % of the floor; with WriteBooster on, all of a minute lands in its SLC cache) | 135 to 240 MB/s | **measured: 30/s for 60 s on four A715s at 1065 MHz, 61 C at most** |
| binned 60/s | 940 MB/s | no uncompressed (the cache holds about 33 GB, 35 s of it) | 270 to 480 MB/s: fits | 12.4 ms of 16.7 at 2367 MHz, but **not sustained: 60/s for 10 s, then 88 C and the clock divider, 38 to 40/s** |
| 1:1 24/s | 1504 MB/s | no | 650 MB/s at 2.3x: the cache covers about 50 s, then a 1 GB backlog | 61 ms a frame on four A715s at 2367 MHz: 16/s, short of 24 |

- **Lossless fits the internal flash in every mode but 1:1**, and binned 24 and 30 even fit uncompressed.
- **The wall was the CPU, not the flash**: 60/s binned was heat-limited and 1:1 at 24/s compute-limited with this codec on the CPU. Since 2026-11-19 the band coder runs on the Mali GPU (Part 4, "On the GPU"): 60/s binned for a minute, none dropped, 71 C at most.
- The visually lossless mode (below) adds ratio (about 3.5 to 3.9x at s = 1) but not speed: it runs the same coder.
- Lossless stays the default.

## What the camera gives

| mode | frame | a frame (RAW10 packed) | a second | a minute |
|---|---|---|---|---|
| binned 24/s | 4080 x 3072, Bayer G B / R G | 15.67 MB | 376 MB/s | 22.6 GB |
| binned 30/s | | | 470 MB/s | 28.2 GB |
| binned 60/s | | | 940 MB/s | 56.4 GB |
| 1:1 24/s | 8160 x 6144, quad (2 x 2 blocks of one colour) | 62.67 MB | 1504 MB/s | 90.2 GB |

The front end writes each line on a 256-byte stride (5120 bytes binned, 10240 at 1:1); the table counts the pixels' 10 bits only.

## Part 1: the flash

### Where a clip could live: the ferros partition, proven

The Pro's ferros partition: LU 0, first LBA G#1B8_B400 (A#28881920), A#33554427 blocks of 4 KiB (128 GiB less 20 KiB), checked against the GPT by `ufs_device::locate` and said by every `diag` (FERROS_PART_FIRST, FERROS_PART_BLOCKS).
Everything the code writes there, by the code that places it (partition-relative blocks):

| blocks | what | placed by |
|---|---|---|
| 0 to 255 | the old ferros_vault file | shim.rs `ferros_windowed(.., 256)` (shim builds only) |
| 2^18 to 2^18 + 2^14 | the old text log (1 GiB in) | flashlog.rs `OLD_OFFSET`, `OLD_BLOCKS` |
| 2^18 + 2^14 to 2^18 + 3 x 2^14 | the ledger's flash ring, two copies | flashlog.rs `OFFSET`, `HALF` |
| 2^19 to 2^19 + 2^17 | the kernel's vault, mirrors A and B (2 GiB to 2.5 GiB) | kvault.rs `MIRROR_OFFSET`, `REGION_BLOCKS` |
| 2^20 to 2^20 + 2^22 | the vault process's window (4 GiB to 20 GiB), its two mirrors 2^21 apart | process.rs `WINDOWS` `vault`, payloads/vault `HALF` |
| 2^20 + 2^22 to the end | **nothing**: about 108 GiB | |

The kernel bounds every SYS_BLOCK call to its window and every window to the partition; no other branch adds a window (searched every remote branch); the hv never writes UFS (its firewall only reads Android's commands); ferros_hal ring.rs's absolute LBAs (an old M1-era layout) have no caller on husky.
One stray found: shim.rs writes and restores absolute LBA A#30000000 (partition block A#1118080, inside the vault process's window) in `FERROS_SHIM_HANDOFF` builds only; harmless while nobody builds that, but it should move.

The measurements below wrote only to a development window, `rawbench` (process.rs `WINDOWS`, development builds only): 2^23 blocks from 2^23 in, 32 GiB to 64 GiB, inside the free stretch on both boards (akita's partition is 77.7 GiB).
Its blocks now hold the bench's patterns; nothing reads them.

### The link and the device

`diag "ufs link"` (new, service.rs) reads the host's own UniPro attributes and the device's descriptors:
- **HS-G4, series B, two lanes each way, fast mode**: the fastest the link offers (the device's advertised receive gear is 4). ferros runs it at its top.
- The device reports spec G#0400 (UFS 4.0), a queue depth of A#64; the controller has 32 transfer slots. **ferros's driver uses one** (queue depth 1, one command at a time, polled), up to 1 MiB a command (256 PRDT entries of 4 KiB).
- **WriteBooster**: a shared SLC buffer of A#8192 units of 4 MiB (32 GiB nominal), user space preserved, life used under a tenth. **ferros never turned it on**: its enable and flush flags are volatile (every power-on clears them) and Linux sets them at each probe. `diag "ufs link wb on|off"` sets them now.

### Measured on the Pro (2026-11-19, b1001 and b1003, this branch's kernel RAM-reloaded; std/rawbench, 1 MiB commands of incompressible bytes, sequential)

| what | rate | over time |
|---|---|---|
| write, queue 1, WriteBooster off | **556 MB/s** | flat for 64 s, no cliff (33960 MiB): the device's direct rate |
| write, queue 4 and 8, WriteBooster off | 558 and 559 MB/s | the same: the queue does not matter without the cache |
| write, queue 1, WriteBooster on | 960 to 1234 MB/s | for about 20 s, then 550 to 565 MB/s |
| write, queue 8, WriteBooster on (8 commands a call) | **1730 MB/s** (1.92 GB/s at the device) | for 19.5 s (about 33 GB), then 548 to 570 MB/s |
| read, queue 1 | 1242 MB/s | flat for 32 s |
| read, queue 8 | **2216 MB/s** (2244 at the device) | flat: the link's ceiling (2 lanes x 11.66 Gb/s x 8/10 = 2.33 GB/s) |

- **The cliff is the SLC cache, not heat**: with WriteBooster off there is none. The cache said 6 tenths free before the queue-8 run and 0 after, and 8 tenths again about 8 minutes later, idle.
- **For a minute's clip**: about 33 GB at up to 1.7 GB/s, then 555 MB/s for as long as the free space lasts, with WriteBooster on and a queue of 8; 555 MB/s flat without it.
- **The vault's large-object path would halve this**: the vault writes, reads back and checks every block, then the mirror the same way; a clip wants its own region (below).
- **What the write path costs a core now**: for each MiB the kernel copies the caller's bytes into its heap and into the bounce (about 50 us each, process.rs), cleans the bounce from the caches (92 us a MiB measured: 390 ms over 4246 MiB, `diag "ufs runs"`), and then the caller's core spins on the doorbell until the command ends (1.86 ms a MiB at 556 MB/s). A writer at queue 1 keeps one core busy the whole time. Completion by interrupt, and the UFS DMA reading the camera's buffer or the encoder's output directly (UFS is the one coherent DMA master on zuma), would leave the copies and the spin out.
- **A reset during the first queue-8 run (b1001)**: the kernel's core went silent 500 ms with interrupts masked and the watchdog reset the Pro. That run held the UFS lock for 64 commands a call (up to about 120 ms, more if the device stalled), and the kernel's own loop takes that lock with interrupts masked; most likely mine. The rerun held it 8 commands a call and ran clean. A recorder must keep each lock hold short, or give the recording its own path.

### What the camera costs the CPU now (cambench, the write-back view, four A715 threads at 2367 MHz, read every line)

| rate | per frame, wall on 4 threads | threads' reading time | cores used of the four | the link |
|---|---|---|---|---|
| 60/s (fll 5170) | 1.03 ms | 2.73 ms | 0.17 | 955 MB/s |
| 30/s (fll 10340) | 0.99 ms | 2.78 ms | 0.09 | 492 MB/s |

Plus the driver's cache maintenance at each MAP: 0.4 to 0.7 ms a binned buffer (20 MiB with the PD rows), 1.9 ms a 1:1 buffer (60 MiB), up to 16 ms seen with Lumis loading every core.
Reading the frame is cheap since the view is write-back (CAMERA.md "A cacheable view"); the encode is what costs.

## Part 2: the lossless codec (tools/raw-codec, a prototype)

- **Shape**: no_std, no alloc, the unpack of MIPI RAW10 fused in. Each sample predicted from its own colour plane (Bayer: neighbours 2 apart; quad: the nearest same-colour sample, 1 inside a 2 x 2 block, 3 across). Per block of 2^5 samples the cheaper of (a+b+c+d)/4 and MED (one bit), the residual wrapped to 10 bits and zigzagged, then Rice coded with k per block from the block's sum by clz, unary capped at 2^4 then the raw 10 bits. Bands of 2^6 rows are independent, so cores split a frame by bands.
- A JPEG-LS (LOCO-I) coder beside it is the ratio reference: on these noisy frames the averaging predictor ties or beats it, at a tenth of its time.
- Every frame round-tripped bit exact on the host, and on the device (the device build decodes every band and compares every sample before it times anything).

### Ratio (against packed RAW10)

On the Pro, the main camera aimed at the bench's indoor scene (std/rawbench job 3: the frame held, copied out, coded on the device):

| frame | exposure, gain | mean | bits a pixel | ratio |
|---|---|---|---|---|
| binned | 32 ms, 1x | 68 DN (black 64) | 2.86 | 3.50 |
| binned | 128 ms, 1x | 84 DN | 3.34 | 2.99 |
| binned | 32 ms, 8x | 106 DN | 4.15 | 2.41 |
| binned | 32 ms, 32x | 220 DN | 5.11 | 1.96 |
| **1:1 quad** | 32 ms, 8x | 106 DN | 4.32 | **2.31** |

On the host's frames (target/agent-rawvideo/codec-results.txt has every variant): the Pro main's October and November frames at 32x 2.7 to 3.15 (dark), 2.21 on the brightest (52 DN over black); the ultrawide 2.4 to 2.7; the 8a's main 2.2.
- **Noise sets the ratio**: these frames are noise-limited, and each step up in gain costs about half a bit. A brightly lit scene at unity gain should sit between 2 and 3.
- Bands of 16 rows cost 0.02 to 0.03 bits a pixel over 64; a whole frame saves 0.01.
- Predicting from the previous frame gains at most 0.1 bit on a still scene and loses when anything moves (the difference carries both frames' noise): not worth a frame buffer.

### Speed (measured on the Pro, clocks held by `diag dvfs`)

| frame | one A715 at 2367 MHz | four A715s at 2367 | four A715s at 1065 | one A510 at 1704 | four A510s |
|---|---|---|---|---|---|
| binned 12.5 Mpix | 46.2 ms (271 Mpix/s, about 8.7 cycles a pixel) | **12.4 ms** (1.01 Gpix/s) | **26.6 ms** | 165 ms | 32 to 42 ms |
| 1:1 50.1 Mpix | 236 ms | **61 ms** | | 984 ms | 268 ms |

At 1065 MHz one A715 takes 103 ms a binned frame: the same 8.8 cycles a pixel, so the codec scales with the clock.
The host (Ryzen 5950X, 4.5 GHz) takes 24.9 ms a binned frame on one core; the decoder is serial, 60 to 69 ms a binned frame on the host, fine for grading offline.

**Sustained, four A715s, the frame held and coded at the clip's rate for 60 s:**
- 60/s at 2367 MHz: every frame in time for the first 10 s, then the zones at 85 to 90 C (past the 85 C this work meant to stop at: a guard that kills the run at 85 C was added after) and the clusters' dividers stepping: 57, then 48, then 38 to 40 frames a second to the end. Back to 62 C a minute after.
- 30/s at 1065 MHz: 30 frames every second for 60 s, the zones 51 to 61 C, a third of the frames past their 33 ms period (the longest 47 ms), each caught up by the next: a recorder needs two frames of slack.

### The visually lossless mode (optional; lossless stays the default)

Each sample mapped to a code whose step is s times its own photon noise, sigma(x)^2 = g (x - black) + r^2, where that exceeds a DN (a generalised Anscombe transform in closed form, integer only); the codes go thru the same coder; the decoder maps each code back to its bin's centroid. g and r are measured from each frame (photon transfer over same-colour second differences); the Pro's main at 32x reads g 0.41 to 0.60 DN an electron, r about 1.5 DN.

| s | ratio (dark Pro frames; the bright one) | RMS error / sigma | largest error / sigma |
|---|---|---|---|
| 1 | 3.47 to 3.91; 3.73 | 0.39 to 0.54 (0.29 to 0.32 in the mid and high levels, the theory's s / sqrt 12) | 0.42 to 0.90, always under one sigma |
| 1/2 | 2.95 to 3.47 | 0.15 to 0.32 | at most 0.69 |
| 1/4 | 2.52 to 3.15 | 0.07 to 0.23 | |

Its bias is largest next to black (up to 0.46 sigma at s = 1: samples at 63 coming back 64); a fractional bit in the reconstruction, or the frame's own centroids (about 1.3 KB a frame), take most of it.
The prototype's forward map is a 1024-entry table; on the hot path it would be about 6 NEON instructions per 4 samples (ucvtf, fmla, frsqrte with one step, fmla, fcvtns), or a piecewise line per octave picked by clz.

## Part 3: what the recorder needs

### On the internal flash

1. **A recording region of its own**, outside the vault: the vault's write, read back, check and mirror halves the rate and holds every block in its tract. Proposal for the owner: a `clips` window in the free stretch (20 GiB to the end, about 108 GiB: a minute of every lossless mode, or two of binned 30/s uncompressed), written as a ring a frame at a time with a BLAKE3 per frame, one copy; each finished clip's index (frames, offsets, hashes) committed to the vault; and its blocks UNMAPped when deleted so the device's SLC cache and garbage collection stay healthy (ferros sends no UNMAP yet).
2. **WriteBooster on**, with its flush flags (one query each at boot; the cache refills within minutes when idle).
3. **A queue deeper than one**: built here as a measure (ferros_hal `run_queued`, SYS_BLOCK op bit 2 on `rawbench` only); a recorder wants it for real, completion by interrupt, and short UFS lock holds.
4. **No copies**: the UFS DMA reading the encoder's output in place (its S2MPU opened to that buffer), so the CPU's only work is the encode.
5. **An encoder fast enough cool**: 30/s binned is there today at 1065 MHz; 60/s binned and 1:1 at 24/s want about 1.5 to 2x (hand-written NEON for the residual and k steps, the A510s taking bands too, or a Mali compute kernel: GPU-KISTS.md), measured against the 85 C line before anything is promised.

### An external SSD over USB 3.2 Gen 2 (estimated, not built)

The SoC's USB is a dual-role DWC3 (USB32DRD at G#1121_0000, `maximum-speed = "super-speed-plus"`, zuma-usb.dtsi) behind a Synopsys USB3/DisplayPort combo PHY; ferros runs it today as a High Speed device on the eUSB2 PHY only (USB.md: 35 to 40 MB/s).
For an SSD it would need:
- **the combo PHY at Gen 2** (soc-gs drivers/phy/samsung phy-exynos-usbdp-gen2, about 33000 lines of register tables to port the needed subset of) and the controller in host mode;
- **Type-C as host**: the TCPC (role, orientation) and VBUS out from the charger's boost (an SSD draws 1 to 3 W from the phone);
- **an xHCI driver**: command, event and transfer rings, slot and endpoint contexts, enumeration (and a hub, if the SSD hangs off one), the HSI0 SysMMU or S2MPU opened to its rings;
- **USB mass storage**: BOT (one command at a time: CBW, data, CSW) is the smaller job; UAS (four pipes, streams, a real queue) is what reaches the drive's speed;
- the write path: SCSI WRITE(10/16) of 1 MiB or more, the same region idea on the SSD.
- What it would give: Gen 2 is 10 Gb/s with 128b/132b coding, about 1.2 GB/s on the wire; drives like the Samsung T7 are rated 1.05 GB/s; BOT typically 0.8 to 0.9 GB/s, UAS about 1 GB/s. That carries binned 60/s uncompressed (940 MB/s) for as long as the SSD keeps its own cache, but not 1:1 uncompressed (1.5 GB/s); 1:1 lossless (650 MB/s) fits.
- Size: weeks, not days (PHY and Type-C bring-up the riskiest part, as the eUSB2 PHY was), and **the phone has one port**: while an SSD hangs on it, the bridge and charging need a powered hub with the phone as host, or WiFi for the bridge.
- Against it: the internal flash already sustains 555 MB/s, which every lossless mode but 1:1 fits; the SSD buys uncompressed 60/s and headroom, not lossless.

## Part 4: the recorder (branch raw-video-rec, 2026-11-19)

The owner's decisions, 2026-11-19: VSF video; lossless only ("I'd just stick with the lossless compressed"); 1-minute clips; a simple FPS control (any whole rate 1 to 60) and start/stop in Lumis; the frame interval the longer of 1/fps and the shutter; clips single-copy in the vault ("single, no dual write"); 60/s a nice-to-have, cool.

### The pieces

| piece | where | what |
|---|---|---|
| the codec | `ferros_rawcodec/` | no_std, no alloc, format Zil: tools/raw-codec's default path, its bytes less the 4-byte stream length (the container keeps both); the bitstream specified in lib.rs; a `neon` feature (residuals, block sums, codes) |
| the format | vsf branch `raw-video`: `visual::clip`, `visual::cfa_band`, `v('c')` | one VSF document a clip: generation 0, far-form `frame` sections as recorded (each naming the one before by BLAKE3), a closing generation with `index.b0`; a clip cut short reads to its last whole frame; vsf's `idt/image.md` "A RAW clip" has the tables |
| the store | manifestus branch `single-copy` (`extent`), payloads/vault | single-copy extents: the table one mirrored vault value, the data written once into the `clips` window, no mirror, no read-back, its BLAKE3 at the seal |
| the kernel | process.rs, service.rs | the `clips` window (48 GiB from 24 GiB, held inside the proven-free stretch at build time) granted to the vault process; `SYS_STORE` x1 bit 6 forwards extent ops (`VOP_EXTENT`, 1 MiB a write); WriteBooster on at the window's first write; bridge `store extent` pulls one |
| the recorder | `ferros_clip/` (std) | bands coded on pinned workers, a writer staging whole blocks a UFS command at a time, the camera's pacing |
| the harness | `std/rawrec`, `tools/clip-check` | records the binned main, hashes frames as captured, decodes the clip from the flash on the device; the host decodes a pulled clip against the hashes |

### Pacing: every whole rate from 1 to 60

The binned main's frame is a whole number of lines of 6320 clocks at 2,007,040,000 Hz (3.1489 us); the clock is 2^16 5^4 7^2, so a rate with a factor 3 (24, 30, 60) is never exact, and none is exact at llp 6320. `Pacing::of(fps)` picks the nearest frame length (the open's `fll`): every rate 1 to 60 lands within 87 ppm (24/s: fll 13232, 41.666454 ms, -5 ppm; 30/s: fll 10586, 33.334422 ms, +33 ppm; 60/s: fll 5293, 16.667211 ms, +33 ppm). The sensor's 16-bit frame length ends at 65535 lines (206 ms, 4.85/s): below 5/s the sensor runs at a multiple (1/s: 5/s, keep every 5th; 2/s: 6/s every 3rd; 3/s: 6/s every 2nd; 4/s: 8/s every 2nd), and the recorder keeps a frame when it is due by its time, so a long shutter still gives every frame. A shutter longer than 1/fps lengthens the sensor's frame itself (the driver's `Timing::plan`: lines + margin past `fll` stretch the frame, then the shift), which is the owner's max(1/fps, shutter).

**Every camera (2026-11-21).** The other three readouts run other lines, so the main's pacing gave them other rates (60 asked: the ultrawide 29, the front 18, the tele 31). `Pacing::of_line(fps, Line::of_ps(line_ps, fll_least))` paces any readout from its list section (`Line::of_list(list, id, mode)` reads it), refusing a rate whose nearest frame is shorter than `fll_least` (the rows and 128 lines of blanking); `Line::fps_most()` says the fastest. On the Pro, whole binned frames: the main 60, the ultrawide 48, the tele 50 (its sensor pads its frame to about 3267 lines: `Cam::blank_least` 250), the front 33 a second; each rate lands within half a line (the ultrawide's 6.56 us is 154 ppm of a 47/s frame). `Pacing::of` is the main's exact clock as before (`MAIN_LINE`). The GPU coder has a kernel for each binned Bayer geometry (ferros_clip build.rs: 4080 on G#1400, 4000 on G#13C0, 3840 on G#12C0, 4032 on G#13C0), each checked on valhall's emulator against `encode_band`. rawrec's +22 names the camera (record and job 8, which now says the frames the camera skipped).

### The recorder's API (ferros_clip), for Lumis

- `Pacing::of(fps)`, then `pacing.open_msg(camera, shutter_ns)`: the camera's OPEN (kind 1, the binned readout, `fll` for the rate, the write-back view). The rate is fixed at the open (the driver has no SET for it): a new rate is CLOSE and OPEN. Exposure, gain and focus are SETs as now; the frame lengthens by itself for a long shutter.
- `Recorder::start(&Camera, &Mode, Box<dyn Sink>)`: `Camera` the geometry (width, height, stride, the mosaic from the MAP header's `cfa` byte by `cfa_of_byte`, black, white) and the still's facts (make, model, sensor, lens, characterization); `Mode` the pacing, the title, the workers' core masks (one a thread), the writer's mask, the slots (coded frames waiting for the flash), the extent's blocks, the start's Eagle time; the sink `ferros::ExtentSink(Extent::new(store_slot, name))`.
- `recorder.frame(&FrameIn)` for every MAPped frame: the held buffer's rows in place and the MAP header's count, Eagle ns, exposure, gain and focus. It returns once every band is coded (the buffer may go back at the next MAP); `Taken::Recorded`, `Paced` (not due, below 5/s), `Dropped` (no slot: the flash fell behind) or `Ended` (the extent full, or a write failed).
- `recorder.stats()`: frames, dropped (no slot, or the camera's count stepping past one between frames handed over), paced, bytes coded and raw, bytes written, the longest coding: Lumis's VIDEO_FRAMES and VIDEO_DROPPED.
- `recorder.stop()`: the closing generation and the seal; `ClipInfo` (the counts, the clip's length, its seal, the writer's time, an error if any). The clip is the extent under its name in Lumis's domain; `Extent::info` reads its record, `ferros::ExtentSource` reads it back as a VSF byte source for `vsf::visual::clip::Reader`.

- `ferros::space(slot)`: the recording space in bytes (the window, free, the longest free stretch, which is what an open can get, this domain's clips and their count); the vault's read-only EOP_SPACE.
- The GPU (since 2026-11-19, nothing to change in a caller): `Recorder::start` codes a Bayer clip on the GPU when the process holds the GPU's grants (a kist's `gpu::kist_slots`, a spawn's `--use gpu --dma layers`), else on the workers; `recorder.gpu_note()` says which and why, `recorder.gpu()` the GPU's counts (`GpuStats`: frames run, the GPU's time, the time from a hand-over to its last band packed, a failure). `gpu::enable(false)` keeps a clip on the CPU; `Recorder::start_gpu` takes a device by hand (`gpu::HostDevice` on a host, for tests).
- Lumis's choices that matter, measured with std/rawrec: workers four A715s and four A510s, the writer on the A510s (on the A715s it took a coder's core: 47 of 64 frames dropped), and Lumis's own preview work thinned while recording (a frame's coding takes about 23 of its 33 ms).

### The write path and the clocks (2026-11-19, the Pro)

- **Straight to the flash.** At the open the vault process gives the recording process its extent's own SYS_BLOCK capability (`Object::Extent`: the `clips` window narrowed to the extent's blocks, so every bound of the window holds it); `Extent::write` uses it, and the clip's bytes no longer pass thru the vault process. The seal takes it back, and so does the writer's end. 736 to 792 MB/s while writing (one copy into the kernel, then the bounce, queue depth one), against 266 thru the vault.
- **The recording's own clocks**, from a clip's open to its seal or its writer's end (`EXTENTS_OPEN`, service.rs `recording`): the A715s run up to 1572 MHz (`diag clock clips <kHz>`, 0 off) and the A510s up to their top, whatever else lifts or not; `diag clock` says it. Not a cap on camera streams in general. `diag clock clipshold 1` holds the A715s at that rate rather than under it (for CPU coders at 60/s); off by default since the GPU path.
- **Measured at 30/s, a minute, lossless, four A715s and four A510s:** at the camera's 2367 MHz lift the zones reached 85 C in 37 s (stopped); capped at 1572, 1800 of 1800 frames, none dropped, 75 C at most, every frame decoded bit exact on the host against the hash of the frame the camera wrote (tools/clip-check); straight to the flash, the same, 71 C at most, 170 MB/s over the clip.
- **The first clip within about a minute of a boot or reload** may drop frames while the system starts its processes (pcie1, the modem, the stower) on the same cores; later clips are clean.

- **A clip left open** (its writer died, the power went, or its seal never came) holds its whole reservation until it is closed. The vault process's recoverer, a thread on an A510, closes every open extent no live writer holds (the kernel says, SYS_BLOCK x1 bits 3 and 5), handed one at a time at the vault's start, at every extent request and after each one it finishes: it walks the clip's frames by their headers (vsf `Reader::open_headers`: each frame's fields, `prev` and closing bytes, about 24 KiB a frame), checks the samples of the last 8 frames against their hashes from the end back (`check_tail`: a torn frame ends the clip before it; a recorder writes its frames in order, so a torn write tears the tail), writes the closing generation right after the last whole frame, and the serving thread seals the extent at that length at its next request and gives its tail back. Nothing is dropped; an extent with no readable clip is sealed empty, its first block kept.
  Never inside a request: an OPEN takes a free stretch at once (3 to 5 ms) while the recoverer works beside it; only an OPEN or DELETE of the name being recovered waits for it. Until 2026-11-19 (4d1cbdf3) the next OPEN itself did the recovery on the serving thread, reading and hashing every frame's samples: a Lumis clip killed at 2,525 frames (18 GB) held that OPEN, every vault request and the camera's thread for 89 s, the Pro reaching 89 C. Measured since (b1031): 587 frames (2.8 GB) in 557 ms (headers 287 ms, the last 8 frames 268 ms, 1237 block reads of 48 MiB); 310 frames (1.5 GB) in 730 ms, the clip then read back sealed by its index with all 310 frames decoding against their hashes (rawrec job 5).
- **The open's shutter**: the driver refuses an open whose integration does not fit its frame less 32 lines (Lumis's 67.8 ms at 30/s); `Pacing::open_msg` opens with at most that, and `open_needs_set` says when the shutter is to be SET after the first frame.

### On the GPU (2026-11-19, the 60/s work)

The owner: "60FPS lock with 2x headroom on GPU or whatever's clever so I can shoot 24 in desert heat and not blink about it."

**The split.** The band coder is cut in two (ferros_rawcodec `split_codes` and `pack_band`, byte for byte `encode_band`'s). The GPU does everything a block can do alone: one Valhall kernel (ferros_clip/src/gpu_kernel.rs, an invocation a block of 32 samples of one row, the whole frame in one run) reads the RAW10 rows, makes both residuals of every sample (Left or Avg4, and MED), the block's sums, the selection and k, every residual's code, and lays the block's codes end to end into a bit string (its body). The CPU chains each block's k code from the block before and copies the bodies' words into the band's two streams (`pack_band_bodies`), BLAKE3s the coded bands, and writes. The kernel is compiled on the host by valhall's SSA recorder (ferros_clip/build.rs) and shipped in the binary as words; the device never runs a compiler.

**Bit exact.** tests/gpu.rs runs the kernel on valhall's emulator over a Pro frame against `split_codes` and `encode_band`; tests/record.rs records with `gpu::HostDevice` (the GPU's work on the host's CPU) and compares the whole clip with the CPU path's, byte for byte; on the Pro, rawrec job 7 compares every band of a frame with `encode_band`, and recordings decode bit exact on the device (rawrec's kept copies sample for sample, and the source hashes).
Two traps found on the Pro: one invocation's stores to one address do not land in the order they were made (storing a body word at every code left about 40 of 393k blocks a frame missing their last codes, different blocks each run: each word is now stored once, whole, the partial stores going to a scratch word in the body's own cache line); and valhall's SSA recorder is one global (two tests recording at once corrupt each other: they take a lock).

**The kernel**: 2332 instructions an invocation, 64 registers, 5.9 ms a frame of 4080 x 3072 at the GPU's 890 MHz (ALU-bound: 393,216 invocations). From 4610 instructions and 9 ms: both residuals kept in registers from the one pass over the frame, a five-instruction zigzag (the residual's ten bits at the top of the word, flipped when negative, shifted down), five instructions a sample's extraction, the Left/Avg4 choice made before its residual.

**The recorder** (ferros_clip): the GPU block grown to two sets (frame, bodies, facts: 256 MiB); a frame's hand-over copies it into a free set (1.7 ms) and runs the GPU (5.9 ms), then returns; four A715 and four A510 workers pack its bands behind it (about 5 ms) and the frames go to the writer in order. With the source hash asked (Lumis's DEV_VIDEO), the workers hash the bands from the set's copy while the GPU runs, and the hand-over waits for its frame.

**Measured on the Pro** (this branch's kernel RAM-reloaded, rawrec, the binned main, lossless, four A715 and four A510 workers, the writer on the A510s, straight to the flash):

| run | frames, dropped | hand-over a frame | hottest | the battery's draw (screen on) |
|---|---|---|---|---|
| 60/s, a minute | 3600, 0 | 7.6 ms average, 9.0 most (the GPU 5.9, at most 6.5) | 71 C (52 at 5 s, 62 at 23 s, 66 at 35 s, 69 at 53 s) | 7.8 W |
| 60/s, 128 frames, source digests on | 128, 0 | 11.1 ms average, 20.2 most | 54 C | |
| 24/s, 5 minutes | 7200, 0 | 7.6 ms | 62 C at a minute, 64 at two, 65 at four, 67 at five | 5.75 W |
| 30/s, 4 minutes, GPU | 7200, 0 | 7.7 ms | 65 C at a minute, 68 at two, 70 at four | 6.19 W |
| 30/s, 4 minutes, CPU (as Lumis recorded before) | 7200, 0 | 16.9 ms, 38.4 most | 70 C at a minute, 74 at two, 78 at four and climbing | 6.5 W |
| the camera alone, 30/s, nothing coded (rawrec job 8) | | | 60 C | 4.9 W |
| idle, the deck lit | | | | 1.8 W |

Each run started at 45 to 50 C, the charger cut for the measure (`diag power`). The clips decoded bit exact on the device: the minute's four kept frames sample for sample, the digests run's 16 against their source hashes.

**Where the power goes.** Recording adds about 1.3 W over the camera alone at 30/s on the GPU path (1.6 W on the CPU); the camera stream itself is most of it: 4.9 W against 1.8 W idle, the A715s 173/256 busy at 1572 MHz with nothing coded. That load (device threads on the A715s while the camera streams) is the next thing to measure (tools/flame) for a cooler desert.

**Lumis's own path at 60/s** (2026-11-20, hand-spawned with the GPU's grants, cool starts at 45 C; lumis-rawvideo baeea3fd merged with this branch):
- With DEV_VIDEO (source digests) the hand-over used to wait for its frame's packing: 16.9 ms on average, about 10 frames a second dropped from 13 s on. Since 6f7d2269 it waits for the frame's hashes alone (taken while the GPU runs): 55 of 1757 dropped in 32 s.
- Without digests (the deck's path): 32 s at 60/s, 0 of 1759 dropped, the hand-over 8.1 ms, the GPU 6.1 ms. A whole minute: 0 dropped for about 30 s, then 200 to 420 dropped as the SoC passes 87 C (the GPU's runs stretching to 22 to 34 ms, the clusters' dividers stepping).
- The heat is Lumis's beside the recorder's: 9.7 W at the battery recording with its preview off, against rawrec's 7.8 W for the same recording (71 C at a minute). Lumis's UI made a pass for every camera frame while recording (about 54 a second, 4.3 ms draw and 8.5 ms present each, with no new preview image). Capping the A510s at 1425 MHz while recording, or packing on the A715s alone, changed neither the heat nor the drops (measured, not kept).
- A clip's first frame on the GPU path waits for its packing (7f31de4d): Lumis divides the room left by `stats().coded / frames`, and a 0 there panicked it at its first frame.
- A kernel RAM-reloaded without its carried vault (FERROS_VAULT_CAPSULE) starts the stored vault, which predates the extent list and drop: `store extent-list` then says "vault error".

**Not done yet**: the camera ring read by the GPU in place (the S2MPU fence opened to it read-only while recording) instead of the copy into the block; the bodies written compactly (each block's body touches its own cache line: about 25 MB a frame written by the GPU and read by the CPU against 4.8 MB of code); the GPU at a lower clock when the rate leaves room (the kernel's GPU clock is its top or its floor); the quad array (the GPU codes Bayer only; a quad clip stays on the CPU).

### A clip from the Pro to the host

The flashed build (raw-video-rec 63916ff9, both slots, 2026-11-19; keys/husky/images/boot_raw-video-rec_63916ff9_vaultcarried.img) carries the commands; until raw-video-rec is merged use its bridge and checker:

```
B=/mnt/Harbor/Code/ferros/target/agent-rawrec/bridge/debug/ferros-bridge
export FERROS_PORT=1-2.2
$B store extent-info <clip name> --kist /mnt/Harbor/Code/keys/husky/images/lumis-0.2.15.capsule.vsf
$B store extent <clip name> all <clip>.vsf --kist /mnt/Harbor/Code/keys/husky/images/lumis-0.2.15.capsule.vsf
/mnt/Harbor/Code/ferros/target/agent-rawrec/host/release/clip-check <clip>.vsf
$B store extent-list x --kist <capsule or lumis.bin>
$B store extent-drop <first block> x --kist <capsule or lumis.bin>
```

`extent-list` gives every extent of the clips area (first block, blocks, bytes, sealed, at, owner) with that domain's marked; `extent-drop` drops that domain's extent at a first block (a clip whose name is lost), its blocks free at once. An extent's name is a derived key the vault never stores, so these are the way to find a clip by its domain alone.

`--kist` names the domain the clip is in: a stowed Lumis's by any Lumis kist capsule (its identity is the same across versions); a hand-spawned Lumis's by the exact `lumis.bin` it was spawned from (`bridge spawn` signs a payload with the developer key and its own content hash, so each build has a domain of its own: keep the image a clip was recorded with); without `--kist` the name is a scope's (`test.rawrec.clip` for std/rawrec).
A clip's name is a value's: printable ASCII G#21 to G#7E (no spaces), 1 to 96 bytes; the kernel refuses any other and says why on the console. The pull runs about 24 MB/s over the bridge (a minute at 30/s, 10 GB, takes about 7 minutes). clip-check decodes every frame on every host core and says the frames' timing and holes; given rawrec's `test.rawrec.hashes` it checks every frame against the camera's.
**The seal's BLAKE3 is not the file's** (`store extent-info`, Lumis's "sealed <hash>"): it is the BLAKE3 of the clip's closing generation alone, the bytes from where its frames end (index.b0's `end`, vsf `clip::Reader::end`) to the end of the file, as the recorder wrote it (ferros_clip `write_clip`) or the recoverer did (payloads/vault `recover_clip`). The closing generation's index lists every frame's id (the BLAKE3 of its header and closing bytes, its header carrying the hash of its samples), and each frame names the one before it (`prev`, back to generation 0), so the seal covers the whole clip without hashing its gigabytes again; a `b3sum` of the pulled file is a different number (Lumis, b1032: seal 918ed7c2..., b3sum 09655614...).
To check a pulled clip, run clip-check on it: it opens the clip by its index (refused unless the closing generation chains to generation 0 and its first frame is the one generation 0 ends at), and decodes every frame against its own samples' hash, with rawrec's hashes or Lumis's console lines against the frames the camera wrote. To match the seal itself, BLAKE3 the file's bytes from the frames' end: `tail -c +$((END + 1)) <clip>.vsf | b3sum`, END being index.b0's `end` (clip-check does not print it yet).

### Every camera, measured (2026-11-21 night; CAMERA.md "The four cameras for the promo")

rawrec on the Pro, 32 s a clip on the GPU coder, straight to the flash: the ultrawide 30/s 960 of 960 (72 C) and 48/s 1105, 0 dropped, to the 85 C guard at 23 s; the front 30/s 960 (76 C) and 33/s 1056 (78 C); the tele 30/s 960 (73 C) and 50/s 1600 (66 C); the main 60/s 1741, 0 dropped, to the guard at 29 s. The GPU 6.1 to 7.0 ms a frame on every geometry.
- **Source digests cost 60/s**: with every frame hashed as captured (rawrec's default, Lumis's DEV_VIDEO) the main at 60/s dropped 12 a second (the hand-over 20 ms); without, none.
- **The screen off** dropped the clock policy to its dark tier under a recording (workers stopped, the GPU at its floor): 5 to 23 frames a second while dark. a9bdb503 keeps the tier out of the dark while the camera's switch is held or a clip's extent is open: 960 of 960 thru a screen off and on (RAM-reloaded; not yet flashed).

### What is not done

- **The clip is not sealed (encrypted) at rest.** Every vault value is sealed with its domain's key; an extent's blocks are the clip's plain bytes, found only thru the record in the (sealed, mirrored) table, but readable by anyone holding the raw flash. Next (the coordinator's call, 2026-11-19): sealed by default under the domain's key, a per-clip switch off, its cost at 30/s measured first.
- **Copies.** A write still goes thru the kernel's heap and the bounce at queue depth one; the next step is the UFS DMA reading the coded bands in place, completion by interrupt, and a queue. On the GPU path a frame is also copied once into the GPU block (1.7 ms; "On the GPU").
- **UNMAP**: a deleted or dropped extent's blocks go back to the clips area's free list at once, but they are not unmapped on the flash (ferros sends no UNMAP), so the device's garbage collection and WriteBooster do not know they are free (2026-11-19: 23 agents' test clips, about 44 GB, dropped without one).
- **The pull** is the bridge's 3 MiB a command thru the kernel's vault channel; a faster path (bulk USB straight from the extent) is open.

### Grading a clip (on the host)

A clip is graded on the owner's computer, never on a device: Opsin (`/mnt/Harbor/Code/opsin`, branch `raw-clips`) opens, plays and grades a clip, and exports it as CinemaDNG for other tools (owner, 2026-11-19: "VSF -> cinema DNG or TIFF or png sequence or video or whatever man"; the one foreign encoder approved, host only).
Opsin builds against vsf `raw-video` with `clip::edit_generation` and `Reader::visual` (a local vsf commit until the raw-video branch takes it).

1. **Pull it.** The extent's record gives its bytes; the pull writes them:
   ```bash
   cargo run -p ferros-bridge -- store extent-info rawrec.clip
   cargo run -p ferros-bridge -- store extent rawrec.clip <bytes> clip.vsf
   ```
   Lumis names its clips `video/<YYYYMMDD_HHMMSS_mmm>.vsf` in its own domain; rawrec's is `rawrec.clip`.
2. **Check it** against the frames as the camera wrote them (rawrec's hashes; Lumis's DEV_VIDEO run prints them on the console):
   ```bash
   tools/clip-check/target/release/clip-check clip.vsf clip.hashes.vsf
   ```
3. **Give it its camera's IDT.** A clip from ferros carries no characterization yet (`ferros_clip::Camera::characterization` is `None` in rawrec and Lumis); the IDT is a property of the sensor, appended to the clip as a generation (undo: `vsfinfo undo`):
   ```bash
   opsin --characterize clip.vsf "pixel 8 pro calibration data/derived/cam2-main.idt.vsf"
   ```
   The device's own copy is the vault's `cam.<key>.idt` (the main is `boitata`).
   Or measured on the shoot: with the colour target in frame for a moment, `Cal` in Opsin (built with `--features calibrate`) on that frame scans it and pastes the solved DSR IDT into the clip at `unit`/`relative`; or copy a profile from a still of the same camera and paste it, as for stills:
   ```bash
   opsin --copy-idt <still.vsf | still.dng | camera.idt.vsf | other clip.vsf>
   opsin --paste-idt clip.vsf
   opsin --unpaste-idt clip.vsf
   ```
   (`Ctrl+C`, `Ctrl+V`, `Ctrl+Shift+V` in the viewer.) The paste is refused when make or model differ (`--force` past it): rawrec names its clips `Google` `Pixel 8 Pro`, Lumis on Android names the main's stills `Google` `Pixel 8 Pro cam 2` and Lumis on ferros names stills and clips alike `ferros` `camera`, so today only Lumis-on-ferros stills paste onto Lumis-on-ferros clips without `--force`.
4. **Look and grade in Opsin.** `opsin clip.vsf`: `,` `.` a frame, `<` `>` a second, Home and End, Space plays by the frames' own times; exposure, HDR, crop and rotation as for a still; `V` appends the grade to the clip as a generation (no frame's bytes move); `E` a frame as JPEG.
5. **Export for Resolve.** `D` in the viewer, or:
   ```bash
   opsin --export-dng clip.vsf <out folder>
   opsin --verify-dng clip.vsf <out folder>
   ```
   One uncompressed DNG a frame in `<out folder>/<clip>/` (`<clip>_000000.dng` on), the mosaic untouched (10-bit samples as 16-bit words, the clip's CFA, black 64, white 1023), `ColorMatrix1/2`, `ForwardMatrix1/2` and `AsShotNeutral` from the IDT, `FrameRate`, `TimeCodes`, capture times, exposure, gain as ISO (100 at unity), make, model and `UniqueCameraModel`.
   The colour derivation is Opsin's `src/cinemadng.rs` module doc: `ColorMatrix = (VSF_RGB2XYZ M diag(s))^-1` normalized at D50, the same matrix at Standard A and D65 (the characterization is linear); `ForwardMatrix_L = Bradford(L to D50) ColorMatrix^-1 diag(ColorMatrix W_L)`; `AsShotNeutral` the camera's response to equal-energy light, VSF RGB's white (the Pro's main: 0.545, 1, 0.667).
   The verify reads every frame back with limbus and compares its samples with the clip's decode bit for bit.
   In Resolve the folder imports as one clip and its Camera RAW panel develops it from these tags (not yet opened in Resolve: Resolve on this host was not run).

Measured 2026-11-19, the Pro's 256-frame 30/s clip (rawrec's clip256): exported in 4.9 s on the 5950X (6.0 GB), 256 of 256 frames bit exact read back by limbus, and clip-check 256 of 256 bit exact against the camera's frames, so every DNG holds exactly the samples the sensor gave; exiftool -validate OK; RawTherapee develops a frame with the hue Opsin renders the clip in.

**A Resolve plugin that reads VSF clips directly: not possible today.** Resolve's only codec plugin SDK (`/opt/resolve/Developer/CodecPlugin`, the "IO Encode Plugin SDK", Studio only) is encode only: its messages are codec init, process data and container add track and write, for the Deliver page; there is no reader or decoder message, so a plugin cannot add an input format or feed the Camera RAW panel.
Blackmagic RAW's SDK decodes BRAW only.
The other route is an OpenFX generator (`/opt/resolve/Developer/OpenFX`, free and Studio): a plugin with a clip path parameter that decodes the frame for the timeline's time and debayers it to scene-linear RGB itself; it would work, but it is not a RAW clip in Resolve (no Camera RAW panel, no media pool clip with its own timecode, the plugin's own debayer).
So the DNG sequence is the RAW route into Resolve.

## The format decision (the owner's)

Nothing below is decided; each is a new format or a new place under Rule -1.
1. **The codec's bitstream is a new encoding.** It needs a name and a VSF type for its payload (a frame, or a band of 2^6 rows, as a VSF binary value), and the parameters it was coded with (predictor set, block 2^5, escape 2^4, band rows, two streams) recorded beside it, so a decoder written years later reads it from the file alone.
2. **The clip as one complete VSF file**: a header (camera, readout, CFA and its phase, black 64, white 1023, the noise model g and r when the visually lossless mode is used, the frame rate), then a section a frame (its Eagle time, exposure, gain, focus, the bands' offsets, its BLAKE3), the provenance hash over the whole. Written while recording, its hash and field count known only at the end: the header gets a fixed reservation filled last, or the clip is a run of complete frame-files plus a closing index file. Which one is the owner's call.
3. **Where it lives**: one copy in a recording region (the vault keeps the index and the hashes), or mirrored after the take (a background copy once the clip ends, the device idle): the owner's single-copy versus write-verify-then-mirror trade.
4. **Lossless only, or the visually lossless mode as a per-clip option** with its g, r and s in the header.
5. **RAW10 kept as is** (the coder takes it packed and gives back 10-bit samples); a grade-time decoder on the host (tools/raw-codec `decode_band`) unpacks to whatever the grading tool takes.

## To repeat

- Build (from a copy, never the worktree: target/agent-rawvideo/build.sh `kernel`, `std rawbench`, a payload's package name), RAM-reload the kernel, then `ferros-bridge spawn rawbench.bin <hex> --name rawbench --blocks rawbench`:
  - `0000400000` a minute of queue-1 writes; `0100200000` 32 s of reads; `04002000000803` queued writes, depth 8, 8 commands a call (keep the calls short: the lock); `05..` queued reads; `diag "ufs link wb on"` first for the cache.
- The camera and the codec on the device (`--use camera`, any kernel): `0300000000001000050404` binned, 32 ms, 8x, the codec four runs on each core set; `0300000000021000050404` the 1:1; `03000000000010000504003C1E` the sustained 30/s run (+11 seconds, +12 the rate). Hold the clocks with `diag "dvfs policy off"` and `diag "dvfs set 3 <kHz>"`, put the policy back after, and watch `diag thermal` (target/agent-rawvideo/guard.sh kills the run past 85 C).
- The codec on the host: tools/raw-codec, `raw-codec report <frames list> <out>`, `raw-codec bench <file> <w> <h> <stride> <period> <runs>`.
