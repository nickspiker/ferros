//! The panel's power (DISPLAY-DESIGN.md §8 layer 6, state B; DISPLAY-HK3.md §6): what only the kernel may do for the google-hk3 panel, its rails over ACPM and its reset line, behind two switches the display server holds (`panel`, `panel-vddd`). The DCS side (the PPS, the init set, display on) is the display server's.
//!
//! The rails, on the S2MPG14 (PMIC.md): VCI = LDO14M (`L14M_CTRL` G#38), VDDD = LDO22M (`L22M_CTRL` G#40), enable bit 7 each; only the enable bit is touched, the voltage stays what ABL set (DISPLAY-HK3.md: the operating voltages are UNCONFIRMED, so ferros never writes one). VDDI (S2MPG15 BUCKA) is always on. The reset line is gpa6-0, active high, in the always-on bank, which also holds the volume-up key the killswitch listens on (gpa6-2): every write changes pin 0's bits alone.
//!
//! The stock order (panel-samsung-drv.c, DISPLAY-HK3.md §6): on, VCI + 10 ms, reset high 1 ms, low 1 ms, high 5 ms, then the display server's PPS and init set, then VDDD + 1 ms; off, VDDD off + 1 ms, reset low, VCI off + 1 ms.

use crate::acpm;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

const L14M_CTRL: u32 = 0x38;
const L22M_CTRL: u32 = 0x40;
const EN: u8 = 0x80;
const GPA6_CON: usize = 0x154D_00A0;
const GPA6_DAT: usize = 0x154D_00A4;
const RESET_PIN: u32 = 0;

/// The panel's VCI as the kernel last read or set it, for SYS_FACTS 81: 2 on, 1 off, 0 not read yet.
pub static VCI: AtomicU64 = AtomicU64::new(0);

/// The rail's enable bit, read through ACPM.
fn on(reg: u32) -> Option<bool> {
    acpm::read(acpm::MAIN, acpm::T_PMIC, reg).ok().map(|v| v & EN != 0)
}

/// Read VCI once (the service loop, at start): what ABL or Android left.
pub fn read_state() {
    VCI.store(match on(L14M_CTRL) { Some(true) => 2, Some(false) => 1, None => 0 }, Relaxed);
}

fn spin_ms(ms: u64) {
    let (f, t0) = (crate::platform::cntfrq(), crate::platform::cntpct());
    while crate::platform::cntpct().wrapping_sub(t0) < f / 1000 * ms {}
}

/// gpa6-0 an output at `high`, its neighbours untouched.
fn reset(high: bool) {
    unsafe {
        let d = read_volatile(GPA6_DAT as *const u32);
        write_volatile(GPA6_DAT as *mut u32, if high { d | 1 << RESET_PIN } else { d & !(1 << RESET_PIN) });
        let c = read_volatile(GPA6_CON as *const u32);
        write_volatile(GPA6_CON as *mut u32, (c & !(0xF << (RESET_PIN * 4))) | 1 << (RESET_PIN * 4));
    }
}

/// The `panel` switch. On: VCI, 10 ms, the reset pulse; the panel then waits for its PPS and init set. Off: VDDD off, 1 ms, reset low, VCI off, 1 ms. False when a rail did not answer.
pub fn power(up: bool) -> bool {
    if up {
        if acpm::update(acpm::MAIN, acpm::T_PMIC, L14M_CTRL, EN, EN).is_err() || on(L14M_CTRL) != Some(true) {
            return false;
        }
        VCI.store(2, Relaxed);
        spin_ms(10);
        reset(true);
        spin_ms(1);
        reset(false);
        spin_ms(1);
        reset(true);
        spin_ms(5);
        return true;
    }
    let vddd = acpm::update(acpm::MAIN, acpm::T_PMIC, L22M_CTRL, 0, EN).is_ok();
    spin_ms(1);
    reset(false);
    let vci = acpm::update(acpm::MAIN, acpm::T_PMIC, L14M_CTRL, 0, EN).is_ok();
    spin_ms(1);
    VCI.store(if on(L14M_CTRL) == Some(false) { 1 } else { 0 }, Relaxed);
    vddd && vci
}

/// The `panel-vddd` switch: VDDD after the init set (on, + 1 ms), or off alone.
pub fn vddd(up: bool) -> bool {
    let w = acpm::update(acpm::MAIN, acpm::T_PMIC, L22M_CTRL, if up { EN } else { 0 }, EN);
    let now = acpm::read(acpm::MAIN, acpm::T_PMIC, L22M_CTRL);
    let ok = w.is_ok() && now.as_ref().is_ok_and(|v| (v & EN != 0) == up);
    // A refusal says why (2026-11-04: refused once just after a jump, LDO22M left at G#22, then accepted by hand a minute later): the write's answer and the register as read back.
    if !ok {
        crate::flashlog::line(&alloc::format!("panel-vddd {}: refused; update {:?}, now {:?}", if up { "on" } else { "off" }, w.map_err(|e| alloc::format!("G#{e:X}")), now.map(|v| alloc::format!("G#{v:02X}")).map_err(|e| alloc::format!("G#{e:X}"))));
    }
    spin_ms(1);
    ok
}
