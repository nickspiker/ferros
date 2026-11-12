# GPU kists: one compute path for the system and for kists

Status: study, design and plan, 2026-11-11; plan step 0 built the same day (below), measured on the host, not yet on the device; plan step 1's isolation built 2026-11-12 (below), not yet run on a device.
Everything not quoted from the owner is a proposal.
GPU.md is the driver as built; this is what it grows into.

## Decided (owner)

- **Compute, not raster** (2026-11-11): "I don't particularly like the GPU strategy of traditional systems anyways ... We have GPU access, we just don't expose the same shader 2D raster and all that crap, yes? aside from wiring it up so kists can actually compute with it, not draw?"
- **The GPU makes layers; `under` and the present stay on NEON** (2026-11-05, memory gpu_role_layers): "We may only run the mandelbrot compute thru the GPU, and possibly some other glow/edge/effects but I'd suspect under is already stupid fast if we layer it out right."
- **The wallpaper is GPU only** (2026-11-11): "We should be using GPU path for the brot in all cases and never 'falling back' to the CPU".
- **One path for the system and kists** (2026-11-11): "we should probably design the GPU kist thing since I'd suspect that's the path we'll use for the wallpaper and ideally the attest screen and stuff too?"
- **Look, not bits** (2026-11-05): "bit identical-ish is totally fine".
- **Mahere** (2026-11-11): "completely triangle raster all the way down. Except for place labels"; "no tilting or 3Dness"; "2-8 triangles per pixel, and each triangle of course has several layers but is one colour"; "The triangle shader shit is more for textures on things, to which we don't have rectangles, no?"; on exact coverage, "and then it'd be a 1:1 per px too, rather than our oversample now, I think 9x or so?"
- **The order** (2026-11-11): "a proper study, design, plan and then if it looks good, we can build the GPU!"
- **f16** (2026-11-11): "f16 is likely worth it, we get 2x and most of the mahere composite work can either be done in integer or f16." Valhall's paired half-precision ops run at twice the rate; light, colour, coverage weights and lookups are f16, coordinates stay integer (Mahere's Q30.16 texel addressing: f16's 11 bits cannot hold them).
- **What a capsule carries** (2026-11-11): "ideally WGSL text becomes VSF compiled by the dev that gets checked and optimized by ferros." The developer's tools compile the source into a typed intermediate form stored as VSF in the capsule; the device validates it (types, buffer bounds, loop limits), optimises it, lowers it to Valhall at install and seals the result; the device never parses shader text (open question 0 closed).
- **Looks the same, never bit for bit** (2026-11-11): "if it looks roughly similar, that's totally fine. the aspect and resolution from one device to another is going to fuck it up more than anything and the human WILL NOT NOTICE. Just needs to look the same." (open question 2 closed; the wallpaper crate's own promise rewritten to match).
- **The kept wallpaper's read first** (2026-11-11): "2.9s for a 24MB read is embarassing by the way... that def needs fixed BEFORE we decide to take the pre-rendered disk read out." (open question 3 waits on that fix).
- **No GPU, no picture** (2026-11-11): "Black until we bring the GPU up. I did say no fallbacks." (open question 4 closed).
- **Spirix** (2026-11-11): "Spirix is a ways out, I wish but hardware ain't there yet."
- **No GPU right** (2026-11-11): "No GPU prompt, that's implicit." Every kist computes on the GPU as it does on the CPU, in its own GPU address space (open question 5 closed).
- **Still implicit, isolation first, development builds only** (2026-11-12): a `gpu` permission was proposed again and agreed ("GPU would be nice, yes"), then dropped the same day: "I feel like GPU should be implicit. If I'm running it on my machine..." So nothing is asked: the kernel hands every launched kist its GPU grants as it gives it CPU time (1.6). Isolation between GPU clients comes first, the GPU's address spaces and the kernel the only protection (plan step 1). A kist's GPU code runs unchecked on development builds only; production gives it no GPU until the checked kernel form exists (plan steps 3-6).
- **Loops, the progress timer** (owner: "Your call on loops"): no bounded-loop rule (termination is not provable in general and the rule would refuse honest kernels); the firmware's progress timer stops a runaway, costing only that client's job (open question 6 closed).
- **Power and clocks** (owner: "Your call on the last two"): the GPU powers up when work is queued and gates when its queue empties, as events, never a timer; its clock steps with load inside the measured floors (open question 7 closed).
- **naga in the developer's tools only** (same): it parses WGSL on the developer's machine; the device runs ferros's own validator over the VSF form, never naga (open question 8 closed).
- **Build it** (2026-11-11): "I say we build it!"
- **Source language, proposed** (owner: "Not sure on the source language, thoughts?"): the VSF form is the contract and any front end that emits it is welcome, all on the developer's machine: WGSL first (Mahere's compose is WGSL, naga parses it), Rust next (rust-gpu to SPIR-V, naga reads SPIR-V into the same form), ferros's own kernels from the `valhall` crate's recorded Rust into the same form, and a Spirix type in the form once Spirix replaces IEEE.

## 0. Study

### 0.0 What the Mali-G715 offers compute (GPU.md, BOARDS.md: husky and akita alike)

| | |
|---|---|
| Cores | 7 (SHADER_PRESENT G#0111_0055), Valhall v11, 16-lane warps, 64 registers a thread or 32 for more warps |
| Queues | CSF: 8 groups of 8 streams; the driver uses one |
| Address spaces | 8; AS0 is the MCU's, so 7 for groups; 4 KiB pages, read-only and execute-never bits |
| Fence | the G3D S2MPU, the kernel's, 4 KiB granular; today `gpu-mem` and the 64 MiB layer arena |
| Coherency | the GPU snoops the CPU's caches both ways for pages mapped MEMATTR 6: no clean, no copy; the bandwidth cost is not measured |
| Speed | memory-bound passes at about 17 GB/s (MIF 2028 MHz); 150 to 890 MHz; a warm dispatch 50-65 us |
| Atomics | ATOM and ATOM_RETURN (add, min, max, and, or, xor, exchange) are in ISA.xml, marked unused in Mesa's copy, never run here |
| Control flow | a flow field on each instruction (wait, reconverge, end); Valhall has no clauses (Bifrost had) |

The ISA source is Mesa's open Valhall/Bifrost compiler (src/panfrost/compiler, MIT), selected files on Harbor at `/mnt/Harbor/tmp/mesa-panfrost/` (ISA.xml, va_pack.c, va_insert_flow.c, va_merge_flow.c, va_validate.c, bi_ra.c, bifrost_compile.c, the genxml), with our verified encodings in `ferros-*-spec.md` beside them.

### 0.1 What exists, and what it lacks

Built:
- `payloads/gpu`, resident from boot (not after a jump): one group in AS1; op 1 runs code at an arena offset over a 2D grid with up to 16 uniforms.
- `valhall` (2.3k lines): a typed builder from ISA.xml, and `ssa`, which records Rust written over a lane type and compiles it (selection, linear-scan registers, the flow rules, one unnested loop with a real break).
- tapuwae's mark recorded thru it: ahau's bonded mark at 900 px in a 5.9 ms job after a 1.2 ms compile.

Lacking:
- isolation: every client shares AS1 and the arena;
- concurrency: one job at a time;
- faults: no MMU fault or progress handling (a bad kernel wedged the queue until `diag "restart gpu"`, 2026-11-05);
- memory: offsets a client picks in a shared arena, nothing per client;
- language: in-tree Rust only, no nested loops, atomics or spilling;
- idle power gating;
- GPU only: ahau's quick pass falls back to the CPU without the service, and the unbound mark (`render_fresh`) has no GPU kernel at all.

### 0.2 The first workloads

| | Wallpaper | Attest mark | Mahere | Lumis preview |
|---|---|---|---|---|
| Who | system | system (ahau) | kist | kist |
| Inputs | a few dozen floats from the seed | the handle's parameters | resident cells of 131072 triangle texels, 7 layers, about 3.5 MiB a cell; a page table; LUTs | the sensor frame, RAW10, 4080 × 3072, 15 MiB |
| Per frame | none | a keystroke's constants | the camera, table changes | pan, zoom, gain, mode, matrix |
| Output | a full-screen layer (32 MB) | a 900 px layer | a full-screen layer | a full-screen layer |
| Rate | once a seed and size | a keystroke, then 16 fine passes | 120 Hz in motion, once at rest | each sensor frame (14-30 fps), each pan |
| Today | CPU: 5.1 s drawn, 33.4 s refined (Pro, 2026-11-07) | GPU, CPU without the service | CPU here; wgpu elsewhere at 9 samples a pixel | not on ferros; GLES elsewhere |

- **Wallpaper:** an orbit with its distance estimate (up to 512 iterations), about 200 lines of shading, a 32-candidate survey with a histogram, an edge mark, a 4 × 4 refine; all gathers, the histogram a small atomic reduction.
- Estimate from the mark's rate (626k pixels, up to 1024 iterations, 3.6 ms): about 0.1 s refined, to measure.
- **Mahere, exact coverage** (agreed 2026-11-11): a pixel's footprint is a parallelogram in texel UV whose area against the triangle lattice is closed form.
- Fully covered areas take the pyramid's parent, coloured as the mean of its children's lit colours in light; triangles are lit once and cached.
- One evaluation a pixel instead of nine, without the micro-triangle cost of hardware raster (2 × 2 quad shading; Nanite moved them to compute).
- Estimate: 16-64 MB of cached colours read and 32 MB written a frame, 2-6 ms; labels stay a CPU layer in front.
- **Lumis:** each screen pixel gathers its CFA cell, applies the matrix, encodes √light; its dither goes, since the display's present dithers every layer.
- On ferros the frame should land by the camera's own DMA in GPU-visible memory: sensor, kernel, layer, compositor, no CPU copy.

## 1. Design (proposal)

### 1.0 One job kind: a gather over a grid

- A job is one kernel over a grid the shape of its output, a layer's pixels or a buffer's elements.
- Each invocation writes only its own element, its store emitted by the compiler from its position, so outputs never race.
- It reads anything in its bound buffers, and may update a small atomic buffer (histograms, view ranges, a layer's tile coverage bytes).
- Inputs are resident buffers written on change, plus a uniform block a run.
- The workgroup is one warp (16 × 1, or 4 × 4 on a 2D grid), the compiler's choice.
- System processes and kists use the same kernel format, buffers, queue and isolation, the system first, so the kist side is proven on our own code.

### 1.1 The language: a compute subset of WGSL

| In | Out, and why |
|---|---|
| `@compute`, `global_invocation_id` | vertex and fragment stages: no raster |
| storage arrays and structs (read, one `read_write`), `var<uniform>` | textures and samplers: integer loads from buffers keep the layout the CPU sees, and Mahere's and Lumis's loads are integer already |
| f32, i32, u32, vectors, matrices, `bitcast`, pack | f64 (no hardware); f16 later if measured worth it |
| arithmetic, compares, `select`, bit ops, sqrt, exp2, log2, sin, cos, atan2, floor, fma | `dpdx` and the other derivatives: fragment only |
| `if`, loops, `break`, functions (inlined) | pointers past function scope |
| `atomicAdd`, Min, Max, And, Or, Xor, Exchange | workgroup variables, barriers, subgroups: a gather needs none |

- Every loop carries a bound known at dispatch, as tapuwae's `iterate(max, …)` does, so every kernel ends by construction.
- No bounds checks are generated: the GPU MMU is the bound, and an access outside the client's memory faults the job, loud (WGSL's own rule clamps).

### 1.2 The compiler on the device

- **Rust recorded** (system code: the mark, the wallpaper): valhall::ssa as today, the same source on NEON.
- **WGSL** (kists): naga's front end and validator, our subset check, then lowering to valhall's SSA.
- **One back end** (valhall, grown): nested control flow, 8- to 64-bit loads and stores, atomics, integer division lowered (Valhall has none), registers with no spilling (a kernel that does not fit is refused, loud), flow insertion and merging, the uniform-port rule.
- **One output, a kernel object:** the words, the register mode, the preload, the bindings and their uses, the uniform layout.

Mesa's sizes for the same work: bifrost_compile.c 4723 lines, bi_ra.c 1373, va_pack.c 1278, the va_ passes about 1600.
Valhall issues in order and hides latency by switching warps, so no scheduler at first.
Ours: about 4-6k lines more in valhall, 2-3k for the naga lowering, and naga itself, a large dependency, in a std compiler process (`gpuc`) whose size is to measure.
A kist's kernels are compiled at install and sealed with its code (trust model A, 2026-10-15: "No reason to trust pre-compiled capsules").
The MMU, not the compiler, keeps a kernel inside its memory, so the compiler is outside the isolation's trusted base.

### 1.3 Buffers and memory

- GPU memory is DMA memory, which never moves (ARCS.md 2): frames from the pool, pinned for their life, granted to the owner as a region (a handle for masked code) and mapped in its GPU address space.
- Mapped coherent on the GPU and write-back on the CPU, so "upload on change" is the owner writing its own buffer: no copy, no cache maintenance.
- The kernel admits exactly the pinned GPU frames to the G3D fence.
- When a process ends, the kernel takes its frames out of the fence before returning them to the pool, so a stale GPU mapping reaches nothing (one shot dead, no help from the service).
- Kernel code is read-only and executable on the GPU, data never executable.

### 1.4 Output layers

- A GPU layer is a mailbox (COMPOSITOR.md rule 1): two pinned buffers in the display's working values, granted to the owner and the display.
- A run writes the back buffer; the owner then publishes it with its tile coverage, which the kernel can fill by atomic or.
- The display composes it front to back with `under` and presents it on NEON; the shared layer arena is retired into these.

### 1.5 Isolation and fairness

- Each client gets its own group and one of AS1-AS7, mapping only its kernels, buffers and layers; past seven, the service rebinds an idle client's space between jobs, as kbase does.
- The service writes those tables, so it is trusted to separate clients; the kernel's fence is the wall around them all.
- A runaway kernel trips the firmware's progress timer (counted in GPU cycles, set at global init) and its job ends: no host timer.
- That is no theory: on 2026-10-09 a Mahere walk that never ended reset Android's GPU for every process.
- Display work first: system groups run at a higher CSF priority than kists, and one run is in flight a layer; whether the firmware preempts a running kist group is to measure.

### 1.6 No right: every kist is handed the GPU (owner, 2026-11-11 and 2026-11-12)

- No `gpu` in RIGHTS, no want, no question, no settings row: a kist computes on the GPU as it does on the CPU.
- At launch the kernel gives every kist three slots after its wants (process.rs `kist_gpu`; ferros_sys::gpu `kist_slots`): `use gpu`, a block of the layer arena of its own (the largest free), and a second surface for a mailbox while another stays free for the next app's window. Any of them is an empty slot when there is none to give.
- A production build gives three empty slots: a kist's GPU code is unchecked until the VSF kernel form and its checker exist, so it runs on development builds only, as its native code does; the GPU service also refuses a kist's job on a production build (status 5).

### 1.7 Failure modes

| What | What happens |
|---|---|
| an access outside the client's mappings | the job ends; the client is told the address and the kind |
| no progress | the job ends; the client is told |
| a kernel past 64 registers or outside the subset | refused at compile, with the reason |
| the GPU's own fatal | reset (10 us) and MCU boot (115 us), groups rebuilt, jobs in flight answered "lost"; each layer keeps its last published buffer |

### 1.8 Ops on the `gpu` endpoint

| Op | Request | Answer |
|---|---|---|
| 0 `buffer` | bytes, use (read, write, atomic) | a region, granted and mapped |
| 1 `run` | a kernel, the grid, buffers in its declared order, the output, the uniforms | held until done: µs, or fault, stopped, lost |
| 2 `layer` | width, height | a mailbox layer for it and the display |
| 3 `free` | a buffer or layer | done |
| 4 `submit` | as `run` | at once; a bell rings at the end, to overlap a frame's run with the last one's compose |

No op polls; op 1 replaces today's.

## 2. Plan

Each step is measured on the Pro before the next; sizes are estimates.

0. **The wallpaper on the GPU, Rust recorded:** the wallpaper crate over a lane type (tapuwae's route), thru today's service; the display's and wallgen's CPU paths go, the CPU code kept only for tests and tools/tune.
   - Proof: the refined 1344 × 2992 wallpaper's time against 33.4 s, its difference from the CPU's measured and shown to the owner.
   - Size: 1-1.5k lines (sin and cos, ldexp in the builder; the survey's histogram read back).
   - **Built (2026-11-11), about 1.3k lines:**
     - `wallpaper` over a lane (lane.rs): f32 and libm on the CPU, bit for bit the old picture (its pinned hashes hold); valhall's recording type on the GPU (gpu.rs). dz's ldexp became a scale multiplied in; sin 2πt a polynomial on the GPU; no builder change for either.
     - Three kernels: the survey (a preview pixel an invocation, counts and distances read back and tallied on the CPU), the first pass (1030 words, run in bands of 2^20 pixels by a uniform), the refine (a listed pixel's 4 × 4 samples, 17 loops, 13812 words of the service's 16368). The marks stay on the CPU (integer compares over the first pass where the GPU left it).
     - valhall: `ssa::uniform`, U + U, 64 labels a kernel, and `sim`, an emulator of the emitted words, checked against the device's Mandelbrot (its sum and cap count, every pixel).
     - wallgen draws it this way only, in the arena's first 32 MiB (gputest's bench region); without the GPU service, no wallpaper. The display's job and its four threads are gone: it shows the person's own or wallgen's, else none.
     - On the host, the words in the emulator against the CPU at an eighth of husky's frame, ΔE in the crate's opponent space × 100: the survey keeps the CPU's candidate for 8 of 8 seeds; the first pass mean 0.02-0.05, 0.3-0.7 % of pixels above 1 (the set's edge); refined mean 0.02-0.04, 99th percentile 0.5-1.1, most 4.1.
     - f16, measured not built: the colour stage looks the same in f16 (mean 0.03), the orbit does not (mean about 50). The recorder has no f16 type; the 2× needs paired v2f16 and lands on the colour stage only.
     - To measure on the device: wallgen's line (survey, first pass, marks, refine, each GPU µs and compile), the refined time against 33.4 s, a capture by eye.
1. **The one path's service, system clients only:** a group and address space per client, pinned buffers and layers with the fence per frame, held calls and bells by interrupt, MMU faults and the progress timer, priorities; clients wallgen, ahau, gputest.
   - Proof: gputest and ahau at once; a gputest kernel reading past its buffer faults alone while the mark lands; an endless kernel stopped with no host timer while the mark draws; the fence logs nothing.
   - Size: 2-2.5k lines in the service, 300-500 in the kernel.
   - **Isolation built (2026-11-12), not yet run on a device** (owner: "GPU would be nice, yes", then "I feel like GPU should be implicit"):
     - **Memory a client's own.** The layer arena is cut into blocks, the whole, its halves and its quarters (process.rs `layers-block`, powers of two aligned to their size, so a masked client reaches one thru a handle), and a spawn gives a client the largest free block up to what it asks (`NewRegion`: `layers` the largest, `layers-32` 32 MiB; a kist the largest), wiped as it is given, freed when its owner ends, never granted by name, never passed to a child. The whole arena (`arena`) is the GPU service's, and init's to pass on as a 32 MiB block (the attest screen's). Surfaces are wiped as they are given too.
     - **Who calls, from the kernel.** Every call on `gpu` goes in behind the kernel's stamp (`gpu_stamp`): the caller's epoch, whether it is a kist and the build production, and every GPU-reachable region it holds (its block, its surfaces). The kernel writes it in front of the caller's bytes on every call, so no caller can forge one, and the service maps a client's address space from it alone (ferros_sys::gpu `Stamp`).
     - **A group and an address space a client.** The service keeps seven slots, CSF groups 0-6 bound to address spaces 1-7 (AS0 is the MCU's), user doorbells 1-7; an eighth client takes the slot used longest ago (its group ended, its space given new tables). A client's space maps its block at `ARENA_VA` (offsets into its own block), each surface it holds at its place from `SURFACES_VA`, and the service's pages for that slot: its ring and descriptors read-only, its code region read-only and the only executable pages, its done flag. Nothing else: no other client's block or surface, none of gpu-mem's firmware, tables or interface. A slot's tables live in its own 160 pages of gpu-mem, rewritten only after its group has ended, then LOCK and FLUSH_PT over the whole space before the new root is installed.
     - **Faults.** The service holds the MMU's line (`gpu-mmu`, slot 8) on a thread of its own: a fault in a client's space is noted, that space switched to UNMAPPED (kbase's way, so the stalled accesses end), and the job's wait ends at once; the job ends with its group and the client is told the GPU address and AS_FAULTSTATUS (status 4). Without the line the fault is found at the job's guard.
     - **Fairness.** One job at a time, in the order the kernel queues the calls (each caller's thread holds one call). Every job is ended at its guard with its group: a system client's 2^10 ms, a kist's 2^8 ms, so one kist holds the GPU from the others a quarter second at most; the firmware's progress timer stops one that stops progressing. A system client's group runs at CSF priority 15, a kist's at 8 (it matters once jobs overlap). A group the firmware will not end ends the service: its next start soft-resets the GPU before it serves.
     - **An ended client's job.** A process that ends with a call the GPU service has taken and not answered (`gpu_job_in_flight`) has its blocks and surfaces held back (`gpu_drain`) until the service takes its next request, which it does only once the last job is over (done, or its group ended): a job still running writes nothing the next owner holds.
     - **The fence** (s2mpu::fence_gpu) stays the wall around every client together: gpu-mem, this board's arena and the surfaces, for every VID. On akita the arena is its upper 64 MiB only (`cachemap::layers`), the range it ran GPU jobs in since 2026-11-09: the lower 64 MiB has no memblock proof there.
     - **The proof to run:** gputest's isolation test (input 13): a keeper fills its block and draws the Mandelbrot; an intruder's kernels store at its own block (must land), just past it, at every other 32 MiB of the arena as the shared layout of before had it, at every surface and at the service's read-only pages (each must fault as status 4, in its own space); then an endless kernel (stopped at its guard, no host timer) and its own store again; then the keeper says whether any word of its changed and draws again.
     - **Not yet:** pinned buffers and layers from the frame pool with the fence per frame (the arena is still a fixed 128 MiB, so a kist's block is as large as what the system's clients leave free); concurrency (one job at a time; the priorities wait for it); held calls and bells; the display's op 22 still takes any live surface as a window's pixels, so one app can show another's surface (the display's isolation, not the GPU's).
2. **The attest mark wholly on the GPU:** `render_fresh` recorded as the bonded mark is; the display's glow and edge layers measured as candidates, moved only where they win.
   - Proof: the unbound mark's time against about 0.5 s on the workers, its difference measured.
3. **WGSL and the grown back end:** naga, the subset check, nested flow, atomics, register refusal.
   - Proof: the wallpaper in WGSL, compiled on the device, looks as step 0's; compile time and `gpuc`'s size measured; each new form checked on the device against the CPU, as the Mandelbrot was.
   - Size: 6-9k lines.
4. **Mahere's exact-coverage compose** as a WGSL kernel over resident cells.
   - Proof: full-screen frames under 8.3 ms at rest and panning, compared by eye with Mahere's nine samples.
   - Size: 1-1.5k lines of kernel, 500 in mahere-ferros.
   - **First cut built (2026-11-11), Rust recorded, one sample a pixel** (owner, that night: "Obviously get mahere on GPU proper and flawless at 120fps"):
     - Mahere's mahere-ferros records its CPU raster's compose (raster.rs `compose_rgb`) as a kernel through `valhall::ssa` every frame, the frame's light, inks, sun, disk, depths and layer mask its constants (64 uniform slots: 206 of the first cut's 836 words were moves of constants before their uses), and runs it through the GPU service over every pixel: 665 to 714 words, no loop.
     - Each pixel finds its 32 px tile, its block (the plan's lattice, 4 px sub-blocks across a diamond's seam), its (u, v) by the block's bilinear steps, and for the terrain and the vector planes the cell among the block's 2 × 2 at the wanted depth (the finest resident stand-in, from the plan's candidates), then its triangle texel as the CPU does.
     - The cells' planes live in 1 MiB slots of the layer arena, written once when a frame first draws them and kept (a frame evicts only what it does not draw): the terrain's packed texels as they are, the vector planes two words a texel.
     - The panel and the screen's marks go in front as an overlay (premultiplied, painted again only when what they show changed), composed front to back in the kernel.
     - Out: the display's working values straight into the app's surface (the GPU's fence admits the apps' surfaces since that night, `s2mpu::fence_gpu`), so no CPU touches a frame's pixels; where the surfaces are not fenced in (a kernel from before), fluor's Argb8 in the arena for fluor's host to present.
     - The light goes out as VSF light: the CPU path's BT.2020 matrix and Opsin's rail are the Android and Linux surfaces' contract, and the ferros CPU path's bytes had been read as `g2t` codes by fluor's host (a mismatch of the CPU path's, untouched).
     - Checked on the host: the words in valhall's emulator against `render_frame` over cells of every plane, turned and spanning cells: 7504 of 7680 pixels the same code, two more than four codes apart.
     - Not on the GPU yet (the CPU path has them): the lettering, imagery, ownership and MVUM, the user's ink and selection, slope bands, raw elevation, the selection being drawn, the measurement's line, a terrain texel with no data climbing to its parent.
   - **Measured on the Pro** (a bench of two fingers panning and pinching every frame; 1344 × 2992):
     - The kernel: 6.6 to 7.7 ms a frame on the GPU at 890 MHz (4 Mpx, compute-bound: about four warp instructions a cycle a core).
     - Mahere's own frame, a town's scale: 15 to 16 ms (the plan 6.5 to 7.5 ms: corners 2.4 to 3, candidates 3.1; cells, blocks, overlay, record and compile together about 1.5; the GPU call); at globe zoom 40 ms (the plan 14 to 21 ms: hundreds of seam tiles of 81 exact sub-corners).
     - The display's show of a whole-screen app frame (op 22): about 20 ms of its render thread (28 ms before that night's split across the publish workers).
     - On the glass: about 49 frames a second at a town's scale, 24 to 25 at globe zoom; before, the CPU raster's frames took 1.5 to 2 s.
   - **What 120 Hz still needs:**
     - The display's publish of a whole-screen app frame near its NEON ideal (about 1.5 ms of wall a phase, GPU.md): it is about 20 ms; where it goes is to measure (the composition, the codes, the dither, the glass, the workers' contention with the app's own threads).
     - A GPU layer as a mailbox (1.4): today the kernel writes the one surface the display reads, so the GPU's next job cannot overlap the display's publish of the last without tearing.
     - The lattice and the cell lookup in the kernel, from the camera and a page table (0.2's "per frame: the camera, table changes"), so the CPU's 6.5 to 21 ms plan goes.
     - The kernel itself to about 4 ms: f16 for the light and the colours (paired), and branches over what a warp does not need.
   - **Second pass (2026-11-12), measured on the Pro the same way** (the network down that night: DNS gave no address, so the cells were the vault's):
     - The display's publish, measured by part (a line every 512 shows), was 77 ms of CPU a show over its four threads: the composition 7 (a copy of every pixel), the glass 14 (every pixel tested for a few dozen a row), the codes 49, the dither and store 7; about 20 cycles a pixel on the X3.
     - Cut to about 50 ms of CPU and 10.5 to 13 ms of wall: the glass over the columns it covers only, an opaque front window read in place, the app light in f16 (`light16h`, within 0.42 % of the f32 form). What is left is the panel stage itself: the codes (the ODT and the panel's curve) about 8 cycles a pixel on the X3, the dither and store about 1.5.
     - A window as a mailbox (op 22's 36-byte form names the buffer; the show is answered once the display has taken it) and Mahere pipelined over it: the next frame planned while the GPU draws this one and the display publishes the last.
     - On the glass: a town's scale 68 to 71 frames a second (49 before), Mahere's draw 9.5 ms, the kernel 6.7 to 8 ms, the publish the slowest stage; the globe 36 (25 before), Mahere's draw 27 ms (the plan 15.7, its seam blocks 9.4; the overlay 8, the loading ring repainted every frame while no cell could load).
     - Heat: with the GPU at 890 MHz and every core busy the SoC reached 82 to 97 C within minutes and the X3 ran at 364 MHz for seconds at a time (a chain of adds in the publish line, its cycles and its count agreeing), with the kernel's own throttle not engaged (`diag thermal` listed only the LITTLE zone, at 80 to 85 C); raising the A715s from 1065 to 2367 MHz took the X3 to 364 MHz at once. Sustained 120 is a power budget as much as a schedule.
   - **For 120, the owner's calls:**
     - The panel stage of a GPU app's frame. On NEON it is about 9.5 cycles a pixel, 38 M cycles a 4 Mpx frame, 4.6 G cycles a second at 120: more than the X3 and the A715s give at their clocks, before Mahere's plan. Either the GPU kernel ends in the panel's codes (the ODT and the panel curve on the special function unit, about 30 instructions a pixel, its light already linear), leaving the display the dither, the pack and the glass at about 1.5 cycles a pixel (about 2 ms of wall), which moves part of the present onto the GPU against the decision of 2026-11-05; or the present stays whole on NEON and a whole-screen GPU app stays near 75 frames a second.
     - The A715s' active clock (1065 MHz, the boot's) and the heat it costs.
     - Lettering in GPU mode: a CPU layer in front means the labeller's box-filtered glyphs every frame of a pan on cores the publish already fills; in the kernel it means the glyph atlas in the arena and each 32 px tile's list of glyphs, a loop a pixel. Not built.
     - The plan into the kernel (the globe's 15.7 ms) is the next step on Mahere's side either way.
5. **Lumis's preview, zero copies:** camera DMA into pinned frames inside both the ISPFE and G3D fences, a kernel from the CFA to a layer; Lumis must first build for ferros.
   - Proof: no CPU copy, CPU time a frame near zero, the sensor's frame end to the layer measured.
   - Size: 200 lines of kernel, 500 in the camera path.
6. **The kist side:** kernels in the capsule, `gpuc` at install, sealed objects, the ops for kists; production gives kists the GPU once their kernels are checked (no `gpu` in RIGHTS, no settings rows: 1.6).
   - Proof: a test kist installed, asked once, running; denied, an empty slot; revoked, respawned without it; Mahere launched as a kist with the right.

## 3. Open questions for the owner

0. **The kernel's form in a capsule:** WGSL text means a parser for untrusted text on the device (the rule against string parsers); proposal: WGSL parsed on the developer's side, the capsule carrying a typed form (naga's IR as VSF, or SPIR-V), validated on the device.
1. **One source language?** System kernels stay Rust recorded (one source for NEON and GPU), or move to WGSL so every kernel takes the kist path.
2. **The wallpaper's promise:** wallpaper/src/lib.rs promises the same pixels bit for bit everywhere; on the GPU, "looks the same" or still bit for bit (which costs the fast forms)?
3. **Keep `wallpaper.made`?** If the GPU makes it in well under a second, store it (24 MB, a 2.9 s read) or make it at each boot?
4. **No GPU service after a jump:** the wallpaper and the mark then have no path; black until the GPU, or the jump brings the service up?
5. **The `gpu` right:** a right at all (the MMU separates kists; power, heat and timing side channels remain), default off? Closed: no right (2026-11-11, again 2026-11-12).
6. **Bounded loops** required, or any loop with the progress timer alone?
7. **The clock:** 890 MHz whenever a `gpu` process lives (today), per job, or by queue depth; and idle gating.
8. **naga** as a dependency, or our own WGSL front end?
