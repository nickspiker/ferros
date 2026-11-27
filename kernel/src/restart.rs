//! ferros's own restart (owner, 2026-11-10: power + volume up inside ferros, "same as Android. Kill everything and reboot but jump style so we don't get the stupid nag and uncontrollable Google boot chain").
//! The kill: the store locked so no write is mid-flight, every worker off, the heaps, pools and framebuffers zeroed (`platform::before_reset`, as a reset does), this core's key registers cleared, and every bus master behind the always-on S2MPUs cut off from memory (hsi1: the modem's PCIe; hsi2: UFS and the WiFi chip; misc: its DMA engines), as the hv cuts them at the jump.
//! The start: the image the hv kept (ferros_hv hvcore `keep`, on the gate's ferros path and on Android's) copied to the link base by RELOAD's stub and entered with the kept device tree, the gate's mark answering 7, so the boot takes the jump's path for hardware (`platform::jumped`: drivers that cannot yet bring their block up from a running state stand down, the display runs its whole modeset) and the reload's for the cores (it boots on the X3 that ran ferros; the firmware TRNG is its own). No bootloader runs, so no warning screen and nothing of Google's boot chain.
//! Without a kept image (an hv from before the restart, or a boot not through the hv) it reboots thru the bootloader instead.

use core::ptr::{read_volatile, write_volatile};

/// Where ferros is linked: the copy stub lands the image there.
const LINK_BASE: usize = 0x8008_0000;
/// The always-on S2MPUs the hv denies at the jump (ferros_hv DENY_AT_JUMP): hsi1, hsi2, misc.
const DENY: [usize; 3] = [0x1209_0000, 0x131F_0000, 0x101F_0000];
/// The gate's mark (ready.rs HV_GATE_MARK, "GATEFORR") and this answer: a restart from ferros.
const GATE_MARK: u64 = 0x4741_5445_464F_5252;
pub const ANSWER_RESTART: u64 = 7;
/// The bootconfig's room in the hv's reservation (ferros_hv JUMP_BOOTCONFIG_MAX).
const BOOTCONFIG_MAX: usize = 0x1_0000;

/// The hv's record of what it kept: the image's and the bootconfig's lengths, when the record is whole.
fn kept() -> Option<(usize, usize)> {
    let r = crate::board::hv_kept_record();
    let (magic, len, bc) = unsafe { (read_volatile(r as *const u64), read_volatile((r + 8) as *const u64) as usize, read_volatile((r + 16) as *const u64) as usize) };
    // WHY: words another program left in DRAM size a copy over this kernel; a record from an older hv, or torn, must never do that.
    // PROOF: an image past the hv's room would be read past its end into the device tree it kept; a bootconfig past its room, the same into the record.
    if magic != crate::board::HV_KEPT_MAGIC || len == 0 || len > crate::board::HV_KEPT_IMAGE_MAX || bc > BOOTCONFIG_MAX {
        return None;
    }
    Some((len, bc))
}

/// Kill everything and start ferros again from the kept image. `stub`: where the copy stub runs (the running `Boot`'s, clear of any image).
pub fn restart(stub: usize) -> ! {
    let Some((len, bc)) = kept() else {
        crate::platform::pstore(b"ferros: power + volume up: no image kept by the hv (an older hv, or a boot not through it); rebooting thru the bootloader\n");
        crate::platform::reboot();
    };
    crate::platform::pstore(b"ferros: power + volume up: restarting, no bootloader\n");
    crate::platform::crumb(0xE7);
    // The store locked and memory zeroed; the workers go off inside the scrub (and here when the scrub had nothing to do).
    crate::platform::before_reset();
    crate::smp::stop_all();
    crate::keys::wipe_for_restart();
    unsafe {
        // The copy overwrites the vector table: no interrupt may land mid-copy. The new image re-arms the killswitch first thing.
        core::arch::asm!("msr daifset, #2");
    }
    for &s in &DENY {
        crate::s2mpu::deny_all(s);
    }
    // The hv's mailbox emptied: the new kernel finds no core parked there and starts each worker by PSCI CPU_ON (platform::start_core), as the stop above left them.
    for c in 0..9usize {
        for w in 0..4 {
            unsafe { write_volatile((crate::board::hv_mailbox() + c * 32 + w * 8) as *mut u64, 0) };
        }
    }
    unsafe {
        // The stub copied while the caches are on, as RELOAD copies it; the hand-over below writes every dirty line back.
        let s = &raw const crate::service::ferros_reload_stub as usize;
        let s_len = &raw const crate::service::ferros_reload_stub_end as usize - s;
        core::ptr::copy_nonoverlapping(s as *const u8, stub as *mut u8, s_len);
    }
    // The last pet before the copy stub, stamped for the next kernel's arm_safety: the copy and its boot run unpetted.
    crate::platform::handover_stamp(b'X');
    crate::cachemap::off_for_handover();
    unsafe {
        // With the MMU off these stores go straight to DRAM: the bootconfig where the kept device tree's initrd range points (as the jump puts it), then the gate's mark.
        for k in 0..bc {
            write_volatile((crate::board::LOW_BOOTCONFIG + k) as *mut u8, read_volatile((crate::board::hv_kept_bootconfig() + k) as *const u8));
        }
        let g = crate::board::hv_gate_mark();
        write_volatile(g as *mut u64, GATE_MARK);
        write_volatile((g + 8) as *mut u64, ANSWER_RESTART);
        core::arch::asm!(
            "dsb sy",
            "ic iallu",
            "dsb sy",
            "isb",
            "br {stub}",
            stub = in(reg) stub,
            in("x0") crate::board::hv_kept_image(),
            in("x1") LINK_BASE,
            in("x2") (len + 7) & !7,
            in("x3") crate::board::hv_kept_dtb(),
            options(noreturn)
        );
    }
}
