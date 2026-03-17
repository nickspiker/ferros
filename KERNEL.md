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
│                 USERSPACE SERVERS                      │
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
× Anything that belongs in userspace
```

---

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
```

---

## Module 3: Ring Memory

### Principle

No NULL. No MAX. No boundaries to overflow. The address space wraps by construction using two's complement arithmetic. Other process memory **does not exist** — not protected, not mapped, not addressable.

### Per-Process State

```rust
struct RingRegion {
    base_offset: usize,    // cryptographically random, continuously rotating
    limit: usize,          // maximum addressable span
}

// Bounds check: 2 instructions
// (ptr - offset) < limit
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
Ring offset rotation cannot invalidate a cap
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
    ring_state:       RingSnapshot,   // all process ring offsets
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
    // Never delivered raw to userspace
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

Total: <1.5s from power-on to userspace
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
│   │   │   ├── region.rs            # RingRegion, bounds check
│   │   │   └── rotation.rs          # Continuous offset rotation
│   │   ├── ipc/
│   │   │   ├── mod.rs               # IPC primitive
│   │   │   ├── endpoint.rs          # Endpoint kernel object
│   │   │   └── reply.rs             # Single-use reply capabilities
│   │   ├── scheduler/
│   │   │   ├── mod.rs               # Thread scheduler
│   │   │   └── thread.rs            # Thread = CSpace + RingRegion
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
├── userspace/
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

*ferros 0.0 — Built by engineers who believe sovereignty is a choice, not a privilege.*  
*First published: 2025 — Author: Nick Spiker*