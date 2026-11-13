# GPU on husky

**Mali-G715 r1p2, 7 cores** (arch 11.8.10), at G#1F00_0000; IRQs SPI 346/347/348 (GPU/JOB/MMU). Kbase r52p0 drives it under Linux (google-modules/gpu/mali_kbase); Google's platform code in mali_pixel.

## Measured under ferros (2026-10-24 night), read only

ABL leaves both power domains on: pd-g3d status G#1546_1A84 = 1, pd-embedded_g3d G#1546_1C84 = 1 (plain reads; EL3's privileged read answers −22 for them). Then the GPU's own registers read safely:

| Register | Value |
|---|---|
| GPU_ID (+0) | G#B8A2_1020 (Mali-G715, r1p2) |
| L2_FEATURES (+4) | G#0814_0306 (as Linux logs) |
| AS_PRESENT (+G#18) | G#FF (8 address spaces) |
| SHADER_PRESENT (+G#100) | G#0111_0055 (7 cores) |
| L2_PRESENT (+G#120) | 1 |
| L2_CONFIG (+G#48) | G#0014_0000 |
| MCU_STATUS (+G#704) | 0 (the CSF microcontroller not started) |

## The path to one job (no TEE, no GSA, no signature)

The CSF firmware is `/vendor/firmware/mali_csffw-r52p0.bin` (294912 bytes, Arm's binary: Harbor and the vault, never the repo): magic G#C3F1_3A6E, interface entries {flags, VA start/end, data start/end}; a Cortex-M MCU (initial SP G#0203_3000, reset G#0080_1FB1), its shared interface page at GPU VA G#0400_0000 (global input G#0400_2000, output G#0400_6000). No signature or hash: only the magic and header version are checked; the protected section is skipped without a protected allocator. The only secure call in the drivers is SMC_PROTECTION_SET for secure rendering.

1. Power and clocks only if a cold path finds them off: `g3d_on`, `embedded_g3d_on` (flexpmu_cal_local_zuma.h), `pd_prepare(G#1EE1_0204)` once a boot; DVFS domains 7 (shaders) and 8 (L2, never below the shaders).
2. Soft reset (GPU_COMMAND 1, RAWSTAT bit 8), ASN_HASH and L2_CONFIG's slice hash, the workaround registers G#F00–F0C (values from a Linux capture), L2_PWRON.
3. The GPU MMU: AArch64 4K tables, address space 0 for the MCU, ≥1 for the job's group; non-coherent, so all GPU memory Normal non-cacheable (a dedicated ~4 MiB region).
4. Load the firmware's sections, MCU_CONTROL = 2 (auto), JOB_IRQ bit 31, GLB_VERSION ≠ 0, a PING (GLB_REQ bit 8 toggled against GLB_ACK, doorbell 0 at G#1F08_0000).
5. Global init, one command-stream group and queue, then a buffer fill made of command-stream instructions alone (MOV48, MOV32, STORE_MULTIPLE, SYNC) — no shader.

About 1.2–1.5k lines; a real compute dispatch ~2k. The project's order puts the GPU after toka (KERNEL.md, DISPLAY-DESIGN.md); this is the plan for when it comes.

## The kernel's side (2026-11-04)

The GPU driver is a process (`payloads/gpu`). The kernel holds only what a process must not:
- **The fence.** G3D's S2MPU (G#1EE7_0000, in pd_g3d) is fenced on a cold boot to `gpu-mem` and nothing else (`s2mpu::fence_gpu`; every VID for now, since only the GPU sits behind it). The GPU is the one bus master a process programs directly (its page tables are the driver's), so the fence is what keeps a GPU job inside the memory ferros gave it. No process may write any S2MPU (`s2mpu::is_fence`).
- **Power.** `pd::gate_at_boot` no longer gates the GPU: ferros had an off sequence but no on (the PLLs come back through ACPM's DVFS), so the GPU stays as ABL leaves it, powered and clocked (about 50 mW, the rail measured 66 against 15 mW gated). A `gpu` power switch with both directions is the next step, so the driver can gate it when idle.

The grants:

| Slot | Grant | What |
|---|---|---|
| 1 | `gpu-regs` (DMA) | the 16 MiB register space at G#1F00_0000, doorbells inside, EL0 Device (cachemap `GPU_REGS`, mapped as the USB controller's block is) |
| 2 | `gpu-mem` (DMA) | 32 MiB at G#B400_0000, Normal non-cacheable, EL0 (cachemap `GPU_MEM`): the top of what was the big arc pool, just below the secure DRAM carveout at G#B600_0000. The first try at G#B800_0000 sat inside that carveout, which no reserved-memory node shows: writes vanished and reads aborted. |
| 3 | the vault scope `gpu.` | `gpu.fw`: `mali_csffw-r52p0.bin`, taken from the device's own vendor image (`/vendor/firmware`, 294912 bytes, sha-256 12ca2252…3ef0; Harbor `/mnt/Harbor/tmp/husky-gpu/`). The vendor image also carries r53p0, r54p0, r54p1 and r54p2. |

First run from EL0 (2026-11-04): GPU_ID G#B8A2_1020, L2_FEATURES G#0814_0306, SHADER_PRESENT G#0111_0055, AS_PRESENT G#FF, MCU_STATUS 0, the firmware's magic G#C3F1_3A6E read from the vault.

## Online (2026-11-04)

`payloads/gpu` boots the CSF microcontroller and runs a command-stream job, from a cold boot and again on top of a running MCU. The sequence is kbase r52p0's (Google's tree), as an agent condensed it from the source with file and line for every value. Measured on the device:

| Step | Result |
|---|---|
| Soft reset (GPU_COMMAND G#101, RESET_COMPLETED polled) | 10 us |
| Quirk registers read back, L2_MMU_CONFIG limits cleared, COHERENCY_ENABLE 31, Google's slice hash (ASN_HASH G#0036_DB6D, G#005B_6DB6, 0; L2_CONFIG bit 24), L2_PWRON | L2 ready, L2_CONFIG G#0114_0000 |
| The firmware's 7 non-protected sections into AS0 | 1.5 MiB of gpu-mem in all; the protected section skipped as kbase skips it |
| MCU_CONTROL 2 (AUTO), JOB_IRQ bit 31 polled | booted in 115 us; GLB_VERSION G#0208_0000, 8 groups of 8 streams |
| Global init (cores G#0111_0055, power-off 800 us, progress and eviction timers, ack mask G#4C81_0F1E) | acknowledged |
| Ping (GLB_REQ bit 8, doorbell 0) | 15 us |
| Group 0, stream 0, address space 1, user doorbell 1 | started; suspend buffer 26368 bytes |
| The job: `MOVE r0:r1 = dst`, `MOVE32 r2 = G#CAFE_F00D`, `SYNC_SET32 [r0:r1] = r2` | the word reads G#CAFE_F00D 139-143 us after the doorbell |

The fence (G3D's S2MPU) saw no access outside `gpu-mem`.

**Memory map.**
- AS0 (the MCU): the firmware's sections at their declared VAs, a group's suspend buffer at G#0410_0000 (64 pages), one queue's user-IO pages at G#0420_0000. Everything is mapped before the MCU starts, so no live update is needed.
- AS1 (the group): the ring at G#1000_0000, the target at G#2000_0000, both non-cacheable (MEMATTR index 5), so the CPU and the GPU agree with no cache maintenance.

**What's certain about instructions.** kbase holds no command-stream assembler. Its sync decoder confirms MOVE (G#01), MOVE32 (G#02), CALL (G#20), SYNC_ADD/SET/WAIT32 (G#25-G#27) and their 64-bit forms (G#33-G#35): the opcode is in bits 63:56, the address register pair in 47:40, the data register in 39:32. Everything else (STORE_MULTIPLE, WAIT, RUN_COMPUTE, FLUSH_CACHE2, the scoreboard entries) comes from Mesa's panfrost `genxml`, which is not on this machine, and needs checking against it before use.

**Gaps:**
- the firmware's trace buffers and its page-fault counter pointer are left at zero (kbase fills them);
- CS_EXTRACT reads 0 after the job even though the word landed (the firmware updates it lazily, or only at idle: not looked at yet);
- the driver polls instead of waiting on its interrupt lines (registered: `gpu-gpu`, `gpu-job`, `gpu-mmu`);
- no idle power gating (the switch would be pd_embedded_g3d alone, whose on/off is short, flexpmu_cal_local_zuma.h `embedded_g3d_on`/`_off`; pd_g3d's own on needs its whole clock controller restored).

**Next:**
1. The driver as a resident service: a `gpu` endpoint taking command streams from processes, each process in its own address space and group, waiting on `gpu-job`.
2. Compute: a shader (Valhall ISA) and RUN_COMPUTE, which needs Mesa's encodings or a capture of a real job. The attest mark (tapuwae) is the first customer: a pixel's 4 × 4 grid is one 16-lane warp.

## Compute (2026-11-04, the same night)

The shader cores run a hand-encoded Valhall compute shader. Sixteen invocations in one workgroup each store G#C0FF_EE00 + their global id to `out[id]`. The output reads G#C0FFEE00 to G#C0FFEE0F, and the done flag is set, 633 us after the doorbell (the MCU powering the cores included). It worked first try, with no fault, and the fence saw nothing outside `gpu-mem`.

| Piece | Words |
|---|---|
| The shader (128-byte aligned, padded with zeros) | `IADD_IMM.i32 r0, r60, #G#C0FFEE00` = G#0110_C0C0_FFEE_003C; `SHADDX.u64 [r2:r3], u0, [r60:r61].w0, shift:2` = G#01A3_C200_2027_3C80; `STORE.i32.slot0.end @r0, [r2:r3]` = G#7861_4002_1800_0002 |
| Shader program descriptor (32 bytes) | G#8000_0018 (type 8 shader, stage 1 compute, 32 registers a thread), G#0000_1000 (preload r60, the global id X), the binary's address, 0 × 4 |
| Local storage (64 bytes) | 0, G#1F (no workgroup memory: G#8000_0000 packed as log2), 0 × 6 |
| Uniforms | one 64-bit entry: the output's address; the FAU register = address \| count << 56 |
| Command stream | REQ_RESOURCE compute; scoreboard entries set (endpoint 3); SRT 0, FAU, SPD, TSD, workgroup 16x1x1 with merging (r33 G#8000_000F), job offsets 0, sizes 1; RUN_COMPUTE (G#0400_0000_0000_8001); WAIT entry 3; FLUSH_CACHE2 (clean L2 and LSC, invalidate others, flush id register 0) and its WAIT; SYNC_SET32 the done flag |

**Sources.** All the words come from Mesa main's v11 genxml, the Valhall `ISA.xml` and its packer, cross-checked against Mesa's assembler test vectors, with file and line for each. They are written up on Harbor: `/mnt/Harbor/tmp/mesa-panfrost/ferros-compute-spec.md`, beside the CSF spec (`ferros-csf-spec.md`) and the Mesa files themselves.

**What this opens.** tapuwae on the GPU (the owner's 2026-10-29 note: "convert to GPU when we get that shit hot"): a pixel's 4 × 4 grid is one 16-lane warp. It needs:
- a compiler from tapuwae's shading (or a small kernel language) to Valhall, or Mesa's compiler run on the host to produce binaries checked in as data;
- resource tables for buffers;
- the driver as a resident service (a `gpu` endpoint, each client its own address space and group).

## A real kernel: the Mandelbrot set (2026-11-04, the same night)

A hand-encoded 26-instruction Valhall kernel computes the Mandelbrot set's escape count over a 64 × 64 image, one invocation per pixel. It runs integer shifts, int-to-float conversion, FMA, float and integer compares, and a divergent loop closed by `BRANCHZ.reconverge`. On the device: 708 us for 4096 pixels at up to 64 iterations; sum 83000 and 1038 pixels at 64, both as predicted. It is **bit-exact against the same arithmetic on the CPU in single precision: 0 of 4096 pixels differ.**

- Uniforms: u0 the output; u1 (log2 width, max_iter); u2 (x0, y0); u3 (step, 0).
- An ALU instruction reads at most one 64-bit uniform slot, so step goes into a register first.
- The words, decoded field by field with their sources and expected pixel values, are in `/mnt/Harbor/tmp/mesa-panfrost/ferros-mandel-spec.md`. It also holds a branch-free fallback (8 unrolled iterations), unused.

This is the shape of tapuwae's work (an orbit per sample, divergent escape), so the attest mark on the GPU is now a compiler question, not a hardware one.

**On the glass.** `tools/husky-gpu.sh show` renders the Mandelbrot set at 1024 × 2048, up to 256 iterations, in 8.7 ms on the GPU. The CPU turns the counts into grey on a surface (black in the set, gamma 2), and the display shows it for 60 s, at the app level since 2026-11-12 (the system level became the system's alone, WINDOWS.md "Whose level"; at the system level until then). Seen by the owner on 2026-11-05: "Looks great".

**Leaving.** The driver ends with a soft reset, which leaves MCU_STATUS 0, L2_READY 0 and SHADER_READY 0, so nothing keeps running or drawing power after it until it becomes a resident service.

## The present stage on the GPU? Measured first (2026-11-05)

The question: should fluor's present stage (the display server's per-pixel ODT, dither and 10-bit pack into the four temporal phases) move to the GPU? The owner's answer on exactness: "bit identical-ish is totally fine".

**The CPU today:**
- A full-screen show (op 22) takes 36 ms wall.
- Its present stage is 19 ms of NEON plus 4 ms of copying, in CPU time over 15.7 Mpx (four phases of 4 Mpx), shared by four threads.
- That is about 1.2 ns of CPU per pixel, roughly 6 ms of CPU and 1.5 ms of wall per phase.

**The GPU** (`gpu.bin` input bit 1, bit 2 for GPU-cached buffers). The proxy has the present stage's shape: a 64-bit pixel in, a 32-bit word out, with 0, 24 or 48 FMAs between, over 1M pixels (a quarter phase).

| Mapping (GPU side) | 1M pixels | A whole phase (4 Mpx, scaled) |
|---|---|---|
| input and output non-cacheable | 1.06 ms | about 4.3 ms |
| input cached, output non-cacheable | 0.69 ms | about 2.8 ms |
| both cached (with the L2 clean at the end) | 0.80 ms | about 3.2 ms |

- The fixed cost of a dispatch is 130-145 us warm (about 600 us cold, the MCU powering the cores).
- 48 FMAs cost nothing measurable: the pass is bound by memory (about 17 GB/s), not arithmetic.
- The GPU runs at whatever clock ABL left, unmeasured; DVFS through ACPM could move these numbers.

**What it says:**
- Per phase the GPU takes about the wall time the CPU does, or somewhat longer.
- What it buys is the CPU: the 6 ms of CPU per phase (23 ms per full repaint) leaves the X3 and the workers.
- It costs:
  - the canvas cleaned from the CPU's caches before the GPU reads it (not yet measured);
  - the GPU's fence widened to the display's phases (or a copy);
  - the GPU kept resident instead of booted per job.

**The Valhall wait field, learnt here.** The flow field (bits 62:59) acts after its instruction, before the next one. A `.wait0` on the consumer of a load comes one instruction too late (it read the register's old value). It belongs on the load itself (as Mesa's test vector has it) or on the instruction just before the consumer.

**The GPU's clock.** ABL leaves the GPU at its table's floor: G3D 150 MHz, G3DL2 160 MHz (zuma-gpu.dtsi `gpu_dvfs_table_v1` runs 150 to 890 MHz). Set by `diag dvfs set 8 900000` and then `set 7 890000` (the L2 first, never below the shaders), the bench on GPU-cached input:

| GPU | MIF (memory) | 1M pixels | A phase (scaled) | Warm dispatch |
|---|---|---|---|---|
| 150 MHz | 2028 MHz | 0.69 ms | 2.8 ms | 140 us |
| 890 MHz | 2028 MHz | 0.55 ms | 2.2 ms | 50-65 us |
| 890 MHz | 3172 MHz (the top row's MIF floor) | 0.38-0.44 ms | about 1.6 ms | 48 us |

- Memory, not the GPU's clock, sets the pace.
- The Mandelbrot kernel (compute-bound) went 630 to 470-500 us at 890 MHz.
- The clocks were put back to the floor afterwards; the kernel's clock policy does not touch the GPU domains, and holds MIF at 1014 MHz at rest and 2028 MHz active.

**Cleaning the canvas.** `diag cvac canvas` (development builds) times the kernel's `dc cvac` over the display's whole 32 MiB canvas. EL0 cannot clean caches (SCTLR_EL2.UCI clear), so handing the canvas to the GPU needs the kernel.
- 0.66 ms settled.
- 2.4 ms right after a full redraw, the same with or without dirty lines, so that is contention with the display's own work, not write-back.
- Only the dirty rectangle needs it.

**So, per phase frame:**
- **GPU:** about 1.6-2.8 ms of wall time and about none of the CPU, plus the clean of what changed and one doorbell.
- **CPU today:** about 1.5 ms of wall time and about 6 ms of CPU across four cores.

## Coherency: the GPU snoops the CPU's caches (2026-11-05)

COHERENCY_FEATURES reads G#1 (ACE-Lite supported), and Google's device tree enables none. The test (`gpu.bin` input bit 3; bit 4 sets COHERENCY_ENABLE to ACE-Lite) runs over a page of `gpu-mem`'s last 2 MiB, which the kernel maps write-back on the CPU's side (cachemap `GPU_SNOOP`). The GPU sees that page twice: non-cacheable (MEMATTR index 5), and coherent (index 6, the shared memory type, shareable both ways: kbase's mapping for coherent memory). The CPU writes and reads through its caches; EL0 cannot clean them.

| COHERENCY_ENABLE | GPU's view | CPU wrote, GPU read | GPU wrote, CPU read from its cache |
|---|---|---|---|
| 31 (none, as Linux) | non-cacheable | 0 of 16 new (stale DRAM) | 0 of 16 (stale cache) |
| 31 (none) | coherent | **16 of 16** | **16 of 16** |
| 0 (ACE-Lite) | either | 0 of 16 | 0 of 16 (the jobs misbehave) |

Repeated three times, the same every time.

**What it means.** The GPU can read what the CPU just drew, and write what the CPU then reads, with no cache maintenance and no copy, as long as the pages are mapped coherent in the GPU's tables. COHERENCY_ENABLE stays at none: setting ACE-Lite broke every access. For the present stage, the canvas needs neither the 0.7-2.4 ms clean nor a copy; what it needs is the GPU's fence widened to the canvas (and to the phases, for its output).

**Not measured yet:** what snooping costs in bandwidth against non-cacheable access.

## The present stage on the GPU: bit-exact (2026-11-05)

The 92-instruction kernel (hand-encoded from the display's `neon.rs`; the words and their sources are in `ferros-present-spec.md` on Harbor) produces the display's present stage on the GPU. The input is 64 full panel rows (86,016 pixels) of black, white, greys and random channels up to 4× white. The tables are built from the display's real `Odt` (`odt.rs`, shared with the display). In all four temporal phases, **every word is identical to `neon::sixteen_canvas`'s on the device: largest channel difference 0.** Six of its instructions had no Mesa test vector; all were right.

At the GPU's floor clock (150 MHz) a 64-row phase took 845-885 us, about 8 ns a pixel. That's too slow for 2992 rows, and to be measured again at the GPU's own speed and over more rows before the display uses it.

**Speed, over 512 rows** (688,128 pixels; every word still exact in all four phases):

| GPU clock | 512 rows | per pixel | a full phase (2992 rows, scaled) |
|---|---|---|---|
| 150 MHz (ABL's floor) | 2.9 ms | 4.2 ns | about 17 ms |
| 890 MHz | 0.93 ms | 1.35 ns | about 5.4 ms |

The CPU does a full phase in about 1.5 ms of wall time (about 6 ms of CPU across four cores).

So at its top clock the GPU is about 3.5 times slower in wall time; what it buys is the CPU's 6 ms a phase. This kernel is compute-bound: 92 instructions at about a third of the cores' FMA peak, where the proxy with the same memory traffic ran in 0.55 ms per 1M pixels.

The cheapest speed-up the GPU allows and NEON does not: the light (the tanh rational with its divide, about 40 of the 92 instructions per pixel) is a function of one 16-bit value, so it can be a 64 Ki-entry f16 table read like the code table already is. One cached load per channel instead of the arithmetic.

## Layers: the shared arena (2026-11-05)

The owner's direction: the GPU produces compute-dense layers (the Mandelbrot and tapuwae, glows, edge effects). The CPU composes them front to back with `under` and presents them on NEON. Buffers are shared in RAM, never copied. The present stage stays on the CPU, where it measured faster. Since 2026-11-12 the present is the dither and the pack alone (COMPOSITOR.md "Baked"), a strict subset of the kernel above, kept on NEON; the GPU touching it last is the documented option there.

**The arena** (cachemap `LAYERS`): 64 MiB at G#9C00_0000.
- **On the CPU's side:** Normal write-back, EL0 read/write, never executable.
- **The grant:** `layers` (the GPU driver and the display server; one shared arena for now, with no isolation between its users).
- **In the GPU's fence:** beside `gpu-mem`, both in one GiB.
- **Zeroed before every reset.**
- **Who uses which part** (no isolation yet, GPU-KISTS.md plan step 1): the first 32 MiB wallgen's (the person's wallpaper on the GPU since 2026-11-11: the frame, the refine's list, the kernel's words) and gputest's bench, never at once; from 32 MiB ahau's (the mark's words, outputs, stage and sums).
- **Where:** the device tree reserves nothing in G#9800_0000-G#A000_0000 (USB's 4 MiB end at G#9740_0000). It reads and writes as ordinary non-secure DRAM.

The GPU driver maps it coherent (MEMATTR index 6, shareable both ways) at GPU VA G#8000_0000. With input bit 6 and `--dma layers` as grant slot 5, the test runs both ways:
- The CPU wrote 16 words through its caches 60 MiB in, and the GPU read all 16.
- The GPU rendered the 64 × 64 Mandelbrot at the arena's start while the CPU held those 4096 words stale in its caches. The CPU then read the exact sum, 83000, from its caches.
- No clean, no copy, no fence faults.

**Next:**
- the GPU driver as a resident service, taking jobs that name layers in the arena;
- a typed Valhall instruction builder in Rust, generated from Mesa's `ISA.xml` and checked against tonight's verified words, which leads to kernels as Rust functions compiled on the device;
- the attest screen's fresh mark as the first customer.

## The resident service, and the attest mark's path (2026-11-05)

**The service.**
- `payloads/gpu` starts at every boot from init. It doesn't start after a jump, because Android ran the GPU and whether the hv cuts it off at the jump is the owner's open decision.
- It boots the GPU once and serves `gpu`. A run names:
  - code at an offset in the layer arena (copied into the service's 32-page executable region);
  - a grid and a preload word;
  - up to 16 uniforms.
- The reply is the status and the job's µs.
- Grants: slots 104-108, with the layer arena at 68 and `use gpu` at 24. The grants page is full at 108 slots (process.rs PREFS).
- `payloads/gputest` is its first client. It builds the Mandelbrot through the valhall builder, runs it through the service, and checks every pixel against the CPU: 0 of 4096 differ, 1.0 ms at the 150 MHz floor.
- **Whose job failed** (2026-11-11): a run may end with `[n][the client's name][n][the kernel's name]` (ahau "quick" and "fine", wallgen "survey", "first pass", "refine"). A failure says the client, the kernel, its arena offset, its words as the GPU fetched them (first two, last), the grid, CS_FAULT by kbase's name, CS_FAULT_INFO and the stream's command pointer. Code whose first word is zero is refused with status 3 before the queue sees it. A CS fault is acknowledged as kbase does (CS_REQ's FAULT bit to CS_ACK's, the stream's CSG doorbell bit, the group's GLB doorbell bit, doorbell 0), so the job fails at once instead of halting the queue for every later job; whether the stream runs on cleanly after the acknowledgement is still to see on the device.
- **A masked client reaches the arena only through its handle** (2026-11-11, the Pro: CS_FAULT G#C51, INSTR_INVALID_ENC, then every job 1 s with 0 interrupts). The masked ahau wrote its kernel's words, its stage and both outputs by pointer, and the mask put them 96 MiB and more into its own 128 MiB arc (over its own statics; the first ARCS boot's fault at arc offset G#760_0000 was GPU_OUT2 there). The GPU service ran whatever the real arena held at 32 MiB, zeros after a scrubbed reset, and the screen showed ahau's own arc as the GPU's output. ahau now writes and reads the arena through `Handle<12>` (the layer arena's slot); the rebuilt kernels' words are identical under valhall before and after 3e9cb0a8 (all 153 of three handles at r = 96, 362, 448, quick and 16 fine passes).
- **The GPU's clock:** ABL leaves it at its floor. The kernel's service loop raises it (G3D 890 MHz, G3DL2 900 MHz, DVFS domains 7 and 8, the L2 first) while a `gpu` process lives, and drops it back to the floor (150 / 160 MHz) otherwise.

**The attest mark: one source (owner, 2026-11-05, "needs done").** The mark is tapuwae's attested Julia ball:
- up to 1024 iterations a sample with eight running accumulators;
- about 215 lines of shading a sample (ripples, two lamps, styles, engraving, arcs);
- all f32, with tapuwae's own exp2, log2, sin and atan2;
- one sample a pixel, then 16 per pixel in the fine pass (one 16-lane warp a pixel).

A hand-encoded copy is ruled out: it would be a second implementation of tapuwae's shading. The path:
1. **A recording lane type.** tapuwae's `orbit` and `shade` are generic over `Lane` / `Mask` / `Bits` (tapuwae lanes.rs). A GPU lane type records each operation into an SSA graph instead of computing it.
   - Its `any()` answers yes: the blocks it guards are select-guarded inside, so running them always gives the same values.
   - The loop's early exit is the one exception.
2. **tapuwae's loop as a hook:** `for n in 0..MAX_ITER { …; if !running.any() { break } }` becomes a loop the lane type provides. It is a plain loop for F4 on NEON, and one recorded body with a real exit for the GPU (not 1024 unrolled copies). This is the one change in tapuwae.
3. **The SSA layer in valhall:**
   - instruction selection to the builder;
   - register allocation (linear scan, values live across the loop pinned);
   - the flow rules (waits on the instruction before a consumer, one 64-bit uniform slot an instruction).
4. **The builder's missing instructions,** each encoded from Mesa's ISA.xml and checked on the device against the CPU, as the Mandelbrot was:
   - select (MUX), min and max;
   - reciprocal and reciprocal square root;
   - float to int with rounding modes, and floor;
   - the logic ops.
   - `sqrt_rounded` must round correctly (one ulp moves the escape): an approximate reciprocal square root, then Newton steps, then a correction.
5. **The ball as a kernel,** then attest drawing its mark through the service into the arena.

### The mark on the GPU, as built and measured (2026-11-05)

Everything above is built. tapuwae's own `orbit` and `shade` record through valhall's SSA (`tapuwae::gpu`) and run through the service. The owner's rule: the GPU mark has to look the same as the CPU's, not match it bit for bit.

**What the kernels are.**
- **The stage:** the index of every pixel inside the circle (`button_stage`, about 78 % of the square), made once per size. Each pass runs one invocation per staged pixel.
- **The stage is padded to whole workgroups of 16,** using pixel 0, a corner outside the circle that comes out as the word 0. The GPU runs every invocation of the last group, and before the padding those past the list read whatever lay beyond it as a pixel index. After another job had left words there, the r = 448 pass stored outside the arena and never finished: a 1 s service timeout, then a wedged queue until `diag "restart gpu"`.
- **Quick pass:** one sample per pixel. **Fine pass:** 16 distinct passes, one stratified jittered sample each, with per-pixel sums in the arena. Each pass's frame is shown as it lands.
- **ahau's two outputs.** ahau keeps two outputs in its region of the arena (33 and 54 MiB). Each pass writes the one the screen isn't showing, and the screen reads straight from it: no copy, no half-written frame.

**What made it fast** (staged quick pass at r = 448, gputest 08):

| Step | Time |
|---|---|
| Staged, exact | 11.1 ms |
| Negations folded into operand modifiers | 8.5 ms |
| Unrefined FRCP/FRSQ, FMA contraction (`ssa::exact` keeps the refined forms) | 5.6 ms |
| Hardware log2 and exp2 (Mesa's sequences; FREXPE's *log* form, the sqrt form is wrong here) | 4.2 ms |
| A stopped invocation leaves after the loop's head (`LoopBreak`; tapuwae's GPU lane at one lane's meaning) | 3.6 ms |

- The orbit's loop went from 207 to 89 instructions.
- Accuracy against the CPU's pixels at r = 96: 0 further than 64/16384 per channel in the quick pass; 1 in the fine pass (on the set's edge, where one rounding moves the escape).
- gputest 10 and 12 measure the special function unit:
  - FRCP and FRSQ are within 1 ulp, and one correction on the raw FRCP gives the correctly rounded quotient for 2^19 random pairs;
  - log2 and exp2 are within 1 ulp over 2^16 arguments.

**In ahau, this device's mark at 900 px:**

| Part | Before | Now |
|---|---|---|
| GPU job | 21.7 ms | 5.9 ms |
| 16 fine passes | 404 ms | 116 ms |

A keystroke now costs:
- the handle's parameters: 1.55 ms (CPU, f64, on the worker cores; 2.8 ms before J(c)'s address points were split across ahau's three helpers and the mark's thread);
- the compile: 1.2 ms;
- the job: 5.9 ms.

The first draw at a size adds the stage, 4.0 ms. Each part is on ahau's console line.

**Still open, toward one 120 Hz frame:**
- **The parameters.** `j_extent`'s points are now split four ways. What remains is serial: the descent (`classify`) and the sprayed cycles (`cycle_point`, also independent, also splittable). Fewer points or steps would move every mark's framing (the owner's call).
- **The compile.** Handle-dependent constants are folded at record time, so each keystroke compiles again.
- **The shading's sin, cos and atan** are still polynomials. Valhall has FSIN_TABLE, FCOS_TABLE, FSINCOS_OFFSET and FATAN_ASSIST/TABLE.

## Clients kept apart (2026-11-12, GPU-KISTS.md plan step 1)

Built and run on the 8a (GPU-KISTS.md step 1 has the whole of it and the test: every store outside a client's own memory faulted in its own address space, the other client's words untouched).
- **The kernel:** the arena as blocks a client owns (`layers` the largest free, `layers-32` 32 MiB; `arena` the whole, the service's), wiped as given; every call on `gpu` stamped by the kernel with the caller and the GPU memory it holds; a client that ends with a job in flight has its memory held back until the service takes its next request; every kist handed `use gpu`, a block and a second surface after its wants on a development build, empty slots on production. akita's arena is its upper 64 MiB.
- **The service:** a CSF group and an address space a client (groups 0-6, AS1-AS7, doorbells 1-7), each space mapping only the client's block at G#8000_0000, its surfaces from G#C000_0000, and its own ring, descriptors and code read-only and done flag; a client's code read from its own block (the request's offset is into the block). The MMU's line (`gpu-mmu`) is its slot 8, on a thread of its own: a fault lets go of that space and ends that job (status 4, with the address). Only MMU_IRQ's page-fault bits (0-15) are faults on this CSF GPU: bit 17 rose with no fault after a LOCK and FLUSH_PT on akita. Each job ends at its guard, 2^10 ms a system client's and 2^8 ms a kist's, with its group.
- **Grants now:** 1 `gpu-regs`, 2 `gpu-mem`, 3 `gpu.`, 4 `gpu-job`, 5 `arena`, 6 serve `gpu`, 7 `surfaces`, 8 `gpu-mmu`.
- **Clients:** ahau and wallgen each own a 32 MiB block, their offsets into it (ahau's rebased from 32 MiB to 0, so the masked ahau is sealed again on each device); gputest asks `--dma layers-32`; a development spawn of Mahere asks `--dma layers`.

## Kists' frames on the GPU (2026-11-11)

Mahere's map draws on the GPU (GPU-KISTS.md plan step 4, its first cut), and the service, the kernel and the display grew for it:
- **The fence admits the apps' surfaces.** `s2mpu::fence_gpu` fences two GiBs now (`fence_gbs`): gpu-mem and the layer arena as before, and the apps' surfaces (cachemap SURFACES) in the GiB above. The GPU service gets them as its slot 7 and maps all four coherent at GPU VA G#C000_0000, so a client's kernel writes its own surface, the buffer the display composes, with no copy. Every GPU client can write every app's surface, as every one already shares the arena: the isolation is plan step 1's. Since 2026-11-12 a surface is 16 frames of the pool and the fence admits exactly the frames of the surfaces held, each as it is given and given back (WINDOWS.md "Surfaces on demand").
- **The layer arena is 128 MiB**, G#9800_0000-G#A000_0000 (memblock: plain memory; the device tree reserves nothing there): Mahere keeps its cells' planes there, a MiB each. The service maps it by its size, which `SYS_DMA` answers with x1 = 1 (ferros_sys `dma_size`); a kernel from before answers the base, and the service then maps 64 MiB. Not checked on akita's memory map.
- **No fixed arena since 2026-11-13.** A client's GPU memory is a block of frames of the pool, taken as it asks (up to 512 MiB) and admitted to the fence frame by frame, mapped for it alone in 2 MiB level-2 entries from `ARENA_VA`; the old arena is the pool's `POOL_LAYERS` (GPU-KISTS.md plan step 4, seventh pass).
- **64 uniform slots a run** (16 before; `valhall::ssa::uniform_slots`): a kernel recorded with a frame's constants reads them from uniforms with no move before each use.
- **A served job is said only when it fails**: a client drawing every frame made a line a job.
- **The display's app show** (op 22): the publish split across the publish workers, the hit map's rows written only when the windows' layout changed, the next phase published before waiting for the armed frame's start; each ten seconds' line says the shows and their publish and wait.

Measured on the Pro: Mahere's kernel 6.6 to 7.7 ms a whole-screen frame at 890 MHz; the display's show of it about 20 ms; about 49 frames a second on the glass at a town's scale (GPU-KISTS.md step 4 has the parts and what 120 Hz still needs).

