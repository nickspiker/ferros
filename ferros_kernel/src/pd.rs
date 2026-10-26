//! The SoC's power domains that ferros does not use, switched off (POWER.md, 2026-10-25): ABL leaves every block's domain on — the GPU, the 2D engine, the camera's whole ISP chain, the video codecs, AUR, EH and the TPU read on through EL3 — and ferros had never turned one off. Linux turns idle ones off as its runtime PM finds them unused.
//!
//! The off sequence is Linux's (cal-if/zuma/flexpmu_cal_local_zuma.h `<pd>_off`, cal-if/pmucal_local.c `pmucal_local_disable`): the secure world's save (exynos-pd_el3.c `exynos_pd_tz_save`: SMC G#8200_0410, 0 = going down, the domain's `need_smc` address from zuma-pm-domains.dtsi, group 2), the domain's CMU controller option bit 24 cleared (its own clock controller, reachable while the domain is still on), then PMU CONFIGURATION bit 0 cleared. Every PMU access goes through EL3 (`priv_reg`): a direct EL2 read of the PMU makes the next EL3 call hang (the EL3 poison rule), so instead of polling STATUS the sequence waits a fixed millisecond and reads CONFIGURATION back through EL3 (word 0 of each domain block is the one EL3 will read).
//!
//! Measured with the screen off (odpm and the gauge, 2026-10-25): the rails' outputs 1558 → 1185 mW (the PLL rail 127 → 25, INT 85 → 23, the GPU's 66 → 15, the camera's 35 → 14), the battery 0.98 → 0.64 W; with the screen on 1.58 → 1.38 W.
//!
//! On by default: the service loop gates every domain here once, 16 s in (`gate_at_boot`). `on` brings a simple domain back — CONFIGURATION bit 0 through EL3, a millisecond, the secure world's restore — without Linux's saved CMU registers, so the block starts from its reset defaults, not from ABL's setup — and it is not yet trusted: a TPU on and off by hand preceded a reset during the next power window (the boot's last crumb G#B4, 2026-10-25), while the same windows on a boot gated once at start ran clean three times. So the camera and TPU diags refuse while their domains are gated, and `diag pd on` is by hand only. The GPU's embedded part needs a PLL and atomic-alias sequence not written yet: once gated, the GPU stays off until that is.

use crate::platform::priv_reg;

const PMU: u64 = 0x1546_0000;

/// (name, the domain's CMU base, its PMU block offset). Its `need_smc` address is the CMU base + G#1_0204 for each of these (zuma-pm-domains.dtsi).
const DOMAINS: [(&str, usize, u64); 14] = [
    ("g2d", 0x1A80_0000, 0x1E80),
    ("gdc", 0x1D40_0000, 0x1F00),
    ("gse", 0x1D80_0000, 0x1F80),
    ("mcsc", 0x1D00_0000, 0x2000),
    ("mfc", 0x1A40_0000, 0x2080),
    ("rgbp", 0x1C40_0000, 0x2100),
    ("tnr", 0x1CC0_0000, 0x2180),
    ("yuvp", 0x1C80_0000, 0x2200),
    ("aur", 0x20A0_0000, 0x2300),
    ("bw", 0x1A60_0000, 0x2380),
    ("eh", 0x16C0_0000, 0x2400),
    ("ispfe", 0x1C00_0000, 0x2480),
    ("tpu", 0x1A30_0000, 0x2500),
    // The GPU last, after its embedded part (`embedded_g3d_off`, a child of this domain).
    ("g3d", 0x1EE0_0000, 0x1A80),
];

fn rd(a: usize) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}
fn ms(n: u64) {
    let t0 = crate::platform::cntpct();
    while crate::platform::cntpct().wrapping_sub(t0) < crate::platform::cntfrq() / 1000 * n {
        core::hint::spin_loop();
    }
}

/// A domain's CONFIGURATION word through EL3 (bit 0: on).
fn conf(off: u64) -> u32 {
    priv_reg(PMU + off, false, 0) as u32
}

fn tz_save(addr: usize) -> i64 {
    let r: i64;
    unsafe { core::arch::asm!("smc #0", inout("x0") 0x8200_0410u64 => r, in("x1") 0u64, in("x2") addr as u64, in("x3") 2u64) };
    r
}

/// The GPU's embedded part (`embedded_g3d_off`): its Q-channel held, its PMU domain off, the Q-channel released, the core clock muxes to their inputs, both GPU PLLs stopped and parked.
fn embedded_g3d_off(out: &mut alloc::vec::Vec<u8>) {
    const CMU: usize = 0x1EE0_0000;
    let before = conf(0x1C80);
    if before & 1 == 0 {
        out.extend_from_slice(b"embedded_g3d: already off\n");
        return;
    }
    wr(CMU + 0x3044, (rd(CMU + 0x3044) & !7) | 6);
    let w = priv_reg(PMU + 0x1C80, true, before & !1);
    ms(1);
    wr(CMU + 0x3044, (rd(CMU + 0x3044) & !7) | 2);
    for m in [0x0630, 0x0640, 0x0650, 0x0660] {
        wr(CMU + m, rd(CMU + m) & !(1 << 4));
    }
    wr(CMU + 0x0100, rd(CMU + 0x0100) & !(1 << 4));
    wr(CMU + 0x0104, 0x1800_0001);
    wr(CMU + 0x0180, rd(CMU + 0x0180) & !(1 << 4));
    wr(CMU + 0x0184, 0x1800_0001);
    out.extend_from_slice(alloc::format!("embedded_g3d: configuration G#{before:X} -> write {w:X} -> G#{:X}\n", conf(0x1C80)).as_bytes());
}

fn off(name: &str, cmu: usize, pmu_off: u64, out: &mut alloc::vec::Vec<u8>) {
    if name == "g3d" {
        embedded_g3d_off(out);
    }
    let before = conf(pmu_off);
    if before & 1 == 0 {
        out.extend_from_slice(alloc::format!("{name}: already off\n").as_bytes());
        return;
    }
    let s = tz_save(cmu + 0x1_0204);
    wr(cmu + 0x800, rd(cmu + 0x800) & !(1 << 24));
    let w = priv_reg(PMU + pmu_off, true, before & !1);
    ms(1);
    out.extend_from_slice(alloc::format!("{name}: secure save {s}, configuration G#{before:X} -> write {w:X} -> G#{:X}\n", conf(pmu_off)).as_bytes());
}

fn tz_restore(addr: usize) -> i64 {
    let r: i64;
    unsafe { core::arch::asm!("smc #0", inout("x0") 0x8200_0410u64 => r, in("x1") 1u64, in("x2") addr as u64, in("x3") 2u64) };
    r
}

/// A simple domain back on (not the GPU): CONFIGURATION bit 0 through EL3, a millisecond, the secure world's restore.
pub fn on(name: &str, out: &mut alloc::vec::Vec<u8>) {
    let Some(&(_, cmu, pmu_off)) = DOMAINS.iter().find(|d| d.0 == name && d.0 != "g3d") else {
        out.extend_from_slice(alloc::format!("{name}: no on sequence here\n").as_bytes());
        return;
    };
    let before = conf(pmu_off);
    if before & 1 == 1 {
        return;
    }
    let w = priv_reg(PMU + pmu_off, true, before | 1);
    ms(1);
    let r = tz_restore(cmu + 0x1_0204);
    out.extend_from_slice(alloc::format!("{name}: configuration G#{before:X} -> write {w:X} -> G#{:X}, secure restore {r}\n", conf(pmu_off)).as_bytes());
}

/// Whether a domain's CONFIGURATION reads on (through EL3).
pub fn is_on(name: &str) -> bool {
    DOMAINS.iter().find(|d| d.0 == name).map(|&(_, _, off)| conf(off) & 1 == 1).unwrap_or(true)
}

/// The camera's blocks back on (by hand, `diag pd on`, not automatic: see the module doc).
pub fn camera_on(out: &mut alloc::vec::Vec<u8>) {
    for n in ["ispfe", "rgbp", "yuvp", "gdc", "gse", "mcsc", "tnr"] {
        on(n, out);
    }
}

static mut GATED: bool = false;
/// From the service loop, once, `after` the service started: every domain here off, the result into the flash log.
pub fn gate_at_boot(booted_for: u64, after: u64) {
    if unsafe { GATED } || booted_for < after {
        return;
    }
    unsafe { GATED = true };
    let mut out = alloc::vec::Vec::new();
    for &(name, cmu, pmu_off) in DOMAINS.iter() {
        off(name, cmu, pmu_off, &mut out);
    }
    for l in out.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        if let Ok(t) = core::str::from_utf8(l) {
            crate::flashlog::line(&alloc::format!("pd: {t}"));
        }
    }
}

/// `diag pd`: every domain's CONFIGURATION through EL3. `diag pd off|on <name>|all`: the off sequence, or a simple domain back on.
pub fn diag(args: &str, out: &mut alloc::vec::Vec<u8>) {
    let mut w = args.split_whitespace();
    match (w.next(), w.next()) {
        (Some("on"), Some(which)) => {
            for &(name, _, _) in DOMAINS.iter().filter(|d| which == "all" || d.0 == which) {
                on(name, out);
            }
        }
        (Some("off"), Some(which)) => {
            for &(name, cmu, pmu_off) in DOMAINS.iter().filter(|d| which == "all" || d.0 == which) {
                off(name, cmu, pmu_off, out);
            }
        }
        _ => {
            for &(name, _, pmu_off) in DOMAINS.iter() {
                out.extend_from_slice(alloc::format!("{name} G#{:X} ", conf(pmu_off)).as_bytes());
            }
            out.extend_from_slice(alloc::format!("embedded_g3d G#{:X}\n", conf(0x1C80)).as_bytes());
        }
    }
}
