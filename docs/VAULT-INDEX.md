# VAULT-INDEX — naming, the index layer, and crypto invariants

**Status:** design (Phase-1 flat dict is built; the target below is not). Complements [HAMT.md](HAMT.md) (the object-lookup trie) and VAULT.md (tract/plow/spine). Captured from a design discussion 2026-08-19.

---

## The two layers, and why both exist

The vault stacks two addressing schemes, and they answer different questions.

**Content addressing (store by hash).** An object's address *is* the hash of its content. `put(bytes)` stores it at `H(bytes)`; `get(h)` returns the object whose content hashes to `h`. The address falls out of the data itself: identical bytes collapse to one slot (dedup), any change yields a totally different address (integrity — recompute and compare). This is git's blobs, or a filesystem inode. The catch: to retrieve something you must already know its hash, and that hash *changes every time the data changes* — useless as a stable handle for "Alice's current contact info."

**Named addressing (store by name).** One level of indirection: a dictionary mapping a human string to a content hash. `set("contacts/alice", bytes)` hashes the *content*, stores the object by that hash, and records `"contacts/alice" -> hash` in the dict; `get_by_key("contacts/alice")` looks the string up and fetches the object at that hash. The name is never hashed — it is a literal key whose *value* is a content hash. The point is mutability: when Alice's data changes, the new content gets a new hash and the dict entry repoints, but the name stays put. Names on top of hashes = mutable, human handles over an immutable, self-verifying store. The mental model: content hash is a git commit SHA / inode; logical name is a git branch `main` / filename — `main` moves as you commit, the SHAs don't.

**The one-way trap.** It is tempting to "solve" long or private names by hashing the name itself. Don't: hashing is one-way, so it does not *solve* enumeration, it *abolishes* it — the moment you hash the name you can never list it, show it, or scan it again. And the instant you need the name back (enumerate a user's files, render a name, prefix-search), you must store it somewhere anyway — at which point **encryption strictly dominates hashing**, because it is reversible: opaque on disk, recoverable in RAM. Hashing only wins if you will *truly never* want the name back, which for a contacts/files vault you obviously will.

**The meta-principle:** hashing throws information away; encryption keeps it and hides it. Any time you reach for a hash to solve a storage problem, ask whether you will ever want the thing back — and for anything a human names, you will.

---

## Current state (Phase-1, honest)

`vault/src/backend.rs` + `vault/src/root_commit.rs`, as built and proven on hardware:

- **Flat dictionary.** `RootCommit` is one flat `BTreeMap<String, ObjectHash>`, stored as a single content-addressed object; the anchor's `root_commit` field points at it. Every write re-encodes the *entire* namespace — `O(total keys)` per commit.
- **Keys stored verbatim**, length-prefixed `[key_len: u16][key bytes][hash: 32]`. That `u16` is a fixed-width integer on disk — already a violation of the EWE convention ("no fixed-width integers on persistent storage, ever") — and it panics via `assert!(key.len() <= u16::MAX)` above 64 KiB (a hung kernel in the shim, since nothing warm-resets after a panic).
- **~~Addresses are bare `blake3(plaintext)`~~** — NOW keyed (`crypto::content_address`), see the checklist below.
- **~~Nothing is encrypted.~~** — NOW every object body is XChaCha20-Poly1305 sealed, and because the root-commit dict is itself an object, the plaintext key names are encrypted too. The `v(b'e', ...)` VSF marker finally means what it says.

The original Phase-1 flat dict (names stored verbatim, one `BTreeMap` re-encoded per commit) still stands as the *index structure*; what changed is that its bytes are now keyed-addressed and sealed. The evolution below still replaces the flat dict with a HAMT — that is the remaining *index* work, orthogonal to the encryption just landed.

---

## Target: an HMAC-keyed HAMT of encrypted nodes

The flat dict is a single flat directory. The structural fix is the HAMT that is already in the design vocabulary ([HAMT.md](HAMT.md)), applied to the *naming* layer:

- **Navigate by `HMAC(vault_secret, name)`.** Any-length names, fixed-size navigation, `O(log₃₂ N)` lookup — the length never touches the hot path.
- **Keyed, not bare.** `HMAC`/BLAKE3-keyed, never bare `blake3`: a bare hash of the name lets an attacker guess-and-confirm offline; a keyed hash makes the index unguessable without the vault open, and scopes dedup to *your* vault (no cross-user convergence).
- **Names live at the leaves**, so enumeration still works (walk the trie) and commits are incremental — only the root->leaf path rewrites (`O(log N)`), not "re-encode the whole namespace every write."
- **Every node encrypted at rest.** Names are opaque blobs on flash, plaintext only once unlocked in RAM. This is the ferros-correct answer to the metadata leak — *encrypt the index*, do not hash it away.
- **EWE lengths, no fixed cap.** Under EWE the length self-describes; a name is whatever length it is. `NAME_MAX = 255` is filesystem legacy with no principle behind it. Length is a non-issue for lookup (fixed 32-byte HMAC) and only costs space in the one encrypted leaf that holds it.
- **Error, never panic.** An over-long or absurd key returns `Err`, it does not `assert!`. Optionally a *generous* sanity threshold ("this key is >1 MiB, did you mean to pass it as a value?") that errors rather than a silent limit. Trust the owner (their device, their call); the non-negotiable is never hanging the kernel.

Strip the jargon and this is a **cryptographic filesystem** — which is what a vault is.

### The one fork to decide later, not now

A HAMT is hash-ordered, so `contacts/*` is no longer a contiguous range — **prefix/range scan is the one thing it gives up.** The decision is: does Photon need prefix/range browsing, or only exact-name `get` + full enumeration?

- Exact-get + enumerate only -> **HAMT keyed on `HMAC(name)`**, nodes encrypted. Clean.
- Prefix/range too -> a **key-ordered** structure (radix/directory tree over the plaintext-in-RAM names) keeps locality. Prefer building this as an *index on top when unlocked* rather than burdening the on-disk structure — don't pay for a query pattern until it's a real requirement.

---

## Crypto invariants

These bind the whole vault, not just the index. Nothing is encrypted yet, so this is greenfield — get it right from the start rather than migrate.

- **XChaCha20-Poly1305 everywhere we do AEAD.** 192-bit (24-byte) nonces, so *random* nonces are birthday-safe essentially forever (collision negligible at 2⁹⁶). **Do NOT use ChaCha20-Poly1305 or AES-256-GCM** — their 96-bit (12-byte) nonces are birthday-bound (~2⁴⁸ messages), and a single nonce reuse under ChaCha/GCM is catastrophic (plaintext XOR leak + forgery). **The `vsf` crate today offers only 96-bit-nonce AEADs** (`WRAP_CHACHA20POLY1305`, AES-256-GCM; its `v'e'` path is X25519 + AES-256-GCM/12-byte nonce), so adopting `vsf`'s crypto as-is inherits the footgun — XChaCha must be added (in `vsf` or done in the vault layer).
- **Random nonces, not counters.** A monotonic per-key counter looks tempting but is fragile under exactly the operations a vault does — rollback, mirror, restore-from-backup — any of which can reuse a counter value and reuse a nonce. 192-bit random nonces sidestep the entire counter-state problem; that's the reason XChaCha, not merely convenience.
- **Addresses must be keyed.** Content addressing should be `BLAKE3-keyed(vault_secret, plaintext)` / `HMAC`, not bare `blake3(plaintext)` — otherwise a disk-holder confirms guessed content by hashing, and dedup would converge across different owners.
- **Dedup and random nonces coexist.** Address = keyed-hash(plaintext); stored bytes = XChaCha(random nonce, plaintext) with the nonce prepended. Each *distinct* plaintext is encrypted exactly once (dedup skips re-encryption of an address that already exists), so every (key, nonce) pair is used once — no reuse — while `get` decrypts the one stored ciphertext. The AEAD tag plus the keyed address give integrity twice over.

### Where the crypto lives (vault vs VSF)

Keep the cipher and keys **in the vault**, not in VSF. The vault already marks its payload `v(b'e', ...)` = "opaque, don't render"; if the vault does XChaCha itself and hands VSF an opaque blob, VSF is a pure container and its 96-bit-nonce weakness never touches vault data. This also keeps key management (Trust Domain A, the anchor key, per-object keys) inside the vault where the threat model lives, instead of making VSF the crypto authority for vault contents. So the vault gets XChaCha without waiting on any VSF change.

VSF *itself* still needs additions — hygiene for its native encrypted fields, decoupled from the vault (separate crate, `/mnt/Harbor/Code/vsf`):

- Add a **XChaCha20-Poly1305 wrap algorithm** alongside `WRAP_CHACHA20POLY1305 = b'c'`, registered key=32 / **nonce=24** / tag=16 in `crypto_algorithms.rs`.
- Make the `v'e'` encrypted-blob format **algorithm-tagged** so the nonce length is self-describing. Today it hardcodes X25519 + AES-256-GCM with a 12-byte nonce (`decrypt.rs`: `nonce_bytes: [u8; 12] = encrypted[32..44]`) — a non-self-describing length field, the same class of bug as the fixed-width `u16` in the index. Any VSF file that leans on built-in encryption is birthday-bound until this lands.

---

## Near-term checklist (concrete, in order of cheapness)

1. **~~Kill the `assert!` panic -> `Err`.~~** DONE — `StoreError::KeyTooLarge` (`set` guards the u16 key ceiling).
2. **~~Encrypt the flat dict blob~~** DONE — and better than planned: the dict is itself a `Record` object, so encrypting *all* object bodies (below) sealed the names for free. Proven by the `on_disk_bytes_are_opaque` test (neither key names nor values appear in plaintext on disk).
3. **~~Key the addresses~~** DONE — `crypto::content_address` = `blake3::keyed_hash(addr_key, plaintext)`; the store's object identity is now the keyed address, dedup stays vault-scoped, convergence defeated.
4. **~~Add XChaCha20-Poly1305~~** DONE — [`vault/src/crypto.rs`](vault/src/crypto.rs): `seal`/`open` (24-byte nonce), `NonceSource` (kernel TRNG / host `rand`), domain-separated `payload_key`/`addr_key`. Non-optional core, compiles no_std on `aarch64-unknown-none`. The store seals every object body; VSF stays a pure opaque container.
5. **Build the HMAC-keyed HAMT index** ([HAMT.md](HAMT.md)) when scale (commit cost, key count) demands more than the flat dict. **← next**

Implementation notes for the current encrypted store (see [`vault/src/backend.rs`](vault/src/backend.rs)): object envelope is `[magic][ver][keyed-address 32][sealed_len u32][vsf_type][generation][sealed = nonce‖ct‖tag]`. The index scan reads only the address (no key needed); `get` is the only decrypt path. Nonces are caller-supplied per seal — the kernel's `TrngNonce` is `BLAKE3-keyed(fresh-per-boot wairua seed, monotonic counter)`, so it needs one entropy draw at boot rather than one per nonce, and nonces never repeat across boots (fresh seed each time). Still open: the `sealed_len` is a fixed-width `u32` on disk (an EWE-convention violation, same class as the root-commit `u16`), to fold into the EWE pass with the HAMT.
