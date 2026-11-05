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
- A syscall buffer is an arc offset. A pointer into a DMA region (gpu-mem) passed as a buffer gets masked into the arc and overwrites the process's own code, so read into an arc buffer and copy.

**How to apply:** only the CS opcodes kbase decodes are confirmed (MOVE, MOVE32, CALL, SYNC_*). Check STORE_MULTIPLE, WAIT and RUN_COMPUTE against Mesa genxml before use.

**Next:** a resident `gpu` endpoint driven by its IRQs, then compute shaders for tapuwae ([[tapuwae-neon-then-gpu]]).

**Compute works** (same night): a 3-instruction Valhall shader runs 16 invocations (G#C0FFEE00 + id). The words with their sources are at Harbor /mnt/Harbor/tmp/mesa-panfrost/ferros-compute-spec.md. Mesa's genxml (v10/v11/v12), cs_builder.h and ISA.xml are fetched there too.

**More traps:**
- Pad every submission to 64 bytes: the CS front end caches the ring's 64-byte line, so a job that starts mid-line runs as stale NOPs.
- A 32-bit sync object is 8 bytes: SYNC_SET32 zeroes the next word.
