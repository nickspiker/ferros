---
name: ferros_process_model_husky
description: What runs on husky since 2026-10-14 night — VHE + static cache map, EL0 processes in arcs, capability tokens, call/recv/reply IPC, signed capsules through the oracle gate, time slices, "Ready?" gate, boot spawns the vault's init capsule
metadata:
  type: project
---

Built and hardware-verified on the cold boot (slot b), all in ferros_kernel/src, documented in KERNEL.md "Built on husky" + BOOT.md:
- **cachemap.rs**: one static identity table at boot (DRAM window cached, `.bss.dma` uncached, everything else Device). VHE (HCR E2H+TGE) so EL0 runs under it; static EL0/EL2 split, arc pool = top 2 MiB of the window. Gotchas: E2H makes CPTR_EL2 CPACR-form (set FPEN or NEON memcpy traps); EL2 can never execute EL0-writable pages, so the RUN staging slot stays outside the pool.
- **process.rs**: arcs (buddy, zeroed), EL0 entry with x28 = arc base / x27 = mask, syscalls exit/call/recv/reply (x8), BLAKE3-keyed capability tokens per epoch, single-use reply tokens, faults end the process, 10 ms EL2-timer slices (PPI 26) resumed from the service loop.
- **oracle.rs**: signed VSF capsules (vsf crypto-verify, no_std) → ihi = spaghettify(system secret ‖ dev key ‖ hp), written at arc G#FE0. System secret is an ira-derived RAM stand-in.
- **ready.rs**: "Ready?" gate (power key gpa4-0 falling-edge EINT, INTID 60); dev auto-press after 1 s unless a volume key cancels it (then waits for power), FERROS_PRODUCTION always waits; KOHI during the wait.
- Boot spawns vault object `init` (signed hello_el0) as the first process.
- Tools: `bridge capsule`, `bridge spawn [--serve E --use E]`, `bridge diag console|bench|devinfo|ready`.

- **First driver process (2026-10-15):** payloads/pcie1 (hardware grants via mmio syscalls) brought PCIe ch1 to L0 Gen1 ×1, read the BCM4398 (chip 4398 rev 3 = d0), downloaded and booted its firmware (RTE 24.100.671.82.2, idle after boot-complete MSI) — WIFI.md has the full recipe and the traps (never read backplane 0 after release).
- **Kernel additions that day:** syscall 6 entropy (capability, wairua-keyed), syscall 7 dma (2 MiB NC DMA_POOL at G#8180_0000, `--dma pcie1-dma`), and the HSI2 S2MPU fence (s2mpu.rs: all VIDs limited to the cold window; replaced cold_ufs's "protection off"). PCIe's VID is still unmeasured.

Open: toka VM as a process — needs a no_std core split of the owner's toka crate (review first); toka also doesn't build since VSF 0.10 (pins vsf ^0.9.3). See [[husky_cold_slot_b]], [[ferros_wake_policy_prox_gate]].
