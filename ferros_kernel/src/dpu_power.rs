//! The DPU's power domains, on and off (DISPLAY-DESIGN.md §8, layers 1 and 2): what only the kernel may do for the display, because the PMU's writes go through EL3.
//! On, in the CAL engine's order (pmucal_local_enable): each domain's CONFIGURATION bit 0 through SMC_CMD_PRIV_REG and its STATUS (+4) polled; the D_TZPC restore (SMC G#8200_0410: 1, need_smc, 2) once for a domain that was really off (a second prepare on a prepared domain never returns, camera.rs); then the CMU words the CAL engine would restore, from the table measured on a cold boot (oracle/display/husky-hk3/cold-power); then the DPHY's isolation released.
//! Off, for the self-test that takes ferros's own display down and back up: CMU CONTROLLER_OPTION bit 24 cleared, CONFIGURATION 0, STATUS polled to 0, the children (DPUF1, DPUF0) before DPUB.
//! Nothing here touches DECON, DSIM or the SysMMUs: with a domain down that would poison EL3 (display.rs).

use alloc::format;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

/// (name, CONFIGURATION, need_smc, CMU base, the cold-boot CMU capture), parent first.
const DOMAINS: [(&str, u64, u64, usize, &[u8]); 3] = [
    ("DPUB", 0x1546_1D00, 0x1941_0204, 0x1940_0000, include_bytes!("../../oracle/display/husky-hk3/cold-power/cmu_dpub.bin")),
    ("DPUF0", 0x1546_1D80, 0x1981_0204, 0x1980_0000, include_bytes!("../../oracle/display/husky-hk3/cold-power/cmu_dpuf0.bin")),
    ("DPUF1", 0x1546_1E00, 0x19C1_0204, 0x19C0_0000, include_bytes!("../../oracle/display/husky-hk3/cold-power/cmu_dpuf1.bin")),
];
/// The words the CAL engine restores, per domain (flexpmu_cal_local_zuma.h), CONTROLLER_OPTION (+G#800) last: offsets, or inclusive ranges as (first, last).
const DPUB_WORDS: [(usize, usize); 4] = [(0x1800, 0x1800), (0x600, 0x600), (0x610, 0x610), (0x3060, 0x30F0)];
const DPUF_WORDS: [(usize, usize); 2] = [(0x600, 0x600), (0x3044, 0x30BC)];
const CONTROLLER_OPTION: usize = 0x800;
/// The DPUF sysregs (G#1982_0000, G#19C2_0000) are never touched: their words read 0 from the non-secure side (D_TZPC secure), and an access poisons EL3.
/// The DSIM0 DPHY's isolation in the PMU (G#3EB8 bit 0, 1 = PHY usable), set and cleared through the PMU's atomic aliases (exynos-pmu-if.c: +G#C000 sets, +G#8000 clears, the value is the bit index).
const DPHY_ISO: usize = 0x1546_3EB8;
const DPHY_ISO_SET: u64 = 0x1546_FEB8;
const DPHY_ISO_CLEAR: u64 = 0x1546_BEB8;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn word(cap: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([cap[off], cap[off + 1], cap[off + 2], cap[off + 3]])
}
fn status(conf: u64) -> u32 {
    rd(conf as usize + 4) & 1
}
/// Poll a domain's STATUS for `want`, 5 ms (the CAL engine's 5000 x 1 us).
fn wait_status(conf: u64, want: u32) -> bool {
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while status(conf) != want {
        if crate::platform::cntpct().wrapping_sub(t0) > f / 200 {
            return false;
        }
    }
    true
}
/// SMC_CMD_PREPARE_PD_ONOFF (G#8200_0410): `wake` 0 saves the domain's D_TZPC state before it goes down (EXYNOS_GET_IN_PD_DOWN), 1 restores it after it comes up (EXYNOS_WAKEUP_PD_DOWN); always as a pair, as Linux does (pmucal_local.c): a restore with no save before it never returned (2026-11-03, the first `dpu on` after an `off` that skipped the save).
fn tz(wake: u64, need_smc: u64) -> u64 {
    let ret: u64;
    unsafe {
        core::arch::asm!("smc #0",
            inout("x0") 0x8200_0410u64 => ret, inout("x1") wake => _, inout("x2") need_smc => _, inout("x3") 2u64 => _,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x16") _, out("x17") _);
    }
    ret
}

/// All three domains: CONFIGURATION and STATUS both on.
pub fn all_on() -> bool {
    DOMAINS.iter().all(|d| rd(d.1 as usize) & 1 == 1 && status(d.1) == 1)
}

pub fn report(rep: &mut Vec<u8>) {
    for d in DOMAINS.iter() {
        rep.extend_from_slice(format!("{}: configuration G#{:X} status G#{:X}\n", d.0, rd(d.1 as usize), rd(d.1 as usize + 4)).as_bytes());
    }
    rep.extend_from_slice(format!("DPHY isolation G#{:X} (bit 0: 1 = PHY usable)\n", rd(DPHY_ISO)).as_bytes());
    // The CMU words only with the domains on (a CMU read in a dead domain is as dangerous as DECON's).
    if all_on() {
        for (i, d) in DOMAINS.iter().enumerate() {
            let words: &[(usize, usize)] = if i == 0 { &DPUB_WORDS } else { &DPUF_WORDS };
            let mut diff = 0;
            for &(a, b) in words.iter().chain([(CONTROLLER_OPTION, CONTROLLER_OPTION)].iter()) {
                let mut o = a;
                while o <= b {
                    // Only words the cold boot read as non-zero: the zeros in these ranges (Q-channel slots, and the DPUF sysregs entirely) read 0 because they are secure (D_TZPC), and a non-secure access to one poisons EL3 like a dead domain does (2026-11-03: a report reading them, then the next EL3 call never returned).
                    if word(d.4, o) == 0 {
                        o += 4;
                        continue;
                    }
                    let now = rd(d.3 + o);
                    if now != word(d.4, o) {
                        diff += 1;
                        rep.extend_from_slice(format!("  CMU {} +G#{o:X}: G#{now:X}, cold boot G#{:X}\n", d.0, word(d.4, o)).as_bytes());
                    }
                    o += 4;
                }
            }
            rep.extend_from_slice(format!("CMU {}: {} of its restored words differ from the cold boot's\n", d.0, diff).as_bytes());
        }
    }
}

/// The domains off, children first. The caller has stopped everything that uses them (the display server killed, DECON and DSIM stopped).
pub fn off(rep: &mut Vec<u8>) -> bool {
    // The pipe stopped first, as Android stops it before hibernation (DISPLAY-MODESET.md, DECON stop; DSIM's HS clock request dropped): only with the domains on and DPUB's user muxes on their clocks (G#10: after a bare power-on they read 0 and a DECON access hung the core, 2026-11-03), the poison rule.
    if all_on() && rd(DOMAINS[0].3 + 0x600) == 0x10 && rd(DOMAINS[0].3 + 0x610) == 0x10 {
        crate::platform::pstore(b"ferros: dpu off: DECON stop\n");
        const DECON: usize = 0x1947_0000;
        let t = rd(DECON + 0x30);
        wr(DECON + 0x30, (t | 1 << 4) & !1);
        wr(DECON + 0x20, rd(DECON + 0x20) & !1);
        wr(DECON + 0x50, rd(DECON + 0x50) | 1 << 31 | 1 << 20);
        let f = crate::platform::cntfrq();
        let t0 = crate::platform::cntpct();
        while rd(DECON + 0x20) >> 4 & 1 == 1 && crate::platform::cntpct().wrapping_sub(t0) < f / 20 {}
        let stopped = rd(DECON + 0x20) >> 4 & 1 == 0;
        // The DSIM only out of reset (SYSREG_DPUB +G#1008 M_RESETN, bit 0; 1 on a cold boot): after a power-up it holds 0 until the DSIM's own init, and a DSIM access then hung the core (2026-11-03).
        const DSIM: usize = 0x1944_0000;
        if rd(0x1942_1008) & 1 == 1 {
            crate::platform::pstore(b"ferros: dpu off: DSIM clock\n");
            wr(DSIM + 0x20, rd(DSIM + 0x20) & !(1 << 20));
        }
        rep.extend_from_slice(format!("DECON {} (GLOBAL_CON G#{:X}); DSIM HS clock request dropped\n", if stopped { "stopped" } else { "STILL RUNNING after 50 ms" }, rd(DECON + 0x20)).as_bytes());
    }
    let _ = crate::platform::priv_reg(DPHY_ISO_CLEAR, true, 0);
    let mut ok = true;
    for d in DOMAINS.iter().rev() {
        if status(d.1) == 0 {
            rep.extend_from_slice(format!("{}: already off\n", d.0).as_bytes());
            continue;
        }
        crate::platform::pstore(format!("ferros: dpu off: {} D_TZPC save\n", d.0).as_bytes());
        let s = tz(0, d.2);
        crate::platform::pstore(format!("ferros: dpu off: {} configuration 0\n", d.0).as_bytes());
        rep.extend_from_slice(format!("{}: D_TZPC saved (EL3 answered G#{s:X})\n", d.0).as_bytes());
        let opt = d.3 + CONTROLLER_OPTION;
        wr(opt, rd(opt) & !(1 << 24));
        let w = crate::platform::priv_reg(d.1, true, 0);
        let off = wait_status(d.1, 0);
        ok &= off;
        rep.extend_from_slice(format!("{}: configuration 0 (EL3 answered G#{w:X}), status {}\n", d.0, if off { "0" } else { "STILL ON after 5 ms" }).as_bytes());
    }
    ok
}

/// The domains on, parent first, each restored as the CAL engine restores it; then the DPHY's isolation released.
/// Without the Q-channel group (2): after a bare power-on every Q-channel word already reads as the cold boot's, and rewriting DPUB's range left DPUF0's power-on request hanging in EL3 (2026-11-03); `report` still checks them against the capture. Without the DPUF sysreg group (8) too: writing their cold-boot words (0, 0) after a power-on left DECON unreachable (the next DECON access hung the core, 2026-11-03); `report` shows what they hold.
pub fn on(rep: &mut Vec<u8>) -> bool {
    on_with(rep, 0x5)
}

/// `restore` false: the domains and the D_TZPC only, no CMU or sysreg word written (the self-test's bisection: the first `dpu on` with the restore hung in EL3 at DPUF0's power-on request, right after DPUB's CMU restore).
/// `restore`: which groups of the CAL engine's restore to write, bits: 1 dividers and user muxes, 2 Q-channels, 4 CONTROLLER_OPTION, 8 the DPUF sysregs (the bisection of the hang at DPUF0's power-on request after DPUB's full restore).
pub fn on_with(rep: &mut Vec<u8>, restore: u32) -> bool {
    for (i, d) in DOMAINS.iter().enumerate() {
        let was = status(d.1);
        if was == 0 {
            crate::platform::pstore(format!("ferros: dpu on: {} configuration\n", d.0).as_bytes());
            let conf = rd(d.1 as usize);
            let w = crate::platform::priv_reg(d.1, true, conf | 1);
            crate::platform::pstore(format!("ferros: dpu on: {} configuration written, EL3 answered G#{w:X}\n", d.0).as_bytes());
            if !wait_status(d.1, 1) {
                rep.extend_from_slice(format!("{}: configuration G#{:X} (EL3 answered G#{w:X}), status still 0 after 5 ms; stopping here\n", d.0, conf | 1).as_bytes());
                return false;
            }
            crate::platform::pstore(format!("ferros: dpu on: {} on, D_TZPC restore\n", d.0).as_bytes());
            let t = tz(1, d.2);
            rep.extend_from_slice(format!("{}: on (EL3 answered G#{w:X}), D_TZPC restore answered G#{t:X}\n", d.0).as_bytes());
        } else {
            rep.extend_from_slice(format!("{}: already on, nothing restored\n", d.0).as_bytes());
            continue;
        }
        let words: &[(usize, usize)] = if i == 0 { &DPUB_WORDS } else { &DPUF_WORDS };
        crate::platform::pstore(format!("ferros: dpu on: {} CMU restore\n", d.0).as_bytes());
        for &(a, b) in words {
            // The Q-channel ranges start at G#3000; the rest are dividers and muxes.
            let group = if a >= 0x3000 { 2 } else { 1 };
            if restore & group == 0 {
                continue;
            }
            let mut o = a;
            while o <= b {
                // As in `report`: never a word the cold boot read as 0 (secure).
                if word(d.4, o) != 0 {
                    wr(d.3 + o, word(d.4, o));
                }
                o += 4;
            }
        }
    }
    // CONTROLLER_OPTION last, once all three domains are up: DPUB's (G#F11F_F03F, the power-management and auto-gating bits over the bare G#1F_F03F) written before DPUF0's power-on request left that request hanging in EL3 (2026-11-03).
    if restore & 4 != 0 {
        crate::platform::pstore(b"ferros: dpu on: CONTROLLER_OPTION\n");
        for d in DOMAINS.iter() {
            wr(d.3 + CONTROLLER_OPTION, word(d.4, CONTROLLER_OPTION));
        }
    }
    // The DPUF S2MPUs (inside their domains): a domain's reset turns protection on over a permission table of power-on noise, and the layer DMA's every read faulted (RDMA_IRQ READ_SLAVE_ERR at the frame buffer's address: the screen black after a jump from a screen-off Android and after `diag dpu off`, 2026-11-04). Fenced now that the domains are up: the display engine reads its two regions and nothing else (s2mpu::fence_dpu).
    let fenced = crate::s2mpu::fence_dpu();
    rep.extend_from_slice(format!("DPU S2MPUs fenced: G#{fenced:X} (1 = fenced)\n").as_bytes());
    crate::platform::pstore(b"ferros: dpu on: DPHY isolation\n");
    let w = crate::platform::priv_reg(DPHY_ISO_SET, true, 0);
    rep.extend_from_slice(format!("DPHY isolation released (EL3 answered G#{w:X}), now G#{:X}\n", rd(DPHY_ISO)).as_bytes());
    true
}

/// The `dpu` power switch (process.rs POWERS): the display server powers the DPU up when it finds it down (SYS_FACTS 30 = 1: Android's hibernation at a jump, or ferros's own `dpu off`), and could take it down.
pub fn power(on: bool) -> bool {
    let mut rep = Vec::new();
    let ok = if on { all_on() || on_with(&mut rep, 0x5) } else { off(&mut rep) };
    crate::platform::pstore(&rep);
    ok
}

/// `diag dpu [on|off]`: the state, or the domains switched (the self-test; the display server must not be running for `off`).
pub fn diag(args: &str, rep: &mut Vec<u8>) {
    match args.trim() {
        "off" => {
            off(rep);
        }
        "on" => {
            on(rep);
        }
        "on raw" => {
            on_with(rep, 0);
        }
        a if a.starts_with("on ") => {
            on_with(rep, a[3..].trim().parse().unwrap_or(0));
        }
        _ => {}
    }
    report(rep);
}
