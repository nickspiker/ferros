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

/// Cluster watchdogs (s3c2410 block: WTCON +0, WTDAT +4, WTCNT +8; RSTEN b0, EN b5, DIV128 3<<3, PRESCALE 0xFF<<8). Armed at WDT_COUNT (1 s; the longest window, ~87 s, until 2026-11-05); a pure spin with no pets resets warm instead of hanging forever.
const WATCHDOGS: [usize; 2] = [0x1006_0000, 0x1007_0000];
/// The watchdog window (owner, 2026-11-05: "1s?"): 750 ticks of prescaler 256 and divider 128 from the 24.576 MHz oscillator, 1.333 ms a tick, is 1 s; written at the arm and reloaded by every pet. Measured before choosing it: the longest stretch without a pet is 213 ms on a cold boot, 80 ms across a reload's hand-over, 316 ms in a reloaded kernel's start (DIAG WDT_GAP_MAX_US, WDT_HANDOVER_US); the hv's re-arm at the jump keeps the longest window until the jump's stretch is measured. `diag wdt rate` checks the tick against the counter.
pub const WDT_COUNT: u32 = 750;

/// Make a hang self-reporting and self-recovering (no button reset, which cold-resets and wipes pmsg).
/// 1. Fault sink: the sync exception handler records the first fault (ELR/FAR/ESR/count) into the result block and warm-resets, so the next Linux boot prints it. The pmsg zone can come up filled with FF (seen on hardware), which would defeat the sink's "already recorded" guard on 0xA0 and pollute every unwritten slot, so the report slots are zeroed first.
/// 2. Cluster watchdogs armed.
/// What the result block held when this boot armed the sink: the last boot's end, if it ended in a fault or a panic — panic number, heap word, ESR, fault count, ELR (or the panic text's first 8 bytes), FAR (or the next 8). The service writes it to the flash log once that is up (`previous_end`), since the block is cleared here and a reboot is otherwise silent.
pub static PREV_END: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static mut PREV_WORDS: [u64; 6] = [0; 6];

/// The last boot's end words (see `PREV_END`), zero when it ended cleanly or nothing was recorded.
pub fn previous_end() -> [u64; 6] {
    unsafe { *(&raw const PREV_WORDS) }
}

pub fn arm_safety() {
    unsafe {
        let r = |off: usize| read_volatile((RAMOOPS_RESULT + off) as *const u64);
        *(&raw mut PREV_WORDS) = [r(0x08), r(0x10), r(0x20), r(0x28), r(0xA0), r(0xA8)];
        PREV_END.store(r(0x20) | r(0x28) | r(0x08), core::sync::atomic::Ordering::Relaxed);
        for off in [0x08usize, 0x10, 0x20, 0x28, 0xA0, 0xA8] {
            write_volatile((RAMOOPS_RESULT + off) as *mut u64, 0);
        }
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 0x180);
        write_volatile(&raw mut crate::__exc_sink, RAMOOPS_RESULT as u64);
    }
    const WDT_ARM: u32 = 0xFF00 | (3 << 3) | (1 << 5) | (1 << 0);
    // The arm is the first pet the window measures from; a hand-over stamp left by the hv's jump or a reload says how long the stretch before it ran unpetted.
    let now = cntpct();
    unsafe {
        let kind = read_volatile((HANDOVER_STAMP + 8) as *const u64);
        if kind == b'J' as u64 || kind == b'R' as u64 {
            let stamp = read_volatile(HANDOVER_STAMP as *const u64);
            HANDOVER_GAP_US.store(now.wrapping_sub(stamp) * 1_000_000 / cntfrq(), core::sync::atomic::Ordering::Relaxed);
            HANDOVER_KIND.store(kind, core::sync::atomic::Ordering::Relaxed);
            write_volatile((HANDOVER_STAMP + 8) as *mut u64, 0);
            core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) HANDOVER_STAMP as u64);
        }
    }
    LAST_PET.store(now, core::sync::atomic::Ordering::Relaxed);
    for &wdt in &WATCHDOGS {
        unsafe {
            write_volatile((wdt + 0x04) as *mut u32, WDT_COUNT);
            write_volatile((wdt + 0x08) as *mut u32, WDT_COUNT);
            write_volatile(wdt as *mut u32, WDT_ARM);
        }
    }
}

/// Reload both cluster watchdog counters. Loops that legitimately run long (the USB service loop) call this every iteration; a real hang stops the pets and the watchdog resets warm.
/// Set once a watchdog reset has been ordered (`reboot_to_bootloader`): from then on nothing pets the watchdogs. The vault pets them on every operation (kvault.rs), and the map writes the vault from the worker cores all the time, so the reset that `bridge reboot fastboot` waits for never came (2026-10-24: 64 s of a frozen service loop, USB gone, ferros still running after).
static RESET_ORDERED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Stop every pet from here on, as an ordered reset does, but with no reboot mode stamped and the watchdogs left as they run: the next expiry resets warm and ABL boots the active slot again (board.rs `probe_starve`: an akita bring-up build that must not hang where no host can see it).
pub fn starve_watchdogs() {
    RESET_ORDERED.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// The watchdog window's measurement (owner, 2026-11-05: "TIME IT!"): the counter at the last pet from any core, the longest gap between two pets and the counter when it ended, and how many pets; DIAG reports them, so the window can be set from the longest stretch the kernel really goes without petting.
static LAST_PET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static PET_GAP_MAX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static PET_GAP_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static PETS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Where a hand-over leaves its last pet for the next kernel, which no running code can pet through: the hv's re-arm at the jump ('J') and a reload's hand-off to the copy stub ('R'). The counter at G#2C0, the kind at G#2C8; the next kernel's arm_safety turns them into WDT_HANDOVER_US and clears the kind.
pub const HANDOVER_STAMP: usize = RAMOOPS_RESULT + 0x2C0;
/// The last hand-over's gap in microseconds and its kind (0 none), from arm_safety.
pub static HANDOVER_GAP_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static HANDOVER_KIND: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The last pet before a hand-over, stamped for the next kernel to measure from.
pub fn handover_stamp(kind: u8) {
    pet_watchdogs();
    unsafe {
        write_volatile(HANDOVER_STAMP as *mut u64, cntpct());
        write_volatile((HANDOVER_STAMP + 8) as *mut u64, kind as u64);
        core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) HANDOVER_STAMP as u64);
    }
}

/// Pets held off for `diag wdt rate`'s measurement (a development tool): the counter must run untouched for it.
pub static PET_HOLD: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The watchdog's tick measured (`diag wdt rate`, development builds): every pet held off for 200 ms, the first watchdog's counter read before and after; returns (ticks counted, microseconds), so the window in seconds is WDT_COUNT · us / ticks / 10^6. 200 ms is 150 ticks at the assumed 1.333 ms, well inside the 1 s window.
pub fn wdt_rate() -> (u32, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    pet_watchdogs();
    PET_HOLD.store(true, Relaxed);
    let (t0, c0) = (cntpct(), unsafe { read_volatile((WATCHDOGS[0] + 0x08) as *const u32) });
    while cntpct().wrapping_sub(t0) < cntfrq() / 5 {
        core::hint::spin_loop();
    }
    let (t1, c1) = (cntpct(), unsafe { read_volatile((WATCHDOGS[0] + 0x08) as *const u32) });
    PET_HOLD.store(false, Relaxed);
    pet_watchdogs();
    (c0.wrapping_sub(c1), t1.wrapping_sub(t0) * 1_000_000 / cntfrq())
}

pub fn pet_watchdogs() {
    if RESET_ORDERED.load(core::sync::atomic::Ordering::Relaxed) || PET_HOLD.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    use core::sync::atomic::Ordering::Relaxed;
    let now = cntpct();
    let prev = LAST_PET.swap(now, Relaxed);
    // Before the arm stamps it (the shim's paths) there is no gap to measure.
    if prev != 0 {
        let gap = now.wrapping_sub(prev);
        if gap > PET_GAP_MAX.fetch_max(gap, Relaxed) {
            PET_GAP_AT.store(now, Relaxed);
        }
    }
    PETS.fetch_add(1, Relaxed);
    for &wdt in &WATCHDOGS {
        unsafe { write_volatile((wdt + 0x08) as *mut u32, WDT_COUNT) };
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
/// ferros's console, mirrored into Android's pstore console ring (ramoops at G#FD3F_F000, 2 MiB: u32 "DBGC", u32 start, u32 size, then the ring), the RAM log the hv writes and that survives a warm reset. A session that dies before anyone reads `diag console` (the jumps from Android that ended in a watchdog reset before USB, 2026-11-01) leaves its whole log there for the next boot (`tools/husky-pstore.sh`). Same layout and wrap as the hv's `pstore::log`; nothing written while the ring has no signature.
const PSTORE_ZONE: usize = 0xFD3F_F000;
const PSTORE_BUF: u32 = 0x20_0000 - 12;
pub fn pstore(bytes: &[u8]) {
    unsafe {
        if read_volatile(PSTORE_ZONE as *const u32) != 0x4347_4244 {
            return;
        }
        let mut start = read_volatile((PSTORE_ZONE + 4) as *const u32);
        let mut size = read_volatile((PSTORE_ZONE + 8) as *const u32);
        // The header is written by others (the bootloader, Linux, the hv): a start or size past the ring would put these writes outside the 2 MiB zone, over whatever lies beyond it. Such a header is not ours to repair: nothing is written.
        if start >= PSTORE_BUF || size > PSTORE_BUF {
            return;
        }
        let first = start;
        for &b in bytes {
            write_volatile((PSTORE_ZONE + 12 + start as usize) as *mut u8, b);
            start = if start + 1 >= PSTORE_BUF { 0 } else { start + 1 };
            if size < PSTORE_BUF {
                size += 1;
            }
        }
        write_volatile((PSTORE_ZONE + 4) as *mut u32, start);
        write_volatile((PSTORE_ZONE + 8) as *mut u32, size);
        // To DRAM now: the ring is cacheable under ferros's map, and a session that dies by watchdog takes its dirty lines with it (the jumps of 2026-11-02 lost their last lines, and the 60 s escape's thread dump, while the crumbs, cleaned, survived). The written span (wrapped: both parts) and the header.
        let clean = |a: usize, n: usize| {
            let mut p = a & !63;
            while p < a + n {
                core::arch::asm!("dc civac, {p}", p = in(reg) p as u64);
                p += 64;
            }
        };
        if first as usize + bytes.len() <= PSTORE_BUF as usize {
            clean(PSTORE_ZONE + 12 + first as usize, bytes.len());
        } else {
            clean(PSTORE_ZONE + 12 + first as usize, PSTORE_BUF as usize - first as usize);
            clean(PSTORE_ZONE + 12, bytes.len() - (PSTORE_BUF as usize - first as usize));
        }
        clean(PSTORE_ZONE, 12);
        core::arch::asm!("dsb sy");
    }
}

/// Each core's last register access a process asked for (SYS_MMIO_READ/WRITE), at +G#180 + core*8: the physical address, bit 63 set for a write; the previous boot's at +G#1C8 (`bridge read FD6001C8 48`). A bus stall on a block whose clock or power is off leaves the core inside that access with the kernel lock held and the watchdog to reset it: this names the register (the jump from Android stalled on the display's modeset, 2026-11-01).
pub const MMIO_NOTES: usize = RAMOOPS_RESULT + 0x180;
pub fn note_mmio(v: u64) {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
    let a = MMIO_NOTES + (((mpidr >> 8) & 0xF) as usize % 9) * 8;
    unsafe {
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
        for i in 0..9 {
            let v = read_volatile((MMIO_NOTES + i * 8) as *const u64);
            write_volatile((MMIO_NOTES + 0x48 + i * 8) as *mut u64, v);
            write_volatile((MMIO_NOTES + i * 8) as *mut u64, 0);
            core::arch::asm!("dc civac, {a}", "dc civac, {b}", a = in(reg) (MMIO_NOTES + i * 8) as u64, b = in(reg) (MMIO_NOTES + 0x48 + i * 8) as u64);
        }
        core::arch::asm!("dsb sy");
    }
}

/// Handoff hang localizer (behind FERROS_FP_SCAN): a monotonic step written to r[9] (RAMOOPS_RESULT + 0x48), a slot nothing else writes, cleaned so it survives the watchdog reset. The furthest step printed in the next Android boot's "ferros: r[08..11]" dump (the second word) is the last point the handoff reached; it hung in the step after it. A no-op unless FERROS_FP_SCAN is set, so normal builds are untouched.
pub fn mark(step: u64) {
    if option_env!("FERROS_FP_SCAN").is_none() {
        return;
    }
    unsafe {
        write_volatile((RAMOOPS_RESULT + 0x48) as *mut u64, step);
        ferros_hal::mmio::cache_clean(RAMOOPS_RESULT, 0x80);
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

/// This core's power-mode hint (PMU CPU_INFORM, G#1546_0860 + 4 per core: 0 none, 1 C2, 2 cluster power-down) cleared, as Linux's online does (pmucal_cpu_enable) on each core it starts. A no-op after the bootloader, whose hints read 0; after the jump from Android the hv leaves 1 on each core it parked, and a core started with its hint still set could be powered down by the PMU whenever it idles.
/// The jump's mailbox in the hv's reservation (ferros_hv `hvcore::MAILBOX`): one 32-byte slot per core, [entry, context, state]; state 1 = an Android core parked there, powered, waiting at EL2.
const JUMP_MAILBOX: usize = 0xA_FFFE_0000;
static JUMPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether this boot is the jump from a running Android: the hv's gate mark with answer 6, read before `ready::answered_by_hv` takes it (the X3's launch comes first). Called once at entry; `jumped` reads the result.
pub fn detect_jump() {
    // akita has no hv and no RAM up there: its 8 GB end at G#A_0000_0000 (board.rs).
    if !crate::board::hv_ram() {
        return;
    }
    let (mark, tel) = unsafe { (read_volatile(0xA_FFFF_FF10usize as *const u64), read_volatile(0xA_FFFF_FF18usize as *const u64)) };
    JUMPED.store(mark == 0x4741_5445_464F_5252 && tel & 0xFF == 6, core::sync::atomic::Ordering::Relaxed);
}
pub fn jumped() -> bool {
    JUMPED.load(core::sync::atomic::Ordering::Relaxed)
}

/// Start core `aff1` (MPIDR Aff1; `mpidr` its whole affinity) at `entry` with x0 = `ctx`: PSCI CPU_ON, or after the jump the hv's mailbox. The jump leaves Android's cores parked and powered, because powering them back on through the secure firmware hung on one jump in two (2026-11-01/02); the context is written before the entry, which the parked core polls. A core that is not parked there (Android had it off) gets PSCI CPU_ON.
pub fn start_core(aff1: usize, mpidr: u64, entry: u64, ctx: u64) -> i64 {
    // A core Android had already taken offline is not on the mailbox: PSCI CPU_ON below, Linux's own way back from its own offline.
    let slot = JUMP_MAILBOX + aff1 * 32;
    if jumped() && unsafe { read_volatile((slot + 16) as *const u64) } == 1 {
        unsafe {
            write_volatile((0xFD60_0C80usize + aff1 * 8) as *mut u64, cntpct() | 1);
            write_volatile((slot + 8) as *mut u64, ctx);
            core::arch::asm!("dsb sy");
            write_volatile(slot as *mut u64, entry);
            core::arch::asm!("dsb sy", "sev");
        }
        return 0;
    }
    let ret: i64;
    unsafe { core::arch::asm!("smc #0", inout("x0") 0xC400_0003u64 => ret, in("x1") mpidr, in("x2") entry, in("x3") ctx) };
    ret
}

/// The jump hand-off trace (ferros_hv hvcore::TRACE): this core's arrival in ferros's Rust, at -G#80 from the hv's word.
/// A worker's progress after the jump, a word per core at G#FD60_0B00 + core*8 (cleaned to DRAM): 1 arrived, 2 GIC armed, 3 EL0 on, 4-6 the first loop passes, 8 the first slice taken. The thirty-second jump: three of the four workers arrived and took no slice.
pub fn worker_step(s: u64) {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
    let a = 0xFD60_0B00usize + ((mpidr >> 8) & 0xF) as usize * 8;
    unsafe {
        write_volatile(a as *mut u64, s | (cntpct() & !0xFF));
        core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) a as u64);
    }
}

/// The boot's mark trail in the report page (G#FD60_0240, cleaned to DRAM, cleared at entry): each step shifts a byte in, newest lowest, so a session that dies by watchdog leaves its last eight steps (`bridge read FD600240 8`). The pstore ring lost its position word on the dying jumps, so its lines after "ira combined" were overwritten by the next boot; the crumb page never was. Marks: G#10 ira combined, G#11-15 the probe's tune/ASV/HPM/DVFS reads and the anchor key, G#16 opening the vault, G#17 opened, G#18 into the service.
const MARK_TRAIL: usize = RAMOOPS_RESULT + 0x240;
pub fn mark_trail(m: u8) {
    unsafe {
        let p = MARK_TRAIL as *mut u64;
        // Mark 0 (entry) keeps the previous session's trail at +8 before clearing: a boot after a dead jump reads it there (`bridge read FD600248 8`).
        if m == 0 {
            write_volatile(p.add(1), read_volatile(p));
            core::arch::asm!("dc civac, {a}", a = in(reg) p.add(1) as u64);
        }
        let v = if m == 0 { 0 } else { read_volatile(p) << 8 | m as u64 };
        write_volatile(p, v);
        core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) p as u64);
    }
}

/// The low two bits say how far: 1 into Rust, before `cpu_inform_clear`'s firmware call; 2 past it (`note_arrival_step(2)`). The seventh jump of 2026-11-03: the X3 took its entry with an intact hand-over block and never wrote the arrival, and that call came first.
/// The last session's hand-off words for cores 4-8, into the pstore ring before this boot starts any core over them (no heap yet): per core the arrival word (G#11-G#17: the entry step it reached; a counter: Rust, low bits 1 or 2) and the hv's two trace words. The ninth jump of 2026-11-03 froze at its first TRNG call with the X3 launched, and the cold boot after it overwrote the X3's marks.
pub fn pstore_prev_handoff() {
    let mut line = [0u8; 5 * 60 + 32];
    let mut n = 0;
    let mut put = |b: &[u8], n: &mut usize| {
        line[*n..*n + b.len()].copy_from_slice(b);
        *n += b.len();
    };
    put(b"ferros: last session's hand-off:", &mut n);
    for c in 4..9usize {
        put(&[b' ', b'c', b'0' + c as u8], &mut n);
        for base in [0xFD60_0B80usize, 0xFD60_0C80, 0xFD60_0C00] {
            let v = unsafe { read_volatile((base + c * 8) as *const u64) };
            let mut h = [b' '; 17];
            for k in 0..16 {
                let d = (v >> (60 - 4 * k)) & 0xF;
                h[k + 1] = if d < 10 { b'0' + d as u8 } else { b'A' + d as u8 - 10 };
            }
            put(&h, &mut n);
        }
    }
    put(b"\n", &mut n);
    pstore(&line[..n]);
}

pub fn note_arrival() {
    note_arrival_step(1);
}
pub fn note_arrival_step(s: u64) {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
    let a = 0xFD60_0B80usize + ((mpidr >> 8) & 0xF) as usize * 8;
    unsafe {
        write_volatile(a as *mut u64, (cntpct() & !3) | s);
        core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) a as u64);
    }
}

pub fn cpu_inform_clear() {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
    let _ = priv_reg(0x1546_0860 + ((mpidr >> 8) & 0xF) * 4, true, 0);
}
pub const REBOOT_MODE_BOOTLOADER: u32 = 0xFC;
pub const REBOOT_MODE_RECOVERY: u32 = 0xFF;

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
/// Run before every reset this kernel orders (AUTH.md): the vault drops the unlocked identity's key, its bytes overwritten, so a warm reset hands nothing to whatever boots next — Android at the gate's other answer, or a crash's next boot. Registered by the service once the vault is open.
pub static mut BEFORE_RESET: Option<fn()> = None;
/// ferros's memory zeroed on the way out (HV.md, dual boot; owner, 2026-11-02): a warm reset keeps DRAM, so whatever boots next (Android from the gate) would find ferros's working set where it was. The roots are in registers and die with the reset; everything else that ever held a decrypted thing is in these regions: the kernel heap (both regions: the vault's working set, capsules in flight), the arc pools (every process's memory), the driver DMA pools, and the display's regions (frames, surfaces, sprites, the canvas). The image's own data and the stack this runs on stay: the stack is in use, and the image holds no secret.
/// The workers are stopped first so no process writes behind the scrub. Each line is zeroed with `dc zva` and cleaned to DRAM at once, so the zeros are in memory before the reset discards the caches: about three quarters of a gigabyte, tens of milliseconds. Only with the cache map on (the zeroing instruction needs Normal memory) and the heap up: an early reset has nothing to scrub. The bytes and the microseconds go to the crumb page at G#FD60_0250 and G#FD60_0258.
#[cfg(not(shim_handoff))]
pub fn scrub_memory() {
    let sctlr: u64;
    unsafe { core::arch::asm!("mrs {v}, sctlr_el2", v = out(reg) sctlr) };
    let (hb, he) = crate::heap_bounds();
    if sctlr & 1 == 0 || hb == 0 {
        return;
    }
    reset_step(2);
    let regions: [(usize, usize); 14] = [
        (hb, he),
        crate::cachemap::HEAP2,
        crate::cachemap::POOL,
        crate::cachemap::POOL_BIG,
        crate::cachemap::DMA_POOL,
        crate::cachemap::USB_DMA,
        crate::cachemap::DISPLAY_FB,
        crate::cachemap::DISPLAY_HEAP,
        crate::cachemap::DISPLAY_PHASES,
        crate::cachemap::DISPLAY_SPRITES,
        crate::cachemap::DISPLAY_CANVAS,
        crate::cachemap::SURFACES,
        crate::cachemap::GPU_MEM,
        crate::cachemap::LAYERS,
    ];
    let total: usize = regions.iter().map(|&(lo, hi)| if hi > lo { hi - lo } else { 0 }).sum();
    // Said before the zeroing: the allocator's own lists are in the heap, so nothing may allocate after it (the first cut formatted its line after, and the reset path died short of the EL3 reboot-mode stamp: two reboots to fastboot that landed as plain boots, 2026-11-03).
    pstore(alloc::format!("memory scrub before the reset: {} MiB\n", total >> 20).as_bytes());
    crate::smp::stop_all();
    crate::cachemap::pools_to_kernel();
    reset_step(3);
    let t0 = cntpct();
    for &(lo, hi) in &regions {
        if hi <= lo {
            continue;
        }
        let mut a = lo;
        while a < hi {
            unsafe { core::arch::asm!("dc zva, {a}", "dc cvac, {a}", a = in(reg) a) };
            a += 64;
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    let us = cntpct().wrapping_sub(t0) * 1_000_000 / cntfrq();
    reset_step(4);
    unsafe {
        write_volatile(0xFD60_0250 as *mut u64, total as u64);
        write_volatile(0xFD60_0258 as *mut u64, us);
        // Cleaned to DRAM, as the trail words are: the report page is cacheable under the map, and the reset discards the caches.
        core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) 0xFD60_0250u64);
    }
}

/// The reset path's own steps, in the report page at G#FD60_0F00 (a magic word) and G#FD60_0F08 (the step: 1 the hook done, 2 the scrub's checks passed, 3 the workers stopped, 4 the memory zeroed), for reading after the boot that follows.
pub fn reset_step(s: u64) {
    unsafe {
        write_volatile(0xFD60_0F00 as *mut u64, 0x4245_4652_4553_4554);
        write_volatile(0xFD60_0F08 as *mut u64, s);
        core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) 0xFD60_0F00u64);
    }
}

pub fn before_reset() {
    if let Some(f) = unsafe { BEFORE_RESET } {
        f();
    }
    reset_step(1);
    #[cfg(not(shim_handoff))]
    scrub_memory();
}

pub fn reboot() -> ! {
    before_reset();
    dss_mark_clean_reboot();
    unsafe { core::arch::asm!("smc #0", in("x0") 0x8400_0009u64, options(noreturn)) }
}

pub fn reboot_to_bootloader() -> u64 {
    reboot_to_mode(REBOOT_MODE_BOOTLOADER)
}

/// As `reboot_to_bootloader`, into the given reboot mode (PMU_REBOOT_CMD's word: REBOOT_MODE_BOOTLOADER, REBOOT_MODE_RECOVERY).
pub fn reboot_to_mode(mode: u32) -> u64 {
    before_reset();
    dss_mark_clean_reboot();
    let r = priv_reg(PMU_REBOOT_CMD, true, mode);
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
    before_reset();
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

/// Set by the service loop while it runs a command from the USB process: `queue_pt_response` then keeps the reply raw.
pub static RAW_REPLY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Whether a raw reply was queued at all: the kernel's own USB sends nothing for some commands (a reload or reboot that acts, a reload with nothing staged), and the process must not send an empty reply in their place.
pub static RAW_REPLIED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Frame a response as a PT outbound transfer: SPEC + DATA packets each padded to exactly 512 so the idle pump's 512-byte chunks align with packet boundaries (blast mode, no FIN — see the cold-boot loop).
pub fn queue_pt_response(out: &mut alloc::vec::Vec<u8>, payload: &[u8]) {
    out.clear();
    // A command the USB process handed over (ptlink.rs) is answered raw: the process frames the reply itself.
    if RAW_REPLY.load(core::sync::atomic::Ordering::Relaxed) {
        out.extend_from_slice(payload);
        RAW_REPLIED.store(true, core::sync::atomic::Ordering::Relaxed);
        return;
    }
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
