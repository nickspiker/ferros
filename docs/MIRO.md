# MIRO — Reversible Avalanching Permutation with a Tunable Secret/Check Split
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** One cord, twisted from two strands and you choose the lay.

---

## What MIRO Is

MIRO is a single primitive that seals a payload into an opaque codeword and opens it back.
It is built from a **reversible avalanching permutation** (MIRO-P) plus one allocation decision: how the fixed-width block is split between *secret you can't extract* and *redundancy you can't fake*.

It exists because of a wall we hit head-on: **you cannot have one function that is reversible, avalanching, and self-verifying all at once** — the first and the third are opposite ends of a single axis (see [The Conservation Law](#the-conservation-law)).
So MIRO does not pretend to be that impossible function.
It is a reversible permutation, and integrity is bought — honestly, in bits — by spending part of the block on a check field.

Distinct from the other secret-handling layers, and complementary:
- **[KEY_REGISTERS.md](KEY_REGISTERS.md)** — live secrets in the register file (detect-and-freeze). MIRO-P's 512-bit state is exactly 4 NEON V-registers, so seal/open can run inside that layer's pinned-asm critical section.
- **[VAULT.md](VAULT.md)** — at-rest object store (`manifestus`). MIRO is the per-object seal, not the store.
- **[PIPE.md](PIPE.md)** — MIRO is a message transform (PT L4-adjacent), medium-agnostic like the rest of the stack.

---

## The Conservation Law

Store an n-bit codeword whose secret domain carries k bits of entropy.
Then r = n − k bits are redundancy, and **everything is denominated in that one split**:

| Quantity | Cost | Drawn from |
|---|---|---|
| **Invert** — recover the secret | 2^k tries | the entropy strand, k |
| **Forge** — fabricate a valid codeword | 2^r tries | the redundancy strand, r |
| **Detection miss** — corruption slips through | 2^−r | the same redundancy strand, r |

n = k + r. One pie.
Every stored bit is either entropy (making inversion hard) or redundancy (making forgery hard *and* detection tight), and the two trade against each other exactly within fixed width.
Reversibility and redundancy are not two knobs — they are the *same* knob seen from opposite ends.

There is a genuinely independent third dial: **per-operation cost, c** — how expensive each try is (round count, or iterated re-permutation).
It draws on compute-time, not stored bits.
Speeding up inversion lowers c; it does **not** mint redundancy — that would violate n = k + r.
So a faster inverse buys cheaper opens or more rounds, never free integrity.

---

## MIRO-P — the permutation

MIRO-P is **BLAKE3's mixing with the feed-forward removed.**
A hash is a permutation plus a deliberate lossy step; delete the lossy step and you are left with the bijection.
This is not a new permutation — it is a well-understood one, run in both directions.

- **State:** 16 × u32 = 512 bits, the 4×4 matrix BLAKE3 uses. Exactly 4 NEON V-registers.
- **Round:** BLAKE3's round verbatim — 4 column-G then 4 diagonal-G.
- **G:** BLAKE3's mixing function. Every operation is a bijection given the round constants — `wrapping_add` undoes by `wrapping_sub`, `^` is self-inverse, `rotate_right` pairs with `rotate_left`. So `G⁻¹` is G's lines in reverse with add→sub and rotations flipped, same instruction count both ways.
- **Constants:** BLAKE3 feeds *data* into G's two message slots. MIRO-P feeds *fixed nothing-up-sleeve constants* (prototype uses the first 16 SHA-256 K words), so G is a fixed permutation, not a compression step. A future key can enter these slots for a cipher mode (Even–Mansour style); v0 is unkeyed.
- **Rounds:** default **24**. BLAKE3 runs 7 inside hash mode, where the feed-forward and mode provide one-wayness. A *bare* permutation exposed to adversarial inversion has no such cover, so it needs margin — and margin is nearly free (see perf).

## Measured (not asserted)

From the prototype harness [ferros_miro/src/main.rs](ferros_miro/src/main.rs), x86-64, `opt-level=3` + LTO, scalar (no SIMD):

- **Round-trip:** `permute ∘ permute_inv == identity` over A#200000 random states × {1,7,8,16,24} rounds — **all identity.** Invertibility is mechanical and verified.
- **Avalanche:** one input bit flipped diffuses to the ideal **50.0% of the 512 output bits by round 2** (A#37.1% after 1 round). Full statistical diffusion is immediate.
- **Detection:** A#20000/20000 clean opens recovered; A#20000/20000 single-bit codeword flips rejected.
- **Perf:** permute = A#78.7 / A#155.5 / A#231.3 ns at 8 / 16 / 24 rounds; seal+open = A#524 ns at 16 rounds (a full 32-byte shard sealed and re-opened — two permutes plus two check derivations). At the default 24 rounds expect roughly A#700 ns; still trivial for a once-per-recovery operation.

**The load-bearing caveat: avalanche is not security.** Diffusion saturates at round 2, but that is a statistical property, not resistance to a deliberate adversary who picks inputs and inverts. Round count is set by cryptanalytic *margin*, and since 16→24 rounds costs only A#155→A#231 ns, we pay for margin freely and default high.

---

## seal / open — the allocation in use

```
seal(secret[k], tweak; r, c):
    state = secret ∥ CHECK(tweak)          # CHECK = BLAKE3(domain ∥ uid ∥ copy), r bits
    return P^c(state)                       # opaque codeword, n bits

open(codeword, tweak; r, c):
    state = (P^c)⁻¹(codeword)
    CHECK(tweak) matches the check field?   # invert-and-check
        yes → return the secret
        no  → zero, fail closed             # NGARO discipline, no repair
```

The check field is not a constant — it is `BLAKE3(domain ∥ uid ∥ copy)`, so a codeword can't be replayed onto another tag, spliced from a sibling mirror copy, or presented under the wrong domain.
Integrity, binding, and domain separation are one field.

---

## The allocation table — one primitive, many products

Sliding the k/r knife and the c dial recovers a family of standard objects as *settings*:

| Setting | You get |
|---|---|
| r = 0 | pure wide-block permutation (bijection, no check) |
| r = 256 | authenticated storage — forgery 2^−256, detection miss 2^−256 |
| r → n | all-redundancy beacon / proof-of-work (creation costs 2^r) |
| c ≫ 1 | slow mode / key-stretching (per-try tax — symmetric, reader pays too) |
| keyed slots | cipher mode (Even–Mansour) |
| unkeyed slots | public all-or-nothing transform |

For a full-entropy 256-bit secret, c = 1 — there is nothing to stretch.
The slow modes are for the small/human-secret aisle only.

---

## First embodiment — the tag codeword

An NTAG213 sticker holds one 256-bit share as a MIRO codeword, mirrored, in 144 bytes:

```
UID (7B, read-only)          → identity / index                [free, not user mem]
─── user memory (144B) ───────────────────────────────────────
VSF header: magic | type | version | share-index    ~12B
copy A:  P^24(share ∥ CHECK)                          64B      # 512-bit codeword
copy B:  P^24(share ∥ CHECK')                         64B      # distinct tweak per copy
spare                                                 ~4B
```

k = 256 (share), r = 256 (the check strand *is* the leftover half of the 512-bit block — free, at zero extra storage).
Versus `share ∥ hash(share)` at the identical 64 bytes: MIRO gives **r = 256 not 64**, **all-or-nothing** (a partial NFC skim yields zero share bits, where plaintext-beside-hash leaks whatever was read), and **binding** (the check is `BLAKE3(domain ∥ uid ∥ copy)`, uncloneable across tags).
The codeword rides inside the VSF document as an opaque blob — no fixed-width integer on persistent storage, it is not an integer.

---

## Recovery — the four-slot rule

The tag's two intra-tag copies (A, B) survive **page rot on one tag**.
Surviving a **lost, destroyed, or maliciously-rewritten tag** is a different tier: the secret is split **2-of-4 Shamir across four independent media** (four tags, or tags + custodian devices), each share sealed as its own MIRO codeword.
This is the [PRINCIPLE.md](PRINCIPLE.md) invariant made concrete — one strand is nothing, the weave is the secret.

Why four and not three — the split is **rot vs adversary**:

- **Rot** is localizable. A rotted share fails its own MIRO open (check-field mismatch, miss prob 2^−r), so it becomes an **erasure** — you know which slot is bad. Any two good shares reconstruct.
- **Adversarial rewrite** is not. An attacker who controls one slot knows its public tweak `(domain ∥ uid ∥ copy)`, so they can seal a *wrong* share into a codeword that **passes its own check**. That is a genuine Byzantine error, and a self-check the attacker can satisfy is not a vote.

Shamir 2-of-4 is a **[4, 2] Reed–Solomon code, minimum distance d = 3**, so it corrects `2e + f ≤ 2`: **one Byzantine strand, or two erasures**, purely algebraically from the four shares — no external anchor required.
Three slots ([3, 2], d = 2) detect a liar but cannot vote one out (three symmetric points, no majority), so they need a fourth slot *or* a commitment anchored off-slot. The fourth slot is chosen because it is **self-sufficient**: recovery needs zero reachable infrastructure, which is the entire point of offline custody.

Recovery flow: open all four tag codewords (rot → erasure, adversarial → candidate share); RS-decode the surviving share *values* to correct ≤ 1 Byzantine or ≤ 2 erasures; reconstruct the secret; recompute the bad share (two points fix the degree-1 line, evaluate at its x); reseal and rewrite the bad tag.
One slot read still yields **nothing** — Shamir t = 2 is information-theoretic, reinforced by MIRO all-or-nothing.

These are **four distributed shares on independent media** — not four identical same-device mirrors, which buy nothing against correlated failure.

---

## Honesty Rails

These are not caveats to bury — they are the terms MIRO ships under.

1. **MIRO-P is not a proven strong PRP.** Encode-then-encipher's forgery bound assumes one; ours is a conjecture backed by BLAKE3's round analysis plus generous margin. Sufficient for rot-detection and skim-resistance today. **Not** yet suitable where a live adversary crafts codewords against the check — that needs outside cryptanalysis first.
2. **The round-constant schedule is first-cut.** The prototype uses a simple `(position, round)`-indexed constant pull; the shipping version should mirror BLAKE3's message-permutation schedule for maximum cryptanalytic transfer.
3. **Tweak enters hashed, never raw** — attacker-controlled sparse differences never touch the state directly.
4. **c is symmetric** — the reader pays the per-try tax too. Use slow modes only for genuinely small secrets.

## Prior Art — MIRO invents no cryptography

Every component is textbook or named. What is ours is the *synthesis and application* — one tunable primitive, wired to the ferros tiers, with the conservation law exposed as the interface. The crypto underneath is borrowed and old:

| Piece | Prior art |
|---|---|
| Rounds invertible in the injected word | message modification (Wang et al., 2005); meet-in-the-middle preimage |
| Feed-forward removed → reversible permutation | cryptographic permutations / sponge (Keccak-f, Gimli, Xoodoo, Ascon) |
| Redundancy-in-plaintext + wide permutation = authenticated | encode-then-encipher (Bellare–Rogaway, 2000) |
| Partial read reveals nothing | AONT / package transform (Rivest, 1997) |
| One permutation, tunable secret/check split | sponge rate/capacity; duplex; deck functions |
| n = k + r | information theory — Singleton bound, rate–distortion |

Any defensible claim is on the **adjustable-allocation mechanism and its systems integration**, not on the mixing or the math.

---

## Status / Next

- [x] MIRO-P prototype + measurement harness ([ferros_miro/](ferros_miro/)) — round-trip, avalanche, detection, perf all measured.
- [ ] `ferros_miro` as a proper `no_std` workspace crate using the ferros BLAKE3, with the Circle-style invertible-newtype `G`/`G⁻¹` (idiom lifted from `sha_inv`).
- [ ] BLAKE3-faithful round-constant schedule.
- [ ] NEON path so seal/open lives in the KEY_REGISTERS pinned-asm zone.
- [ ] Outside cryptanalysis before MIRO-P guards anything against a live adversary.
