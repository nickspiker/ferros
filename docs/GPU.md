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

**On the glass.** `tools/husky-gpu.sh show` renders the Mandelbrot set at 1024 × 2048, up to 256 iterations, in 8.7 ms on the GPU. The CPU turns the counts into grey on a surface (black in the set, gamma 2), and the display shows it at the system level for 60 s. Seen by the owner on 2026-11-05: "Looks great".

**Leaving.** The driver ends with a soft reset, which leaves MCU_STATUS 0, L2_READY 0 and SHADER_READY 0, so nothing keeps running or drawing power after it until it becomes a resident service.
