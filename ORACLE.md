# ORACLE — ferros Process Identity Architecture
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Identity is recognized, not asserted.

---

## 0. The Problem with Passwords

Passwords were invented by western computer science and immediately
thought to be improvable. Add complexity requirements. Add expiry.
Add recovery. Add two factors. Add hardware tokens on top of passwords.
Sixty years of piling assertions on top of assertions, and the
fundamental problem was never the password's strength — it was the
model.

Passwords ask: what do you know?
Capabilities ask: what do you have?

Both are assertion-based. The process claims something. The system
checks the claim. Identity flows outward from the claimant.

Every other human tradition arrived independently at a different
model. Not what you know. Not what you have. What you ARE —
recognized by the universe, not asserted to it.

That model has a name. Multiple names actually, from cultures that never
communicated with each other: mana, wakan, axé, orenda. All the same
insight. All describing force that flows thru an entity from its
nature and lineage, recognized by authority, impossible to forge.

Western CS? Missed it entirely.

---

## 1. The Unix Thread

Unix 1970 got one thing right: a file sitting on disk shouldn't
accidentally become runnable. You need to explicitly grant it. That
instinct — make someone look at what they're about to execute before
it runs — is the seed of everything that follows.

The execute bit is primitive. It's 1 bit. It answers "is this allowed
to run" but not "what is this," "who authorized it," or "is this
actually running right now rather than a replay." Linux spent fifty
years bolting on answers to those questions: capabilities, namespaces,
SELinux, AppArmor — each layer trying to compensate for what the
foundation didn't provide.

The oracle is where that thread was always going. Not a patch. The
logical conclusion.

---

## 2. Android ID and Google's Version

Android 8.0 accidentally built a deterministic oracle and called it
an ID.

Pre-8.0 ANDROID_ID was a device-wide identifier — same value for
every app on the device, trivially usable for cross-app tracking.
Google 'fixed' this in 8.0 by scoping it per signing key: apps signed
with different keys see different ANDROID_IDs on the same device.
Apps signed with the same key share one. The intent was logical —
prevent cross-app tracking while allowing same-developer app families
to coordinate.

What they accidentally built is not an ID. It is a deterministic oracle:
`f(device, signing_key) → stable_value`. That is the right model.
They named it wrong, documented the signing-key scoping poorly enough
that I spent days rediscovering it, and it stops well short
of what the model implies.

```
Android ID (8.0+):
  signing-key domain separation:   ish?
  named correctly:                 no
  hardware-bound:                  no
  survives factory reset:          no
  auditable hardness:              no
  owner sovereign:                 no
```

Google's interest was controlling tracking and funneling
authentication thru Google. The domain separation was a privacy
patch, not an architectural commitment.

ferros does the opposite of Google not to compete but because owner
sovereignty and Google's model are structurally incompatible. You
cannot serve both.

---

## 3. Ihi

The process has mana — what it provably is. The oracle produces **ihi**
— what others can recognize it to be.

In Māori understanding, mana is the force that flows thru an entity
from its nature and lineage — not possessed, not earned by choice,
not claimed. Recognized. Ihi is the specific quality of mana that is
outwardly perceptible: the awe-inspiring presence that stops others,
that can be witnessed and verified from outside.

This is not a metaphor borrowed for flavor. The oracle computes exactly
this. The process doesn't assert its identity. The kernel looks at
what the process provably is — binary content, signing authority,
hardware state — and derives what flows from that nature. The process
receives it at spawn. Other entities can verify it. It cannot be
faked.

The developer does not earn the ihi. They write the binary but
cannot control the blake3 output. They generate the keypair but
proper keygen means the entropy hands it to them, not the other way
around. The provenance flows from those.

The work shapes what you are. The math recognizes it. The process
receives it.

To forge ihi requires navigating a minimum of 10^792 paths thru
spaghettify. There are ~10^80 atoms in the observable universe.

---

## 4. Spawn

**Spawn** is the moment the kernel instantiates a process.

In ISOMEM terms: the kernel allocates a ring — an offset and extent
in the continuous two's complement address space, randomly positioned,
cryptographically isolated from every other ring. The process receives
this ring at spawn. It is that ring, for this instance, until it dies.

Spawn is not exec. Exec replaces a process image. Spawn creates a new
ring, a new capability space, a new identity. Nothing is inherited by
default. Everything must be explicitly granted.

At spawn, the oracle runs. The ihi is computed and handed to the
process. This is the only moment it can be computed — after signature
verification passes, after the ring is allocated, before the first
instruction executes.

---

## 5. The Oracle Function

```
ihi = oracle(system_persistent_secret || developer_pubkey || option)
```

**system_persistent_secret:** In a write-only CSR. Never in RAM. The
hardware physically prevents reading it back. It is fed into the
oracle by the kernel and exists in no addressable location at any
point. This is what makes ihi impossible to steal — there is
nothing to steal. No address. No bus access. No DMA path. Not even
the kernel can read it after writing.

**developer_pubkey:** Always present. Always load-bearing. But the
developer cannot feed this to the oracle directly. It only reaches
the oracle after surviving the gate — see section 6.

**option:** The semantic slot. The developer chooses what granularity
of identity they need, and the kernel constructs it from the capsule:

```
option = hp              → identity stable across versions
option = signature       → identity locked to this exact binary
option = ∅               → ecosystem-wide: all capsules by this
                           developer share a namespace
option = hp || signature → stable identity, version verification
```

Same function. Completely different trust boundaries. The kernel
enforces nothing about what goes in option — the semantics are the
developer's problem. The math treats them all identically.

### 5.0 Provenance Hash (hp)

```
hp = blake3(binary || eagle_time_nonce)
```

The Eagle Time nonce is not the boot time. Not a clock. Not anything
that reveals when this device was last restarted. It is a value
derived using Eagle Time — the monotonically increasing hydrogen-line
based time standard anchored to the Apollo 11 landing — used purely
for uniqueness. No timing information survives spaghettify. An
observer cannot correlate two ihi values to the same boot session
or determine anything about when the device was powered on.

Per-process uniqueness comes from a fresh Eagle Time nonce generated
at spawn and incorporated into option. Same capsule, same developer,
same device — two launches produce two completely different ihi.
Neither can impersonate the other. Session compromise cannot replay
into a future session.

hp is recommended for storage that should survive version updates. The
developer mints it once, keeps it stable. Their call.

---

## 6. The Gate

The oracle is unreachable without surviving signature verification.
This is load-bearing.

```
capsule loads
  → kernel verifies: blake3(binary) == declared hash
  → kernel verifies: ed25519(signature, hash, developer_pubkey)
  → both pass → kernel extracts developer_pubkey, feeds oracle
  → either fails → oracle never called, spawn never completes
```

You cannot feed the oracle a developer_pubkey directly. Userspace
cannot construct oracle inputs. The developer_pubkey only reaches
the oracle after the private key behind it has proven this exact
binary — meaning the person who holds the private key signed off on
this specific content.

To impersonate another developer's ihi you need their private
key. The public key is worthless without it. The oracle is behind a
door only a valid signature can open, and only the kernel holds that
door.

The oracle is not just a function. It is a function behind a door
that only opens when the math says it should.

---

## 7. Why Spaghettify

BLAKE3 is the right tool for content addressing thruout ferros.
Fast, well-studied, correct for ledger entries, capsule hashes, spine
records. Its preimage resistance is conjectured — "we believe this
permutation is hard to invert." That belief is well-founded. Decades
of cryptanalysis support it.

For the oracle, conjecture is not enough. The oracle is responsibility
#0. Everything else stands on it. If it fails, nothing above it means
anything.

Spaghettify's hardness is auditable:

```
53 buckets. 23 operations per bucket per round. 11-23 rounds.

Minimum paths to invert: 23^(53×11) ≈ 10^792
Maximum paths to invert: 23^(53×23) ≈ 10^1656
Atoms in observable universe: ~10^80
```

You don't need to believe inversion is hard. Count the paths. The
operations are listed. The collision ratios are measured. count_ones
alone produces 10^75:1 collisions — that's not a claim, that's
arithmetic.

The math does not care whether you trust it. It is what it is.

Speed: spaghettify runs in ~1ms. App launch takes 50-200ms. The
oracle overhead is unmeasurable in practice. Provable hardness costs
nothing perceptible.

See SPAGHETTIFY.md for full specification and path explosion proof.

---

## 8. Hardware Substrate

The system_persistent_secret is a register. Not a memory region.
Not an encrypted page. Not a secure world allocation. A CPU register
that hardware physically prevents reading.

```
Write it in:   possible
Read it back:  hardware refuses
Bus access:    no — CSRs are not on the memory bus
DMA:           no — not an addressable location
Debug:         no — not a debug register
sudo:          irrelevant — there is no address to read from
```

Every prior enclave design stores secrets somewhere addressable:
SGX in encrypted RAM, TrustZone in secure world memory, TPM across
a bus, AMD SEV in encrypted pages. All of them have surfaces to
attack. Cold boot, power analysis, bus interception, memory forensics.

ferros oracle has none of these surfaces because the secret has no
address. The ihi is computed transiently and handed to the
process. After spawn, it exists only in the process's ring — which
is itself isolated by ISOMEM. The computation is gone. The input is
gone. What remains is what was always going to be recognized.

PIPE (Physically Isolated Processing Enclave) is the ideal substrate
— independent power domain, async clock, physical isolation from the
main die (photonic, galvanic, inductive, or capacitive). The oracle
runs correctly on any hardware with write-only key registers. PIPE
makes the guarantees physically unbreakable rather than merely very
strong.

---

## 9. Kernel Responsibility 0

```
0. Oracle:      hardware-bound process identity — ihi
1. Memory:      ring allocation, grants, bounds enforcement (ISOMEM)
2. Scheduling:  time slicing, IPC dispatch
3. IPC:         capability-gated message passing
4. Boot:        seed verification, vault root scan, state restore
5. Hardware:    interrupt routing to userspace drivers
```

Oracle is #0 because without it, nothing above it is meaningful.
Memory grants are only meaningful if the process holding them is
provably what it claims to be. Capability tokens are only meaningful
if they cannot be forged by impersonating a verified identity. The
boot chain is only meaningful if processes cannot claim to be the
kernel.

Mana is the ground. Ihi is what the oracle makes visible. Everything
else is built on it.

---

## 10. Content and Licensing

The oracle solves a problem that has defeated the content industry
for thirty years: cryptographic copy prevention without a central
authority.

Traditional DRM's fundamental failure: to display content, bits must
exist in memory. Bits in memory can be copied. The key is extractable.
Every DRM scheme is eventually broken because the secret lives
somewhere addressable.

With ihi:

```
content_key = oracle(ihi || content_id)
```

The content is licensed to what the process IS, not to something
it holds. To decrypt, you need ihi. To get ihi, you need
to be the verified process on this hardware. You cannot copy ihi
— it is computed from a secret that has no address, behind a
gate that requires a valid signature from a private key you don't
have.

The analog hole still exists at the output stage. But the
cryptographic copy problem — extracting a key and using it
elsewhere — is closed. Not by legal threat. By physics.

Owner sovereignty and content licensing are compatible because the
owner is the root of trust. Replace the developer key with your own,
sign your own kernel, derive your own ihi. Content licensed to
your ihi follows you thru your own modifications. Content
licensed to the developer's chain requires the developer's chain —
modify the kernel without signing it properly, that content becomes
inaccessible, which is correct behavior.

---

## 11. Formal Claims

**Claim 0:** A method for hardware-bound process identity comprising:

A system_persistent_secret residing exclusively in a write-only
hardware register, never written to addressable memory, volatile or
otherwise, not accessible via memory bus, DMA, or debug interfaces.

An oracle function producing ihi:

```
oracle(system_persistent_secret || developer_pubkey || option)
  → ihi
```

Where developer_pubkey is provided exclusively by the kernel following
successful capsule signature verification — it cannot be supplied
by userspace under any condition — and option is a developer-controlled
semantic slot determining identity granularity: version-stable via
provenance hash, version-locked via signature, or ecosystem-wide via
omission.

Hardness derived from provably exponential path explosion
(23^(53×R), minimum 10^792 paths) via transparent combinatorial
construction auditable by inspection, not conjectured primitive
security.

Unlike prior art wherein secrets reside in volatile or addressable
memory, this method maintains no secrets in any addressable location
at any point. Unlike prior art wherein identity is asserted by the
process, this method computes identity from what the process provably
is and delivers it to the process at spawn. The process receives its
identity. It does not claim it.

---

## 12. Document Map

```
ORACLE.md           this document
SPAGHETTIFY.md      chaos amplifier, path explosion proof
ARCHITECTURE.md     six kernel responsibilities, structural elimination
SECURITY_CHAIN.md   boot trust model, signature chain, owner sovereignty
ISOMEM.md           ring memory, offset/extent isolation (forthcoming)
PIPE.md             physically isolated enclave substrate
```

---

**Status:** Specification complete. Implementation: kernel spawn path.
**Prior Art:** Established. See README.md publication date.
**Patent:** Provisional pending (combined ISOMEM + PIPE + ORACLE).
**Author:** Nick Spiker
**Contact:** fractaldecoder@proton.me

---