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
