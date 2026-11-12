# The vault out of the kernel

Status: decided with the owner, 2026-11-07 (the three questions below, and the permission rule). Built so far is listed below; the build order is at the end.

## Why

Today the vault is the kernel's (`kvault.rs`, `vault_boot`, `ufs_device`).
Every stored byte any process keeps goes through the kernel's `STORE` call, and the kernel assembles each value whole in its heap to seal it.
That caps a value at 3 MiB (`service.rs` `STORE_VALUE_MAX`): a 24 MB wallpaper is refused, and the display server's own capsule (2.8 MB) is already near the line.
It also means the kernel parses and holds everyone's data.
(Since 2026-11-12 the kernel's own vault keeps a value over 512 KiB as sealed pieces under a manifest, `ferros_pieces`: storing one never holds it whole in the kernel heap, and a restart streams a capsule's pieces into a staging arc. The 3 MiB cap stays for the readers that still take a value whole: init's spawn and `store get`.)
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
| Kist, per person | oracle(identity key ‖ developer key ‖ option) | while the person is present and only to that kist | each stowed kist's own data (AUTH.md's `app.<name>.` plan, separated by key and not only by name) |
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
2. **The common area is named, and access is a grant per name, read and write apart.** A small documented vocabulary (`wallpaper` first; `images`, `contacts`, `theme` as they come), never a kist's private data (that is its own domain). Nothing sees the common area by default. System kists get their grants from init (the display reads `wallpaper`, settings writes it). Stowed kists ask for named grants at stow. The vault enforces the grant on every call. A name means the same to every kist granted it: no first-writer ownership. Write implies read.
   STORE.md (2026-11-11, a draft for the owner) proposes these names become kinds of the person's items, with developers minting their own.

## Permissions are asked once, ever (owner, 2026-11-07)

"If Lumis asks access to store images in the shared images location, that should only ask once, ever, for all my devices, ever ever, until I change it. 1 means 1."

- A permission is the person's answer to a kist's request (a named grant in the common area, or any other right a kist asks for). It is asked once per person, per kist, per permission.
- The answer is stored in the person's own space (identity class), so it travels with them: every device of their fleet already knows it, a new device included, and none asks again.
- Restowing the kist, stowing it on another device, an update, a reset of the device: none of these asks again. Only the person changing it in settings changes it.
- The answer is keyed by the kist's identity as the gate sees it (its developer key and option, ORACLE.md), so a different developer's app of the same name is a different kist and is asked on its own.

## Build order

1. The vault process (`payloads/vault`): the `ferros_vault` layout, values in pieces, block I/O on its LU, the scope calls it serves today, keyed by the kernel per request.
2. The kernel: embed and spawn the vault capsule first; issue domain keys against live calls; stamp callers; drop `kvault`'s storage path, `vault_boot` and `ufs_device`.
3. Domains and the common area: per-kist keys through the oracle, the named grants, the permission answers in the person's space.
4. Migration of the existing vault, on first run.
5. First use: the person's wallpaper (below), stored once at bind.

## Wear and bad blocks (owner, 2026-11-07)

- The vault keeps manifestus in plain mode on managed flash (UFS, NVMe): everything rotates (the spine's slot is `generation & (N−1)`, the tract is a log and its reap moves even cold data once a lap), so no block is rewritten in place. Ordinary filesystems leave fixed blocks that the controller's leveling cannot spread; ours has none, so the controller levels evenly, and its own bad-block remap comes with it at no cost to us.
- Relocating bad blocks ourselves (manifestus's designed, unbuilt relocation: a corrupt read falling back to the other mirror, a failed write-verify landing elsewhere) is left as a future option, not built now. The migrating root ring and a bad-block tombstone map belong to raw flash with no controller (PIPE, a raw SD), when we get there.

## Built so far

- **`ferros_store` (2026-11-07):** the vault's values of any size over manifestus, no_std, host-tested.
  - A value is a head (a VSF document: length, value id, or the value inline when it fits one piece) and 64 KiB pieces. Each is sealed on its own under the domain's sealing key, with its own address as associated data.
  - Ranged reads open only the pieces they touch. Writes stream a piece at a time. Rewrites are atomic (a fresh id; the head moves in the same commit as the last piece).
  - The last head and piece opened are kept, keyed by address and sealing key.
  - Each domain keeps its own books under names callers cannot use: its list of names, and a pending list that `sweep` clears after an interrupted write.
  - Every call takes the domain's keys; nothing derives one.
- **The block window and the vault process (2026-11-07), proven on the Pro:**
  - The kernel's `SYS_BLOCK` (29): a `Blocks` capability names a window of the ferros partition (`process.rs` `WINDOWS`), and the call moves 1 to 256 blocks of 4 KiB by number within it, as one UFS command (a run: ferros_hal `read_run_lun`, the PRDT's entry stride probed once, a 4 KiB entry a block as Linux's ufs-exynos sends them, `diag "ufs runs"`); 32 from 2026-11-08 to 2026-11-11, and 16 before, each its own command. The run lands in a 1 MiB bounce in UFS's own DMA pages, the only cacheable ones (each command cleans it before the doorbell and cleans and invalidates it after), and is copied to the caller. The kernel checks the window and moves bytes; it does not hold, parse or seal them. `bridge spawn --blocks vault` grants it.
  - A transfer (`SYS_STORE`'s one call, process.rs `STORE_MAX`) is 256 KiB since 2026-11-08 (4 KiB before), and a get forwarded to the vault process up to 1 MiB since 2026-11-11 (`STORE_GET_MAX`), copied from the vault process's buffer straight into the caller's. Its pieces (up to sixteen of `ferros_store`'s 64 KiB) are fetched by the serving thread and handed to three helper threads one at a time as each arrives, so they open while the next is fetched (`Store::read_with`, the streamed `Opener`, payloads/vault `Helpers`); the request's sealing key is on their board only while its batch opens.
  - The read-ahead (2026-11-11): a large value's blocks lie in one stretch of the tract (each piece's furrows, then its leaf), so while a get of a transfer or more is served a furrow run the window does not hold fills it with 256 blocks in one command, and the leaves and runs after it are copies. Every write drops it; every block still meets its BLAKE3 guard and every piece its seal.
  - A large read says where its time went on the console: `vault: read …` (serving split into the fetch, the block reads, the opening left after them, the copy out, and the time between requests) and `vault: read blocks …` (the kernel's own split of the block calls: in the UFS commands, its copies, the rest of the calls; and what the read-ahead gave). `diag "ufs runs"` splits every UFS command into its cache upkeep and the device's time.
  - `vault`: 2 × 8 GiB from 4 GiB into the partition (past the flash log at 1 GiB and the kernel's vault at 2 to 2.5 GiB).
  - `payloads/vault` opens manifestus over the window (mirror A from block 0, mirror B from half way, each 512 MiB to start) and serves `ferros_store`. In this first cut its keys were fixed, non-secret test domains; the next entry replaced them.
  - `payloads/vaulttest` is its client. On the Pro:
    - a blank region opens in 21 s (read whole once, before the first commit), and a committed one reopens in 6 ms;
    - 1 MiB is written in 154 ms (4000-byte messages) and read back in 45 ms, the same by BLAKE3;
    - names follow the puts and the delete, domain 2 finds nothing of domain 1, and sweep runs;
    - after the vault process is killed and spawned again, the 1 MiB value reads back whole (50 ms).
- **Every store call goes to the vault process, keyed by the kernel (2026-11-07), proven on the Pro:**
  - **Forwarding.** While a live process holds the right to serve `vault`, the kernel forwards every `SYS_STORE` call there as an IPC call from the caller (`process.rs` `store_forward`). The caller waits as for any call; no process changes its calls. With no vault process, the kernel's vault answers, as before.
  - **The stamp.** The kernel stamps each call with its scope's domain keys, one domain per (scope, class):
    - the device's keys derive from the ira;
    - the person's derive from their unlock, and exist only while they are unlocked (kvault `identity_domain`, wiped at lock);
    - a listing of a scope that holds both classes carries both;
    - the lock epoch rides along, and when it moves the vault drops its cache's plaintext and any write of the person's in progress.
  - **Keys never outlive their request.** The kernel wipes each message once delivered; the vault zeroes the keys after the request; the store's cache knows a key only by its hash.
  - **The endpoint is the kernel's.** Nothing may be granted `use vault`. Serving it needs the vault right (`Grant::Vault`, `bridge spawn --vault`). A call made while the vault opens waits for it.
  - **Migration.** The vault right's call (`SYS_VAULT`, 30) hands the vault each old entry of a scope with its domain keys (the one exception to keys only against a live call). The vault writes the entry into its domain and, once committed, the kernel removes it from its own vault. It runs before the first request after anything may have left old entries: boot, a bridge `store put` of a scoped name, a write while no vault served, an unlock. Names the kernel reads itself (`aoc.fw.*`) stay in the kernel's vault.
  - **Boot.** The kernel starts the vault before init (`start_home`; init's grants page is full) from the capsule `vault` in its own vault. `diag restart vault` restarts it.
  - **Measured.** 21 device entries (the WiFi, GPU and GPS files and the AoC registry scripts, 2.98 MB) moved in 594 ms. `payloads/vaulttest`, through the ordinary store calls on a `test` scope:
    - a 6 MiB value (past the old 3 MiB limit) written in 843 ms and read back in 648 ms, the same by BLAKE3;
    - names, delete and absent all hold.
  - **After a reboot.** The vault opens in 139 ms; the GPU boots from `gpu.fw` and the sensors process loads its 15 scripts, both from the vault.
- **The kernel's own channel, and the bridge's `store` commands through it (2026-11-07, built, not yet on the device):**
  - The kernel calls the `vault` endpoint as a caller with no thread behind it (`process.rs` `vault_call`): queued and delivered as any call, stamped as `store_forward` stamps (the shared `stamp`), one call at a time. The vault's reply, or an error should the vault end first (`fail_callers`, `fail_orphaned_calls`), lands in a mailbox the service loop reads (`vault_answer`), with an event to wake it.
  - A bridge `store` command for a name the vault process holds (a scope's, not one the kernel keeps) becomes a job in the service loop (`service.rs` `VaultJob`): a get read a transfer at a time, a put written in pieces from the command (which stays with the job: ptlink takes no other command until its reply is taken), a remove; `ls` lists the kernel's vault, then every scope from the vault process, each name made whole with its prefix. The PT reply waits for the job's last call; the USB process already polls for a reply not ready.
  - A get or a remove the vault process does not find goes to the kernel's vault, where a name not yet moved still is. A put to the vault process takes up to the 6 MiB one command carries; a get is held to 3 MiB (the reply is built whole in the kernel's heap and copied once more on its way out). The person's names need them unlocked, as before.
  - With no vault process serving, every command is the kernel's vault's, as before.
- **Release drops the person's domains in the vault process (2026-11-07, built, not yet on the device):**
  - At release (`SYS_UNLOCK` op 4, a completed departure, or `diag "identity release"`) the kernel stamps a drop of each of the person's domains (each scope that holds the person's names: `map.`, `gps.`, `wifi.`, `fgtw.`, `ahau.`) while their keys still exist, then releases the kernel's vault as before (`process.rs` `vault_stamp_drops`).
  - The service loop sends the drops through the kernel's channel, a call a pass, before its restart. The vault process drops each domain whole (`ferros_store` `drop_domain`: every value, the pieces of unfinished writes, the books; host-tested to leave the disk as it was before the domain's first write).
  - A drop that fails, or no vault process to send it to, is logged and the restart goes ahead: the renewed device-only secret already makes those entries unopenable. Released while locked, nothing can be stamped, and the same holds.
  - `diag "identity release"` now takes the kernel lock (it wrote the vault outside it).
- **The identity export through the vault process (2026-11-07, built, not yet on the device):**
  - `SYS_UNLOCK` op 5, while the vault process serves, is forwarded to it as a call from the caller (`process.rs` `export_forward`), stamped with the person's domain in every scope that holds their names less the identity service's own (`kvault::EXPORT_SKIP`: `fgtw.`, `ahau.`, as the kernel's export skipped them) and each scope's prefix. The vault builds the same document the kernel did (section `identity_export`, an `entry` per name: the full name, the bytes) at offset 0 and hands it out a transfer at a time; it is wiped when read to its end or when the person locks.
  - The op gains an offset (x4); `ferros_sys::export_identity` reads in transfers, with the same signature. Without a vault process the kernel keeps its own export between the reads in the same way (each build stamps its own time, so pieces of two builds would not be one document).
  - Bounded, because the document is built whole in the vault's 8 MiB heap: a value over 1 MiB is left out and named on the console (the person's made wallpaper, `map.wallpaper.made`, 24 MB, is this screen's alone), and an export over 4 MiB is refused (`ferros_store` `export`, host-tested).
  - Nothing calls op 5 yet (the Fleet page's "save first" is to come).
- **The vault's capsule in the kernel image (2026-11-07, built, optional, not yet on the device):**
  - `FERROS_VAULT_CAPSULE=<signed capsule>` at the kernel's build embeds it (`build.rs`, `service.rs` `VAULT_CARRIED`). At home the kernel starts the vault from it, through the signature gate as any capsule; refused or absent, from the `vault` entry of its own vault as before. `diag restart vault` takes the stored capsule first (the development loop pushes one), the carried one when none is stored.
  - Unset, the kernel is as before. Set, the image grows by the capsule (about 450 KB: 1.74 to 2.20 MB).
  - The stored copy goes once a slot b cold boot has started the vault from the image (decision 0's "boot path last").
- **Still to do:**
  - the person's entries moving at the first attest (built, not yet seen on the device);
  - the common area's named grants (step 3's rest; per-kist domains exist since 2026-11-08: `Object::Domain`, KISTS.md "Built", keyed from the kist's ihi under the person's key);
  - the kernel's own names moved behind a channel of its own, then `kvault`, `vault_boot` and `ufs_device` retired (step 4).

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

The person's wallpaper (owner, 2026-11-07: "we don't want to be recomputing the wallpaper every attest"): made once, when the device binds to their fleet (by wallgen on the GPU since 2026-11-11, GPU-KISTS.md plan step 0; the display's four wallpaper threads took 5.1 s to draw it and 33.4 s to refine it on the Pro, and the display draws none now), and stored in their space for that device (owner, 2026-11-07: per device, since each has its own size and aspect; the MacBook's is not the Pro's), a VSF visual of the screen's size (24 MB at 16 bits on the Pro). At every attest the display reads it in bands (`wall_own`); nothing is drawn again. The person changes it from settings: their own picture (tools/wallpaper converts one), or a fresh one made from a new seed.
