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

Indirect branches (`br`, `blr`, `ret`) are allowed: the invariant does not depend on control flow — **provided execution can only ever reach checked code.** That is the W^X rule below, and it is part of the invariant from day 0, not an add-on.

## W^X (day 0)

The scan only speaks for code it saw, so the process may execute nothing else. At spawn the kernel sets the arc's pages in the one static translation table (no per-process tables): the sealed code pages **EL0 read + execute, not writable**; every other page of the arc (read-only data, data, stack) **EL0 read/write, never executable**; guards and everything outside the arc not accessible at EL0. The pool is mapped with 4 KiB entries for this, and the kernel invalidates the TLB after changing them. Code and read-only data stay in the same arc, so PC-relative addressing (`adrp`) keeps working, and loads of read-only data go through the masked forms like any other access; a store into the code pages is a permission fault that ends the process. (Today's static map gives EL0 the whole pool as read/write/execute — the reason nothing can be called sandboxed until this lands.)

## Placement and guards

Immediate offsets reach up to G = 64 KiB past a masked address (every AArch64 scaled 12-bit immediate, up to 65520 for 16-byte accesses; unscaled ones reach 256 below). So every arc needs at least G of memory nobody else owns on both sides.

- An arc of size S (a power of two, at least 64 KiB) is placed at an **S-aligned base inside a 2S block whose upper half is left unused**. The half above is its guard; the half below belongs to the previous block's guard (the pool starts with an unused G). So each arc has at least S ≥ G of guard on both sides.
- Because the base is S-aligned, masking an absolute pointer gives its offset directly: `and x26, xN, x27` — an address inside the arc maps to itself, one outside wraps inside (still safe). One instruction.
- Cost: an arc takes twice its size in the pool (the pool grows accordingly; DRAM is not the constraint).

## What the emitter rewrites (measured on today's payloads: ~15% of instructions)

x24 is the emitter's scratch register (reserved in codegen alongside x25–x28).

| rustc emits | becomes |
|---|---|
| `ldr x0, [x1, #8]` (any base other than sp) | `and x26, x1, x27` · `add x25, x28, x26` · `ldr x0, [x25, #8]` |
| `ldr x0, [x1]` | `and x26, x1, x27` · `ldr x0, [x28, x26]` |
| `ldr x0, [x1, x2, lsl #3]` | `add x24, x1, x2, lsl #3` · `and x26, x24, x27` · `ldr x0, [x28, x26]` |
| `ldr x0, [x1, #8]!` / `[x1], #8` | the base update as an `add`, then the masked access |
| `sub sp, sp, #n` (any sp write) | `sub x24, sp, #n` · `and x26, x24, x27` · `add sp, x28, x26, uxtx` |
| `stp x29, x30, [sp, #-16]!` | the sp adjustment above · `stp x29, x30, [sp]` |
| `ldp x29, x30, [sp], #16` | `ldp x29, x30, [sp]` · the sp adjustment |

Consecutive accesses through the same unchanged base can share one masked x25 (a later peephole).

## The stack (option a)

The stack area is the top of the arc. Every stack-pointer adjustment is compiled as: compute the new value in a scratch register, subtract x28, `and x26, scratch, x27`, `add sp, x28, x26`. Accesses then use `[sp, #imm]` with no masking of their own. Prologue/epilogue pushes with writeback are rewritten into an explicit adjustment plus plain stores.

## Components

- **Checker** (`ferros_sandbox`, no_std, in the kernel's install path): decodes each A64 instruction and enforces the rules above. Tested exhaustively against instruction tables and cross-checked with `objdump`.
- **Emitter** (host tool, Rust): rewrites rustc's assembly output (`--emit asm`) into the allowed forms before assembling — rewriting before assembly keeps every label and branch correct. When toka exists, its on-device backend emits the same forms directly, and the checker stays as an assertion over its output at install time.
- **Kernel**: install (check + seal + store), spawn only sealed images, arcs placed with guards, W^X page attributes per arc at spawn.

## Status

- **Checker** (`ferros_sandbox`): written, tested against GNU-assembled encodings.
- **Emitter** (`tools/ferros-mask` + `tools/ferros-masked-build.sh`): every crate (payload, core, alloc, compiler_builtins) built from source to assembly with nightly `-Z build-std`, rewritten, assembled with `llvm-mc`, linked with `ld.lld`; the build fails unless the checker accepts every instruction. 2026-10-15: `ip` (5,078 instructions, +46% code) and `nettest` (742, +10%) pass with zero refusals and **run correctly on husky** — DHCP + SLAAC, ping and UDP echo over IPv4 and IPv6 (`MASKED=1 tools/husky-net.sh`).
- **Checker in the gate (2026-10-15):** a capsule with a `masked` section (code + `text` = how many bytes are instructions) must pass `ferros_sandbox` over exactly that span or the kernel refuses to spawn it; a plain `aarch64` section is native dev code, spawned unchecked (a production build refuses it). Verified on husky: the masked `ip` passes and runs; the native `ip` labelled masked is refused ("failed the sandbox checker"); the full stack (`MASKED=1 tools/husky-net.sh`) runs through the gate — DHCP, SLAAC, ping and UDP echo over IPv4 and IPv6. The declared text span is only safe together with W^X (everything after it non-executable), which is next.
- **Seal (2026-10-15):** `bridge seal <masked.bin> --text-len N` (PT capability `ferros.dev.seal`) runs the gate and the checker once and adds the image's seal — BLAKE3 keyed with the OS-only key (derived from the system persistent secret, oracle.rs) over developer key, provenance, masked flag, instruction span and image — to the vault's `sealed` list. Spawning a masked capsule requires its seal in that list and does not re-check. Verified on husky: unsealed spawn refused; seal accepted; a native image labelled masked refused at seal time; sealed spawn runs; the list survives a reboot (sealed `ip` spawns with no re-seal); the full stack (`MASKED=1 tools/husky-net.sh`, which seals first) runs — DHCP, SLAAC, ping and UDP echo over IPv4 and IPv6.
- **Guards + W^X (2026-10-15):** the pool is mapped at 4 KiB (cachemap `POOL_L3`); a fresh/freed page is writable, non-executable data. At spawn a masked process gets its checked `.text` pages read-only + executable and everything else (grants, read-only data, input, stack) non-executable; a native dev process gets its arc RWX. `arc_free` returns pages to data. Verified on husky: the sealed masked stack runs under W^X (DHCP, SLAAC, ping and UDP echo over IPv4 and IPv6); native `init` still spawns.
- **Escape proof (2026-10-15, `payloads/wxtest`):** a masked, sealed payload whose instructions the checker accepts (an in-arc store and an indirect call are legal forms) tries to (0) store over its own code and (1) call into its stack. Both FAULT — a data abort (EC G#24) writing its code page, an instruction abort (EC G#20) at the stack — and the kernel carries on; neither reaches "SURVIVED". The checker bounds what instructions exist; W^X bounds where they may write and what may execute, so the two together hold.
- **Next:** migrate the WiFi driver to streamed firmware + its DMA region inside its arc + masked code (then native is retired on husky). Then the WiFi driver: its DMA region must become part of its arc (non-cacheable pages inside the arc, the S2MPU window over them), since masked accesses only reach the arc.
