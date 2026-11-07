//! The kernel's vault on manifestus (the owner's storage engine, shared with Photon): two mirror regions of the ferros partition as manifestus BlockDevs, and a thin keyed, sealed layer on top.
//!
//! Replaces ferros_vault's single-file Phase-1 store in the kernel (2026-10-20), whose 64 KiB payload and key-derived anchor slots let a large object overwrite the anchors and roll the vault back across a reboot (pinned with the flash log).
//!
//! Layout (ferros partition, LU 0, 4 KiB blocks): mirror A at +2 GiB, mirror B at +2 GiB + 256 MiB, 65536 blocks each. The flash log lives at +1 GiB; the old ferros_vault file at the partition start is left untouched.
//! Names: a logical name maps to the 32-byte manifestus address BLAKE3-keyed(addr key, name); values are XChaCha20-Poly1305 sealed (24-byte nonce in front) under a payload key. Both keys derive from the ira through ferros_vault's AnchorKey, as before. manifestus is non-enumerable by design, so the kernel keeps its own list of names (sealed, under a reserved name) for `store ls`.

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
/// The kernel's list of names ('\n'-separated), itself stored under this name. Every name starting with NUL is the kernel's own (the list, the selftest's scratch): never listed, never indexed, so `store ls` shows only what a caller stored.
const NAMES: &str = "\0names";
fn reserved(name: &str) -> bool {
    name.starts_with('\0')
}

/// The identity class (AUTH.md, owner decisions 2026-10-31): entries under these prefixes are the person's, not the device's — the map's arrangement, the identity service's chain and key (payloads/fgtw), the attest screen's own, the WiFi credential (networks travel with the identity; the per-network MAC stays the device's), and the GPS driver's last fix (owner, 2026-11-04: where the device has been is the person's, and no GPS runs while nobody has attested; the replays it aids from keep no place, gps scrub). They are sealed under a key bound to this device and to the person, held only while an identity is unlocked: before attest none of them can be read, their names included. One identity per device: another person's handle is refused once the device is bound, and wipes an earlier attempt's leftovers while it is not.
const IDENTITY: [&str; 5] = ["map.", "fgtw.", "ahau.", "wifi.cred", "gps.last"];
pub fn identity_class(name: &str) -> bool {
    IDENTITY.iter().any(|p| name.starts_with(p))
}
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

/// Which keys an entry is sealed under.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Device,
    Identity,
}
fn class_of(name: &str) -> Class {
    if identity_class(name) { Class::Identity } else { Class::Device }
}

/// The unlocked identity's keys: the address key its names hash under and the cipher its values seal under.
struct IdKeys {
    addr_key: [u8; 32],
    /// Plain bytes, the cipher built from them per use: a stored cipher's key is not wiped when it is dropped, and these must be (`lock`).
    payload_key: [u8; 32],
    /// The base of the person's vault domains (the vault process's, VAULT-OUT.md).
    domain: [u8; 32],
}

/// Block I/O counters for `selftest`.
static READS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static WRITES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// One mirror region: `blocks` 4 KiB blocks of LU 0 from `base`.
pub struct Region {
    base: u64,
    blocks: u64,
}

impl BlockDev for Region {
    fn block_count(&self) -> u64 {
        self.blocks
    }
    fn read(&mut self, lba: u64, buf: &mut Block) -> manifestus::Result<()> {
        if lba >= self.blocks {
            return Err(manifestus::Error::Bounds(lba));
        }
        // A blank pair of regions is scanned whole before manifestus genesises it (131072 reads): keep the watchdogs fed through that.
        if lba % 1024 == 0 {
            crate::platform::pet_watchdogs();
        }
        READS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let u = UfsController::new(UFS_BASE);
        if u.read_block_lun(0, (self.base + lba) as u32) != 0 || u.last_response_status() != 0 {
            return Err(manifestus::Error::Corrupt(alloc::format!("UFS read failed at region block {lba}")));
        }
        buf.copy_from_slice(u.data_buffer());
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &Block) -> manifestus::Result<()> {
        if lba >= self.blocks {
            return Err(manifestus::Error::Bounds(lba));
        }
        WRITES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let u = UfsController::new(UFS_BASE);
        u.data_buffer_mut().copy_from_slice(buf);
        if u.write_block_lun(0, (self.base + lba) as u32) != 0 || u.last_response_status() != 0 {
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

/// manifestus has no clock requirement beyond a recorded time: seconds since this boot.
fn now() -> i64 {
    (crate::platform::cntpct() / crate::platform::cntfrq()) as i64
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
}
impl From<manifestus::Error> for KvError {
    fn from(e: manifestus::Error) -> Self {
        KvError::Engine(e)
    }
}

pub struct KVault {
    vault: Vault<Region, Region>,
    addr_key: [u8; 32],
    cipher: XChaCha20Poly1305,
    nonce: crate::wairua::TrngNonce,
    /// The unlocked identity, while one is: dropped when power goes or the device switches to another OS (`lock`).
    id: Option<IdKeys>,
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
        Ok(KVault { vault, addr_key, cipher: XChaCha20Poly1305::new(Key::from_slice(&payload_key)), nonce: crate::wairua::TrngNonce::new(), id: None, mirror_a: a })
    }

    /// The address key for `class`: the device's always, the identity's only while one is unlocked.
    fn addr_key_of(&self, class: Class) -> Result<&[u8; 32], KvError> {
        match class {
            Class::Device => Ok(&self.addr_key),
            Class::Identity => self.id.as_ref().map(|k| &k.addr_key).ok_or(KvError::Locked),
        }
    }

    /// The cipher for `class`, likewise.
    fn cipher_of(&self, class: Class) -> Result<XChaCha20Poly1305, KvError> {
        match class {
            Class::Device => Ok(self.cipher.clone()),
            Class::Identity => self.id.as_ref().map(|k| XChaCha20Poly1305::new(Key::from_slice(&k.payload_key))).ok_or(KvError::Locked),
        }
    }

    fn addr(&self, class: Class, name: &str) -> Result<[u8; 32], KvError> {
        Ok(*blake3::keyed_hash(self.addr_key_of(class)?, name.as_bytes()).as_bytes())
    }

    fn seal(&mut self, class: Class, plain: &[u8]) -> Result<Vec<u8>, KvError> {
        let n = self.nonce.next_nonce();
        let cipher = self.cipher_of(class)?;
        // One allocation the size of the result: the nonce, the plaintext encrypted where it lies, then the tag — the same bytes `encrypt` gives. Sealing through `encrypt` into a growing Vec held two copies of the value at once (and a third while the Vec grew), and a 2.78 MB display capsule ran the 12 MiB heap out (2026-10-26, panic "memory allocation … failed" in the crumbs).
        let mut out = Vec::with_capacity(24 + plain.len() + 16);
        out.extend_from_slice(&n[..]);
        out.extend_from_slice(plain);
        let tag = cipher.encrypt_in_place_detached(XNonce::from_slice(&n), b"", &mut out[24..]).map_err(|_| KvError::Seal)?;
        out.extend_from_slice(&tag);
        Ok(out)
    }

    /// Opened where it lies, as `seal` closes: the nonce taken off the front, the rest decrypted in the same buffer and its tag dropped — one copy of a large value in the heap, not two.
    fn unseal(&self, class: Class, mut sealed: Vec<u8>) -> Result<Vec<u8>, KvError> {
        if sealed.len() < 24 + 16 {
            return Err(KvError::Open);
        }
        let cipher = self.cipher_of(class)?;
        let mut n = [0u8; 24];
        n.copy_from_slice(&sealed[..24]);
        sealed.drain(..24);
        cipher.decrypt_in_place(XNonce::from_slice(&n), b"", &mut sealed).map_err(|_| KvError::Open)?;
        Ok(sealed)
    }

    fn raw_get_in(&mut self, class: Class, name: &str) -> Result<Option<Vec<u8>>, KvError> {
        let a = self.addr(class, name)?;
        match self.vault.get(&a)? {
            Some(s) => Ok(Some(self.unseal(class, s)?)),
            None => Ok(None),
        }
    }

    fn raw_put_in(&mut self, class: Class, name: &str, value: &[u8]) -> Result<[u8; 32], KvError> {
        let a = self.addr(class, name)?;
        let sealed = self.seal(class, value)?;
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
            Some(b) => {
                let s = core::str::from_utf8(&b).map_err(|_| KvError::BadNames)?;
                Ok(s.split('\n').filter(|n| !n.is_empty() && !reserved(n)).map(String::from).collect())
            }
        }
    }

    fn put_names(&mut self, class: Class, names: &[String]) -> Result<(), KvError> {
        let list = if class == Class::Device { NAMES } else { ID_NAMES };
        self.raw_put_in(class, list, names.join("\n").as_bytes())?;
        Ok(())
    }

    /// The value stored under `name`; `Locked` for an identity-class name while no identity is unlocked.
    pub fn get_by_key(&mut self, name: &str) -> Result<Option<Vec<u8>>, KvError> {
        self.raw_get_in(class_of(name), name)
    }

    /// Store `value` under `name`, durably (manifestus commits per put). Returns the value's vault address.
    /// Store `value` under `name`. A slice, not an owned buffer: the store command's put used to copy the whole payload into a Vec just to hand it here, a third copy of a 2.7 MB capsule in the kernel heap (2026-10-23: it ran the heap out).
    pub fn set(&mut self, name: &str, value: &[u8]) -> Result<[u8; 32], KvError> {
        let class = class_of(name);
        let a = self.raw_put_in(class, name, value)?;
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

    /// Remove `name`; true if it existed.
    pub fn remove_key(&mut self, name: &str) -> Result<bool, KvError> {
        let class = class_of(name);
        let a = self.addr(class, name)?;
        let existed = self.vault.delete(&a, now())?;
        if class == Class::Identity {
            self.track(&a, false)?;
        }
        let mut names = self.names_in(class)?;
        if let Some(i) = names.iter().position(|n| n == name) {
            names.remove(i);
            self.put_names(class, &names)?;
        }
        Ok(existed)
    }

    /// Entries a process keeps by 32-byte address (kete on ferros, STD-TARGET.md): the address goes through the same keyed hash a name does, with the scope's prefix in front, so one scope cannot reach another's; the scope's address list is kept under a reserved name for listing, 32 bytes an entry. The scope's class is its prefix's.
    fn addr_scoped(&self, prefix: &str, addr: &[u8; 32]) -> Result<[u8; 32], KvError> {
        let mut h = blake3::Hasher::new_keyed(self.addr_key_of(class_of(prefix))?);
        h.update(prefix.as_bytes());
        h.update(addr);
        Ok(*h.finalize().as_bytes())
    }

    fn addrs_name(prefix: &str) -> String {
        alloc::format!("\0addrs.{prefix}")
    }

    pub fn get_addr(&mut self, prefix: &str, addr: &[u8; 32]) -> Result<Option<Vec<u8>>, KvError> {
        let a = self.addr_scoped(prefix, addr)?;
        match self.vault.get(&a)? {
            Some(s) => Ok(Some(self.unseal(class_of(prefix), s)?)),
            None => Ok(None),
        }
    }

    pub fn set_addr(&mut self, prefix: &str, addr: &[u8; 32], value: &[u8]) -> Result<(), KvError> {
        let class = class_of(prefix);
        let a = self.addr_scoped(prefix, addr)?;
        let sealed = self.seal(class, value)?;
        self.vault.put(&a, &sealed, now())?;
        if class == Class::Identity {
            self.track(&a, true)?;
        }
        let mut list = self.addrs(prefix)?;
        if !list.chunks_exact(32).any(|c| c == addr) {
            list.extend_from_slice(addr);
            self.raw_put_in(class, &Self::addrs_name(prefix), &list)?;
        }
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
            self.raw_put_in(class, &Self::addrs_name(prefix), &kept)?;
        }
        Ok(existed)
    }

    /// The scope's addresses, 32 bytes each, concatenated.
    pub fn addrs(&mut self, prefix: &str) -> Result<Vec<u8>, KvError> {
        Ok(self.raw_get_in(class_of(prefix), &Self::addrs_name(prefix))?.unwrap_or_default())
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
            Some(s) if s.len() == 32 => secret.copy_from_slice(&s),
            _ => {
                if !crate::keys::stream(ID_SECRET_CONTEXT, &crate::platform::cntpct().to_le_bytes(), &mut secret) {
                    return Err(KvError::NoIra);
                }
                self.raw_put(ID_SECRET, &secret)?;
            }
        }
        let mut base = crate::ira::derive(ID_CONTEXT).ok_or(KvError::NoIra)?;
        let mut h = blake3::Hasher::new_keyed(&base);
        h.update(&secret);
        h.update(seed);
        let mut k = *h.finalize().as_bytes();
        let addr_key = blake3::derive_key(ID_ADDR_CONTEXT, &k);
        let payload_key = blake3::derive_key(ID_PAYLOAD_CONTEXT, &k);
        let domain = blake3::derive_key(ID_DOMAIN_CONTEXT, &k);
        self.id = Some(IdKeys { addr_key, payload_key, domain });
        // What the person had in the kernel's vault may now move to the vault process.
        crate::process::MIGRATE_PENDING.store(true, core::sync::atomic::Ordering::Release);
        PERSON.store(true, core::sync::atomic::Ordering::Release);
        for b in [&mut secret, &mut base, &mut k] {
            b.fill(0);
        }
        // One identity per device: a seed whose key opens no name list is not the bound identity's (refused), or, on a device not bound yet, a different handle from the last one (whose leftovers go).
        if self.raw_get_in(Class::Identity, ID_NAMES)?.is_none() {
            if self.bound()? {
                self.lock();
                return Err(KvError::Belongs);
            }
            self.wipe_identity()?;
        }
        if keep {
            self.raw_put(ID_UNLOCK, seed)?;
        }
        Ok(())
    }

    /// Lock the identity: its keys dropped, their bytes overwritten first. What it had stored stays sealed on disk.
    pub fn lock(&mut self) {
        PERSON.store(false, core::sync::atomic::Ordering::Release);
        // The vault process drops what it holds of the person's when it sees this change (the stamp carries it).
        crate::process::LOCK_EPOCH.fetch_add(1, core::sync::atomic::Ordering::Release);
        if let Some(mut k) = self.id.take() {
            k.addr_key.fill(0);
            k.payload_key.fill(0);
            k.domain.fill(0);
            core::hint::black_box(&k);
        }
    }

    pub fn unlocked(&self) -> bool {
        self.id.is_some()
    }

    /// The base of the unlocked person's vault domains; None while locked.
    pub fn identity_domain(&self) -> Option<[u8; 32]> {
        self.id.as_ref().map(|k| k.domain)
    }

    /// Whether an identity's fleet has bound this device.
    pub fn bound(&mut self) -> Result<bool, KvError> {
        Ok(self.raw_get(ID_BOUND)?.is_some())
    }

    /// The unlocked identity's fleet has bound this device: from now on only it unlocks here, until release.
    pub fn bind(&mut self) -> Result<(), KvError> {
        if self.id.is_none() {
            return Err(KvError::Locked);
        }
        self.raw_put(ID_BOUND, &[1])?;
        Ok(())
    }

    /// Release the device (a completed departure, or the developer's `diag "identity release"`): every identity-class entry deleted, the binding and any kept seed gone, the identity locked, and the device-only secret deleted so a new one is made at the next unlock — anything the address list missed can no longer be opened by anyone.
    pub fn release(&mut self) -> Result<usize, KvError> {
        let n = self.wipe_identity()?;
        for name in [ID_BOUND, ID_UNLOCK, ID_SECRET] {
            let a = self.addr(Class::Device, name)?;
            self.vault.delete(&a, now())?;
        }
        self.lock();
        Ok(n)
    }

    /// Every identity-class entry deleted by its address (`ID_ADDRS`), without needing its key. Returns how many.
    fn wipe_identity(&mut self) -> Result<usize, KvError> {
        let list = self.raw_get(ID_ADDRS)?.unwrap_or_default();
        for a in list.chunks_exact(32) {
            let mut b = [0u8; 32];
            b.copy_from_slice(a);
            self.vault.delete(&b, now())?;
        }
        if !list.is_empty() {
            let a = self.addr(Class::Device, ID_ADDRS)?;
            self.vault.delete(&a, now())?;
        }
        Ok(list.len() / 32)
    }

    /// Keep `ID_ADDRS` in step with the identity class's entries.
    fn track(&mut self, a: &[u8; 32], add: bool) -> Result<(), KvError> {
        let list = self.raw_get(ID_ADDRS)?.unwrap_or_default();
        let has = list.chunks_exact(32).any(|c| c == a);
        if add && !has {
            let mut l = list;
            l.extend_from_slice(a);
            self.raw_put(ID_ADDRS, &l)?;
        } else if !add && has {
            let kept: Vec<u8> = list.chunks_exact(32).filter(|c| *c != a).flatten().copied().collect();
            self.raw_put(ID_ADDRS, &kept)?;
        }
        Ok(())
    }

    /// The unlocked identity's entries on this device, for saving to its fleet before a departure (the Fleet page's "save first"): every identity-class name and value except the identity service's own (`fgtw.`: the chain and key, which the fleet already holds) and the attest screen's (`ahau.`: re-derived at every attest). Refused while locked.
    pub fn identity_export(&mut self) -> Result<Vec<(String, Vec<u8>)>, KvError> {
        if self.id.is_none() {
            return Err(KvError::Locked);
        }
        let mut out = Vec::new();
        for name in self.names_in(Class::Identity)? {
            if name.starts_with("fgtw.") || name.starts_with("ahau.") {
                continue;
            }
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
            (true, Some(s)) if s.len() == 32 => {
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&s);
                let r = self.unlock(&seed, false);
                seed.fill(0);
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
        let r = self.raw_put(NAMES, n.join("\n").as_bytes());
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
