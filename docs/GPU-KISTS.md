# GPU kists: one compute path for the system and for kists

Status: study, design and plan, 2026-11-11; plan step 0 built the same day (below), measured on the host, not yet on the device; plan step 1's isolation built 2026-11-12 and run on the 8a the same day (below), not yet on the Pro; plan step 4's fifth pass (the kernel compiled once, f16, branches) run on the 8a the same day; Lumis's asks (section 3: the queue, the clock by events, per-job GPU time, faults to the kist, the camera ring read-only on the GPU, the display's frames, layers, the back end's ops) built 2026-11-20 on branch gpu-kists-lumis, host-tested, not yet run on a device.
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
- **What a capsule carries** (2026-11-11): "ideally WGSL text becomes VSF compiled by the dev that gets checked and optimized by ferros." The developer's tools compile the source into a typed intermediate form stored as VSF in the capsule; the device validates it (types, buffer bounds, loop limits), optimises it, lowers it to Valhall at stow and seals the result; the device never parses shader text (open question 0 closed).
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
A kist's kernels are compiled at stow and sealed with its code (trust model A, 2026-10-15: "No reason to trust pre-compiled capsules").
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
- At launch the kernel gives every kist three slots after its wants (process.rs `kist_gpu`; ferros_sys::gpu `kist_slots`; after the leading implicit rights and its asked wants since 2026-11-12, KISTS.md "The implicit set"): `use gpu`, a block of the layer arena of its own (the largest free), and a second surface for a mailbox while another stays free for the next app's window. Any of them is an empty slot when there is none to give.
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

As built since 2026-11-20 ("The queue" below), the ops are: 1 run (as before, now answering the GPU's own µs too), 2 load a kernel for a queue, 3 open a queue, 4 close it, 5 look at the caller's memory again. Memory is the kernel's (a GPU block asked of the kernel, SYS_DMA x1 = 2; surfaces asked of it, SYS_DMA x1 = 10), never the service's, and a job is submitted without a call at all: the proposal's ops 0, 2 and 3 are the kernel's, and its op 4 is the queue.

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
   - **Isolation built (2026-11-12), run on the 8a** (owner: "GPU would be nice, yes", then "I feel like GPU should be implicit"):
     - **Memory a client's own.** The layer arena is cut into blocks, the whole, its halves and its quarters (process.rs `layers-block`, powers of two aligned to their size, so a masked client reaches one thru a handle), and a spawn gives a client the largest free block up to what it asks (`NewRegion`: `layers` the largest, `layers-32` 32 MiB; a kist the largest), wiped as it is given, freed when its owner ends, never granted by name, never passed to a child. The whole arena (`arena`) is the GPU service's, and init's to pass on as a 32 MiB block (the attest screen's). Surfaces are wiped as they are given too.
     - **Who calls, from the kernel.** Every call on `gpu` goes in behind the kernel's stamp (`gpu_stamp`): the caller's epoch, whether it is a kist and the build production, and every GPU-reachable region it holds (its block, its surfaces). The kernel writes it in front of the caller's bytes on every call, so no caller can forge one, and the service maps a client's address space from it alone (ferros_sys::gpu `Stamp`).
     - **A group and an address space a client.** The service keeps seven slots, CSF groups 0-6 bound to address spaces 1-7 (AS0 is the MCU's), user doorbells 1-7; an eighth client takes the slot used longest ago (its group ended, its space given new tables). A client's space maps its block at `ARENA_VA` (offsets into its own block), each surface it holds at its place from `SURFACES_VA`, and the service's pages for that slot: its ring and descriptors read-only, its code region read-only and the only executable pages, its done flag. Nothing else: no other client's block or surface, none of gpu-mem's firmware, tables or interface. A slot's tables live in its own 160 pages of gpu-mem, rewritten only after its group has ended, then LOCK and FLUSH_PT over the whole space before the new root is installed.
     - **Faults.** The service holds the MMU's line (`gpu-mmu`, slot 8) on a thread of its own: a fault in a client's space is noted, that space switched to UNMAPPED (kbase's way, so the stalled accesses end), and the job's wait ends at once; the job ends with its group and the client is told the GPU address and AS_FAULTSTATUS (status 4). Without the line the fault is found at the job's guard.
     - **Fairness.** One job at a time, in the order the kernel queues the calls (each caller's thread holds one call). Every job is ended at its guard with its group: a system client's 2^10 ms, a kist's 2^8 ms, so one kist holds the GPU from the others a quarter second at most; the firmware's progress timer stops one that stops progressing. A system client's group runs at CSF priority 15, a kist's at 8 (it matters once jobs overlap). A group the firmware will not end ends the service: its next start soft-resets the GPU before it serves.
     - **An ended client's job.** A process that ends with a call the GPU service has taken and not answered (`gpu_job_in_flight`) has its blocks and surfaces held back (`gpu_drain`) until the service takes its next request, which it does only once the last job is over (done, or its group ended): a job still running writes nothing the next owner holds.
     - **The fence** (s2mpu::fence_gpu) stays the wall around every client together: gpu-mem, this board's arena and the surfaces, for every VID (the surfaces frame by frame since 2026-11-12, as each is given and given back: WINDOWS.md "Surfaces on demand"). On akita the arena is its upper 64 MiB only (`cachemap::layers`), the range it ran GPU jobs in since 2026-11-09: the lower 64 MiB has no memblock proof there.
     - **The proof, run on the 8a (2026-11-12,** the kernel reloaded in RAM, the service and gputest spawned, ahau killed for the test so two 32 MiB blocks were free, then the 8a rebooted to its flashed kernel**):** gputest's isolation test (input 13). The keeper (AS1) filled its block (G#9C00_0000) and drew the Mandelbrot there: 0 of 4096 pixels wrong. The intruder (its own block G#9E00_0000, AS2, then AS3 on a second run) stored 16 words at each of 13 addresses: its own block landed (status 0); just past its block (G#8200_0000), each other 32 MiB of the arena as the shared layout had it (G#8210_0000, G#8410_0000, G#8610_0000), all four surfaces (G#C000_0040 to G#C600_0040) faulted as translation faults, and the service's ring, descriptors and code (G#1000_0000, G#3100_0000, G#3400_0000) as permission faults, each in the intruder's own space, status 4 with the address, 70-230 us; its own store after each worked (its group started afresh). An endless kernel ran its 2^10 ms guard and its group ended (CSG TERMINATE stops a running compute job); its own store after it worked in 154 us. The keeper then found 0 of 7864320 pattern words changed and drew again with 0 pixels wrong. The G3D fence logged no fault. A client killed while its endless job ran (the drain): a spawn 40 ms later found no block free (its slot empty: "no layer arena"), one after the guard got the block and drew with 0 pixels wrong.
     - **Learnt there:** on this CSF GPU MMU_IRQ's bits from 16 are not bus faults (kbase's CSF handler takes bits 0-15 alone): bit 17 rose for AS1 after its LOCK and FLUSH_PT with no fault in it, and the first cut took it for one. The service unmasks and reads the page-fault bits only.
     - **Not run yet:** a kist launched with its GPU slots (akita has no person attested), ahau and wallgen in their blocks (ahau's GPU mark needs a bound identity; ahau is masked, so the rebuilt one needs sealing), Mahere as a kist on the Pro.
     - **Not yet:** pinned buffers and layers from the frame pool with the fence per frame (built for a client's block on 2026-11-13, the seventh pass of plan step 4; until then the arena was a fixed 128 MiB, so a kist's block was as large as what the system's clients left free); concurrency (one job at a time; the priorities wait for it); held calls and bells; the display's op 22 still takes any live surface as a window's pixels, so one app can show another's surface (the display's isolation, not the GPU's).
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
   - **Third pass (2026-11-12): the plan in the kernel** (Mahere ferros-plan 19164a8, built on the 8a):
     - Each pixel finds its own place on the ball: its direction in view coordinates from three uniforms (the camera's centre and the globe's radius), its face the nearest centroid among the frame's faces in view (the encoder's choice, mahere-coord `face_map`), its depth-30 (u, v) as a ratio of two linear forms of the direction (the gnomonic projection and the barycentric solve in one, a reciprocal a pixel). When every face in view is within 50 degrees of the camera point the forms are measured from it (z − 1 as −q / (1 + z)), so f32 keeps a unit (6.6 mm) of precision at any zoom; at the globe from the origin.
     - Its cell from a page table in its own block: for each role (the terrain, the vector planes) and each diamond in view, a box of cell-grid squares at the role's depth, each square one word, the texel base of the cell standing in for it and its texel shift. Two loads where the blocks' candidates took a dozen.
     - One face in view: every coefficient is a constant of the recording. More (a seam, a corner of the icosahedron, the globe): an argmax over the faces (three FMAs and two selects each), then the face's row of 24 words from the arena.
     - The CPU's part (mahere-engine `viewplan`): the faces in view, about 3500 points projected without trigonometry (the 32 px lattice, every 8 px along the edges, the faces' corners on the screen) for the desired set, the boxes and the elevation range, and each square's stand-in. No lattice of blocks, no 4 px seam sub-blocks.
     - Checked on the host in valhall's emulator: every pixel's (u, v) within a unit of the plan's f64 at a town, a seam, the north pole, 2^20 pixels a degree on an icosahedron corner, and the globe; every square and texel address as the host's table names it; the frame against the CPU raster as before (7505 of 7680 pixels the same code), across a seam and at the pole too outside the CPU's own straddling tiles (it samples those at the pixel's centre, every other tile at the corner, as the kernel does everywhere).
     - Measured on the 8a (1080 × 2400, its 60 Hz panel; DNS gave no address, so no cells and the loading mesh everywhere; built with a 128 MiB heap for both, since akita's frame pool is 336 MiB with about 206 free and the 640 MiB ceiling is not grantable there):

       | | Before | After |
       |---|---|---|
       | Town: the plan | 3.83 ms (corners 1.44, candidates 1.74) | 0.93 ms |
       | Town: the kernel on the GPU | 5.34 ms, 666 words | 4.64 ms, 612 words (14 loads, 31 before) |
       | Town: Mahere's CPU work a frame | about 5 ms | about 1.4 ms (then 3.6 ms waiting on the GPU) |
       | Town: on the glass | 60 a second | 60 a second |
       | Globe: the plan | 11.6 ms (seam blocks 8.1) | 1.08 ms |
       | Globe: the kernel on the GPU | 5.34 ms, 680 words | 6.17 ms, 891 words (15 faces searched) |
       | Globe: Mahere's draw a frame | 18.4 ms | 9.3 ms (7.3 of it the overlay: the loading ring repainted every frame while no cell loads) |
       | Globe: on the glass | 54 a second | 60 a second |
   - **What 120 on the Pro still needs, after the third pass:**
     - The display's publish of a whole-screen app frame (10.5 to 13 ms of wall on the Pro), and with it the panel stage's owner's call above: the slowest stage now by far.
     - The overlay: the loading ring's marks painted every frame (1.3 ms) and summed into the arena every frame (2.5 to 6 ms); paint the ring's marks on the GPU, or only when it changes. Done in the fourth pass.
     - The kernel at the globe: the argmax over up to 15 faces; a face found from the octant and a few planes, or the faces culled to those whose diamond holds a point, would bring it back toward the town's 4.6 ms. The fold done in the fourth pass; then f16 for the light and the colours.
     - Measured on the Pro with cells, the network up and the full heap: not yet (the 8a ran without cells).
   - **Fourth pass (2026-11-12): the overlay costs nothing on a frame where it did not change** (Mahere ferros-overlay 1fc4428, built on the 8a):
     - The overlay is a resident layer in each frame region, updated by dirty boxes: the marks clear only the box they last covered and say the box that may differ (each stroke answers the box it touched), the panel's box of shown pixels is found when it is painted again, and a region's copy is summed over the box changed since that region was last drawn, nothing else.
     - The loading ring is the kernel's own, from a few uniforms (its centre, radius, half width and the sweep's end): under the overlay and over the map, its code premultiplied by its coverage as the marks' sum did, its edge smoothstepped. No upload, about 80 words, and it follows the globe's radius every frame of a pinch. Chosen over sending its box: the ring circles the globe, so its box is most of the screen.
     - At the globe (far from the camera point, more than four faces) the face comes from a fold instead of the search: the icosahedron turned to the pose whose vertices are (0, ±1, ±φ) and their cyclic turns has the dodecahedron for its face centres, kept by each axis's reflection, so the direction's octant and the nearest of four centres to its absolute value name the face, a row of 32 in the arena (mahere-coord `face_fold`, checked against the encoder's search over 2^16 directions; the frame the same pixel for pixel as the search's at three globes, about 80 words fewer).
     - Measured on the 8a as the third pass was (no cells, the loading mesh and the ring at its first sweep, a 128 MiB heap build for both):

       | | Before | After |
       |---|---|---|
       | Globe: the overlay a frame | 7.15 to 7.3 ms (the marks 1.34, the whole screen summed 6) | 0.12 ms (the compass painted again as the bearing turns, 80 us; its box of about 4000 pixels summed, 50 us) |
       | Globe: the kernel on the GPU | 6.1 ms, 891 words | 5.79 ms, 897 words (the ring's in them) |
       | Globe: Mahere's draw a frame | 9.2 ms | 6.1 ms (4.4 of it waiting on the GPU) |
       | Town: the overlay a frame | 50 to 70 us | 50 to 90 us |
       | Town: a frame whose panel is painted again (the zoom's depth changing, 2 to 6 in 10 s) | 7.2 ms (the paint 2.2, the sum 5) | about 5 ms (the paint 2.1, its box 1, the sum of about 800000 pixels 1.9) |
       | Town: the kernel; the draw | 4.53 ms; 4.8 ms | 4.57 ms; 4.9 ms |
       | On the glass | 60 a second | 60 a second (the panel's rate) |
   - **What 120 on the Pro still needs, after the fourth pass:**
     - The display's publish of a whole-screen app frame (10.5 to 13 ms of wall on the Pro) and the panel stage's owner's call above: the slowest stage by far.
     - The kernel: 4.6 ms at a town and 5.8 at the globe on the 8a's 2.6 Mpx; the Pro's 4 Mpx at its clock wants it near 4 ms: f16 for the light and the colours, and branches over what a warp does not need.
     - Mahere's CPU a frame, now about 1.7 ms beside the kernel: the plan 0.9 to 1.1 ms (its 3500 points), the kernel recorded and compiled again every frame 0.33 to 0.47 ms (the same words whenever the constants are; worth a cache keyed on them).
     - The panel painted again as the zoom's depth changes: 2.1 ms of paint and 1 ms finding its box on those frames; whether a closed panel shows anything the depth changes is to check (its key holds the depth either way).
     - Measured on the Pro with cells, the network up and the full heap: still not (the 8a ran without cells again).
   - **Fifth pass (2026-11-12): the kernel compiled once, f16, and branches** (Mahere ferros-kernel, valhall in this repo):
     - **The compiler (valhall::ssa), for every kernel:** constants by value (Mahere made 0 and 1 dozens of times, each its own uniform word: 91 of the town kernel's 615 words were constant moves, 13 after); a constant added to a load's index goes into its byte offset, so a table's row reads from one address, its loads under one wait, and the high word of the address is set once; max(x, 0) and min(that, 1) become the destination clamp of the add or multiply that made x (ISA.xml's `Clamp`, bits 33:32). The town kernel 615 words to 490 and the globe's 821 to 570 before any change to Mahere, every pixel the same code as before in the emulator.
     - **Params** (`ssa::param`): a value placed as a constant is (a uniform word, or an immediate where the uniforms are full) but never folded, so the words and the recording's key (`Recording::key`, FNV over the operations, constants by bits, params by index) do not depend on it. Mahere's frame values (the camera, the face's row, the light, the sun, the pole and disk, the inks, the depths, where its tables are) are params; gpu.rs keeps up to 2^3 compiled kernels by key, and a frame of a structure met before binds its params into that one's words (`Compiled::bind`: the uniform words and the patched immediates) with no compile. The structure is the layers, one face or several or the fold, the ring and the overlay: two compiles in a run of the bench, every other frame bound.
     - **Blocks** (`ssa::when`): code that runs only where a mask is set, a BRANCHZ.eq with reconverge over it, the block's last instruction (or a NOP after a load's wait) reconverging; its values merged by the mask after it. In Mahere: the whole map off the globe and under an opaque overlay, the terrain's shading where no cell stands in and the loading mesh where one does, the vector planes' land cover where there is none, each line where there is one, the water where there is any, the ring's coverage beside it, the overlay's table where it is clear.
     - **f16** (`ssa::H`, v2f16: FADD, FMA, FMIN, FMAX; an f32 narrowed into a half by FADD.f32, one instruction for a broadcast; a half read by an f32 instruction as its widen, by a v2f16 one as its swizzle, at no cost): the light (the quadratic form, red and green in one pair and blue with the sun's diffuse in the other, 18 paired FMAs for the CPU's 30 and its 3-term dot), every colour after it (red and green, blue twice), the line, land and hypsometric tables as f16 pairs (two words a row for four). Coordinates, the cell lookup, the elevation and the contours stay f32 and integer.
     - **Checked on the host** (valhall's emulator, now with the v2f16 forms, widens, half destinations, clamps and NOP): the frame against the CPU raster as before (7153 of 7680 pixels the same code, 7505 in f32; still 2 past four codes, the largest difference the same 39), the seam, the pole, the fold, the ring and the locate checks unchanged; a bound kernel the same words and uniforms as one compiled afresh for a pan and a pinch. valhall's own: f16 lanes and clamps against the same steps on the CPU, a block divergent within a warp and one skipped by whole warps (`sim::run_marked`: what a warp issues is the union of its invocations'), 40 params against 28 uniform words bound three ways, Mesa's assembler vectors for the widen, the v2f16 swizzle, the clamp and NOP; the wallpaper's GPU twins and tapuwae's kernels compile and pass as before.
     - **Issued a warp** (the emulator, 96 × 80): a town with every cell 615 to 421, a town with none 611 to 236, the globe with none 898 to 135.
     - **Measured on the 8a** (the same bench and builds as the fourth pass, both from ferros-gpu 2c6db2c with a 128 MiB heap; nobody attested, so no cells: the loading mesh everywhere, and the terrain's f16 light not run on the device):

       | | Before | After |
       |---|---|---|
       | Town: the kernel on the GPU | 4.55 ms, 611 words | 2.99 ms, 482 words |
       | Town: recording and compiling a frame | 333 to 343 us | 38 us (record 28, find and bind 8, the words 2) |
       | Town: Mahere's draw a frame | 4.85 to 5.0 ms | 3.34 to 3.52 ms |
       | Globe: the kernel on the GPU | 5.79 ms, 898 words | 1.88 ms, 628 words |
       | Globe: recording and compiling a frame | 466 to 473 us | 64 to 68 us |
       | Globe: Mahere's draw a frame | 6.1 ms | 2.04 to 2.11 ms |
     - The two kernels' times against what they issue fit a floor of about 2 ms a frame plus 4.2 ns a warp instruction on the 8a: the floor is about what the frame's memory takes (8 bytes a pixel written, the overlay's 4 read: 31 MB, 1.8 ms at 17 GB/s). With cells the town's 421 a warp puts the 8a's kernel near 3.8 ms (4.55 before, which did not depend on what it drew).
   - **What 120 on the Pro still needs, after the fifth pass:**
     - The Pro's 4 Mpx (1.55 times the 8a's): the town with cells near 5.8 ms by that fit (7 before), without 4.6, the globe 2.9. The memory floor alone is about 3 ms there: a 32-bit surface (the working values' 16 bits a channel are the display's) or the overlay read only where its tiles hold anything would cut it; then what a warp issues with cells (the view normal and the cell lookups are f32 and integer, about half of it).
     - The display's publish of a whole-screen app frame and the panel stage's owner's call, as before; measured with cells on the Pro, as before.
   - **Sixth pass (2026-11-12 night): the map whole in a small block, and still when nothing moves** (Mahere branch `ferros-morning`, off ferros-gpu; measured on the 8a with an identity attested, the network up and cells from the vault and the bucket, until the 8a went silent on USB at 22:25 under another session's kernel flash):
     - **A kist's block is what the kists started first left.** The 8a gave Mahere 64 MiB with Photon running and 32 MiB with Photon and Lumis (the attest screen holds a quarter; `NewRegion` takes the largest block free at launch, whoever launched first). At 1080 x 2400 a 64 MiB block held 26 cell slots, 32 MiB 13.
     - **Starved slots drew the loading mesh.** The globe's plan named 268 squares' cells at depths 1 and 2; each frame placed the first 26 planes and left the rest to the mesh (302111 planes wanted with no slot free in ten seconds), so the globe was a patchwork. Now Mahere's `fit` holds each stand-in to the finest cell the pool has at or above a cap, the cap the deepest that fits; the ten depth-0 cells always fit, so the map is whole, softer where the slots are short. Measured: the globe whole from depth 0, the starved planes stopped at the first frames' 145.
     - **7 of each frame region's 19 MiB were squares no plan names.** The overlay moved from 8 MiB into the region to 1 MiB (192 Ki squares, about twenty times the most a 1344 x 2992 screen names); a region at 1080 x 2400 is 12 MiB. Two regions only when they leave the globe's 20 slots and two cells more, else one with the frames unpipelined. 64 MiB now holds 40 slots and two regions, 32 MiB 20 slots and one (measured on the 8a: the globe whole at 32 MiB).
     - **A still map drew at the panel's rate.** With the globe at rest, 480 of 480 ticks asked for a frame, every one the panel: a font pack arriving marks Mahere's panel dirty until a paint loads it, and the GPU host painted the panel only when its own key moved, so it never did, and the kernel (3.1 ms) and a show ran every frame for ever. The panel's own dirtiness now repaints it (`DeviceApp::panel_changed`). Built, not yet run on a device.
     - **Memory.** At the globe Mahere's arc reached 538 MiB within eight seconds of launch with the pool's cells at 190 MiB; the 8a ended once at the 640 MiB ceiling on an 11.9 MB allocation, the Pro on "memory allocation of 33554432 bytes failed" (a 17 MB font pack's buffer doubling to 2^25 in `read_to_end`). Fixed: a body of known length read into a buffer of that size, and font packs kept out of kete's read cache (which held a second copy of each, up to 64 MiB). A counting allocator now says the heap held and at most every ten seconds beside the pool. Built, not yet run on a device.
     - Kernel time with cells at the globe on the 8a: 3.07 to 3.25 ms (682 words); Mahere's plan from its cache 41 to 65 us a frame; first view complete 8.4 s after launch, 101 cells, fonts from the vault 0.26 to 0.62 s each.
     - Open for the owner: the arena's blocks go first come, first served (with Photon and Lumis started first Mahere got 32 MiB, whether or not they draw on the GPU), and a kist's spare surface is one of four (on the Pro Photon did not start, "no free surface (four are in use)", with Mahere's two and Lumis's one taken). A block and a spare surface given when a kist first calls the GPU service, not at launch, would leave both to the kists that draw on it.
   - **Seventh pass (2026-11-13): GPU memory from the frame pool on demand** (owner on the 8a: "mahere does not load high rez"; the coordinator's call, the owner may resize it; Mahere branch `ferros-gpub`, off ferros-morning):
     - **A block is frames.** The fixed 128 MiB arena is gone: a client's GPU block is a slot of the blocks' window (cachemap `GPUB_VA`, G#58_0000_0000, sixteen slots of 512 MiB of addresses), mapping 2 MiB frames of the pool from the slot's start, taken from the pool's top zeroed and wiped as they go back, as surfaces are (WINDOWS.md "Surfaces on demand"). A block grows in place, so a client's pointers and its kernels' offsets from `ARENA_VA` never change, and each slot is aligned to the ceiling, so a block of any power of two is a handle for masked code.
     - **Asked for, not left over.** A spawn's `layers` gives 128 MiB, `layers-32` and init's `arena` 32 MiB, a kist 32 MiB at launch (halved down to 32 MiB when the pool cannot give it). Its holder asks for more with SYS_DMA x1 = 2 (2^x2 bytes, `ferros_sys::gpu_block_ask`), up to the ceiling `GPU_BLOCK_MOST`, 512 MiB, a constant; refused with why on the console ("gpu block: ... refused") past the ceiling or when the pool would keep fewer than 64 MiB free. A kist that never draws on the GPU holds 32 MiB.
     - **The fence follows the frames.** Each frame is admitted thru the GPU's S2MPU as it is given and shut out as it goes back (`gpu_admit`, `gpu_sync`; `gpu_refence` when the fence is made again); an ended client's block waits for its GPU job to drain (`gpu_drain`); a block given back maps the blank frame. The kernel's stamp carries a block's frames after its region as it does a surface's (ferros_sys::gpu `Stamp` reads them from the message, `Key` names a client's memory), and the GPU service maps them for that client alone, in 2 MiB level-2 entries from `ARENA_VA` (`BLOCK_RW`, kbase's ENTRY_IS_ATE_L02: the first level-2 blocks this driver has used).
     - **The old arena** (G#9800_0000-G#A000_0000) is the pool's `POOL_LAYERS` on both boards and after jumps (memblock: plain memory, G#9740_0000-G#B61F_FFFF); the GPU service's arena self-test (input bit 6) goes with it.
     - **Run on the 8a** (the kernel flashed, the service stored, Mahere stowed as a kist and launched from the deck, cells from the network): the pool 2785 frames (64 more, the arena's); ahau's attest mark drew in its 32 MiB block of frames on the GPU; Mahere grew from 32 to 512 MiB in 20.1 ms under the kernel lock, 4470 MiB of the pool free after; 488 cell slots, 0 planes wanting a slot, 0 frames capped, the plans 0 depths short of the scale's (two short on ferros-morning in its 32 or 64 MiB); the kernel 4.5 to 5.2 ms a frame (13 faces at a regional view, 1 at 20 km); killed, its 512 MiB went back to the pool at once; the GPU's S2MPU no fault, its sync never late.
     - **What held the map short then was Mahere's CPU pool.** At its 192 MiB budget the pool sat at 101 cells with no cell arriving and the map drawn from coarse stand-ins. With 384 MiB when the GPU draws (Mahere `GPU_POOL_BUDGET`) it filled to 211 cells and the heap held 448 MiB at most, but the arc grew to 674 MiB of its 692 MiB ceiling (430 at 192): the owner's call whether to keep that, lower it, or keep a cell's planes on the GPU alone once written there.
5. **Lumis's preview, zero copies:** camera DMA into pinned frames inside both the ISPFE and G3D fences, a kernel from the CFA to a layer; Lumis must first build for ferros.
   - Proof: no CPU copy, CPU time a frame near zero, the sensor's frame end to the layer measured.
   - Size: 200 lines of kernel, 500 in the camera path.
6. **The kist side:** kernels in the capsule, `gpuc` at stow, sealed objects, the ops for kists; production gives kists the GPU once their kernels are checked (no `gpu` in RIGHTS, no settings rows: 1.6).
   - Proof: a test kist stowed, asked once, running; denied, an empty slot; revoked, respawned without it; Mahere launched as a kist with the right.

## 3. Lumis on the GPU (2026-11-20)

The Lumis session's asks, sent at the owner's ask. The owner frames the GPU as "NEON on steroids": compute only, one job kind, a gather over a grid. Lumis's pipe: (a) a camera layer, a kernel gathering each screen pixel's sensor cell straight from the camera ring, the colour matrix applied and encoded (fit, 1:1, PHASE, Slitscan); (b) a UI layer, one kernel over a small shape list and an 8-bit glyph atlas; (c) the display composing them with `under` on NEON.

Its asks in its order, and where each stands:

| | Ask | Built |
|---|---|---|
| 1 | the camera ring read-only in the holder's GPU space, no copy | yes ("The camera ring on the GPU"); the owner: "I think it should always be allowed, unless it's a security issue" |
| 2 | layers as mailboxes, two or more a kist, each with its tile coverage | layers and their mailboxes yes, from windows the display already stacks ("Layers"); the coverage's form yes, the display reading it no |
| 3 | a submit ring written from the kist's own thread, a completion word and doorbell, no call a job | yes ("The queue") |
| 4 | a display frame event | yes ("The display's frames") |
| 5 | the GPU's power and clock up by events, never timers | yes ("Power and clock by events") |
| 6 | Rust-recorded kernels now, with integer multiply-add, shifts, 8- and 16-bit loads, atomicAdd, paired f16 | yes ("The back end") |
| 7 | per-job GPU time with the completion | yes (the GPU's own stamps) |
| 8 | faults to the kist with address and kind; a bad kernel ends its kist's work only | yes |

Everything below is built and host-tested; nothing of it has run on a device yet ("To run on a device").

### The queue (asks 3, 7, 8)

**The ring is the client's own memory.** A queue lies in the client's GPU block at an offset it picks (ferros_gpuq):
- a header page: `SUBMIT` (the jobs submitted, the client's alone), `DONE` (the jobs ended, the service's alone), `ENDED` (why the queue ended, 0 while it runs);
- 2^depth records of 1 KiB (depth at most 2^4), job n in record n mod 2^depth: its kernel handle and uniform count, its grid, its uniforms (read by the GPU from the record itself: no copy), and a result line (its status, the GPU's own stamps of its start and end, when the service rang its doorbell and saw it end, a fault's address and status).

**A job, with no call.** The client fills a record, stores `SUBMIT` = n + 1 and wakes that word (SYS_WAKE on a word of its own block, new: below). The service's thread for that client's slot, asleep on the same word, writes the job's command-stream words into the client's own stream (ferros_gpuq `job_words`: the run's words exactly as since 2026-11-04, plus STORE_STATE timestamps before the run and after its wait) and rings the client's doorbell. At the GPU's job interrupt the service's events thread lands every job that ended in order (the stream sets the slot's sequence word as its last act), writes its result, stores `DONE` and wakes that word, where the client sleeps. A call is made only to open a queue, load a kernel, close a queue or look at the client's memory again.

**One path.** Calls (op 1) and queue jobs go into the same stream of the same group in the same address space, end with the same sequence word and are landed by the same thread; the old one-job-at-a-time loop is gone (payloads/gpu serve.rs). Clients' groups now run at once on the GPU, the firmware choosing by priority (a system client's 15, a kist's 8).

**Per-job GPU time (7).** STORE_STATE with the timestamp state (genxml v11 "CS STORE_STATE", opcode 40) at r76:r77 + 8 before RUN_COMPUTE and + 16 after its WAIT, synchronous (wait mask 0). The GPU's timestamp (GPU_TIMESTAMP, G#98, panthor_gpu_regs.h) is the system counter; the service reads its lead on the CPU's counter once at its start and writes the stamps back in the CPU's counter ticks, so `Ended::gpu_ticks` is the GPU's own time and `started - submitted` the doorbell to the GPU's start. The stamps are used only when the stream has the registers (CS_FEATURES' work registers, read at the start) and the timestamp reads nonzero; the service says which on the console. Calls answer the GPU's own µs after the service's.

**Faults (8).** A page fault in the client's own space (the MMU's line, as since 2026-11-12), a command-stream fault, or the job's guard passed (2^8 ms a kist's, 2^10 ms a system client's, counted from when the job became the oldest of its slot: the events thread's one deadline) ends that client's group and its queue: the failing job's status says which (`S_OUTSIDE` with the GPU address and AS_FAULTSTATUS, or `S_FAILED`), every later job `S_LOST`, `ENDED` = `E_FAULT`, and the client opens the queue again to go on (`Queue::reopen`). No other client's work is touched: its own group, its own space.

**A client that ends with jobs on the GPU.** No call is held for a queue's jobs, so the kernel's drain (a call the service has taken: `gpu_job_in_flight`) cannot see them. The service holds the client's epoch instead from the queue's open (SYS_DMA x1 = 7 on `arena`, with the queue's `SUBMIT` word): a held epoch's blocks and surfaces stay drained past its end whatever requests the service takes, until the service lets it go. At the client's end the kernel sets bit 63 (`GONE`) of that word and wakes it, so the slot's thread finds its client gone even when the end falls between its look and its wait, asks whether the epoch lives (x1 = 8), ends its group and lets it go, and only then does the kernel return the memory. At the service's own end the kernel marks every held queue's `DONE` the same way, so no client waits for ever (`ended()` says `E_SERVICE`), and what the holds kept stays drained until the next service's first request (after its GPU's reset), as a call's always did.

**The kernel's words between processes.** SYS_WAIT and SYS_WAKE take a word of a GPU block when the caller holds the block or the whole blocks' window (the GPU service): waiters are kept under the word's window address, the same in every holder's view, as a mailbox's are (process.rs `gpub_word`).

### Power and clock by events (ask 5)

The owner's decision of 2026-11-11, "the GPU powers up when work is queued and gates when its queue empties, as events, never a timer", built:
- a power switch, `gpu-clock` (the GPU service's grant 9; process.rs `POWERS`, service.rs `gpu_clock`): on, the GPU at its top (G3D 890, G3DL2 900 MHz) while the screen is lit; off, its floor (150 / 160 MHz). Quiet (no console line a turn); `diag clock` counts the turns and the longest.
- the service turns it on when its first job is put in flight and off when its last lands (serve.rs `fly`, `land`, `WANT`), from a thread that lives on the kernel's core (every switch runs there), one syscall a turn. It puts the GPU at its floor when it starts.
- the kernel's old rule (the top while a `gpu` process lives) holds until the switch's first use, for a service from before.
- The shader cores' own power is the firmware's: it powers them down 800 µs after they idle (global init), as before.

What it costs is to measure: a turn is two ACPM requests and their answers on the kernel's core, and a kist working once a frame turns it up and down every frame. If that costs, the down turn can wait for a display frame with nothing queued (the frame mailbox below): the owner's call, with the numbers.

### The camera ring on the GPU (ask 1)

The owner (via the coordinator, 2026-11-20): "I think it should always be allowed, unless it's a security issue", with the conditions that make it the same as the CPU view the holder already has. Each built:
- **The holder alone.** The kernel puts the camera view in the GPU stamp of its holder's calls only (process.rs `gpu_cam_stamp`: the view's region in its window and its frames), never another's.
- **Read-only, twice.** The GPU's S2MPU admits the view's frames with the read nibble alone (s2mpu `gpu_admit_ro`, G#4 a page), from the holder's first GPU call carrying the view; the service maps them read-only (a 2 MiB block with AP's read-only bit) at `CAMERA_VA` (G#1_0000_0000), the view's first frame there and the rest after it as the CPU sees them. A store there faults in the client's own space (`S_OUTSIDE`).
- **Non-cacheable on the GPU.** The front end writes the ring without snooping, and a line the GPU's L2 kept from an earlier lap would be that lap's (each job ends with an L2 clean, not an invalidate). Whether a cacheable mapping with an invalidate per job is faster is to measure.
- **Gone before the frames serve anyone else.** A view taken back (the camera's CLOSE, its holder's end, a view given anew) whose holder's GPU calls carried it makes that epoch stale and rings the service's bell (SYS_BELL source 15, `BELL_GPU_CAMERA`, the `arena` holder's alone). The service then, under its lock: ends that client's group if a job of it is in flight (stopped before the unmap; its queue ends `E_CAMERA`), makes its tables again without the camera and installs them with LOCK and FLUSH_PT (the GPU's TLB and walk caches dropped), and says so (SYS_DMA x1 = 9). Only then does the kernel shut the frames out of the fence. Meanwhile no stamp carries a camera view, a view given anew shuts the old frames out of the fence before its own frames land in them, and a camera run given back waits out of the pool. Until the service's word the old frames stay admitted, so a job still reading one is stopped by the service, never by a fence closed under it (a bus fault the GPU may not ride out); the one exception is a new OPEN before that word, which shuts them at once.
- **After the camera's OPEN**, the client calls `Queue::refresh` (op 5) and its space maps the view; its jobs then read a buffer at `CAMERA_VA` plus the buffer's place in the view (the MAP header's +48). Wait for the queue's jobs that read the camera before a CLOSE: one in flight is stopped when the view goes.
- The service's slots now make a client's tables again in place when its memory changes (two pools of table pages a slot, the running group moved to the new tables at once): a block grown, a surface or a camera view given or taken no longer binds a second slot to the same client.

Not done: ferros_clip still copies each frame into its block (1.7 ms) for the band coder; with the ring mapped it can read the frame in place (its kernel's frame address a uniform: the change is ferros_clip's).

### The display's frames (ask 4)

A mailbox (KERNEL.md "Mailboxes"): `frame` (ferros_mailbox `FRAME`, layout `frame`: the frame starts and the pictures armed since boot, the header's time the start's counter). The kernel is its one writer: at every display frame start it takes for the display (DECON0's frame-start interrupt, process.rs `irq_pending_take`) it publishes and wakes every reader (`frame_mailbox_flush`). Anyone opens it with no proof (`mailbox::Reader::open(mailbox::FRAME, 0, &mut token)`): when a picture starts on the glass is what everyone sees.

On the Pro a frame starts only when the display arms a picture (the panel holds its image between), so at rest nothing is published; on the 8a the controller sends one on every TE, 120 a second at rest. A kist with nothing new does not need a frame; one with something new (a camera frame) shows it and the display's frame follows.

### Layers (ask 2)

The display already composes every window as a layer, front to back, chains cut at the first opaque window (WINDOWS.md "The layer stack"), and takes a window's pixels from another surface of its owner's with each show (op 22's 36-byte form: a mailbox). So a kist's layers are its windows:
- **Surfaces asked for at run time** (SYS_DMA x1 = 10 on a surface it holds, `ferros_sys::surface_ask`): up to four a process, each its own capability, admitted to the GPU's fence and mapped in its GPU space as every surface is. Two layers of two buffers each.
- **A layer's mailbox show**: `window::show_buffer(display, window, buffer, ...)`, answered once the display has taken it, so the buffer shown before is the kist's to draw into again.
- **The stack's order**: a window keeps its place across shows that leave its place, level, format and mode as they were, so a kist shows its camera layer first (behind) and its UI layer second (in front) once, then swaps buffers in each.
- **Partial α** (the UI's antialiased edges over the camera) is fluor's 16-bit α + darkness (format 1), which the display folds with `under`; working values are opaque or see-through.
- **Tile coverage** (COMPOSITOR.md rule 2): the form is ferros_gpuq `coverage`, a byte a 64-pixel tile (empty, partial, opaque, and a changed bit) past the pixels in the layer's surface, made by a kernel counting each tile's pixels with an atomic add. The display does not read it yet (it composes a window by its rectangle and the show's changed rectangle): the display's change, its own session's.

### The back end (ask 6)

valhall's recorder (valhall::ssa), each new form checked in the emulator against the CPU (tests/sim.rs `lumis_ops_against_the_cpu`) and its encoding against Mesa's vectors (`the_atomic_and_narrow_loads_encode_as_mesa`):
- `ssa::load8`, `ssa::load16`: LOAD.i8 and LOAD.i16 with the w0 lane, zero-extended to 32 bits;
- `ssa::atomic_add(slot, index, shift, offset u8, value)`: ATOM.i32.aadd, no value back (opcode G#068, the operation at 25:22; Mesa's `32 00 80 18 02 4c 68 08`);
- `U::mul_add`: IMUL.i32 then IADD.u32 (Valhall has no fused integer form);
- shifts by constants and by values, and paired f16 (`H`), as since 2026-11-12.

On the device: gputest input 15 runs the same kernel over 2^11 invocations (a histogram of atomics from every warp at once) and compares every word with the CPU.

Kernels stay Rust recorded, compiled where the kist chooses (ferros_clip compiles on the host and ships words: the owner's 2026-11-19 decision for the RAW codec); the device runs no Mesa.

### The API Lumis uses

```rust
use ferros_sys::gpu::{queue::*, ARENA_VA, CAMERA_VA, PRELOAD_X, PRELOAD_Y, kist_slots};
let (service, block, spare) = kist_slots(wants);                      // the kist's grants
ferros_sys::gpu_block_ask(cap(g, block), 27);                         // grow the block first: a block grown later is another client
let mut q = Queue::open(cap(g, service), cap(g, block), QUEUE_AT, 2)?; // the ring at QUEUE_AT in the block, 4 deep
// the kernels' words written into the block (valhall::ssa::compile()'s `words`), then:
let cam_k = q.kernel(CAM_CODE_AT, cam_words, PRELOAD_X | PRELOAD_Y).unwrap();
let ui_k = q.kernel(UI_CODE_AT, ui_words, PRELOAD_X | PRELOAD_Y).unwrap();
// after the camera's OPEN:
q.refresh();                                                          // the ring mapped read-only at CAMERA_VA
// the layers: two windows, each a mailbox of two surfaces
let mut t = [0u8; 32];
let cam_b = ferros_sys::surface_ask(cap(g, spare), &mut t).unwrap(); // and two more for the UI layer
// a frame: wait on the frame mailbox (or a camera frame), then
let n = q.submit(&Job { kernel: cam_k, groups: (w / 16, h), uniforms: &[CAMERA_VA + place, layer_va, ...] }).unwrap();
let e = q.wait(n);                                                    // e.status, e.gpu_ticks(), e.fault_address, e.fault_status
window::show_buffer(display, cam_window, cam_buffer, 0, 0, w, h, window::APP, 1, changed, 0);
```

A surface's GPU address is `SURFACES_VA` plus its address less `SURFACES_PA` (as Mahere reckons it); a kernel's uniforms are its buffers' GPU addresses and its own constants (`Compiled::consts`).

### Measured on the host

- ferros_gpuq (tests/ring.rs): the job's words without stamps are the old run's word for word over five grids; the stamps decode as genxml's STORE_STATE (offset 8 and 16, timestamp state, r76, synchronous) around the run's WAIT; the protocol thru a ring of depth 2^2 between two host threads, 2^16 jobs in order, none lost, 254 ns a job (spinning, no syscalls: the protocol's own cost, for scale); the `GONE` marks; the coverage's classes and its room on both boards.
- valhall: all 10 emulator tests and 11 builder tests pass, the two new ones among them; ferros_clip's band coder kernel unchanged (tests/gpu.rs against `encode_band` on a Pro frame).
- The kernel, the service, gputest and ferros_sys build for the device.

What the host cannot measure is the asked-for number, submit to done on the GPU: that is the device's (below).

### To run on a device

Each needs this branch's kernel (RAM-reloaded) and its GPU service stored (`ferros-bridge capsule gpu.bin gpu.capsule.vsf`, `store put gpu @gpu.capsule.vsf`; the kernel starts it at home). Build from target/agent-gpukists/build.sh (`kernel`, `ferros-payload-gpu`, `ferros-payload-gputest`).
1. `spawn gputest.bin 0E --name gputest --use gpu --dma layers-32`: the empty kernel thru the queue one at a time (submit to wake, doorbell to seen, the GPU's own time, doorbell to the GPU's start; 2^8 each), four in flight, thru calls; the band coder over a full binned frame thru the queue and thru calls (submit to wake, the GPU's time); the Mandelbrot thru the queue, every pixel against the CPU; a job storing outside its block (its address and status, the job behind it lost, the queue opened again and running). Then `diag clock` for the clock's turns.
2. `spawn gputest.bin 0F ...`: Lumis's ops on the device against the CPU.
3. `spawn gputest.bin 10 ...` with the display presenting: a job a frame, the frames' spacing, the wake's lateness, frame start to the job's end.
4. A client killed with its queue open (`diag "kill gputest"` mid-run of 1): the service's line "ended with its queue open", the block back in the pool (`diag arcs`).
5. The camera (needs a camera holder that calls the GPU: Lumis with the queue, or a camtest mode): its view mapped (op 5 answers 1), a kernel reading it, a store into it faulting, CLOSE with a job in flight (its queue ends `E_CAMERA`), the service's settle line, the frames shut out after it, a second OPEN by another holder reading nothing of the first's.

6. The camera on the GPU: `spawn camtest.bin 1001000B00 --name camtest --use camera --use gpu --dma layers-32` (16 frames of the main).
7. The fence after each: `diag "gpu fence"` (the GPU S2MPU's latched fault, decoded, then cleared).

### Measured on the 8a (2026-11-20)

This branch's kernel RAM-reloaded on the 8a (it ran forge fe1783a0 flashed, through the hv's gate; identity unlocked at boot each time, nothing of it touched), its GPU service stored (the one before in keys/akita/capsules-before-gpu-kists-2026-11-20/gpu.capsule.vsf, blake3 cee7a99c...). The GPU's timestamp is the CPU's counter (24 ticks apart, a read's width), so the job stamps are on.

**Submit to done** (gputest 0E, 2^8 jobs each unless said; least / mean / most, µs; the last of three runs, the others alike):

| | submit to the client's wake | doorbell to the service seeing the end | the GPU's own time | doorbell to the GPU's start |
|---|---|---|---|---|
| empty kernel, one at a time, thru the queue | 62 / 88 / 751 | 29 / 50 / 704 | 2 / 5 / 359 | 8 / 16 / 269 |
| empty kernel thru calls (op 1) | 71 / 126 / 225 (call to reply) | | | |
| empty kernel, four in flight | 28 µs a job | | | |
| ferros_clip's band coder, a binned 4080 × 3072 frame, thru the queue (16) | 6203 / 6657 / 6993 | | 5966 / 6384 / 6729 | |
| the band coder thru calls (16) | 5541 / 6273 / 7056 | | 5280 / 5982 / 6688 | |

- The queue's own cost over the GPU's time is about 90 µs for an empty job (mean) and about 270 µs for the band coder (its stamp at the run's end comes before the L2 clean of the 25 MB it wrote, so that clean is in the difference). Four in flight, 28 µs a job.
- The Mandelbrot thru the queue: 0 of 4096 pixels off the CPU's. A store outside the block: status 4 with the address and AS_FAULTSTATUS (a translation fault), the job behind it lost, the queue ended and opened again, the next job done.
- Lumis's ops (0F) on the device: 0 words wrong of each (multiply-add, shifts, the f16 pair, the narrow loads), the histogram's 16 atomic counters and its sum exact, from 2^11 invocations.

**A kernel a display frame** (gputest 10, 2^7 frames; the 8a's panel at 60 a second in ferros): the frames 16.71 ms apart (16.37 to 17.05), none missed; the frame mailbox's reader woken 4 / 10 / 29 µs after the publish; frame start to an empty job's end 454 / 482 / 613 µs with the clock turning, 440 / 474 / 574 with it held up (op 6). Most of that half millisecond is the shader cores' power-up: the firmware powers them down 800 µs after they idle, and a job a frame finds them down every time (back to back, an empty job's round is about 90 µs).

**The clock's turns** (`diag clock`): a turn 566 µs on average (744 the longest), two ACPM requests and their answers on the kernel's core. A job a frame turned it up and down 114 times in 128 frames: about 1.1 ms of the kernel's core a frame, at 60 a second about 7 % of it. The job itself did not notice (8 µs on the mean). The coordinator's to decide with these.

**A client killed with its queue open** (twice, once in the band coder's run): the service's line "slot k's client (epoch e) ended with its queue open: its group ended, its GPU work let go", its 128 MiB block back in the pool (`diag arcs`), the fence clean.

**The camera on the GPU** (camtest 11, the main, 4624 × 3472, 5824 bytes a line), three runs:
- mapped read-only: `refresh` said mapped; every frame copied whole by the GPU from `CAMERA_VA` (19.7 MiB) equal to the CPU's read of the view, 0 words of 5,055,232 different in 48 frames;
- non-cacheable against cacheable with the L2 and LSC invalidated before each job: a whole frame's copy 1588, 1582 and 1322 µs non-cacheable, 923, 1099 and 896 cacheable (8 frames each); the invalidate costs less than it saves;
- a store into the ring: a permission fault in the holder's own space (AS_FAULTSTATUS G#10003CA), the queue ended, opened again;
- CLOSE with a long job reading the ring in flight (133 to 154 ms, the driver's close): the service's line "the camera view went with a job in flight: its group ended, its queue ended", the job lost, the queue `E_CAMERA`; after it `refresh` said unmapped and a job reading the ring took a translation fault; the run went back to the pool after the service's settle each time.

**Found and fixed on the device: the GPU's caches after a group ended mid-job.** After a camtest run (its long job stopped at CLOSE), the next client's job (0F) left the GPU's fence a latched fault: a write, refused, at G#8_9240_1A40, the line of the camtest queue's record whose job was stopped (its start stamp). Reproduced the same way twice. The stopped job's stamp sat dirty in the GPU's L2 (no end-of-job flush ran), and the next client's end-of-job flush wrote it after camtest's frames had gone back to the pool: refused there because the frames were out of the fence, but frames given to another client and admitted again would have taken it. The service now flushes the GPU's caches (GPU_COMMAND FLUSH_CACHES, L2 and LSC cleaned and invalidated) after every group it ends with a job in it, before the memory may go; the same sequence twice since: no fault. No other run left a fault (0E, 0F, 10, the kills, camtest, checked after each).

**The flashed kernel with the new service** (the 8a rebooted to forge fe1783a0 at the end, the new `gpu` stored): the service starts ("the clock the kernel's (no gpu-clock grant)"), calls work (gputest's Mandelbrot, 0 of 4096 off), a queue's open is refused cleanly (status 1: that kernel has no hold), the fence clean. So the new service stays stored.

## 4. Open questions for the owner

0. **The kernel's form in a capsule:** WGSL text means a parser for untrusted text on the device (the rule against string parsers); proposal: WGSL parsed on the developer's side, the capsule carrying a typed form (naga's IR as VSF, or SPIR-V), validated on the device.
1. **One source language?** System kernels stay Rust recorded (one source for NEON and GPU), or move to WGSL so every kernel takes the kist path.
2. **The wallpaper's promise:** wallpaper/src/lib.rs promises the same pixels bit for bit everywhere; on the GPU, "looks the same" or still bit for bit (which costs the fast forms)?
3. **Keep `wallpaper.made`?** If the GPU makes it in well under a second, store it (24 MB, a 2.9 s read) or make it at each boot?
4. **No GPU service after a jump:** the wallpaper and the mark then have no path; black until the GPU, or the jump brings the service up?
5. **The `gpu` right:** a right at all (the MMU separates kists; power, heat and timing side channels remain), default off? Closed: no right (2026-11-11, again 2026-11-12).
6. **Bounded loops** required, or any loop with the progress timer alone?
7. **The clock:** 890 MHz whenever a `gpu` process lives (today), per job, or by queue depth; and idle gating.
8. **naga** as a dependency, or our own WGSL front end?
9. **The clock's down turn** (section 3, "Power and clock by events"): at the queue's emptying, as built, or at a display frame with nothing queued since the last, if a turn a frame costs more than it saves. To decide with the device's turn counts and the longest turn. Measured on the 8a: a turn 566 µs of the kernel's core, about 1.1 ms a frame at a job a frame; the job's own latency unchanged (section 3, "Measured on the 8a").
10. **The display reading a layer's tile coverage** (section 3, "Layers"): the form is set (a byte a 64-pixel tile past the pixels); skipping a layer's empty tiles and stopping at its opaque ones is the display's change, not built here.
11. **The camera on the GPU cacheable** (section 3): non-cacheable by default; measured on the 8a, a whole frame read 1.4 to 1.7 times faster cacheable with an invalidate a job (896 to 1099 µs against 1322 to 1588). Making it the default is the next step's.
12. **A bad kernel ends its kist's GPU work, not its kist** (Lumis's ask 8): decided (the coordinator, 2026-11-20): end the kist's GPU work and report it; the kist decides whether to end itself.
