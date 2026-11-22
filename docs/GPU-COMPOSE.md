# GPU-COMPOSE: the compositor on the GPU

Status: design for the owner, 2026-11-21, written against forge 15813c55 (the reversal's commit) with be536647's measurements; built the same night, steps 0 to 4 on the host and step 0 measured on the Pro (section 6, "Built").
The owner's rule for the display: the knobs and the ideal design first, then the build (memory display_driver_design_first).
Everything not quoted from the owner is a proposal.

**Decided (owner, 2026-11-21, GPU-KISTS open question 13: "Reverse, yes.")**
- The GPU composites full frames, scrolling and any damage above the measured crossover; NEON keeps small damage below it (about 2^14 to 2^15 pixels at held clocks, 2^11 to 2^12 under the policy). The line comes from measurement, never a picked constant.
- The noise needs no match between the two sides: "If it's close, literally nobody will notice. if it flickers, I'll probably like it." The GPU uses the parallel form (lowbias32 hashes and a segmented scan, payloads/compbench noise.rs and kernel.rs `record_par`); fluor's CPU `noise_under` stays for NEON's patches.
- The condition from the start: "not sacrifice security" (compbench main.rs, the owner's words that opened the question).

This reverses 2026-11-05 ("the GPU makes layers; `under` and the present stay on NEON", COMPOSITOR.md "Not used"). GPU.md's earlier finding that the present on the GPU was 3.5 times slower was the old present (light, ODT, code, dither, pack at 92 instructions a pixel); since 2026-11-12 the canvas is baked and the present is the dither and the pack alone, and since 2026-11-21 the noise is parallel.

## The numbers this rests on

The Pro, 1344 × 2992 (4.02 Mpx), a frame of chrome, a pane read at an offset and fluor's noise under what is not opaque (payloads/compbench, GPU-KISTS section 3 "Workgroups and the parallel noise"). GPU times are the GPU's own, held clocks (A715 2367, A510 1704, MIF 3744, DSU 1800, X3 2914, GPU 890 MHz):

| Stage (whole frame, a workgroup of 2^7 a half-row) | GPU ms |
|---|---|
| the noise alone | 3.35 |
| the compose (noise, chrome, pane, `under`) | 3.93 |
| the compose and the bake | 7.35 |
| the compose, the bake, the dither and the pack into all four phases | 8.83 |
| the floor: a pane copied, 8 bytes read and 8 written a pixel | 1.95 |

| Whole frame, sustained up to 120 a second | held: frames/s | held: mJ a frame above idle | policy: frames/s | policy: mJ a frame |
|---|---|---|---|---|
| NEON (four A715s make and bake, X3 + three A715s present) | 41.1 | 147 | 38.5 | 109 |
| GPU bake, NEON present (compbench mode 14) | 94.0 | 63 | 63.1 | 49 |
| GPU phases, no CPU a frame (mode 15) | 108.2 | 43 | 91.7 | 36 |

Small damage (a rect walked out from the noise's axis): the parallel clip kernel's GPU time is 56 to 290 µs up to 2^15 pixels, its submit to wake 0.17 to 1 ms (the queue's round). NEON on the X3 is cheaper below about 2^12 pixels far from the axis and 2^14 near it; the GPU wins from 2^15 at held clocks and from about 2^11 to 2^12 under the policy (the X3 at its rest clock).

The queue since END_FLUSH (gputest 0E on the Pro): an empty job one at a time 123 to 131 µs mean submit to wake (621 to 1209 the most), four in flight 31 to 34 µs a job. The clock's turn up or down: 566 µs on the kernel's core.

What today's NEON path costs the display (COMPOSITOR.md, DISPLAY-SCROLL.md): the present is 1.17 cycles a pixel on the X3; a format 0 or 1 window's bake 10.56; a whole-pane show of Photon's scroll 4 to 5 ms of wall and about 24 ms of the display's four threads.

## 0. Knobs inventory

Each knob: what it sets, what it is today or what was measured, and the proposal. "Measured" means it is set from a measurement the build makes, never picked.

| # | Knob | Today / measured | Proposal |
|---|---|---|---|
| 0 | **The crossover**: a phase's owed damage composed by NEON (`publish_app_rows`) or by a GPU job | NEON everything; the bench's line above | Measured live (section 1.6): a cost model per path from the display's own last frames, seeded by compbench. Not a pixel count: the GPU's cost goes with the rows touched (each row's whole half-row is hashed and scanned), NEON's with the rows times the walk from the axis to the rect's far column, and neither pays for noise where a front layer's tile is opaque. |
| 1 | **Workgroup size** of the compose kernel (one workgroup a half-row of 672 pixels) | measured: 2^6 3.52 ms, **2^7 3.35**, 2^8 4.72 (the noise) | 2^7, re-measured whenever the kernel changes (valhall::ssa `workgroup`) |
| 2 | **What a GPU job writes**: the phases' words (mode 15) or a baked frame NEON presents (mode 14) | mode 15: 8.83 ms GPU, 0 CPU, 43 mJ; mode 14: 7.35 ms GPU + 2.9 ms of the X3 and three A715s, 63 mJ | Mode 15. Mode 14 kept as the bench's other form, the way to 120 if 15 cannot close (knob 4). |
| 3 | **Phases a job writes** | NEON: one phase a show while moving, the other three at rest (DISPLAY-SCROLL "One phase a frame while moving"); the bench's GPU kernel wrote all four | One phase a job while moving (estimate 7.35 + 1.48 / 4 = 7.7 ms, to measure), all owed phases in one job at rest |
| 4 | **Jobs in flight**: one GPU job at a time, or the next frame's while the last one's runs | the display publishes synchronously | Two in flight. Four phases are exactly enough: one on the glass, one armed, one being written, one for the next job (section 1.5). |
| 5 | **Frame pacing**: what starts a compose | a show (op 22) publishes the next phase at once; husky starts a panel frame only when one is armed (`PANEL_HOLDS`), the 8a on every TE | Unchanged: the show starts it, newest show wins, the arm follows the job's end. No timer. |
| 6 | **Rest**: when the three phases behind are filled and the 10 Hz flips resume | a timer: no show for an eighth of a second (`last_interaction`) | Counted, not timed: N = 2^1 panel frames with no show (DISPLAY-SCROLL knob 14, not built), then one GPU job for every owed phase |
| 7 | **The bake on the GPU**: how a G2t16 pixel (the noise, a format 0 or 1 window) becomes a panel code | compbench: the display's tables (light: 2^16 f16 entries, 128 KiB; code: 2^12 + 2 f32) gathered three times a pixel, 3.42 ms of the 7.35 | Tables as now for formats 0 and 1; for the noise, its colour is the base plus small steps, so the bake linearised at the base (a 3 × 3 matrix a frame, nine FMAs a pixel, f16 pairs) is a candidate to measure against the tables by eye. A brightness change rewrites the tables or the matrix in the display's block: params, no compile (as Mahere's bake). |
| 8 | **The noise: generated or kept** | the bench generates it every frame (3.35 ms); fluor generates it in the app every frame it is drawn | Generated each frame (no 32 MiB layer kept, nothing to re-bake at a brightness change, and the `frame` uniform gives the twinkle for free if the owner wants it). Kept as a baked ring only if the one-phase budget does not close (section 5, question 3). |
| 9 | **The twinkle** (`slot::FRAME`, noise.rs `row_twinkle`) | 0, a still noise | 0. The owner may like it ("if it flickers, I'll probably like it"); it costs nothing on the GPU, and NEON patches cannot follow it without a seam. |
| 10 | **Coverage tiles** (COMPOSITOR rule 2; ferros_gpuq `coverage`) | the form exists (a byte a 64 × 64 tile, empty / partial / opaque and a changed bit); the display reads none | 2^6 pixel tiles. In the kernel the test is per warp (16 lanes of one row lie in one tile), so a skip is a uniform branch, never per pixel. |
| 11 | **The GPU's clock** | by events: up when a job is queued, down at the second display frame in a row with nothing run; the cores kept on across a frame while up | Unchanged. A compose every frame while moving keeps it up; at rest it goes down after two quiet frames. Proposed: the touch server's finger-down as one more "up" event, so a pan's first frame does not pay the turn (566 µs) and the cores' power-up; to measure. |
| 12 | **The CPU and MIF clocks** | compbench held MIF at 3744; under the policy the profile stays at rest while the GPU composes | Nothing asked of them. The crossover follows them (knob 0 is measured per clock state), and under the policy the GPU wins earlier because the X3 sits at its rest clock. |
| 13 | **Screen off** | the display neither flips nor publishes while dark; shows are taken | No compose job while dark; the display's queue stays open and its slot bound (its tables are cheap to keep); the clock goes down with the dark profile. A show while dark is kept the way a window behind keeps one: its place and format, nothing published (section 1.7). |
| 14 | **The display's GPU slot and priority** | none: the display is no GPU client; system clients 15, kists 8 | Slot 0 (CSG 0, AS1) reserved for the display, never given to another client; priority 15 for the display alone, 14 for the other system clients (ahau, wallgen), 8 for kists |
| 15 | **Kist GPU jobs while a compose runs** | clients' groups run at once, the firmware choosing by priority; whether it preempts a running kist job is unmeasured (GPU-KISTS 1.5) | Measure first (section 4 step 0). If the compose's lateness under a hostile kist passes a frame, the service holds new kist jobs from going in flight while the display has one queued or running (the compose window). |
| 16 | **The guard** for the display's jobs | 2^10 ms (a system client's) | Unchanged: it only ends a hung kernel; the firmware's progress timer is the first line |
| 17 | **The phases' mapping on the GPU** | written by the CPU, Normal non-cacheable (cachemap `DISPLAY_PHASES`, 64 MiB at G#8C00_0000) | Coherent like the bench's in-block phases, END_FLUSH cleaning them before `DONE`; the DPU does not snoop, so step 2's gate reads the phases back over the bridge. Non-cacheable on the GPU is the other setting to measure. |
| 18 | **The phases in the GPU's fence** | not admitted (s2mpu `fence_gpu` admits gpu-mem and the frames of surfaces and blocks) | Admitted read-write while the display's GPU view is bound, shut out when its epoch ends (section 2.3) |
| 19 | **Formats the GPU composes** | the display takes 0 (working), 1 (G2T16, baked per row by NEON) and 2 (BAKED) | All three; apps hand BAKED (they do since 2026-11-12, op 65), so the GPU's work on an app's pixels is arithmetic only (section 2.7, "timing") |
| 20 | **The display's GPU block** | none | 64 MiB (section 1.2 lists what lives there) |
| 21 | **The hit map** (a u16 a pixel, 7.7 MiB) | written by the publish's rows only when the layout changed (`HITS_STALE`) | Stays the CPU's, written as now when the layout changes; it costs 4.4 ms once per layout change, not a frame |
| 22 | **The deck** (the map's canvas, the wallpaper, a region's text, flights) | NEON (`publish_band`, flight frames 10 to 15 ms) | NEON until apps are done on the GPU (section 4 step 6), then the GPU, with the canvas and the sprites admitted to the fence |
| 23 | **Sustained 120 and heat** | the SoC reached 82 to 97 C with the GPU at 890 MHz and every core busy (GPU-KISTS step 4) | Not this design's knob: THERMAL.md's. Mode 15 at 120 a second is about 5.2 W above idle (43 mJ × 120) at held clocks. |

## 1. The ideal pipeline

### 1.0 Who does what

| Who | What it does a frame |
|---|---|
| An app (fluor's ferros host) | renders the layers that changed, bakes them in its process (op 65's bake, as today), fills their coverage bytes, and shows them: one message naming its layers and what changed (section 3) |
| The display, receive thread | validates the show (whose surfaces, which level: "Whose surface", "Whose level"), queues it for the render thread |
| The display, render thread (the X3) | picks the phase to write, owes the change to every phase, decides NEON or GPU for that phase's owed damage, writes the frame's stack into its queue's record and submits, or runs the NEON patch on itself and the three publish workers |
| The display, landing thread (new) | asleep on its queue's `DONE` word; at each job's end it hands the phase back to the render thread (ring the render thread's bell), so the arm and the mailbox's answer follow at once |
| The GPU service | as built: the job's words into the display's own stream, its doorbell, the end landed by the events thread, `DONE` stored and woken |
| The GPU | the compose kernel over the owed rows: every layer front to back, the noise under what is still clear, the bake where a layer is not baked, the dither, the pack, into phase k |
| The DPU | scans the armed phase, as today; it reads only `display-fb` and `display-phases` (s2mpu `fence_dpu`) |
| The kernel | stamps the display's GPU calls with what it may map (section 2.1), holds released surfaces until the display's GPU view lets them go (2.2), admits the phases to the GPU's fence (2.3) |

The display is a GPU client like any other: its own epoch, its own slot (pinned), its own address space, a queue in its own block, kernels recorded on the host and shipped as words in its capsule (as compbench's build.rs and ferros_clip's band coder do; the device compiles nothing).

### 1.1 What the display takes from apps: layers, not a flattened frame

A window is a short stack of layers, frontmost first (DISPLAY-SCROLL section 5, "B+", which the owner asked for on 2026-11-17: "I'd assume we layer it so we can wrap the background and keep the content separate so it's one merge pass?"):
- **Pixel layers**: a surface (or a mailbox's buffer), BAKED (the panel's codes, premultiplied, transparency in bits 49-62, bit 63 see-through), a screen rect, flags (opaque, fixed), and for a pane taller than the screen a ring: its first row and height in the source and its origin, screen row y reading source row `(y − y0 + origin) mod H`. The chrome (fluor's `DefaultChrome`, partial α and darkness on its antialiased edges) is a fixed pixel layer; the content (Photon's list) is a ringed pixel layer, its strip rendered ahead of the viewport.
- **The background as parameters**, never pixels: the noise's axis column, its scroll (texture row offset), its base colour, its shimmer, whether full screen, its clip, its frame (the twinkle). Opaque by construction, so the walk ends there.
- **Each layer's coverage**: a byte a 64 × 64 tile in its own coordinates, past the pixels in its surface (ferros_gpuq `coverage`).

The system's own layers are the display's, in its own GPU block, frontmost of all: the glass's plan (per row the masked columns, the hole, and the few antialiased pixels of the edge, `plan_glass`), the bars (the console's circles at the top and the helm's wedges at the bottom, today `BAR_W` in the display's heap, moved into its block so the GPU reads what the console's worker writes, coherent, no copy).

An app that hands one surface (op 22 as today) is a window of one pixel layer and no noise: every app keeps working while fluor's host moves to layers.

### 1.2 The display's GPU context

Its address space maps (section 2.0 has the security view):
- its GPU block, read-write, 64 MiB: the kernels' words (2^16 bytes of room a kernel, as compbench sets aside), its queue (a header page and 2^2 records of 1 KiB), the workgroup storage's room, the frame stacks (one per queue record, so a record's stack is never rewritten while its job may read it), the bake's tables and ODT, the blue noise (16 × 16 bytes), the noise's seed table (a u64 a texture row, 2992 rows, since the GPU has no 64-bit multiply), the glass's plan, the bars and their back buffer (part of the 15 MiB the bars hold in the heap today);
- its code pages, read-only and the only executable pages (the service's slot code region, as for every client);
- the four phases, read-write, never executable;
- the surfaces of the windows it composes, read-only, never executable;
- nothing else: no other client's block, no surface of a window it does not compose, none of gpu-mem's firmware, tables or interface.

### 1.3 One frame, step by step (an app moving)

0. The app renders what changed (with a strip, a pure scroll renders nothing), bakes it, fills its coverage, and shows: its layers, the changed rects, the buffers.
1. The receive thread checks each named surface is the caller's (SYS_CALLER) and each ring and rect lies inside its surface (external input: validated here, so the kernel's reads stay inside the layer). The call is held: a mailbox's buffer is answered only once no job reads it (step 6).
2. The render thread owes the change to all four phases (`PHASE_DIRTY`) and picks phase k: the one after the armed one, not on the glass, not armed, not being written (section 1.5).
3. It takes phase k's owed rect and asks the cost model (section 1.6): NEON or GPU.
4. GPU: it writes the frame's stack into the record's stack area (the windows in front-to-back order from `layout_build`'s spans, each layer's GPU address in the display's space, rect, ring, format, mode, coverage address; the noise's parameters; phase k's address and its `TURN[k]`; the owed rect), the uniforms point at it, `SUBMIT` = n + 1 and a wake. Phase k is now the GPU's (seq n).
5. The GPU runs the compose kernel (1.4) over the owed rows; END_FLUSH cleans and invalidates its L2 and load-store caches; the service stores `DONE`.
6. The landing thread wakes on `DONE`, rings the render thread, which settles the frame on the glass, points the layer at phase k (`program_layer`), arms it, and answers the show's call: the buffer shown before is the app's to draw into again.
7. While job n runs, the next show may already go to the spare phase as job n + 1 (knob 4), so the GPU never idles between frames in a pan; the CPU spends a frame only the stack's writing and two thread wakes.

### 1.4 The compose kernel

One kernel for whole frames and damage alike (compbench's clip form): a grid of one workgroup a half-row for each owed row and each side of the axis the owed rect touches (both sides for a whole frame: 2 × 2992 workgroups), the owed columns its stores' bounds. Per workgroup, values uniform across it are worked out once: the row, each layer's source row thru its ring, the row's glass plan.

Per row:
0. **The noise's scan** (only where a window's background shows on this row): each invocation hashes its run of ⌈672 / 2^7⌉ pixels (two lowbias32 of the texture row, side and distance from the axis), the runs' sums joined by a Hillis-Steele scan in workgroup memory (two barriers a step), each pixel's noise colour staged in workgroup memory.
1. **Front to back**, neighbouring invocations on neighbouring pixels, the accumulator empty: the glass (masked columns opaque black, the hole's knockout, the edge's antialiased pixels with their coverage and hairline), the bars, then the windows by level and order, each layer of a window in its order. For each layer: its coverage tile (per warp: empty skips the layer, opaque copies and ends), else its pixel `under` the accumulator (`under_pm` on baked codes: the codes premultiplied, the transparency multiplied). Where the accumulator is opaque nothing behind is read.
2. **The noise under** what is still clear, baked (knob 7), then the transparency left over black.
3. **The present**: the code plus its threshold (`BLUE[y & 15][x & 15] ^ TURN[k]`), the high byte, packed 10:10:10:2 with the channel mask (`neon::present_app16`'s arithmetic, integer, so the GPU's words can equal NEON's exactly where no noise is involved), stored into phase k inside the owed columns.

No Porter-Duff over anywhere, no `1 − α` between layers: `under` only, front to back, as AGENT.md "Compositing" and COMPOSITOR.md rule 3 hold. Nothing here is a painter's write: every phase word is written once by one job (feedback_painters_needs_proof).

### 1.5 The phases: never two writers on a frame

Each phase k has one state, held by the render thread alone: on the glass, armed, free, NEON's, or the GPU's (seq n).
- **R0.** A phase on the glass or armed is never written (as today: `CUR_PHASE`, `pipe.settle`).
- **R1.** A GPU job takes phase k only when k is free; k is the GPU's until `DONE` ≥ n.
- **R2.** A NEON patch takes phase k only when k is free; the publish is synchronous on the render thread and its workers, so k is free again when it returns.
- **R3.** Phase k is armed only when free (after its job landed or its patch returned).
- **R4.** A job takes everything phase k owes at its submit (its `PHASE_DIRTY` cleared then); a show that lands while it runs owes its change to k again, as to the others, so nothing is lost.
- **R5.** A NEON patch to phase j while a GPU job writes phase k ≠ j is allowed: the phases are disjoint 16 MiB buffers, never sharing a line, and non-cacheable on the CPU.

So a phase is written by one writer at a time, ordered on the completion doorbell; no lock is taken on the frame path, no timer, no poll.
At 120 the four phases cycle as on the glass, armed, written by job n, written by job n + 1.

### 1.6 Damage and the crossover, per frame

The render thread keeps, per path and per clock state (the GPU's clock up or down, the X3's point), a small cost model fitted to its own measured frames:
- GPU: the queue's round (the job's `submitted` to the landing's wake) plus a cost per owed row and side;
- NEON: a cost per pixel composed plus a cost per row times the walk from the axis to the rect's far column (fluor's `noise_under` is a serial walk from the axis), and nothing for rows whose owed columns are opaque in a front layer's tiles.

Seeded from compbench (the numbers above), updated from every frame either path runs (the GPU's own stamps, the publish's PMU cycles), so the line follows the clocks, the heat and the contention, and nobody picks it. The decision is one compare a frame, never per pixel.

Which way a patch goes can change from frame to frame; where it changes over the noise the two generators meet at the patch's edge: the same statistics (every mean, spread and step within 0.84 %), not the same values. The owner's ruling covers it.

### 1.7 At rest, screen off, no GPU

- **At rest**: after N = 2^1 panel frames with no show (knob 6), one GPU job writes every owed phase, then the 10 Hz flips resume (`flip_due` waits for the next phase to be current, as now). The GPU's clock goes down after two quiet frames.
- **Screen off**: no compose job; shows taken and kept as a window behind keeps them (place and format), nothing published; the display's slot and queue kept.
- **No GPU service** (the GPU did not come to its reset at boot, as after a jump that found it powered down; before it starts at boot; while it restarts after its own end, `E_SERVICE`): NEON composes everything, the path that exists today (whole frames at 16 ms and up of four A715s). The compositor is the screen itself, so this is its only path then, not a fallback in the wallpaper's sense (section 5, question 6).
- **The hit map's development view** (the fourth phase painted from the map, `hit::on_layer`): stays NEON's; the GPU's jobs leave that phase out while the view is on, as `publish_app_rows` does.
- **The screen capture** (op 30, op 84): stays NEON's, composed on the CPU into the display's own buffer and unbaked, as now.

### 1.8 The budget at 120

8.33 ms a frame. Estimate for mode 15 with one phase a job: 7.7 ms of GPU at held clocks (to measure), plus the queue's round (tens of µs pipelined). Under the policy the whole-frame mode 15 took 10.9 ms busy a frame, so 120 there needs the kernel cuts GPU-KISTS lists as not yet built: v2i16 lanes for the three channels, keeping the steps rather than the hashes across the scan, the bake's tables in f16 lanes (or the linearised noise bake, knob 7). A pure scroll inside a strip changes every row, so it is a whole-frame job each frame; nothing in the app's render is on that frame.

## 2. Security: the owner's condition

### 2.0 What the display's GPU space maps

| Memory | In the display's GPU space | Already the display's on the CPU |
|---|---|---|
| its GPU block | read-write, never executable | yes (its own block) |
| its kernels' code | read-only, executable | its capsule's |
| the four phases | read-write, never executable | yes (`display-phases`, it writes them today) |
| a composed window's surfaces (both of a mailbox) | **read-only**, never executable | yes, read-write (it holds the whole surfaces window) |
| any other surface, any client's block, the camera, gpu-mem | not mapped | (surfaces yes; the rest no) |

The display's GPU context gets nothing the display process does not hold already, and less: no write to any app's surface. The one new thing is a bus master (the GPU) writing the phases, which only the display's space maps.

### 2.1 A read-only stamp

Today `gpu_stamp` names every surface and block a caller holds, read-write, and stamps a holder of the whole surfaces window (the display, init, the GPU service) with none: "no client's memory is its to have the GPU write". That rule stays. Added, for the caller that holds both the whole surfaces window and `display-phases` (the display alone):
- **The phases**, as one region (base, size; they are fixed memory, not frames).
- **A read-only list**: the surfaces the display names when it looks at its memory again (op 5, with `[n][slot, generation]…`: the windows it composes), each checked by the kernel to be held now under that generation, stamped with its frames under a flag bit of its own (bit 3, "read-only regions follow", after the frames). The service maps those with `PAGE_RO | NX` and never otherwise.
- An older service reading the new stamp finds the read-only list where it expects the caller's request, takes it for a malformed request and refuses it: it can never map them read-write.

The display calls op 5 when its window set changes (`layout_build`, the slots' bell), never a frame; the service makes its tables again in place (two pools a slot, as since 2026-11-20).

### 2.2 Surfaces held from reuse while a compose may read them

The danger: an app ends, its surface's frames are wiped and given to the next owner while a compose job still reads them. The job would show the next owner's pixels on the glass for a frame, or, once the frames are shut out of the fence, take a bus fault the GPU may not ride out.
The rule, the camera's (GPU-KISTS "The camera ring on the GPU") applied to the display's view:
0. The kernel marks a surface viewed when it stamps it read-only for the display's epoch.
1. A viewed surface given back (its owner's end, a kill, a relaunch) keeps its frames held and admitted, and the slots' bell rings the display, which drops the window (its reaper, as now) and calls op 5 without it.
2. The service makes the display's tables again without it only after every display job submitted before that call has landed (`DONE` ≥ the last), installs them with LOCK and FLUSH_PT (the GPU's TLB and walk caches dropped), and tells the kernel (SYS_DMA on `arena`, a new x1, as x1 = 9 settles the camera).
3. Only then does the kernel wipe the frames, shut them out of the fence, and return them to the pool.
4. The backstop: if the display ends, its epoch's view goes with its group (ended, its tables torn, the caches flushed with FLUSH_CACHES as after any group ended mid-job), then the frames go.

Proof that no compose reads a reused frame: a frame returns to the pool only after (a) no table of the display's maps it (step 2's install), (b) no job that could have read it is in flight (step 2's wait), (c) the GPU's TLB and walk caches were dropped (LOCK, FLUSH_PT), (d) no line of it is in the GPU's L2 from the display (END_FLUSH ended each of those jobs, and the display never writes a surface, so no dirty line of it exists). A job submitted after step 1 that still named the surface reads translation faults in the display's own space: a frame dropped, loud, never another owner's pixels.

### 2.3 The phases in the fence

The GPU's S2MPU admits the phases (G#8C00_0000 to G#9000_0000, in GiB 2 beside gpu-mem, so in the fence's existing table) read-write while the display's GPU view is bound, set by the kernel at the display's first stamp that names them, shut out when the display's epoch ends (after its jobs drain) and when the service ends.
The fence is the wall around all GPU clients together, so admitting the phases is safe only because no other client's space maps them: the service maps a client only from its stamp, and the kernel names the phases only for the holder of `display-phases`. The trusted base is what it was for blocks and surfaces: the kernel's stamp, the GPU service's tables, the GPU's MMU.
The canvas and the sprites (GiB 3) are admitted the same way only from the deck's step (section 4 step 6).

### 2.4 The display's slot pinned

The service keeps seven slots; a new client takes a free one or the one used longest ago with no queue and no job (`slot_for`). Seven kists with open queues would leave no slot. So slot 0 (CSG 0, AS1, doorbell 1) is the display's alone, never given to another epoch, and its group runs at priority 15 alone (other system clients at 14, kists at 8). No number of kists can take the compositor's GPU.

### 2.5 END_FLUSH between contexts

Every job, queued, called or the service's own, ends with FLUSH_CACHE2 cleaning and invalidating the L2 and the load-store caches (`ferros_gpuq::END_FLUSH`, G#233), proven on the Pro (compbench modes 10 and 13: 425 and 683 of 2^10 jobs stale before, 0 after). So no job of the display reads a line another job left, and the phases' words are in DRAM before `DONE`, where the DPU reads them. Groups that run at once share the L2 while they run, but a line is only there because a job of a space that maps it touched it; no space but the display's maps the phases or the display's block, and no space but its owner's and the display's (read-only) maps a surface.

### 2.6 Can an app still never read another's pixels?

An app A reads memory by two paths only: its CPU mappings (its arc, its own DMA regions: its block, its surfaces) and its GPU space (built from its stamp alone: its block, its surfaces, its camera view while it holds the camera).
- Another app B's surface is in neither: surfaces are made at a spawn or `surface_ask` as the asker's own capability and never passed, and SYS_CALLER and the stamp follow the capability.
- The phases (where B's pixels end up) are in neither: no kist holds `display-phases`, and the kernel stamps them for its holder alone.
- The display's GPU block (its frame stacks, staged rows of noise) is in no stamp but the display's.
- The display's job writes only the phases and its own block; its view of B's surface is read-only, so it cannot carry B's pixels into anything A maps.
- Reuse: section 2.2. Stale lines: section 2.5.
- The only flows of B's pixels out of B are the glass and the screen capture (op 30, op 84, to the clipboard's offer by the system's gesture), the same two as today.
So the information flow is today's: the design adds no path. The display, which reads every surface already, is the one process that sees them all, as it is now.

### 2.7 What an attacker app could do

| Attack | What happens | Answer |
|---|---|---|
| Take every GPU slot | refused past the six left: slot 0 is the display's (2.4) | none needed |
| Fill the GPU with long jobs | its jobs run at priority 8 against the display's 15; one kist job lasts up to its 2^8 ms guard; whether the firmware takes the endpoints back mid-job is unmeasured | step 0 measures the compose's lateness under a hostile kist; past a frame, the compose window (knob 15). Worst case frames come late, never wrong pixels: availability, which a kist can already spend by burning the A715s the NEON publish shares today |
| Fault, or hang the GPU | its fault ends its own group and queue (its space alone is set UNMAPPED); a hang ends at its guard; a GPU-wide fatal resets the GPU (10 µs) and the MCU (115 µs), jobs in flight answered lost | the display's lost job leaves its phase owed: it submits again, or composes on NEON until the service answers (1.7). The glass keeps showing the last armed phase meanwhile |
| Show huge damage every frame | its own window costs a whole-frame compose a show, at most one a panel frame (newest show wins) | as today with NEON: an app may draw at the panel's rate |
| Name another's surface, or a ring or rect past its own | refused at the show (`not_theirs`, NOT_YOURS) or the rect checked against its surface (1.3 step 1) | the kernel's reads stay in the caller's surface; a bug that let one thru reads only read-only surfaces of visible windows or faults in the display's space |
| Put pixels in front of the system or over the attest screen | the stack's order is the display's: the glass and the bars first, then levels 2, 1, 0 (`level_not_theirs`) | unchanged by the GPU |
| Write the display's block, queue or kernels | not mapped in its space; the service's ring and descriptor pages are mapped read-only in each client's own space | none needed |
| Tear its own window | it writes a buffer the compose is reading | its own window only; the mailbox answer at the job's end (1.3 step 6) gives a well-behaved app no reason to |
| Time its own GPU jobs | it sees its own job's stamps (`gpu_ticks`, submit to start) and contention when the compose runs | it learns when frames are composed, which the frame mailbox already tells everyone ("when a picture starts on the glass is what everyone sees"), and roughly how much was composed. With app layers BAKED the compose's work on app pixels is arithmetic with no gather indexed by a pixel's value (`under`, the threshold add, the pack), so pixel values do not steer its timing or its addresses; the only value-indexed gathers are the noise's bake (public parameters) and formats 0 and 1 (the system's own windows and old apps). The attest screen (format 0, where the handle is typed) runs at boot before any kist can (kists launch from the deck after attest); if the owner wants it, the compose window can also hold kist jobs whenever a level 2 window is up. The owner's threat model is harm, not watchers (memory threat_model_harm_not_observation): no harm path is opened here. |

### 2.8 What this adds to the trusted base

Nothing new in kind. The kernel's stamp, the GPU service's tables and the GPU's MMU already keep every client's block and surfaces apart; the screen's integrity already rests on them, since a compromised service could make the GPU write any surface the display shows. What is new: the phases join the fence's admitted memory, the stamp gains a read-only list, and the kernel gains the camera's settle pattern for surfaces viewed by the display.

## 3. What fluor's ferros host must hand over

The Photon session owns fluor and asked exactly this.

### 3.0 Today

One flattened surface a frame: op 22's 50-byte form (rect, surface, level, format 2 `BAKED`, the changed rect, mode with `OPAQUE`, the mailbox's buffer, the frame's own µs, the ring `[x][y][w][h][origin]`). The app lays the noise under its own scratch every frame it draws (`noise_under`), flattens the chrome into it, and the host bakes the damage into the surface.

### 3.1 The layered show (a new op beside op 22, its number taken when built)

`[x u16][y u16][w u16][h u16]` the window's rect, `[level u8]`, `[mode u8]`, `[frame us u32]`, `[n u8]` layers frontmost first, then up to four changed rects in screen coordinates (`[n u8][x u16][y u16][w u16][h u16]…`, fluor's damage).
A pixel layer:
- `[kind u8 = 0]`
- `[source u64]`: the surface or mailbox buffer this show reads (the caller's own; "Whose surface" applies to each)
- `[format u8]`: 2, `BAKED` (0 and 1 still taken)
- `[flags u8]`: bit 0 opaque, bit 1 fixed (no ring)
- `[x u16][y u16][w u16][h u16]`: the screen rect it covers
- `[src y u16][src h u16][origin u16]`: its ring in the source (h 0: none)
- `[tiles u32]`: where its coverage bytes lie in its surface past the pixels (ferros_gpuq `coverage::AT`), 0 for none (every tile partial)

A background layer (the noise), always last and opaque:
- `[kind u8 = 1]`
- `[x u16][y u16][w u16][h u16]`: the clip it covers
- `[axis u16]`: the mirror column (the frame's centre, or Settings' split)
- `[scroll i32]`: the texture's row offset (fluor `noise_under`'s `scroll`)
- `[base u32]`: the colour, 8-bit RGB as fluor's API authors it (`BG_BASE` or the theme's)
- `[shimmer u16]`, `[full u8]`: as `background_noise` takes them
- `[frame u32]`: the twinkle's frame (0 still)
- `[generator u8]`: 1 the parallel form (lowbias32 + the segmented scan); the display's NEON patch uses fluor's `noise_under` with the same parameters

A few hundred bytes, inside a message slot (4096). Every show states every layer, so the display holds nothing of a window the host did not just say (as the ring's rule on op 22).

### 3.2 What the host does differently

- **The noise is not drawn in the app on ferros.** `Context` takes it as a call (DISPLAY-SCROLL's `ctx.lay_noise(axis, scroll, base, shimmer, clip)`): the ferros host records the parameters into the show; Android and the desktop host lay it under the frame as `noise_under` does today, the same pixels. Photon's two calls change (its noise and its chrome flatten), nothing else.
- **The chrome is its own layer** (`ctx.lay_chrome(&chrome)`): baked into its own surface when it changes, its coverage filled; not flattened into the content.
- **The content is a ringed strip**: rendered by rows in strip coordinates, baked into the ring's rows (option B's ring, built, extended to a strip taller than the pane), so a scroll inside what is rendered is a new origin and no render.
- **Coverage**: after baking a rect, the host classes the tiles it touched (empty, partial, opaque, changed) on the CPU from the α it just wrote: a byte for each 64 × 64 tile, written past the pixels.
- **Surfaces**: chrome and content each double-buffered for the mailbox is four, the most `surface_ask` gives a process; the noise takes none (B+ had baked it into a slot of its own).

### 3.3 What stays the same

- The bake: op 65 and `EV_BAKE`, every layer baked in the app's process (`PanelPixel::to_baked`), shown `BAKED`.
- The mailbox: show a buffer, draw into the other one once the call is answered; the answer now comes when the compose job reading the shown buffer has ended, not when it was submitted.
- Events (op 62: front, behind, keys, text, the safe area `EV_SAFE`, the inset), the keyboard (op 68), levels and modes, the display started again (op 80's rules), touch and op 27 (the display's hit map picks the window, fluor's `HitId` map the widget).
- An app that hands one surface by op 22 is one pixel layer: Mahere's and Lumis's GPU frames (`own_surface`), apps on an older fluor, every other host.
- Op 5's feature byte (14) gains a bit, "layers", as bit 0 says "rings": the host sends the layered show only to a display that says it reads it.

### 3.4 What fluor must expose for the GPU's noise

The generator's constants the GPU kernel reproduces (compbench layout.rs copies them because fluor's are private: `STIR`, `STEP`, `DOWN`, `SPECKLE_BELOW`, `widen_mask`, `widen_base`, `seed`), public, so the display records its kernel from fluor's own numbers. The parallel generator itself is the Photon session's (noise.rs's scalar reference); whether fluor's CPU side ever takes it is that session's call, not this design's.

## 4. Migration

Each step leaves the screen working, is testable alone, and is gated by a measurement on the Pro before the next.

0. **Measure in the display's conditions** (compbench, no display change): one phase a job against four; two jobs in flight; the linearised noise bake against the tables, by eye; the cost model's terms (the queue's round, a row's cost, NEON's walk cost) at held clocks and under the policy; and the hostile kist: a gputest mode running endless ALU kernels at priority 8 while compbench composes at 15.
   Gate: the compose's submit-to-done at its 99th percentile under the hostile kist, and whether one phase a job closes 8.33 ms. Decides knobs 3, 4, 7 and 15.
1. **The kernel and the service**: the read-only stamp, the phases region in the display's stamp and the fence, the viewed surfaces' settle, slot 0 reserved, priorities 15 / 14 / 8.
   Test (a test client holding the display's grants, before the display uses it): a store into a viewed surface faults as a permission fault in its own space; a kist's store at the phases' GPU address faults as a translation fault; a window's owner killed with a compose job in flight: the frames go back to the pool only after the service's settle line, never before, zero words of the next owner read; seven kists with open queues, the display still gets slot 0; `diag "gpu fence"` clean after each.
2. **Whole frames of today's windows on the GPU** (op 22, single surfaces, `BAKED`, no noise in the display yet), behind a Developer checkbox ("GPU compose").
   Gate: the phases read back over the bridge equal NEON's publish of the same frame word for word (`under`, the threshold add and the pack are integer: no noise is involved yet); a whole-screen show's CPU time in the display about 0 (it was 4 to 5 ms of wall and about 24 ms of thread time a Photon scroll show); Mahere's pan and Photon's scroll frames a second; energy a frame by `diag power`.
3. **The crossover live** (NEON patches, GPU frames, the phase states of 1.5).
   Gate: a caret's blink and a hover stay NEON, a scroll and a pan go GPU; a counter of any phase written by two writers stays 0 over a mixed run; each frame costs within a few percent of the cheaper path's measured cost.
4. **Layers** (the display's layered op; fluor's host on a branch taking `lay_noise` and `lay_chrome`; the noise generated by the GPU and by NEON's `noise_under` for patches).
   Gate: Photon's scroll on the Pro at 120 (frames 8.33 ms apart) with no render on a pure scroll frame inside the strip; the noise's statistics within 1 % of fluor's (0.84 % measured); the owner's look at the seam of a NEON patch over the GPU's noise.
5. **120 sustained**: one phase a job while moving, two in flight, rest counted in panel frames.
   Gate: 16 s of Photon's scroll and of Mahere's pan at 120 with no frame missed; mJ a frame at or under the bench's (43 held, 36 under the policy); the SoC's temperature over the run.
6. **The deck**: the map's canvas, the wallpaper, a region's text and the flights composed on the GPU (canvas and sprites admitted while bound).
   Gate: a flight frame under 8.33 ms (10 to 15 ms today), the deck's look unchanged by eye and by capture.
7. **The edges**: screen off, after a jump with no service, the service restarted under the display.
   Gate: power at rest within the ±50 mW of today's; frames on the glass after a jump (NEON alone); a service restarted mid-pan costs frames, not wrong pixels, and the GPU path comes back by itself.

## 5. Open questions for the owner

0. **One phase a GPU job while moving, the owed ones at rest?** (knob 3) Recommend yes: it is NEON's rule already (DISPLAY-SCROLL, your "we only need to render one phase as we go"), and it is what brings the whole-frame job under 8.33 ms by estimate (7.7 against 8.83 ms).
1. **The GPU writes the phases itself (mode 15), or bakes a frame NEON presents (mode 14)?** Recommend mode 15: no CPU a frame, 43 mJ a frame against 63 at held clocks (36 against 49 under the policy). Mode 14 stays the route to 120 if 15 cannot close: its 2.9 ms present overlaps the next GPU bake.
2. **The noise's bake: exact tables, or linearised at its base colour?** The tables are 3.42 ms of a whole frame; a 3 × 3 matrix a frame (nine FMAs, f16 pairs) is a candidate. Recommend measuring both and choosing by your eye, as the noise needs no match.
3. **The noise generated each frame, or kept as a baked ring?** Generating is 3.35 ms of a whole frame and holds no memory; a kept ring is about 1 ms of reads but 32 MiB a background, baked again at every brightness change, and no twinkle. Recommend generated; kept only if 120 does not close otherwise.
4. **The twinkle** (the noise's `frame`): off as now, or on? Free on the GPU; NEON patches would not follow it and their edges would show. Recommend off unless you want to look at it.
5. **Kist GPU work while the compositor runs**: priority alone, or the compose window (no new kist job goes in flight while the display has one queued or running)? Recommend measuring first (step 0); the window only if the compose comes later than a frame under a hostile kist. It costs kists a little latency, never their work.
6. **No GPU service** (after a jump, before it starts, while it restarts): NEON composes everything, or black? Recommend NEON. Your "no fallbacks" was for the wallpaper; the compositor is the screen itself, the attest screen included, and its NEON path exists and is measured.
7. **The display's slot pinned with priority 15 alone, other system clients at 14?** Recommend yes (section 2.4).
8. **The phases in the GPU's fence only while the display's view is bound, or always?** Recommend while bound (section 2.3).
9. **The crossover live-fitted per clock state, or a table from the bench?** Recommend live, seeded by compbench: the line then follows the clocks, heat and contention by itself, and is never a number someone picked.
10. **The hit map**: stays the CPU's, written only when the layout changes? Recommend yes; written by the compose kernel only if a layout change's 4.4 ms ever shows.
11. **The deck on the GPU after apps** (step 6), or never? Recommend after: its frames are mostly small damage (the console, the clock), but flights and pans are whole frames that miss 120 today.
12. **The mailbox answered when the compose job ends** rather than when it is taken? Recommend yes: it is what keeps an app from drawing into a buffer a job still reads.
13. **A finger-down as an event that raises the GPU's clock** before the first frame of a pan? Recommend trying it in step 5 and keeping it if the first frame's lateness (the turn's 566 µs and the cores' power-up) shows.

## 6. Built (2026-11-21 night, the gpu-compose agent)

The owner's word that night: "build the GPU path, whilst we are we can tweak and adjust. Once it's built we can profile it and look at hotspots and think about tricks", and "support a full window and a frame". Section 5's questions took the recommendations as defaults (listed again in 6.5); the three the owner answered (after a jump, screen off, both handoffs) as he answered them.

### 6.0 Step 0: measured (compbench, forge 035e46e3)

compbench modes 16 to 18: the whole frame (chrome, pane, the parallel noise, the bake, the dither and the pack) into one phase a job, the phases in turn, paced at 120 a second; the one-phase kernel 1971 words (the four-phase one 2343), checked on the host (0 of 26880 pixels off) and on the device (after a run, the four phases the one-phase jobs left equal the four-phase kernel's word for word, 0 of 8.0 M).

On the Pro under the device's policy (no held clocks; Lumis in front), 8 s each:

| | frames a second | late for their slot | round, median / 99th / most µs | the GPU's own, median / 99th / most µs |
|---|---|---|---|---|
| mode 16, one job at a time | 107 | 480 of 860 | 8151 / 11689 / 13020 | 7964 / 9985 / 12763 |
| mode 17, two in flight | 113 | 309 of 909 | 16915 / 30890 / 32520 (queued behind the one running) | 7964 / 10089 / 12652 |

So one phase a job is 7.96 ms of the GPU at the policy's clocks: under 8.33 at the median, over it at the 99th. Two in flight keeps the GPU busy (113 a second) but cannot pass what the kernel costs. Knob 3 and 4 as recommended; 120 sustained needs the kernel cuts of 1.8 or the bake linearised (knob 7, not measured yet).

**The hostile client at the same priority** (mode 16 beside mode 18 in another process, both system clients at 15, the hostile keeping two whole-frame jobs queued): the hostile ran 1867 jobs in 16 s (8.45 ms each, the GPU whole) and the bench's job was ended by its guard (status 2) a few seconds in. The CSF firmware does not share the GPU between groups of one priority: whichever has work queued keeps it. Two consequences, both built (6.1, 6.2): the display's priority must be its own (knob 14), and kists, which all share 8, must take turns.

### 6.1 Step 1: the kernel's stamp and the GPU service (forge 727a23db)

- **The stamp** (process.rs `dv_stamp`, ferros_sys::gpu `View`): for the caller that holds `display-phases` (the display), flags bit 3 and a section after the frames: [0][seq u64][the phases' base][their bytes][m][m × (surface slot, generation)][m × 16 frames]. The windows are the ones the display's last op 5 named (`Queue::view`: `[5][1][n][n × (slot, generation)]`), each stamped only while its owner still holds it under that generation; every other call of the display's carries the same list less what has gone. An older service finds a 0 where it expects an op and refuses.
- **The phases in the GPU's fence** while the display's view holds them (admitted at its first stamp, shut when the view is let go), and again at every refence.
- **Surfaces held from reuse** (GPU-COMPOSE 2.2): a window's surface given back while a display stamp named it stays held and admitted (`surface_give_back`) until the GPU service says the display's tables in force came from a stamp at or after the last one that named it and map it no more (SYS_DMA x1 = 10 on `arena`, `dv_settled`): then it is wiped back into the pool. The display's own end, or the service's, lets the view go once nothing of its can run (`dv_ending`: at once with no GPU work, else at the service's next request or its letting go of the display's queue; the stamps after that point keep what they named).
- **The display's grants**: the kernel gives the receiver of `display` the GPU service's send right and a GPU block (32 MiB, grown to 64 by the display) as slots 27 and 28, at init's spawn and at a keeper's restart alike (init's page is full); both empty when no GPU service runs (after a jump), so no call of the display's waits on a server that never comes.
- **The GPU service** (payloads/gpu serve.rs): the view mapped from the stamp alone, the phases read-write at `PHASES_VA` (2 MiB blocks, coherent, never executable), each named window read-only at its own place from `SURFACES_VA` (2 MiB blocks, never executable); slot 0 the display's alone; priorities 15 the display, 14 the other system clients, 8 the kists; after every display call whose tables are in force (installed with LOCK and FLUSH_PT, which clean and invalidate the L2, or waiting for its group's start) it tells the kernel (`view_settled`).
- **Probes** (compbench modes 19 to 21, `view_test`): a store into a viewed window, the window read after it is named no more, the phases read by a client that is not the display, a window's owner ended while viewed.

### 6.2 The kists take turns (forge e1037e11)

The coordinator's ask after the hostile finding: one kist keeping work queued could starve every other kist (Lumis's RAW encode among them). At each kist job's end, when another kist's group has work in flight, the kists' running groups are ranked by when each was last served: the longest ago at 8, the next 7, and so on down (CSG_EP_REQ's priority, ENDPOINT_CONFIG toggled against its ack, no wait). An event, never a timer; nothing changes while one kist alone has work; the display and the system clients are above every kist and never turned. Op 7 runs a client as a kist (a measure: a client may only give up its standing); compbench modes 16 to 18 take it as a third input byte.

### 6.3 Steps 2 and 3: the display composes on the GPU (forge 8655de24, 22d576cc)

- **Op 86** `gpu compose [0 off, 1 on, 2 toggle, 3 ask, 4 check]` → [0][on][state], development builds, off at every start. A thread of the display's makes the calls (the block grown to 64 MiB, the queue, the kernel, the view), so a call never holds the render thread.
- **The frame it takes** (`gpu::eligible`): today's windows, op 22, each `BAKED` in Normal, none overlapping, at most four (Photon, Lumis, Mahere, a GPU app full screen). Everything else stays NEON's frame by frame: formats 0 and 1, blend modes, a partly covered pixel over another window (the keyboard over an app: main.rs `partial_run` composes those in working values), the screencap, the hit map's view, a window whose surface the view does not name yet (named for the next frame).
- **The kernel** (payloads/display/src/gpu_kernel.rs, 304 words, 16 registers, recorded by build.rs): one invocation a pixel, a warp a 16-pixel block; the front layer (the glass's edge, the hole's knockout and the bars, composed front to back once on the CPU whenever they change, `front_row`: `glass_front` and `bars_front` up to the scene), the window thru its ring, the scene under the front (`under_pm`, integer), the present (BLUE ^ the phase's turn, the no-dither bit, the channels' masks, the pack). One job a phase, waited for, so the phases' rules are NEON's. tools/compose-check runs it in valhall's emulator against the display's own arithmetic: 0 words off.
- **The check** (op 86's 4): the next app frame is published by NEON, composed again by the GPU into the block, and compared word for word; the console says how many differ.
- **The crossover** (step 3, `gpu_cheaper`): each path's cost a + b pixels from the display's own frames (a from frames under 2^14 pixels, b from those over 2^18, each forgetting 2^-4 a frame), seeded from compbench; the cheaper taken, one frame in 2^5 the other so both stay measured. The once-a-second line says the GPU's jobs and times, both costs and where they cross.
- **compbench mode 23** is the bench client: a full-screen BAKED window scrolled on its ring at up to 120 a second.

### 6.4 Step 4: the layered window's kernel (forge b2b56f03, host only)

payloads/display/src/gpu_layers.rs (3712 words, 44 registers, a workgroup of 2^7 a half-row, 8 KiB of storage), recorded by build.rs beside the step 2 kernel and not yet used: the window's pixel layers front to back (`BAKED`, the transparency in bits 49-62, see-through bit 63, a layer's ring, opaque by flag), in the panel's codes as 1.4 has it; the parallel noise (compbench's form, a shimmer added to each row's start) baked thru the display's own tables under what is still clear, inside its clip, black past it; the front layer and the present as step 2. tools/compose-check runs it in valhall's emulator against compbench's scalar noise and bake (6.4's numbers there).
**The display side** (forge 5dc381dd, host-built): the layered show is op 22's 50-byte form plus a tail, `[n]` and n layers frontmost first (layered.rs has the bytes): pixel layers (a surface of the caller's, `BAKED`, its screen rect, opaque by flag, a ring of its own rows), then the background (its clip, axis, scroll, base, shimmer, full, frame). Op 5's feature byte 14 bit 1 says the display reads it. Each source is checked as the caller's in the receive thread (`not_theirs`); a tail that does not parse is a plain show. A layered window is kept under its own format (3) and composed on the GPU by the layered kernel when the compose is on and it is the only window up (the bake's tables copied into the block at each bake generation; its own cost line for the crossover), else on NEON (`layered::row`: its layers front to back in codes, fluor's `noise_under` baked under what they leave clear). compbench mode 24 is its bench client (a chrome, cards on a ring, the noise).
Differences from 3.1 as written: a new op became op 22's tail (one path for the window's bookkeeping, the mailbox answer and the hit map); a pixel layer's ring is its own rows (the strip taller than the pane, 3.2, is not built); coverage tiles are carried and not read.

### 6.5 The reload that went silent (2026-11-21, 23:43), found and fixed

A RAM reload of forge 8f04f065 on the Pro never came back on USB. The cause was 6.1's grants: the kernel gave "the receiver of `display`" two slots past the ones it was handed, and init holds that receive right too (it hands it to the display), with its grants page already full at 108 slots; the two more made `build_from` refuse init's spawn ("more grants than the grants page holds"), so no init, no USB, no shell. The flashed kernel never had it (a hand reset cold-boots as before), and nothing was stored on the device. Fixed (e4a30393): the two slots go only to a receiver of `display` that does not hold the blocks' window whole (init does; the display never), and only when the page has room for both, the guard every slot added after the given ones has (`LINES_WITH`); and only the display stamps a GPU view (5e97f9a1, init holds the phases only to hand them on). To prove on the device before any flash: a RAM reload with init and every shell process up.
Not done: a check that fails the build when a change would push init's page past 108 (the kernel has no host tests; the list is built at run time, its partitions chosen by board).

### 6.6 What has run on the Pro, and what has not

Run: step 0 (6.0). Not run: steps 1 to 4 (the reload above ended the night's device time before them). The order for the morning: a RAM reload showing init and the shell up; then compbench 20 and 19 (a client that is not the display reads the phases: a translation fault; the display's store into a viewed window: a permission fault), 21 with a window's owner killed (the kernel's "held until the GPU service unmaps it" then "settled" lines), 16 as the display beside 18 at 14 (the compose's 99th percentile), 16 and 18 both as kists (the turns); the new `gpu` and `display` stored, op 86 on, its check (4) against NEON on compbench 23's scrolled window, compbench 24 for the layered window on both paths, frames a second and `diag power`.

### 6.7 Section 5's questions: what was taken for now

0 one phase a job while moving: yes (step 2 writes the phase a show owes; the rest at the idle fill). 1 the GPU writes the phases (mode 15): yes. 2 the noise's bake: the tables (the linearised form not measured). 3 the noise generated each frame: yes. 4 the twinkle: off (the frame word carried, 0). 5 kist work while the compositor runs: priority alone, plus the kists' turns (6.2); the compose window not built, to decide on the display-15 against system-14 measure. 6 no GPU service: NEON composes everything (the display's GPU slots are empty then and op 86 does nothing). 7 the display's slot pinned at 15 alone: yes. 8 the phases in the fence only while the view is bound: yes. 9 the crossover live: yes (step 3). 10 the hit map stays the CPU's: yes. 11 the deck on the GPU after apps: not started. 12 the mailbox answered when the job ends: today the jobs are waited for before the show is answered, so yes. 13 a finger-down raising the GPU's clock: not built.
Answered by the owner on 2026-11-21: after a jump the GPU and the hardware come up (not built here; nothing depends on the GPU being absent: the display takes its GPU slots when a service is running at its start); screen off, no UI work on the GPU (the display composes nothing while dark), a Lumis recording's GPU encode untouched; both handoffs, the layered show and the plain op 22 frame (6.3, 6.4).

### 6.8 What fluor's ferros host and Photon still need (the Photon session's)

- The layered show (6.4's bytes; op 5 feature bit 1): `lay_noise` recording the noise's parameters instead of drawing them, `lay_chrome` as its own surface, the content in its own surface on a ring of its own rows (the strip taller than the pane, 3.2, is not in the display yet).
- fluor's noise constants public (`STIR`, `STEP`, `DOWN`, `SPECKLE_BELOW`, `widen_mask`, `widen_base`, `seed`; 3.4): the display and compbench copy them today.
- Its frames `BAKED`, Normal, its window alone or not overlapping another (the keyboard over it goes to NEON: format 0 and the partly covered edge).

### 6.9 Run on the Pro (2026-11-22 morning, b1102: forge 947c2988's kernel in RAM, init and the shell up)

The `gpu` and `display` built from forge e5f4da21 stored (native, read back equal: gpu blake3 0aa7c9fa…, display b0261f16…; copies in keys/husky/capsules-gpu-compose-e5f4da21/, the ones before in keys/husky/capsules-before-gpu-compose-2026-11-22/), gpu restarted, display and map restarted.
- **Step 1's walls**: a client that is not the display reading the phases (G#2_0000_0000) or a window's place: TRANSLATION_FAULT in its own space. A holder of the phases took slot 0, read the window it named (done), its store into it a PERMISSION_FAULT, its read after naming it no more a TRANSLATION_FAULT. A window's owner killed while viewed: "held until the GPU service unmaps it", the held frames still read (done), then at the view's change "settled, wiped back into the pool"; at the client's end the view let go and the phases shut out of the fence. `diag "gpu fence"` G#0 after all of it.
- **The display's priority** (compbench 16 holding the phases, so at 15, beside 18 at 14): 103 frames a second, round 7652 / 9471 / 10723 / 17425 µs (least / median / 99th / most), the GPU's own median 8957; the hostile's jobs stretched from 8.4 to 15.3 ms. The firmware takes the GPU from a lower priority for a higher one: the compose window (knob 15) is not needed for a system client; a kist (8) is further below.
- **The kists' turns** (16 and 18 both as kists, op 7): the bench ran 61 frames a second, round median 16.2 ms (one hostile job then its own), where at one priority without turns its job was ended by its guard.
- **Step 2, op 86 on**, compbench 23 (a full-screen BAKED window scrolled on its ring): the check against NEON 0 of 4,021,248 words differ, twice (phases 3 and 0); 2364 and 2388 shows in 20 s (118 a second) composed on the GPU, against 2400 with op 86 off.
- **Step 4**, compbench 24 (the layered window): 1522 shows in 20 s, no failed job, no panic (which path took each frame is in the stats line from the next display: the line was cut at 256 bytes before the GPU's part, now a line of its own).
- Not measured yet: `diag power` either way, the GPU compose's own times a frame, the layered window's check (op 86's 4 covers step 2's windows only).

### 6.10 Real-path numbers (the Pro, 2026-11-22, flashed e5f4da21; display b4f97c16 stored)

Each row an 8 s `diag power` window (the device's own clock policy) while the bridge's `input` drove it; the display's own lines for the rest. Photon in a conversation, flung up and down for 20 s at a time (two runs each way); the deck idle, panned (drag), zoomed (pinch) and packed (Back's swipe).

**Photon, a fling in a conversation** (Photon renders a frame in 22 to 28 ms, so about 23 shows a second either way: the app, not the compose, sets the rate):

| | NEON (op 86 off) | GPU (op 86 on) |
|---|---|---|
| power, two runs | 4112, 4808 mW | 3962, 4199 mW |
| the display's CPU a show | 21.1 ms of thread time (four threads, 5.0 to 5.8 ms each, the X3 at 499 MHz) | 0.57 to 0.85 ms of the render thread (the front layer's rows and the submit) |
| a show's publish, wall | 4.38 ms | 3.8 to 3.95 ms (the GPU's own 3.47 to 3.74, the round 3.62 to 3.89) |
| shows a second | 22 to 24 | 18 to 25 |
| idle in the conversation | 1813 mW | 1723 mW |

An earlier pair (the X3 at 879 MHz then): NEON 3449 mW with 35 ms of thread time a show, the GPU 3664 mW. The CPU freed is plain (about 20 to 35 ms of thread time a frame to under 1 ms); the power is inside the windows' spread (a few hundred mW either way), so it needs longer windows or the rails (`diag odpm`) to settle.
**The 118 a second** of compbench 23 is the panel's 120: the display arms one frame a panel frame, the newest show wins, and the bench's own pacing lost 2 of 120 slots a second; NEON and the GPU both made it.

**The deck today (NEON)**:

| | power | frames a second | a frame |
|---|---|---|---|
| idle, the pucks ticking | 2163 mW | 9 | a puck batch: 6.0 ms whole, its scene 4.4 ms (CPU, the circles drawn), its publish 1.1 ms |
| panning (drag) | 2864 mW | up to 90 | a camera frame: the scene 1.9 ms, the publish 1.9 ms |
| zooming (pinch) | 3289 mW | 26 | a camera frame: the scene 9.6 ms (the glow 9.5 of it), the publish 5.4 ms |
| the pack (Back's swipe) | 2548 mW | the flight's 13 steps over about 24 panel frames (about 60 a second) | |

**What the deck on the GPU would take, and save** (not built): the deck's frame is mostly the scene, drawn on the CPU into the canvas (the circles, the pucks, the glows: 1.9 to 9.6 ms), and its publish (0.7 to 5.4 ms) composes the canvas, a region's text layer, the wallpaper under them and the flights' sampled planes. The step 2 kernel takes the publish's place only once the GPU can read those buffers: the canvas (`display-canvas`) and the sprites stamped read-only and admitted to the fence (2.3: a kernel change), the wallpaper moved out of the display's heap into its block, and the flights' sampling as a kernel of its own. That saves the publish (up to 5.4 ms a frame when zooming, about 1 ms at rest), not the scene; moving the scene (the glow is most of a zoom's frame) is the larger win and a larger job.
