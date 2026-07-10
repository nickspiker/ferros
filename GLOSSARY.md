# GLOSSARY — TOKEN Terminology
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** One word per thing, one thing per word.

---

## Convention

Following the convention the PIPE patent sets (see [pipe](../pipe/patent/patent.tex) §Terminology), two categories of term are typographically distinct:

- **lowercase italics** — *whakaira*, *ira*, *wairua*, *ihi*, *mana*, *motuhake* — name the language-level primitives themselves (ceremonies, identifiers, essences), which exist independently of any particular machine that implements them.
- **CAPITALS** — KORE, WHARA, HARA, ORA, NGARO, KOHI, HORO, TATARI, TAHU — name finite-state-machine states and protocol constants, following the hardware-description-language convention for state identifiers.

The PIPE patent is **authoritative** for the hardware terms (*whakaira*, *ira*, *wairua*, the five states, the four *whakaira* sub-steps). This file is the cross-stack reference that ties them to the software layers; each crate's README owns the etymology of its own name.

---

## The identity story in one breath

A device has *mana* — what it provably is.
The oracle (on ferros) or *tohu* (in software, interim) reads that *mana* and produces *ihi* — what others can recognise it to be.
The hardware root of *mana* is the *ira*, a permanent identifier begotten once by the *whakaira* ceremony (interim: derived from the SoC HUK); alongside it the chip holds a *wairua*, a fresh per-session secret.
**TOKEN** is the substrate that binds a *handle* to a device's *ira* and publishes the resulting *ihi*; **custodes** recovers it when every device is lost; **RUA** carries messages to it when the device is offline.

---

## Identity essence

- **mana** — the force that flows thru an entity from its nature and lineage; not possessed, not earned by choice, not claimed — *recognised*. A process's *mana* is its provable substance: binary content, signing authority, hardware state.
- **ihi** — the specific quality of *mana* that is **outwardly perceptible**: the presence that can be witnessed and verified from outside. The oracle ([ORACLE.md](ORACLE.md) §3) and the [`ihi`](../ihi) crate both *compute* this — the process does not assert its identity; the math recognises what it provably is and hands it the *ihi*. At the handle layer this is `handle_proof`. Forging an *ihi* requires navigating ≥ 10^792 paths thru `spaghettify` (the observable universe has ~10^80 atoms).

---

## Hardware genesis (PIPE)

- **whakaira** — the owner-initiated, irreversible ceremony in which the chip's permanent identifier is brought into existence from physical entropy captured on-chip; witnessed only by the chip itself. The te reo word is the causative *whaka-* prefixed to *ira* — *"to bring the ira into being."* Its four sub-steps:
  - **KOHI** — gather entropy from on-chip true-random-number generation (TRNG).
  - **HORO** — chaotic-dynamical amplification of the gathered entropy (swift motion / landslide / to swallow whole — the rapid information-destroying mixing).
  - **TATARI** — a randomized pre-burn delay (waiting) interposed to defeat timing-predictable power-cut attacks on the burn.
  - **TAHU** — the write-once burn of the conditioned entropy.
- **ira** — the permanent identifier the *whakaira* begets; the chip's lifelong device identity. Never transmitted; attestations are domain-separated commitments derived from it. (te reo: a mark; *ira tangata* = the life-essence carried in lineage.)
- **wairua** — the per-session secret drawn from fresh physical entropy at the first handshake after power-on, held in volatile registers until power interruption, distinct from the *ira*. (te reo: spirit, soul.)
- **motuhake** — a canonical test-domain identifier used in protocol exercises. (te reo: separate, distinct.)

---

## Operational states

A two-bit field, value = the chip's vitality on the four-pair redundancy scale (rising with attesting capability):

| code | bits | meaning |
|------|------|---------|
| **KORE** | 00 | pre-provisioning void — write-once cells zero, no *whakaira* performed, no identity (primordial absence of form) |
| **WHARA** | 01 | two redundancy pairs valid (two invalid) — minimum byzantine quorum (bruise, injury) |
| **HARA** | 10 | three pairs valid (one invalid) — single-fault tolerated (breach, fault, error) |
| **ORA** | 11 | all four pairs valid — fully operational (alive and well) |
| **NGARO** | — | silent failure: wire output gated off, indistinguishable from a chip not physically present (lost, vanished, silenced) — no numeric code; inferred from absence of response |

---

## The substrate

- **TOKEN** — the identity substrate: handle namespace, device binding, *ihi* derivation, billing attestation. Everything below composes because it is all one identity substrate.
- **handle** — the user's chosen identity: any Unicode string, a shared secret given to contacts out-of-band, never transmitted by the network.
- **handle_seed** = `BLAKE3(NFC(handle))` — the **secret** local derivation root; never leaves the device.
- **handle_proof** = `spaghettify(handle_seed)` — the **public** presence/lookup key (memory-hard, ~1 s). The handle-layer *ihi*.
- **device_secret** — the device root. In software (interim) it is *tohu*'s `BLAKE3(machine_fingerprint())` — the stand-in for the *ira*.
- **oracle** — ferros's spawn-time function producing a process's *ihi* from its *mana* (`ihi = oracle(system_persistent_secret ‖ developer_pubkey ‖ option)`); see [ORACLE.md](ORACLE.md).

---

## Crate roster

| crate | is | name |
|-------|-----|------|
| [`vsf`](../vsf) | Versatile Storage Format — the self-describing wire/storage encoding | — |
| [`voca`](../voca) | wordlist encoding of values — human-readable, unambiguous | vocable |
| [`ihi`](../ihi) | `handle_proof` / `spaghettify` — *mana* made outwardly verifiable | *ihi* (perceptible mana) |
| [`tohu`](../tohu) | device identity: per-platform oracle + frozen derivation; the software stand-in for *ira*/PIPE | *tohu* (a sign drawn from the formless) |
| [`manifestus`](../manifestus) | content-addressed storage engine (mirrored blocks, generation ring, COW HAMT) | manifest |
| [`miro`](../ferros_miro) | reversible avalanching permutation with tunable secret/check split; seals a payload into an opaque codeword (see [MIRO.md](MIRO.md)) | *miro* (to twist strands into cord) |
| [`custodes`](../custodes) | K-of-N social recovery for total device loss | custodians |
| `photon` | the messenger — CLUTCH key ceremony, CHAIN rolling encryption, FGTW transport, RUA dead-drop | a quantum of light |
| `fluor` | CPU compositor / GUI toolkit | fluorescence |
| `ferros` | the field — OS, kernel, oracle, vault, ledger, RUA | iron (Fe) |
| `pipe` | the PIPE identity chip — patent + RTL; *whakaira*/*ira*/*wairua* in silicon | the wire interface |

---

## Interim ↔ endgame

The same split ferros draws everywhere ([ORACLE.md](ORACLE.md), [SEED.md](SEED.md), [PIPE.md](PIPE.md), [RUA.md](RUA.md)):

| primitive | endgame (PIPE) | interim (commodity silicon) |
|-----------|----------------|------------------------------|
| *ira* | begotten by *whakaira*, burned write-once, owner-witnessed | derived from the SoC **HUK** at EL3 — permanent, per-device, unextractable, but **vendor-provisioned** (the sovereignty gap *whakaira* closes) |
| *wairua* | fresh TRNG entropy in a volatile chip register | SoC TRNG read at boot into a crypto-engine key register (volatile, on-die); mlock'd RAM as fallback |
| oracle / *ihi* | kernel-mediated, rooted in the on-chip *ira* | `tohu` in software, rooted in `device_secret` |

The wire and the derivations are identical across the split, so the kernel/hardware service drops in under the apps unchanged.
