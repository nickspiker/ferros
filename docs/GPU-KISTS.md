# GPU kists: one compute path for the system and for kists

Status: study, design and plan, 2026-11-11; nothing here is built.
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

### 1.6 The right

- `gpu` joins RIGHTS, declared in the capsule with its reason.
- Asked once, ever, per person and kist (2026-11-07): at the install drop when the kist cannot run without it, else at the first action that needs it (2026-11-10).
- Denied, the slot is empty; revoked, kill and respawn without it.

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
1. **The one path's service, system clients only:** a group and address space per client, pinned buffers and layers with the fence per frame, held calls and bells by interrupt, MMU faults and the progress timer, priorities; clients wallgen, ahau, gputest.
   - Proof: gputest and ahau at once; a gputest kernel reading past its buffer faults alone while the mark lands; an endless kernel stopped with no host timer while the mark draws; the fence logs nothing.
   - Size: 2-2.5k lines in the service, 300-500 in the kernel.
2. **The attest mark wholly on the GPU:** `render_fresh` recorded as the bonded mark is; the display's glow and edge layers measured as candidates, moved only where they win.
   - Proof: the unbound mark's time against about 0.5 s on the workers, its difference measured.
3. **WGSL and the grown back end:** naga, the subset check, nested flow, atomics, register refusal.
   - Proof: the wallpaper in WGSL, compiled on the device, looks as step 0's; compile time and `gpuc`'s size measured; each new form checked on the device against the CPU, as the Mandelbrot was.
   - Size: 6-9k lines.
4. **Mahere's exact-coverage compose** as a WGSL kernel over resident cells.
   - Proof: full-screen frames under 8.3 ms at rest and panning, compared by eye with Mahere's nine samples.
   - Size: 1-1.5k lines of kernel, 500 in mahere-ferros.
5. **Lumis's preview, zero copies:** camera DMA into pinned frames inside both the ISPFE and G3D fences, a kernel from the CFA to a layer; Lumis must first build for ferros.
   - Proof: no CPU copy, CPU time a frame near zero, the sensor's frame end to the layer measured.
   - Size: 200 lines of kernel, 500 in the camera path.
6. **The kist side:** `gpu` in RIGHTS, kernels in the capsule, `gpuc` at install, sealed objects, the ops for kists, the settings rows.
   - Proof: a test kist installed, asked once, running; denied, an empty slot; revoked, respawned without it; Mahere launched as a kist with the right.

## 3. Open questions for the owner

0. **The kernel's form in a capsule:** WGSL text means a parser for untrusted text on the device (the rule against string parsers); proposal: WGSL parsed on the developer's side, the capsule carrying a typed form (naga's IR as VSF, or SPIR-V), validated on the device.
1. **One source language?** System kernels stay Rust recorded (one source for NEON and GPU), or move to WGSL so every kernel takes the kist path.
2. **The wallpaper's promise:** wallpaper/src/lib.rs promises the same pixels bit for bit everywhere; on the GPU, "looks the same" or still bit for bit (which costs the fast forms)?
3. **Keep `wallpaper.made`?** If the GPU makes it in well under a second, store it (24 MB, a 2.9 s read) or make it at each boot?
4. **No GPU service after a jump:** the wallpaper and the mark then have no path; black until the GPU, or the jump brings the service up?
5. **The `gpu` right:** a right at all (the MMU separates kists; power, heat and timing side channels remain), default off?
6. **Bounded loops** required, or any loop with the progress timer alone?
7. **The clock:** 890 MHz whenever a `gpu` process lives (today), per job, or by queue depth; and idle gating.
8. **naga** as a dependency, or our own WGSL front end?
