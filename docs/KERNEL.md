# ferros Zil (v0) — Kernel Blueprint
**Author:** Nick Spiker  
**Status:** Pre-implementation architectural specification  
**Principle:** In mathematics we trust.

---

## Philosophy

ferros security is not built on hardening. It is built on elimination.

Every vulnerability class in this kernel is addressed by making the attack surface **not exist**, not by protecting it. 

```
Normal OS:   protect the attack surface
             harden the address space
             audit the memory
             patch the vulnerabilities

ferros:      remove the attack surface
             make the address space larger than the universe
             make plaintext not exist
             make the vulnerability class dissapear
```

The formal expression of this principle:

```
∀ attack surface A in ferros:
  A does not exist for an attacker without the key
  NOT: A exists but is protected
  NOT: A exists but is hardened
  A does not exist.
```

---

## Formal Verification Strategy

### The Proof Stack

```
Layer 0: Abstract Specification (Verus spec functions)
         "What the kernel must do"
         Pure mathematics, no implementation

Layer 1: Design Specification (Verus executable spec)
         "How it does it, abstractly"
         Proven equivalent to Layer 0

Layer 2: Rust Implementation
         "The actual code"
         Proven equivalent to Layer 1
         Type system: memory safety
         Verus: functional correctness
         unsafe blocks: individually proven, documented

Layer 3: Compilation
         Ferrocene (safety-qualified rustc)
         Proven compilation correctness
         RISC-V and ARM64 target derived from formally modeled ISA (SAIL)

Layer 4: Hardware
         SAIL formal synthesized model
         Kill circuit formally specified
         Crypto coprocessor formally specified
         Hardware assumptions: explicit, auditable
```

### Tools

| Tool | Purpose |
|------|---------|
| Verus | Rust-native formal verification |
| Isabelle/HOL | Reference: seL4 proof methodology |
| SAIL | Formal RISC-V ISA model |
| Iris | Concurrent separation logic (killswitch proof) |
| Ferrocene | Safety-qualified Rust compiler |
| RustBelt | Semantic foundation of Rust type system |

### Core Theorems to Prove

```
Theorem 0: AddressSpaceConfinement
  ∀ process p without boot_key k:
    ∀ address a in p's execution path:
      a is not computable from observable information
  P(valid execution without k) ≤ 2^-256

Theorem 1: RingMemoryIsolation
  ∀ processes p1, p2 with distinct (offset, limit) pairs:
    no address reachable by p1 is reachable by p2
    holds under wraparound by two's complement construction

Theorem 2: CapabilityConfinement
  Rights flow downward thru derivation tree only
  No process acquires rights not granted from root
  Proof: type system enforces at compile time

Theorem 3: Killswitch_Safety
  ∀ system state S at kill instant t:
    ∀ t' > t:
      kernel_instructions_executed(t, t') = 0
      observable(secret_data, t') = false
      observable(partial_state, t') = false

Theorem 4: Killswitch_Recovery
  ∀ boot instant t_boot after kill:
    system_state(t_boot) = known_clean_initial_state
    derives_from(state(t_boot), pre_kill_state) = false

Theorem 5: StorageTotalRecovery
  ∀ kill instant t, ∀ f ∈ PhysicallyRealizable:
    ∃ recovery path r:
      boot_after_kill() → known_clean_consistent_state
  P(no valid header exists) = p_corrupt ^ 1,000,000 ≈ 0

Theorem 6: GlitchResistance
  ∀ single fault glitch g on kernel security path:
    corrupt(g, encrypted_memory) → decrypt → garbage
    garbage fails all N independent checks
    default disposition: DENY
  P(spurious APPROVE) ≤ 2^(-256N)

Theorem 7: EpochSoundness
  No capability valid in epoch E accepted in epoch E+1
  Dead process caps: unreachable, not merely rejected

Theorem 8: IPCIntegrity
  Messages delivered exactly to capability-addressed endpoint
  No message observable without receive capability
  Reply caps: single-use by construction, atomically destroyed

Theorem 9: DataIntegrity
  ∀ data d in process address space:
    BLAKE3(d) = expected_hash
  Enforced at DMA level, not software level
  Normal world never receives data before coprocessor asserts valid
```

### Formal Hardware Assumptions (Explicit, Not Hidden)

```
H1: SSD Durability
    ∀ header h where h.generation < (current - K):
      h is durably committed to NAND
      not in write buffer
      K determined by SSD PLP specification

H2: DMA Integrity Gate
    Coprocessor correctly implements BLAKE3
    DMA gate is physically enforced in silicon
    No bypass path exists

H3: TrustZone World Separation (dev target)
    ARM TrustZone hardware correctly enforces world separation
    Per ARM Architecture Reference Manual
    
H4: Manufacturer Independence
    Primary and mirror SSD failure modes are independent
    Correlated failure: explicitly out of scope

H5: Kill Circuit Atomicity
    Bistable relay transition LIVE→DEAD is atomic at physics level
    No intermediate state exists
    Maxwell's equations, not software
```

---

## Architecture Overview

```
┌────────────────────────────────────────────────────────┐
│                    GLYPH HARDWARE                      │
│  RISC-V + Custom CSRs + ChaCha20/BLAKE3 + Kill Relay  │
└───────────────────────┬────────────────────────────────┘
                        │ raw hardware
┌───────────────────────▼────────────────────────────────┐
│                   FERROS KERNEL                        │
│                                                        │
│  ┌──────────────┐ ┌─────────────┐ ┌────────────────┐  │
│  │ Cryptographic│ │  Capability │ │   Scheduler    │  │
│  │  Address     │ │   System    │ │                │  │
│  │  Space       │ │  (no PIDs)  │ │  (no TLB)      │  │
│  └──────────────┘ └─────────────┘ └────────────────┘  │
│                                                        │
│  ┌─────────────────────────────────────────────────┐   │
│  │              Ring Memory Manager                │   │
│  │   Per-process (offset, limit), continuous rot   │   │
│  └─────────────────────────────────────────────────┘   │
│                                                        │
│  ┌─────────────────────────────────────────────────┐   │
│  │                  IPC Layer                      │   │
│  │  Cap-addressed endpoints, single-use reply      │   │
│  │  BLAKE3 token, epoch-bound, nonce-consumed      │   │
│  └─────────────────────────────────────────────────┘   │
│                                                        │
│  ┌─────────────────────────────────────────────────┐   │
│  │            Hardware Abstraction                 │   │
│  │  CSR access, kill circuit, crypto coprocessor   │   │
│  │  ALL unsafe lives here. Nowhere else.           │   │
│  │  Every unsafe block: documented proof obligation│   │
│  └─────────────────────────────────────────────────┘   │
└───────────────────────┬────────────────────────────────┘
                        │ capability-gated IPC only
┌───────────────────────▼────────────────────────────────┐
│                 SERVER PROCESSES                       │
│                                                        │
│  ┌──────────┐ ┌──────────┐ ┌───────┐ ┌────────────┐   │
│  │ Ring FS  │ │ Drivers  │ │ TOKEN │ │    VSF     │   │
│  │ Server   │ │(isolated)│ │  Auth │ │ Compositor │   │
│  └──────────┘ └──────────┘ └───────┘ └────────────┘   │
│  ┌──────────┐ ┌──────────┐                             │
│  │  Photon  │ │   toka   │                             │
│  │Messenger │ │    VM    │                             │
│  └──────────┘ └──────────┘                             │
└────────────────────────────────────────────────────────┘
```

### What the Kernel Does (Exactly Five Things)

```
1. Cryptographic Address Space Management
2. Capability System
3. Ring Memory Management
4. IPC Primitive
5. Hardware Boundary (only unsafe zone)
```

### What the Kernel Explicitly Does Not Do

```
× Filesystem
× Network stack
× Device drivers
× Authentication
× Display
× Anything that belongs in a process
```

---

## Execution Model: one compiler, arcs, go (decided 2026-10-14)

ferros runs only native code that its own compiler produced.
Isolation is a property of that compiler's output, not of page tables: no per-process translation tables, no TLB-based isolation, no address-space switch.

### Ingress: one backend

```
toka bytecode      (ferros-native capsules: handles, no pointers) ──────┐
wasm32             (real-world Rust: Photon, Lumis, their C deps) → toka ┼→ toka → aarch64 → sealed in vault → go
future front ends  (an RPN / AI-tailored Rust fork, ...)          → toka ┘
```

- toka is the only thing the backend compiles. A wasm module's flat memory becomes one toka handle to a byte arc; its loads and stores are accesses through that handle, so "no pointers" still holds.
- One compiler, one trust boundary: what has to be proven (Verus) is the toka → aarch64 backend plus the kernel, not every app.
- Real-world apps stay 100% Rust. They build to wasm32 with their usual toolchains; their ferros platform shim (the counterpart of their Kotlin/Android shim) talks to ferros through WASI-shaped host calls, which become capability handles to servers (network, display, camera, storage).
- Threads of one app share its arcs, each with its own stack, so tokio and rayon get real threads.
- SIMD is 128-bit (wasm simd128 / toka), which is NEON width.

### Stow, compile, seal, go

1. **Stow:** verify the capsule — the developer's ed25519 signature over its BLAKE3 hash ([ORACLE.md](ORACLE.md)).
2. **Compile once, on device:** toka → aarch64. Accesses the compiler proves in range carry no check (a row is carved once; the loop inside it is proven). The rest are masked, `base + (addr & mask)`: one AND, no branch, and no speculative out-of-bounds read.
3. **Seal:** the native code is stored in the vault, authenticated by a keyed BLAKE3 MAC under an ira-derived key, bound to (bytecode hash, compiler hash). Only this device ever verifies it, so a public-key signature buys nothing; a compiler update invalidates every seal and code is recompiled. The seal is not an identity: the process's *ihi* still comes from the developer's capsule.
   Built today over the checked image's address (owner, 2026-11-17: "worth doing. do it."): the MAC covers the capsule's BLAKE3, which every launch computes from the bytes it runs and checks anyway, in place of every byte of the code, so a launch makes no pass of its own for the seal; seals made before move to this form at their image's next launch, the old one kept until the new one reads back (SANDBOX.md "The seal is by address").
4. **Go:** what the first three steps leave is a **kist** (owner, 2026-11-07: the signed, checked, optimised capsule ferros runs; Photon and Lumis are about to be the first). Load the arc base and mask registers, set the entry point, `eret` to EL0. Switching processes is switching those registers: no table switch, no TLB, no flush.

### Registers and EL0

- Arc base and mask live in two registers the backend reserves and never lets program code write.
- Processes still run at EL0. Bounds come from the compiled code; EL0 only takes away the system controls. Code at EL2 could mask IRQs (silencing the killswitch), issue SMCs or rewrite system registers; at EL0 it physically cannot, so a backend bug can't become a dead killswitch. It costs one `eret` and no page tables.

### Arcs under masking

- A masked arc is a power of two in size and aligned to it. The requested size is rounded up; the slack is zeroed at allocation and belongs to that arc alone, never shared with a neighbour, which closes the uninitialised-neighbour leak ISOMEM describes for pages.
- An out-of-range offset wraps inside the process's own arc: ring memory at the instruction level.
- Glyph silicon does base–limit in the load/store unit, in parallel (ISOMEM proper: exact size, zero instructions). Compiled masking is the stock-silicon embodiment.
- Hardware grants (MMIO arcs) reach a process only as handles; a driver's register accesses go through them like any other arc.

### Built on husky (2026-10-14) — the process ABI as it stands

What runs today (cold boot, slot b), against the modules below:

| Module | Status |
|---|---|
| 1 Cryptographic address space | not started |
| 2 Capabilities | tokens = BLAKE3 keyed with a per-session key (derived from the wairua) over domain, object, rights, nonce, epoch; a CSpace per process; each spawn a new epoch, so a dead process's tokens match nothing; lookup compares every entry without early exit. Not yet: processes minting/deriving caps, the derivation tree, TTLs |
| 3 Arcs | power-of-two buddy allocator over the arc pool (cold boot: 8 MiB at G#8100_0000, inside the kernel window), zeroed on allocation; the static map lets EL0 touch only the pool. Masking: the checker, the emitter and the seal exist (SANDBOX.md) and handles (2026-11-03) let a masked process reach its grants; the daily processes still run native and migrate one by one |
| 4 IPC | call / recv / reply, synchronous rendezvous, messages copied arc to arc, reply tokens single-use; named dev endpoints granted at spawn |
| 5 Branchless security path | only the token lookup is branch-free |
| 6 Encrypted kernel memory | not started |
| 7 Killswitch | the key presses are the interrupt (KILLSWITCH.md) |
| 8 Storage | the ira-rooted vault (VAULT.md) |
| Oracle / spawn (ORACLE.md) | signed VSF capsules through the gate (Ed25519 over the whole file, vsf crypto-verify), ihi = spaghettify(system secret ‖ developer key ‖ hp) written into the arc; the system secret is an ira-derived RAM stand-in for the write-only CSR |

The process ABI (`ferros_kernel/src/process.rs`):
- **Entry:** EL0 (VHE: EL2 host, EL2&0 regime) at arc offset G#1000, x0 = grants offset, x1 = input offset, x2 = input length, **x28 = arc base, x27 = arc mask**, every other register (general and SIMD) zero, interrupts unmasked.
- **Arc layout:** G#0 grants page — u32 count, then 32-byte tokens (slot 0 = console, then the endpoint grants in order); the *ihi* at G#FE0 (32 bytes); code at G#1000; input after the code; stack at the top.
- **Syscalls** (`svc #0`, number in x8, every offset an arc offset the kernel masks, ERR = all ones): 0 `exit(code)`; 1 `call(cap, msg, len, reply, reply_cap)` (the message whole at any length since docs/NO-CEILINGS.md step 5; `reply_cap` bit 61 `CALL_LONG`: a reply longer than the buffer answers `TOO_LONG` | its length); 2 `recv(cap, buf, buf_cap, reply_token, flags)` (flags bit 0 = poll: NOTHING at once if no message is queued; bit 1 `RECV_LONG`: a message longer than `buf_cap` answers `TOO_LONG` | its length and stays queued, where without it the message is refused, its caller answered ERR, never cut); 3 `reply(reply_token, msg, len)`; 4 `mmio_read(grant, off)` / 5 `mmio_write(grant, off, value)` on a hardware grant; 6 `entropy(cap, buf, len)`: up to 4096 bytes BLAKE3-keyed from the session's wairua over a kernel counter (no wairua, no bytes); 7 `dma(cap)` → the physical base of a DMA region grant; 8 `input(start, buf, len)` streams spawn input; 9 `sleep(deadline)` parks the process until CNTPCT reaches the deadline (a periodic process must not spin: the service loop hands slices out on every idle pass, and a spinning process slows PT transfers until the host gives up: measured 2026-10-16; the loop also withholds slices while a transfer is open). 13 `store(cap, flags, key, key_len, buf)` on a Store grant (a scope of the device vault, keys relative to it, up to 96 printable bytes: a vault's prefix and a 32-byte address in hex): flags bit 0 set, bit 1 a piece at the offset in bits 32-63 (a read of a large value, or with bit 0 a write staged until the piece without bit 2 commits it), bit 3 delete, bit 4 list the scope's entries (read like a large value: names one per line, or with bit 5 the addresses, 32 bytes each), bit 5 the entry is kept by a 32-byte address at x2 instead of a name (kete's vaults: the address is the app's, the kernel scopes it through its keyed hash); the length in bits 8-31, at most 4096 a transfer.
- **Grants** are capabilities like any other (`bridge spawn --serve/--use E`, `--hw BLOCK`, `--entropy`, `--dma REGION`). A partition grant (`--part NAME`, process.rs `PARTS`; since 2026-10-29) reads files on one of the phone's own partitions and never writes them: the modem driver's firmware image, NV and carrier configs (partfs.rs `part_call`: [1] path → size, [2] offset path → up to 64 KiB, the last file kept open). A DMA region is a 2 MiB block of non-cacheable memory (cachemap DMA_POOL) the driver reads and writes directly; its device can DMA only there and into ferros's window (`s2mpu.rs`, the HSI2 S2MPU fence), never anywhere else in DRAM.
- **The saved frame** (816 bytes, `process::Frame`, built by `.Lel0_sync`/`.Lel0_preempt`): x0–x30, SP_EL0, ELR, SPSR, ESR, FAR, then **v0–v31, FPCR, FPSR** — the vector state since 2026-10-17. Before that no FP state was saved across a syscall or a slice tick, and the kernel's own memcpy uses the vector registers, so every f32 a process carried past a trap came back 0 (fontdue mostly survived in short bursts; a full-screen f32 render didn't). `payloads/fptest` is the regression check: runtime arithmetic, `fsqrt`, and a 4 s run across slices and syscalls counting corruptions. The kernel's own d8–d15 are saved around a process run too (`__el0_kctx`).
- **Faults** end the process (exit value FAULT | EC | PC | FAR), never the kernel. Every process's end is also written to the console (`name: exit G#…`), and a fault first prints the whole state — ESR, ELR / SP as arc offsets, FAR, x0..x30 — so it can be debugged from the console alone.
- **x27 and x28 are reserved in codegen** (`.cargo/config.toml`: `+reserve-x27,+reserve-x28` for aarch64-unknown-none). The ABI promises the arc base and mask are never written by compiled code; until 2026-10-15 nothing enforced it, rustc used x28 as a scratch register, and a process reading its base mid-call (the `ferros_sys` shim does, for every syscall buffer) got garbage — a fault report showed x28 = G#8141A748, not an aligned arc base. rustc flags the feature as unstable but passes it to LLVM; check with `objdump -d | grep -E "x2[78]"` (reads only). The precompiled core/compiler_builtins save and restore them as callee-saved.
- **Trust model A (owner, 2026-10-15):** ferros compiles everything it runs; it never trusts precompiled code. The toka bytecode is WASM forked and extended at both ends of the rustc/LLVM pipe — native i128/u128, 64-bit addresses, then spirix Scalar/Circle as value types — and the on-device compiler does the masking (so no off-device masking pass or verifier). Native `aarch64` capsule sections are a dev-only bring-up path; a production build refuses them.
- **How masking stays sound (LFI / Native Client style invariant):** x28 = arc base and x27 = mask are never written; one dedicated address register (x26) may only be written by `and x26, <any>, x27`, so it is inside the arc at *every* instruction; every load/store addresses `[x28, x26]` (or the stack, whose pointer is re-masked on every write); no system instruction but `svc`. Then a jump can land anywhere and still not escape, and checking code is a linear scan of a few hundred lines. The on-device toka compiler emits this pattern by construction (the scan stays as a second check on its output); native dev capsules don't follow it yet (an LLVM pass in the ferros Rust target + the scan in the gate would close that before toka exists).
- **`ferros_sys`** is the only crate that touches the machine (svc, the timer, the arc base, the DMA barrier); payloads use it, so moving to the one backend is a retarget.
- **Build order (decided with the owner 2026-10-15):** drivers and stacks stay plain no_std Rust compiled natively (capsule section `aarch64`) through hardware bring-up and the first Photon beam; everything touching the machine goes through one shared syscall shim (svc, timer, arc base), so the move to the one backend (Rust → WASM → toka → native with compiled masking) is a retarget, not a rewrite — and the working native drivers become the backend's first test inputs. Native payloads' two gaps until then: no compiled masking (they can reach the whole arc pool) and no relocation (below).
- **Native payloads run unrelocated at their arc base** — unless linked `-pie` and relocated at entry, which `payloads/display` does (its `payload.ld` keeps `.rela.dyn`, its `build.rs` passes `-pie --no-dynamic-linker -z norelro -z notext`, and `ferros_sys::relocate()` applies the R_AARCH64_RELATIVE entries before anything else runs; fontdue's `dyn` vtable was the first absolute address to bite, 2026-10-16). Any payload pulling in real crates should link that way. Otherwise: code must not dereference absolute pointers baked into data — e.g. a Rust `match` that yields string slices can compile to a pointer table in `.rodata` (seen 2026-10-15: data abort at FAR G#…9927 in the WiFi driver). Branch to direct uses instead; `objdump -d | grep "lsl #3]"` finds table loads. (toka's one backend removes the problem.)
- **CALL_LIVE** (bit 63 of a call's reply length; since 2026-10-29): the call fails with ERR at once, rather than waiting, when no server is receiving on the endpoint — none holds the right, or the one that holds it has not received since it started — and a queued CALL_LIVE caller gets ERR when the endpoint's last server ends. Without the bit a call waits for a server, as across a restart. The IP process calls its links (`wlan`, `rmnet`) with it, so a WiFi driver still bringing its device up, or none at all, leaves it on cellular instead of hung. Since 2026-11-14 not on an endpoint whose server the kernel starts on demand (process.rs `ON_DEMAND`: `torch`, `audio`, `camera`): its server is coming, so a live call waits for it as a plain one does, and gets ERR if it cannot be started (Lumis's first call on `camera` found the driver not yet started and said there was none).
- **A caller's end watched** (SYS_CALLER x1, 2026-11-18; docs/CAMERA.md "Serving while a frame waits"): a server holding a call's reply token names a word of its own with it, and when the process behind that call ends, any way (its exit, a fault, a kill), the kernel sets bit 63 of the word (an atomic or, so a count the server keeps in the low bits stays whole) and wakes the server's threads waiting on it (process.rs `end_watch`, `end_watch_ring`). The answer carries `CALLER_WATCHED` (bit 62); a kernel from before ignores x1. One watch a process, a second moves it; the watcher's own end drops it; a word its arc no longer holds is never written. Why: a reply token alone tells its server nothing until the reply fails, and a server holding a call for a frame 14 years long (the camera, the Pro, 2026-11-18) never replies; with the watch the camera driver closes its stream the moment its holder ends, from the wait it already makes, with no poll.
- **Time slices:** 10 ms on the EL2 timer; a preempted process continues in later slices from the service loop.
- **Boot:** after the vault opens, the kernel spawns the vault's `init` capsule — ferros's first process.
- **Dev:** `bridge capsule` signs a binary with the developer key, `bridge spawn` runs one (with `--serve` / `--use` endpoint grants), `bridge diag console` reads what processes said.
- **The console (2026-11-14):** one ring of 256 KiB (ferros_console), allocated at its first write and never grown, each byte tagged with its writer. A full ring overwrites its oldest text; `diag console` drains what it has not read yet, opening with `… A#<n> bytes dropped from <name>` for each writer whose text was overwritten first, and `diag "console kept"` reads the ring's last bytes without draining. A process's write is a copy into the ring: it never allocates, fails or waits. Until then the console was a Vec that grew until drained, and a chatty process (ip saying every echo under a ping flood) ran the kernel heap out and rebooted the Pro. Measured on the 8a (2026-11-14, the ring kernel with the old chatty ip, 64 x 1400-byte pings every 200 ms for 1000 s, about 17 KB of console a second): kernel heap in use A#1080968 bytes at all 16 samples (once A#1081000), no refusal, 319869 of 319960 pings answered, and each 256 s drain opened with about A#4.4 M bytes dropped from ip.

### Bring-up order

1. EL0 entry and the base/mask register ABI: the existing RUN payload path becomes a real process.
2. The toka VM as ferros's first process (safe by construction) while the backend is written.
3. The toka → aarch64 backend, sealing, go.
4. wasm → toka lowering and the WASI-shaped host calls; then Photon, then Lumis.

### Data caches: one static map at boot (decided and built 2026-10-14)

On ARM, with the MMU off every data access is Device memory: no data cache, exclusives (the atomics behind every `Arc`, mutex, tokio and rayon) not architecturally guaranteed, and unaligned accesses fault. There is no back door at EL2 (HCR_EL2.DC only covers the lower levels and forces stage-2 tables on). So ferros writes **one identity table at boot and never touches it again** (`ferros_kernel/src/cachemap.rs`):
- virtual = physical; every 1 GiB block is Device, exactly the MMU-off world, except ferros's own window (image, stack, heap, staging), which is Normal write-back;
- device DMA buffers live in `.bss.dma` and are mapped non-cacheable, so DMA needs no cache maintenance;
- no per-process tables, no switching, no flushes; nothing in it isolates anything — it is the cache switch, not a protection mechanism;
- RELOAD writes the window back and turns the MMU off before its copy stub; RUN cleans the payload to the point of unification.

Measured on husky: a 4 MiB copy 157 ms → 0.78 ms (~200×), BLAKE3 1.6 → 215 MB/s (~135×), cold vault open 84 → 10 ms. On Glyph silicon even this table goes away.

### The kernel heap (TLSF since 2026-11-12)

Two fixed regions in the window, never grown: 12 MiB after the kernel stack (`main.rs` `HEAP_SIZE`, 4 MiB on the handoff path) and, a couple of seconds into the service loop, 10 MiB from G#8260_0000 (`cachemap::HEAP2`). The largest block one allocation can get is therefore under 12 MiB, and what a busy kernel leaves scattered in both decides whether a large one fits.
The allocator is TLSF, the same source std processes run (`ferros_rt/src/tlsf.rs`, included by `ferros_heap/src/kernel.rs`): an alloc or a free is a few bit operations whatever the number of free blocks, and realloc grows in place when the block behind is free. The first-fit free list before it walked every free block under the lock and grew a Vec only by a new block and a copy.
Large things stay out of it: a PT command over 1 MiB lands in its own 6 MiB region (`ptlink.rs` `BIG`); a vault value over 512 KiB is sealed and stored a piece at a time (`ferros_pieces`, `kvault.rs`); a restart streams a capsule in pieces into a staging arc and gates it there (`service.rs` `spawn_from_pieces`). Before that, storing and restarting the display's 2.8 MB capsule held five 3 MB blocks of the heap between them, and with Mahere running the heap had the bytes but not the holes: two panics on the Pro (PREV_EXIT G#EE91, 2026-11-12).
`diag heap` reports it: the regions, in use and its peak, the free bytes and blocks and the largest, the free blocks by size (one line a power of two), and the last refused allocation. A refused allocation is recorded before it is answered (its size, alignment, the core, the return address into whoever asked, the heap then), and a panic carries it: r[6..8] of the report block, a line in the RAM log, and the next boot's flash log.

### Text on the glass when things go south (2026-11-17)

The owner, 2026-11-17: "Do we have any text dump panic hook code to show something on screen if shit goes south?" Until then a panic's report reached a person only over USB, on the next boot's DIAG.

- **The glass console** (`ferros_glass`, host-tested): the 8x16 bitmap font scaled by the span over 2^9 (3 on the Pro, 2 on the 8a), word wrap at the screen's width, manual line returns, line by line down to the bottom margin. Past the bottom one row says `...`: a panic keeps its earliest lines (the headline and the message), the notices their latest. No allocation, no locks, no input that makes it panic.
- **Which buffer:** the DPU's own registers (`display::scanned`): the full-screen window's layer, its DMA base, stride and format, after the PMU says the DPU is on. ABL's splash before the display server runs; the phase on the layer after, and then all four phases (G#8C00_0000, 16 MiB apart), in the panel's baked BGRA1010102 codes. Android's compressed layer after a jump is left alone. The frame goes at the panel's next TE (trigger armed, no wait).
- **A kernel panic** (`platform::report_panic`): the report into the result block first, then the screen ("ferros panic", the message, file:line:column, the core and last crumb, the heap's last refusal), then a hold until a press of power or 32 s, then the warm reset. The first core to panic draws; another parks. A nested panic falls straight to the report and reset; a fault while the screen is up resets at once (`.Lexc_glass_reset`). Every other pet stops at the claim, so a hang in the screen resets within the watchdog's second. The kernel lock is taken if free, so the display server cannot redraw over it.
- **A process that dies** by a fault or a panic: one line (`name: panicked at file:line:column: message (thread …)`, or `name: fault, ESR …, FAR …, PC +… LR +… in its arc, thread …`) on the console, and on the glass when no display serves (it was the display, or none came up); the whole of it in the flash log as typed fields ("A process that dies", below).
- **A stalled boot:** 16 s after init started, the vault, usb or display not up is named on the console, in the flash log and on the glass.
- **Development:** `diag panic test` panics on purpose; `diag glass <text>` puts a notice on the glass (`\n` a line return). Neither is in a production build.

### A process that dies (2026-11-18)

The owner, 2026-11-18, after three deaths on the 8a whose flash log said only `GPS: panicked, its last line:` (empty), the same for Compass, and the display's last line: "can we get proper panic unwind so that's a bit more helpful? and is there a way to actually log that so someone doesn't have to put eyes on it and then it vanishes?"

- **A panic tells the kernel** (SYS_PANIC, 46): the message (the first 2^9 bytes, UTF-8, formatted once into a buffer on the stack), the source file (2^8 bytes) and the line and column, as values in registers and the process's own memory, never a line to parse. It does not end the process; the exit that follows does, and its end carries the report. A second report replaces the first.
  - A payload's panic handler: `ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)` (every payload's since this date), which calls `panic_tell` and exits G#DEAD. A payload that gave its grants (`panic_grants`, the pucks) also says `name: panicked at …` on its console first, for a kernel from before SYS_PANIC. A panic inside the message's own formatting goes straight to the exit (`TELLING`).
  - A std program: std's default hook calls `ferros_rt::panic_report(msg, file, line, column)` before it prints (the fork's `default_hook`, STD-TARGET.md), then panic=abort ends it. A custom hook (`set_hook`) tells the kernel nothing.
- **The kernel walks the call chain** (process.rs `frames`) at the SYS_PANIC trap from its x29 and at a fault from the faulting frame's x29: the PC, then each frame record's return address, as offsets into the arc. Each record is read only where `span` resolves its 16 bytes (the arc but its given-back holes, or a DMA region the process holds), 8-byte aligned; each next record must lie above the last (the walk ends on any chain); at most 2^5 frames; a return address outside the image's code (`text_end`) ends it.
  A fault adds ESR, FAR, PC and the link register (a leaf function's caller, which no frame record holds yet) and the thread (`name.k (label)`, its SYS_THREAD_NAME).
- **Frame pointers everywhere**: every process keeps an x29 frame record in every function (the `el0` alias's `-C force-frame-pointers=yes`, `"frame-pointer": "always"` in both ferros targets, so std and a masked build's core and alloc too). Without them the payloads kept a record in 7 functions of 50 (bt) and the chain broke at the first function without one.
  - Measured, native (`.text`, same source): panictest 6092 to 6260 bytes (+2.8 %), vaultbench 16548 to 16840 (+1.8 %), pqprobe 87928 to 89524 (+1.8 %), display 1165864 to 1183392 (+1.5 %); the image with its data +0.6 % for the display.
  - Masked (puck_compass): 168322 to 190359 words of `.text`, +13 %: with a frame pointer LLVM addresses locals off x29 (1620 such accesses become 6482), and ferros-mask masks every access not off sp. Teaching the mask and the checker that x29 is a base like sp (masked on every write, a load into it included) would take most of that back; a design for the owner, not built.
  - Speed, native (pqprobe on the Pro's X3, b977, the flashed kernel, alternating builds): SPHINCS+ keygen 51, 36 ms without and 39, 36 ms with; SPHINCS+ signing 350, 357 ms without and 358, 353 ms with; the SHA-256 chain 2502, 2420 us without and 2421, 2498 us with. No cost shows above the run-to-run spread (about 2 %).
  - Line tables: the ELFs carry DWARF line tables (`profile.release.debug = "line-tables-only"` in the alias and the masked build), never the image (objcopy keeps only what loads).
- **The end, whole, in the flash log**: every process's end is a `kernel.process` ledger entry (flashlog.rs `death`), its words the sentence, its fields typed (ferros_ledger death.rs, LEDGER.md "Deaths"): `end` (exit, panic, fault), `exit`, `message`, `file`, `line`, `column`, `thread`, `esr`, `far`, `pc`, `lr`, `frames`, `code` (where the image starts in the arc, G#1000) and `last_words`. Until this date an end was a text line, and a panic's was its last console line.
- **Reading them**: `ferros-bridge deaths [n]` (BRIDGE.md) asks `diag "log deaths [n] [blocks] [exits]"` for the newest n panics and faults (plain exits too with `exits`) in the newest blocks of the ring (2^4 and 2^10 by default; the ring is 2^14 blocks a copy) and names each frame from the process's ELF. The whole ring (1549 blocks used, about 14 thousand entries, each opened) answered in 0.83 s with the USB round trip (the Pro, b980).
- **A panic inside the message's formatting** (a Display that panics): the second panic tells the kernel its own place, with its message when that needs no formatting (else "a panic while formatting a panic's message"); a third exits at once.
- **Run on the Pro** (2026-11-18, b978, this kernel RAM-reloaded, payloads/panictest and std/panictest hand-spawned):
  - panictest 00: `panictest: panicked at payloads/panictest/src/main.rs:53:14: range end index 410176 out of range for slice of length 64 (thread panictest)` on the console, and `ferros-bridge deaths` named nine frames: `panic_tell`, `panic_report`, the handler, `core::panicking::panic_fmt`, `slice_index_fail`, the index inlined into `panictest::inner` (main.rs:53), `middle` (36), `outer` (31), `payload_main` (24).
  - panictest 02 (a load from G#10): ESR G#9200000E, FAR G#10, PC +G#10D4 `read_volatile` inlined into `inner` (main.rs:58), then `middle`, `outer`, `payload_main`, the LR `middle`.
  - stdpanic (a slice index in a thread named worker): `stdpanic: panicked at src/main.rs:9:6: range end index 410176 out of range for slice of length 64 (thread stdpanic.1 (worker))`, eleven frames from `ferros_rt::panic_report` thru std's `default_hook`, `panic_fmt`, `sum_to` (main.rs:9), `work` (16), the thread's closure and `thread_start` to ferros_rt's `trampoline`.
  - `diag "panic test"`: the panic screen, the warm reset into the flashed kernel (b979, whose old line said `previous boot ended: … text "rvice.rsdiag pan"`), then this kernel reloaded (b980) logged `kernel.panic`: `at ferros_kernel/src/service.rs:4632:21: diag panic test: a panic asked for on purpose …, in boot b978, core A#8, last crumb G#75`. A clean reload (b978) logged nothing; the flashed kernel's next clean boot (b981) logged the old line over the vault's words (`panic-num 10490964300627057462 … text "....?.....T\".wzK"`: the wairua's fingerprint and 16 bytes of the vault's root, as a panic).
  - The flashed kernel opened the new entries (death fields and all) and showed their words: the old reader passes over the fields.
- **The kernel's own panic** (platform.rs `record_panic`): besides the report block's words, a record of its own at RESULT + G#1000: the boot, the core, the crumb, the line and column, the file and message whole, the heap's last refusal, sealed by a BLAKE3 over all of it. The fault sink (main.rs `.Lexc_recover`) leaves its own at G#1400 (a magic, ELR, FAR, ESR). The next boot believes a record only when it holds (the magic, the hash, the lengths within their fields) and writes it as a `kernel.panic` entry (`end` 3 panic, 4 fault; `of_boot`, `core`, `crumb`). Until this date the next boot logged "previous boot ended: panic-num … text …" whenever any of the block's shared words was not zero, and the vault's own boot report fills them every boot: after every clean boot it logged the vault's words as a panic, with 16 bytes of text when there was one.

### Restarts (2026-11-21)

The owner, 2026-11-21, after the deck (the map) panicked on the Pro after an unstow and nothing started it again: the kernel and everything else ran, but nothing answered a touch, so the device looked frozen until the bridge restarted it: "Yup. keyboard, speaker, whatever you think needs to be on the restart list. Not sure if there's a proper crashloop detect too but your call there."

- **Who owns it: the kernel** (process.rs `KEPT`, `crash_answer`; service.rs, the service loop's kept block). Kernel-out prefers a process, and init's keepers hold restart policy for plain exits (KERNEL-OUT.md row 9), but the death of a process is the kernel's to see: it ends the process (`end_process`) and reads how (a panic, a fault, a plain exit) at that moment; it is the only one that can cover init itself and the processes it starts with grants of their own (the vault, the GPU service, the stower: init's grants page is full); and the restart path, with its grants, the identity's gates, the seals and the streamed capsule, is already the kernel's (`diag restart`, the vault's restart, the radio switches, the on-demand servers). One mechanism and one crash-loop count for all of them.
- **What is answered:** a death by a panic (exit G#DEAD, or 101 from std) or a fault (`FAULT` in the top byte). A plain exit is the process's own choice (usb ends after its tries with no cable and init's keeper starts it again, the torch and the speaker end when idle), and a kill from outside (`diag kill`, a radio switched off, the GPS unclaimed, the person's Stop) is the kernel's, the bridge's or the person's: neither is answered.
- **The list** (`KEPT`, in the order several are started when they died together): `display`, `touch`, `ahau` (the keyboard and the attest screen), `usb` (the bridge's way in and its rescue), `ip` (the network), `nfc` (NFC and the motor: its start power-cycles its chip), `gpu` (its start brings a wedged GPU back, GPU.md), `fgtw` (the identity service), `map` (the deck), `kists` (the stower), `init`.
  Already restarted by their own paths, and counted toward the same rule: the vault (any end after it served, `vault_ended`) and the on-demand servers (`ON_DEMAND`: the speaker, the torch, the camera, the icon decoder: started again by the next call on them).
  Left out: the camera driver as a kept process (never killed while streaming; a dead one is started again by the next open, on demand), GPS (started and stopped by its claims), the modem and WiFi (power sequences of their own; init's keeper starts WiFi again on its exit with its throttle backoff), Bluetooth (its chip shares WL_REG_ON with WiFi: a BT power cycle under a live WiFi wedged it once), sensors (init's keeper starts it again on any end, and its start asks the AoC's start again), kists' own processes (the person's; the deck shows the stop).
- **Event-driven:** the death sets the process's bit (`KEPT_WANT`) and wakes the service loop (`sev`), which starts it from its capsule at once, as `diag restart` does (`restart`): its grants as init gives them, the deck only on a bound device, the identity service and the stower only with a person present. One running again already (started by hand or by init's keeper meanwhile) is left alone: nothing is killed, and the spawn is made only while no live process has that name, checked under the kernel lock as the process is made (`restart_once`, process.rs `spawn_as`), as init's `spawn_unique` checks, so the two never start a second beside each other; one refused for want of an arc is tried again once an arc comes back (`ARC_FREES`), never on a timer.
- **Crash loop** (ferros_kist::restart `Deaths`, host-tested): the 2^2nd death within 2^5 s of the first of those four is not answered, timestamps compared at the death. The console and the flash log say so (`restarts: map died A#4 times within A#32 s (the last by a panic): not started again until diag restart map`), and the person gets a note. An on-demand server in a loop is not started at its next call (its callers answered ERR, said once); a vault in a loop is gone, its waiting calls refused. `diag restart <name>` clears the count and the stop (`restart_clear`).
- **Loud:** the death itself is said as before ("A process that dies"); the restart is a console and flash-log line (`restarts: map died by a panic and was started again`); the person sees a note from the restarts source (ferros_kist::note `RESTARTS`, status, one a process, revised by its next restart): the Deck button shows the settings circle's ring in for you's violet, a tap on it opens the about page, which lists each line ("The deck restarted after a panic", "The deck kept crashing: not restarted") and marks them seen. A note needs a person present: one made before attest waits in the service loop until someone has. The deck's own restart is noted to the new deck, which reads the list as it starts.
- **Clients:** apps on a restarted display and a restarted deck re-attach as they do since 2026-11-20 ("Restarts:" commits: pucks re-attach, the map re-raises the app in front, the display tells re-attaching apps where they stand).
- **The identity across a restart:** unlocked, bound and the person key are kernel state (and the keys live in registers): a restarted ahau or fgtw finds them as they were. ahau started again with a person unlocked and bound shows no attest screen, only the keyboard, while fgtw holds the roots. fgtw holds the identity's roots (seed, handle proof) in RAM alone unless unlock at boot is on: a restarted fgtw then holds none, so the kernel starts ahau again after it, whose start asks fgtw's state and, finding no roots, brings the attest screen up over whatever runs (AUTH.md "Never stuck"): the handle typed hands them over again. With unlock at boot on, fgtw loads them from the vault and ahau stays the keyboard.
- **init started again** starts only what of the shell is missing (payloads/init: `spawn_unique` for every shell process, NOTHING read as running already), registers its identity doorbell again and serves `launch` again; its keepers start with it.
- **Run on the Pro** (2026-11-21, forge 82b908c4 RAM-reloaded as b1090, then flashed): init (masked, .textlen 236344, sealed) and the map stored from the same tree, each read back equal. A death is made by `diag "kill <name>"` and then panictest spawned under that name (`ferros-bridge spawn panictest.bin 00 --name map`; 02 for a fault), so the kernel sees a kept name die by a panic thru `end_process`.
  - map: started again 36 ms after the panic (`restarts: map died by a panic and was started again`), its kists re-launched as running, the display re-attached; a fault (02) the same.
  - The crash loop: four deaths of map within 27 s, three restarted, the fourth `restarts: map died A#4 times within A#32 s (the last by a panic): not started again until diag restart map`, map left dead, one note (the restart note revised); `diag "restart map"` answered `restarts: the crash loop's stop cleared for map` and started it, and the next fault was restarted again.
  - display: started again in 34 ms, the deck re-attached (`the deck says how the windows stood`).
  - fgtw: started again, then ahau after it: ahau found the roots (unlock at boot on) and said `the identity was unlocked at boot; no attest screen`, staying as the keyboard.
  - init: started again with init's grants; display, touch, ahau, usb, pcie1, ip, sensors, nfc, bt and modem each "running already", fgtw and the map left as they were, nothing started twice.
  - The four notes waited in the person's vault and came back after the cold boot (`notify: A#4 notes waiting on the person, from the vault`), then cleared by hand (`diag "notify clear ff…ff"`, 64 digits). The Deck button and the about page were not looked at on the glass.
  - Flashed both slots (the flashed image's own hv "5624 ufs core", the 2026-10-17 Image, the vault carried, two ferrosRT images; the old gate rebuilt byte for byte first), whole 64 MiB, first try, read back equal: keys/husky/images/boot_restarts_82b908c4_vaultcarried.img (blake3 444e2403...); before (both acf1b778): keys/husky/images/before-restarts-2026-11-21/. Cold boot b1091: identity unlocked at boot, every process up; `diag "android here"` reached Android (boot_completed, CPUs 0-2,4-8), `adb reboot` came back to ferros (b1092) with the identity unlocked at boot.
- **Left for later:** init's keepers still wake every 4 s to ask whether usb, pcie1 and sensors are alive (a poll); a death bell would let them wait instead.

### When init does not start (2026-11-21)

The night of 2026-11-21 a RAM reload of forge 8f04f065 left the Pro silent: two grant slots added for the receiver of `display` (which init is too) pushed init's full page past its 108 slots, `build_from` refused init's spawn ("more grants than the grants page holds"), and with no init there was no USB, no display and no shell (GPU-COMPOSE.md 6.5). The kernel has had no USB of its own since 2026-11-01, so nothing could reach the device until a hand held its power button.

- **The rescue** (service.rs `usb_rescue`): whenever init is not running at home, the kernel starts the usb process itself, from the native capsule the kernel image carries (`USB_CARRIED`, target/akita-usb.capsule.vsf beside the tree at build), exactly as a vault with no init has since 2026-11-18: usb's grants as init would give them, checked by its signature, no ihi. One copy of the capsule in the image serves both (still one usb "ferrosRT" header in ferros.bin, two with the carried vault).
- **When:** init's spawn refused at home (any reason: grants, the gate, a seal, no ira, no arc); the vault holding no init; `home.off` (no shell on purpose, and a measurement session needs the bridge; not drawn on the panel there); and at the boot's 16 s deadline (`glass::boot_watch`) when no usb is up and init is not running (it died or crash-looped before it started usb). Nothing is started when a usb process runs already. init's own start is unchanged.
- **Loud:** each case says `usb rescue: <why>: the usb process started from the kernel's capsule, so the bridge can reach this device` on the console (`diag console`, the pstore ring), in the flash log (`diag log`) and on the panel (glass.rs `notice`); after a refusal `home: init refused: <the reason>` comes first. Refused or not carried, it says so: `no USB until a reset`.
- **The display, checked (not changed):** init carries on past a display it could not start (`could not start display`, payloads/init) and starts usb fourth: the bridge still works. It waits for the attest screen's window only with a 4 s deadline (`wait_front`), so with no display serving usb comes up about 4 s late (at once when the identity is unlocked and bound at boot). A display that starts and then crash-loops is the kernel's kept restart's, and `boot_watch` names it at 16 s.
- **Not covered:** init running but stuck before usb with no deadline (none found: every wait before usb in init has one), and a usb capsule that is itself broken while init runs (init's keeper keeps trying the stored one; `FERROS_RESCUE` is the remedy there).

**The morning test** (whoever has the Pro next; not yet run):
0. Build the test kernel from forge with init's spawn refused on purpose, the vault carried as the flashed builds do, and target/akita-usb.capsule.vsf beside the tree: `FERROS_INIT_REFUSE=1 FERROS_VAULT_CAPSULE=<a native vault capsule from the same tree> cargo build -p ferros_kernel --target aarch64-unknown-none --release` (its own CARGO_TARGET_DIR in the farm). Check: two "ferrosRT" headers in the flat binary. `FERROS_INIT_REFUSE` builds are RAM reloads only, never flashed.
1. From the flashed kernel, a minute after boot (a reload 8 s in once hung), with no camera process alive: `ferros-bridge reload <that kernel>`.
2. Expect the device back on the bridge within a few seconds: `ferros-bridge status`, then `ferros-bridge diag console` showing `home: init refused: spawn: more grants than the grants page holds (108 slots and the console)` and `usb rescue: init refused (...): the usb process started from the kernel's capsule, so the bridge can reach this device`; the same line in `diag log`; the amber line on the panel; `diag arcs` showing vault and usb, no init, no display.
3. `ferros-bridge reboot` back to the flashed kernel (cold boot, identity unlocked at boot as before), then the same reload with a forge kernel built without the flag: init and every shell process up, no `usb rescue` line (the ordinary path untouched).
4. Optional, the deadline case: right after the flag-free reload, within 16 s of init's start, `ferros-bridge diag "kill init usb"` (its answer may be lost with usb; a kill is not answered by the kept restart, so neither comes back). At 16 s after init's start expect `boot stalled: usb not up A#16 s after init started`, then `usb rescue: init is not running and no usb is up A#16 s after init started: ...`, and the bridge back. Then `ferros-bridge reboot`.
---

## Where it runs on husky: the big core (2026-10-22)

The kernel is one core's worth of code (no locks, one service loop, processes in time slices on the same core), and on husky that core is the **Cortex-X3** (CPU8, affinity G#800), not the A510 the bootloader starts on.

How it gets there (`ferros_kernel/src/x3.rs`):
1. Right after the cache map is on, CPU0 asks EL3 to start CPU8 (PSCI CPU_ON). The X3 enters with its MMU off, loads CPU0's live translation and per-core registers from a hand-over block in non-cacheable memory, and parks on its own stack. The registers are HCR, MAIR, TCR, TTBR0, SCTLR, CPTR, VBAR and CNTHCTL. CNTHCTL is the one easily missed: without it, EL0's first counter read traps.
2. The boot continues on CPU0: the gate, the checklist, the disk, the vault. On a cold boot the X3 has long since parked by then, and reloads show it parked within 90 ms.
3. At service start, CPU0 moves the boot state into a static, says go, and parks itself (interrupts masked, in WFI; no CPU_OFF, so its cluster and the watchdogs stay as they were).
4. The X3 takes the state and runs `killswitch::arm` and `arm_power_key`, which route the keys to it. It raises its own clock to 2.91 GHz through ACPM and runs the service loop. The slice timer's PPI is enabled by the first slice, on whichever core that is.

If the X3 is not parked by service start, or doesn't take the hand-over within a quarter second, CPU0 runs the service as before. A hot reload issued from the X3 starts the new kernel there, and it stays. `FERROS_X3=0` builds keep everything on CPU0.

Measured (`diag x3`, `diag dvfs`): CPU0 at 1.70 GHz does about 4.9 G simple adds a second over four chains; the X3 at 2.91 GHz does 11.4 G. On the display server's real work (bilinear icons, discs, NEON publish), a pan frame went from 135–185 ms to 20 ms and a pinch frame from 257–305 ms to 29 ms. The A510's NEON is shared between core pairs and is weak; the X3's is not.

## Threads, interrupts and cores (design, 2026-10-23)

What exists: one core, one thread per process, run-to-block with a 10 ms slice timer as the backstop, one FIFO run queue, and the kernel's own service loop (USB, PT, the watchdogs) taking a turn between slices. It is enough for the shell at 93 fps, and the day it looked too slow (the quarter-second drag stall) the fault was a server spinning its slice on a stale deadline, not the model — found with `diag sched`, the scheduler trace. Two rules came out of that day and hold for everything below: **a server never holds the core while it waits for anything**, and **a wait with no future deadline is a yield**. The rest of this section is how the model grows to threads and eight cores without giving those up, in the order it should be built.

### Scheduling on one core (the base every core runs)

- **Direct hand-off both ways.** A `call` to a server waiting in `recv`, or asleep between polls, switches straight into it on the caller's core (built; the sleeper since 2026-11-11). A `reply` switches straight back to the caller; the server goes on from the front of the queue while its front credit holds (its run since it last waited at the back, leaking an eighth of the time passing, under a slice), else from the back (2026-11-11: at the back always, a polling driver waited ~9.7 ms for a core after every reply). process.rs "who runs next after a wake" has the rule and its fairness bound.
- **A sleep whose deadline has passed is a yield**: anyone runnable goes first, the sleeper comes back round (built 2026-10-23). A server's end-of-pass sleep therefore always gives the core up, whatever its bookkeeping did to the deadline.
- **The slice timer is a backstop**, never the pacing. If a trace shows round trips quantised to SLICE_MS, something is spinning; find it, do not shorten the slice.
- **The kernel's service loop is the idle task.** It runs when nothing is runnable (and between slices only while a PT transfer is open, which it must pace). Nothing runnable means WFI on the event stream, 333 µs ticks (built 2026-10-23; before it the shell kept the X3 at 2.9 GHz for as long as ferros was up).

### Threads

A **thread** is a saved frame, a stack and a scheduling state, belonging to a process; the process is the arc, the capability space, the name and the *ihi*. Threads of one process share the arc and every capability in it (KERNEL.md Execution Model: "each with its own stack, so tokio and rayon get real threads"). The run queue holds threads. `call`, `recv`, `reply`, `sleep` already act on "the current runnable thing"; most of the scheduler is a rename.

- **Syscalls added:** `thread_spawn(entry, stack_offset, arg)` and `thread_exit(value)`; `wait(word_offset, expected)` and `wake(word_offset, count)` on a word in the arc — a futex, so threads of one process hand work to each other without IPC. Every offset is an arc offset the kernel masks, as for every other syscall.
- **The kernel's doorbells (bells, since 2026-11-10).** A process waits for a change of the kernel's own state the same way: it hands the kernel a word in its arc (`bell_register`: SYS_BELL for a link's measures, the facts and the windows' owners, SYS_CLAIM op 4, SYS_NOT_STARTED op 1, SYS_OFFER op 0, SYS_NOTIFY op 4, SYS_UNLOCK op 11 for the identity), the kernel writes the source's word there at once and again at each change, and wakes the process's threads waiting on it (`bell_ring`), so the waiter `wait`s on the value it last read and never asks on a timer. One word a source a process; any number of processes. The identity's (2026-11-16) carries the state itself (op 0's bits) beside its change count, rung by `identity_ring` after every unlock, lock, bind, release and person key, only when the state differs (AUTH.md); init's `wait_person` sleeps on it where it asked 64 times a second.
- **Threads are not addressable.** No thread identifiers leave the kernel; IPC stays endpoint-addressed (Module 4). A reply token belongs to the process, so any thread of a server may reply. `recv` by two threads on one endpoint is allowed: the kernel delivers to whichever is waiting (a thread pool is that and nothing more).
- **Exit and kill are per process**: every thread ends, the arc is freed, queued calls and reply capabilities are cleaned as `kill` does today.
- **The frame** is the one `.Lel0_sync`/`.Lel0_preempt` save now (816 bytes with the vector state); the per-thread kernel stack is not needed — the kernel runs on its own stack and processes trap in, as today.

### Servers: receive and work are different threads

A server's endpoint must be answerable in microseconds whatever its hardware is doing. The pattern:

- **One receive thread** that blocks only in `recv`, never on hardware. It takes requests into state in the arc and `wake`s a worker.
- **Workers** do the slow things: wait for a frame to go out, for an I2C transaction, for a DMA to land. They `wait` on a word when idle.
- **The display server**: a receive thread (scene ops, camera ops, an app's publish, into a double-buffered scene), a render thread (draws whatever scene is newest into the canvas, publishes into the ring's next phase, arms the frame), a panel thread (DCS writes, brightness, screen off/on, which take milliseconds the others must not wait for). A client's call then costs microseconds with a frame in flight.
- **The touch server**: an interrupt thread feeds the contact table; the receive thread answers reads from it. No polling cadence anywhere.

### Interrupts to user space

A **grant of an interrupt** is a capability object like a hardware grant: `Grant::Irq(intid)`, minted at spawn. `irq_wait(cap)` blocks the thread until the line fires; the kernel masks the line at the GIC and wakes the waiter; the driver handles its device and calls `irq_ack(cap)`, which unmasks it.

### Armed actions (2026-11-18, the owner's choice of the generic shape)

A driver whose device needs an answer sooner than the kernel lock can promise arms a few register operations on its own line; `.Lirq` runs them at the interrupt, before it masks the line and wakes the driver, and never takes the lock.
Why: the camera's next PDMA descriptor must be released inside the frame's blanking (403 us at 96 frames a second); the frame thread's wake goes thru the lock (run_slice), which other work held 1 to 148 ms on the Pro, and a release 2 ms late stopped the stream every time (docs/CAMERA.md "The cause, measured").
- **The call**: `SYS_IRQ_ARM` (45): x0 the line's `Irq` token, x1 a `Hardware` token of the same process, x2 and x3 the program (arc offset, bytes; 0 bytes disarms the line). ferros_sys `irq_arm(line, hw, program)`; the encoding is ferros_layout `irqarm` (`encode`).
- **The program**: one to eight operations of 16 bytes (op, offset, a, b), run in order, a failed test ending the run: `TEST_BITS` (go on if `read(off) & a`), `TEST_EQ` (go on if `read(off) == read(b)`), `ADVANCE` (`write(off, (read(off) + a) & b)`, b of the form 2^k - 1: a ring index), `WRITE` (`write(off, b)`), and as the last only `UNMASK` (2026-11-18, slit-robust: `read(off)` once, so the writes before have reached the device, then the line is left unmasked after this run, `.Lirq_spi_open`; for a program that clears its own interrupt's source, so the next event fires whatever its driver is doing; a run whose test fails masks the line as before). Offsets are from the grant's start.
- **Taken, not yet pending** (slit-robust): `ferros_irq_armed` marks the line taken (irqarm.rs `TAKEN`) before its program runs, and the waiter's take of the pending byte clears it; a SYS_IRQ_WAIT that finds nothing pending does not unmask a line marked taken. Before this, a wait in the few microseconds between the interrupt and its mask unmasked the still-asserted line after the mask, and the program ran twice on one event (the camera: two descriptors released for one DMA_DONE, 8 us apart).
- **Routed to the waiter**: an armed line follows its waiter's core as devsched `follow` would (process.rs `irq_route`): on the kernel's core its waiter was noticed only when the service loop came round to `run_slice`.
- **Checked once, at the arm, under the lock**: both tokens the caller's; ferros_layout irqarm `check` (host-tested: whole registers inside the grant, known operations, sane masks, at most eight, UNMASK last); and the grant not one only EL3 may write, not a DMA fence, not a PCIe window (a read never answered would wedge the core taking the interrupt). Stored resolved to absolute addresses per line (irqarm.rs), so the interrupt does no lookup and no check.
- **At the interrupt**: `.Lirq` tests `__irq_armed[intid]` (a load and a branch, every granted line); an armed line saves the caller-saved registers as the profiler's tick does and calls `ferros_irq_armed`, which copies the program under a sequence count (an arm or disarm meanwhile: left for that once), runs it, and counts runs, acted, stopped by a test. Then the line's own path, unchanged.
- **Disarmed** when the arming process ends (both end paths, beside `cam_free`) and when it turns a power switch off (before the switch runs), so no action touches a block whose clocks or power go. Another holder of the line (init holds every grant) may arm it anew; the last arm wins.
- **Diagnostics**: `diag irqs` lists each armed line and its counts (runs, acted, stopped, mid-arm, left open) and the gaps between acted runs by 2^k us; `diag "irqs runs"` the last 64 runs with what each operation read, the stopped runs kept apart, and at a freeze every core's last 8 crumbs; `diag "irqs freeze <us>"` and `"irqs freeze short <us>"` freeze that record (and devsched's trace) at the first gap between acted runs longer or shorter than that; `diag clock camfast on|off` turns every armed action off and on (irqarm.rs `ON`), for an A/B.
- **A device thread's place** (slit-robust): one parked unasked in its run for its own interrupt (an interrupt for another line taken at EL0 on its core, or a slice it inherited ending) keeps its place, urgent, behind the thread it was parked for, and `make_room` asks a core for it (`ferros_el0_preempt`); a device thread's front credit leaks a quarter of the time passing, not an eighth (`credit`). Both measured on the camera's frame thread under Lumis (docs/CAMERA.md "Slits under Lumis").
- **The camera's program** (payloads/camera `arm`, armed at stream on): `TEST_BITS` loch0's image status DMA_DONE, `TEST_EQ` the engine's place and its index (every released program run), `ADVANCE` the index by 16 round the ring; a slit ring adds `WRITE` G#1B to the image status (the slit's bits cleared) and `UNMASK`, with loch0's image interrupt DMA_DONE alone, and its frame thread counts slits from the engine's index. The frame thread writes the index only while the engine still reads its own, so on a kernel that refuses the call (ERR: no such syscall) it releases as before. Level and edge both work this way (the line stays masked until acked). The kernel's own interrupts stay its own: the two volume keys are the killswitch and never reach a process, the slice timer and the power key stay in the kernel. First lines: the FTM5 touch INT, DECON frame-done and TE (so the render thread is paced by the panel, not by a 0.5 ms sleep loop), DSIM, and the sensors as they come. USB stays with the kernel's service loop until that loop itself becomes a process.

### Mailboxes (2026-11-19, the owner's shape)

The owner, after the GPS driver ran out of WATCH slots with six watchers: "the driver for GPS should have a capability based public read only mailbox so you could have an unlimited number of items accessing it. same with accelerometer, mic, etc."
A mailbox is one writer and any number of readers on one frame of memory: the writer publishes under a seqlock, the readers map it read-only and read it without calling the writer, and the writer never learns how many read.
Why: a call per reader (a held WATCH, a FIX asked at each ring) caps the readers at the server's slots and turns every reader into the server's work; six watchers went round the GPS driver's four slots some 40,000 times a second (GPS.md, cold boot b1002).

**Built from what was there, one syscall added.**
- **The memory** is a DMA region (`Object::Dma`) of each window, in the `DMA` table: `<name>-mailbox`, the writer's, read-write at cachemap `MBOX_VA`, and `<name>-view`, a reader's, read-only at `MBOX_VIEW_VA` (AP[2], which binds EL2 too, as the camera's view; `dma_span` leaves the views out). One frame of the pool behind both, taken and zeroed at the first ask by anyone (`mbox_frame`) and kept for the boot, so a reader's view never moves. The windows lie in the surfaces' GiB past its slots. The mailboxes are named in ferros_mailbox `NAMES`: `gps`, `motion`, `mic` (no writer yet), `test` (payloads/mbtest).
- **Mapping** is SYS_DMA on the token: the frame taken if it is not yet, the base answered. On the writer's region it also makes the caller the one writer (`MBOX` writer); a second live process is refused, with a line on the console.
- **Waiting** is SYS_WAIT on the sequence word, named thru either window as a buffer outside the arc is (`mbox_wait_word`). Every waiter on a mailbox, of every process, is kept under the view's address of that word (`State::Waiting`), whichever window it named.
- **Publishing** is the writer's stores (ferros_mailbox `publish`: the sequence odd, a release fence, the words, the sequence even and two on), then SYS_WAKE on its own sequence word, which wakes every thread of every process waiting on that mailbox in the one scan of the threads any wake makes (`mbox_wake_word`, `mbox_wake`). The writer makes no call per reader and the kernel keeps no list of readers; a reader learns of no other.
- **A torn read** is caught by the sequence: a reader that found it odd waits on it (the writer's wake ends the wait), and one that found it moved during the copy reads again (ferros_mailbox `try_read`; Boehm's seqlock, every word an atomic, so a racing read is stale or new words and never undefined behaviour). Host-tested with readers racing a writer that publishes back to back (ferros_mailbox tests: no torn publish ever read as whole).
- **The writer's end**: when its process is gone, every thread of it (`mailboxes_gone`, where the arc is freed: `end_process`, `kill_locked`, `drain_dying`'s last thread, never before, so a thread still on another core cannot finish a publish over the mark), the kernel sets the sequence's bit 63 (`DEAD`), zeroes the payload thru the frame's own address, and wakes every reader. A dead mailbox carries no place or voice of a driver that has gone. A new writer's first publish counts on from the dead sequence, so a reader waiting on it wakes. The service loop also rings a dead GPS mailbox when the driver a claim asked for did not start (`mailbox_ring_dead`), so no reader waits on a publish that will not come.
- **The capability** is the permission (sensor_mailboxes, rights_implicit_set, permissions_asked_once). A view is granted at spawn by name (`--dma gps-view`, the kernel's own lists), or opened with a proof by SYS_MAILBOX (47) op 0 (`mbox_open`): x1 the mailbox, x2 the proof's token, x3 where the view's token is written; answered the view's base. The proofs: GPS takes what proves a GPS claim (Settings, the `gps` right as its endpoint's send right) or a GPS view, so the location right asked once is the only way in; motion takes none yet, as gravity and the field are in SYS_FACTS for anyone today (a `sensors` right is the owner's to decide); the mic and the test mailbox only by name. A GPS view is itself a proof for SYS_CLAIM.
- **Writers' grants**: the GPS driver's is to be the slot after its last (service.rs `gps_grants`, slot 11 now that 10 is `rmnet`), added with its move (kept in ferros-branches-2026-11-20.bundle as mailboxes-gps-staged: it waits for the 8a's gnss_kepler to publish the mailbox too; until then both drivers serve FIX and WATCH and their readers call them); the sensors process gets `motion-mailbox` with the Sensors right, the slot after its a32 line, as that line is added (init's page is full), and publishes gravity, the field and the accelerometer there (SENSORS.md "The motion mailbox").
- `diag arcs` lists each mailbox with a frame: its writer, its sequence, dead or not, and how many readers wait.

**The isolation, said plainly.** A reader cannot write thru its view: the table says read-only for EL0 and EL2. A masked reader reaches nothing but its capability's regions. A native reader could still name the writers' window by its address, as it could any region in this one shared table (no per-process tables yet: pages_allowed_when_equivalent); native code is trusted by its signature, as for every region.

**The cost**: a frame (2 MiB) per mailbox in use; a publish is the stores and one syscall, whose wake scans the threads once (as every SYS_WAKE); a reader's wait is SYS_WAIT. Measured on the Pro (b1005, this kernel RAM-reloaded, payloads/mbtest): a writer at 2^7 a second, 1024 publishes of 64 words. With one reader each publish cost the writer 12.6 us on average (93 us at most) and the reader woke 19 us after the publish on average (105 us at most). With 64 readers (four processes of 16 threads) every wake made all 64 runnable, the publish cost 122 us on average (208 us at most: the wake's enqueue, about 1.7 us a reader, the writer's only per-reader cost), the readers woke 184 to 461 us late on average per process (1.5 ms at most; most between 128 us and 1 ms, the A715s queueing 64 threads), none skipped a publish, none read a torn one; each saw the writer's end as dead. The motion mailbox (gravity, the field, the accelerometer, some 300 publishes a second) under 64 readers: 1024 reads each, 0 torn, 10 to 14 looks a process caught a publish under way and read again, woke 176 to 345 us late on average; readers that slept thru a publish read the next (latest-value semantics).
**Not built**: a "someone waits" word the writer reads to skip the wake when nobody waits (a Dekker pair of fences on both sides); a ring layout for streams (the microphones': SENSORS.md "The microphones").

### Cores

husky has 1× Cortex-X3, 4× A715, 4× A510 (ASID.md). The kernel today is one core's worth of code: `k()` is a bare global, the run queue is one, there are no locks, and the one core is the X3 (previous section). Going to eight:

- **Per-core state**: a run queue, `current`, the idle loop and the slice timer per core. The kernel's shared state (processes, endpoints, capabilities, the trace) sits behind **one kernel lock** — a spinlock taken for the microseconds a trap holds it. The kernel's paths are short enough that one lock is right at this scale; splitting it per object is an optimisation with a proof cost, later if ever.
- **A thread runs on the core that woke it** unless it is pinned: a `call` into a server waiting on this core switches directly, as now; into a server waiting on another core, the caller blocks and the callee is queued there with an SGI to wake that core if it sleeps. A reply likewise. Migration is explicit (a pin), never a balancer's guess.
- **Pinning by latency class, not "UI core vs OS core."** X3: the display server's render thread (the one real compute path: discs, text, effects, the NEON publish). A715s: app processes, and render workers when a frame is tiled across them. A510s: everything that only waits — the service loop and USB, touch, sensors, the always-on display — at their lowest DVFS step, so the big cores sleep whenever nothing is runnable on them. The kernel's own path runs on whatever core trapped; it has no home.
- **Bring-up**: PSCI CPU_ON per core with the hand-over block generalised to a slot per core (the X3 hop is the first instance: the same registers, its own stack); each core initialises its own redistributor, enables its slice-timer PPI and event stream, and parks in WFI until its queue gets a thread. CPU0 may then be a worker like the others instead of parking for good.
- **Memory**: kernel structures are Normal cacheable inner-shareable, so the cores are coherent in hardware; the non-cacheable regions (DMA, the phase ring, the hand-over page) stay as they are. Processes are isolated by compiled masking regardless of core (Execution Model).
- **Power**: an idle core is in WFI; a cluster with nothing runnable drops its DVFS step through ACPM (`dvfs_set` exists per domain); the watchdogs stay petted by the service loop wherever it runs.
- **Proof**: the single-core theorems carry over per core; the new obligation is the kernel lock: every access to shared state under it, no path that sleeps or waits for hardware while holding it. That is the same rule as for servers, one level down. The register syscall (`mmio_read`/`mmio_write`) waits for hardware, a PCIe read in L1.2 for tens of µs, so since 2026-11-11 it runs without the lock when its token comes from its grants-page slot: it reads only the running core's own `FAST` entry and its process's immutable register table, and a process's end waits for its accesses in flight (process.rs `mmio_fast`, `mmio_quiesce`: the argument is there).

### Order

1. Reply hand-off and the past-deadline yield — built 2026-10-23.
2. Threads: `thread_spawn`/`thread_exit`, `wait`/`wake` (with a deadline), the run queue of threads — built 2026-10-23 (`payloads/threads` is the test; `diag threads` lists every thread and its state).
3. The display server split into receive, render, vsync and panel threads (a request ring from receive to render; a map call costs 5 µs with a frame in flight; the panel thread takes the DSI writes and the 120 ms sleep-out, under one futex mutex shared with the dev DSI ops) — built 2026-10-23.
4. Interrupt grants: `irq_wait`/`irq_ack`, the `IRQS` registry (touch INT gpn0-0 = SPI 0; DECON0 frame-start 243, frame-done 242) — built 2026-10-23: the touch server waits on its line, the display's vsync thread counts frame starts and `settle`/`present` wait for them. Two lessons: DECON comes up with every interrupt source enabled and frame-done pending, so a handler must clear everything pending or the line never drops (162,000 wakes a second); and a line is armed at the first `irq_wait`, not at grant, since init holds every grant and waits on none. TE is on a shared PERIC0 line (SPI 634, gpp0-3) and is not granted yet.
   **Interrupt storms (2026-11-13).** A level line its device still asserts fires again the moment it is unmasked, and each fire costs an interrupt on the kernel's core, a wake SGI to every core and a preemption; a modem build that acked its MSI line without clearing the CP's MSI status watchdog-reset the 8a that way (PREV_EXIT G#A3, inside syscall 19). The guard (process.rs `irq_storm_check`): before every unmask (an `irq_ack`, or an `irq_wait` that finds nothing pending) the line's GICD_ISPENDR bit says whether the device still asserts it; more than `IRQ_STORM_RUN` (1 << 10) such unmasks in a row and the line stays masked, the flash log says once "line X storms: masked; the process must clear its source", and every ack and wait on it answers ERR until a process is next built holding it. No branch on the normal path (the unmask writes the bit times "not stormed" to GICD_ISENABLER); the killswitch's INTIDs 60, 61 and 64 are never in a grant and are left out of the test as well. Measured on the 8a with the guard out of the way (`diag "irqs storm N"` sets the run until the next boot; `diag irqs` gives each line's unmasks, those still asserted and the longest run): over a few minutes of 7 MB of USB reads, display, touch, the AoC, the GPU, Bluetooth and the modem, the longest run was 7 (usb-dwc3), then 2 (aoc-mbox-a32). A test modem that never clears its MSI status had modem-msi masked 88 ms after its first MSI, at the 1025th unmask, with DAIF G#340 and the killswitch's count climbing; started again, the fixed modem had the line working.
5. Cores — built 2026-10-23, the first cut: one kernel lock over the shared tables (`process.rs`: every `pub fn` takes it, nothing holds it across a wait or a run at EL0), the saved kernel context per core, the four A715s brought up from the kernel's core through their own hand-over blocks (`smp.rs`, the X3's entry generalised) into an idle loop — a slice to whatever is runnable, `wfe` otherwise, every enqueue a `sev`; a lock-free count of the queue so an idle core looks before it locks. One global run queue rather than per-core ones, and `sev` rather than SGIs: both hold up at this scale and are simpler to reason about; revisit when the trace says so. A RELOAD switches the workers off (PSCI CPU_OFF) before the copy stub runs. Processes ended on one core while a thread runs on another are drained at that thread's next trap (`dying`). Lessons: TLB invalidation of pool pages must be inner shareable (`tlbi vae1is`) or a worker fetches through its stale entry and takes a permission fault at a new process's entry; and a kernel that is per-core anywhere (`__el0_kctx`) must be per-core everywhere.
   Idle workers (2026-10-23, later the same day): a fruitless look sleeps — with the render thread pinned to the X3 and queued whenever the X3 was busy, `RUNNABLE_N` read 1 forever and the four workers spun `run_slice → nothing I may run → look again` at 7,000 contended lock acquisitions a second; a kick for a thread wakes only the cores that may run it (`kick(mask)`, `sev` for the X3 and SGI 1 for the workers); the workers' slice-timer PPI and SGI 1 are enabled per redistributor (`killswitch::arm_core`). Contention now ~270/s, a worker wakes about once a second at rest. A `wfi` idle (the EL2 timer armed to the next deadline, woken by SGI 1, IRQs masked across the look so a kick before the `wfi` stays pending) is written behind `FERROS_IDLE_WFI=1` and **shelved**: it hangs non-deterministically on a reload (one reload fine, the next dead at boot with an "APC Watchdog Early"), and every post-mortem channel tried was overwritten by a boot-time loader (the ramoops page by the gate, the X3 result page by a running log, G#80F0_0000 by the 17 MB gate image, G#C1FF_0000 by the modem firmware in cp_rmem — which also means `APP_SURFACE` sits in modem memory and must move before cellular). Notes now live at G#B000_0000 (`platform::note`, `NOTE_n`/`PREV_NOTE_n` in DIAG), untested across a hang yet. `stop_all` sends SGI 1 and waits on PSCI AFFINITY_INFO per worker before a reload rewrites the kernel (two workers once stayed on and came back ALREADY_ON).
   **The eret race (found and fixed 2026-10-23 evening).** `ferros_el0_resume` ran with interrupts enabled (it is called from the idle loops), and `.Lel0_restore` loads ELR_EL2 and SPSR_EL2 from the frame some thirty instructions before its `eret`. An interrupt in that window — SGI kicks arrive at kilohertz once the shell moves — made the exception entry overwrite ELR/SPSR with the `eret`'s own address and EL2h; the handler returned to the `eret`, and the `eret` returned to itself, forever: a core parked on one instruction with interrupts open, no slice ticks, no traps, its thread "running" to the scheduler (the touch, map and vsync threads in turn; the map's calls to a parked touch server then froze the shell, and when the X3 itself hit it the watchdog reset the phone). The trap-return path was immune because exception entry masks IRQs; the resume and enter paths now mask them first. Found by sending a core an SGI whose handler dumps its ELR/SPSR/SP (`diag core <k>`) — the stuck core's PC was the `eret`, its SPSR EL2h — after the per-core crumbs (`NOTE_k`), the IRQ-handler stages (`IRQ_STAGE_Ck`) and the GIC active/pending words had ruled out everything else. Lesson: anything that loads ELR/SPSR for an `eret` runs with interrupts masked, no exceptions.
   **The lock's rules (2026-10-23, after two freezes).** (1) Nothing that runs with the lock held — every syscall, and anything inside `with_lock` — calls a function that takes it: those have `_held`/`_locked` variants (`alive_locked`, `set_pref_held`, `set_wall_held`); the facts provider called `alive()` from inside SYS_FACTS and the core waited on itself, freezing the phone whenever settings were open. (2) Nothing that can block on hardware runs with the lock held: the facts provider read the battery gauge over I2C inside the syscall, on a worker core; the X3 now samples it in the service loop and facts reads the cache. (3) The vault is part of the locked state: syscalls reach it holding the lock (the map's places, a setting, a spawn's capsule), and the service loop takes the lock around its own vault work (`with_lock` around the store command, the key count, a restart's reads) — one owner at a time, and with one lock there is no order to get wrong. A 2.7 MB put now holds the lock for its duration; the shell stalls for that time, which only happens on a deploy. **Since 2026-11-18 (item 9) the vault, the flash log, the partition reader, the UFS and the camera's fence have locks of their own, ranked under the kernel lock**: rule (3) now reads "the vault is one owner at a time under `VAULT`", and a fourth rule joins: (4) a core holding one of those never takes the kernel lock or a lower-ranked one (`lock` and `Rank::take` panic on it).
6. Pinning — built 2026-10-23: a per-thread core mask (`pin`, syscall 20; the workers by default, inherited by spawned threads), a core takes only threads allowed on it, with a fallback when none of a mask's cores is up. The display's render thread pins itself to the X3; receive, vsync, the map and the touch server stay on the workers, so a call hands off on the caller's own core. Unpinned, the drag fell to 70-81 frames/s (the render thread wandering between A715s); pinned, 114. Still to come: tiled rendering across the workers, the A510s for the waiting servers, `diag sched` has its core column.
   **The A510s as workers (2026-11-18, branch a510-workers; for Lumis's camera intake, which wants to leave the A715s free).** smp.rs starts all four A510s (cores 0-3) beside the A715s, through the same hand-over block (block c XOR 4 in the result page: the A715s keep +G#200 to +G#380), the same entry, redistributor and timer setup. CPU0 is free after the hop: x3.rs's park switches it off with PSCI CPU_OFF, so it comes back as a worker; after the jump from Android the boot core is core 3, parked the same way, and cores 0-2 wait on the hv's mailbox (platform `on_mailbox`). An A510 not on the mailbox is started by PSCI only once AFFINITY_INFO reads it off (a quarter second at most): the hop's core may not have finished its CPU_OFF when the service's first `launch_all` runs, and a CPU_ON of a core EL3 holds on hung EL3 on one jump in two.
   - **Only pinned threads run there.** `ferros_sys::CORES_LITTLE` (G#000F); the default mask stays the A715s. An idle A510 takes the lock only when the run queue holds a thread it may run (process.rs `RUNNABLE_LITTLE`, counted in `runnable_sync`); it leaves interrupts, deadlines and the frames' reclaim, wipe and clean to the A715s and the X3, whose wake of its thread kicks it; it sleeps up to a second, not to the next deadline. With nothing pinned there the A510s never take the kernel lock.
   - **`smp::UP` is a core mask now** (bit = MPIDR Aff1, G#FF with all eight up), as are `SMP_ON_c`, `IDLE_WAKES_Cc` and `SLICES_C0..C8` in DIAG; `IDLE_RUNNABLE_LITTLE` and `SMP_CTR_ALL` are new. `diag smp little off|on` stops or starts the A510s alone; `FERROS_LITTLE=0` builds leave them off; with the kernel on CPU0 (akita, FERROS_X3=0) they stay off, as their cluster is then the kernel's.
   - **Their clock** (service.rs `governor`, a third cluster, CPUCL0 domain 2): the busiest A510's share of time spent running threads (smp.rs `busy`, a run still going counted as it goes), not of time awake, since they sleep until an event and a sleep not yet ended would read as busy. From the 324 MHz floor to the calm ceiling of 1425 MHz (the step before the energy model's cliff), the whole table to 1704 MHz while lifted; never boosted by input; over 955 MHz they ask the shared tier active (cause "A510"). With the kernel on CPU0 the tiers keep setting the cluster, as before.
   - **Care taken:** Cortex-A510 erratum 2441009 (a TLBI and DSB can complete while an A510's access through the old entry is in flight): every broadcast invalidation in cachemap.rs ends with one more TLBI and DSB (`tlbi_done`). CTR_EL0 is taken as the AND of every core's (`CTR_ALL`, read G#9444_C004 on the Pro: IDC on every core, DIC on none), so `sync_exec` never skips a clean an A510 would need. The keys' blob now waits for the A510s too (keys.rs `AFTER_HOP`); `forget_missing` drops any that never come up. Erratum 2658417 (BF16 BFMMLA) is not worked around: nothing here emits it.
   - **Measured on the Pro** (a510-workers RAM-reloaded over forge 01c3e43d, not attested; payloads/corebench): all eight workers up (`SMP_UP` G#FF, every CPU_ON 0), DAIF G#340, the killswitch's count climbing. One thread pinned to G#000F: a dependent add chain at 1410-1693 MHz (the clock), 2.99 adds a cycle over four chains, NEON FMLA 22.6 GFLOP/s at 1410 MHz (16 flops a cycle), one FMLA chain 4 cycles a step; four at once 11.3 GFLOP/s each (8 flops a cycle: two cores share a NEON unit). The same on an A715 at its calm 1065 MHz: 3.16 adds a cycle, 16.8 GFLOP/s (16 flops a cycle), 2 cycles a step, and 16.8 GFLOP/s each with four at once. The A510s' clock under a pinned load: floor to 1425 MHz in about 0.19 s, back to the floor within half a second idle. Unpinned threads stayed off them (every mask G#F0 or G#100; the A510s' slices and lock waits only corebench's). The main at 96 a second (fll 3200, cambench's u16 add on three A715 threads) for 11520 frames, ended by its close: 1 skipped by the consumer, no error word, ispfe0 interrupt to run mean 26 us, longest 80 us. The cost: CPUCL0's rail 96-97 mW with the four A510s up and idle against 45-58 mW with them off (`diag odpm`, at rest, screen lit): about 40-50 mW, the A510s waking on every `sev` and broadcast SGI (about 2,600 times a second each). Starting them only while a thread is pinned there would take that back.
7. Device threads and the lock, measured — built 2026-11-16 (devsched.rs; the owner: "Measure the kernel lock, see if we can dedicate it something, would be nice to truly max out the sensor readout"). The camera's front end must have its next PDMA descriptor released inside the frame's blanking (about 400 us at the main's 96 a second), by the driver's frame thread woken on SPI 479.
   **Measures.** `diag lock`: every hold of the lock by length, by syscall (with its x1's low byte: the sub-call) and by site, and the last 2^8 holds past a mark (2^-13 s, `lock long <us>`) with the thread that held it; a hold of 2^-7 s or more is a "lock held long" line in the flash log. `diag dev`: per granted line, the interrupt (stamped by `.Lirq` with the counter and its core, `__irq_at`, `__irq_core`) to its waiter's wake and run, by length, the cores it landed on, was noticed on and ran on, waits that found it already pending, and every lock wait a thread made inside its interrupt's run with the hold it waited behind; `dev freeze <intid> <us>` stops the scheduler trace at the first run later than that, so `diag sched` shows what led to it.
   **What the lock held (the Pro, a cold session, idle and streaming).** Per hold: init's identity poll (64 a second, SYS_UNLOCK op 0) 300 us each, a vault read of the bound mark every time; SYS_BLOCK (the vault process's UFS commands) to 1.2 ms; a flash-log line to 0.66 ms; SYS_POWER the whole switch (the camera's on 42-66 ms, pcie1's 19.5 ms); SYS_SPAWN its capsule's read (24 ms for pcie1's 633 KB, 61 ms for the map's 1.7 MB) and its gate and build (7 ms); the service loop's restarts reading capsules (13-24 ms); the camera's region taken (4.9 ms) and wiped at its end (5.8 ms). Workers spent about 3 % of their time spinning on it while the shell and the camera ran.
   **What killed the stream** (2048-frame runs of cambench, main binned at its 96 a second, u16 add): with nothing below on, 10 of 10 runs died within 45 to 163 frames (reading the frames without the add, 3 of 3 ran clean): the frame thread waited up to 5 ms for a core behind the display's publish workers and the bench's threads (a kick reaches only idle cores; nothing preempts a compute thread for a woken one), and a keeper's respawn of pcie1 held the lock 30.8 ms and its power switch 19.5 ms twice.
   **Built:** three run-time switches (`diag "dev first|follow on|off"`, `diag "dev core <k>|off"`), off by default. `first`: a thread woken by its own line with front credit left is urgent, picked ahead of every other, and when no core it may run on is idle the first core running a thread that is not in its own interrupt's run is sent SGI 3; its thread keeps its place at the front (a preemption that is not the slice's end, `ferros_el0_preempt`). `follow`: a line is routed to the core its waiter waits from (its pinned core when it has one); every line goes home to the kernel's core before workers switch off. `core <k>`: core k runs only device threads (a thread that has waited on a line) and they run there alone. And the lock: kvault keeps the bound mark (init's poll now 20 us); an audited power switch (gps, nfc, bt, touch, amp, torch: `SWITCH_UNLOCKED`) runs with the lock let go and interrupts open (every switch, every ACPM call, the camera PMIC's bus and the pd gating pass run on the kernel's core alone, so the lock never ordered them against each other; the camera's keeps the lock while its region's fence, under the lock on any core, writes the S2MPUs its switch powers, and so do the USB link's, the sensors', the display's, the panel's and android's until audited); spawns and restarts read capsules from a 2^22-byte copy kept per name (a put or remove of that name drops it), taken out again in kernel #4: kernel #3 (dev first on by default, the cache in) hung inside SYS_SPAWN on core 6 with the lock held and interrupts masked right after the person attested (the first spawns of fgtw and the map with the cache in; the cluster watchdog reset it), cause unproven (the cache's heap pressure, dev first's SGI 3, or a wait inside the spawn); each spawn now leaves its step in its core's crumb (`spawn_step`, PREV_EXIT G#9A with the step in bits 24-31) and in the flash log while the heap's largest free block is under 2^22 bytes.
   **Measured after** (2048 frames each): `first` 2 of 2 clean with 3 and 4 threads (4 threads died 3 of 3 before the lock fixes); `core 7` + `first` 3 of 3 with 4 threads (the bench's on three cores); `first` + `follow` 2 of 3 (the death: a spawn's gate and build, 7 ms). Interrupt to run 24-30 us on average, 70-90 us at most; lock waits inside the frame thread's runs 5 us on average. Pinned instead: to the X3 the stream dies in 11 to 19 frames (it runs the service loop, the render thread and ahau), to core 7 with nothing on in 5, to core 7 with `first` + `follow` 1024 frames clean.
   **Still under the lock:** a spawn's gate and build (signature and hash over the capsule, the arc), the camera's region (zeroing and cleaning 64 frames), a cold capsule read, SYS_BLOCK, the flash log. The way out for all of them is the same: the vault and the UFS under a lock of their own, the kernel lock taken only for the tables (a syscall that reads the vault lets the kernel lock go meanwhile, as SYS_POWER now does). Built 2026-11-18: item 9.
   The owner's choice to make: whether `first` (and `core` for heavy consumers) become the default.
8. std threads: futex wakes, yields and deadlines — built 2026-11-17 (the owner: "there's zero reason we can't hit 120 for both", scrolling and pinch in Photon on the Pro).
   **Measured first** (b917, Photon scrolling by injected drags, `diag sched`): a frame took about 69 ms, Photon's render 47 to 69 ms on its UI thread, and the four rayon workers (Photon.9 to .12) did their parallel sections as whole 10 ms slices on all four A715s at once. Whatever woke meanwhile waited for a slice to end: the display's receive thread, the touch server and the map 2.4 to 4.5 ms past their deadlines, futex wakes up to 9.4 ms (word wakes 156 us on average, with 22 of 1864 past 5.3 ms carrying half the total wait). After each section the four workers yielded to each other every 2.5 us or so (rayon spins 32 `yield_now`s before it sleeps; 102 yields in 38 ms of trace), each yield a locked syscall that re-queued the thread at the back and kicked every other core (`sev` and SGI 1 to all), so idle cores woke into the lock to find the yielder already taken. Over about 1.5 s of scrolling the lock's waits were about 91 ms on each A715 and 140 ms on the X3; `sleep` (the yields) held it 40,249 times. A word wake went to the back of the queue always, a deadline's too, and nothing preempted a compute thread for a woken one except devsched `first` for interrupts. The A715s run at most A#1065 MHz (the clock policy's table, THERMAL.md decision 3), so Photon's ~46 ms of CPU a frame (12.6 ms on its UI thread, ~33 ms across rayon's workers) cannot fit 8.3 ms there however it is scheduled: about 21 ms at best.
   **Built** (process.rs, smp.rs):
   - A yield (`sleep(0)`, std's `yield_now`) with nothing queued, no interrupt waiting and no deadline passed answers without the lock (`ferros_el0_trap`); one with nothing queued that its core may run answers at once under it; a yield never kicks (`queue_back`): its own core picks from the queue at once.
   - A futex wake (SYS_WAKE) and a deadline (a sleep's, a timed wait's) join the queue as calls and interrupts do (`enqueue_woken`): to the front while the thread's front credit holds, else to the back. The fairness bound above holds as it is: it never asked how a thread was woken, only that it spends credit to be at the front.
   - `make_way`: a thread a call, a word or its deadline woke to the front, with no worker it may run on idle, has the worker whose thread has run longest in its turn asked to yield (SGI 3, as `make_room` asks for a device thread): only past 2^-11 s of that turn and past the woken thread's own spent credit, never the X3, never the waker's core, never a thread of the woken one's process (a parallel section's threads share the work), never a device thread in its interrupt's run. The thread asked goes second in the queue (`enqueue_second`), behind the one it made way for, which its core then takes; `make_room`'s yields do the same now, with or without `first`.
   - Any trap that finds the earliest deadline passed wakes the sleepers (`wake_sleepers`), so a deadline is noticed at the next syscall anywhere, not at the next slice's end; and an idle worker arms the EL2 timer at the next deadline before its `wfe`, as the kernel's core does (it waited for its event stream, 2.7 ms).
   - The lock spun test-then-test-and-set until 2026-11-18; since then it is a ticket lock (process.rs `TICKET_NEXT`, `TICKET_SERVING`: taken in the order asked, a waiter spinning on its shared copy of the serving word), because test-and-set has no order: under Lumis's pool the camera's frame thread waited up to 1029 us for one syscall with no hold that long (docs/CAMERA.md "Slits under Lumis").
   - `diag sched` counts the yields by path and the light wakes that found no worker idle and the cores asked to make way; `diag "sched all"` lists the whole ring (512 events are a few milliseconds of rayon's yields).
   - `diag "sched old on|off"` puts the behaviour before all this back at run time (locked kicking yields, word and deadline wakes to the back, no `make_way`, deadlines noticed between slices only, idle workers on their event stream), so the two are measured in adjacent runs on one boot.
   **Measured after** (b919, one boot, adjacent runs, Photon on the Pro, `sched old` against the new, tools in /mnt/Harbor/tmp/schedwake):
   | | scroll, old | scroll, new | pinch, old | pinch, new |
   |---|---|---|---|---|
   | futex (word) wake to run, mean | 247 us | 120 us | 982 us | 540 us |
   | word wakes waiting 1.3 ms or more | 848 | 325 | 911 | 401 |
   | deadline wake to run, mean | 1000 us | 266 us | 613 us | 157 us |
   | deadline wakes waiting 1.3 ms or more | 1363 | 206 | 984 | 129 |
   | lock waits, each A715 / the X3 | 710-737 / 1049 ms | 340-690 / 522 ms | 350-376 / 387 ms | 321-462 / 307 ms |
   | a thread woken with every worker busy, in the ring | 1.52-1.56 ms | 0.08-0.23 ms | | |
   | Photon's render (full) / fluor draw + present | 49 ms | 49 ms | 9.6 + 18.8 ms | 11.0 + 18.5 ms |

   The waits shrank three to six times; the frames did not move. The A715s are saturated: in 67 ms of a scroll Photon.1 ran 51 ms (a thread of Photon's taking whole slices), Photon's UI thread 34, rayon's four ~80 (with ~1,700 yields between them), the display's publish workers ~40, the modem ~8 to 12, the cores idle ~15 % in all; in 337 ms of a pinch rayon's four took 700 ms of core time and the display's ~440 ms, the A715s ~20 % idle, while the X3 was ~60 % idle (only pinned threads may use it). What is left of the stalls is oversubscription, eight compute threads on four cores: a section's last worker waits a slice (rayon's at every section start waited 8.5 ms behind the display's publish workers; the display's publish is taken by stripes, so a late worker of its own costs nothing). Two run-time knobs, off by default, to measure next: `diag "sched slice <n>"` (the slice timer at 2^-n s, 6 to 12, 0 the 10 ms default; the front credit stays 10 ms) and `diag "sched x3 on|off"` (an unpinned non-device thread may run on the kernel's core while it is free; a thread pinned there that wakes while one runs has `make_way` ask it to yield).
   For 120 frames a second the kernel's side is the smaller part: Photon's ~46 ms of CPU a frame on cores at A#1065 MHz (the clock policy's ceiling for the A715s, THERMAL.md decision 3, the owner's) cannot fit 8.3 ms however it is scheduled, and the modem driver spins on its PCIe registers (`poll`, `us(10)`) for 5 to 7 ms each time `ap_wake` brings its link back.
9. The kernel lock's long holds, built 2026-11-18 (branch `lock-holds`; run on the Pro the same day, below). The goal: no hold of the kernel lock past about 2^-10 s in normal operation, none near a frame period ever. The camera's frame interrupt is answered by an armed write (camera-full-rate, SYS_IRQ_ARM), but its stream still dies when the frame thread is woken a frame period late (10 ms at 96 a second), and every other thread waiting on the lock pays the same holds.
   **Measured before** (the Pro, `diag lock`, the camera session of 2026-11-18): the AoC's firmware pieces (aoc.fw.N, 2 MiB each) read by the service loop under `with_lock`, 43 to 130 ms a piece, every 64 s on a reloaded kernel (init's keeper restarts sensors, the AoC never comes online); the sensors process's SYS_POWER about 42 ms; the camera's SYS_POWER 43 ms and its SYS_DMA region 4.8 ms at an open; `with_lock` holds of 36 to 148 ms around spawns (init.3's), all reading as one site, process.rs's own line inside `with_lock`; the map's SYS_SET_PREF 3.3 ms after a tap; a `with_lock` of about 1 ms. From the 2026-11-16 measures: SYS_BLOCK to 1.2 ms a run, a flash log line to 0.66 ms, a process's end with the camera's region 5.8 ms.
   **Why the sites read as one:** the panic screen (2026-11-17) put `seize_for_panic` between `#[track_caller]` and `with_lock`, so every `with_lock` hold was named by its own `lock()` call. The attribute is back on `with_lock`; `diag lock` names each caller's line again.
   **Built:**
   - **The storage locks** (process.rs `Rank`, `VAULT`, `PART`, `LOG`, `UFS`, `FENCE`, in that rank order under the kernel lock): the kernel's vault, the partition reader's open file, the flash log, the UFS controller (one command and its data block at a time, the run bounce, the link's hibernate) and the camera's S2MPU fence. Each is re-entrant on its holder's core. A core holding one never takes the kernel lock or a lower rank: `lock` and `Rank::take` panic on it, so a wrong order shows the first time its path runs. Every block of the vault is read and written holding `VAULT` once the service loop lends it to the syscalls (`VAULT_SHARED`; kvault.rs `Region` panics otherwise). `diag lock` gives each one's holds, its longest and where, and the waits for it a core made holding the kernel lock (the measure of what is left to move).
   - **The service loop's vault work holds `VAULT` alone** (service.rs `vault`): the AoC's firmware pieces, a restart's capsule (`spawn_from_pieces`: the length, the staging arc under the kernel lock alone, the pieces; a value grown between the two reads refused, never copied past the arc), `start_home`'s capsules, the seals, the key count, the zones, the boot span. Code that touches both the vault and the tables (a host's store command, the bound mark's first read) takes the kernel lock first, then `VAULT`.
   - **The identity's state in memory** (kvault's `id`, the bound mark's cache) is written only holding both locks (an unlock, a lock, a bind, a release, the person key, the mark's first read), so it is read holding either: the domain keys for every call forwarded to the vault process, a kist's alias and init's identity poll (SYS_UNLOCK op 0, 64 a second) read it under the kernel lock alone and never wait for a vault read; the service loop reads whether a person is present from kvault's `PERSON`.
   - **`unlocked`** (process.rs): a syscall lets the kernel lock go for work that touches no table, with interrupts open, and takes it back. The calling thread stays current on its core, so its process's arc is neither freed nor moved meanwhile; no reference into the tables is used across it, and no process memory is read or written inside it (another of its threads may shrink its arc), only kernel buffers. A yield or a hold asked of the core meanwhile (SGI 3, which parks nothing at EL2) and a device line gone pending are asked again once the interrupts are masked, so the thread parks at its eret as it would have.
   - **SYS_SPAWN** reads the capsule and the seals, gates it, checks its seal and derives its ihi with the lock let go, takes the arc under the lock, copies the code with it let go, and makes the process under it (`spawn_queued`, `spawn_build`, `arc_for`); `once` (init's keepers) is checked again after. `spawn` (the service loop's starts) copies its code with the lock let go too.
   - **Settings** (SYS_SET_PREF, SYS_CONTROL, SYS_ZONE, SYS_LANG): the change applied under the lock, the document snapshotted with a generation at the trap's end, written with the lock let go before the call returns (`keep_settings`, service.rs `settings_write`); a write older than one kept is not made, so writes from different cores never put an older document over a newer one. A refused write answers ERR with the setting applied (it was refused unapplied before; undoing it now would race the changes after it). The service loop's own writes (`diag pref`, the zone found) go thru the same.
   - **SYS_BLOCK** runs its command with the lock let go under `UFS`, the bytes thru a copy in the kernel heap on either side (the caller's memory only under the kernel lock); **SYS_CALL on a partition** (the modem's firmware, 1 MiB a chunk) reads under `PART` with the lock let go.
   - **Power**: `sensors` (an atomic for the service loop, no hardware) and `camera` join the switches run with the lock let go (`SWITCH_UNLOCKED`). The camera's was kept under the lock because its region's fence (`cam_ask`, `cam_free`, under the lock on any core) writes the S2MPUs it powers: the fence and the region it follows now have `FENCE` (camera_power.rs `set_region`, `fence_now`), and the front end's domain, once on, is never gated, so a fence never finds it down.
   - **The camera's region** (`cam_ask`): the frames taken under the lock, zeroed (those not known clean) and cleaned with it let go, the region reserved meanwhile for the caller with no frames, mapped and fenced once the lock is back, given back if the caller ended meanwhile.
   - **Frames coming back** (frames.rs `give`, `RETURNING`): a frame given back is queued, still marked used, and wiped with the lock let go by idle workers and the service loop (`reclaim_step`), not zeroed under the lock at every process's end; a take that finds the pool short wipes the waiting ones at once under the lock as before, and a reset's scrub zeroes them with every used frame.
   - The vault's UFS commands (each block), the flash log's, the partition reader's, the slots' (devinfo), the ira's sources and the link's hibernate hold `UFS`, not the kernel lock; `vault fresh` holds every lock to its reset and shuts SYS_BLOCK (`BLOCKS_SHUT`).
   **Still under the kernel lock** (to measure on the device): an identity's unlock at attest (SYS_UNLOCK op 1: key derivations and vault reads under both locks), a tag's ops, the kernel-scope SYS_STORE, the export, the host's store commands (a deploy), `build_from` (the cspace, W^X over the arc, the code's cache maintenance), a flash log line written from a syscall (under `LOG` inside the kernel lock), the USB link's, the display's, the panel's and android's power switches. A syscall that waits for `VAULT` under the kernel lock is counted in `diag lock`'s storage lines.
   **To run** (the coordinator's go): `diag "lock reset"`, then a boot, a spawn (`diag restart`), a sensors restart (its keeper, on a reloaded kernel), a camera open, a tap on the map; `diag lock` should show no kernel lock hold past about 1 ms, and the storage lines how long the vault and the UFS were held meanwhile.
   **Run on the Pro** (2026-11-18, lock-holds 977fc960 RAM-reloaded over forge, not attested; the camera driver and cambench hand-spawned, nothing stored): no lock-order panic; `diag "lock reset"`, `diag restart nfc`, then the main at 96 a second (fll 3200, cambench's u16 add on three threads) for 16424 frames (171 s) with init's keeper restarting sensors every 20 s and a `diag restart sensors` in it, four taps (they went to the deck and launched Lumis, which ended at once on the busy camera), the stream ended by its own close.
   - The stream: 16424 frames, every DMA_DONE released by the armed action, no error word ever set, 2 frames skipped by the consumer; the frame thread 93 us an answer on average, 1011 us at most. On the camera agent's kernel the same keeper loop killed the stream (CAMERA.md, holds of 43 to 130 ms).
   - The kernel lock after the reset, past 1 ms: an arc's growth 3549 us (cambench's 72 MiB, its frames zeroed by the take), a kist's launch 3166 us (process.rs `spawn_kist`'s arc), process ends 1.0 to 1.56 ms (`exit`, sensors' each 20 s and Lumis's), a call 1028 us (the camera driver's console line kept in the flash log, under the kernel lock); everything else under 0.9 ms. SYS_POWER 31 us at most (the camera's on and off, the sensors' 5 times), SYS_DMA 24 us (the camera's region), SYS_SPAWN 406 us, SYS_BLOCK 26 us.
   - Before the reset (the boot): a spawn's arc 12.3 ms (process.rs `spawn`, frames never taken this boot zeroed by their take), SYS_NOTIFY 71 ms (the person's notes read thru the kernel's vault at the first notify call, under the kernel lock, waiting `VAULT` behind a capsule's read: the one wait under the kernel lock the vault saw), a power switch 11 ms (x1 1, a switch still under the lock: the USB link's, the display's or the panel's).
   - The storage locks: `VAULT` held 149 ms at most (the AoC's firmware, service.rs `sensor_step`) with no wait for it under the kernel lock after the reset; `UFS` 545060 holds, 1.66 ms at most (a SYS_BLOCK run), no wait under the kernel lock; `LOG` 1.29 ms; `FENCE` 4 holds, 379 us at most (the camera's on and off and its region), no wait under the kernel lock.
   **Built after the run, then run** (the Pro, 2026-11-18, lock-holds 13890fd9 RAM-reloaded, not attested; the same test: `diag "lock reset"`, `diag restart nfc`, the main at 96 a second for 16422 frames (171 s) with a `diag restart sensors` and init's keeper restarting sensors, taps that launched Lumis mid-stream, cambench's arc growth, the stream ended by its close):
   - free frames zeroed ahead of the takes (above);
   - SYS_NOTIFY reads the person's notes before the syscall takes any table reference, with the lock let go (notify.rs `preload_held`, from `ferros_el0_trap`), and queues every note's write for the trap's end, written with the lock let go by one writer at a time (`keep`, `flush_held`);
   - a flash log line said holding the kernel lock waits in its core's queue and is written once that core lets the lock go (flashlog.rs `DEFERRED`, `deferred`, from `unlock`), with the time it was said;
   - the `usb` switch runs with the lock let go (`SWITCH_UNLOCKED`; its link reset under the lock inside it): it was the 11 ms switch (at this boot every SYS_POWER held the lock 2 us at most, with dpu, panel and android unchanged).
   - The stream: 16422 frames, every DMA_DONE released by the armed action, no error word, none skipped by the consumer; the frame thread 97 us an answer on average, 988 us at most.
   - The kernel lock after the reset: no hold of 666 us or more. The longest: the kist's launch 583 us (process.rs `spawn_kist`'s build, from 3166), SYS_SPAWN 390 us, a call 313 us (pcie1), a process's end 282 us (from 1.0 to 1.56 ms); an arc's growth 49 us (from 3549), SYS_CALL 114 us (from 1028), SYS_POWER 20 us, SYS_DMA 20 us. `VAULT` held 129 ms (the AoC's firmware), never waited for under the kernel lock; `UFS` 753 us at most; `LOG` 1.5 ms; `FENCE` 364 us; none waited for under the kernel lock.
   - At the boot (before the reset): SYS_VAULT op 0 (the vault process's migration listing the kernel's vault) 46.5 ms, waiting for `VAULT` behind the AoC's firmware; a spawn's build 2.0 and 1.1 ms (process.rs `spawn`'s `build_from`; not measured further); SYS_NOTIFY 6 us.
   **Built after the second run, then run** (75258d16 RAM-reloaded, a boot and a 5760-frame stream at 96 a second ended by its close, clean, 7 skipped by the consumer): SYS_VAULT's migration ops make their vault calls (the listing, the read, the removal) with the kernel lock let go, the domain keys and the read's cache under it. At the boot SYS_VAULT held the lock 12 us at most (from 46.5 ms) and nothing waited for `VAULT` under the kernel lock; no hold reached the alarm (7.8 ms). Past 1 ms, all at the boot: init's SYS_SPAWN 2.6 ms and a spawn's `build_from` 2.1 and 1.1 ms. The stream added no hold of 666 us or more.
   **Left under the kernel lock** (from the list above): `build_from` (the cspace, W^X over the arc, the code's cache maintenance; 2.0 ms at a boot, under 0.6 ms after), and, under 1 ms when measured or not met in these runs,, an identity's unlock at attest, a tag's ops, the kernel-scope SYS_STORE, the export, the host's store commands, and the `dpu`, `panel` and `android` switches (`dpu`: EL3 calls into the PMU, which a via-EL3 grant's SYS_MMIO also makes from another core; `panel`: 17 ms of rail and reset waits at a cold on, its reset line in GPA6_CON beside the volume-up key's pin mode).
10. The service loop's lock holds, and its doze: built 2026-11-19 (branch `gov-busy-lift`). The Lumis session's saves on the Pro read the X3 "busy last 249/256, average 251/256" with the save's pool pinned off it, and BIG reaching 87-90 C.
   **Measured** (12.5 MP Lumis saves, Lumis 0.2.23 from lumis-slit hand-spawned with DEV_SAVE, forge 12c89949 RAM-reloaded, `tools/flame` at 512 Hz over the save, `diag lock`, DIAG X3_SLEEPS): the X3 was 76 % kernel and 24 % the display's render, and the kernel's part was the service loop's own passes. Every enqueue on any core does `sev`, so with Lumis's threads and a polling driver on the eight workers the loop ran a pass about 2,500 times a second, and each pass took the kernel lock up to 18 times: `power_offs_apply` once per power switch (13, 229,515 holds in 7 s, 26 % of the X3 waiting for the lock and 10 % letting it go), then the launch, stow, seal and inbox takes and `run_slice`, each finding nothing. Each pass also writes about 25 crumbs, each a clean to DRAM and a `dsb sy` (8 % of the X3, and 28 % once the passes were cheap enough to run 4,800 to 32,000 times a second). The X3 sat at 2840 MHz on average, 205/256 busy, its divider stepping (6 slow seconds).
   **Built:**
   - `power_offs_apply` reads its pending word (now an atomic, still written under the lock) and takes the lock only for a switch whose bit is set.
   - The launch, stow and seal queues set a bit in `QUEUED` under the lock; the service loop takes the lock for one only when its bit is set. The USB inbox is taken only when `INBOX_FULL` says so.
   - `RUNNABLE_KERNEL` (kept in `runnable_sync`, as `RUNNABLE_LITTLE`): the loop calls `run_slice` only when the queue holds a thread the kernel's core may run, a deadline is due, a device line is pending (`__irq_any`: with every worker busy the X3 is the one that looks), or no worker is up.
   - **The doze** (`DOZE_LOG2`, `diag clock doze <n>`, 9 by default, 0 off; DIAG X3_DOZES): a `wfe` wake with none of those, no command in the inbox and under 2^-9 s since the sleep began goes back to sleep without a pass. At rest the event stream's 2.7 ms wake is past the bound, so the passes are as before; under a storm of other cores' `sev`s there is at most one pass in 2^-9 s plus the work that is the X3's. What a pass polls that no check sees (a claim, an on-demand server, a launch) waits at most 2^-9 s.
   **Measured after** (one boot, b991, adjacent 12.5 MP saves, the A715s at 2367 MHz under the camera's lift in both): doze off, the X3 99/256 busy at 1079 MHz on average, 31,900 passes a second, BIG to 89-90 C with its divider stepping; doze on, 41/256 at 606 MHz, 782 passes a second and 25,400 wakes dozed, BIG 79 C at the end of the window with no X3 divider step. Against the start (b985): 205/256 at 2840 MHz. The save itself barely moved (6.0 s against 5.6 s): it is the A715s' work, and the heat left is theirs (THERMAL.md "The busy lift").
   **Not changed:** the crumbs (a pass still writes them; the doze keeps the passes few), and the `sev` per enqueue (a targeted wake for the X3 would end the storm at its source; the `wfi` idle that would need it is shelved, item 5).
   **Seen while measuring, not explained:** two of eleven RAM reloads (b989, b995) came up with ACPM answering nothing (every zone read G#1, the wall clock and the USB rails unread, "pd: ispfe: left on ... the APM still holds it") and the Pro reset about a minute later, as b950 did (CAMERA.md). Both were reloads with a camera process alive or just ended; the nine others (from the flashed kernel, the forge build and these) were clean. Reload with no camera process alive.
11. The arc's frames under the kernel lock, built and run 2026-11-22 (kernlock). On the Pro (b1129, flashed e5f4da21) a bridge dev run of Lumis held the kernel lock 138 ms at the staging of its arc (service.rs, the first piece's `arc_alloc_room`) and 60 to 64 ms in `build_from`, and the ultrawide's stream, open in the stored Lumis, died at that moment. The cause: `stream_room` took off only the 1 GiB the kernel lets a heap grow, where the bridge counts Lumis's whole declared 4 GiB, so the staged arc was about 3.1 GiB (about 1550 frames taken, zeroed and mapped under the lock, about 89 us a frame), and `set_arc_wx` then rewrote and invalidated it one broadcast TLBI a page.
   **Built:** `stream_room` takes off the heap as the image declares it (the arc starts at the image's first heap, 26 MiB for Lumis), with the stack's guard page on top; `set_arc_wx` walks a slot's table once and invalidates as `unmapped` does (one full invalidation past 8 MiB); `sync_code` skips the clean when every core reads CTR_EL0.IDC (the Pro does); frames.rs `prepare(n)` zeroes frames with the lock let go before a take that would zero them under it (the service loop's three arc allocations, SYS_ARC's grow and the GPU block's grow, the last two thru `unlocked`). A take that finds the pool short is handed frames waiting to come back one at a time, each zeroed by its taker, never all of them under the lock (`reclaim_all`, gone: about 138 ms after a dev run's 3 GiB arc came back).
   **Measured** (the Pro, a RAM reload of forge with these changes and the UFS flush): camtest streaming the main (bin 8, 67 ms frames) while lumis.bin was dev-run: the staging's hold 6 us (was 138 ms), the build's 231 us (was 60 to 64 ms), the longest hold of the window 837 us (camtest's camera region), none of 2^-7 s or more; camtest's 4080 frames all came, 67.28 ms apart.
   **The arc ceiling** (3e3293d5): a stored kist's arc ceiling follows its image's declared heap up to the board's pool (`image_tail_ceiling`, was capped at 1 GiB; the stored Lumis's full-resolution integrator's 1.07 GB allocation failed under the cap). Measured (slot 2, a RAM reload of forge a91b7f5d): the stored Lumis 0.2.58 opened the main's 1:1 readout (8160 x 6144) with no "Failed to allocate", its arc grown from 120 MiB to 1146 MiB in one SYS_ARC (about 535 frames, that hold 375 us, the frames made clean first), 576 whole frames integrated; the window's longest hold 979 us (the camera region at the open), none of 2^-7 s or more.
   **The log's answer and the heap** (a91b7f5d): `diag "log 131072"` (which ran the 22 MiB heap out on b1131) answered 260,809 bytes, the heap's peak 1.28 to 2.31 MB, no refusal; the heartbeat line carries the heap's in use, size and peak.
   **Host store commands** (kernlock, after a91b7f5d): under the vault's lock alone; `store get display` (1.6 MB) with camtest streaming held the kernel lock not at all (was 126 ms), the window's longest hold 178 us.
   **Still under the kernel lock:** a flash log line from a syscall; vault lock waits under it (one this boot, 1.47 ms).

What the GPU and TPU are not for: the compositor at 1344×2992 fits on NEON across the A715s, and effects (glow, shadow, shine) are separable integer blurs on sprite-cached pixels. The Mali needs its closed CSF firmware and a shader compiler we do not have; it belongs after toka, when an app needs it. The TPU is closed end to end and is not on the path.

## Module 1: Cryptographic Address Space

### Principle

The address space **does not exist** without the boot key. This is not ASLR. ASLR randomizes a base and uses a predictable layout. This makes every address independently uncomputable without the key.

### Execution Model

```
current_addr = ChaCha20_decrypt(boot_key, location_nonce)
execute instruction at current_addr
next_addr = BLAKE3(boot_key || current_addr || epoch || nonce)
nonce consumed — unrepeatable
```

### Key Properties

```
Search space per step:     2^256
At 10^18 guesses/second:   10^59 years to find next address
Universe age:              10^10 years
Attack: cosmologically impossible

ROP chains:    require known gadget addresses — impossible
Code injection: require a target address — impossible
Glitch redirect: must land on valid encrypted instruction — 2^-256
Fuzzing:        2^256 search space per execution step
```

### Boot Key Derivation

```
boot_key = ChaCha20(device_key_in_CSR, boot_nonce)
boot_nonce = BLAKE3(hardware_entropy || Eagle_Time_epoch)
Fresh every boot
Never written to RAM
Device key: in custom RISC-V CSR, read-disabled hardware enforced
```

### Formal Status

```
Theorem 1: AddressSpaceConfinement — PROVABLE
  Proof: key never observable, BLAKE3 preimage resistance,
         ChaCha20 semantic security
```

---

## Module 2: Capability System

### Principle

An object you do not hold a capability to **does not exist** from your perspective. Not access denied. Not an error. The object is not in your CSpace. You cannot formulate a valid request.

```
seL4:   object doesn't exist without capability
ferros: object doesn't exist without capability
        address doesn't exist without capability  ← novel extension
        execution path doesn't exist without capability ← novel extension
```

### Capability Token Structure

```rust
struct Cap<Rights: RightSet, E: Epoch> {
    token: [u8; 32],           // BLAKE3(domain || object || rights || nonce || epoch)
    _rights: PhantomData<Rights>,
    _epoch: PhantomData<E>,
}

// Rights violations: COMPILE ERROR, not runtime error
// Wrong epoch: COMPILE ERROR
// Missing capability: object does not exist
```

### Token Construction

```
token = BLAKE3(
    domain       ||   // security domain — cross-domain tokens invalid by construction
    object_ref   ||   // kernel object identifier
    rights_bitmap||   // what operations are permitted
    nonce        ||   // single use, consumed on presentation
    epoch             // bound to process lifetime
)
```

### Capability Derivation

```
Parent cap: Cap<SendRecvGrant, E>
Mint child: Cap<SendOnly, E>     ← type system enforces subset
            token = BLAKE3(parent_cap || restricted_rights || nonce_c)

Rules:
  Can mint downward (restrict rights): yes
  Can mint upward (expand rights): COMPILE ERROR
  Can fabricate from nothing: COMPILE ERROR
  Kernel validates child derives from valid parent
```

### Reply Capabilities

```
Call = send + atomic single-use reply cap issuance
Reply cap: used once → ceases to exist
           not "should only use once"
           physically cannot be used twice
Formal: single-use by construction, proven in Verus
```

### Revocation

```
Derivation tree: every cap knows its parent
Revoke parent → all children atomically unreachable
Epoch mechanism: process death increments epoch
                 all caps for dead epoch: invalid
                 not rejected — unreachable
TTL construction: cap valid for N ms, must be reissued
                  replay window bounded by TTL
                  revocation implicit on expiry
```

### Type System Proof Contribution

```
C seL4:   capability is an integer
           type system cannot enforce rights
           runtime kernel rejection required

ferros:   capability is Cap<Rights, Epoch>
           rights violation = compile error
           wrong epoch = compile error
           type system IS part of the proof
           Verus proves the rest
           
Delta: type system ate a portion of the kernel proof obligation
       proof is smaller because Rust enforces it structurally

Vault objects additionally carry cryptographic access control:
  Capabilities gate the fast path (kernel enforced, in-memory)
  Crypto enforces the hard path (math enforced, survives physical access)
  See VAULT.md Access Control for the full dual-layer model
```

---

## Module 3: Ring Memory

**Built (2026-11-11, branch `arcs`, [ARCS.md](ARCS.md) steps 0-4; not yet run on a device):** one frame pool of the DRAM memblock proves free (husky: the two old pools and 9 GiB of the high bank; akita: the two old pools until its capture), in 2 MiB frames; every arc a window of virtual addresses whose frames lie anywhere in the pool; arcs grow and shrink on demand (SYS_ARC, ferros_rt's heap a ceiling), give frames back inside themselves, and move at any moment by copy and remap, held off the cores for it. Still one arc per process, no rotation yet (step 5), DMA pinned outside the arcs.

### Principle

No NULL. No MAX. No boundaries to overflow. The address space wraps by construction using two's complement arithmetic. Other process memory **does not exist** — not protected, not mapped, not addressable.

### Per-Process State

```rust
// One base-limit pair: a contiguous piece of the wrapping address space.
struct Arc {
    base_offset: usize,    // cryptographically random, continuously rotating
    limit: usize,          // span in bytes: a power of two under compiled masking (stock silicon), any size in Glyph silicon
}

// A process holds one or more arcs (ISOMEM: one or more base-limit pairs per process).
// Fragments are just more arcs; a hardware grant is an arc of registers.
struct ProcessMemory {
    arcs: Vec<Arc>,
}

// Bounds, per arc, compiled into the code (Execution Model): proven accesses carry no check,
// the rest are masked: base + (addr & mask), one AND, no branch.
// Glyph silicon: (ptr - offset) < limit in the load/store unit, zero instructions.
// No TLB required
// No page tables
// 5-10x faster context switch
```

### Properties

```
-1 is valid:         no NULL dereference class
Wraparound expected: no integer overflow class
No boundaries:       no buffer overflow class
Per-process offsets: other process memory not addressable
Continuous rotation: ROP impossible (addresses moving)
                     combined with cryptographic addressing: doubly impossible
```

### Interaction with Capability System

```
Ring memory:    isolation mechanism (memory)
Capabilities:   authorization mechanism (communication)
Relationship:   orthogonal — they do not interfere

Cap addresses kernel objects, not memory addresses
Arc offset rotation cannot invalidate a cap
This separation is architecturally load-bearing
```

### Formal Status

```
Theorem 2: RingMemoryIsolation — PROVABLE
  Pure Rust proof, no hardware assumptions required
  Two's complement wraparound: mathematical property
  Per-process offsets: distinct by construction
```

---

## Module 4: IPC

### Principle

Communication happens thru capability-addressed endpoints. PIDs do not exist in the IPC model. You cannot address a thread. You can only address an endpoint you hold a Send right to.

### Endpoint Model

```rust
struct Endpoint {
    // Kernel object
    // Addressable only via Cap<SendRight, E>
    // Existence: only in CSpaces that hold a cap to it
}

// Send: requires Cap<SendRight, E>
// Receive: requires Cap<RecvRight, E>  
// Cannot enumerate endpoints
// Cannot probe for endpoint existence
// Cannot guess endpoint address (cryptographic addressing)
```

### Message Passing

```
Call (atomic):
  1. Send message to endpoint
  2. Kernel issues single-use reply cap to callee
  3. Callee processes, sends reply via reply cap
  4. Reply cap ceases to exist
  5. Caller receives reply
  
No shared memory by default
Shared memory regions: explicit cap grant, both parties hold caps
No implicit sharing
```

### No PIDs

```
Unix IPC:   target identified by PID
            PID recycled after death
            TOCTOU race on PID reuse
            signal wrong process possible

ferros IPC: target identified by capability token
            token bound to epoch
            dead process → epoch incremented → token invalid
            pidfd problem: does not exist
            TOCTOU: does not exist
```

---

## Module 5: Branchless Security Path

### Principle

Conditional jumps are glitch targets. The security path contains **no conditional jumps**. Every check is arithmetic on masks. There is no jump to corrupt.

### Implementation Pattern

```rust
// WRONG: has jump target
fn check_capability(cap: &Cap) -> bool {
    if valid_chacha(cap) && valid_blake(cap) {
        return true;  // ← glitch target
    }
    false
}

// CORRECT: branchless, no jump exists
fn check_capability(cap: &Cap) -> Decision {
    let chacha_mask  = (valid_chacha(cap)  as u64).wrapping_neg();
    let blake_mask   = (valid_blake(cap)   as u64).wrapping_neg();
    let epoch_mask   = (valid_epoch(cap)   as u64).wrapping_neg();
    let nonce_mask   = (valid_nonce(cap)   as u64).wrapping_neg();
    let domain_mask  = (valid_domain(cap)  as u64).wrapping_neg();

    let combined = chacha_mask & blake_mask &
                   epoch_mask  & nonce_mask & domain_mask;

    // One assignment. No branches. Ever.
    Decision((APPROVE & combined) | (DENY & !combined))
}
```

### Glitch Resistance

```
Glitch corrupts mask value:
  combined → 0 → result = DENY
  still fail closed
  no jump was corrupted (no jump exists)

P(glitch produces APPROVE) = P(random bits pass all N checks)
                            = 2^(-256 × N)
For N=5: 2^-1280
```

### Verification Requirement

```
CI check: verify_branchless!() on all security path functions
Compiler output: reject any conditional branch in annotated paths
Verus annotation: prove constant-time execution
Assembly audit: security path assembly reviewed per change
```

---

## Module 6: Encrypted Kernel Memory

### Principle

Kernel memory is always ChaCha20 encrypted with a per-boot key. Plaintext kernel memory **does not exist** in RAM. A glitch that corrupts encrypted memory produces garbage that fails all validation checks.

### Key Properties

```
Per-boot key:  fresh each boot, in CSR only, never RAM
ChaCha20:      stream cipher, constant time, hardware accelerated
Garbage:       fails BLAKE3 check
BLAKE3 fail:   → DENY (fail closed)

Attack requirement:
  Must corrupt encrypted memory such that:
    ChaCha20 decryption produces valid plaintext AND
    BLAKE3 hash matches AND
    Capability token valid AND
    Epoch current AND
    Nonce unconsumed
  P(all pass) = 2^-256 per attempt
```

### Combined with Branchless Execution

```
No jump targets  +  no plaintext memory  =

Attacker cannot:
  Redirect execution (no jump targets)
  Corrupt meaningful data (only ciphertext in RAM)
  Forge valid check output (2^-256)
  Replay (nonce consumed)
  Use old caps (epoch bound)
```

---

## Module 7: Killswitch

### Hardware Path

```
Bistable relay: two stable states only {LIVE, DEAD}
LIVE → DEAD:   kill signal OR physical impact
Transition:    atomic at physics level (Maxwell's equations)
               no intermediate state exists
               
Kill sequence:
  Relay fires →
    Custom CSRs zeroed (keys gone) →
      Power cut →
        Done

Kernel executes: 0 instructions in this path
Software:        not in this path
Latency:         0ms by construction
```

### What Is Provable

```
Theorem 4: Killswitch_Safety
  Kernel not in path: proven by relay hardware spec
  Key erasure: proven by CSR hardware spec (read-disabled)
  Power cut: proven by relay physics
  Partial state: cannot exist (no software state written post-kill)

Theorem 5: Killswitch_Recovery
  Ring FS header ring: 10^6 valid headers
  BLAKE3 per header: corrupt header detected, skipped
  Mirror device: different manufacturer, independent failure
  Generation numbers: total ordering, u64 (584M years at 1000/s)
  Recovery: deterministic, always reaches consistent state
```

### Proof Tools Required

```
Concurrent separation logic (Iris):
  Kill relay and CPU execute truly concurrently
  No synchronization between them
  Rely-guarantee reasoning:
    CPU relies on: relay fires atomically
    Relay guarantees: atomicity, CSR zero, power cut
    CPU guarantees: no secrets in RAM
    Relay relies on: nothing in RAM needs protection post-kill

SAIL (RISC-V formal model):
  Prove behavior of every instruction at kill instant
  Kill fires between instruction N and N+1: proven safe for all N
```

### Fairphone 5 / Dev Target Gap

```
No bistable relay available
Partial mitigation:
  Secure world (TrustZone) holds keys
  Battery pull: mechanical, physical, achieves kill
  Kill latency: bounded microseconds (not zero)
  
Honest theorem on dev target:
  Killswitch_Safety: kill latency provably bounded at N μs
                     NOT: provably zero
  Gap: explicitly stated in proof, not hidden
```

---

## Module 8: Storage — Ring Filesystem Boot

### Recovery Algorithm

```
Boot sequence:
  0. Scan 1GB header ring (DMA, feeds crypto coprocessor directly)
  1. BLAKE3 each header whilst copying (parallel, not sequential)
  2. Collect valid set (hash matches = valid)
  3. Sort by generation number (u64, never wraps in deployment)
  4. Select max valid generation
  5. Load complete snapshot vector from that header
  6. Restore all process state
  7. Jump to entry points

If primary corrupt:
  Mirror has valid headers
  Higher valid generation wins
  Copy back to primary
  Re-sync
  
If copy-back interrupted by kill:
  Primary partial → BLAKE3 fails → skip
  Mirror intact → use mirror
  Next boot: copy from mirror again
  Always recoverable
```

### Header Structure

```rust
struct Header {
    generation:       u64,            // monotonic, total ordering
    arc_state:        ArcSnapshot,    // all process arc offsets
    capability_state: CSpaceSnapshot, // all valid caps at this instant
    entry_points:     Vec<VirtAddr>,  // per-process resume addresses
    storage_map:      RingFSMap,      // current block allocation state
    blake3:           [u8; 32],       // BLAKE3 over all fields above
}
// Corrupt any field → hash mismatch → header skipped
// Cannot forge valid header without knowing full state + key
```

### DMA Pipeline

```
Storage → DMA → [BLAKE3 coprocessor] → RAM
                       ↓
                  valid? → hand to process
                  invalid? → next header

Normal world never receives unverified data
Coprocessor is physically between storage and RAM
This is a topology guarantee, not a software guarantee
```

### Statistical Guarantees

```
P(single header corrupt) = p
P(all 10^6 headers corrupt) = p^1,000,000

For any physically realizable p:
  p^1,000,000 ≈ 0

This is not a confidence interval
This is the mathematics of independent probabilities
The dual-manufacturer mirror adds:
  P(both devices simultaneously corrupt all headers)
  = p_primary^1,000,000 × p_mirror^1,000,000
  ≈ 0 × 0
  = not a concern in any physically realizable scenario
```

---

## Module 9: Hardware Abstraction

### Scope

All `unsafe` code lives here. Nowhere else. Every unsafe block has:
- A documented proof obligation
- A stated hardware invariant it relies on
- A corresponding formal assumption in the proof

### Components

```rust
mod hardware {
    // Custom RISC-V CSRs
    // read-disabled key storage
    unsafe fn write_device_key(key: &[u8; 32]);
    unsafe fn use_key_for_operation(op: CryptoOp) -> CryptoResult;
    // No read. Hardware enforced.

    // Kill circuit interface
    unsafe fn arm_kill_circuit();
    // After this: software cannot prevent kill
    // Hardware takes over completely

    // Crypto coprocessor
    unsafe fn dma_to_coprocessor(src: StorageAddr, dst: SecureRAMAddr, len: usize);
    unsafe fn await_coprocessor_valid() -> BlakeResult;
    // Data accessible to normal world only after valid asserted

    // Interrupt delivery
    // Interrupts converted to capability notifications
    // Never delivered raw to a process
    unsafe fn deliver_interrupt_as_notification(irq: IrqNumber, cap: Cap<NotifyRight, E>);
}
```

---

## Boot Sequence

See **SEED.md** (trust anchor) and **BOOT.md** (kernel boot) for full specifications.

```
ABL → Seed (verify self, verify kernel, jump)     <1s
  → Kernel Boot Stage 0-8 (see BOOT.md)           <500ms
    → Running system with restored state

Total: <1.5s from power-on to the first process
       (excludes ABL nag screen — eliminated when locked)
```

---

## Development Target: Fairphone 5

### Proof Status on Dev Target

| Theorem | Glyph RISC-V | Fairphone 5 |
|---------|-------------|-------------|
| AddressSpaceConfinement | ✓ Proven | ✓ Proven |
| RingMemoryIsolation | ✓ Proven | ✓ Proven |
| CapabilityConfinement | ✓ Proven | ✓ Proven |
| Killswitch_Safety (0ms) | ✓ Proven | ✗ — no relay |
| Killswitch_Safety (bounded) | N/A | ✓ Battery pull |
| Killswitch_Recovery | ✓ Proven | ✓ Proven |
| StorageTotalRecovery | ✓ Proven | ✓ Proven |
| GlitchResistance | ✓ Proven | ✓ Proven |
| EpochSoundness | ✓ Proven | ✓ Proven |
| IPCIntegrity | ✓ Proven | ✓ Proven |
| DataIntegrity (topology) | ✓ Proven | ✓ (TZ assumption) |
| Key isolation | ✓ Proven | ✓ PAC registers + RPMB |
| Open silicon audit | ✓ | ✗ ARM closed |
| Side channels (EM/power) | ✓ PISPE | ✗ Not addressed |

### Key Storage on Dev Target (Fairphone 5 / QCM6490)

```
PAC Key Registers (ARM Pointer Authentication):
  5 × 128-bit registers = 640 bits total
  APIAKey_EL1, APIBKey_EL1, APDAKey_EL1, APDBKey_EL1, APGAKey_EL1
  Writable at EL1, never touch RAM or cache
  ferros repurposes for runtime crypto key storage
  PAC itself unnecessary — ring memory eliminates its use case

  Allocation:
    APIAKey + APIBKey = 256-bit ChaCha20 session key
    APDAKey           = 128-bit nonce/counter
    APDBKey + APGAKey = 256-bit derived key / temp material

RPMB (Replay Protected Memory Block):
  UFS hardware feature — write-authenticated with SHA256-HMAC
  Key provisioned once, cannot be read back
  Used for persistent secrets (device identity, key material)
  Production: RPMB
  Glyph target: BLAKE3 silicon oracle with optical link

TrustZone:
  QCM6490 TZ is Qualcomm-locked (QSEE/QHEE)
  Cannot load custom Trusted Applications without Qualcomm SDK
  Not usable for ferros key isolation on FP5
  Glyph target: own the secure world, formally verified
```

### TrustZone as Proof Asset (Glyph Target)

```
Android TrustZone: black box (OEM TEE)
                   unauditable, unprovable

ferros TrustZone:  you own the secure world (Glyph only)
                   no OEM TEE loaded
                   Rust code, formally verified
                   SMC interface formally specified
                   Key isolation: provable under ARM AArch64 spec

ARM Architecture Reference Manual:
  Extremely detailed, precise
  Not open silicon but formally documented
  SAIL has AArch64 models
  Hardware assumption much stronger than "trust the OEM"
```

---

## Implementation Scaffold

### Directory Structure

```
ferros/
├── kernel/
│   ├── src/
│   │   ├── main.rs                  # Entry point, boot sequence
│   │   ├── address_space/
│   │   │   ├── mod.rs               # Cryptographic address space
│   │   │   ├── key.rs               # Boot key derivation
│   │   │   └── navigation.rs        # BLAKE3 next-address computation
│   │   ├── capability/
│   │   │   ├── mod.rs               # Capability system core
│   │   │   ├── types.rs             # Cap<Rights, Epoch> type definitions
│   │   │   ├── cspace.rs            # CSpace management
│   │   │   ├── derivation.rs        # Capability minting and tree
│   │   │   └── revocation.rs        # Epoch-based revocation
│   │   ├── ring_memory/
│   │   │   ├── mod.rs               # Ring memory manager
│   │   │   ├── arc.rs               # Arc, bounds check
│   │   │   └── rotation.rs          # Continuous offset rotation
│   │   ├── ipc/
│   │   │   ├── mod.rs               # IPC primitive
│   │   │   ├── endpoint.rs          # Endpoint kernel object
│   │   │   └── reply.rs             # Single-use reply capabilities
│   │   ├── scheduler/
│   │   │   ├── mod.rs               # Thread scheduler
│   │   │   └── thread.rs            # Thread = CSpace + arcs
│   │   ├── security/
│   │   │   ├── mod.rs               # Branchless security path
│   │   │   ├── checks.rs            # Mask-based capability validation
│   │   │   └── crypto.rs            # Encrypted memory operations
│   │   └── hardware/                # ALL unsafe lives here
│   │       ├── mod.rs
│   │       ├── csr.rs               # Custom RISC-V CSR access
│   │       ├── kill.rs              # Kill circuit interface
│   │       ├── coprocessor.rs       # Crypto coprocessor DMA
│   │       └── interrupts.rs        # IRQ → capability notification
│   └── Cargo.toml
│
├── proof/
│   ├── spec/
│   │   ├── abstract.rs              # Layer 1: Abstract specification (Verus)
│   │   ├── design.rs                # Layer 2: Design specification (Verus)
│   │   └── hardware_assumptions.rs  # H1-H5: Formal hardware assumptions
│   ├── theorems/
│   │   ├── address_confinement.rs   # Theorem 1
│   │   ├── ring_isolation.rs        # Theorem 2
│   │   ├── capability_confinement.rs# Theorem 3
│   │   ├── kill_switch.rs           # Theorems 4-5 (Iris)
│   │   ├── storage_recovery.rs      # Theorem 6
│   │   ├── glitch_resistance.rs     # Theorem 7
│   │   ├── epoch_soundness.rs       # Theorem 8
│   │   ├── ipc_integrity.rs         # Theorem 9
│   │   └── data_integrity.rs        # Theorem 10
│   └── hardware/
│       ├── kill_circuit.sail        # Relay formal specification
│       └── coprocessor.sail         # Crypto coprocessor formal specification
│
├── processes/
│   ├── ring_fs/                     # Ring filesystem server
│   ├── token_auth/                  # TOKEN identity server
│   ├── vsf_compositor/              # VSF display compositor
│   ├── photon/                      # Photon messenger
│   └── toka/                        # toka VM
│
├── boot/
│   ├── header_ring.rs               # Ring filesystem boot scan
│   ├── state_restore.rs             # Process state restoration
│   └── root_cap.rs                  # Root capability bootstrap
│
└── target/
    └── riscv64gc-unknown-none-elf/  # Ferrocene-compiled binary
```

### Build Requirements

```toml
[toolchain]
channel = "nightly"          # Inline asm, custom CSR
components = ["rust-src"]

[target.riscv64gc-unknown-none-elf]
# Ferrocene for safety-qualified compilation (production)
# Standard nightly for development

[dependencies]
verus = "..."                # Formal verification
```

### First Implementation Steps

```
Week 1-2:  Read seL4 abstract spec (l4v/spec/abstract/)
           Understand proof methodology before writing code

Week 3-4:  Write ferros abstract spec in Verus
           Theorem 3 (killswitch) first — most novel, cleanest to state
           If you can state it formally, you understand the problem

Week 5-6:  Write Theorem 2 (ring memory) in Verus
           Pure mathematics, no hardware assumptions
           Easiest to close completely

Week 7-8:  Capability type definitions
           Cap<Rights, Epoch> — no implementation yet
           Just the types, prove type system properties

Week 9-10: Branchless check skeleton
           No capability lookup yet
           Just the mask arithmetic pattern
           Verify no branches in security path (CI check)

Week 11-12: Hardware abstraction stubs
            Define the unsafe boundary
            Document every proof obligation
            No implementation — just signatures and obligations

Week 13+:  Iterative implementation
           Each component proven before next begins
           Proof and implementation co-evolve
```

---

## Contribution Beyond seL4

ferros is not seL4 in Rust. The novel formal results are:

```
1. Cryptographic address space confinement
   — execution paths do not exist without the key
   — not a property seL4 proves or claims

2. Killswitch formal proof
   — seL4 proof assumes hardware runs
   — ferros proves properties when hardware stops
   — concurrent separation logic, SAIL hardware model

3. Branchless security path theorem
   — glitch attack surface provably zero
   — not addressed in seL4

4. Storage total recovery under arbitrary kill
   — probabilistic theorem (10^-1,000,000)
   — dual-device manufacturer-independent guarantee

5. Type system as proof contribution
   — Cap<Rights, Epoch> makes violations compile errors
   — type system ate part of the kernel proof obligation
   — proof is smaller because Rust enforces it structurally

6. Unified elimination model
   — every security property expressed as non-existence
   — not protection, not hardening, removal
   — consistent architectural philosophy thruout
```

---

## The Principle, Restated

```
∀ vulnerability class V traditionally mitigated by protection:
  ferros answer: V does not exist

Buffer overflow:    boundaries do not exist (ring memory)
NULL dereference:   NULL is just an address, class does not exist
ROP:                gadget addresses do not exist (cryptographic space)
Code injection:     target addresses do not exist
Glitch redirect:    valid targets do not exist (branchless + 2^-256)
Capability forgery: caps do not exist without derivation from root
PID confusion:      PIDs do not exist as an IPC primitive
Key extraction:     keys do not exist in RAM (CSR only)
Partial state:      partial states do not exist (ring FS + BLAKE3)
Stale caps:         caps for dead epochs do not exist

The attack surface that does not exist cannot be exploited.
In mathematics we trust.
```

---
