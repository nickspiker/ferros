# TOKA — ferros's compiler — Zil (0)

toka compiles WebAssembly, forked and extended where ferros needs it, into aarch64 that ferros's sandbox checker accepts (SANDBOX.md).
It is the compiler of trust model A (KERNEL.md, the owner 2026-10-15: the device compiles everything it runs): a kist carries toka bytecode, and the device compiles it once at stow, checks it, seals it and runs it.
Today it runs on the developer's machine; it is built to run on the device (pure Rust, Cranelift's codegen is `no_std` with alloc).

## Decided (the owner, 2026-11-26)

- **Ship WebAssembly** ("Still seems like we basically ship WASM").
  Measured on Photon's ferros build (zstd -19, the 24.5 MB of fonts and tables left out, they ship either way): stripped Rust source 8.8 MB, MIR 95 MB, LLVM bitcode 86 MB, native `.text` 7.0 MB.
  Source ties native and every stage between is about ten times bigger; source would put all of rustc on the device and run every build.rs and proc macro there, and AST, HIR, THIR and MIR are rustc's internal formats, changing every nightly.
  WebAssembly is a stable format the device needs only a back end for, compiled after generics are expanded, and its memories, multiple memories and 64-bit addresses map onto arcs and handles.
- **The back end is Cranelift** ("Crainlift it is! Let's build it!"), forked: pure Rust, already compiles WebAssembly to aarch64, its lowering rules have published verification work.
  Our own back end, provable from day one, was the alternative.
- **ferros-as is the bridge** (SANDBOX.md "The masking assembler"): native kists masked on the developer's machine until toka runs on the device; then retired as native dev code was.

## The pieces

- **The Cranelift fork:** /mnt/Harbor/Code/wasmtime, upstream v51.0.0 with ferros's commits on its one line, `main`.
  A shared setting, `ferros_mask` (`none`, `ring`, `fixed` with `ferros_fixed_log2`; `exact` until its retirement, SANDBOX.md "Modes"), turns on the ferros masking in the aarch64 back end, at emission (cranelift/codegen/src/isa/aarch64/inst/ferros.rs):
  - x21 to x28 are never allocated; `get_pinned_reg` reads x28.
  - Every load and store not off sp is emitted in the checker's forms, the same rules ferros-as applies to text, and every write of sp goes through x24 and the mode's one sp writer; writeback becomes an explicit adjustment.
  - No data in code: float and vector constants are built in registers (no literal pools), jump tables are tables of branches (`bti j; b target` under BTI), symbol addresses are ADRP and ADD.
  - Under BTI every return is `br x30` and every call is followed by `bti j`. Pointer authentication stays off (its keys hold shares of the ira and the wairua).
  - The atomic retry loops and CASP take fixed registers from x8 to x15, and a trap is `brk #G#C11F` (the checker refuses `udf`).
  - Branches resolve after the masking is in, and Cranelift bridges a short branch the masking moved out of reach with a veneer (ferros-as's assembler relaxes nothing).
- **toka** (toka/, its own workspace and lock, as tools/transcribe): the module reader and validator (wasmparser), wasmtime's operator translator (toka/src/translate, NOTICE beside it) over toka's own environment, toka's runtime functions, the linker, and the checker over every function.
  `cargo build --manifest-path toka/Cargo.toml` (needs the fork beside the checkout, at ../wasmtime), then `toka/target/debug/toka <in.wasm|in.wat> -o <out.bin> [--fixed K] [--bti]`.

## What toka takes

The MVP, multi-value, sign extension, saturating conversions, bulk memory, SIMD and relaxed SIMD, threads' atomics, tail calls, 64-bit memories, multiple memories, extended constants, wide arithmetic.
Refused, with the reason: garbage collection, reference types and every table operator but `call_indirect`, exceptions, stack switching, shared-everything threads, imports other than functions, and (until threads come) `memory.atomic.wait` and `notify`.

WebAssembly's out-of-bounds trap is not kept: an access is masked, so it wraps inside its arc's window, and the window's unbacked rest faults.
Division by zero, an overflowing signed division, an unrepresentable float conversion, `unreachable`, a `call_indirect` out of its table or of the wrong type: those trap (`brk`), and the process ends.

## Memories: private by default, shared by capability (decided 2026-11-26)

The owner: "we need to think about shared memory since we will likely want to have it and other fun memory tricks. none of this isolated page table shit. sharing is caring but privacy is key"; on read-only sharing thru a second window ("view, yes"), "sounds super gooder", and "Was honestly stressing on how to share the font stack among all the kists."

- **Memory 0 and the module's other own memories** are private: they live in the process's arc after its image, nothing else ever maps them, and their accesses are masked into the arc.
- **A shared memory is an import from "ferros":** `(import "ferros" "share" (memory ...))` read-write, `(import "ferros" "view" (memory ...))` read-only. The i-th such import takes handle slot 31 - i; the program binds a view it was given to that slot at runtime (SYS_SHARE's BIND, KERNEL.md "Views"), and toka compiles its accesses to the handle forms (SANDBOX.md "Handles"): the pair selected before every access (`ldp x23, x22, [x28, #16 * slot]`), the offset masked by the region's mask (`and x21, xN, x22`), the access at `[x23, x21]`. A store into a `view` memory is refused when compiling; the view's pair points into the pool's read-only mirror, so a store that got past toka would fault.
- **Its size** is its region's (the pair's mask plus one, read from the grants page); it cannot grow (its server's pool grows), and an active data segment into it is refused.
- **Not yet, each a step of its own:** atomics on a shared memory (an atomic takes a bare base register, and a handle offers none: it needs one more address register usable only with no offset, an owner's call since it costs a register; `[x25, #imm]` cannot serve, its immediates reach past a run that has no guard); the bulk operations on a shared memory (toka's runtime works on addresses); a view of a view; shared code (one copy of std or fluor in an executable mirror for every kist, running under each one's own arc).
- **Revocation** is the kernel clearing the pair: every access selects it again, so the holder's next access after a TAKE finds nothing (the kernel does not yet clear a pair a thread holds in its registers; in toka's code that is the one instruction between a select and its access).

## The image

No runtime context pointer: every function is Cranelift's tail calling convention with only its WebAssembly parameters, and what it reaches lies in its own image at a place the linker fixes, reached PC-relatively.
From the image's start (arc offset G#1000, a 4 KiB boundary, so ADRP's pages are the arc's):

- the code, each function on 16 bytes, padding `brk #0`;
- (a shared memory has no place here: its bytes are its server's, reached thru its handle);
- the globals, 16 bytes each (an immutable global is folded into the code as its constant);
- each table: per slot the callee's canonical type id and its offset from the table (a null slot's type matches none, so calling it traps);
- each memory's size in pages;
- each data segment's bytes (for `memory.init`);
- each memory, on 64 KiB, its active segments in place; `memory.grow` is branchless, up to the memory's reserve (`--reserve`, default 2^26 bytes).

toka's runtime (`memmove`, `memset`) is Cranelift IR built by toka and compiled with the rest, so the image holds no code toka did not emit.

## Status

- **M0, built 2026-11-26:** the fork masks at emission and toka compiles modules end to end, checker-clean.
  A hand-written module touching every path (loads and stores at every offset class, globals, `call_indirect`, `br_table`, loops, f64 and v128 constants, shuffles, bulk memory, `memory.grow`, atomics, tail calls, division) and a Rust crate built for wasm32 by rustc (BLAKE3, `core::fmt`, sorting, `dyn` calls, i128): every instruction accepted in ring, fixed, BTI and exact mode (the Rust crate 45 functions, 14,057 instructions in ring mode, 18,185 in exact).
  Nothing has run yet.
- **M1, next:** the image runs on the device.
  The entry stub and the image header (ferros_rt's), the intrinsic imports (`svc` and the arc's grants, ferros_sys's syscall shim compiled for wasm), where the memories sit in the arc, and the zeroed tail in place of the image carrying a memory's zeroes.
  The arc layout and how a toka process reaches its grants are a design for the owner first.
- **Then:** threads (the kernel's wait and wake on a word), the capsule section for toka bytecode and stow-time compiling on the device (toka as a std process, the seal over the bytecode's and the compiler's hashes, KERNEL.md "Stow, compile, seal, go"), std for wasm through ferros's own target, Photon; and toka's extensions to WebAssembly in CPU-FEATURES.md's order (AES and PMULL, SHA, CRC, dot product and I8MM, FP16 and BF16, 128-bit atomics, then SVE2; spirix's types when hardware runs them).

## Open for the owner

- The arc layout for a toka process, and whether a toka process's memory is the arc or a handle region in it (M1).
- Publishing the fork (a public nickspiker/wasmtime on GitHub, as ferros-rust and softbuffer are), so the toka build works away from this machine.
