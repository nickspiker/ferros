//! The ira-rooted vault on the ferros partition: open it (or genesis + seal it) over a UFS link that is already up, and report the result into the ramoops block. Needs a working UFS link, which today only the Linux-shim handoff provides (the cold boot's own link startup is still blocked, UFS.md).

use core::ptr::write_volatile;

use crate::platform::{crumb, LAST_CRUMB, RAMOOPS_RESULT, RESULT_MAGIC};

/// pixel8 / Tensor G3 UFS host base (UFSHCI).
const UFS_BASE: usize = 0x1320_0000;

/// Display fingerprints: the first 64 bits of BLAKE3 derive_key under a context of their own, so a fingerprint is one-way and matches no other hash of the same secret anywhere.
/// The ira's, one per board as the ira's own context is (ira.rs `ira_context`).
fn ira_fp_context() -> &'static str {
    if crate::board::akita() { "ferros akita 2026-11-06 ira display fingerprint v0" } else { "ferros husky 2026-10-14 ira display fingerprint v1" }
}

fn fingerprint(context: &str, secret: &[u8; 32]) -> [u8; 8] {
    let mut fp = [0u8; 8];
    fp.copy_from_slice(&blake3::derive_key(context, secret)[..8]);
    fp
}

/// What one ira-rooted vault open-or-genesis found, plus the ira/wairua side readings reported with it.
pub struct VaultProbe {
    pub link_up: bool,
    pub opened: u8,
    pub verified: u8,
    pub sealed: u8,
    /// 0 ok; 1 format, 2 set, 3 read failed.
    pub stage: u8,
    pub root16: [u8; 16],
    /// One-way display fingerprint of the ira (`fingerprint`): the same every boot. Never the secret itself.
    pub ira_fp: [u8; 8],
    /// 0 = ira derived; else why not (`ira::derive_ira`: G#100 + source = reads disagreed, G#200 + source = degenerate, G#300 = devinfo records missing).
    pub ira_fail: u32,
    pub ap_tune: [u8; 32],
    pub asv_nz: bool,
    pub hpm_nz: bool,
    pub dvfs: u8,
}

impl VaultProbe {
    /// No probe (akita, storage-off until its partitions are backed up; board.rs): no link, nothing opened, the ira not derived (G#300, its devinfo records not read).
    pub fn none() -> VaultProbe {
        VaultProbe { link_up: false, opened: 0, verified: 0, sealed: 0, stage: 0, root16: [0; 16], ira_fp: [0; 8], ira_fail: 0x300, ap_tune: [0; 32], asv_nz: false, hpm_nz: false, dvfs: 0 }
    }
}

/// The ira-keyed vault on the ferros partition, kept open by the USB service for `ferros.dev.store`.
/// The kernel's store: manifestus (kvault.rs) since 2026-10-20; ferros_vault's Phase-1 single-file store before.
pub type ShimStore = crate::kvault::KVault;

/// The real boot pattern on the adopted link: open the ferros vault keyed by the ira, else genesis + seal it. Crumbs bracket each first-on-hardware step so a stall pins it. Returns the open store too (None if open and genesis both failed).
pub fn vault_probe(link_up: bool) -> (VaultProbe, Option<ShimStore>) {
    use ferros_vault::anchor::AnchorKey;

    let mut ira = crate::ira::derive_ira(); // every stable per-device source, each read twice and checked (derive_ira v2)
    crate::platform::pstore(b"ferros: ira combined\n");
    crate::platform::mark_trail(0x10);
    if let Ok(mut i) = ira {
        crumb(0xC3);
        // Into the registers (keys.rs); this copy is wiped there, the one in `ira` at the end of the probe.
        crate::ira::keep_ira(&mut i);
        // The flash log can seal now: what the kernel said since boot, held in the pre-boot buffer, goes to the flash (flashlog.rs).
        crate::flashlog::flush();
    }
    let ira_fail = ira.err().unwrap_or(0);
    let ira_fp = ira.map(|i| fingerprint(ira_fp_context(), &i)).unwrap_or([0u8; 8]);
    crumb(0xC1); // ira derived (or refused)
    crate::platform::pstore(b"ferros: ira derived (or refused)\n");

    crumb(0xC2);

    let nz = |b: &[u8]| b.iter().any(|&x| x != 0);
    let mut p = VaultProbe {
        link_up,
        opened: 0,
        verified: 0,
        sealed: 0,
        stage: 0,
        root16: [0u8; 16],
        ira_fp,
        ap_tune: { crate::platform::mark_trail(0x11); crate::ira::read_ap_hw_tune() },
        asv_nz: { crate::platform::mark_trail(0x12); nz(&crate::ira::read_asv_tbl()) },
        hpm_nz: { crate::platform::mark_trail(0x13); nz(&crate::ira::read_hpm_asv()) },
        dvfs: { crate::platform::mark_trail(0x14); crate::ira::read_dvfs_version() },
        ira_fail,
    };

    crate::platform::pstore(b"ferros: tune, ASV, HPM and DVFS read\n");
    crate::checklist::step(crate::checklist::Step::Ira);
    // No valid ira, no vault (PIPE: refuse to operate). The probe reports why.
    let Ok(ira) = ira.as_mut() else {
        return (p, None);
    };
    crate::platform::mark_trail(0x15);
    let key: AnchorKey = AnchorKey::from_ira(ira); // the ira-rooted vault key
    crate::oracle::set_system_secret(ira);
    let _ = key;

    // The genesis value: a `genesis` document (ferros_pieces::kv) since 2026-11-17, a text marker before (moved by kvault's open).
    const LKEY: &str = crate::kvault::GENESIS;
    let mut kept: Option<ShimStore> = None;
    let t_open = crate::platform::cntpct();
    crate::platform::pstore(b"ferros: opening the vault\n");
    crate::platform::mark_trail(0x16);
    crumb(0xE0); // before KVault::open (manifestus open ladder: bootstrap the spine, or genesis a blank pair of regions)
    let opened = crate::kvault::KVault::open(ira);
    crate::checklist::step(crate::checklist::Step::Vault);
    match opened {
        Ok(mut store) => {
            p.opened = 1;
            crate::flashlog::line(&alloc::format!("kvault open ok in {} ms: {}", (crate::platform::cntpct() - t_open) / (crate::platform::cntfrq() / 1000), store.describe()));
            match store.get_by_key(LKEY) {
                Ok(Some(v)) if ferros_pieces::kv::genesis_of(&v).is_some() => p.verified = 1,
                Ok(other) => {
                    // A fresh (genesised) vault, or the genesis value missing: write it. Never a format: manifestus refuses to genesis over sealed blocks it cannot bootstrap.
                    crate::flashlog::line(&alloc::format!("kvault: genesis value {}, writing it", if other.is_some() { "wrong" } else { "absent" }));
                    let written = ferros_pieces::kv::genesis_document(crate::settings::eagle_now()).map(|doc| store.set(LKEY, &doc));
                    match written {
                        Some(Ok(_)) => p.sealed = 1,
                        other => {
                            crate::flashlog::line(&alloc::format!("kvault: genesis write FAILED: {:?}", other.map(|r| r.map(|_| ()))));
                            p.stage = 2
                        }
                    }
                }
                Err(e) => {
                    crate::flashlog::line(&alloc::format!("kvault: genesis read FAILED: {e:?}"));
                    p.stage = 3
                }
            }
            if let Ok(Some(_)) = store.get_by_key(LKEY) {
                let r = blake3::hash(store.describe().as_bytes());
                p.root16.copy_from_slice(&r.as_bytes()[..16]);
            }
            crate::checklist::step(crate::checklist::Step::Verify);
            kept = Some(store);
        }
        Err(e) => {
            crate::flashlog::line(&alloc::format!("kvault open FAILED (nothing formatted): {e:?}"));
            p.stage = 1
        }
    }
    // The vault is open; the registers hold the ira from here (keys.rs).
    crate::keys::wipe(ira);
    crumb(0xE3); // vault path returned
    (p, kept)
}

/// Write the vault probe into the kernel's HARDCODED ramoops print slots (ferros_handoff.c ferros_ramoops_read; see husky_ferros_probe_boot memory).
/// r[1]/0x08 `link_up=` := the wairua's fingerprint if KOHI has run, else 0 (differs each boot; never the wairua itself — this block is printed by Android's kernel); r[2]/0x10 bytes := opened|verified<<1|sealed<<2|wairua drawn<<3|link_up<<4, then stage, then ap_nz|asv_nz<<1|hpm_nz<<2; r[3]/0x18 `mbr_sig` := exit | dvfs<<8 (exit 0 = vault run done, G#Bx = USB service exit reason); r[4]/0x20 := ap_hw_tune[0..5]; r[5]/0x28 := sync exception count; r[20..24]/0xA0 `pattern-readback` := root_commit hash[0..16] — non-zero + stable across boots == the vault booted from the ira.
pub fn report_vault(p: &VaultProbe, exit: u8) {
    let wairua = crate::wairua::peek().is_some_and(|s| s.0);
    let f_open = (p.opened as u64) | ((p.verified as u64) << 1) | ((p.sealed as u64) << 2) | ((wairua as u64) << 3) | ((p.link_up as u64) << 4);
    let ap_nz = p.ap_tune.iter().any(|&x| x != 0) as u64;
    let flags = f_open | ((p.stage as u64) << 8) | ((ap_nz | ((p.asv_nz as u64) << 1) | ((p.hpm_nz as u64) << 2)) << 16);
    LAST_CRUMB.store(exit as u64, core::sync::atomic::Ordering::Relaxed);
    unsafe {
        let w = |off: usize, v: u64| write_volatile((RAMOOPS_RESULT + off) as *mut u64, v);
        w(0x00, RESULT_MAGIC);
        w(0x08, if wairua { crate::wairua::fingerprint().map(u64::from_le_bytes).unwrap_or(0) } else { 0 });
        w(0x10, flags);
        w(0x18, (exit as u64) | ((p.dvfs as u64) << 8));
        core::ptr::copy_nonoverlapping(p.ap_tune.as_ptr(), (RAMOOPS_RESULT + 0x20) as *mut u8, 5);
        w(0x28, crate::exception_count()); // r[5] wrote_ok := sync exceptions taken (0 expected; the sink resets on the first one, so nonzero here means a fault slipped past)
        core::ptr::copy_nonoverlapping(p.root16.as_ptr(), (RAMOOPS_RESULT + 0xA0) as *mut u8, 16);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 0x180);
    }
}

/// A reset into a bootloader mode asked for in a session the bootloader cannot hand it to (the jump from Android), carried across that reset in the vault for the cold boot after it (2026-11-15, the Pro: `bridge reboot fastboot` from the jump b875 showed the ROM's USB recovery and came back into ferros as b876).
/// Why the vault: every reset ferros has ordered from a jumped session ended in a power cycle (b863 to b864, b871 to b872, b875 to b876: DRAM decayed each time), the PMU word the mode is stamped in does not survive that, and ABL restores it after a power-on reset only from RSBM, the copy Linux keeps in the fuel gauge's storage (ABL's mode getter at G#C48C of ripcurrent-16.4-14540574), which ferros never writes.
/// A kernel-only name: no process lists or reaches it.
const NEXT_BOOT_KEY: &str = "\0boot.next";
fn next_boot_schema() -> vsf::schema::SectionSchema {
    vsf::schema::SectionSchema::new("next_boot").field("mode", vsf::schema::TypeConstraint::AnyUnsigned).field("from_boot", vsf::schema::TypeConstraint::AnyUnsigned)
}

/// Keep the wish: the reboot mode (platform REBOOT_MODE_*) and the flash log's number for this boot, so only the very next ferros boot takes it. True when the vault holds it, read back.
pub fn next_boot_keep(store: &mut ShimStore, mode: u32) -> bool {
    let boot = crate::flashlog::boot();
    // No flash log, no boot number: the next boot could not tell this wish from a stale one.
    if boot == 0 {
        return false;
    }
    let Some(section) = next_boot_schema().build().set("mode", vsf::VsfType::u(mode as usize, false)).ok().and_then(|b| b.set("from_boot", vsf::VsfType::u(boot as usize, false)).ok()).and_then(|b| b.encode().ok()) else { return false };
    let Some(doc) = vsf::VsfBuilder::new().creation_time_oscillations(crate::settings::eagle_now()).provenance_only().add_unboxed("next_boot", section).build().ok() else { return false };
    store.set(NEXT_BOOT_KEY, &doc).is_ok() && store.get_by_key(NEXT_BOOT_KEY).ok().flatten().as_deref() == Some(&doc[..])
}

/// At a cold boot, before anything else runs: the wish the previous boot kept, removed from the vault first (and its removal read back) so that a failure on the way can never repeat it. Some(mode) only when it came from the boot just before this one; a stale one is dropped and logged.
pub fn next_boot_take(store: &mut ShimStore) -> Option<u32> {
    let doc = store.get_by_key(NEXT_BOOT_KEY).ok().flatten()?;
    let gone = store.remove_key(NEXT_BOOT_KEY).is_ok() && store.get_by_key(NEXT_BOOT_KEY).ok().flatten().is_none();
    if !gone {
        crate::flashlog::line("next boot: a kept wish could not be removed, so it is not acted on");
        return None;
    }
    let section = vsf::schema::SectionBuilder::parse_document(next_boot_schema(), &doc, None).ok()?;
    let read = |name: &str| section.get_fields(name).first().and_then(|f| f.values.first()).and_then(|v| v.as_u64());
    let (mode, from) = (read("mode")?, read("from_boot")?);
    let boot = crate::flashlog::boot();
    // WHY the boot == 0 test: 0 means no flash log this boot, where `boot - 1` would wrap.
    // PROOF: flashlog::boot is 0 before init or without a log, otherwise at least 1; `from` is the disk's and is only compared, never added to.
    if boot == 0 || from != boot - 1 {
        crate::flashlog::line(&alloc::format!("next boot: a wish for mode G#{mode:X} from b{from} dropped (this is b{boot})"));
        return None;
    }
    Some(mode as u32)
}
