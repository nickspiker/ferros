# SANDBOX — how ferros keeps a process inside its arc — Zil (0)

Decided with the owner 2026-10-15. Trust model A: ferros compiles everything it runs and trusts nothing precompiled (KERNEL.md). Strict from day 0: no capsule runs unless the device itself compiled it (today: checked it) and sealed the result.

## Vocabulary (settled with the owner 2026-11-03)

One word per thing, used the same way in the docs, the kernel, ferros_sys and the tools:

- **capsule** — the signed VSF file the gate verifies (ORACLE.md). A capsule is **native** when it carries an `aarch64` section (unchecked machine code: development only, a production build refuses it) or **masked** when it carries a `masked` section (code the emitter rewrote so every access goes through the mask, with the byte count of its instructions).
- **masked** — the property of a capsule and of the process it becomes: every memory access is `base + (offset & mask)` in the checker's forms, its code pages read-only and executable, everything else non-executable. The same word in the kernel (`spawn`'s and `arc_alloc`'s `masked` flag; it was `guard` until 2026-11-03), in ferros_sys and in the build tools (`tools/ferros-masked-build.sh`, `--masked`).
- **sealed** — a masked capsule the device has checked once and recorded (its seal, a keyed hash, in the vault's `sealed` list). Only sealed masked capsules run; running one never re-runs the checker.
- **the sandbox** — the whole mechanism: the checker, the emitter, the mask, W^X, the seal and the handles. "Sandboxed" is a plain-English way to say masked and sealed, not a third state.
- **arc** — a process's memory: a power of two at a base aligned to it, from one of the two arc pools; its **block** is what the pool gave (2S for a masked arc: the upper half its guard).
- **grant** — a capability handed at spawn; its **token** sits in the grants page at its **slot** (ferros_sys::cap); the first 32 slots have a **handle**.
- **handle** — a granted region's (base, mask) pair in the handle table, the only way masked code reaches outside its arc (below, "Handles").
- **the gate** and **ihi** — the oracle's verification of a capsule and what it writes into the arc (ORACLE.md).

## Lifecycle: compile once, then run and gun

1. **Install** (once per capsule version). The capsule's signature passes the oracle gate (ORACLE.md). The device compiles it — the toka compiler from bytecode once it exists; until then the *checker* over native code produced by the ferros emitter. A capsule that fails is refused, in dev builds too.
2. **Seal.** The installed native image is sealed with a keyed BLAKE3 hash under an OS-only key derived from the system persistent secret (oracle.rs), and stored in the vault. Nothing outside the OS can produce a seal.
3. **Spawn** runs only sealed images. No instruction scan at spawn: one BLAKE3 over the image to check the seal, then `eret`.
4. **Run.** Isolation costs only the masking instructions compiled in — plain `and`/`add`/`sub`, no compares, no branches, nothing for the branch predictor. Zero cost after compilation, the way Rust's checks are.

## The invariant

Registers (reserved in codegen by the payload target, `targets/aarch64-ferros.json`, so the rebuilt `core` and `compiler_builtins` honour them too — see `targets/README.md`; `.cargo/config.toml` carries the same set for the kernel and plain builds):

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

## Handles (decided with the owner 2026-11-03, built the same day)

A masked process reaches memory through one pair, x28 and x27, so it reaches one region: its arc. Drivers and the display hold many: DMA pools, surfaces, MMIO blocks at the chip's addresses. A handle is how a granted region is reached under the mask, and it is the model KERNEL.md already names for toka (a wasm module's memory is one handle; hardware reaches a process only as handles), so the emitter and the toka backend share it.

- **The table.** For the first 32 grant slots the kernel writes a pair, the region's base and its mask, at the start of the grants page (slot k at 16k; the token count moved to 0x200 and the tokens to 0x204 + 32k, `ferros_sys::cap`). The page is written and then made read-only at every level (the map has no "kernel writes, process reads" permission), so the process cannot forge a pair; the kernel opens it to write a preference and closes it again. A grant that is not a region, or a region that is not a power of two aligned to its size, has the pair (0, 0): masked code reaching through it lands on address 0 and faults. A handle is exact or nothing (ISOMEM Claim 5: a grant reaches its own bytes and no neighbour's); an MMIO block of any other size is reached through the kernel's mmio calls, never rounded up over the block next to it (the first cut rounded up, 2026-11-03). (The first cut put the table below the arc base in a guard page, which made every native arc take a 2S block and ran init's hundred grants into its own tokens; a day's lesson, 2026-11-03.)
- **The pair.** x23 the selected region's base, x22 its mask, x21 an offset masked by it. Reserved in codegen like the arc's registers (targets/aarch64-ferros.json, .cargo/config.toml).
- **The forms,** the only three the checker admits for them: the select, `ldp x23, x22, [x28, #16k]`, one instruction so the base and the mask change together and the invariant holds at every boundary; the offset, `and x21, xN, x22`; the access, `[x23, x21]` with no shift, any width, SIMD included. No immediate offsets through the pair: a shared region has no guards, so every access is masked. Nothing else may write x21-x23.
- **The source.** `ferros_sys::Handle<SLOT>` writes those forms by hand (`read8..64`, `write8..64`, `fill64`, `pair`, `len`); the emitter passes them through untouched and refuses any other use of the registers. Native and masked code run the same instructions, so a process migrates by replacing its pointer accesses to granted regions with handle calls, and runs natively until its masked build is sealed.
- **Sharing** is the same pair written into two tables. Rights come later as a second, read-only pair that the checker allows only in loads; revocation is rewriting the pair, and a pair already in registers dies at the next select or re-entry.
- **Frame buffers under the mask.** A process that draws into its surface with the shared drawing libraries (honeycomb, tapuwae, draw) keeps them untouched: `surf()` hands them a buffer inside the arc, and the rows touched are presented through the handle (`Handle::write_words`) before any text the display draws into the shown surface (text labels are recorded during the draw and replayed after the present, or the present would wipe them). The buffer holds only the rows the process draws (the keyboard: the lowest 1600), indexed by absolute rows through a slice that starts before it, so the arc stays at 32 MiB. Such an arc lives in the big pool, which carries 4 KiB pages too (256 level-3 tables) since 2026-11-03, and ends at the secure DRAM carveout (G#B600_0000), not at a power of two; the buddy allocator takes it as aligned chunks. The image header (ferros_rt's, carried by a payload's own entry stub: ahau, the modem driver, hive, wintest) tells the kernel the zeroed tail, and the checker steps over its words.
- **The masked link** is position-independent like the native one (`-pie`, no dynamic linker, relocations kept): a static link baked link-time offsets into the formatting vtables, and the first masked process that formatted a number branched to an arc offset.
- **Verified:** `payloads/wintest` draws its surface only through `Handle<3>`; the same binary ran natively and, built by the masked pipeline, sealed and spawned masked, from the page pool and from the big pool, with the red block read back from the glass each time. `payloads/hive`, the honeycomb keyboard, runs masked and sealed from the big pool (32 MiB arc, 22 KiB of text, every instruction accepted), drawing its keys and labels through its handle (`diag arcs` lists every process's arc). `payloads/ahau`, the attest screen, runs masked and sealed as the vault's own `ahau`, started by init with its ten grants: a 128 MiB arc (its mark buffers, its proof heap, the frame buffer), 166 KiB of text, both its threads (the mark renders on the workers), the second surface kept for the display's labels and reached through its own handle, and the frame row for row what the native build drew. `payloads/touch`, the first driver, masked as it was: it reaches its registers only through the kernel's mmio calls, so it never left its arc; 10 KiB of text, running from the page pool. Masked the same way and running from the vault since 2026-11-03: `init`, `nfc`, `ip`, `bt` (no pointer outside the arc), `sensors` (its AoC IPC region through a handle) and `modem` (its three shared-memory windows through three handles; registered and the data call up in 16 s, as native) and `pcie1` (its DMA region through a handle, its vault files in buffers in its arc; firmware down and the rings up, as native). Still native: `display` (25 pointer sites over 6 regions, the NEON loops), `usb` (12 sites, 4 regions), `map` (fluor's ferros host writes the surface by pointer), `photon` (std). A masked build of a driver that still holds raw pointers to a region dies at once; the kernel's FERROS_RESCUE switch (service.rs) mends the vault's usb, pcie1 and bt with native capsules when that happens, because with usb dead the bridge cannot put them back.
- **Next:** the drivers, touch first, the display last (its NEON loops over surfaces become handle loads with the SIMD register-offset form); the read-only pair when the display needs the surfaces read-only.

## Status

- **Checker** (`ferros_sandbox`): written, tested against GNU-assembled encodings.
- **Emitter** (`tools/ferros-mask` + `tools/ferros-masked-build.sh`): every crate (payload, core, alloc, compiler_builtins) built from source to assembly with nightly `-Z build-std`, rewritten, assembled with `llvm-mc`, linked with `ld.lld`; the build fails unless the checker accepts every instruction. 2026-10-15: `ip` (5,078 instructions, +46% code) and `nettest` (742, +10%) pass with zero refusals and **run correctly on husky** — DHCP + SLAAC, ping and UDP echo over IPv4 and IPv6 (`MASKED=1 tools/husky-net.sh`).
- **Checker in the gate (2026-10-15):** a capsule with a `masked` section (code + `text` = how many bytes are instructions) must pass `ferros_sandbox` over exactly that span or the kernel refuses to spawn it; a plain `aarch64` section is native dev code, spawned unchecked (a production build refuses it). Verified on husky: the masked `ip` passes and runs; the native `ip` labelled masked is refused ("failed the sandbox checker"); the full stack (`MASKED=1 tools/husky-net.sh`) runs through the gate — DHCP, SLAAC, ping and UDP echo over IPv4 and IPv6. The declared text span is only safe together with W^X (everything after it non-executable), which is next.
- **Seal (2026-10-15):** `bridge seal <masked.bin> --text-len N` (PT capability `ferros.dev.seal`) runs the gate and the checker once and adds the image's seal — BLAKE3 keyed with the OS-only key (derived from the system persistent secret, oracle.rs) over developer key, provenance, masked flag, instruction span and image — to the vault's `sealed` list. Spawning a masked capsule requires its seal in that list and does not re-check. Verified on husky: unsealed spawn refused; seal accepted; a native image labelled masked refused at seal time; sealed spawn runs; the list survives a reboot (sealed `ip` spawns with no re-seal); the full stack (`MASKED=1 tools/husky-net.sh`, which seals first) runs — DHCP, SLAAC, ping and UDP echo over IPv4 and IPv6.
- **Guards + W^X (2026-10-15):** the pool is mapped at 4 KiB (cachemap `POOL_L3`); a fresh/freed page is writable, non-executable data. At spawn a masked process gets its checked `.text` pages read-only + executable and everything else (grants, read-only data, input, stack) non-executable; a native dev process gets its arc RWX. `arc_free` returns pages to data. Verified on husky: the sealed masked stack runs under W^X (DHCP, SLAAC, ping and UDP echo over IPv4 and IPv6); native `init` still spawns.
- **Escape proof (2026-10-15, `payloads/wxtest`):** a masked, sealed payload whose instructions the checker accepts (an in-arc store and an indirect call are legal forms) tries to (0) store over its own code and (1) call into its stack. Both FAULT — a data abort (EC G#24) writing its code page, an instruction abort (EC G#20) at the stack — and the kernel carries on; neither reaches "SURVIVED". The checker bounds what instructions exist; W^X bounds where they may write and what may execute, so the two together hold.
- **Next:** migrate the WiFi driver to streamed firmware + its DMA region inside its arc + masked code (then native is retired on husky). Then the WiFi driver: its DMA region must become part of its arc (non-cacheable pages inside the arc, the S2MPU window over them), since masked accesses only reach the arc.
