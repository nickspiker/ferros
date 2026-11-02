# CPU-FEATURES — what husky's cores actually implement — Zil (0)

Read from the CPU's own ID registers (`bridge diag cpu`, 2026-10-15), not from spec sheets. Owner's direction: cover this hardware 1:1 — crypto, NEON, wide and the rest — before inventing new types (spirix instructions wait for hardware that runs them).

## Measured (boot core: Cortex-A510 r1p1, MIDR G#411FD461)

| Area | Feature (ID field = value) |
|---|---|
| Crypto | AES + PMULL (ISAR0.AES=2), SHA1 (1), SHA-256 + SHA-512 (SHA2=2), SHA3 (1), SM3 (1), SM4 (1) |
| Integer / wide | CRC32 (1), LSE atomics incl. CASP 128-bit pairs (Atomic=2), RDM (1), DotProd (1), I8MM (1), flag manipulation v2 (TS=2) |
| FP / SIMD | FP + AdvSIMD with FP16 (PFR0 FP=1, AdvSIMD=1), FHM (1), BF16 (1), JSCVT (1), FCMA (1), FRINTTS (1) |
| SVE | SVE2 (ZFR0.SVEver=1) with SVE-AES + PMULL128 (2), BitPerm (1), BF16 (1), SHA3 (1), SM4 (1), I8MM (1); no F32MM/F64MM; no SME |
| Memory model | RCPC2 (LRCPC=2), DPB2 (2), SB (1), SSBS (2), DIT (1) |
| Security | PAuth2 with QARMA (APA=5, GPA=1), BTI (1), MTE3 (PFR1.MTE=3), CSV2=2, CSV3=1, SPECRES (1) |
| Absent | **RNDR** (ISAR0.RNDR=0 — no hardware RNG instruction; KOHI already skips it), LS64, SME, F32MM/F64MM, WFxT |
| Caches | 64-byte lines (I and D), DC ZVA block 64 bytes; 40-bit physical addresses (PARange=2) |

The X3 and A715 cores are not started yet; big.LITTLE requires matching ISA features, but they will be read too once secondaries run.

## Where each feature stands

- **Codegen** (`.cargo/config.toml`, aarch64-unknown-none): enabled — `aes, sha2, sha3, sm4, crc, lse, rdm, dotprod, fp16, fhm, i8mm, bf16, rcpc, jsconv, fcma, flagm, sb, ssbs, frintts, dit`. Not yet: `sve2*` (the sandbox checker has no SVE addressing rules yet), `mte` (tag instructions refused), `pauth` (it emits `retaa`/`braa`, refused), `bti` (fine, not needed yet).
- **Sandbox checker** (SANDBOX.md): every SIMD/FP/crypto data-processing instruction writes only vector registers and is accepted; CRC32 and the integer extensions are ordinary register writes; LSE atomics and LDAPR go through the `[x25|sp]` rule. SVE loads/stores (including gathers with vector offsets) need their own masking rules before SVE2 is enabled.
- **toka** (KERNEL.md trust model A): WASM's SIMD128 covers part of NEON only; the first toka extensions are the instructions above that WASM lacks — AES/PMULL, SHA/SM, CRC32, dot product and I8MM, FP16/BF16, 128-bit atomics, then SVE2 — mapped 1:1.
- **Now usable for free**: BLAKE3's NEON path (the kernel builds blake3 `pure` today), hardware SHA-2/SHA-3 for the post-quantum signatures Photon's fleet may require, AES/PMULL where a protocol needs them.
- **Worth a design look later**: MTE3 (hardware memory tags) as a second, independent check under the masking invariant.
