//! The SoC's power domains that ferros does not use, switched off (POWER.md, 2026-10-25): ABL leaves every block's domain on — the GPU, the 2D engine, the camera's whole ISP chain, the video codecs, AUR, EH and the TPU read on through EL3 — and ferros had never turned one off. Linux turns idle ones off as its runtime PM finds them unused.
//!
//! The off sequence is Linux's (cal-if/zuma/flexpmu_cal_local_zuma.h `<pd>_off`, cal-if/pmucal_local.c `pmucal_local_disable`): the secure world's save (exynos-pd_el3.c `exynos_pd_tz_save`: SMC G#8200_0410, 0 = going down, the domain's `need_smc` address from zuma-pm-domains.dtsi, group 2), the domain's CMU controller option bit 24 cleared (its own clock controller, reachable while the domain is still on), then PMU CONFIGURATION bit 0 cleared. CONFIGURATION is written and read through EL3 (`priv_reg`; word 0 of each domain block is the one EL3 will read), STATUS (+4) read plainly, as display.rs reads the DPU's; the sequence waits a fixed millisecond rather than polling STATUS. The EL3 poison rule (a firmware call that never returns after an access to a block that is down or secure) is an SError left waiting on the core, which EL3 bounces back until it panics (platform.rs `take_serror`): before each firmware call here one waiting is taken at EL2 and named.
//!
//! After a jump Linux's sequence ran into a frozen g2d twice (2026-11-10 and 2026-11-15 b883, the kernel's last line "pd: gating g2d after the jump"). Now each step says itself in the flash log after a jump, each CMU access is checked for an error answer, and a domain whose CMU answered with one is left on with its S2MPUs denied all (`left_on`) instead of hanging the next firmware call. A cut thru the PMU alone is no way out: it never completes and wedges the APM (`off`, measured on b885). `diag pd poison <name>` makes the EL3 poison on purpose (a read of a down domain's CMU) and cures it.
//!
//! Measured with the screen off (odpm and the gauge, 2026-10-25): the rails' outputs 1558 → 1185 mW (the PLL rail 127 → 25, INT 85 → 23, the GPU's 66 → 15, the camera's 35 → 14), the battery 0.98 → 0.64 W; with the screen on 1.58 → 1.38 W.
//!
//! On by default: the service loop gates every domain here once, 16 s in (`gate_at_boot`). `on` brings a simple domain back — CONFIGURATION bit 0 through EL3, a millisecond, the secure world's restore — without Linux's saved CMU registers, so the block starts from its reset defaults, not from ABL's setup — and it is not yet trusted: a TPU on and off by hand preceded a reset during the next power window (the boot's last crumb G#B4, 2026-10-25), while the same windows on a boot gated once at start ran clean three times. So `diag pd on` is by hand only; the one domain brought back in use is the camera's front end, by the `camera` switch (camera_power.rs), once a boot, its restore the pair of the gating's save. The GPU's embedded part needs a PLL and atomic-alias sequence not written yet: once gated, the GPU stays off until that is.

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

/// After a jump, a line on the flash log before each step of a domain's switch: the power cycle that ends a jumped session keeps nothing else, and a step that hangs is then the last one named (2026-11-15, b883: the jump's kernel froze after "pd: gating g2d after the jump", with five steps inside that line).
fn step(name: &str, what: &str) {
    if crate::platform::jumped() {
        crate::flashlog::line(&alloc::format!("pd: {name}: {what}"));
    }
}

/// An SError this core had waiting, taken at EL2 before a firmware call (platform.rs `take_serror`: left waiting, EL3 bounces the call back until it panics), and said: into the report, and after a jump at once into the flash log, since it names the access before it as the one answered with an error.
fn serror_before(name: &str, what: &str, out: &mut alloc::vec::Vec<u8>) {
    if crate::platform::take_serror() {
        let t = alloc::format!("{name}: an SError was waiting before {what}, taken at EL2");
        if crate::platform::jumped() {
            crate::flashlog::line(&alloc::format!("pd: {t}"));
        }
        out.extend_from_slice(alloc::format!("{t}\n").as_bytes());
    }
}

/// A domain's PMU STATUS (the block's +4, bit 0), read plainly: the PMU is readable from here, and EL3's read of a STATUS register does not return it (display.rs `dpu_on`).
fn status(pmu_off: u64) -> u32 {
    rd((PMU + pmu_off) as usize + 4)
}

/// Each domain's S2MPUs (zuma-s2mpu.dtsi, each inside its domain), in front of every DMA its block makes. The GPU's is s2mpu.rs's own (`deny_gpu`).
fn s2mpus(name: &str) -> &'static [usize] {
    match name {
        "g2d" => &[0x1A8A_0000],
        "gdc" => &[0x1D4F_0000],
        "gse" => &[0x1D8A_0000],
        "mcsc" => &[0x1D0D_0000],
        "mfc" => &[0x1A4A_0000],
        "rgbp" => &[0x1C4D_0000, 0x1C51_0000],
        "tnr" => &[0x1CCB_0000, 0x1CCF_0000, 0x1CD3_0000],
        "yuvp" => &[0x1C8A_0000],
        "aur" => &[0x20A8_0000],
        "bw" => &[0x1A67_0000],
        "eh" => &[0x16C7_0000],
        "ispfe" => &[0x1C14_0000, 0x1C18_0000, 0x1C1C_0000],
        "tpu" => &[0x1A37_0000],
        _ => &[],
    }
}

/// A domain whose CMU answered an access with an error, left on: its power-down cannot be asked for (`off`), so its block's DMA is stopped at its S2MPUs instead, each denied all (s2mpu.rs `deny_all`, the hv's steps at the jump), each said with whether its own accesses were answered with an error too.
fn left_on(name: &str, what: &str, out: &mut alloc::vec::Vec<u8>) {
    let mut t = alloc::format!("{name}: the CMU's controller option {what} was answered with an error (an SError, taken at EL2): left on, no secure save, no configuration write (a power-down the CMU does not answer wedged the APM until its watchdog reset the Pro, 2026-11-15 b885); its S2MPUs denied all:");
    for &b in s2mpus(name) {
        step(name, &alloc::format!("denying its S2MPU G#{b:X}"));
        let r = crate::s2mpu::deny_all(b);
        t.push_str(&alloc::format!(" G#{b:X} G#{r:X}{}", if crate::platform::take_serror() { " (an error answered)" } else { "" }));
    }
    if crate::platform::jumped() {
        crate::flashlog::line(&alloc::format!("pd: {t}"));
    }
    out.extend_from_slice(alloc::format!("{t}\n").as_bytes());
}

/// Linux's off sequence (flexpmu_cal_local_zuma.h `<pd>_save` then `<pd>_off`): the CMU's controller option read (the save reads it first), the secure save, bit 24 of the controller option cleared, CONFIGURATION off through EL3.
/// The CMU's part is not optional: a domain cut thru the PMU alone (CONFIGURATION off with bit 24 still set) never reached STATUS off, and the APM wedged until its watchdog reset the device (`diag pd cut`, 2026-11-15 b885, removed).
/// Each CMU access is checked for an error answer (an SError, taken at EL2): after a jump the block is as Android left it, and the chord freezes Android in the middle of a domain's job (G2D's driver holds its clock and EL3's protection for each job and gives both back only in its completion), a state Linux's own off never meets. A CMU that answered with an error leaves the domain on, fenced at its S2MPUs (`left_on`).
/// After a jump every step says itself in the flash log first, so a step that hangs is the last one named (b883 froze after "pd: gating g2d after the jump", five steps inside that line).
fn off(name: &str, cmu: usize, pmu_off: u64, out: &mut alloc::vec::Vec<u8>) {
    if name == "g3d" {
        embedded_g3d_off(out);
    }
    serror_before(name, "its configuration read", out);
    let before = conf(pmu_off);
    if before & 1 == 0 {
        out.extend_from_slice(alloc::format!("{name}: already off\n").as_bytes());
        return;
    }
    // STATUS as well, as the hv's jump checks the display's domains: CONFIGURATION is only the request, and reads on while a domain is still coming up or going down. A CMU access in a domain that is down is answered with an error (measured, `diag pd poison`).
    step(name, "reading its status");
    let st = status(pmu_off);
    if st & 1 == 0 {
        out.extend_from_slice(alloc::format!("{name}: requested on (G#{before:X}) but its status reads off (G#{st:X}): left alone, no CMU access\n").as_bytes());
        return;
    }
    step(name, "reading the CMU's controller option");
    let opt = rd(cmu + 0x800);
    if crate::platform::take_serror() {
        left_on(name, "read", out);
        return;
    }
    step(name, &alloc::format!("controller option G#{opt:X}; the secure save"));
    serror_before(name, "the secure save", out);
    let s = tz_save(cmu + 0x1_0204);
    step(name, "writing the CMU's controller option");
    wr(cmu + 0x800, opt & !(1 << 24));
    if crate::platform::take_serror() {
        left_on(name, "write", out);
        return;
    }
    step(name, "configuration off thru EL3");
    let w = priv_reg(PMU + pmu_off, true, before & !1);
    ms(1);
    out.extend_from_slice(alloc::format!("{name}: controller option G#{opt:X}, secure save {s}, configuration G#{before:X} -> write {w:X} -> G#{:X} status G#{:X}\n", conf(pmu_off), status(pmu_off)).as_bytes());
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
    // The front end's restore is the camera's prepare call (the same SMC): made a second time it never returns (2026-10-21), so the camera's `PREPARED` must know it was made (camera.rs `prepare_once`).
    if name == "ispfe" {
        crate::camera::mark_prepared();
    }
    out.extend_from_slice(alloc::format!("{name}: configuration G#{before:X} -> write {w:X} -> G#{:X}, secure restore {r}\n", conf(pmu_off)).as_bytes());
}

/// Whether a domain's CONFIGURATION reads on (through EL3).
pub fn is_on(name: &str) -> bool {
    DOMAINS.iter().find(|d| d.0 == name).map(|&(_, _, off)| conf(off) & 1 == 1).unwrap_or(true)
}

static mut GATED: bool = false;
/// From the service loop, once, `after` the service started: every domain here off, the result into the flash log.
pub fn gate_at_boot(booted_for: u64, after: u64) {
    if unsafe { GATED } || booted_for < after {
        return;
    }
    unsafe { GATED = true };
    let mut out = alloc::vec::Vec::new();
    // An SError waiting from anything before this pass would hang its first firmware call (the configuration read below): taken first, and named.
    serror_before("pass", "the first domain", &mut out);
    // The GPU stays as ABL leaves it, powered and clocked, since ferros drives it (GPU.md, 2026-11-04): this file has its off sequence but no on, which needs its PLLs back through ACPM's DVFS. Gating it saved about 50 mW (its rail 66 to 15 mW, 2026-10-25); the GPU driver will gate it itself once it has a power switch with both directions.
    // The front end's too while the camera switch brought it on before this pass (a camera opened in a boot's first 16 s): the driver is streaming thru it.
    for (i, &(name, cmu, pmu_off)) in DOMAINS.iter().enumerate().filter(|(_, d)| d.0 != "g3d" && !(d.0 == "ispfe" && crate::camera_power::fe_on())) {
        // Said before each domain is gated, crumb and line both (the crumb G#E0 + its index, the core's note), so a hang inside one names it: twice a jump froze the kernel's core in this pass with nothing logged after the display's fence (2026-11-09).
        crate::platform::crumb(0xE0 + i as u64);
        if crate::platform::jumped() {
            crate::flashlog::line(&alloc::format!("pd: gating {name} after the jump"));
        }
        off(name, cmu, pmu_off, &mut out);
    }
    for l in out.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        if let Ok(t) = core::str::from_utf8(l) {
            crate::flashlog::line(&alloc::format!("pd: {t}"));
        }
    }
}

/// `diag pd`: every domain's CONFIGURATION through EL3. `diag pd off|on <name>|all`: the off sequence, or a simple domain back on. `diag pd poison <name>`: the EL3 poison made and cured, on a domain that is down.
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
        (Some("poison"), Some(which)) => {
            // The EL3 poison and its cure, on purpose: one read of a domain's CMU (its controller option, a word Linux reads before every off) with the domain down, which is answered with an error; whether an SError then waits on this core; taken at EL2; then a firmware call. Refused unless the domain reads down, so it is never a live block's register.
            for &(name, cmu, pmu_off) in DOMAINS.iter().filter(|d| d.0 == which) {
                if conf(pmu_off) & 1 == 1 || status(pmu_off) & 1 == 1 {
                    out.extend_from_slice(alloc::format!("{name}: on, refused (the test needs it down)\n").as_bytes());
                    continue;
                }
                let v = rd(cmu + 0x800);
                let isr: u64;
                unsafe { core::arch::asm!("dsb sy", "isb", "mrs {i}, isr_el1", i = out(reg) isr) };
                let taken = crate::platform::take_serror();
                out.extend_from_slice(alloc::format!("{name}: CMU G#{:X} read G#{v:X} with the domain down; ISR_EL1 G#{isr:X}, an SError taken at EL2 {taken}; then EL3 reads its configuration G#{:X}\n", cmu + 0x800, conf(pmu_off)).as_bytes());
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
