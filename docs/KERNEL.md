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

### Install, compile, seal, go

1. **Install:** verify the capsule — the developer's ed25519 signature over its BLAKE3 hash ([ORACLE.md](ORACLE.md)).
2. **Compile once, on device:** toka → aarch64. Accesses the compiler proves in range carry no check (a row is carved once; the loop inside it is proven). The rest are masked, `base + (addr & mask)`: one AND, no branch, and no speculative out-of-bounds read.
3. **Seal:** the native code is stored in the vault, authenticated by a keyed BLAKE3 MAC under an ira-derived key, bound to (bytecode hash, compiler hash). Only this device ever verifies it, so a public-key signature buys nothing; a compiler update invalidates every seal and code is recompiled. The seal is not an identity: the process's *ihi* still comes from the developer's capsule.
4. **Go:** load the arc base and mask registers, set the entry point, `eret` to EL0. Switching processes is switching those registers: no table switch, no TLB, no flush.

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
| 3 Arcs | power-of-two buddy allocator over the arc pool (cold boot: 8 MiB at G#8100_0000, inside the kernel window), zeroed on allocation; the static map lets EL0 touch only the pool. Not yet: compiled masking (processes are hand-written native code; the toka backend is what will mask) |
| 4 IPC | call / recv / reply, synchronous rendezvous, messages copied arc to arc, reply tokens single-use; named dev endpoints granted at spawn |
| 5 Branchless security path | only the token lookup is branch-free |
| 6 Encrypted kernel memory | not started |
| 7 Killswitch | the key presses are the interrupt (KILLSWITCH.md) |
| 8 Storage | the ira-rooted vault (VAULT.md) |
| Oracle / spawn (ORACLE.md) | signed VSF capsules through the gate (Ed25519 over the whole file, vsf crypto-verify), ihi = spaghettify(system secret ‖ developer key ‖ hp) written into the arc; the system secret is an ira-derived RAM stand-in for the write-only CSR |

The process ABI (`ferros_kernel/src/process.rs`):
- **Entry:** EL0 (VHE: EL2 host, EL2&0 regime) at arc offset G#1000, x0 = grants offset, x1 = input offset, x2 = input length, **x28 = arc base, x27 = arc mask**, every other register (general and SIMD) zero, interrupts unmasked.
- **Arc layout:** G#0 grants page — u32 count, then 32-byte tokens (slot 0 = console, then the endpoint grants in order); the *ihi* at G#FE0 (32 bytes); code at G#1000; input after the code; stack at the top.
- **Syscalls** (`svc #0`, number in x8, every offset an arc offset the kernel masks, ERR = all ones): 0 `exit(code)`; 1 `call(cap, msg, len, reply, reply_cap)`; 2 `recv(cap, buf, buf_cap, reply_token, flags)` (flags bit 0 = poll: NOTHING at once if no message is queued); 3 `reply(reply_token, msg, len)`; 4 `mmio_read(grant, off)` / 5 `mmio_write(grant, off, value)` on a hardware grant; 6 `entropy(cap, buf, len)` — up to 4096 bytes BLAKE3-keyed from the session's wairua over a kernel counter (no wairua, no bytes); 7 `dma(cap)` → the physical base of a DMA region grant; 8 `input(start, buf, len)` streams spawn input; 9 `sleep(deadline)` parks the process until CNTPCT reaches the deadline (a periodic process must not spin: the service loop hands slices out on every idle pass, and a spinning process slows PT transfers until the host gives up — measured 2026-10-16; the loop also withholds slices while a transfer is open). 13 `store(cap, flags, key, key_len, buf)` on a Store grant (a scope of the device vault, keys relative to it, up to 96 printable bytes — a vault's prefix and a 32-byte address in hex): flags bit 0 set, bit 1 a piece at the offset in bits 32-63 (a read of a large value, or with bit 0 a write staged until the piece without bit 2 commits it), bit 3 delete, bit 4 list the scope's entries (read like a large value: names one per line, or with bit 5 the addresses, 32 bytes each), bit 5 the entry is kept by a 32-byte address at x2 instead of a name (kete's vaults: the address is the app's, the kernel scopes it through its keyed hash); the length in bits 8-31, at most 4096 a transfer.
- **Grants** are capabilities like any other (`bridge spawn --serve/--use E`, `--hw BLOCK`, `--entropy`, `--dma REGION`). A partition grant (`--part NAME`, process.rs `PARTS`; since 2026-10-29) reads files on one of the phone's own partitions and never writes them: the modem driver's firmware image, NV and carrier configs (partfs.rs `part_call`: [1] path → size, [2] offset path → up to 64 KiB, the last file kept open). A DMA region is a 2 MiB block of non-cacheable memory (cachemap DMA_POOL) the driver reads and writes directly; its device can DMA only there and into ferros's window (`s2mpu.rs`, the HSI2 S2MPU fence), never anywhere else in DRAM.
- **The saved frame** (816 bytes, `process::Frame`, built by `.Lel0_sync`/`.Lel0_preempt`): x0–x30, SP_EL0, ELR, SPSR, ESR, FAR, then **v0–v31, FPCR, FPSR** — the vector state since 2026-10-17. Before that no FP state was saved across a syscall or a slice tick, and the kernel's own memcpy uses the vector registers, so every f32 a process carried past a trap came back 0 (fontdue mostly survived in short bursts; a full-screen f32 render didn't). `payloads/fptest` is the regression check: runtime arithmetic, `fsqrt`, and a 4 s run across slices and syscalls counting corruptions. The kernel's own d8–d15 are saved around a process run too (`__el0_kctx`).
- **Faults** end the process (exit value FAULT | EC | PC | FAR), never the kernel. Every process's end is also written to the console (`name: exit G#…`), and a fault first prints the whole state — ESR, ELR / SP as arc offsets, FAR, x0..x30 — so it can be debugged from the console alone.
- **x27 and x28 are reserved in codegen** (`.cargo/config.toml`: `+reserve-x27,+reserve-x28` for aarch64-unknown-none). The ABI promises the arc base and mask are never written by compiled code; until 2026-10-15 nothing enforced it, rustc used x28 as a scratch register, and a process reading its base mid-call (the `ferros_sys` shim does, for every syscall buffer) got garbage — a fault report showed x28 = G#8141A748, not an aligned arc base. rustc flags the feature as unstable but passes it to LLVM; check with `objdump -d | grep -E "x2[78]"` (reads only). The precompiled core/compiler_builtins save and restore them as callee-saved.
- **Trust model A (owner, 2026-10-15):** ferros compiles everything it runs; it never trusts precompiled code. The toka bytecode is WASM forked and extended at both ends of the rustc/LLVM pipe — native i128/u128, 64-bit addresses, then spirix Scalar/Circle as value types — and the on-device compiler does the masking (so no off-device masking pass or verifier). Native `aarch64` capsule sections are a dev-only bring-up path; a production build refuses them.
- **How masking stays sound (LFI / Native Client style invariant):** x28 = arc base and x27 = mask are never written; one dedicated address register (x26) may only be written by `and x26, <any>, x27`, so it is inside the arc at *every* instruction; every load/store addresses `[x28, x26]` (or the stack, whose pointer is re-masked on every write); no system instruction but `svc`. Then a jump can land anywhere and still not escape, and checking code is a linear scan of a few hundred lines. The on-device toka compiler emits this pattern by construction (the scan stays as a second check on its output); native dev capsules don't follow it yet (an LLVM pass in the ferros Rust target + the scan in the gate would close that before toka exists).
- **`ferros_sys`** is the only crate that touches the machine (svc, the timer, the arc base, the DMA barrier); payloads use it, so moving to the one backend is a retarget.
- **Build order (decided with the owner 2026-10-15):** drivers and stacks stay plain no_std Rust compiled natively (capsule section `aarch64`) through hardware bring-up and the first Photon beam; everything touching the machine goes through one shared syscall shim (svc, timer, arc base), so the move to the one backend (Rust → WASM → toka → native with compiled masking) is a retarget, not a rewrite — and the working native drivers become the backend's first test inputs. Native payloads' two gaps until then: no compiled masking (they can reach the whole arc pool) and no relocation (below).
- **Native payloads run unrelocated at their arc base** — unless linked `-pie` and relocated at entry, which `payloads/display` does (its `payload.ld` keeps `.rela.dyn`, its `build.rs` passes `-pie --no-dynamic-linker -z norelro -z notext`, and `ferros_sys::relocate()` applies the R_AARCH64_RELATIVE entries before anything else runs; fontdue's `dyn` vtable was the first absolute address to bite, 2026-10-16). Any payload pulling in real crates should link that way. Otherwise: code must not dereference absolute pointers baked into data — e.g. a Rust `match` that yields string slices can compile to a pointer table in `.rodata` (seen 2026-10-15: data abort at FAR G#…9927 in the WiFi driver). Branch to direct uses instead; `objdump -d | grep "lsl #3]"` finds table loads. (toka's one backend removes the problem.)
- **CALL_LIVE** (bit 63 of a call's reply length; since 2026-10-29): the call fails with ERR at once, rather than waiting, when no server is receiving on the endpoint — none holds the right, or the one that holds it has not received since it started — and a queued CALL_LIVE caller gets ERR when the endpoint's last server ends. Without the bit a call waits for a server, as across a restart. The IP process calls its links (`wlan`, `rmnet`) with it, so a WiFi driver still bringing its device up, or none at all, leaves it on cellular instead of hung.
- **Time slices:** 10 ms on the EL2 timer; a preempted process continues in later slices from the service loop.
- **Boot:** after the vault opens, the kernel spawns the vault's `init` capsule — ferros's first process.
- **Dev:** `bridge capsule` signs a binary with the developer key, `bridge spawn` runs one (with `--serve` / `--use` endpoint grants), `bridge diag console` reads what processes said.

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

- **Direct hand-off both ways.** A `call` to a server waiting in `recv` switches straight into it (built). A `reply` switches straight back to the caller; the server goes to the back of the queue (to build — today the caller queues and the server runs on, which is the wrong priority: the one who waited goes first).
- **A sleep whose deadline has passed is a yield**: anyone runnable goes first, the sleeper comes back round (built 2026-10-23). A server's end-of-pass sleep therefore always gives the core up, whatever its bookkeeping did to the deadline.
- **The slice timer is a backstop**, never the pacing. If a trace shows round trips quantised to SLICE_MS, something is spinning; find it, do not shorten the slice.
- **The kernel's service loop is the idle task.** It runs when nothing is runnable (and between slices only while a PT transfer is open, which it must pace). Nothing runnable means WFI on the event stream, 333 µs ticks (built 2026-10-23; before it the shell kept the X3 at 2.9 GHz for as long as ferros was up).

### Threads

A **thread** is a saved frame, a stack and a scheduling state, belonging to a process; the process is the arc, the capability space, the name and the *ihi*. Threads of one process share the arc and every capability in it (KERNEL.md Execution Model: "each with its own stack, so tokio and rayon get real threads"). The run queue holds threads. `call`, `recv`, `reply`, `sleep` already act on "the current runnable thing"; most of the scheduler is a rename.

- **Syscalls added:** `thread_spawn(entry, stack_offset, arg)` and `thread_exit(value)`; `wait(word_offset, expected)` and `wake(word_offset, count)` on a word in the arc — a futex, so threads of one process hand work to each other without IPC. Every offset is an arc offset the kernel masks, as for every other syscall.
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

A **grant of an interrupt** is a capability object like a hardware grant: `Grant::Irq(intid)`, minted at spawn. `irq_wait(cap)` blocks the thread until the line fires; the kernel masks the line at the GIC and wakes the waiter; the driver handles its device and calls `irq_ack(cap)`, which unmasks it. Level and edge both work this way (the line stays masked until acked). The kernel's own interrupts stay its own: the two volume keys are the killswitch and never reach a process, the slice timer and the power key stay in the kernel. First lines: the FTM5 touch INT, DECON frame-done and TE (so the render thread is paced by the panel, not by a 0.5 ms sleep loop), DSIM, and the sensors as they come. USB stays with the kernel's service loop until that loop itself becomes a process.

### Cores

husky has 1× Cortex-X3, 4× A715, 4× A510 (ASID.md). The kernel today is one core's worth of code: `k()` is a bare global, the run queue is one, there are no locks, and the one core is the X3 (previous section). Going to eight:

- **Per-core state**: a run queue, `current`, the idle loop and the slice timer per core. The kernel's shared state (processes, endpoints, capabilities, the trace) sits behind **one kernel lock** — a spinlock taken for the microseconds a trap holds it. The kernel's paths are short enough that one lock is right at this scale; splitting it per object is an optimisation with a proof cost, later if ever.
- **A thread runs on the core that woke it** unless it is pinned: a `call` into a server waiting on this core switches directly, as now; into a server waiting on another core, the caller blocks and the callee is queued there with an SGI to wake that core if it sleeps. A reply likewise. Migration is explicit (a pin), never a balancer's guess.
- **Pinning by latency class, not "UI core vs OS core."** X3: the display server's render thread (the one real compute path: discs, text, effects, the NEON publish). A715s: app processes, and render workers when a frame is tiled across them. A510s: everything that only waits — the service loop and USB, touch, sensors, the always-on display — at their lowest DVFS step, so the big cores sleep whenever nothing is runnable on them. The kernel's own path runs on whatever core trapped; it has no home.
- **Bring-up**: PSCI CPU_ON per core with the hand-over block generalised to a slot per core (the X3 hop is the first instance: the same registers, its own stack); each core initialises its own redistributor, enables its slice-timer PPI and event stream, and parks in WFI until its queue gets a thread. CPU0 may then be a worker like the others instead of parking for good.
- **Memory**: kernel structures are Normal cacheable inner-shareable, so the cores are coherent in hardware; the non-cacheable regions (DMA, the phase ring, the hand-over page) stay as they are. Processes are isolated by compiled masking regardless of core (Execution Model).
- **Power**: an idle core is in WFI; a cluster with nothing runnable drops its DVFS step through ACPM (`dvfs_set` exists per domain); the watchdogs stay petted by the service loop wherever it runs.
- **Proof**: the single-core theorems carry over per core; the new obligation is the kernel lock — every access to shared state under it, no path that sleeps or waits for hardware while holding it. That is the same rule as for servers, one level down.

### Order

1. Reply hand-off and the past-deadline yield — built 2026-10-23.
2. Threads: `thread_spawn`/`thread_exit`, `wait`/`wake` (with a deadline), the run queue of threads — built 2026-10-23 (`payloads/threads` is the test; `diag threads` lists every thread and its state).
3. The display server split into receive, render, vsync and panel threads (a request ring from receive to render; a map call costs 5 µs with a frame in flight; the panel thread takes the DSI writes and the 120 ms sleep-out, under one futex mutex shared with the dev DSI ops) — built 2026-10-23.
4. Interrupt grants: `irq_wait`/`irq_ack`, the `IRQS` registry (touch INT gpn0-0 = SPI 0; DECON0 frame-start 243, frame-done 242) — built 2026-10-23: the touch server waits on its line, the display's vsync thread counts frame starts and `settle`/`present` wait for them. Two lessons: DECON comes up with every interrupt source enabled and frame-done pending, so a handler must clear everything pending or the line never drops (162,000 wakes a second); and a line is armed at the first `irq_wait`, not at grant, since init holds every grant and waits on none. TE is on a shared PERIC0 line (SPI 634, gpp0-3) and is not granted yet.
5. Cores — built 2026-10-23, the first cut: one kernel lock over the shared tables (`process.rs`: every `pub fn` takes it, nothing holds it across a wait or a run at EL0), the saved kernel context per core, the four A715s brought up from the kernel's core through their own hand-over blocks (`smp.rs`, the X3's entry generalised) into an idle loop — a slice to whatever is runnable, `wfe` otherwise, every enqueue a `sev`; a lock-free count of the queue so an idle core looks before it locks. One global run queue rather than per-core ones, and `sev` rather than SGIs: both hold up at this scale and are simpler to reason about; revisit when the trace says so. A RELOAD switches the workers off (PSCI CPU_OFF) before the copy stub runs. Processes ended on one core while a thread runs on another are drained at that thread's next trap (`dying`). Lessons: TLB invalidation of pool pages must be inner shareable (`tlbi vae1is`) or a worker fetches through its stale entry and takes a permission fault at a new process's entry; and a kernel that is per-core anywhere (`__el0_kctx`) must be per-core everywhere.
   Idle workers (2026-10-23, later the same day): a fruitless look sleeps — with the render thread pinned to the X3 and queued whenever the X3 was busy, `RUNNABLE_N` read 1 forever and the four workers spun `run_slice → nothing I may run → look again` at 7,000 contended lock acquisitions a second; a kick for a thread wakes only the cores that may run it (`kick(mask)`, `sev` for the X3 and SGI 1 for the workers); the workers' slice-timer PPI and SGI 1 are enabled per redistributor (`killswitch::arm_core`). Contention now ~270/s, a worker wakes about once a second at rest. A `wfi` idle (the EL2 timer armed to the next deadline, woken by SGI 1, IRQs masked across the look so a kick before the `wfi` stays pending) is written behind `FERROS_IDLE_WFI=1` and **shelved**: it hangs non-deterministically on a reload (one reload fine, the next dead at boot with an "APC Watchdog Early"), and every post-mortem channel tried was overwritten by a boot-time loader (the ramoops page by the gate, the X3 result page by a running log, G#80F0_0000 by the 17 MB gate image, G#C1FF_0000 by the modem firmware in cp_rmem — which also means `APP_SURFACE` sits in modem memory and must move before cellular). Notes now live at G#B000_0000 (`platform::note`, `NOTE_n`/`PREV_NOTE_n` in DIAG), untested across a hang yet. `stop_all` sends SGI 1 and waits on PSCI AFFINITY_INFO per worker before a reload rewrites the kernel (two workers once stayed on and came back ALREADY_ON).
   **The eret race (found and fixed 2026-10-23 evening).** `ferros_el0_resume` ran with interrupts enabled (it is called from the idle loops), and `.Lel0_restore` loads ELR_EL2 and SPSR_EL2 from the frame some thirty instructions before its `eret`. An interrupt in that window — SGI kicks arrive at kilohertz once the shell moves — made the exception entry overwrite ELR/SPSR with the `eret`'s own address and EL2h; the handler returned to the `eret`, and the `eret` returned to itself, forever: a core parked on one instruction with interrupts open, no slice ticks, no traps, its thread "running" to the scheduler (the touch, map and vsync threads in turn; the map's calls to a parked touch server then froze the shell, and when the X3 itself hit it the watchdog reset the phone). The trap-return path was immune because exception entry masks IRQs; the resume and enter paths now mask them first. Found by sending a core an SGI whose handler dumps its ELR/SPSR/SP (`diag core <k>`) — the stuck core's PC was the `eret`, its SPSR EL2h — after the per-core crumbs (`NOTE_k`), the IRQ-handler stages (`IRQ_STAGE_Ck`) and the GIC active/pending words had ruled out everything else. Lesson: anything that loads ELR/SPSR for an `eret` runs with interrupts masked, no exceptions.
   **The lock's rules (2026-10-23, after two freezes).** (1) Nothing that runs with the lock held — every syscall, and anything inside `with_lock` — calls a function that takes it: those have `_held`/`_locked` variants (`alive_locked`, `set_pref_held`, `set_wall_held`); the facts provider called `alive()` from inside SYS_FACTS and the core waited on itself, freezing the phone whenever settings were open. (2) Nothing that can block on hardware runs with the lock held: the facts provider read the battery gauge over I2C inside the syscall, on a worker core; the X3 now samples it in the service loop and facts reads the cache. (3) The vault is part of the locked state: syscalls reach it holding the lock (the map's places, a setting, a spawn's capsule), and the service loop takes the lock around its own vault work (`with_lock` around the store command, the key count, a restart's reads) — one owner at a time, and with one lock there is no order to get wrong. A 2.7 MB put now holds the lock for its duration; the shell stalls for that time, which only happens on a deploy.
6. Pinning — built 2026-10-23: a per-thread core mask (`pin`, syscall 20; the workers by default, inherited by spawned threads), a core takes only threads allowed on it, with a fallback when none of a mask's cores is up. The display's render thread pins itself to the X3; receive, vsync, the map and the touch server stay on the workers, so a call hands off on the caller's own core. Unpinned, the drag fell to 70-81 frames/s (the render thread wandering between A715s); pinned, 114. Still to come: tiled rendering across the workers, the A510s for the waiting servers, `diag sched` has its core column.

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