//! The kernel, as this process reaches it (KERNEL.md, the process ABI): the grants page, an ira key, and the preferences block every process reads (the person's base, language, theme and zone, ferros_sys `pref`).
//! Inlined as Lumis's, basecalc's and Photon's are: ferros_sys is no_std and not a published crate.

use core::arch::asm;

const IRA: u64 = 23;

#[inline(always)]
fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    r
}

/// The arc base (x28, the kernel's, never written by compiled code on this target).
#[inline(always)]
fn arc_base() -> u64 {
    let b: u64;
    unsafe { asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
    b
}

#[inline(always)]
fn off<T: ?Sized>(p: *const T) -> u64 {
    (p as *const u8 as u64).wrapping_sub(arc_base())
}

/// The grants page is arc offset 0: the handle table in its first 512 bytes, the u32 count at G#200, the 32-byte tokens after it (SANDBOX.md, "Handles").
pub fn grant_count() -> u64 {
    unsafe { ((arc_base() + 0x200) as *const u32).read_volatile() as u64 }
}

/// A grant's token, as an offset in the arc (ferros_items `kist::Sys`'s `cap`).
pub fn cap(slot: u64) -> u64 {
    0x204 + 32 * slot
}

/// Whether `slot` holds a grant: a slot past the count, or an empty one (a right the person said no to: its token is zeros, process.rs `Object::Empty`), holds none.
pub fn held(slot: u64) -> bool {
    if slot >= grant_count() {
        return false;
    }
    let t = unsafe { core::slice::from_raw_parts((arc_base() + cap(slot)) as *const u8, 32) };
    t.iter().any(|&b| b != 0)
}

/// A 32-byte key for `domain` rooted in the device's ira (ferros_sys `ira_key`), bound to this kist's gate identity. None without the ira this boot.
pub fn ira_key(slot: u64, domain: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let r = svc(IRA, cap(slot), off(domain.as_ptr()), domain.len() as u64, off(out.as_mut_ptr()), 0);
    (r == 32).then_some(out)
}

/// The preferences block (ferros_sys `PREFS`, `PREFS_HI`): preference k at byte k of the first, 32 + k at byte k of the second.
const PREFS: u64 = 0xFC0;
const PREFS_HI: u64 = 0xFB0;

fn pref(at: u64) -> u8 {
    unsafe { core::ptr::read_volatile((arc_base() + at) as *const u8) }
}

/// The device's number base right now (preference 0, tatau's code).
pub fn base() -> tatau::Base {
    tatau::Base::from_code(pref(PREFS)).unwrap_or(tatau::Base::Dms)
}

/// The deck's theme (preference 5).
pub fn theme() -> usize {
    pref(PREFS + 5) as usize
}

/// The language in effect (preferences 32-33, an ISO 639-3 code packed base 26), as the shell draws it: English while there is none, or one the face cannot draw (ferros_lang `current`).
pub fn lang() -> ferros_lang::Lang {
    let p = u16::from_le_bytes([pref(PREFS_HI), pref(PREFS_HI + 1)]);
    // WHY the bound: the kernel keeps only codes below 26^3 (ferros_sys lang::unpack); a zeroed block is none.
    let code = (p != 0 && p < 26 * 26 * 26).then(|| [b'a' + (p / 676) as u8, b'a' + (p / 26 % 26) as u8, b'a' + (p % 26) as u8]);
    match code.and_then(ferros_lang::Lang::from_iso) {
        Some(l) if ferros_lang::renders(l) => l,
        _ => ferros_lang::Lang::En,
    }
}

/// The zone the base shows (ferros_sys `zone`): arabic the legal offset, dozenal and hex the standard one, seconds east of UTC (preferences bytes 24-31).
pub fn zone() -> i64 {
    let v = unsafe { core::ptr::read_volatile((arc_base() + PREFS + 24) as *const u64) };
    let (standard, legal) = (v as u32 as i32 as i64, (v >> 32) as u32 as i32 as i64);
    if pref(PREFS) == 10 {
        legal
    } else {
        standard
    }
}
