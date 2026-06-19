# RUA — ferros Dead-Drop Specification
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** A pit named by a shared secret. Knowing the handle is the only key to write; owning it, the only key to read; the field reclaims what's taken.
**Glossary:** the TOKEN vocabulary (*handle* / seed / proof, *ihi*, *ira*, *wairua*) is defined in [GLOSSARY.md](GLOSSARY.md).

---

## Philosophy

*rua* (Māori): a storage pit — the dug-in cache you drop food into and dig back out later. **RUA is ferros's asynchronous dead-drop**: one pit per handle, kernel-mediated, any-size, VSF-encoded, optionally sealed so only the intended recipient can decrypt. A sender who knows a handle (out-of-band) drops a sealed message; the handle's owner digs it out and clears it.

It is the **async complement** to synchronous P2P. An application establishes live sessions directly (Photon: CLUTCH + CHAIN); RUA carries what can't be delivered live — an offline peer, a first contact before any session exists, an IP-mobility gap. Best-effort, TTL'd: a store, not a ledger.

**Interim and endgame** — the same split ferros draws everywhere (see ORACLE.md, SEED.md, PIPE.md). The eventual RUA is a ferros field service: kernel-mediated, ownership enforced on the device's `handle_seed`, garbage-collected by the ring/vault reachability sweep. Until ferros provides it, an application implements the interim against a network rendezvous (Photon over FGTW). The wire — handle-addressed, sealed, best-effort — is identical, so the kernel service drops in under the apps unchanged.

There is **no proof-of-work, no rate limiter, no size ceiling**. RUA assumes honest writers and earns that structurally (§3): the address is derived from the handle, and the handle is a secret given only to contacts — so the only parties who can find your pit are the ones you chose to trust.

---

## 0. Addressing — the public proof, never the secret seed

A sender has only the recipient's **handle**, never their `device_secret` or `handle_seed`. The address derives from the handle alone, and *which* handle value keys the pit is a privacy decision:

```
rua_key = handle_proof          // memory_hard(handle_seed); already the network presence key
```

- `handle_seed = BLAKE3(NFC(handle))` — instant, the **secret local derivation root**. `seed → proof` is computable (one-way), so handing the seed to a network store would let it recompute the proof *and* leave it hoarding the secret root. **The seed never leaves the device** — on ferros it is the local ownership key the kernel checks (§2), nothing more.
- `handle_proof = memory_hard(handle_seed)` — ~1 s, **already public** (it is the presence/lookup key). Keying the pit on it leaks nothing the network doesn't already hold.

So the store is keyed on `handle_proof`; the device-identity layer (tohu, in software) supplies the addressing root; the secret seed stays home.

---

## 1. Write

A write is a VSF blob, optionally sealed to the recipient:

```
RuaEntry (VSF):
  enc      : bool — payload is sealed to the recipient's pubkey bundle
  payload  : VsfType — plaintext VSF, or a ciphertext sealed to the recipient
             (sender provenance, if any, lives INSIDE the sealed payload — §3)
```

The recipient's pubkey bundle is discoverable via `handle_proof`. A sealed payload is readable only by the handle's owner; the store and any other party see ciphertext. Plaintext is allowed for content meant to be public (a presence beacon), but most traffic is sealed. No size cap and no sender cost — §2 is why that's safe.

---

## 2. Access control — the handle is the capability

The address *is* the secret. To write your pit a sender must know your **handle**, which you give only to contacts, out-of-band, and which the network never transmits. So the set of parties who can even *find* your pit is exactly the set you handed your handle to. A stranger can't address a pit they can't name.

That collapses the "world-writable" worry into ordinary trust management:

- A **contact** who abuses your pit is a *social* problem — block them, or rotate to a new handle.
- An **app** that abuses it is a *removal* problem — the user uninstalls it.
- A **broadly-leaked handle** opens the pit to whoever leaked it — but that is the handle-confidentiality problem (the fix is a new handle, which burns the old; see tohu's `docs/handle.md`), not something a per-message proof-of-work would solve.

So there is deliberately no anti-flood crypto. It would guard a door the handle-secrecy already locks, at the cost of complexity and a tax on every honest sender. Capability-by-secrecy is the model.

---

## 3. Permissions & provenance

Capability, not an ACL:

- **Write** = know the address (= know the handle = be someone the owner trusted with it).
- **Read** = own the handle: recompute `handle_proof` to locate the pit, decrypt the sealed payloads with the bundle's private keys. On ferros the **kernel gates read to the holder of the device's `handle_seed`** — and that is the one place the secret seed is legitimately used as an ownership root, because it never leaves the device.
- **Provenance** — who wrote each entry — is sealed *inside* the payload (the sender signs/identifies within the ciphertext), so only the recipient sees it and the store sees opaque blobs. It gates nothing (the address already did); it exists for attribution, threading, and blocking one sender. Near-moot for local honest apps (one user, one handle); meaningful when entries arrive from network senders.

---

## 4. Read & garbage collection

The handle's owner is notified of new mail by push (FCM in the interim; a kernel signal under ferros) or polls on its own cadence, fetches entries at `rua_key`, verifies each seal, and decrypts.

GC has no bespoke machinery — it rides the field layer:

0. **Read-and-delete (primary).** The consumer removes processed entries; honest apps clean up after themselves.
1. **TTL backstop.** Never-picked-up entries expire so an abandoned pit drains rather than growing forever.
2. **Reachability sweep.** A deleted or expired entry's blocks become unreachable, and the ring/vault mark-sweep from live roots reclaims them — the same GC every vault uses (RING.md, VAULT.md). "Take it from the pit; the field fills it back in."

A torn entry (failed seal) is dropped like a corrupt block — best-effort, no recovery for a single lost message (the sender retries, or falls to live P2P).

---

## 5. Consumers — how an application uses the pit

RUA is a primitive; an app composes it. Photon's use:

- **First contact.** Alice knows Bob's handle (out-of-band), computes `handle_proof(Bob)`, fetches his published bundle, seals a `ClutchFullOffer` to it (large — the "any size" case), and drops it at `rua_key(Bob)`. Bob, next online, is notified, digs it out, and the CLUTCH ceremony proceeds.
- **Offline delivery.** A CHAIN message for an unreachable peer waits in the pit until they surface.

| Path | When |
|------|------|
| CLUTCH (synchronous P2P key ceremony) | both parties online, establishing a relationship |
| CHAIN (rolling per-message encryption) | active session, both reachable |
| **RUA (async dead-drop)** | **offline peer, first contact, IP-mobility gap** |

Direct P2P is always preferred when available (lower latency, no rendezvous metadata). RUA is the fallback and the bootstrap.

---

## 6. What RUA is not

- **Not the local handle slot.** That is tohu's device-private, app-shared store keyed on `device_secret` (`handle.md`) — *this device's one handle*, shared across the apps on it. RUA is network-side, keyed on `handle_proof`. Same word "handle," different store.
- **Not the hardware mailbox.** ferros's coprocessor IPC mailboxes (ASC/SMC/RTKit; see KILLSWITCH.md) are register-level message channels — unrelated to this handle-addressed pit.
- **Not guaranteed delivery.** Best-effort, TTL'd. A message can expire unread; senders retry or fall to P2P.
- **Not a ledger.** No consensus, no cross-writer ordering — a store, not a chain (cf. LEDGER.md, which *is* the ordered chain).
- **Not anti-flood-hardened, by design.** The address is a secret given only to contacts, so access control is the handle itself (§2); abuse is handled socially, not cryptographically.
- **Not safe against a leaked handle.** The address *is* the handle; confidentiality of the handle is what keeps the pit yours.
