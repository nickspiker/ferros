---
name: Vault Architecture Design
description: Vault design decisions — HAMT, tract/plow, object modes, access control, key hierarchy, revocation
type: project
---

## Vault Storage Architecture (finalized)

- **Tract**: log-structured gravity ring, ~230GB, plow-managed, no block allocator
- **Plow**: sole write head for the tract, advances and wraps
- **Stem**: kernel ring (256 entries at G#C00), scanned by seed
- **Spine**: vault root ring (65536 entries at G#2000), commit log
- **Lone**: inline object in HAMT leaf (< ~3.9KB)
- **Direct**: furrow LBAs in HAMT leaf (< ~4MB)
- **Chained**: extent chain for large objects (> ~4MB)
- **Furrow**: extent data block, minimal VSF: m(index) + hp + hb + v(payload)

## Access Control (Dual-Layer, finalized)

- **Layer 1**: Kernel capabilities — fast path, in-memory, doesn't survive physical access
- **Layer 2**: Cryptographic access control — math enforced, survives physical disk access
- **access() section**: first-class VSF section inside the object, not a separate document
- Spills to own block via `acl(h u)` pointer when too many readers
- Namespace-level by default, per-object override when needed

## Key Hierarchy

- device_key → CSR (PAC registers), never RAM
- boot_key → ChaCha20(device_key, boot_nonce), per-boot
- namespace_key → BLAKE3(boot_key || namespace_hash)
- content_key → random per-namespace, wrapped per-reader in access() section
- System namespaces (Boot, Ledger, Stem) use boot_key directly

## Revocation

- **Hard revoke**: plow re-encrypts now, verify, zero old on both disks. Immediate.
- **Soft revoke**: remove from ACD, kernel enforces gap, plow re-encrypts on rotation.
- Same operation, different timing. Kernel caps cover the gap.

## New VSF Type

- `kx` — X25519 public key (32 bytes), key exchange. `k` family: `ke` = Ed25519 (signing), `kx` = X25519 (exchange).

## RedoxFS Research Notes (for reference)

- Uses 256-block generation ring (same pattern as our rings)
- Buddy allocator for block management (we eliminated this with the plow)
- COW on modification (we do this via HAMT COW)
- Extent tree for files (we use lone/direct/chained instead)
- SeaHash (we use BLAKE3), XTS-AES (we use ChaCha20)
