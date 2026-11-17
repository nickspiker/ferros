//! The two PCIe links' guard (WIFI.md "A BAR access that never completes"): a register access to a link's memory or config window that never comes back is ended by taking that link down, and a reload or a jump crowbars both links before any driver runs.
//!
//! Measured on the Pro (2026-11-16, b908, a RAM reload into a cold session with WiFi and cellular on and Bluetooth switched off): pcie1's read of BAR0 + G#40 (the sysmem's bank info, stage 6) never completed with the link in L0 (PCS G#188 read 0), and the modem's config read on the other link, issued a few milliseconds later, never completed either.
//! The controller's completion timeout (DEVCTL2 range B, 28 to 44 ms in this controller) ended neither in 130 s.
//! PERST taken low on WiFi's link (one GPIO write from the host) ended both at once: pcie1's read came back and it went on, and the modem's read came back with the right answer and its bring-up finished.
//! So the WiFi endpoint, not the link, held the read (the chip wedged: see bt.rs, the pins Bluetooth's off never drove), and the other link's access waited behind it somewhere both share.
//! Only a link going down ends such an access; the oldest stuck one is the one released, since the others may only be waiting behind it.

use core::sync::atomic::{AtomicU64, Ordering::{AcqRel, Acquire, Relaxed, Release}};

/// Per core: when its access to a link's window began (the counter with bit 0 set, so 0 is none).
static AT: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
/// Per core: that access's physical address with its link in bit 63 (0 the modem's ch0, 1 WiFi's ch1).
static WHAT: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
/// Per core: the start (`AT`) of the access last released, so one that a link-down did not end is not released again every pass.
static RELEASED: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];

/// The link whose memory or config window holds `addr`: ch0 (the modem) G#4000_0000 to G#4100_0000 (pcie0-mem, then pcie0-cfg at G#40FF_E000), ch1 (WiFi) G#6000_0000 to G#6100_0000 (pcie1-mem, pcie1-cfg).
pub fn link_of(addr: usize) -> Option<u64> {
    match addr >> 24 {
        0x40 => Some(0),
        0x60 => Some(1),
        _ => None,
    }
}

/// Core `c` begins a register access at `addr`: noted when it is in a link's window. Returns whether it was, for `end`.
#[inline]
pub fn begin(c: usize, addr: usize) -> bool {
    let Some(k) = link_of(addr) else { return false };
    WHAT[c].store(addr as u64 | k << 63, Relaxed);
    AT[c].store(crate::platform::cntpct() | 1, Release);
    true
}

/// Core `c`'s access noted by `begin` came back.
#[inline]
pub fn end(c: usize) {
    AT[c].store(0, Release);
}

/// PERST of link `k` driven low (gph0-0 in gpio-hsi1 for ch0, gph3-0 in gpio-hsi2 for ch1, as payloads/modem and payloads/pcie1 drive it): the endpoint held in reset, its link down, and every access the controller still holds for it ended.
/// The data bit first, then the pin's mode as an output, so the line never goes high on the way (a pin left an input is not driven, bt.rs has what that cost).
/// One read-modify-write of each word: the drivers that write these words are either stuck in the access this ends or not running (the GPS's nSTANDBY shares gph3's word; its driver runs only after attest).
fn perst_low(k: u64) {
    let base: usize = if k == 0 { 0x1204_0000 } else { 0x1304_0000 };
    unsafe {
        let d = core::ptr::read_volatile((base + 4) as *const u32);
        core::ptr::write_volatile((base + 4) as *mut u32, d & !1);
        let c = core::ptr::read_volatile(base as *const u32);
        core::ptr::write_volatile(base as *mut u32, (c & !0xF) | 1);
    }
}

/// The look, from the service loop's every pass and the workers' idle loop (beside the stall catcher), so a stuck core is seen whichever cores still run: the oldest access to a link's window older than 2^-3 s is ended by taking its link down, then said in the flash log.
/// WHY 2^-3 s: every access the endpoint received ends by its completion or by the completion timeout (at most 44 ms here); one past three times that never will (measured, above), and a link-down is all that ends it.
/// One release a look, the oldest first: an access on the other link stuck behind it comes back with it.
/// The release is claimed by a compare-and-swap on `RELEASED`, so two cores looking at once release it once; the link goes down before the line is written, since the flash log takes the kernel lock, which a stuck access on the locked path holds.
pub fn release_stuck() {
    let late = crate::platform::cntfrq() >> 3;
    let mut oldest: Option<(usize, u64, u64)> = None;
    for (c, at) in AT.iter().enumerate() {
        let t = at.load(Acquire);
        if t == 0 || RELEASED[c].load(Relaxed) == t {
            continue;
        }
        // The counter read after the start is loaded, and compared as `now > t + late`, never `now - t`: an access that began after an earlier read of the counter has a start newer than it, and the difference wrapped to a huge age released three healthy accesses of a firmware download (b909, 2026-11-16).
        let now = crate::platform::cntpct();
        if now > t + late && oldest.is_none_or(|(_, o, _)| t < o) {
            oldest = Some((c, t, now));
        }
    }
    let Some((c, t, now)) = oldest else { return };
    let seen = RELEASED[c].load(Relaxed);
    if seen == t || RELEASED[c].compare_exchange(seen, t, AcqRel, Relaxed).is_err() {
        return;
    }
    let w = WHAT[c].load(Relaxed);
    // Still the same access (its start unchanged after the address was read): one that came back since the look is left alone.
    if AT[c].load(Acquire) != t {
        return;
    }
    let k = w >> 63;
    perst_low(k);
    crate::flashlog::line(&alloc::format!(
        "pcie guard: core {c} in an access to G#{:X} for A#{} ms with no answer: PERST of {} taken low to end it (the driver sees the error and ends; its restart takes the chip down first)",
        w & !(1 << 63),
        // `now` is the read that found this start late (now > t + late), so the difference cannot go below zero.
        (now - t) * 1000 / crate::platform::cntfrq(),
        if k == 0 { "the modem's link (ch0)" } else { "WiFi's link (ch1)" }
    ));
}

/// A reload or a jump, before init starts any driver: Bluetooth's core switched off with its pins driven (bt.rs `power`), and both links' endpoints held in reset (PERST low), so nothing the previous session (ferros's own or Android's) left running on them masters the bus or answers half reset until its driver's own `off` takes the chip down.
/// GPIO words only: never the controllers' ELBI or DBI, which stall when a PHY is powered down with its isolation released (WIFI.md section 13 item 16), the state a suspended link leaves.
/// The modem's link only when HSI1 is powered (its PMU STATUS, read plainly as payloads/modem's survey reads it); HSI2 carries UFS, which the kernel has read from by now.
pub fn crowbar(why: &str) {
    let rd = |a: usize| unsafe { core::ptr::read_volatile(a as *const u32) };
    let (bt_con, bt_dat) = (rd(0x1506_0100), rd(0x1506_0104));
    crate::bt::power(false);
    let hsi1 = rd(0x1546_2B04) & 1 == 1;
    let (ch0, ch1) = (if hsi1 { rd(0x1204_0004) } else { 0 }, rd(0x1304_0004));
    if hsi1 {
        perst_low(0);
    }
    perst_low(1);
    crate::flashlog::line(&alloc::format!(
        "radios crowbarred ({why}): Bluetooth's REG_ON found con G#{bt_con:X} dat G#{bt_dat:X}, now driven low; PERST low on {} (gph0 dat was G#{ch0:X}) and WiFi's link (gph3 dat was G#{ch1:X})",
        if hsi1 { "the modem's link" } else { "not the modem's link (HSI1 off)" }
    ));
}
