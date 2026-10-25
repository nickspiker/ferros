//! Pixel 8 Pro (husky, Tensor G3) platform services shared by both boot paths — the cold boot from ABL and the Linux-shim handoff: the ramoops report block, crumbs, the panic reporter, cluster watchdogs, the EL3 privileged-register call, DSS clean-reboot marking, resets (PSCI, or straight into fastboot), the USB DMA path, and the PT framing helpers.

use core::ptr::{read_volatile, write_volatile};

use ferros_pt::transfer::OutboundTransfer;

/// Result drop inside the ramoops pmsg zone (zuma-rmem ramoops_mem@fd3ff000: console [0xfd3ff000,0xfd5ff000), pmsg [0xfd5ff000,0xfd7ff000)).
/// Unlike the low carveout scratch, this DRAM survives a reset (bootloader preserves ramoops), so the next boot's early initcall can read what ferros left here.
pub const RAMOOPS_RESULT: usize = 0xFD60_0000;
/// Marks a valid ferros result block at RAMOOPS_RESULT. "FeRoUfS1" LE.
pub const RESULT_MAGIC: u64 = 0x3153_6655_6F52_6546;

/// Last crumb stamped by either boot path, so `report_panic` can say how far the run got.
pub static LAST_CRUMB: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Panic → ramoops report + PSCI warm reset (the pixel8 `#[panic_handler]`, both boot paths).
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
    let heap = match crate::KERNEL_HEAP.try_lock() {
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
/// Set once a watchdog reset has been ordered (`reboot_to_bootloader`): from then on nothing pets the watchdogs. The vault pets them on every operation (kvault.rs), and the map writes the vault from the worker cores all the time, so the reset that `bridge reboot fastboot` waits for never came (2026-10-24: 64 s of a frozen service loop, USB gone, ferros still running after).
static RESET_ORDERED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn pet_watchdogs() {
    if RESET_ORDERED.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    for &wdt in &WATCHDOGS {
        unsafe { write_volatile((wdt + 0x08) as *mut u32, 0xFFFF) };
    }
}

/// Breadcrumb at G#18 (kernel prints it as `mbr_sig`): bumped per step so a stall leaves the last marker; the completed report writes low byte 0 (or an exit reason).
/// Where the post-mortem notes live: the ramoops result page, beside the crumb (+G#18) that has survived every kind of reset — slots 0-7 at +G#100, the previous boot's at +G#140. Every other home tried got overwritten by something at boot (2026-10-23: the gate, a log, the 17 MB gate image, the modem firmware, and G#B000_0000 read back as noise).
pub const NOTES: usize = RAMOOPS_RESULT + 0x100;
/// Slot 0: the kernel lock's holder (core + 1, 0 = free; process.rs). Slots 1-5: cores 4-8's last crumb, the core in the top byte. Slot 6: the service loop's last crumb.
pub fn note(slot: usize, v: u64) {
    unsafe {
        let a = NOTES + (slot & 7) * 8;
        write_volatile(a as *mut u64, v);
        core::arch::asm!("dc civac, {a}", a = in(reg) a as u64);
    }
}
pub fn note_read(slot: usize) -> u64 {
    unsafe { read_volatile((NOTES + (slot & 15) * 8) as *const u64) }
}

/// At boot, before any note is written: the previous boot's notes copied to RESULT+G#980 (`bridge read 819FF980 64`), and the live slots cleared — so a boot that hung leaves its last per-core crumbs readable from the boot after it.
pub fn notes_rotate() {
    unsafe {
        for i in 0..8 {
            let v = read_volatile((NOTES + i * 8) as *const u64);
            write_volatile((NOTES + 0x40 + i * 8) as *mut u64, v);
            write_volatile((NOTES + i * 8) as *mut u64, 0);
            core::arch::asm!("dc civac, {a}", "dc civac, {b}", a = in(reg) (NOTES + i * 8) as u64, b = in(reg) (NOTES + 0x40 + i * 8) as u64);
        }
        core::arch::asm!("dsb sy");
    }
}

pub fn crumb(c: u64) {
    LAST_CRUMB.store(c, core::sync::atomic::Ordering::Relaxed);
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
    let core = ((mpidr >> 8) & 0xF) as usize;
    if (4..=8).contains(&core) {
        note(core - 3, c | (core as u64) << 56);
    }
    unsafe {
        write_volatile((RAMOOPS_RESULT + 0x00) as *mut u64, RESULT_MAGIC);
        write_volatile((RAMOOPS_RESULT + 0x18) as *mut u64, c);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 0x20);
    }
}

/// Exynos EL3 monitor "privileged register" call: the secure monitor performs the MMIO access for us, so registers the S2MPU blocks from EL2 (the PMU) are still reachable. Same call Linux's set_priv_reg/get_priv_reg make (exynos-pd_el3.c): x0 = SMC_CMD_PRIV_REG, x1 = physical address, x2 = 0 read / 1 write, x3 = value. Returns x0: 0 on a successful write, the register value on a read.
const SMC_CMD_PRIV_REG: u64 = 0x8200_0504;

/// Read-modify-write through the same EL3 call (x2 = 2): the bits of `mask` in `addr` become `val`, as Linux's rmw_priv_reg does. Returns x0 (0 on success).
pub fn priv_rmw(addr: u64, mask: u32, val: u32) -> u64 {
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") SMC_CMD_PRIV_REG => ret,
            inout("x1") addr => _,
            inout("x2") 2u64 => _,
            inout("x3") mask as u64 => _,
            inout("x4") val as u64 => _,
            out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
        );
    }
    ret
}

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
/// `reboot_to_bootloader` result when the watchdog never reset the SoC. Distinct from every EL3 return: the monitor reports errors as negative values (e.g. -1 = G#FFFF_FFFF_FFFF_FFFF), which an earlier u64::MAX sentinel could not be told apart from.
pub const WATCHDOG_DID_NOT_FIRE: u64 = 0xDEAD_B007;

/// A plain reset through EL3 (PSCI SYSTEM_RESET). Measured on husky: the bootloader reads it as "Reboot mode: 0x0", a normal boot of the active slot — which is what the gate's Android choice wants after `slot::set_active(0)`.
pub fn reboot() -> ! {
    dss_mark_clean_reboot();
    unsafe { core::arch::asm!("smc #0", in("x0") 0x8400_0009u64, options(noreturn)) }
}

pub fn reboot_to_bootloader() -> u64 {
    dss_mark_clean_reboot();
    let r = priv_reg(PMU_REBOOT_CMD, true, REBOOT_MODE_BOOTLOADER);
    if r != 0 {
        return r;
    }
    RESET_ORDERED.store(true, core::sync::atomic::Ordering::Relaxed);
    // Fire the cluster watchdogs: count = reload = 1, prescaler 0, DIV16, RSTEN, so the counter expires every tick. Measured on husky with payloads/wdtfire: the reset is NOT immediate — these watchdogs reset the SoC about 15-20 s after they start expiring (a single expiry with the running configuration reset ~1 min later), so this waits up to 64 s by the clock. Writes are spaced 1 ms apart (the block sits behind a slow clock; back-to-back writes with the I-cache on were not proven). The bootloader then reads the stamped mode and stops in fastboot (RST_STAT CLUSTER0_NONCPU_WDTRESET, "Reboot mode: 0xfc").
    let ms = |n: u64| {
        let t0 = cntpct();
        while cntpct().wrapping_sub(t0) < cntfrq() / 1000 * n {
            core::hint::spin_loop();
        }
    };
    for &wdt in &WATCHDOGS {
        unsafe {
            write_volatile(wdt as *mut u32, 0);
            ms(1);
            write_volatile((wdt + 0x04) as *mut u32, 1);
            ms(1);
            write_volatile((wdt + 0x08) as *mut u32, 1);
            ms(1);
            write_volatile(wdt as *mut u32, (1 << 5) | (1 << 0));
            ms(1);
        }
    }
    let t0 = cntpct();
    while cntpct().wrapping_sub(t0) < cntfrq() * 64 {
        core::hint::spin_loop();
    }
    // Still here after 64 s: report it, and let the watchdogs be petted again.
    RESET_ORDERED.store(false, core::sync::atomic::Ordering::Relaxed);
    crumb(0xB7);
    WATCHDOG_DID_NOT_FIRE
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

/// HSI0 S2MPU in front of the USB DMA master.
const S2MPU_HSI0: usize = 0x1107_0000;
/// USB SYSMMU (samsung sysmmu v9). Per-VM control at 0x8000 + vm * 0x1000, bit 0 = translation enable; VM count in NUM_CONTEXT (0x100) bits 4:0.
const USB_SYSMMU: usize = 0x1104_0000;
const SYSMMU_CTRL_VM: usize = 0x8000;
const SYSMMU_VM_STRIDE: usize = 0x1000;
const SYSMMU_NUM_CONTEXT: usize = 0x100;

pub fn cntpct() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) v) };
    v
}

pub fn cntfrq() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) v) };
    // Unprogrammed CNTFRQ would make every timeout zero; fall back to the Tensor G3 arch timer rate.
    if v == 0 { 24_576_000 } else { v }
}

/// Append "LABEL=<8 hex digits>\n".
pub fn append_hex(log: &mut alloc::vec::Vec<u8>, label: &[u8], val: u32) {
    log.extend_from_slice(label);
    let hex = b"0123456789ABCDEF";
    for i in (0..8).rev() {
        log.push(hex[((val >> (i * 4)) & 0xF) as usize]);
    }
    log.push(b'\n');
}

/// Frame a response as a PT outbound transfer: SPEC + DATA packets each padded to exactly 512 so the idle pump's 512-byte chunks align with packet boundaries (blast mode, no FIN — see the cold-boot loop).
pub fn queue_pt_response(out: &mut alloc::vec::Vec<u8>, payload: &[u8]) {
    out.clear();
    let mut ob_bitmap = alloc::vec![0u64; ferros_pt::transfer::outbound_bitmap_words(payload.len())];
    let mut spec_buf = [0u8; 512];
    if let Some((mut ob, spec_len)) = OutboundTransfer::start(ferros_pt::StreamId::FIRST, payload, &mut ob_bitmap, &mut spec_buf) {
        out.extend_from_slice(&spec_buf[..spec_len]);
        out.resize(512, 0);
        let mut pkt = [0u8; 512];
        while !ob.all_sent() {
            let plen = ob.next_data_packet(payload, &mut pkt);
            if plen == 0 {
                break;
            }
            let start = out.len();
            out.extend_from_slice(&pkt[..plen]);
            out.resize(start + 512, 0);
        }
    }
}

/// SYSMMU + S2MPU state as Linux left it, and what the disable read back. DIAG reports it.
pub struct DmaPath {
    pub s2mpu_ctrl: u32,
    pub num_vm: usize,
    pub vm_ctrl_before: [u32; 8],
    pub vm_ctrl_after: [u32; 8],
}

/// Put the DWC3 DMA master on physical addresses: bypass the HSI0 S2MPU, disable translation in every SYSMMU VM context.
pub fn open_usb_dma_path() -> DmaPath {
    let rd = |a: usize| unsafe { read_volatile(a as *const u32) };
    let wr = |a: usize, v: u32| unsafe { write_volatile(a as *mut u32, v) };
    let s2mpu_ctrl = rd(S2MPU_HSI0);
    wr(S2MPU_HSI0 + 0x54, 0xFF);
    wr(S2MPU_HSI0, 0);
    let num_vm = ((rd(USB_SYSMMU + SYSMMU_NUM_CONTEXT) & 0x1F) as usize).clamp(1, 8);
    let mut d = DmaPath { s2mpu_ctrl, num_vm, vm_ctrl_before: [0; 8], vm_ctrl_after: [0; 8] };
    for vm in 0..num_vm {
        let reg = USB_SYSMMU + SYSMMU_CTRL_VM + vm * SYSMMU_VM_STRIDE;
        d.vm_ctrl_before[vm] = rd(reg);
        wr(reg, d.vm_ctrl_before[vm] & !1);
        d.vm_ctrl_after[vm] = rd(reg);
    }
    unsafe { core::arch::asm!("dsb sy") };
    d
}

/// Instruction cache on (SCTLR_EL2.I). Both boot paths enter with the MMU and caches off; with I=0 every instruction fetch goes to DRAM (a 258 KB RELOAD took ~55 s, ~3 s with I=1). With the MMU off, I=1 makes fetches Normal inner write-through cacheable and touches nothing on the data side, so DMA coherency is unaffected. Code is only ever written by the RELOAD stub and RUN staging, and both invalidate the I-cache before executing it.
pub fn enable_icache() {
    unsafe {
        core::arch::asm!(
            "ic iallu",
            "dsb sy",
            "mrs {t}, sctlr_el2",
            "orr {t}, {t}, #(1 << 12)",
            "msr sctlr_el2, {t}",
            "isb",
            t = out(reg) _,
        );
    }
}

/// The previous run's report block (32 words at RAMOOPS_RESULT), if it carries the result magic. Read it before `arm_safety`, which zeroes the report slots. Ramoops DRAM survives warm resets (PSCI, watchdog), so a cold boot can show why the last session ended.
pub fn read_report() -> Option<[u64; 32]> {
    let mut r = [0u64; 32];
    for (i, w) in r.iter_mut().enumerate() {
        *w = unsafe { read_volatile((RAMOOPS_RESULT + i * 8) as *const u64) };
    }
    if r[0] == RESULT_MAGIC { Some(r) } else { None }
}

/// Minimal report for a run without a vault probe (the cold boot): magic, the exit crumb, the sync exception count.
pub fn report_exit(exit: u8) {
    LAST_CRUMB.store(exit as u64, core::sync::atomic::Ordering::Relaxed);
    unsafe {
        write_volatile(RAMOOPS_RESULT as *mut u64, RESULT_MAGIC);
        write_volatile((RAMOOPS_RESULT + 0x18) as *mut u64, exit as u64);
        write_volatile((RAMOOPS_RESULT + 0x28) as *mut u64, crate::exception_count());
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 0x40);
    }
}


/// A canary across the kernel's .bss: 64 KiB of a known pattern, written once at boot and checked by DIAG (CANARY_BAD, CANARY_AT, CANARY_VAL). A reloaded generation read garbage from its statics (UFS_LOST came back as an address, 2026-10-23); the first word to change, and what it changed to, names the writer.
static mut CANARY: [u64; 8192] = [0; 8192];
pub fn canary_init() {
    unsafe {
        for (i, w) in (*(&raw mut CANARY)).iter_mut().enumerate() {
            core::ptr::write_volatile(w, 0xC0FF_EE00_0000_0000 | i as u64);
        }
    }
}
/// (words changed, address of the first, its value).
pub fn canary_check() -> (u32, usize, u64) {
    let mut bad = 0u32;
    let mut first = (0usize, 0u64);
    unsafe {
        for (i, w) in (*(&raw const CANARY)).iter().enumerate() {
            let v = core::ptr::read_volatile(w);
            if v != 0xC0FF_EE00_0000_0000 | i as u64 {
                if bad == 0 {
                    first = (w as *const u64 as usize, v);
                }
                bad += 1;
            }
        }
    }
    (bad, first.0, first.1)
}
pub fn canary_base() -> usize {
    unsafe { &raw const CANARY as usize }
}

unsafe extern "C" {
    static __stack_bottom: u8;
    static __stack_top: u8;
}
const STACK_PAINT: u64 = 0x5AC4_5AC4_5AC4_5AC4;
/// Paint the kernel stack below the current frame, so `stack_used_max` can read how deep it has ever gone.
pub fn stack_paint() {
    unsafe {
        let sp: u64;
        core::arch::asm!("mov {}, sp", out(reg) sp);
        let bottom = &raw const __stack_bottom as usize;
        let mut a = bottom;
        while a + 8 <= (sp as usize).saturating_sub(512) {
            core::ptr::write_volatile(a as *mut u64, STACK_PAINT);
            a += 8;
        }
    }
}
/// (stack size, deepest use so far in bytes): the first word from the bottom that is no longer paint.
pub fn stack_used_max() -> (u32, u32) {
    unsafe {
        let bottom = &raw const __stack_bottom as usize;
        let top = &raw const __stack_top as usize;
        let mut a = bottom;
        while a + 8 <= top && core::ptr::read_volatile(a as *const u64) == STACK_PAINT {
            a += 8;
        }
        ((top - bottom) as u32, (top - a) as u32)
    }
}
