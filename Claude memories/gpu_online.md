---
name: gpu-online
description: "Mali-G715 CSF online under ferros 2026-11-04 night (payloads/gpu): MCU boots r52p0 firmware, ping, group+queue, a 3-instruction CS job writes G#CAFEF00D; gpu-mem at G#B400_0000; traps (secure carveout at G#B600_0000, arc-offset syscall buffers)"
metadata:
  node_type: memory
  type: project
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-05T05:02:42.617Z
---

GPU online 2026-11-04 night (GPU.md "Online"): `payloads/gpu` (native, `bridge spawn gpu.bin --name gpu --dma gpu-regs --dma gpu-mem --store gpu`) does kbase r52p0's sequence and runs a CS job (MOVE, MOVE32, SYNC_SET32) that writes G#CAFE_F00D in ~140 us; cold boot verified. Spec with file:line for every value: scratchpad gpu-spec.md (session), condensed into GPU.md.

Kernel side: `gpu-regs` (G#1F00_0000 16 MiB, EL0), `gpu-mem` (G#B400_0000 32 MiB NC, top of the old big pool), vault `gpu.fw` (mali_csffw-r52p0.bin from husky vendor.img, Harbor /mnt/Harbor/tmp/husky-gpu), G3D S2MPU fenced to gpu-mem, `pd::gate_at_boot` no longer gates the GPU (~50 mW).

**Traps:**
- The secure DRAM carveout starts at G#B600_0000 and no device-tree node shows it: writes vanish and reads take a synchronous external abort (DFSC G#10).
- (2026-11-04) A pointer into a DMA region passed as a syscall buffer was masked into the arc and overwrote the process's own code. Since 2026-11-08 a buffer in one of the process's own DMA regions is used directly and anything else traps ([[syscall_buffers_resolve_or_trap]]).

**How to apply:** take command-stream and descriptor layouts from Mesa's v11 genxml (on Harbor). Confirmed on the device: MOVE48, MOVE32, SYNC_SET32, STORE_MULTIPLE, WAIT, REQ_RESOURCE, SET_STATE_IMM32, RUN_COMPUTE, FLUSH_CACHE2.

**Next:** a resident `gpu` endpoint (each client its own AS and group), then tapuwae as a shader ([[tapuwae-neon-then-gpu]]).

**Compute works** (same night): a 3-instruction Valhall shader runs 16 and 16384 invocations, and a 26-instruction Mandelbrot kernel (divergent float loop, BRANCHZ.reconverge) is bit-exact against the CPU (ferros-mandel-spec.md on Harbor). The words with their sources are at Harbor /mnt/Harbor/tmp/mesa-panfrost/ferros-compute-spec.md. Mesa's genxml (v10/v11/v12), cs_builder.h and ISA.xml are fetched there too.

**More traps:**
- Pad every submission to 64 bytes: the CS front end caches the ring's 64-byte line, so a job that starts mid-line runs as stale NOPs.
- A 32-bit sync object is 8 bytes: SYNC_SET32 zeroes the next word.

**Coherency (2026-11-05):**
- The GPU snoops the CPU's caches both ways when its page tables map the memory coherent (MEMATTR index 6 shared + SH both) with COHERENCY_ENABLE left at none (31). Setting ACE-Lite (0) broke all access.
- So there's no dc cvac or copy for CPU↔GPU sharing; only the S2MPU fence must admit the region.
- Clocks: ABL leaves the GPU at 150/160 MHz (`diag dvfs set 8 900000` then `set 7 890000` for the top). The present pass is memory-bound (MIF).
- The present-stage kernel (92 instructions) spec is in Harbor /mnt/Harbor/tmp/mesa-panfrost/ferros-present-spec.md (with its model in the session scratchpad).
