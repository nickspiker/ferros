# The vault out of the kernel

Status: proposal, 2026-11-07, for the owner's OK. Nothing here is built yet.

## Why

Today the vault is the kernel's (`kvault.rs`, `vault_boot`, `ufs_device`).
Every stored byte any process keeps goes through the kernel's `STORE` call, and the kernel assembles each value whole in its heap to seal it.
That caps a value at 3 MiB (`service.rs` `STORE_VALUE_MAX`): a 24 MB wallpaper is refused, and the display server's own capsule (2.8 MB) is already near the line.
It also means the kernel parses and holds everyone's data.
Owner, 2026-11-07: "the vault needs to be moved out ... a 3MB cap on files and having the kernel involved makes me nervous."

## The rule (owner, 2026-11-07)

- **The kernel is the only one that issues and manages keys.** It holds the roots (the ira and the wairua in registers, KEY_REGISTERS.md) and derives every key from them. Nothing else derives a key.
- **The vault process does the encryption.** It stores, seals, opens and moves bytes, with keys the kernel issues it, one domain at a time.
- **Each kist's data is its own domain.** A kist's key is computed from its signature: the developer's key as it survived the signature gate, through the oracle (ORACLE.md §5-6), so no kist can open another's data, and a kist's data cannot be opened by anything that is not that kist on this device.
- **The person has a common area, keyed by the user.** What belongs to the person across kists (the wallpaper, and later whatever the person shares between their kists) lives under the identity key, open only while they are present (AUTH.md's identity class).

## The domains

| Domain | Key (derived by the kernel) | Open | Holds |
|---|---|---|---|
| Device | from the ira (as the device class today) | from boot | what the device needs before anyone is present: drivers' state, the sealed list, the kernel's bookkeeping |
| Person, common | the identity key (as the identity class today: bound to the ira, the device-only secret and the person's seed) | while the person is present | the person's own things across kists: the wallpaper (`wallpaper`), the arrangement, credentials that travel with them |
| Kist, per person | oracle(identity key ‖ developer key ‖ option) | while the person is present and only to that kist | each installed kist's own data (AUTH.md's `app.<name>.` plan, separated by key and not only by name) |
| Kist, device | oracle(ira key ‖ developer key ‖ option) | from boot, only to that kist | a driver's or service's own state that is not the person's |

The option is ORACLE.md's: `hp` (stable across versions) by default, chosen by the developer.

## How a request flows

1. A process calls the vault server over IPC. The kernel stamps the call with what the caller is (its verified developer key and option, the ihi inputs), which the caller cannot forge.
2. The vault asks the kernel for that caller's domain key (or the common key, if the caller holds the common-area grant for that name). The kernel derives it and hands it to the vault process for that request.
3. The vault seals or opens the value in pieces (XChaCha20-Poly1305, the house sealing; each piece its own nonce from the value's address and the piece's offset), streaming to or from the disk. A value has no size limit beyond the disk.

## What moves, what stays

- **Moves to `payloads/vault`:** the on-disk layout (the `ferros_vault` crate: tracts, plows, the HAMT, the spine), write-verify-then-mirror, names, the block I/O on the vault's LU (through a block grant on that LU only), values in pieces.
- **Stays in the kernel:** the roots, key derivation and issuance, the oracle, the signature gate, the identity unlock (`SYS_UNLOCK`: it opens the person's domain keys), and stamping each call with its caller.
- **Boot:** the kernel spawns the vault process first, from a capsule it can reach without the vault (open question 0), then init and everything after it from the vault as today.
- **Migration:** the existing vault is opened by the process with the same crate. Entries move into their domains on first run: device class to the device domain, identity class to the person's common area, and each kist's own entries to its domain once kists exist.

## Open questions for the owner

0. Where the vault's own capsule lives at boot: in the boot image beside the kernel, or at a fixed place on the vault's LU the kernel reads raw.
1. Whether the kernel hands the vault a domain key per request, or opens a sealed piece straight into the caller's buffer itself, so the vault process never holds a key or the person's plaintext (stronger, more kernel work).
2. The common area's access rule: which kists may read and write which names there (the display reads `wallpaper`; the map writes it when the person picks one).

## First use

The person's wallpaper: a VSF visual of the screen's size (24 MB at 16 bits), in the person's common area as `wallpaper`. The display reads it a band of rows at a time (`wall_own`), unlocked at handle entry. Until the vault is out, the default wallpaper (made on the device from a 32-byte seed) is what shows.
