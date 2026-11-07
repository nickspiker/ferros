# The vault out of the kernel

Status: decided with the owner, 2026-11-07 (the three questions below, and the permission rule). Nothing here is built yet; the build order is at the end.

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

## Decided (owner, 2026-11-07)

0. **The vault's capsule rides with the kernel**: in the boot image on husky and akita, in the kernel's ring entry on the seed's path, embedded at build time. Whatever verifies the kernel verifies it, and the kernel's signature gate checks it again as a capsule. The kernel keeps no disk code: the vault process owns the UFS link, so `ufs_device` and `vault_boot` leave the kernel too. Kernel and vault ship and roll back together (A/B is the recovery path). Development restarts the vault process from a capsule pushed over PT, as `husky-shell` restarts the others.
1. **The kernel hands the vault a domain key; the vault encrypts.** The kernel stays out of the data path (no value in its heap, no size cap) and never hands out a root. A key is issued only against a live call from the kist whose domain it is (the kernel stamps every call with its caller, so the vault cannot ask for a kist that is not asking), and a person's keys only while their space is unlocked. Keys are bound to the unlock: at lock or release the kernel tells the vault to drop them and issues none until the next attest; the vault zeroes a key after the request it came with.
2. **The common area is named, and access is a grant per name, read and write apart.** A small documented vocabulary (`wallpaper` first; `images`, `contacts`, `theme` as they come), never a kist's private data (that is its own domain). Nothing sees the common area by default. System kists get their grants from init (the display reads `wallpaper`, settings writes it). Installed kists ask for named grants at install. The vault enforces the grant on every call. A name means the same to every kist granted it: no first-writer ownership. Write implies read.

## Permissions are asked once, ever (owner, 2026-11-07)

"If Lumis asks access to store images in the shared images location, that should only ask once, ever, for all my devices, ever ever, until I change it. 1 means 1."

- A permission is the person's answer to a kist's request (a named grant in the common area, or any other right a kist asks for). It is asked once per person, per kist, per permission.
- The answer is stored in the person's own space (identity class), so it travels with them: every device of their fleet already knows it, a new device included, and none asks again.
- Reinstalling the kist, installing it on another device, an update, a reset of the device: none of these asks again. Only the person changing it in settings changes it.
- The answer is keyed by the kist's identity as the gate sees it (its developer key and option, ORACLE.md), so a different developer's app of the same name is a different kist and is asked on its own.

## Build order

1. The vault process (`payloads/vault`): the `ferros_vault` layout, values in pieces, block I/O on its LU, the scope calls it serves today, keyed by the kernel per request.
2. The kernel: embed and spawn the vault capsule first; issue domain keys against live calls; stamp callers; drop `kvault`'s storage path, `vault_boot` and `ufs_device`.
3. Domains and the common area: per-kist keys through the oracle, the named grants, the permission answers in the person's space.
4. Migration of the existing vault, on first run.
5. First use: the person's wallpaper (below), stored once at bind.

## Built so far

- **`ferros_store` (2026-11-07):** the vault's values of any size over manifestus, no_std, host-tested.
  - A value is a head (a VSF document: length, value id, or the value inline when it fits one piece) and 64 KiB pieces. Each is sealed on its own under the domain's sealing key, with its own address as associated data.
  - Ranged reads open only the pieces they touch. Writes stream a piece at a time. Rewrites are atomic (a fresh id; the head moves in the same commit as the last piece).
  - The last head and piece opened are kept, keyed by address and sealing key.
  - Each domain keeps its own books under names callers cannot use: its list of names, and a pending list that `sweep` clears after an interrupted write.
  - Every call takes the domain's keys; nothing derives one.

## Found while mapping the kernel's vault (2026-11-07)

- **The layout is manifestus**, not the `vault/` crate (`ferros_vault`): the kernel uses `ferros_vault` only for `AnchorKey`. `ferros_vault/` at the repo root is not a crate at all (no Cargo.toml; a host-file prototype).
- **The UFS link is shared:** the flash log, the ira (devinfo on LUN 3), slot marking, partfs (the modem's partition grants), the write-protect flag and the `diag ufs` tools all use it, under the kernel lock. The vault process cannot own the link outright until those have somewhere to go.
- **The kernel reads the vault itself:** `init`, `sealed`, `device`, `boot`, `genesis`, `android.allowed`, `home.off`, `aoc.fw.*`, `cam.*`. It needs a client channel of its own to the vault process.
- **`vault_probe` stays in the kernel:** it derives the ira, puts it in the key registers and sets the oracle's system secret before anything is stored.
- **Two grant bugs:** a display restarted with `diag restart` gets no `map.` scope (`service.rs` `shell_grants` lacks it; init's list has it), and `std/probe` asks for a `photon` scope that does not exist.

## The interfaces still to settle (proposal, for the owner)

1. **Blocks, first cut:** the kernel keeps its UFS driver for now and grants the vault process a block window: its two mirror regions only, 4 KiB reads and writes by block number through a new call. The kernel moves blocks; it does not hold values, parse or seal. UFS moves out entirely later, with a block service for the flash log, the ira, slots and partfs. This defers part of decision 0 ("the kernel keeps no disk code") and needs the owner's word.
2. **Keys:** a new call, open only to the vault process. Given the call token of a request it is serving, the kernel derives and returns that request's domain keys:
   - the kist's own domain: through the oracle, from the caller's verified developer key and option, under the device or the person's key;
   - or the common area's keys, if the caller holds the grant for that name.
   It refuses for a token the vault is not serving, and for the person's domains while no one is unlocked.
3. **Stamping:** `recv` already returns a reply token. The kernel adds to it what the caller is (its kist identity, and which common names it holds), so the vault never takes a caller's word for who it is.
4. **The protocol:** the vault serves the operations processes use today (`ferros_sys::store_*`: get and get at an offset, set in pieces, delete, list names, the address-named entries kete uses), as op-byte messages like the display's. A scope's prefix becomes a domain, so existing callers keep their calls and change only whom they call.
5. **The kernel's own channel:** a client endpoint the kernel alone calls, at boot and in the service loop, for the names it reads today, served from the device domain.
6. **Migration:** the vault process opens the existing manifestus vault. Its first run reads each old entry through the kernel's old sealing (the kernel issues the old class keys once, for this) and writes it again into its domain in pieces.

## First use

The person's wallpaper (owner, 2026-11-07: "we don't want to be recomputing the wallpaper every attest"): made once, when the device binds to their fleet (on the GPU once its kernel exists; the display's four wallpaper threads until then, 5.1 s drawn and 33.4 s refined on the Pro), and stored in their space for that device (owner, 2026-11-07: per device, since each has its own size and aspect; the MacBook's is not the Pro's), a VSF visual of the screen's size (24 MB at 16 bits on the Pro). At every attest the display reads it in bands (`wall_own`); nothing is drawn again. The person changes it from settings: their own picture (tools/wallpaper converts one), or a fresh one made from a new seed.
