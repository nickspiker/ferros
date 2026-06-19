# PIPE — Physically Isolated Processing Enclave

**Glossary:** the TOKEN vocabulary (*mana*, *ihi*, *whakaira*, *ira*, *wairua*, the chip states KORE/WHARA/HARA/ORA/NGARO) is defined in [GLOSSARY.md](GLOSSARY.md); the patent ([../pipe](../pipe/patent/patent.tex)) is authoritative for the hardware terms.

## What it is

PIPE is the substrate the [oracle](ORACLE.md) wants. A small processing enclave, physically isolated from the host system that uses it — independent power, internal ungoverned clock, single-wire authenticated channel. The enclave holds a write-once key in a register that has no read path, computes keyed BLAKE3 MACs, and answers challenges. The host learns the enclave's identity by enrolling its key once; from then on the host trusts MACs because nothing else can produce them.

The acronym widens deliberately: **physically** isolated, not photonically. Photonic, galvanic, inductive, capacitive — all separate isolation embodiments under one umbrella claim. The security property is physical isolation. The barrier medium is an implementation detail.

## Plain-language protocol

Two devices share one wire.

One side — the enclave — pulses the wire constantly. On off on off on off. Like a heartbeat. This means it's alive. It also lets the other side track its clock.

Either side can talk. If the enclave wants to send something it puts a beat where there shouldn't be one. If the host wants to send something it skips a beat. One bit. That's the entire handshake. Both sides are always watching so neither can accidentally talk at the same time.

When data flows it's always a cryptographic hash. Random-looking by construction. Which means the wire never gets stuck at one value long enough to cause problems. No special encoding needed. No overhead. Every bit is payload.

When the exchange is done the heartbeat resumes.

## The whole protocol

- **Idle** — heartbeat
- **Initiate** — one unexpected bit
- **Data** — raw crypto, no encoding
- **Done** — heartbeat resumes

## Why this is the tightest possible serial protocol

Every other serial protocol ever built adds overhead to solve problems that cryptographic payload solves for free. UART has start/stop bits because raw data could ambiguously sit at one level. 8b/10b encoding ensures DC balance and recovery clock edges. HDLC has framing flags because data could collide with control. SPI has chip selects because multiple peers could clobber each other.

PIPE has none of that. One wire. One bit to start. Every subsequent bit is real data. No framing. No encoding. No collision avoidance. No checksum (the MAC is the integrity check — and it's the entire payload).

That's it. The simplest possible protocol that is also cryptographically authenticated, collision-proof, self-clocking, and physically attested.

## Module map

The FPGA implementation lives at `/mnt/Octopus/Code/pipe/` (separate repo from ferros). RTL modules:

| Module | Role |
|---|---|
| `pipe/rtl/ring_osc.v` | The only clock source — N-stage LUT cascade, no external timebase |
| `pipe/rtl/trng.v` | Dual ring oscillator entropy accumulator |
| `pipe/rtl/blake3/` | Vendored BLAKE3 compression core (BSD-3, from spirix / 6a-62) |
| `pipe/rtl/key_register.v` | A#256-bit write-once secret, fanout-1 to blake3 |
| `pipe/rtl/boot_fsm.v` | One-way boot to OPERATIONAL trap state |
| `pipe/rtl/prng.v` | ChaCha20 nonce generator |
| `pipe/rtl/ping_pong.v` | Double buffer between blake3 and shift_tx |
| `pipe/rtl/shift_tx.v` | Serializer with idle heartbeat + TX initiate bit |
| `pipe/rtl/shift_rx.v` | A#8x oversample CDR + RX initiate detect |
| `pipe/rtl/cdc.v` | Half-duplex bidirectional IO with 2-flop synchronizer |
| `pipe/rtl/top.v` | Integration |

## Boot sequence

```
power on
    ↓
ring_osc starts (ungoverned, immediate — no clock primitive)
    ↓
trng accumulates A#256 entropy bits
    (time depends on ring frequency; typically milliseconds)
    ↓
blake3 compresses entropy → A#256-bit seed
    ↓
seed → key_register (write once, lock immediately)
    ↓
ChaCha20 seeded from a derivative of the boot secret
    ↓
OPERATIONAL state (irreversible — only power loss exits)
    ↓
shift_tx begins heartbeat on the wire
    ↓
ready for challenges
```

## Normal operation cycle

```
enclave: heartbeat ……………….
host:    observes idle, wants challenge
host:    pulls expected 1 down to 0 (init bit)
enclave: detects unexpected 0, stops idle, listens
host:    sends challenge bits
enclave: deserializes challenge into blake3.i_mblock
enclave: blake3(secret_in_key_register || challenge || nonce)
enclave: MAC → ping_pong → shift register
enclave: sends unexpected 1 (init bit)
host:    detects unexpected 1, listens
enclave: sends A#256 bits of MAC
enclave: returns to heartbeat
host:    verifies MAC against enrolled key
```

## Hard structural invariants

Five netlist-level assertions (`pipe/constraints/netlist_assertions.py`):

1. `key_register.key_out` has fanout exactly equal to A#1 — the single sink is `blake3.i_chain`.
2. Zero PCLKT pad usage — no FPGA clock-capable input pin reaches a clock net.
3. Exactly one bidirectional IO pin (the wire). No second side channel.
4. `boot_fsm` OPERATIONAL is a trap state — no edge exits.
5. Zero forbidden clock primitives (`EHXPLLL`, `EHXPLLL_CORE`, `OSCG`, `PLLREFCS`).

Any commit that breaks one of these changes the threat model.

## Relationship to Glyph

PIPE on ECP5 is the FPGA prototype of the same write-once-key anchor that [Glyph silicon](README.md#hardware-platform-glyph) will hold in a custom RISC-V CSR. The capability shape is identical:

- Hardware-controlled write-once register, no read path back.
- Output usable only as the key input to a keyed cryptographic hash.
- Killswitch zeroes the register; once gone, every MAC the enclave previously produced becomes uncorroborable.

`ferros_vault::anchor` consumes the keyed-MAC output regardless of which substrate produced it. PIPE is just the embodiment that proves the architecture in fabric you can actually buy today.

## Repository

Implementation: `/mnt/Octopus/Code/pipe/` — standalone repo, peer to ferros. Not a member of the ferros Cargo workspace.

Status (as of 2026-05-08): Phase A scaffold complete; module bodies and Phase B/C measurements pending.

## Patent

Provisional pending — combined ISOMEM + PIPE + ORACLE. Independent claim covers physical isolation. Photonic / galvanic / inductive / capacitive isolation are dependent claims under it.
