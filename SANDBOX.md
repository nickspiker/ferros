# SANDBOX — how ferros keeps a process inside its arc — Zil (0)

Decided with the owner 2026-10-15. Trust model A: ferros compiles everything it runs and trusts nothing precompiled (KERNEL.md). Strict from day 0: no capsule runs unless the device itself compiled it (today: checked it) and sealed the result.

## Lifecycle: compile once, then run and gun

1. **Install** (once per capsule version). The capsule's signature passes the oracle gate (ORACLE.md). The device compiles it — the toka compiler from bytecode once it exists; until then the *checker* over native code produced by the ferros emitter. A capsule that fails is refused, in dev builds too.
2. **Seal.** The installed native image is sealed with a keyed BLAKE3 hash under an OS-only key derived from the system persistent secret (oracle.rs), and stored in the vault. Nothing outside the OS can produce a seal.
3. **Spawn** runs only sealed images. No instruction scan at spawn: one BLAKE3 over the image to check the seal, then `eret`.
4. **Run.** Isolation costs only the masking instructions compiled in — plain `and`/`add`/`sub`, no compares, no branches, nothing for the branch predictor. Zero cost after compilation, the way Rust's checks are.

## The invariant

Registers (reserved in codegen, `.cargo/config.toml`):

| Register | Meaning | May be written by |
|---|---|---|
| x28 | arc base | nobody (set by the kernel at entry) |
| x27 | arc mask (arc size − 1) | nobody |
| x26 | a masked offset, always in `[0, mask]` | only `and x26, <reg>, x27` |
| x25 | an address inside the arc, always `x28 + x26` | only `add x25, x28, x26` |
| sp | the stack pointer, always inside the arc's stack area | only `add sp, x28, x26` (after masking) |

Because every write to x25/x26/sp is a single instruction that re-establishes its range, the invariant holds at **every** instruction boundary — a branch can land anywhere and the next access is still inside the arc. That is what makes a linear scan sufficient.

Memory access forms allowed:
- `[x28, x26]` — register offset, no shift, no extend.
- `[x25, #imm]` and `[sp, #imm]` — immediate offsets within the guard size G, no writeback.
- Pair loads/stores (`ldp`/`stp`) only with x25 or sp as base, immediate within G, no writeback.

Everything else is refused: any other base register, writeback (pre/post-index) on any base, register offsets other than x26, any write to x27/x28, writes to x25/x26/sp outside the forms above, and system instructions — except `svc #0`, reading the EL0 timer (`mrs` of `cntvct_el0` / `cntfrq_el0`) and the barriers `isb` / `dsb` / `dmb` (a driver orders writes to its DMA region). The rules cover every load and store, integer and SIMD/FP alike.

Indirect branches (`br`, `blr`, `ret`) are allowed: the invariant does not depend on control flow. Code is not writable by the process (the arc's code region is EL0 read/execute only once sealed — a later step; until then the scan plus no self-modification in emitted code).

## Guards

Immediate offsets reach up to G bytes past a masked address. Each arc is placed with **G bytes of unused guard memory before and after it** (kernel-reserved, never allocated to anyone), so an access at `x25 + imm` or `sp + imm` lands in the process's own arc or in its own guard — never in another arc or the kernel. G = 64 KiB covers every AArch64 scaled 12-bit immediate (up to 65520 for 16-byte accesses).

Arcs need not be size-aligned: offsets are computed relative to x28 (`sub`, then `and` with x27), so placement is free.

## The stack (option a)

The stack area is the top of the arc. Every stack-pointer adjustment is compiled as: compute the new value in a scratch register, subtract x28, `and x26, scratch, x27`, `add sp, x28, x26`. Accesses then use `[sp, #imm]` with no masking of their own. Prologue/epilogue pushes with writeback are rewritten into an explicit adjustment plus plain stores.

## Components

- **Checker** (`ferros_sandbox`, no_std, in the kernel's install path): decodes each A64 instruction and enforces the rules above. Tested exhaustively against instruction tables and cross-checked with `objdump`.
- **Emitter** (host tool, Rust): rewrites rustc's assembly output (`--emit asm`) into the allowed forms before assembling — rewriting before assembly keeps every label and branch correct. When toka exists, its on-device backend emits the same forms directly, and the checker stays as an assertion over its output at install time.
- **Kernel**: install (check + seal + store), spawn only sealed images, arcs placed with guards.

## Status

Spec only (2026-10-15). Order: checker → emitter → kernel install/seal/spawn → rebuild the drivers through the emitter and re-verify the network stack.
