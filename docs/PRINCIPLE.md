# PRINCIPLE — No Strand Is the Cord
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Meaning lives in the weave, never in a single strand; own one medium, own noise.

---

## The Invariant

Every secret in ferros is a **relationship across independent media**, never a property of any one of them.
An adversary can seize objects; they cannot seize a correlation they do not participate in.
So controlling any single medium yields — on every axis at once — nothing.

| Axis | Adversary with one medium | Mechanism |
|---|---|---|
| **Read** (confidentiality) | learns nothing | Shamir share below threshold is information-theoretically independent of the secret; MIRO all-or-nothing on top |
| **Forge** (integrity) | cannot fabricate a valid whole | the check binds to identity + anchors upward; a single forged medium is checked against references it does not hold |
| **Destroy** (availability) | destroys nothing | redundancy — mirror, generation ring, k-of-n — reconstructs from the survivors |

The secret was never *in* a medium.
It lives in the correlation across media, and correlation is the one thing an adversary holding one strand cannot hold.

---

## It Is *ira meaningless alone*, Generalized

This is not a new property.
It is the PIPE principle — *the ira has no meaning without TOKEN binding* — discovered to have never been special to the ira.
It is the spine of the architecture, and the same shape recurs at every layer:

- **ira** — meaningless without the binding.
- **the sticker** — "just a message into the PIPE"; its meaning is entirely device-side (see [PIPE.md](PIPE.md), the NFID sticker model).
- **MIRO** — the secret is the whole codeword's joint state; no fragment is anything ([MIRO.md](MIRO.md)).
- **NFID** — identity is the joint likelihood across signals; no single signal is you.
- **vault** — keyed-name + keyed-contents; meaning is in the key relationship ([VAULT.md](VAULT.md)).

Every one says: *the thing is in the weave, not the strand.*
ferros never stores secrets — it stores strands whose weave is the secret, and no strand is anything.

---

## The Threshold t and the Coding Dial

The invariant is parameterised by a single number, the **threshold t**: an adversary must control **≥ t** independent media to gain anything.
"One medium is nothing" is the case t = 2.

Redundancy — the n − k extra media beyond the k needed to reconstruct — is a second, independent budget, and it is pure coding theory.
A k-of-n Shamir split is an **[n, k] Reed–Solomon (MDS) code** with minimum distance **d = n − k + 1**, and that d spends exactly as:

- **detect** up to `d − 1` corrupt media,
- **correct** up to `d − 1` **erasures** (known-bad),
- **correct** up to `⌊(d − 1)/2⌋` **Byzantine** errors (unknown-bad),
- mixed: `e` Byzantine + `f` erasures are correctable when `2e + f ≤ d − 1`.

The decisive split is **rot vs adversary**:

- **Random corruption (rot)** is localizable — each medium is a self-verifying MIRO codeword, so a bad one *fails its own check* and becomes an **erasure**. Cheap to correct.
- **Adversarial corruption** is not — an attacker who controls a medium and knows its public tweak can write a *self-valid but wrong* strand. That is a genuine **Byzantine** error, and self-checks the attacker can satisfy are not votes.

So the correction budget must be sized for the *adversarial* case.
The chosen operating point is **2-of-4** ([4,2], d = 3): one medium reveals nothing (t = 2), and it corrects **one Byzantine strand algebraically from the four alone** — no external anchor required.
(Contrast three slots: [3,2], d = 2, detects a liar but cannot vote one out — three symmetric points, no majority.)

---

## The Anchor Rule

There are two ways to localize a malicious strand, and the architecture's deepest recurring law decides between them:

**The authority must live outside the thing it authenticates.**

- The vault's hash comes from the **parent**, not the neighbor.
- Recovery needs a **trusted device**, not the lost one.
- A share's commitment must sit **off the strand** it commits to.

So you localize an adversarial strand by *either*:
1. **A verifiable-secret-sharing commitment anchored outside the media** — works with three slots, but recovery then depends on that anchor being reachable, *or*
2. **A fourth slot** — [4,2] corrects one Byzantine strand with no anchor at all.

Option 2 is chosen for the distributed tier precisely because it is **self-sufficient**: four independent strands recover with zero infrastructure, which is the whole point of physical, offline, doomsday-proof custody.
Note these are **four distributed Shamir shares** on independent media — *not* four identical same-device mirrors, which buy nothing against correlated failure.

---

## Three Boundaries

The invariant is only useful if you know where it breaks — and its three failure modes are precisely the architecture's three hardest problems, which is the sign the frame is real:

1. **The live device is the exception.** At the moment of use the correlation collapses into one place — the reconstruction/trusted PIPE holds the assembled secret. That singular surface is what [KEY_REGISTERS.md](KEY_REGISTERS.md) (detect-and-freeze) and the MPC-reconstruction frontier defend. Everything at-rest and in-transit is null alone; the live weave is the crown jewel.
2. **The threshold is never t = 1.** The instant any secret sits with t = 1 — a raw key on one medium — the invariant is void *for that secret*. Nothing is ever t = 1. This is the guardrail welded onto every sticker: a share, never a raw key.
3. **Media must be independent.** Two mirrors on one UFS, or custodians all behind one compromised cloud, mean "one medium" secretly became "all of them." Independence is what makes t real — cross-device, plane-diverse, cross-custodian placement is load-bearing, not fussiness.

---

## The One-Line Design Test

For any new component, ask:

> **If an enemy owns exactly one of these, what do they get?**

The answer must be **nothing**.
If it is not, there is a t = 1 leak or a hidden correlation — and you have found a bug before shipping it.
That single question audits the whole system.

The adversary model states the same thing: *the enemy controls up to t − 1 independent media*, and the entire design goal is to make (t − 1)-compromise a **no-op**.

---

## Prior Art

This is a **known property class**, and ferros invents none of it: threshold cryptography, information dispersal (Rabin IDA), robust and verifiable secret sharing, and the "no single point of trust/compromise" doctrine all say pieces of it.
What is ferros's is the *unusually consistent* application of one frame to **every** layer at once — storage, transport, identity, recovery, and the live register.
The consistency is the contribution, not the property.

## Naming note (verify the gloss)

A single **whenu** — one warp thread — is not the cloth, and alone it carries **KORE**, the void state already defined in [GLOSSARY.md](GLOSSARY.md).
*An isolated medium is in KORE:* it exists, but holds no identity until it is woven.
Proposed, pending your gloss check.
