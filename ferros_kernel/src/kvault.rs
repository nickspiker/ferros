//! The kernel's vault on manifestus (the owner's storage engine, shared with Photon): two mirror regions of the ferros partition as manifestus BlockDevs, and a thin keyed, sealed layer on top.
//!
//! Replaces ferros_vault's single-file Phase-1 store in the kernel (2026-10-20), whose 64 KiB payload and key-derived anchor slots let a large object overwrite the anchors and roll the vault back across a reboot (pinned with the flash log).
//!
//! Layout (ferros partition, LU 0, 4 KiB blocks): mirror A at +2 GiB, mirror B at +2 GiB + 256 MiB, 65536 blocks each. The flash log lives at +1 GiB; the old ferros_vault file at the partition start is left untouched.
//! Names: a logical name maps to the 32-byte manifestus address BLAKE3-keyed(addr key, name); values are XChaCha20-Poly1305 sealed (24-byte nonce in front, tag behind) under a payload key, and the envelope stored as a VSF document (`ferros_pieces::kv`, section `vault_seal`). Both keys derive from the ira through ferros_vault's AnchorKey, as before. manifestus is non-enumerable by design, so the kernel keeps its own list of names (sealed, under a reserved name) for `store ls`.
//! The vault's own values (the name lists, the address lists, the identity's secret, kept seed and bound mark, the genesis value) are each a VSF document too (`ferros_pieces::kv`; docs/VSF-AUDIT.md P11 to P13, 2026-11-17).

use alloc::string::String;
use alloc::vec::Vec;
use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use ferros_hal::ufs::UfsController;
use ferros_vault::crypto::NonceSource;
use manifestus::{Block, BlockDev, DeletePolicy, Mirror, Vault};

const UFS_BASE: usize = 0x1320_0000;
const REGION_BLOCKS: u64 = 65_536;
/// Mirror A's offset in the ferros partition (2 GiB in); B follows it.
const MIRROR_OFFSET: u64 = 524_288;
/// The two mirror regions' first LBAs, in the ferros partition as the GPT names it (`ufs_device::part`): none without it, or when it is too small to hold both (the vault is refused then, never written past the partition's end).
fn mirrors() -> Result<(u64, u64), KvError> {
    match crate::ufs_device::part() {
        Some((first, blocks)) if blocks as u64 >= MIRROR_OFFSET + 2 * REGION_BLOCKS => Ok((first as u64 + MIRROR_OFFSET, first as u64 + MIRROR_OFFSET + REGION_BLOCKS)),
        _ => Err(KvError::NoPartition),
    }
}
const RING_LOG2: u8 = manifestus::HOST_RING_LOG2;
const ADDR_CONTEXT: &str = "ferros.kernel.store.addr.v0";
const PAYLOAD_CONTEXT: &str = "ferros.kernel.store.payload.v0";
/// The kernel's list of names (a `names` document, `ferros_pieces::kv`), itself stored under this name. Every name starting with NUL is the kernel's own (the list, the selftest's scratch): never listed, never indexed, so `store ls` shows only what a caller stored.
const NAMES: &str = "\0names";
fn reserved(name: &str) -> bool {
    name.starts_with('\0')
}
/// The genesis value (vault_boot.rs, a `genesis` document, `ferros_pieces::kv`): read back at every boot to show the vault opens under this ira.
pub const GENESIS: &str = "genesis";
/// The seals of the installed masked images (service.rs; a `seals` document, ferros_kist::seal `list_document`).
pub const SEALED: &str = "sealed";

/// The identity class (AUTH.md, owner decisions 2026-10-31): entries under these prefixes are the person's, not the device's: the map's arrangement, the identity service's chain and key (payloads/fgtw), the attest screen's own, the WiFi credential (networks travel with the identity; the per-network MAC stays the device's), and the GPS driver's last fix (owner, 2026-11-04: where the device has been is the person's, and no GPS runs while nobody has attested; the replays it aids from keep no place, gps scrub). They are sealed under a key bound to this device and to the person, held only while an identity is unlocked: before attest none of them can be read, their names included. One identity per device: another person's handle is refused once the device is bound, and wipes an earlier attempt's leftovers while it is not. Since 2026-11-12 also the notes kists have waiting on the person (process notify.rs, docs/NOTIFY.md), under a reserved name no process scope reaches: who wrote to the person and what about is theirs.
const IDENTITY: [&str; 7] = ["map.", "fgtw.", "ahau.", "wifi.cred", "gps.last", "kists.", "\0notes."];
pub fn identity_class(name: &str) -> bool {
    IDENTITY.iter().any(|p| name.starts_with(p))
}
/// The identity service's own scopes (its roots, its fleet chain, the attest screen's verifier), never in the person's export: it holds what the export is sealed into.
pub const EXPORT_SKIP: [&str; 2] = ["fgtw.", "ahau."];
/// Whether a scope (its prefix) holds any identity-class names: a listing of it covers the person's domain as well as the device's.
pub fn identity_within(prefix: &str) -> bool {
    IDENTITY.iter().any(|p| p.starts_with(prefix) || prefix.starts_with(p))
}
/// The device-only secret mixed into every identity key: 32 bytes from this session's wairua the first time an identity is unlocked, kept in the device class. Google has no record of it and Android cannot read the partition it is on (the hypervisor's net, AUTH.md), so the identity key needs more than the chip's published values and a guessed handle.
const ID_SECRET: &str = "\0id.secret";
/// The seed kept for unlock at boot (`device.unlock_at_boot`, SETTINGS.md): only while that setting is on, in the device class, deleted at the first boot with it off. Whoever holds the device then holds the identity — the warning the setting carries.
const ID_UNLOCK: &str = "\0id.unlock";
/// The identity class's own list of names, sealed under the identity key.
const ID_NAMES: &str = "\0id.names";
/// Whether an identity is unlocked now (a person is present): the vault's own `id`, mirrored lock-free for SYS_FACTS 83, which any process may ask, so the radios' drivers can refuse while locked without holding the unlock right (AUTH.md "Radios while locked").
pub static PERSON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// One identity per device (AUTH.md; the network owns the assignment): set when the unlocked identity's fleet binds this device, cleared on release (a completed departure). While set, only that identity unlocks.
const ID_BOUND: &str = "\0id.bound";
/// Every identity-class entry's vault address, 32 bytes each, in the device class: opaque (no names, no contents, nothing but a count), so the identity class can be wiped whole without its key — on release, and when a different handle unlocks a device that is not bound yet.
const ID_ADDRS: &str = "\0id.addrs";
const ID_CONTEXT: &str = "ferros identity vault v1";
const ID_ADDR_CONTEXT: &str = "ferros identity vault addr v1";
const ID_PAYLOAD_CONTEXT: &str = "ferros identity vault payload v1";
/// The base the kernel derives the person's vault domains from (process.rs `domain_keys`, VAULT-OUT.md): one per unlock, wiped at lock.
const ID_DOMAIN_CONTEXT: &str = "ferros 2026-11-07 identity vault domain";
const ID_SECRET_CONTEXT: &str = "ferros identity secret v1";
/// The person's ledger key, from the identity root as the vault keys are (`IdKeys.ledger`).
const ID_LEDGER_CONTEXT: &str = "ferros 2026-11-17 identity ledger v0";
/// Attest with a tag (AUTH.md "Attest with a tag"; owner, 2026-11-05: a plain NTAG-type tag in his glasses opens the device without the handle typed, and nothing is remembered without the tag). One record per kept tag, in the device class, kept by its id (a hash of its UID, never the UID) as an address under this reserved prefix (`set_addr`), so the address list is the list of kept tags and nothing names them.
const TAGS: &str = "\0tag.";
/// The ira-derived device key a tag's id and seal key are made from (`tag_keys`): derived in the kernel, the ira never handed out (keys in registers).
const TAG_DEVICE_CONTEXT: &str = "ferros 2026-11-11 tag unlock device";
/// A tag's id: BLAKE3 derive_key over (the device key ‖ the UID). Keyed by the device, so the id says nothing of the UID to anyone without this device's ira, and differs on every device. The record's address and its seal's associated data.
const TAG_ID_CONTEXT: &str = "ferros 2026-11-11 tag unlock id v0";
/// A tag's seal key: BLAKE3 derive_key over (the device key ‖ the device-only secret ‖ the UID). The secret too, as the identity key takes it, so the chips' published values and the tag are not enough without this device's own 32 bytes, and a release (which renews the secret) leaves any record it missed unopenable.
const TAG_SEAL_CONTEXT: &str = "ferros 2026-11-11 tag unlock v0";
/// A tag record (VSF, its own section): `added`, when it was kept (Eagle time), and `seal`, the identity seed sealed with XChaCha20-Poly1305 under the tag's seal key (a fresh 24-byte nonce in front, the 16-byte tag behind, the tag's id as associated data).
const TAG_SECTION: &str = "tag_unlock";
const TAG_ADDED: &str = "added";
const TAG_SEAL: &str = "seal";
/// A sealed seed's length: nonce, seed, Poly1305 tag.
const TAG_SEALED: usize = 24 + 32 + 16;
/// How many bytes of a tag's id the list shows and a forget names: 64 bits, enough to tell a person's few tags apart, and what settings shows.
pub const TAG_SHOWN: usize = 8;

/// Which keys an entry is sealed under.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Device,
    Identity,
}
fn class_of(name: &str) -> Class {
    if identity_class(name) { Class::Identity } else { Class::Device }
}

/// The unlocked identity's keys, each held sealed under the wairua (keys.rs `Held`; KEY_REGISTERS.md "Sealed under the wairua at rest", owner 2026-11-17): opened only in registers, by the lifted code, never stored in the clear.
/// The address key its names hash under, the key its values seal under, the base of its vault domains, the seed, the person key.
struct IdKeys {
    addr_key: crate::keys::Held,
    /// Opened into memory for each seal or open of an identity-class value (`cipher_of`): XChaCha20-Poly1305 does not run in registers yet.
    payload_key: crate::keys::Held,
    /// The base of the person's vault domains (the vault process's, VAULT-OUT.md).
    domain: crate::keys::Held,
    /// The identity seed itself, held only while the person is present so a tag can be kept from the settings page (`tag_keep`), which never sees the seed.
    /// Wiped with the keys at `lock`.
    seed: crate::keys::Held,
    /// The person's ledger key (LEDGER.md "Classes"): personal entries of the flash log are sealed under it, so they open only while this person is present (flashlog.rs `set_personal`).
    ledger: crate::keys::Held,
    /// The person key (FACETS.md D0, D1): handed in by the identity service once it opens it from the fleet's space (SYS_UNLOCK op 10), the root every kist's alias is derived from here (`alias`), with its one-way id (`PERSON_ID_CONTEXT`), which tells a key handed in again from a new one.
    /// None until then; wiped with the keys at `lock`.
    person: Option<(crate::keys::Held, [u8; 32])>,
}

/// The person key's one-way id in the kernel, for telling the same key handed in again from another (`set_person`), never shown.
const PERSON_ID_CONTEXT: &str = "ferros 2026-11-17 person key id in the kernel";

/// A kist's alias (FACETS.md "The alias"): BLAKE3 derive_key under this context over the person key, the kist's developer key and its option, each 32 bytes. The same bytes as `fgtw::alias::seed`, whose known answer `alias_known_answer` checks here.
const ALIAS_CONTEXT: &str = "ferros 2026-11-12 kist alias v0";

/// The alias seed of the kist (`developer`, `option`) under `person`, in software: the known answer's check of the derivation alone (`alias_known_answer`), never a person's key; the person's aliases are derived from the held key in registers (`KVault::alias`).
fn alias_seed(person: &[u8; 32], developer: &[u8; 32], option: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key(ALIAS_CONTEXT);
    h.update(person);
    h.update(developer);
    h.update(option);
    let seed = *h.finalize().as_bytes();
    // The hasher's buffer holds the person key.
    crate::keys::wipe_value_to_dram(&mut h);
    seed
}

/// Whether this kernel's derivation gives fgtw's known answer (fgtw src/alias.rs `seed_known_answer`: the 96 bytes 0 to 95), for `diag identity`.
pub fn alias_known_answer() -> bool {
    let mut f = [[0u8; 32]; 3];
    for (k, b) in f.iter_mut().flatten().enumerate() {
        *b = k as u8;
    }
    let want = [
        0xaa, 0x02, 0xea, 0x59, 0x45, 0xdb, 0x02, 0x07, 0x81, 0x6b, 0xc2, 0xd1, 0x9e, 0xf5, 0x80, 0x8d, 0x2a, 0x70, 0x00, 0xbe, 0x7c, 0xe0, 0xa4, 0xdb, 0x91, 0x13, 0xbc, 0xe8, 0x14, 0x8c, 0x37, 0x7f,
    ];
    // The register-only path too: the same answer from the test person key held and derived in registers (a kernel without the wairua held answers false).
    let mut m = [0u8; 64];
    m[..32].copy_from_slice(&f[1]);
    m[32..].copy_from_slice(&f[2]);
    let held = crate::keys::hold(&mut f[0].clone()).and_then(|h| crate::keys::held_derive(&h, ALIAS_CONTEXT, &m));
    alias_seed(&f[0], &f[1], &f[2]) == want && held == Some(want)
}

/// Block I/O counters for `selftest`.
static READS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static WRITES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// One mirror region: `blocks` 4 KiB blocks of LU 0 from `base`.
pub struct Region {
    base: u64,
    blocks: u64,
}

/// Block operations since the last feed of the watchdogs (`feed`).
static OPS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Feed the watchdogs every 2^6 block operations of the kernel vault: even at a millisecond a block that is a feed every 64 ms, far inside their one-second window (platform.rs WDT_COUNT).
fn feed() {
    if OPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) & ((1 << 6) - 1) == 0 {
        crate::platform::pet_watchdogs();
    }
}

/// Every block of the vault is read and written holding `VAULT` once the vault is shared (process.rs `VAULT_SHARED`): the vault's state is one owner's at a time, and a path that reached a block without it is a missed one, said here with a panic the first time it runs rather than left a rare race.
/// And each command holding `UFS`, the controller's one command at a time with its data block, taken for the command and its copy only, so another core's flash log line or vault process's run waits one block at most.
fn vault_held() {
    if crate::process::VAULT_SHARED.load(core::sync::atomic::Ordering::Relaxed) && !crate::process::VAULT.mine() {
        panic!("kvault: a block reached without the vault lock");
    }
}

impl BlockDev for Region {
    fn block_count(&self) -> u64 {
        self.blocks
    }
    fn read(&mut self, lba: u64, buf: &mut Block) -> manifestus::Result<()> {
        if lba >= self.blocks {
            return Err(manifestus::Error::Bounds(lba));
        }
        vault_held();
        // The watchdogs fed every 2^6 block reads or writes, by count (`feed`): a loop over many values (a list's deletes, an export, manifestus's whole scan of a blank pair) never goes a second unfed, wherever its blocks lie.
        // Until 2026-11-18 only a read whose block number was a multiple of 1024 fed them and writes never did, so a long run of scattered reads and writes went unfed (VSF-AUDIT wave 0, the converter build that did not boot on the Pro).
        feed();
        READS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let ok = crate::process::UFS.with(|| {
            let u = UfsController::new(UFS_BASE);
            if u.read_block_lun(0, (self.base + lba) as u32) != 0 || u.last_response_status() != 0 {
                return false;
            }
            buf.copy_from_slice(u.data_buffer());
            true
        });
        if !ok {
            return Err(manifestus::Error::Corrupt(alloc::format!("UFS read failed at region block {lba}")));
        }
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &Block) -> manifestus::Result<()> {
        if lba >= self.blocks {
            return Err(manifestus::Error::Bounds(lba));
        }
        vault_held();
        feed();
        WRITES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let ok = crate::process::UFS.with(|| {
            let u = UfsController::new(UFS_BASE);
            u.data_buffer_mut().copy_from_slice(buf);
            u.write_block_lun(0, (self.base + lba) as u32) == 0 && u.last_response_status() == 0
        });
        if !ok {
            return Err(manifestus::Error::Corrupt(alloc::format!("UFS write failed at region block {lba}")));
        }
        Ok(())
    }
    /// Each write is polled to completion by the HAL (OCS), as the old UfsDevice relied on.
    fn flush(&mut self) -> manifestus::Result<()> {
        Ok(())
    }
}

/// manifestus's storage events (verify retries, relocations, reap windows, averted laps, a degraded mirror) go into the flash log.
struct LogSink;
impl manifestus::EventSink for LogSink {
    fn emit(&mut self, e: manifestus::StorageEvent) {
        crate::flashlog::line(&alloc::format!("manifestus event: {e:?}"));
    }
}

/// The time every commit, document and envelope is stamped with: Eagle time in oscillations from the device's clock (settings.rs `eagle_now`), as manifestus's spine and VSF's `e` take it.
/// Until 2026-11-17 this was seconds since the boot, stamped as if it were Eagle time (docs/VSF-AUDIT.md P10).
/// The clock is read before the vault opens (main.rs, `rtc::sync_early`); if that read failed it is 0, the Eagle epoch, the value every reader of ferros's documents already takes as "no clock" (settings.rs): never a made-up time. manifestus records the time and never compares it (its spine asks nothing of it), so a commit stamped 0 between two stamped with the clock is harmless.
fn now() -> i64 {
    crate::settings::eagle_now()
}

#[derive(Debug)]
pub enum KvError {
    Engine(manifestus::Error),
    Seal,
    Open,
    BadNames,
    /// An identity-class entry while no identity is unlocked (AUTH.md).
    Locked,
    /// No ira this boot, so no identity key can be derived.
    NoIra,
    /// The device is bound to another identity: this seed does not open its space.
    Belongs,
    /// No ferros partition found in the GPT, or one too small for the two mirrors.
    NoPartition,
    /// A value kept in pieces whose manifest or pieces are not what they should be (`ferros_pieces::Error::Corrupt`).
    Pieces(&'static str),
    /// One of the vault's own values is not its document (`ferros_pieces::kv`): which. Never read as absent, so nothing is written over it.
    Document(&'static str),
}
impl From<manifestus::Error> for KvError {
    fn from(e: manifestus::Error) -> Self {
        KvError::Engine(e)
    }
}
impl From<ferros_pieces::Error<KvError>> for KvError {
    fn from(e: ferros_pieces::Error<KvError>) -> Self {
        match e {
            ferros_pieces::Error::Store(e) => e,
            ferros_pieces::Error::Corrupt(why) => KvError::Pieces(why),
        }
    }
}

/// The vault as `ferros_pieces` keeps a large value in it: each piece and the manifest at its own address (`ferros_pieces::Place`, under a key derived from the class's address key), sealed under the class's key with that address bound in, so a piece moved to another address does not open. An identity-class name's pieces are tracked as its whole value is (`ID_ADDRS`), so a wipe without the key takes them too.
struct Pieces<'a> {
    kv: &'a mut KVault,
    class: Class,
}

impl ferros_pieces::Store for Pieces<'_> {
    type Error = KvError;
    fn get(&mut self, addr: &[u8; 32]) -> Result<Option<Vec<u8>>, KvError> {
        match self.kv.vault.get(addr)? {
            Some(s) => Ok(Some(self.kv.unseal(self.class, s, addr)?)),
            None => Ok(None),
        }
    }
    fn put(&mut self, addr: &[u8; 32], value: &[u8]) -> Result<(), KvError> {
        let sealed = self.kv.seal(self.class, value, addr)?;
        self.kv.vault.put(addr, &sealed, now())?;
        if self.class == Class::Identity {
            self.kv.track(addr, true)?;
        }
        Ok(())
    }
    fn delete(&mut self, addr: &[u8; 32]) -> Result<bool, KvError> {
        let existed = self.kv.vault.delete(addr, now())?;
        if existed && self.class == Class::Identity {
            self.kv.track(addr, false)?;
        }
        Ok(existed)
    }
}

pub struct KVault {
    vault: Vault<Region, Region>,
    addr_key: [u8; 32],
    cipher: XChaCha20Poly1305,
    nonce: crate::wairua::TrngNonce,
    /// The unlocked identity, while one is: dropped when power goes or the device switches to another OS (`lock`).
    /// Written only holding both the kernel lock and the vault's (process.rs `VAULT`), so read holding either (service.rs `SPAWN_STORE`): the domain keys and a kist's alias read it under the kernel lock alone.
    id: Option<IdKeys>,
    /// Whether an identity's fleet has bound this device, once read (`bound`), kept in step by `bind` and `release`, the only writers of the mark: init asks 64 times a second while it waits for a person, and each read of the mark from the flash held the kernel lock about 300 us (2026-11-16, `diag lock`).
    /// As `id`: written holding both locks, read holding either (`bound_known`, init's poll under the kernel lock alone).
    bound: Option<bool>,
    /// Mirror A's first LBA, as `open` found it (`mirrors`).
    mirror_a: u64,
}

impl KVault {
    /// Open the kernel vault (manifestus genesises it on a blank pair of regions and refuses to format over sealed blocks it cannot bootstrap).
    pub fn open(ira: &[u8; 32]) -> Result<Self, KvError> {
        let base = ferros_vault::anchor::AnchorKey::from_ira(ira);
        let addr_key = ferros_vault::anchor::AnchorKey::derive(ADDR_CONTEXT, &base.0);
        let payload_key = ferros_vault::anchor::AnchorKey::derive(PAYLOAD_CONTEXT, &base.0);
        let (a, b) = mirrors()?;
        let mirror = Mirror::new(Region { base: a, blocks: REGION_BLOCKS }, Region { base: b, blocks: REGION_BLOCKS }).with_events(alloc::boxed::Box::new(LogSink));
        let vault = Vault::open(mirror, RING_LOG2, now(), DeletePolicy::UnlinkOnly)?;
        Ok(KVault { vault, addr_key, cipher: XChaCha20Poly1305::new(Key::from_slice(&payload_key)), nonce: crate::wairua::TrngNonce::new(), id: None, bound: None, mirror_a: a })
    }

    /// The identity's held keys, while one is unlocked.
    fn id_keys(&self) -> Result<&IdKeys, KvError> {
        self.id.as_ref().ok_or(KvError::Locked)
    }

    /// `class`'s keyed hash of `msg` under its address key: the device's in memory (device class, from the ira at boot), the identity's held one in registers (`keys::held_keyed`).
    fn addr_hash(&self, class: Class, msg: &[u8]) -> Result<[u8; 32], KvError> {
        match class {
            Class::Device => Ok(*blake3::keyed_hash(&self.addr_key, msg).as_bytes()),
            Class::Identity => crate::keys::held_keyed(&self.id_keys()?.addr_key, msg).ok_or(KvError::NoIra),
        }
    }

    /// Where `name`'s pieces live in `class` (`ferros_pieces::Place`).
    /// The identity's place key is derived from its held address key in registers and then lies in memory for the one vault operation it is made for (KEY_REGISTERS.md: memory-touching); the caller wipes it (`Place::wipe`).
    fn place<'a>(&self, class: Class, name: &'a str) -> Result<ferros_pieces::Place<'a>, KvError> {
        match class {
            Class::Device => Ok(ferros_pieces::Place::new(&self.addr_key, name.as_bytes())),
            Class::Identity => {
                let mut k = crate::keys::held_derive(&self.id_keys()?.addr_key, ferros_pieces::KEY_CONTEXT, &[]).ok_or(KvError::NoIra)?;
                let p = ferros_pieces::Place::from_key(&k, name.as_bytes());
                crate::keys::wipe_to_dram(&mut k);
                Ok(p)
            }
        }
    }

    /// The cipher for `class`.
    /// The identity's is made from its held payload key opened into memory (XChaCha20-Poly1305 does not run in registers yet): the caller wipes it once its one seal or open is done (`keys::wipe_value_to_dram`).
    fn cipher_of(&self, class: Class) -> Result<XChaCha20Poly1305, KvError> {
        match class {
            Class::Device => Ok(self.cipher.clone()),
            Class::Identity => {
                let mut k = crate::keys::held_open(&self.id_keys()?.payload_key).ok_or(KvError::NoIra)?;
                let c = XChaCha20Poly1305::new(Key::from_slice(&k));
                crate::keys::wipe_to_dram(&mut k);
                Ok(c)
            }
        }
    }

    fn addr(&self, class: Class, name: &str) -> Result<[u8; 32], KvError> {
        self.addr_hash(class, name.as_bytes())
    }

    /// Sealed under `class`'s key with `ad` bound in (associated data: empty for a name's own value, its address for a piece, `ferros_pieces`).
    fn seal(&mut self, class: Class, plain: &[u8], ad: &[u8]) -> Result<Vec<u8>, KvError> {
        let n = self.nonce.next_nonce();
        let mut cipher = self.cipher_of(class)?;
        // One allocation the size of the result: the nonce, the plaintext encrypted where it lies, then the tag — the same bytes `encrypt` gives. Sealing through `encrypt` into a growing Vec held two copies of the value at once (and a third while the Vec grew), and a 2.78 MB display capsule ran the 12 MiB heap out (2026-10-26, panic "memory allocation … failed" in the crumbs).
        // Its document below copies the envelope twice more (the section, then the file); a value past `ferros_pieces::PIECE` (512 KiB) is sealed a piece at a time (`set`), so that is at most a few piece-sized blocks at once.
        let mut out = Vec::with_capacity(24 + plain.len() + 16);
        out.extend_from_slice(&n[..]);
        out.extend_from_slice(plain);
        let tag = cipher.encrypt_in_place_detached(XNonce::from_slice(&n), ad, &mut out[24..]);
        // The identity's key, opened for this one seal (`cipher_of`), wiped now; the device's is a clone of the one kept.
        crate::keys::wipe_value_to_dram(&mut cipher);
        let tag = tag.map_err(|_| KvError::Seal)?;
        out.extend_from_slice(&tag);
        // The envelope as a VSF document (P13): its bytes moved into the field, the document built around them.
        ferros_pieces::kv::sealed_document(out, now()).ok_or(KvError::Seal)
    }

    /// Opened where it lies, as `seal` closes: the envelope taken out of its document, the nonce taken off its front, the rest decrypted in the same buffer and its tag dropped.
    fn unseal(&self, class: Class, doc: Vec<u8>, ad: &[u8]) -> Result<Vec<u8>, KvError> {
        let mut sealed = ferros_pieces::kv::envelope_of(&doc).ok_or(KvError::Open)?;
        drop(doc);
        let mut cipher = self.cipher_of(class)?;
        let mut n = [0u8; 24];
        n.copy_from_slice(&sealed[..24]);
        sealed.drain(..24);
        let opened = cipher.decrypt_in_place(XNonce::from_slice(&n), ad, &mut sealed);
        crate::keys::wipe_value_to_dram(&mut cipher);
        opened.map_err(|_| KvError::Open)?;
        Ok(sealed)
    }

    fn raw_get_in(&mut self, class: Class, name: &str) -> Result<Option<Vec<u8>>, KvError> {
        let a = self.addr(class, name)?;
        match self.vault.get(&a)? {
            Some(s) => Ok(Some(self.unseal(class, s, b"")?)),
            None => Ok(None),
        }
    }

    fn raw_put_in(&mut self, class: Class, name: &str, value: &[u8]) -> Result<[u8; 32], KvError> {
        let a = self.addr(class, name)?;
        let sealed = self.seal(class, value, b"")?;
        self.vault.put(&a, &sealed, now())?;
        if class == Class::Identity {
            self.track(&a, true)?;
        }
        Ok(a)
    }

    fn raw_get(&mut self, name: &str) -> Result<Option<Vec<u8>>, KvError> {
        self.raw_get_in(Class::Device, name)
    }

    fn raw_put(&mut self, name: &str, value: &[u8]) -> Result<[u8; 32], KvError> {
        self.raw_put_in(Class::Device, name, value)
    }

    /// The list of names of `class`, kept under its own reserved name and sealed with it.
    fn names_in(&mut self, class: Class) -> Result<Vec<String>, KvError> {
        let list = if class == Class::Device { NAMES } else { ID_NAMES };
        match self.raw_get_in(class, list)? {
            None => Ok(Vec::new()),
            // A list that is not its document is never read as empty: a name added over it would drop every other.
            Some(b) => {
                let mut n = ferros_pieces::kv::names_of(&b).ok_or(KvError::BadNames)?;
                // A reserved name is never listed (the selftest writes one to prove it).
                n.retain(|n| !reserved(n));
                Ok(n)
            }
        }
    }

    fn put_names(&mut self, class: Class, names: &[String]) -> Result<(), KvError> {
        let list = if class == Class::Device { NAMES } else { ID_NAMES };
        let doc = ferros_pieces::kv::names_document(names, now()).ok_or(KvError::BadNames)?;
        self.raw_put_in(class, list, &doc)?;
        Ok(())
    }

    /// The value stored under `name`; `Locked` for an identity-class name while no identity is unlocked. A value kept whole (at most `ferros_pieces::PIECE`, or stored before 2026-11-12) is read as it is; one in pieces is read into one buffer of its length, a piece at a time.
    pub fn get_by_key(&mut self, name: &str) -> Result<Option<Vec<u8>>, KvError> {
        let class = class_of(name);
        if let Some(v) = self.raw_get_in(class, name)? {
            return Ok(Some(v));
        }
        let mut place = self.place(class, name)?;
        let r = ferros_pieces::read_whole(&mut Pieces { kv: self, class }, &place).map_err(KvError::from);
        place.wipe();
        r
    }

    /// The value under `name` appended to `out` (a reply being built, its status byte already in it): a value in pieces goes in a piece at a time, `out` grown once to take it, so it is never in the heap twice. Whether there was one.
    pub fn get_into(&mut self, name: &str, out: &mut Vec<u8>) -> Result<bool, KvError> {
        let class = class_of(name);
        if let Some(mut v) = self.raw_get_in(class, name)? {
            out.extend_from_slice(&v);
            v.fill(0);
            return Ok(true);
        }
        let mut place = self.place(class, name)?;
        let mut ps = Pieces { kv: self, class };
        let n = ferros_pieces::len(&mut ps, &place);
        let Ok(Some(n)) = n else {
            place.wipe();
            return n.map(|_| false).map_err(KvError::from);
        };
        out.reserve_exact(n as usize);
        let from = out.len();
        let r = ferros_pieces::read(&mut ps, &place, |_, b| out.extend_from_slice(b));
        place.wipe();
        if r.is_err() {
            out[from..].fill(0);
            out.truncate(from);
        }
        Ok(r?.is_some())
    }

    /// The length of the value under `name` when it is kept in pieces (None when it is kept whole, or there is none): what a reader that streams it (`read_pieces`) sizes its destination by.
    pub fn pieces_len(&mut self, name: &str) -> Result<Option<usize>, KvError> {
        let class = class_of(name);
        let mut place = self.place(class, name)?;
        let r = ferros_pieces::len(&mut Pieces { kv: self, class }, &place);
        place.wipe();
        Ok(r?.map(|n| n as usize))
    }

    /// The value under `name`, kept in pieces, handed to `sink(offset, bytes)` a piece at a time as each opens, and then held to the manifest's hash (`ferros_pieces::read`: on an error the sink must not trust what it was given). None when it is not kept in pieces.
    pub fn read_pieces(&mut self, name: &str, sink: impl FnMut(usize, &[u8])) -> Result<Option<usize>, KvError> {
        let class = class_of(name);
        let mut place = self.place(class, name)?;
        let r = ferros_pieces::read(&mut Pieces { kv: self, class }, &place, sink);
        place.wipe();
        Ok(r?.map(|n| n as usize))
    }

    /// Store `value` under `name`, durably (manifestus commits per put). Returns the value's vault address (its manifest's, for a value in pieces).
    /// A slice, not an owned buffer: the store command's put used to copy the whole payload into a Vec just to hand it here, a third copy of a 2.7 MB capsule in the kernel heap (2026-10-23: it ran the heap out).
    /// A value over `ferros_pieces::PIECE` is kept in sealed pieces under a manifest (2026-11-12): sealed whole, a 2.8 MB capsule held the sealed copy and manifestus's staging of it (each a 3 MB block) in the kernel heap at once, and with Mahere running the 22 MiB heap had the bytes but no such hole and the kernel panicked. A value of either kind replaces the other: the whole entry or the pieces left from before go.
    pub fn set(&mut self, name: &str, value: &[u8]) -> Result<[u8; 32], KvError> {
        // WHY: the list keeps each name as a VSF `d`, which is ASCII; a name that is not would be stored and then missing from the list, so it is refused before anything is written. Names come from processes and the bridge.
        if !reserved(name) && !name.is_ascii() {
            return Err(KvError::BadNames);
        }
        let class = class_of(name);
        let mut place = self.place(class, name)?;
        let a = self.set_placed(class, name, value, &place);
        place.wipe();
        let a = a?;
        if reserved(name) {
            return Ok(a);
        }
        let mut names = self.names_in(class)?;
        if !names.iter().any(|n| n == name) {
            names.push(String::from(name));
            names.sort();
            self.put_names(class, &names)?;
        }
        Ok(a)
    }

    /// `set`'s write, whole or in pieces, at `place`.
    fn set_placed(&mut self, class: Class, name: &str, value: &[u8], place: &ferros_pieces::Place) -> Result<[u8; 32], KvError> {
        if value.len() > ferros_pieces::PIECE {
            ferros_pieces::put(&mut Pieces { kv: self, class }, place, value, crate::settings::eagle_now())?;
            let whole = self.addr(class, name)?;
            if self.vault.delete(&whole, now())? && class == Class::Identity {
                self.track(&whole, false)?;
            }
            Ok(place.manifest())
        } else {
            let a = self.raw_put_in(class, name, value)?;
            ferros_pieces::remove(&mut Pieces { kv: self, class }, place, crate::settings::eagle_now())?;
            Ok(a)
        }
    }

    /// Remove `name`, kept whole or in pieces; true if it existed.
    pub fn remove_key(&mut self, name: &str) -> Result<bool, KvError> {
        let class = class_of(name);
        let a = self.addr(class, name)?;
        let mut existed = self.vault.delete(&a, now())?;
        if class == Class::Identity {
            self.track(&a, false)?;
        }
        let mut place = self.place(class, name)?;
        let r = ferros_pieces::remove(&mut Pieces { kv: self, class }, &place, crate::settings::eagle_now());
        place.wipe();
        existed |= r?;
        let mut names = self.names_in(class)?;
        if let Some(i) = names.iter().position(|n| n == name) {
            names.remove(i);
            self.put_names(class, &names)?;
        }
        Ok(existed)
    }

    /// Entries a process keeps by 32-byte address (kete on ferros, STD-TARGET.md): the address goes through the same keyed hash a name does, with the scope's prefix in front, so one scope cannot reach another's; the scope's address list is kept under a reserved name for listing, 32 bytes an entry. The scope's class is its prefix's.
    fn addr_scoped(&self, prefix: &str, addr: &[u8; 32]) -> Result<[u8; 32], KvError> {
        let mut m = Vec::with_capacity(prefix.len() + 32);
        m.extend_from_slice(prefix.as_bytes());
        m.extend_from_slice(addr);
        self.addr_hash(class_of(prefix), &m)
    }

    fn addrs_name(prefix: &str) -> String {
        alloc::format!("\0addrs.{prefix}")
    }

    pub fn get_addr(&mut self, prefix: &str, addr: &[u8; 32]) -> Result<Option<Vec<u8>>, KvError> {
        let a = self.addr_scoped(prefix, addr)?;
        match self.vault.get(&a)? {
            Some(s) => Ok(Some(self.unseal(class_of(prefix), s, b"")?)),
            None => Ok(None),
        }
    }

    pub fn set_addr(&mut self, prefix: &str, addr: &[u8; 32], value: &[u8]) -> Result<(), KvError> {
        let class = class_of(prefix);
        let a = self.addr_scoped(prefix, addr)?;
        let sealed = self.seal(class, value, b"")?;
        self.vault.put(&a, &sealed, now())?;
        if class == Class::Identity {
            self.track(&a, true)?;
        }
        let mut list = self.addrs(prefix)?;
        if !list.chunks_exact(32).any(|c| c == addr) {
            list.extend_from_slice(addr);
            self.put_addrs(class, prefix, &list)?;
        }
        Ok(())
    }

    /// A scope's address list written as its document (`ferros_pieces::kv`, section `addresses`).
    fn put_addrs(&mut self, class: Class, prefix: &str, list: &[u8]) -> Result<(), KvError> {
        let doc = ferros_pieces::kv::addresses_document(list, now()).ok_or(KvError::Document("a scope's address list"))?;
        self.raw_put_in(class, &Self::addrs_name(prefix), &doc)?;
        Ok(())
    }

    pub fn remove_addr(&mut self, prefix: &str, addr: &[u8; 32]) -> Result<bool, KvError> {
        let class = class_of(prefix);
        let a = self.addr_scoped(prefix, addr)?;
        let existed = self.vault.delete(&a, now())?;
        if class == Class::Identity {
            self.track(&a, false)?;
        }
        let list = self.addrs(prefix)?;
        if list.chunks_exact(32).any(|c| c == addr) {
            let kept: Vec<u8> = list.chunks_exact(32).filter(|c| *c != addr).flatten().copied().collect();
            self.put_addrs(class, prefix, &kept)?;
        }
        Ok(existed)
    }

    /// The scope's addresses, 32 bytes each, concatenated; none when it keeps no list. A list that is not its document is refused, never read as empty.
    pub fn addrs(&mut self, prefix: &str) -> Result<Vec<u8>, KvError> {
        match self.raw_get_in(class_of(prefix), &Self::addrs_name(prefix))? {
            None => Ok(Vec::new()),
            Some(doc) => ferros_pieces::kv::addresses_of(&doc).ok_or(KvError::Document("a scope's address list")),
        }
    }

    /// Every name stored, sorted: the device's, and the identity's while one is unlocked.
    pub fn keys(&mut self) -> Result<Vec<String>, KvError> {
        let mut names = self.names_in(Class::Device)?;
        if self.id.is_some() {
            names.extend(self.names_in(Class::Identity)?);
            names.sort();
        }
        Ok(names)
    }

    /// Unlock the identity whose seed is `seed` (AUTH.md): its key is BLAKE3 keyed by the ira's identity-vault key over the device-only secret and the seed — bound to this device (the ira and the secret) and to the person (the seed). The secret is made from this session's wairua the first time and kept in the device class. Whatever identity was unlocked before is locked first. The seed is not kept.
    pub fn unlock(&mut self, seed: &[u8; 32], keep: bool) -> Result<(), KvError> {
        self.lock();
        let mut secret = [0u8; 32];
        match self.raw_get(ID_SECRET)? {
            // The vault's copy of the device-only secret wiped once taken: it is half of what the identity key is made from.
            Some(mut s) => {
                let got = ferros_pieces::kv::secret_of(&s);
                crate::keys::wipe_to_dram(&mut s);
                // A secret that does not read is never replaced by a new one: every identity entry here is sealed under a key made from it.
                let Some(mut got) = got else { return Err(KvError::Document("the device-only secret")) };
                secret.copy_from_slice(&got);
                crate::keys::wipe_to_dram(&mut got);
            }
            None => {
                if !crate::keys::stream(ID_SECRET_CONTEXT, &crate::platform::cntpct().to_le_bytes(), &mut secret) {
                    return Err(KvError::NoIra);
                }
                let doc = ferros_pieces::kv::secret_document(&secret, now());
                let Some(mut doc) = doc else {
                    crate::keys::wipe_to_dram(&mut secret);
                    return Err(KvError::Document("the device-only secret"));
                };
                let r = self.raw_put(ID_SECRET, &doc);
                crate::keys::wipe_to_dram(&mut doc);
                if let Err(e) = r {
                    crate::keys::wipe_to_dram(&mut secret);
                    return Err(e);
                }
            }
        }
        // The keys made and held in registers (keys.rs `id_root`, `held_rederive`, `hold`): the ira's identity key, the root and the three keys from it never lie in memory; the seed is held from the copy it came in.
        // The wairua is drawn now if nothing has drawn it yet: without it nothing can be held, and the unlock is refused.
        let made = (|| {
            if !crate::wairua::draw() {
                return None;
            }
            let root = crate::keys::id_root(ID_CONTEXT, &secret, seed)?;
            let keys = IdKeys {
                addr_key: crate::keys::held_rederive(&root, ID_ADDR_CONTEXT, &[])?,
                payload_key: crate::keys::held_rederive(&root, ID_PAYLOAD_CONTEXT, &[])?,
                domain: crate::keys::held_rederive(&root, ID_DOMAIN_CONTEXT, &[])?,
                ledger: crate::keys::held_rederive(&root, ID_LEDGER_CONTEXT, &[])?,
                seed: crate::keys::hold(&mut seed.clone())?,
                person: None,
            };
            Some(keys)
        })();
        crate::keys::wipe_to_dram(&mut secret);
        self.id = Some(made.ok_or(KvError::NoIra)?);
        crate::flashlog::set_personal(self.id.as_ref().map(|k| k.ledger));
        // What the person had in the kernel's vault may now move to the vault process.
        crate::process::MIGRATE_PENDING.store(true, core::sync::atomic::Ordering::Release);
        PERSON.store(true, core::sync::atomic::Ordering::Release);
        // One identity per device: a seed whose key opens no name list is not the bound identity's (refused), or, on a device not bound yet, a different handle from the last one (whose leftovers go).
        if self.raw_get_in(Class::Identity, ID_NAMES)?.is_none() {
            if self.bound()? {
                self.lock();
                return Err(KvError::Belongs);
            }
            self.wipe_identity()?;
            // The (empty) name list written at once: it is what recognises this identity here after a bind, and one that stores nothing in the kernel's vault (everything in the vault process since 2026-11) would otherwise have none and be refused as another's at its next unlock (the 8a, 2026-11-09: bound in boot 117, its own handle refused in 118).
            self.put_names(Class::Identity, &[])?;
        }
        if keep {
            let mut doc = ferros_pieces::kv::unlock_document(seed, now()).ok_or(KvError::Document("the kept seed"))?;
            let r = self.raw_put(ID_UNLOCK, &doc);
            crate::keys::wipe_to_dram(&mut doc);
            r?;
        }
        // The person's wallpaper seed for wallgen (payloads/wallgen), which makes their wallpaper once on this device and keeps it: derived one way, as the attest screen derives it (ahau_core::wallpaper_seed, the same context), so nothing of the identity seed leaves here.
        if let Some(ws) = crate::keys::held_derive(&self.id_keys()?.seed, "ferros 2026-11-07 wallpaper seed", &[]) {
            crate::service::wallgen_after_unlock(ws);
        }
        Ok(())
    }

    /// Lock the identity: its keys dropped, their bytes overwritten first, in place (`keys::wipe_to_dram`).
    /// What it had stored stays sealed on disk.
    /// In place, never taken out first: `Option::take` moved the keys into a copy and wiped that, while the bytes in `self.id` stayed where they were, its `None` written into the person key's tag alone (until 2026-11-17).
    pub fn lock(&mut self) {
        PERSON.store(false, core::sync::atomic::Ordering::Release);
        // The vault process drops what it holds of the person's when it sees this change (the stamp carries it); the identity service learns it from its bell (process.rs `identity_bell`).
        crate::process::LOCK_EPOCH.fetch_add(1, core::sync::atomic::Ordering::Release);
        crate::flashlog::set_personal(None);
        if let Some(k) = self.id.as_mut() {
            crate::keys::wipe_value_to_dram(k);
        }
        self.id = None;
    }

    pub fn unlocked(&self) -> bool {
        self.id.is_some()
    }

    /// The person key handed in by the identity service (SYS_UNLOCK op 10), held beside the identity's keys until `lock`: Ok(true) taken, Ok(false) the same key was held already. Refused while nobody is present. A different key replaces the one held (the fleet settled on the first minted after this device minted its own: fgtw::person), its bytes overwritten first.
    /// The key is held at once (`keys::hold`, which wipes `key`); the one held before is told apart by its one-way id, derived in registers.
    pub fn set_person(&mut self, key: &mut [u8; 32]) -> Result<bool, KvError> {
        if self.id.is_none() {
            crate::keys::wipe_to_dram(key);
            return Err(KvError::Locked);
        }
        let held = crate::keys::hold(key).ok_or(KvError::NoIra)?;
        let pid = crate::keys::held_derive(&held, PERSON_ID_CONTEXT, &[]).ok_or(KvError::NoIra)?;
        let id = self.id.as_mut().ok_or(KvError::Locked)?;
        if id.person.as_ref().is_some_and(|p| p.1 == pid) {
            return Ok(false);
        }
        if let Some(p) = id.person.as_mut() {
            crate::keys::wipe_value_to_dram(p);
        }
        id.person = Some((held, pid));
        Ok(true)
    }

    /// Whether the person key is held (a person present and their identity service has handed it in).
    pub fn person_held(&self) -> bool {
        self.id.as_ref().is_some_and(|k| k.person.is_some())
    }

    /// The alias seed of the kist (`developer`, `option`) for the person present, derived from the held person key in registers (the seed itself goes to the kist); None while locked or before the person key is handed in.
    pub fn alias(&self, developer: &[u8; 32], option: &[u8; 32]) -> Option<[u8; 32]> {
        let p = &self.id.as_ref()?.person.as_ref()?.0;
        let mut m = [0u8; 64];
        m[..32].copy_from_slice(developer);
        m[32..].copy_from_slice(option);
        crate::keys::held_derive(p, ALIAS_CONTEXT, &m)
    }

    /// The person's domain keys under `prefix` (process.rs `domain_keys`, VAULT-OUT.md): the address key under `addr_context` and the sealing key under `seal_context`, each `derive_key(context, domain ‖ prefix)` from the held domain key in registers.
    /// They go to the vault process in its stamp.
    /// None while locked.
    pub fn identity_domain_keys(&self, prefix: &[u8], addr_context: &str, seal_context: &str) -> Option<[u8; 64]> {
        let d = &self.id.as_ref()?.domain;
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(&crate::keys::held_derive(d, addr_context, prefix)?);
        out[32..].copy_from_slice(&crate::keys::held_derive(d, seal_context, prefix)?);
        Some(out)
    }

    /// The bound mark as kept in memory, without reading the vault: None until `bound` has read it once.
    pub fn bound_known(&self) -> Option<bool> {
        self.bound
    }

    /// Whether an identity's fleet has bound this device.
    pub fn bound(&mut self) -> Result<bool, KvError> {
        if let Some(b) = self.bound {
            return Ok(b);
        }
        let b = match self.raw_get(ID_BOUND)? {
            None => false,
            Some(doc) => ferros_pieces::kv::bound_of(&doc).ok_or(KvError::Document("the bound mark"))?,
        };
        self.bound = Some(b);
        Ok(b)
    }

    /// The unlocked identity's fleet has bound this device: from now on only it unlocks here, until release.
    pub fn bind(&mut self) -> Result<(), KvError> {
        if self.id.is_none() {
            return Err(KvError::Locked);
        }
        // The identity's name list before the mark, for an identity unlocked by a kernel from before `unlock` wrote it: without it the bound identity's next unlock is refused.
        if self.raw_get_in(Class::Identity, ID_NAMES)?.is_none() {
            self.put_names(Class::Identity, &[])?;
        }
        let doc = ferros_pieces::kv::bound_document(true, now()).ok_or(KvError::Document("the bound mark"))?;
        self.raw_put(ID_BOUND, &doc)?;
        self.bound = Some(true);
        Ok(())
    }

    /// Release the device (a completed departure, or the developer's `diag "identity release"`): every identity-class entry deleted, the binding and any kept seed gone, the identity locked, and the device-only secret deleted so a new one is made at the next unlock — anything the address list missed can no longer be opened by anyone.
    pub fn release(&mut self) -> Result<usize, KvError> {
        let n = self.wipe_identity()?;
        // The cached mark forgotten first: a delete that fails part way leaves it read again from the flash.
        self.bound = None;
        for name in [ID_BOUND, ID_UNLOCK, ID_SECRET] {
            let a = self.addr(Class::Device, name)?;
            self.vault.delete(&a, now())?;
        }
        self.lock();
        Ok(n)
    }

    /// Every identity-class entry deleted by its address (`ID_ADDRS`), without needing its key, and every kept tag with them (a tag opens only the identity it was kept for, which goes here: on release, and when another handle unlocks a device not bound yet). Returns how many identity entries.
    fn wipe_identity(&mut self) -> Result<usize, KvError> {
        self.wipe_tags()?;
        let list = self.id_addrs()?;
        for a in list.chunks_exact(32) {
            let mut b = [0u8; 32];
            b.copy_from_slice(a);
            crate::platform::pet_watchdogs();
            self.vault.delete(&b, now())?;
        }
        if !list.is_empty() {
            let a = self.addr(Class::Device, ID_ADDRS)?;
            self.vault.delete(&a, now())?;
        }
        Ok(list.len() / 32)
    }

    /// The identity class's vault addresses (`ID_ADDRS`), concatenated; none when it keeps none. A list that is not its document is refused, never read as empty: the wipe it serves would miss every entry on it.
    fn id_addrs(&mut self) -> Result<Vec<u8>, KvError> {
        match self.raw_get(ID_ADDRS)? {
            None => Ok(Vec::new()),
            Some(doc) => ferros_pieces::kv::entries_of(&doc).ok_or(KvError::Document("the identity's address list")),
        }
    }

    /// Keep `ID_ADDRS` in step with the identity class's entries.
    fn track(&mut self, a: &[u8; 32], add: bool) -> Result<(), KvError> {
        let list = self.id_addrs()?;
        let has = list.chunks_exact(32).any(|c| c == a);
        let l = if add && !has {
            let mut l = list;
            l.extend_from_slice(a);
            l
        } else if !add && has {
            list.chunks_exact(32).filter(|c| *c != a).flatten().copied().collect()
        } else {
            return Ok(());
        };
        let doc = ferros_pieces::kv::entries_document(&l, now()).ok_or(KvError::Document("the identity's address list"))?;
        self.raw_put(ID_ADDRS, &doc)?;
        Ok(())
    }

    /// The unlocked identity's entries on this device, for saving to its fleet before a departure (the Fleet page's "save first"): every identity-class name and value except the identity service's own (`fgtw.`: the chain and key, which the fleet already holds) and the attest screen's (`ahau.`: re-derived at every attest). Refused while locked.
    pub fn identity_export(&mut self) -> Result<Vec<(String, Vec<u8>)>, KvError> {
        if self.id.is_none() {
            return Err(KvError::Locked);
        }
        let mut out = Vec::new();
        for name in self.names_in(Class::Identity)? {
            if EXPORT_SKIP.iter().any(|p| name.starts_with(p)) {
                continue;
            }
            crate::platform::pet_watchdogs();
            if let Some(v) = self.get_by_key(&name)? {
                out.push((name, v));
            }
        }
        Ok(out)
    }

    /// How many entries the unlocked identity has on this device (0 while locked): a person who has used this device before finds a space that is not empty.
    pub fn identity_entries(&mut self) -> usize {
        if self.id.is_none() {
            return 0;
        }
        self.names_in(Class::Identity).map(|n| n.len()).unwrap_or(0)
    }

    /// At boot, once the settings are loaded: with unlock at boot on, the kept seed's identity unlocks now (true when it did); off, a kept seed is deleted.
    pub fn boot_unlock(&mut self, on: bool) -> Result<bool, KvError> {
        match (on, self.raw_get(ID_UNLOCK)?) {
            (true, Some(mut s)) => {
                let got = ferros_pieces::kv::unlock_of(&s);
                // The vault's copy of the kept seed, as the one it is copied into.
                crate::keys::wipe_to_dram(&mut s);
                let Some(mut seed) = got else { return Err(KvError::Document("the kept seed")) };
                let r = self.unlock(&seed, false);
                crate::keys::wipe_to_dram(&mut seed);
                r.map(|_| true)
            }
            (false, Some(_)) => {
                let a = self.addr(Class::Device, ID_UNLOCK)?;
                self.vault.delete(&a, now())?;
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// A tag's id and seal key from its UID (`TAG_ID_CONTEXT`, `TAG_SEAL_CONTEXT`), both from the ira-derived device key; the seal key with the device-only secret too. None without the ira this boot, or before any identity has unlocked here (no secret yet, so nothing can have been kept).
    fn tag_keys(&mut self, uid: &[u8]) -> Result<([u8; 32], [u8; 32]), KvError> {
        let mut secret = match self.raw_get(ID_SECRET)? {
            Some(mut doc) => {
                let s = ferros_pieces::kv::secret_of(&doc);
                crate::keys::wipe_to_dram(&mut doc);
                s.ok_or(KvError::Open)?
            }
            None => return Err(KvError::Open),
        };
        let mut dev = crate::ira::derive(TAG_DEVICE_CONTEXT).ok_or(KvError::NoIra)?;
        let id = *blake3::Hasher::new_derive_key(TAG_ID_CONTEXT).update(&dev).update(uid).finalize().as_bytes();
        let key = *blake3::Hasher::new_derive_key(TAG_SEAL_CONTEXT).update(&dev).update(&secret).update(uid).finalize().as_bytes();
        crate::keys::wipe_to_dram(&mut dev);
        crate::keys::wipe_to_dram(&mut secret);
        Ok((id, key))
    }

    fn tag_schema() -> vsf::schema::SectionSchema {
        use vsf::schema::{SectionSchema, TypeConstraint};
        SectionSchema::new(TAG_SECTION).field(TAG_ADDED, TypeConstraint::AnyEagleTime).field(TAG_SEAL, TypeConstraint::Wrapped(b'e'))
    }

    /// A record read back: when it was kept, and the sealed seed. None for anything that is not a tag record of this shape.
    fn tag_record(doc: &[u8]) -> Option<(i64, Vec<u8>)> {
        let section = vsf::schema::SectionBuilder::parse_document(Self::tag_schema(), doc, None).ok()?;
        let first = |name: &str| section.get_fields(name).first().and_then(|f| f.values.first()).cloned();
        let added = match first(TAG_ADDED) {
            Some(vsf::VsfType::e(vsf::EtType::e6(t))) => t,
            _ => 0,
        };
        match first(TAG_SEAL) {
            Some(vsf::VsfType::v(b'e', s)) if s.len() == TAG_SEALED => Some((added, s)),
            _ => None,
        }
    }

    /// Keep a tag for the unlocked identity (settings' "add a tag", AUTH.md): its seed sealed under the tag's key, the record stored in the device class under the tag's id, stamped with the Eagle time. Ok(true) kept, Ok(false) this tag is kept already (its record left as it is); `Locked` while nobody is present.
    pub fn tag_keep(&mut self, uid: &[u8]) -> Result<bool, KvError> {
        self.id_keys()?;
        let (id, mut key) = self.tag_keys(uid)?;
        if self.get_addr(TAGS, &id)?.is_some() {
            crate::keys::wipe_to_dram(&mut key);
            return Ok(false);
        }
        // The seed opened into memory for this one seal (XChaCha20-Poly1305 does not run in registers yet; KEY_REGISTERS.md lists it), wiped below.
        let Some(mut seed) = crate::keys::held_open(&self.id_keys()?.seed) else {
            crate::keys::wipe_to_dram(&mut key);
            return Err(KvError::NoIra);
        };
        let n = self.nonce.next_nonce();
        // A cipher keeps its key by value and does not wipe it when dropped: wiped here once used.
        let mut cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
        let mut sealed = Vec::with_capacity(TAG_SEALED);
        sealed.extend_from_slice(&n[..]);
        sealed.extend_from_slice(&seed);
        let tag = cipher.encrypt_in_place_detached(XNonce::from_slice(&n), &id, &mut sealed[24..]);
        crate::keys::wipe_value_to_dram(&mut cipher);
        crate::keys::wipe_to_dram(&mut key);
        crate::keys::wipe_to_dram(&mut seed);
        sealed.extend_from_slice(&tag.map_err(|_| KvError::Seal)?);
        let now = crate::settings::eagle_now();
        let section = Self::tag_schema().build().set(TAG_ADDED, vsf::VsfType::e(vsf::EtType::e6(now))).and_then(|b| b.set(TAG_SEAL, vsf::VsfType::v(b'e', sealed))).and_then(|b| b.encode()).map_err(|_| KvError::Seal)?;
        let doc = vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_unboxed(TAG_SECTION, section).build().map_err(|_| KvError::Seal)?;
        self.set_addr(TAGS, &id, &doc)?;
        Ok(true)
    }

    /// Unlock by tag (the attest screen, AUTH.md): the record kept under the tag's id opened with its seal key, and its seed unlocked exactly as a typed handle's is (`unlock`, the same binding rule: a bound device opens only its own identity). The seed is returned for the attest screen to hand to the identity service, as it would the one it derived. `Open` for a tag kept for nothing here or a record that does not open (one refusal, no detail).
    pub fn tag_unlock(&mut self, uid: &[u8], keep: bool) -> Result<[u8; 32], KvError> {
        let (id, mut key) = self.tag_keys(uid)?;
        let record = self.get_addr(TAGS, &id)?.and_then(|d| Self::tag_record(&d));
        let Some((_, mut sealed)) = record else {
            crate::keys::wipe_to_dram(&mut key);
            return Err(KvError::Open);
        };
        // Its key wiped once used, as `tag_keep`'s.
        let mut cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
        crate::keys::wipe_to_dram(&mut key);
        let mut n = [0u8; 24];
        n.copy_from_slice(&sealed[..24]);
        sealed.drain(..24);
        let opened = cipher.decrypt_in_place(XNonce::from_slice(&n), &id, &mut sealed);
        crate::keys::wipe_value_to_dram(&mut cipher);
        // The AEAD's check is what binds the record to the tag: a wrong one is a clean refusal here, never a garbage seed (which, on a device not bound yet, would open and make a junk identity space).
        if opened.is_err() {
            crate::keys::wipe_to_dram(&mut sealed);
            return Err(KvError::Open);
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&sealed);
        crate::keys::wipe_to_dram(&mut sealed);
        match self.unlock(&seed, keep) {
            Ok(()) => Ok(seed),
            Err(e) => {
                crate::keys::wipe_to_dram(&mut seed);
                Err(e)
            }
        }
    }

    /// The kept tags while a person is present: each one's id, first `TAG_SHOWN` bytes, and when it was kept (Eagle time, 0 if the record does not say). `Locked` while nobody is present.
    pub fn tag_list(&mut self) -> Result<Vec<([u8; TAG_SHOWN], i64)>, KvError> {
        if self.id.is_none() {
            return Err(KvError::Locked);
        }
        let ids = self.addrs(TAGS)?;
        let mut out = Vec::new();
        for c in ids.chunks_exact(32) {
            let mut id = [0u8; 32];
            id.copy_from_slice(c);
            let added = self.get_addr(TAGS, &id)?.and_then(|d| Self::tag_record(&d)).map_or(0, |r| r.0);
            let mut shown = [0u8; TAG_SHOWN];
            shown.copy_from_slice(&id[..TAG_SHOWN]);
            out.push((shown, added));
        }
        Ok(out)
    }

    /// Forget a kept tag by the start of its id as the list shows it, or every tag (None). How many went. `Locked` while nobody is present.
    pub fn tag_forget(&mut self, which: Option<[u8; TAG_SHOWN]>) -> Result<usize, KvError> {
        if self.id.is_none() {
            return Err(KvError::Locked);
        }
        match which {
            None => self.wipe_tags(),
            Some(p) => {
                let ids = self.addrs(TAGS)?;
                let mut n = 0;
                for c in ids.chunks_exact(32) {
                    if c[..TAG_SHOWN] == p {
                        let mut id = [0u8; 32];
                        id.copy_from_slice(c);
                        self.remove_addr(TAGS, &id)?;
                        n += 1;
                    }
                }
                Ok(n)
            }
        }
    }

    /// Every kept tag deleted, and their list, needing no key: on release, and with the identity they open (`wipe_identity`).
    fn wipe_tags(&mut self) -> Result<usize, KvError> {
        let ids = self.addrs(TAGS)?;
        for c in ids.chunks_exact(32) {
            let mut id = [0u8; 32];
            id.copy_from_slice(c);
            let a = self.addr_scoped(TAGS, &id)?;
            self.vault.delete(&a, now())?;
        }
        if !ids.is_empty() {
            let a = self.addr(Class::Device, &Self::addrs_name(TAGS))?;
            self.vault.delete(&a, now())?;
        }
        Ok(ids.len() / 32)
    }

    /// `diag kv selftest`: put a 30 000-byte pattern, count the block I/O it costs, re-read every block the put appended (mirror A) and count how many still carry a valid seal, then get the value back and compare.
    pub fn selftest(&mut self) -> String {
        use core::sync::atomic::Ordering::Relaxed;
        let mut out = String::new();
        let before = self.vault.engine_stats();
        let (r0, w0) = (READS.load(Relaxed), WRITES.load(Relaxed));
        let val: Vec<u8> = (0..30_000u32).map(|j| (j * 7) as u8).collect();
        let put = self.raw_put("\0selftest", &val);
        let after = self.vault.engine_stats();
        let (r1, w1) = (READS.load(Relaxed), WRITES.load(Relaxed));
        out.push_str(&alloc::format!("put: {:?}; plow {} -> {}, block reads {} writes {}\n", put.map(|_| ()), before.plow, after.plow, r1 - r0, w1 - w0));
        let mut region = Region { base: self.mirror_a, blocks: REGION_BLOCKS };
        let (mut sealed, mut unsealed) = (0u32, 0u32);
        let mut first_bad = None;
        let tract_base = 1u64 << RING_LOG2;
        for pos in before.plow..after.plow {
            let mut b = [0u8; 4096];
            if region.read(tract_base + pos % before.tract_blocks.max(1), &mut b).is_ok() && manifestus::sealed_hp(&b).is_some() {
                sealed += 1;
            } else {
                unsealed += 1;
                first_bad.get_or_insert((pos, b[0], b[1], b[2], b[3]));
            }
        }
        out.push_str(&alloc::format!("appended blocks re-read: {sealed} sealed, {unsealed} not (first bad {first_bad:?})\n"));
        let check = |kv: &mut Self, what: &str, out: &mut String| {
            let got = kv.raw_get("\0selftest");
            let st = kv.vault.engine_stats();
            out.push_str(&alloc::format!("{what}: get {} (plow {})\n", match got { Ok(Some(g)) => if g == val { alloc::string::String::from("identical") } else { alloc::format!("DIFFERS ({} bytes)", g.len()) }, Ok(None) => "absent".into(), Err(e) => alloc::format!("{e:?}") }, st.plow));
        };
        check(self, "after the value put", &mut out);
        // The rest of `set`: read the name list, write it back with one more name.
        let names = self.keys();
        out.push_str(&alloc::format!("keys: {:?}\n", names.as_ref().map(|n| n.len())));
        check(self, "after reading the names", &mut out);
        let mut n = names.unwrap_or_default();
        n.push(alloc::string::String::from("\0selftest-marker")); // reserved: dropped by `keys` on the way back
        let r = match ferros_pieces::kv::names_document(&n, now()) {
            Some(doc) => self.raw_put(NAMES, &doc),
            None => Err(KvError::BadNames),
        };
        out.push_str(&alloc::format!("names put: {:?}\n", r.map(|_| ())));
        check(self, "after the names put", &mut out);
        out
    }

    /// A one-line state summary for the flash log and DIAG.
    pub fn describe(&mut self) -> String {
        let st = self.vault.engine_stats();
        alloc::format!("manifestus gen {:?} {:?}", self.vault.generation(), st)
    }
}

/// The kernel vault's two mirror regions in the ferros partition (first block, blocks: A then B), for the owner's fresh start (service.rs `vault fresh`), which zeroes them: manifestus genesises a blank pair at the next open.
pub fn regions() -> (u64, u64) {
    (MIRROR_OFFSET, 2 * REGION_BLOCKS)
}
