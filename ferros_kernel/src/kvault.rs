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
const MIRROR_A: u64 = crate::ufs_device::FERROS_BASE_LBA as u64 + 524_288;
const MIRROR_B: u64 = MIRROR_A + REGION_BLOCKS;
const RING_LOG2: u8 = manifestus::HOST_RING_LOG2;
const ADDR_CONTEXT: &str = "ferros.kernel.store.addr.v0";
const PAYLOAD_CONTEXT: &str = "ferros.kernel.store.payload.v0";
/// The kernel's list of names ('\n'-separated), itself stored under this name. Every name starting with NUL is the kernel's own (the list, the selftest's scratch): never listed, never indexed, so `store ls` shows only what a caller stored.
const NAMES: &str = "\0names";
fn reserved(name: &str) -> bool {
    name.starts_with('\0')
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
}

impl KVault {
    /// Open the kernel vault (manifestus genesises it on a blank pair of regions and refuses to format over sealed blocks it cannot bootstrap).
    pub fn open(ira: &[u8; 32]) -> Result<Self, KvError> {
        let base = ferros_vault::anchor::AnchorKey::from_ira(ira);
        let addr_key = ferros_vault::anchor::AnchorKey::derive(ADDR_CONTEXT, &base.0);
        let payload_key = ferros_vault::anchor::AnchorKey::derive(PAYLOAD_CONTEXT, &base.0);
        let mirror = Mirror::new(Region { base: MIRROR_A, blocks: REGION_BLOCKS }, Region { base: MIRROR_B, blocks: REGION_BLOCKS }).with_events(alloc::boxed::Box::new(LogSink));
        let vault = Vault::open(mirror, RING_LOG2, now(), DeletePolicy::UnlinkOnly)?;
        Ok(KVault { vault, addr_key, cipher: XChaCha20Poly1305::new(Key::from_slice(&payload_key)), nonce: crate::wairua::TrngNonce::new() })
    }

    fn addr(&self, name: &str) -> [u8; 32] {
        *blake3::keyed_hash(&self.addr_key, name.as_bytes()).as_bytes()
    }

    fn seal(&mut self, plain: &[u8]) -> Result<Vec<u8>, KvError> {
        let n = self.nonce.next_nonce();
        // One allocation the size of the result: the nonce, the plaintext encrypted where it lies, then the tag — the same bytes `encrypt` gives. Sealing through `encrypt` into a growing Vec held two copies of the value at once (and a third while the Vec grew), and a 2.78 MB display capsule ran the 12 MiB heap out (2026-10-26, panic "memory allocation … failed" in the crumbs).
        let mut out = Vec::with_capacity(24 + plain.len() + 16);
        out.extend_from_slice(&n[..]);
        out.extend_from_slice(plain);
        let tag = self.cipher.encrypt_in_place_detached(XNonce::from_slice(&n), b"", &mut out[24..]).map_err(|_| KvError::Seal)?;
        out.extend_from_slice(&tag);
        Ok(out)
    }

    /// Opened where it lies, as `seal` closes: the nonce taken off the front, the rest decrypted in the same buffer and its tag dropped — one copy of a large value in the heap, not two.
    fn unseal(&self, mut sealed: Vec<u8>) -> Result<Vec<u8>, KvError> {
        if sealed.len() < 24 + 16 {
            return Err(KvError::Open);
        }
        let mut n = [0u8; 24];
        n.copy_from_slice(&sealed[..24]);
        sealed.drain(..24);
        self.cipher.decrypt_in_place(XNonce::from_slice(&n), b"", &mut sealed).map_err(|_| KvError::Open)?;
        Ok(sealed)
    }

    fn raw_get(&mut self, name: &str) -> Result<Option<Vec<u8>>, KvError> {
        let a = self.addr(name);
        match self.vault.get(&a)? {
            Some(s) => Ok(Some(self.unseal(s)?)),
            None => Ok(None),
        }
    }

    fn raw_put(&mut self, name: &str, value: &[u8]) -> Result<[u8; 32], KvError> {
        let a = self.addr(name);
        let sealed = self.seal(value)?;
        self.vault.put(&a, &sealed, now())?;
        Ok(a)
    }

    /// The value stored under `name`.
    pub fn get_by_key(&mut self, name: &str) -> Result<Option<Vec<u8>>, KvError> {
        self.raw_get(name)
    }

    /// Store `value` under `name`, durably (manifestus commits per put). Returns the value's vault address.
    /// Store `value` under `name`. A slice, not an owned buffer: the store command's put used to copy the whole payload into a Vec just to hand it here, a third copy of a 2.7 MB capsule in the kernel heap (2026-10-23: it ran the heap out).
    pub fn set(&mut self, name: &str, value: &[u8]) -> Result<[u8; 32], KvError> {
        let a = self.raw_put(name, value)?;
        if reserved(name) {
            return Ok(a);
        }
        let mut names = self.keys()?;
        if !names.iter().any(|n| n == name) {
            names.push(String::from(name));
            names.sort();
            self.raw_put(NAMES, names.join("\n").as_bytes())?;
        }
        Ok(a)
    }

    /// Remove `name`; true if it existed.
    pub fn remove_key(&mut self, name: &str) -> Result<bool, KvError> {
        let a = self.addr(name);
        let existed = self.vault.delete(&a, now())?;
        let mut names = self.keys()?;
        if let Some(i) = names.iter().position(|n| n == name) {
            names.remove(i);
            self.raw_put(NAMES, names.join("\n").as_bytes())?;
        }
        Ok(existed)
    }

    /// Every name the kernel has stored, sorted.
    pub fn keys(&mut self) -> Result<Vec<String>, KvError> {
        match self.raw_get(NAMES)? {
            None => Ok(Vec::new()),
            Some(b) => {
                let s = core::str::from_utf8(&b).map_err(|_| KvError::BadNames)?;
                Ok(s.split('\n').filter(|n| !n.is_empty() && !reserved(n)).map(String::from).collect())
            }
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
        let mut region = Region { base: MIRROR_A, blocks: REGION_BLOCKS };
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
