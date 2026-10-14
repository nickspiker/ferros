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

/// Last crumb stamped by `entry_vault`, so `report_panic` can say how far the run got.
static LAST_CRUMB: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Panic → ramoops report + PSCI warm reset (shim build's `#[panic_handler]`).
/// Slots (kernel print names): r[3]/0x18 `mbr_sig` := G#EE00 | last crumb; r[1]/0x08 `link_up` := first decimal number in the panic message (e.g. the failed allocation size), 0 if none; r[2]/0x10 bytes := heap free bytes (3 bytes LE) + free block count (`scsi_status`), all FF if the heap lock was held; r[4]/0x20 `read..verify` := line (u32 LE) + column byte; r[5]/0x28 `wrote_ok` := sync exception count; 0xA0 `pattern-readback` := last 8 bytes of the panicking file's path + first 8 bytes of the message.
pub fn report_panic(info: &core::panic::PanicInfo) -> ! {
    struct Head { buf: [u8; 64], n: usize }
    impl core::fmt::Write for Head {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for &b in s.as_bytes() {
                if self.n < 64 { self.buf[self.n] = b; self.n += 1; }
            }
            Ok(())
        }
    }
    let mut head = Head { buf: [0u8; 64], n: 0 };
    let _ = core::fmt::write(&mut head, format_args!("{}", info.message()));
    // First decimal run in the message.
    let mut num = 0u64;
    let mut seen = false;
    for &b in &head.buf[..head.n] {
        if b.is_ascii_digit() {
            seen = true;
            num = num.wrapping_mul(10).wrapping_add((b - b'0') as u64);
        } else if seen {
            break;
        }
    }
    let heap = match crate::SHIM_ALLOC.try_lock() {
        Some(h) => ((h.free_bytes() as u64) & 0xFF_FFFF) | (((h.free_blocks() as u64) & 0xFF) << 24),
        None => 0xFFFF_FFFF,
    };
    let (line, col, file) = match info.location() {
        Some(l) => (l.line(), l.column(), l.file()),
        None => (0, 0, ""),
    };
    let fb = file.as_bytes();
    let tail = &fb[fb.len().saturating_sub(8)..];
    let mut t = [0u8; 16];
    t[..tail.len()].copy_from_slice(tail);
    t[8..].copy_from_slice(&head.buf[..8]);
    unsafe {
        let w = |off: usize, v: u64| write_volatile((RAMOOPS_RESULT + off) as *mut u64, v);
        w(0x00, RESULT_MAGIC);
        w(0x08, num);
        w(0x10, heap);
        w(0x18, 0xEE00 | (LAST_CRUMB.load(core::sync::atomic::Ordering::Relaxed) & 0xFF));
        w(0x20, (line as u64) | (((col as u64) & 0xFF) << 32));
        w(0x28, crate::exception_count());
        core::ptr::copy_nonoverlapping(t.as_ptr(), (RAMOOPS_RESULT + 0xA0) as *mut u8, 16);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 0x180);
        core::arch::asm!("dsb sy");
        core::arch::asm!(
            "mov w0, #0x0009",
            "movk w0, #0x8400, lsl #16",
            "smc #0",
            options(nomem, nostack, noreturn),
        );
    }
}

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

    match Store::format(dev, AnchorKey([0x5Au8; 32]), DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE, alloc::boxed::Box::new(crate::wairua::TrngNonce::new())) {
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

/// Adopt the live UFS link Linux left running: bypass the HSI2 S2MPU, clear UFS IOCC coherency (ferros does non-coherent DMA), rebase the transfer list onto ferros's own descriptors, clear the stale vendor IS. Returns whether the link reports up.
pub fn adopt_ufs() -> bool {
    use ferros_hal::ufs::UfsController;
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
    ufs.link_is_up()
}

/// Cluster watchdogs (s3c2410 block: WTCON +0, WTDAT +4, WTCNT +8; RSTEN b0, EN b5, DIV128 3<<3, PRESCALE 0xFF<<8). Armed at the longest window (~1-2 min); a pure spin with no pets resets warm instead of hanging forever.
const WATCHDOGS: [usize; 2] = [0x1006_0000, 0x1007_0000];

/// Make a hang self-reporting and self-recovering (no button reset, which cold-resets and wipes pmsg).
/// 1. Fault sink: the sync exception handler records the first fault (ELR/FAR/ESR/count) into the result block and warm-resets, so the next Linux boot prints it. The pmsg zone can come up filled with FF (seen on hardware), which would defeat the sink's "already recorded" guard on 0xA0 and pollute every unwritten slot, so the report slots are zeroed first.
/// 2. Cluster watchdogs armed.
pub fn arm_safety() {
    unsafe {
        for off in [0x08usize, 0x10, 0x20, 0x28, 0xA0, 0xA8] {
            write_volatile((RAMOOPS_RESULT + off) as *mut u64, 0);
        }
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 0x180);
        write_volatile(&raw mut crate::__exc_sink, RAMOOPS_RESULT as u64);
    }
    const WDT_ARM: u32 = 0xFF00 | (3 << 3) | (1 << 5) | (1 << 0);
    for &wdt in &WATCHDOGS {
        unsafe {
            write_volatile((wdt + 0x04) as *mut u32, 0xFFFF);
            write_volatile((wdt + 0x08) as *mut u32, 0xFFFF);
            write_volatile(wdt as *mut u32, WDT_ARM);
        }
    }
}

/// Reload both cluster watchdog counters. Loops that legitimately run long (the USB service loop) call this every iteration; a real hang stops the pets and the watchdog resets warm.
pub fn pet_watchdogs() {
    for &wdt in &WATCHDOGS {
        unsafe { write_volatile((wdt + 0x08) as *mut u32, 0xFFFF) };
    }
}

/// Breadcrumb at G#18 (kernel prints it as `mbr_sig`): bumped per step so a stall leaves the last marker; the completed report writes low byte 0 (or an exit reason).
pub fn crumb(c: u64) {
    LAST_CRUMB.store(c, core::sync::atomic::Ordering::Relaxed);
    unsafe {
        write_volatile((RAMOOPS_RESULT + 0x00) as *mut u64, RESULT_MAGIC);
        write_volatile((RAMOOPS_RESULT + 0x18) as *mut u64, c);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 0x20);
    }
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
    pub wairua_head: u64,
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

    let key: AnchorKey = crate::ira::anchor_key(); // ira-rooted key from the validated chipid trims (derive_ira v1)
    crumb(0xC1); // anchor_key (chipid trims + BLAKE3 ira) returned

    let wairua = crate::wairua::draw(); // TRNG (SMC then RNDR fallback)
    crumb(0xC2);
    let (wairua_ok, wairua_head) = match wairua {
        Some(w) => {
            let mut h = [0u8; 8];
            h.copy_from_slice(&w[..8]);
            (1u8, u64::from_le_bytes(h))
        }
        None => (0u8, 0),
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
        wairua_head,
        ap_tune: crate::ira::read_ap_hw_tune(),
        asv_nz: nz(&crate::ira::read_asv_tbl()),
        hpm_nz: nz(&crate::ira::read_hpm_asv()),
        dvfs: crate::ira::read_dvfs_version(),
    };

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
/// r[1]/0x08 `link_up=` := wairua (differs each boot); r[2]/0x10 bytes := opened|verified<<1|sealed<<2|wairua_ok<<3|link_up<<4, then stage, then ap_nz|asv_nz<<1|hpm_nz<<2; r[3]/0x18 `mbr_sig` := exit | dvfs<<8 (exit 0 = vault run done, G#Bx = USB service exit reason); r[4]/0x20 := ap_hw_tune[0..5]; r[5]/0x28 := sync exception count; r[20..24]/0xA0 `pattern-readback` := root_commit hash[0..16] — non-zero + stable across boots == the vault booted from the ira.
pub fn report_vault(p: &VaultProbe, exit: u8) {
    let f_open = (p.opened as u64) | ((p.verified as u64) << 1) | ((p.sealed as u64) << 2) | ((p.wairua_ok as u64) << 3) | ((p.link_up as u64) << 4);
    let ap_nz = p.ap_tune.iter().any(|&x| x != 0) as u64;
    let flags = f_open | ((p.stage as u64) << 8) | ((ap_nz | ((p.asv_nz as u64) << 1) | ((p.hpm_nz as u64) << 2)) << 16);
    LAST_CRUMB.store(exit as u64, core::sync::atomic::Ordering::Relaxed);
    unsafe {
        let w = |off: usize, v: u64| write_volatile((RAMOOPS_RESULT + off) as *mut u64, v);
        w(0x00, RESULT_MAGIC);
        w(0x08, p.wairua_head);
        w(0x10, flags);
        w(0x18, (exit as u64) | ((p.dvfs as u64) << 8));
        core::ptr::copy_nonoverlapping(p.ap_tune.as_ptr(), (RAMOOPS_RESULT + 0x20) as *mut u8, 5);
        w(0x28, crate::exception_count()); // r[5] wrote_ok := sync exceptions taken (0 expected; the sink resets on the first one, so nonzero here means a fault slipped past)
        core::ptr::copy_nonoverlapping(p.root16.as_ptr(), (RAMOOPS_RESULT + 0xA0) as *mut u8, 16);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 0x180);
    }
}

/// Exynos EL3 monitor "privileged register" call: the secure monitor performs the MMIO access for us, so registers the S2MPU blocks from EL2 (the PMU) are still reachable. Same call Linux's set_priv_reg/get_priv_reg make (exynos-pd_el3.c): x0 = SMC_CMD_PRIV_REG, x1 = physical address, x2 = 0 read / 1 write, x3 = value. Returns x0: 0 on a successful write, the register value on a read.
const SMC_CMD_PRIV_REG: u64 = 0x8200_0504;

pub fn priv_reg(addr: u64, write: bool, val: u32) -> u64 {
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") SMC_CMD_PRIV_REG => ret,
            inout("x1") addr => _,
            inout("x2") write as u64 => _,
            inout("x3") val as u64 => _,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
        );
    }
    ret
}

/// PMU SYSIP_DAT0 (pmu_alive G#1546_0000 + reboot-cmd-offset G#810): the reboot mode the bootloader reads after a reset. Values from pixel-zuma-reboot.c: G#FC bootloader, G#FA fastbootd, G#FF recovery, 0 normal.
pub const PMU_REBOOT_CMD: u64 = 0x1546_0810;
pub const REBOOT_MODE_BOOTLOADER: u32 = 0xFC;

/// Debug-snapshot (DSS) header, reserved-memory `header` node (G#D810_0000, 64 KiB, no-map). Offsets from debug-snapshot-def.h. The bootloader reads it to classify the last reset: Linux's clean-reboot path (dbg_snapshot_restart_handler) stamps EMERGENCY_REASON = DSS_SIGN_NORMAL_REBOOT, clears SCRATCH and every core's PANIC_STAT. Without that stamp the bootloader logs an unexpected "PIN Reset" (reason G#CECA) and boots normally with the reboot mode ignored — measured on hardware: mode G#FC written via EL3, bootloader read "Reboot mode: 0x0".
pub const DSS_HEADER: usize = 0xD810_0000;
pub const DSS_OFFSET_SCRATCH: usize = 0x100;
pub const DSS_OFFSET_EMERGENCY_REASON: usize = 0x300;
const DSS_OFFSET_PANIC_STAT: usize = 0x500;
const DSS_SIGN_NORMAL_REBOOT: u32 = 0xCAFE;
const NR_CPUS: usize = 9;

/// Mark the coming reset as a clean reboot in the DSS header, exactly as Linux's restart handler does.
pub fn dss_mark_clean_reboot() {
    unsafe {
        write_volatile((DSS_HEADER + DSS_OFFSET_EMERGENCY_REASON) as *mut u32, DSS_SIGN_NORMAL_REBOOT);
        write_volatile((DSS_HEADER + DSS_OFFSET_SCRATCH) as *mut u32, 0);
        for cpu in 0..NR_CPUS {
            write_volatile((DSS_HEADER + DSS_OFFSET_PANIC_STAT + cpu * 4) as *mut u32, 0);
        }
        ferros_hal::mmio::cache_clean(DSS_HEADER, 0x600);
        core::arch::asm!("dsb sy");
    }
}

/// Reset straight into the bootloader (fastboot): mark the reset clean in the DSS header, stamp the reboot mode through the EL3 monitor, then fire the cluster watchdog with a count of 1.
/// Why the watchdog and not PSCI: measured on husky, a PSCI SYSTEM_RESET issued by ferros comes back as RST_STAT PIN_RESET (reason G#CECA) and the bootloader reads "Reboot mode: 0x0" even though the EL3 readback showed G#FC just before; an APC watchdog reset (RST_STAT CLUSTER0_NONCPU_WDTRESET) with the same mode stamped boots into fastboot ("Reboot mode: 0xfc - Bootloader"). Linux's own PSCI reset comes back as PIN_RESET|PO_RESET with the mode intact, so EL3 treats the two differently; the watchdog sidesteps whatever that is.
/// Returns only if the monitor refused the write (nonzero) or the watchdog never fired, so the caller can report it.
pub fn reboot_to_bootloader() -> u64 {
    dss_mark_clean_reboot();
    let r = priv_reg(PMU_REBOOT_CMD, true, REBOOT_MODE_BOOTLOADER);
    if r != 0 {
        return r;
    }
    // WTCON: RSTEN b0 + EN b5, prescaler 0, DIV16 (0 << 3); WTDAT = WTCNT = 1 → reset on the next watchdog tick.
    for &wdt in &WATCHDOGS {
        unsafe {
            write_volatile(wdt as *mut u32, 0);
            write_volatile((wdt + 0x04) as *mut u32, 1);
            write_volatile((wdt + 0x08) as *mut u32, 1);
            write_volatile(wdt as *mut u32, (1 << 5) | (1 << 0));
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    for _ in 0..(1u32 << 28) {
        core::hint::spin_loop();
    }
    u64::MAX
}

/// PSCI SYSTEM_RESET: warm reset, DRAM (and the ramoops report) survives, the bootloader boots Android, which prints the report.
pub fn psci_reset() -> ! {
    unsafe {
        core::arch::asm!(
            "dsb sy",
            "mov w0, #0x0009",
            "movk w0, #0x8400, lsl #16",
            "smc #0",
            options(nomem, nostack, noreturn),
        );
    }
}

/// Persistence entry: open the ferros vault if one exists, else genesis + seal it, report, warm-reset. Proven on hardware: root stable across boots, opened=1 verified=1.
pub fn entry_vault(x0: u64) -> ! {
    let link_up = adopt_ufs();
    arm_safety();
    let (p, _store) = vault_probe(link_up);
    report_vault(&p, 0);
    let _ = (x0, SCRATCH, MAGIC, STAGE_M0, STAGE_M1, CRUMB_ENTRY, CRUMB_RUST);
    psci_reset()
}
