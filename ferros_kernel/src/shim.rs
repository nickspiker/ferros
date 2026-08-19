//! Post-handoff entry — ferros chainloaded from a running Linux shim.
//!
//! The shim kernel (Google GKI + exynos UFS module, built from source) brings the machine up far enough that the UFS link is alive, then jumps here via the kexec primitive `cpu_soft_restart(is_hyp_nvhe(), 0x92400000, ...)`.
//! That re-enters at EL2 with the MMU off — exactly the state ferros's `_entry` fast path expects (it detects MMU-off and skips the cache/MMU teardown).
//!
//! The whole point: whatever off-chip / analog / device-firmware condition the working Linux bring-up satisfies and ferros's own link startup does not, we stop needing to know — Linux satisfies it, and ferros adopts the live link.
//!
//! M0 (this file) proves only the jump: stamp a result marker into the reserved scratch and spin.
//! The next Linux boot reads the scratch back (ferros_scratch initcall in the shim kernel) — definitive even if no display is attached.
//! M1 layers the UFS adoption + SCSI read on top, writing block data to the same scratch.

use core::ptr::write_volatile;

/// Result scratch — last 1 MB of the ferros_handoff carveout (zuma-rmem.dtsi:
/// ferros_handoff@92400000, 12 MB, no-map). Placed well past ferros's image +
/// 64 KB stack (all under ~3 MB from base), so writing it never touches code.
pub const SCRATCH: usize = 0x92F0_0000;

/// Proof ferros executed after the handoff. ASCII "FeRoShIm", little-endian.
pub const MAGIC: u64 = 0x6D_49_68_53_6F_52_65_46;

/// Milestone tag written at SCRATCH+16 so the readback distinguishes builds.
pub const STAGE_M0: u64 = 0x4D30; // "M0"
pub const STAGE_M1: u64 = 0x4D31; // "M1"

/// pixel8 / Tensor G3 UFS host base (UFSHCI). Same value the cold-boot path uses.
const UFS_BASE: usize = 0x1320_0000;
/// HSI2 S2MPU gating UFS DMA.
/// Bypassed so the UFS master DMAs to physical carveout addresses (zuma UFS has no stage-1 SysMMU — S2MPU is the only gate).
const S2MPU_HSI2: usize = 0x131F_0000;

/// Result drop inside the ramoops pmsg zone (zuma-rmem ramoops_mem@fd3ff000: console [0xfd3ff000,0xfd5ff000), pmsg [0xfd5ff000,0xfd7ff000)).
/// Unlike the low carveout scratch, this DRAM survives a reset (bootloader preserves ramoops), so the next boot's early initcall can read what ferros left here.
const RAMOOPS_RESULT: usize = 0xFD60_0000;
/// Marks a valid ferros result block at RAMOOPS_RESULT. "FeRoUfS1" LE.
const RESULT_MAGIC: u64 = 0x3153_6655_6F52_6546;

/// Diagnostic breadcrumb slots (survive a crash for the kernel readback):
///   +0x100 = 0xBEE1 written by _entry asm  (jump landed, ferros started)
///   +0x108 = 0xBEE2 written here in Rust   (BSS zero + stack + kernel_main OK)
pub const CRUMB_ENTRY: usize = SCRATCH + 0x100;
pub const CRUMB_RUST: usize = SCRATCH + 0x108;

/// M0 entry: stamp the scratch, prove the exception level, spin.
pub fn entry(x0: u64) -> ! {
    // Rust reached: BSS was zeroed, stack is set, kernel_main dispatched here.
    unsafe {
        write_volatile(CRUMB_RUST as *mut u64, 0xBEE2);
        ferros_hal::mmio::cache_clean(CRUMB_RUST, 8);
    }

    let el: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack));
    }

    unsafe {
        write_volatile(SCRATCH as *mut u64, MAGIC);
        write_volatile((SCRATCH + 8) as *mut u64, x0); // the arg Linux handed us (dtb phys or 0)
        write_volatile((SCRATCH + 16) as *mut u64, STAGE_M0);
        write_volatile((SCRATCH + 24) as *mut u64, el >> 2); // landed EL — must read 2
        // MMU is off here, so these stores go straight to DRAM; clean to PoC anyway so the value survives a warm reset.
        ferros_hal::mmio::cache_clean(SCRATCH, 64);
    }

    // Success signal that does not depend on DRAM persistence: trigger a fast, clean PSCI SYSTEM_RESET.
    // If the handoff worked, the phone reboots in a few seconds (vs the ~168s hang-then-fallback of a crash).
    // A warm PSCI reset may also preserve the scratch, so the recovery boot can read the magic.
    // PSCI SYSTEM_RESET = 0x8400_0009; SMC from EL2 traps to EL3 (TF-A).
    unsafe {
        core::arch::asm!(
            "mov w0, #0x0009",
            "movk w0, #0x8400, lsl #16",
            "smc #0",
            options(nomem, nostack, noreturn),
        );
    }
}

/// M1 entry: adopt the live UFS link Linux brought up and read LBA 0.
///
/// Wired once M0 proves the jump: the shim kernel's trigger moves from the M0 late_initcall to a hook in ufs-exynos.c's link-up POST path, so the handoff fires only after Linux has the UFS link fully up (HS-G4, device initialized).
/// linkstartup and fDeviceInit are skipped entirely here — so whatever off-chip condition made ferros's own link startup fail never gets exercised.
///
/// Scratch layout (all little-endian, read back by the M1 initcall):
///   [0]=MAGIC [1]=x0 [2]=STAGE_M1 [3]=landed_EL
///   [4]=link_up [5]=nop_ocs|nop_rsp<<8|read_ocs<<16|status<<24
///   +64: first 64 bytes of the block read from LBA 0
pub fn entry_m1(x0: u64) -> ! {
    use ferros_hal::ufs::UfsController;
    use core::ptr::write_volatile;

    // Bypass the HSI2 S2MPU so the UFS master DMAs to our physical carveout buffers.
    // (zuma UFS has no stage-1 SysMMU; disabling this DMA gate does not disturb the live M-PHY/UniPro link.)
    unsafe {
        write_volatile((S2MPU_HSI2 + 0x54) as *mut u32, 0xFF);
        write_volatile((S2MPU_HSI2 + 0x00) as *mut u32, 0x00);
        // Non-coherent DMA: ferros runs MMU-off (SCTLR.C=0), out of the coherency domain, so the master reads DRAM directly where our cache-cleaned descriptors live.
        // (0x13020710 = sysreg_ufs iocc.)
        write_volatile(0x1302_0710 as *mut u32, 0);
        core::arch::asm!("dsb sy");
    }

    let rd = |off: usize| unsafe { core::ptr::read_volatile((UFS_BASE + off) as *const u32) };
    let wr = |off: usize, v: u32| unsafe { core::ptr::write_volatile((UFS_BASE + off) as *mut u32, v) };
    // Exynos vendor HCI block (reg_hci @ 0x13201100) — separate from the std base.
    const HCI: usize = 0x1320_1100;
    let hci_r = |off: usize| unsafe { core::ptr::read_volatile((HCI + off) as *const u32) };
    let hci_w = |off: usize, v: u32| unsafe { core::ptr::write_volatile((HCI + off) as *mut u32, v) };

    let ufs = UfsController::new(UFS_BASE);

    // Adopt the controller: rebase the transfer list onto our own descriptors.
    // Clear IS by writing back the read value (standard W1C) — NOT 0xFFFFFFFF, which would set the Exynos non-W1C vendor bits.
    // UTRLBA is only sampled while the list is stopped (RSR=0).
    let ferros_utrd = ufs.utrd_phys();
    let is_pending = rd(0x20);
    wr(0x60, 0);
    wr(0x50, ferros_utrd as u32);
    wr(0x54, (ferros_utrd >> 32) as u32);
    wr(0x20, is_pending);
    wr(0x60, 1);
    hci_w(0x38, 0x0018_0000); // clear any stale vendor_IS error bits

    // Per-command HCI_UTRL_NEXUS_TYPE, exactly as the driver's setup_xfer_req: SCSI command sets the tag's bit, a NOP / device-mgmt command clears it.
    // The adopt-path skips full_init so this per-command step is ours to do; getting it wrong wedges the transfer engine (this was THE dead-doorbell bug).
    let nexus = |scsi: bool| {
        let t = hci_r(0x40);
        hci_w(0x40, if scsi { t | 1 } else { t & !1 });
        unsafe { core::arch::asm!("dsb sy") };
    };

    let link_up = ufs.link_is_up();

    // NOP OUT (device management): proves the doorbell round-trips.
    nexus(false);
    let (nop_ocs, nop_rsp) = ufs.live_nop();

    // READ(10) LBA 0 (SCSI): confirm real data — GPT protective MBR sig 0xAA55.
    nexus(true);
    let r0_ocs = ufs.read_block(0);
    let r0_status = ufs.last_response_status();
    let mbr_sig = {
        let d = ufs.data_buffer();
        (d[510] as u16) | ((d[511] as u16) << 8) // expect 0xAA55
    };

    // WRITE verification.
    // A scratch LBA deep in userdata (2449894..62436347 in 4KB blocks); ~120 GB in, encrypted free-ish space, and we RESTORE the original after — so a live Android sees no change.
    // read -> pattern-write -> read-verify -> restore-write -> read-verify.
    const SCRATCH_LBA: u32 = 30_000_000;
    let mut saved = [0u8; 4096];
    let mut pat_readback = [0u8; 16];
    nexus(true);
    let rs_ocs = ufs.read_block(SCRATCH_LBA);
    saved.copy_from_slice(ufs.data_buffer());
    {
        let b = ufs.data_buffer_mut();
        for (i, x) in b.iter_mut().enumerate() {
            *x = 0xF0u8 ^ (i as u8);
        }
        b[..8].copy_from_slice(b"FERROSw!");
    }
    nexus(true);
    let w_ocs = ufs.write_block(SCRATCH_LBA);
    nexus(true);
    let rv_ocs = ufs.read_block(SCRATCH_LBA);
    pat_readback.copy_from_slice(&ufs.data_buffer()[..16]);
    let wrote_ok = &ufs.data_buffer()[..8] == b"FERROSw!"
        && ufs.data_buffer()[100] == (0xF0u8 ^ 100);
    // restore the original block
    ufs.data_buffer_mut().copy_from_slice(&saved);
    nexus(true);
    let restore_ocs = ufs.write_block(SCRATCH_LBA);
    nexus(true);
    let rr_ocs = ufs.read_block(SCRATCH_LBA);
    let restored_ok = ufs.data_buffer()[..] == saved[..];

    // Report to ramoops (survives the reset; next-boot initcall prints it).
    //   +0x00 magic  +0x08 link_up  +0x10 nop/read ocs word  +0x18 mbr_sig
    //   +0x20 write-test ocs word  +0x28 wrote_ok|restored_ok
    //   +0xA0 pattern readback (16 B)
    unsafe {
        let w = |off: usize, v: u64| write_volatile((RAMOOPS_RESULT + off) as *mut u64, v);
        w(0x00, RESULT_MAGIC);
        w(0x08, link_up as u64);
        w(0x10, (nop_ocs as u64) | ((nop_rsp as u64) << 8)
            | ((r0_ocs as u64) << 16) | ((r0_status as u64) << 24));
        w(0x18, mbr_sig as u64);
        w(0x20, (rs_ocs as u64) | ((w_ocs as u64) << 8) | ((rv_ocs as u64) << 16)
            | ((restore_ocs as u64) << 24) | ((rr_ocs as u64) << 32));
        w(0x28, (wrote_ok as u64) | ((restored_ok as u64) << 8));
        core::ptr::copy_nonoverlapping(pat_readback.as_ptr(), (RAMOOPS_RESULT + 160) as *mut u8, 16);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 192);
    }

    let _ = (x0, SCRATCH, MAGIC, STAGE_M1);
    // Warm reboot (SYSTEM_RESET) — the ramoops dump is the result signal.
    unsafe {
        core::arch::asm!(
            "mov w0, #0x0009",
            "movk w0, #0x8400, lsl #16",
            "smc #0",
            options(nomem, nostack, noreturn),
        );
    }
}

/// Genesis entry: adopt the live UFS link, then format a fresh ferros vault onto the ferros partition (sda35, LBA 28_881_920..) and prove a store/retrieve round-trip through the real `Store<UfsDevice>` — the same append-only backend the host tests exercise, now driving flash over the adopted link.
///
/// We do NOT re-open on-device to prove persistence: `Store::open` allocates `device.capacity()` bytes (128 GiB here) — a Phase-2 fix.
/// Persistence is verified out-of-band from rooted Android: `dd` the partition, decode with vaultinfo.
/// format/put/get never allocate capacity-sized buffers, so they run fine in the carveout heap.
pub fn entry_genesis(x0: u64) -> ! {
    use crate::ufs_device::UfsDevice;
    use ferros_hal::ufs::UfsController;
    use ferros_vault::anchor::AnchorKey;
    use ferros_vault::backend::{Store, DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE};
    use ferros_vault::object::{Object, VsfType};
    use ferros_vault::store::ObjectStore;

    // --- adopt the live link (identical prologue to entry_m1) ---
    unsafe {
        write_volatile((S2MPU_HSI2 + 0x54) as *mut u32, 0xFF);
        write_volatile((S2MPU_HSI2 + 0x00) as *mut u32, 0x00);
        write_volatile(0x1302_0710 as *mut u32, 0);
        core::arch::asm!("dsb sy");
    }
    let rd = |off: usize| unsafe { core::ptr::read_volatile((UFS_BASE + off) as *const u32) };
    let wr = |off: usize, v: u32| unsafe { core::ptr::write_volatile((UFS_BASE + off) as *mut u32, v) };
    let hci_w = |off: usize, v: u32| unsafe { core::ptr::write_volatile((0x1320_1100 + off) as *mut u32, v) };

    let ufs = UfsController::new(UFS_BASE);
    let ferros_utrd = ufs.utrd_phys();
    let is_pending = rd(0x20);
    wr(0x60, 0);
    wr(0x50, ferros_utrd as u32);
    wr(0x54, (ferros_utrd >> 32) as u32);
    wr(0x20, is_pending);
    wr(0x60, 1);
    hci_w(0x38, 0x0018_0000); // clear stale vendor_IS
    let link_up = ufs.link_is_up();
    drop(ufs);

    // --- genesis + round-trip on the ferros partition ---
    // Window the device to 1 MiB so put/get's whole-file read (device.capacity() bytes)
    // fits the carveout heap. The vault lives in the first 256 blocks of the partition.
    const MSG: &[u8] = b"FERROS-GENESIS-0";
    let dev = UfsDevice::ferros_windowed(UfsController::new(UFS_BASE), 256);

    let mut format_ok = 0u8;
    let mut put_ok = 0u8;
    let mut get_ok = 0u8;
    let mut verify_ok = 0u8;
    let mut stage = 0u8; // 0 all-ok; 1 format 2 put 3 get 4 verify failed
    let mut root16 = [0u8; 16];

    match Store::format(dev, AnchorKey([0x5Au8; 32]), DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE) {
        Ok(mut store) => {
            format_ok = 1;
            root16.copy_from_slice(&store.root_commit_hash().0[..16]);
            let obj = Object::content_addressed(VsfType::Record, MSG.to_vec());
            match store.put(obj) {
                Ok(h) => {
                    put_ok = 1;
                    match store.get(&h) {
                        Ok(got) => {
                            get_ok = 1;
                            verify_ok = (got.content.as_slice() == MSG) as u8;
                            if verify_ok == 0 {
                                stage = 4;
                            }
                        }
                        Err(_) => stage = 3,
                    }
                }
                Err(_) => stage = 2,
            }
        }
        Err(_) => stage = 1,
    }

    // Report into entry_m1's ramoops layout (kernel reader prints the same fields;
    // reinterpret: +0x10 bytes = format|put|get|verify, +0x18 = stage, pattern = root[..16]).
    unsafe {
        let w = |off: usize, v: u64| write_volatile((RAMOOPS_RESULT + off) as *mut u64, v);
        w(0x00, RESULT_MAGIC);
        w(0x08, link_up as u64);
        w(0x10, (format_ok as u64) | ((put_ok as u64) << 8) | ((get_ok as u64) << 16) | ((verify_ok as u64) << 24));
        w(0x18, stage as u64);
        w(0x20, 0);
        w(0x28, 0);
        core::ptr::copy_nonoverlapping(root16.as_ptr(), (RAMOOPS_RESULT + 160) as *mut u8, 16);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 192);
    }

    let _ = (x0, SCRATCH, MAGIC, STAGE_M0, STAGE_M1, CRUMB_ENTRY, CRUMB_RUST);
    unsafe {
        core::arch::asm!(
            "mov w0, #0x0009",
            "movk w0, #0x8400, lsl #16",
            "smc #0",
            options(nomem, nostack, noreturn),
        );
    }
}

/// Persistence entry: the real boot pattern — open the ferros vault if one exists,
/// else genesis + seal it — proving a populated vault survives a power cycle.
///
/// Boot 1 (empty/unsealed partition): format, put "FERROS-GENESIS-0", then `commit_root` — the store's durable seal (advances the on-disk anchor's object_tail past the object + read-back-verifies).
/// Reports opened=?, sealed=1.
/// Boot 2 (after a real warm reset): open succeeds, the anchor scan rediscovers the object, `get` returns it byte-exact.
/// Reports opened=1, verified=1 — PERSISTED ACROSS REBOOT.
/// Idempotent thereafter.
///
/// Spans the whole partition (bounded reads mean the store never allocs capacity()), stores by logical key through the vault's keyed API, and runs on the freeing free-list allocator.
pub fn entry_vault(x0: u64) -> ! {
    use crate::ufs_device::UfsDevice;
    use ferros_hal::ufs::UfsController;
    use ferros_vault::anchor::AnchorKey;
    use ferros_vault::backend::{Store, DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE};

    // --- adopt the live link (identical prologue to entry_genesis) ---
    unsafe {
        write_volatile((S2MPU_HSI2 + 0x54) as *mut u32, 0xFF);
        write_volatile((S2MPU_HSI2 + 0x00) as *mut u32, 0x00);
        write_volatile(0x1302_0710 as *mut u32, 0);
        core::arch::asm!("dsb sy");
    }
    let rd = |off: usize| unsafe { core::ptr::read_volatile((UFS_BASE + off) as *const u32) };
    let wr = |off: usize, v: u32| unsafe { core::ptr::write_volatile((UFS_BASE + off) as *mut u32, v) };
    let hci_w = |off: usize, v: u32| unsafe { core::ptr::write_volatile((0x1320_1100 + off) as *mut u32, v) };

    let ufs = UfsController::new(UFS_BASE);
    let ferros_utrd = ufs.utrd_phys();
    let is_pending = rd(0x20);
    wr(0x60, 0);
    wr(0x50, ferros_utrd as u32);
    wr(0x54, (ferros_utrd >> 32) as u32);
    wr(0x20, is_pending);
    wr(0x60, 1);
    hci_w(0x38, 0x0018_0000);
    let link_up = ufs.link_is_up();
    drop(ufs);

    // --- open-or-genesis, by logical key ---
    const LKEY: &str = "genesis";
    const MSG: &[u8] = b"FERROS-GENESIS-0";
    const KEY: AnchorKey = AnchorKey([0x5Au8; 32]);

    let mut opened = 0u8;
    let mut verified = 0u8;
    let mut sealed = 0u8;
    let mut stage = 0u8; // 0 ok; 1 format 2 set 3 read failed
    let mut root16 = [0u8; 16];
    let mut need_seal = false;

    // Bounded reads (read_wrapped) mean the store never allocs capacity() — so the device spans the whole 128 GiB partition, no window.
    let fresh_dev = || UfsDevice::ferros(UfsController::new(UFS_BASE));

    match Store::open(fresh_dev(), KEY) {
        Ok(store) => {
            opened = 1;
            root16.copy_from_slice(&store.root_commit_hash().0[..16]);
            match store.get_by_key(LKEY) {
                Ok(Some(v)) if v.as_slice() == MSG => verified = 1,
                Ok(_) => need_seal = true, // vault present but key absent or mismatched
                Err(_) => stage = 3,
            }
        }
        Err(_) => need_seal = true, // no vault — genesis it
    }

    if need_seal {
        match Store::format(fresh_dev(), KEY, DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE) {
            Ok(mut store) => match store.set(LKEY, MSG.to_vec()) {
                Ok(_) => {
                    sealed = 1;
                    root16.copy_from_slice(&store.root_commit_hash().0[..16]);
                }
                Err(_) => stage = 2,
            },
            Err(_) => stage = 1,
        }
    }

    // Report (reuse the ramoops layout; reinterpret: +0x10 bytes = opened|verified|sealed,
    // +0x18 = stage, pattern-readback = root_commit[..16]).
    unsafe {
        let w = |off: usize, v: u64| write_volatile((RAMOOPS_RESULT + off) as *mut u64, v);
        w(0x00, RESULT_MAGIC);
        w(0x08, link_up as u64);
        w(0x10, (opened as u64) | ((verified as u64) << 8) | ((sealed as u64) << 16));
        w(0x18, stage as u64);
        w(0x20, 0);
        w(0x28, 0);
        core::ptr::copy_nonoverlapping(root16.as_ptr(), (RAMOOPS_RESULT + 160) as *mut u8, 16);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 192);
    }

    let _ = (x0, SCRATCH, MAGIC, STAGE_M0, STAGE_M1, CRUMB_ENTRY, CRUMB_RUST);
    unsafe {
        core::arch::asm!(
            "mov w0, #0x0009",
            "movk w0, #0x8400, lsl #16",
            "smc #0",
            options(nomem, nostack, noreturn),
        );
    }
}
