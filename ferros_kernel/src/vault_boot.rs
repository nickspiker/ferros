//! The ira-rooted vault on the ferros partition: open it (or genesis + seal it) over a UFS link that is already up, and report the result into the ramoops block. Needs a working UFS link, which today only the Linux-shim handoff provides (the cold boot's own link startup is still blocked, UFS.md).

use core::ptr::write_volatile;

use crate::platform::{crumb, LAST_CRUMB, RAMOOPS_RESULT, RESULT_MAGIC};

/// pixel8 / Tensor G3 UFS host base (UFSHCI).
const UFS_BASE: usize = 0x1320_0000;

/// Display fingerprints: the first 64 bits of BLAKE3 derive_key under a context of their own, so a fingerprint is one-way and matches no other hash of the same secret anywhere.
const IRA_FP_CONTEXT: &str = "ferros husky 2026-10-14 ira display fingerprint v1";
const WAIRUA_FP_CONTEXT: &str = "ferros husky 2026-10-14 wairua display fingerprint v1";

/// How long this boot's KOHI took, for DIAG.
pub static KOHI_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

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
    pub wairua_ok: u8,
    /// One-way fingerprints for display (`fingerprint`): the ira's is the same every boot, the wairua's differs every boot. Never the secrets themselves.
    pub ira_fp: [u8; 8],
    /// 0 = ira derived; else why not (`ira::derive_ira`: G#100 + source = reads disagreed, G#200 + source = degenerate, G#300 = devinfo records missing).
    pub ira_fail: u32,
    pub wairua_fp: [u8; 8],
    pub ap_tune: [u8; 32],
    pub asv_nz: bool,
    pub hpm_nz: bool,
    pub dvfs: u8,
}

/// The ira-keyed vault on the ferros partition, kept open by the USB service for `ferros.dev.store`.
pub type ShimStore = ferros_vault::backend::Store<crate::ufs_device::UfsDevice>;

/// The real boot pattern on the adopted link: open the ferros vault keyed by the ira, else genesis + seal it. Crumbs bracket each first-on-hardware step so a stall pins it. Returns the open store too (None if open and genesis both failed).
pub fn vault_probe(link_up: bool) -> (VaultProbe, Option<ShimStore>) {
    use crate::ufs_device::UfsDevice;
    use ferros_hal::ufs::UfsController;
    use ferros_vault::anchor::AnchorKey;
    use ferros_vault::backend::{Store, DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE};

    let ira = crate::ira::derive_ira(); // every stable per-device source, each read twice and checked (derive_ira v2)
    let ira_fail = ira.err().unwrap_or(0);
    let ira_fp = ira.map(|i| fingerprint(IRA_FP_CONTEXT, &i)).unwrap_or([0u8; 8]);
    crumb(0xC1); // ira derived (or refused)

    let t_kohi = crate::platform::cntpct();
    let wairua = crate::wairua::draw(); // this session's KOHI (run on first use)
    KOHI_MS.store(((crate::platform::cntpct() - t_kohi) / (crate::platform::cntfrq() / 1000)) as u32, core::sync::atomic::Ordering::Relaxed);
    crumb(0xC2);
    let (wairua_ok, wairua_fp) = match wairua {
        Some(w) => (1u8, fingerprint(WAIRUA_FP_CONTEXT, &w)),
        None => (0u8, [0u8; 8]),
    };

    let nz = |b: &[u8]| b.iter().any(|&x| x != 0);
    let mut p = VaultProbe {
        link_up,
        opened: 0,
        verified: 0,
        sealed: 0,
        stage: 0,
        root16: [0u8; 16],
        wairua_ok,
        ira_fp,
        wairua_fp,
        ap_tune: crate::ira::read_ap_hw_tune(),
        asv_nz: nz(&crate::ira::read_asv_tbl()),
        hpm_nz: nz(&crate::ira::read_hpm_asv()),
        dvfs: crate::ira::read_dvfs_version(),
        ira_fail,
    };

    // No valid ira, no vault (PIPE: refuse to operate). The probe reports why.
    let Ok(ira) = ira else {
        return (p, None);
    };
    let key: AnchorKey = AnchorKey::from_ira(&ira); // the ira-rooted vault key

    const LKEY: &str = "genesis";
    const MSG: &[u8] = b"FERROS-GENESIS-0";
    let mut need_seal = false;
    let mut kept: Option<ShimStore> = None;
    let fresh_dev = || UfsDevice::ferros(UfsController::new(UFS_BASE));

    crumb(0xE4);
    let dev0 = fresh_dev();
    crumb(0xE5); // UfsDevice constructed
    let nonce_src = crate::wairua::TrngNonce::new();
    crumb(0xE6); // TRNG seed drawn (SMC)
    let nonce0 = alloc::boxed::Box::new(nonce_src); // coerces to Box<dyn NonceSource> at the call
    crumb(0xE0); // first heap allocation done; before Store::open (UFS prefix read -> bounded read -> decode -> anchor decrypt)
    match Store::open(dev0, key, nonce0) {
        Ok(store) => {
            p.opened = 1;
            p.root16.copy_from_slice(&store.root_commit_hash().0[..16]);
            match store.get_by_key(LKEY) {
                Ok(Some(v)) if v.as_slice() == MSG => p.verified = 1,
                Ok(_) => need_seal = true, // vault present but key absent/mismatched
                Err(_) => p.stage = 3,
            }
            if !need_seal {
                kept = Some(store);
            }
        }
        Err(_) => need_seal = true, // no vault (or wrong key) — genesis it
    }
    crumb(0xE1); // Store::open returned
    if need_seal {
        crumb(0xE2); // before Store::format (seal + UFS write)
        match Store::format(fresh_dev(), key, DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE, alloc::boxed::Box::new(crate::wairua::TrngNonce::new())) {
            Ok(mut store) => match store.set(LKEY, MSG.to_vec()) {
                Ok(_) => {
                    p.sealed = 1;
                    p.root16.copy_from_slice(&store.root_commit_hash().0[..16]);
                    kept = Some(store);
                }
                Err(_) => p.stage = 2,
            },
            Err(_) => p.stage = 1,
        }
    }
    crumb(0xE3); // vault path returned
    (p, kept)
}

/// Write the vault probe into the kernel's HARDCODED ramoops print slots (ferros_handoff.c ferros_ramoops_read; see husky_ferros_probe_boot memory).
/// r[1]/0x08 `link_up=` := the wairua's fingerprint (differs each boot; never the wairua itself — this block is printed by Android's kernel); r[2]/0x10 bytes := opened|verified<<1|sealed<<2|wairua_ok<<3|link_up<<4, then stage, then ap_nz|asv_nz<<1|hpm_nz<<2; r[3]/0x18 `mbr_sig` := exit | dvfs<<8 (exit 0 = vault run done, G#Bx = USB service exit reason); r[4]/0x20 := ap_hw_tune[0..5]; r[5]/0x28 := sync exception count; r[20..24]/0xA0 `pattern-readback` := root_commit hash[0..16] — non-zero + stable across boots == the vault booted from the ira.
pub fn report_vault(p: &VaultProbe, exit: u8) {
    let f_open = (p.opened as u64) | ((p.verified as u64) << 1) | ((p.sealed as u64) << 2) | ((p.wairua_ok as u64) << 3) | ((p.link_up as u64) << 4);
    let ap_nz = p.ap_tune.iter().any(|&x| x != 0) as u64;
    let flags = f_open | ((p.stage as u64) << 8) | ((ap_nz | ((p.asv_nz as u64) << 1) | ((p.hpm_nz as u64) << 2)) << 16);
    LAST_CRUMB.store(exit as u64, core::sync::atomic::Ordering::Relaxed);
    unsafe {
        let w = |off: usize, v: u64| write_volatile((RAMOOPS_RESULT + off) as *mut u64, v);
        w(0x00, RESULT_MAGIC);
        w(0x08, u64::from_le_bytes(p.wairua_fp));
        w(0x10, flags);
        w(0x18, (exit as u64) | ((p.dvfs as u64) << 8));
        core::ptr::copy_nonoverlapping(p.ap_tune.as_ptr(), (RAMOOPS_RESULT + 0x20) as *mut u8, 5);
        w(0x28, crate::exception_count()); // r[5] wrote_ok := sync exceptions taken (0 expected; the sink resets on the first one, so nonzero here means a fault slipped past)
        core::ptr::copy_nonoverlapping(p.root16.as_ptr(), (RAMOOPS_RESULT + 0xA0) as *mut u8, 16);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 0x180);
    }
}
