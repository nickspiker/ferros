//! zuma's CoreSight trace system (zuma-debug.dtsi; the exynos-etm driver's map), stopped after a jump from Android before the CPUCL0 S2MPU in front of its bus master is closed (2026-11-15, HV.md "The high bank after a jump").
//!
//! Its one DRAM writer is the TMC-ETR, whose buffer Linux allocated (exynos-coresight-etm.c: 1 MiB, `dma_alloc_coherent`, circular): after a jump that buffer lies in memory ferros now owns, and the ETR writes it through the CPUCL0 S2MPU, which ABL and Android leave open.
//! Closing that S2MPU with trace still flowing froze a jumped ferros (2026-11-04): every core silent, most likely the trace backing up into the cores once its sink lost memory.
//! So the order is sources, then sinks, then the wall: each core's trace unit (ETE, by its memory-mapped frame, the way the exynos-etm driver stops it, so no system register EL3 may trap), then the TMCs (ETR, the two ETFs, the BDU's ETF), then the ETR's bridge to USB in HSI0, and only then the S2MPU.
//! Every register is read first and written only when it differs from stopped, and what was found is the report (feedback: measure before fixing).

use core::ptr::{read_volatile, write_volatile};

/// Core k's trace unit (ETE, external frame): `cs_base` G#2A00_0000 + (k + 1) * G#10_0000 + G#113_0000 (exynos-etm's per-core offsets), cores 0-8.
fn ete(k: usize) -> usize {
    0x2B13_0000 + k * 0x10_0000
}
/// TRCPRGCTLR (bit 0: the trace unit enabled), TRCSTATR (bit 0: idle), TRCOSLAR / TRCOSLSR (the OS lock), TRCLAR (the software lock's key goes here).
const TRCPRGCTLR: usize = 0x004;
const TRCSTATR: usize = 0x00C;
const TRCOSLAR: usize = 0x300;
const TRCOSLSR: usize = 0x304;
const TRCLAR: usize = 0xFB0;
const CS_UNLOCK: u32 = 0xC5AC_CE55;

/// The TMCs: the ETR (the sink that writes DRAM), ETF0 and ETF1, and the BDU's ETF (zuma-debug.dtsi, exynos-etm).
const TMCS: [(&str, usize); 4] = [("etr", 0x2A00_A000), ("etf0", 0x2A00_4000), ("etf1", 0x2A00_5000), ("bdu-etf", 0x2A01_1000)];
/// TMC registers: RSZ, STS (bit 2 TMCReady), RWP and RWPHI (where it writes next), CTL (bit 0 TraceCaptEn), MODE, DBALO and DBAHI (the ETR's buffer), LAR.
const TMC_RSZ: usize = 0x004;
const TMC_STS: usize = 0x00C;
const TMC_RWP: usize = 0x018;
const TMC_CTL: usize = 0x020;
const TMC_MODE: usize = 0x028;
const TMC_RWPHI: usize = 0x03C;
const TMC_DBALO: usize = 0x118;
const TMC_DBAHI: usize = 0x11C;
const TMC_LAR: usize = 0xFB0;

/// The ETR's bridge to USB (ETR_MIU in HSI0, f_etr_miu.c): ETR_MIU_ON at +G#20.
const ETR_MIU_ON: usize = 0x110D_0020;

/// Whether core k is powered (PSCI AFFINITY_INFO 0, MPIDR Aff1 = k): a core's trace frame sits in its debug power domain, and an access to a core that is off answers with a bus error (the exynos-etm driver touches online cores only). Parked cores (the jump's mailbox) are on.
fn core_on(k: usize) -> bool {
    let info: u64;
    unsafe { core::arch::asm!("smc #0", inout("x0") 0xC400_0004u64 => info, inout("x1") (k as u64) << 8 => _, inout("x2") 0u64 => _, inout("x3") 0u64 => _, out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _, out("x10") _, out("x11") _, out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x16") _, out("x17") _) };
    info == 0
}

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) };
    unsafe { core::arch::asm!("dsb sy") };
}

/// What the trace system holds now, read only: each core's trace unit, each TMC, the ETR's bridge.
pub fn report(out: &mut alloc::string::String) {
    use core::fmt::Write;
    for k in 0..9 {
        if !core_on(k) {
            let _ = writeln!(out, "ete core {k}: off (not read)");
            continue;
        }
        let b = ete(k);
        let _ = writeln!(out, "ete core {k}: PRGCTLR G#{:X} STATR G#{:X} OSLSR G#{:X}", rd(b + TRCPRGCTLR), rd(b + TRCSTATR), rd(b + TRCOSLSR));
    }
    for &(name, b) in &TMCS {
        let _ = write!(out, "{name}: CTL G#{:X} STS G#{:X} MODE G#{:X} RSZ G#{:X} RWP G#{:X}", rd(b + TMC_CTL), rd(b + TMC_STS), rd(b + TMC_MODE), rd(b + TMC_RSZ), rd(b + TMC_RWP));
        if name == "etr" {
            let _ = write!(out, " RWPHI G#{:X} DBA G#{:X}_{:08X}", rd(b + TMC_RWPHI), rd(b + TMC_DBAHI), rd(b + TMC_DBALO));
        }
        out.push('\n');
    }
    let _ = writeln!(out, "etr-miu: ON G#{:X}", rd(ETR_MIU_ON));
}

/// Trace stopped, sources first: each trace unit found enabled is unlocked, disabled and locked again (the exynos-etm stop: LAR, OSLAR 0, PRGCTLR 0, OSLAR 1, LAR), then each TMC found capturing is disabled and waited on for TMCReady (a hardware handshake, at most 2^16 reads: an ETR that never drains is reported, not waited on), then the ETR's bridge off. The report says what each was and what it is now.
pub fn stop(out: &mut alloc::string::String) {
    use core::fmt::Write;
    let mut stopped = 0;
    for k in (0..9).filter(|&k| core_on(k)) {
        let b = ete(k);
        let was = rd(b + TRCPRGCTLR);
        if was & 1 == 0 {
            continue;
        }
        wr(b + TRCLAR, CS_UNLOCK);
        wr(b + TRCOSLAR, 0);
        wr(b + TRCPRGCTLR, 0);
        wr(b + TRCOSLAR, 1);
        wr(b + TRCLAR, 0);
        stopped += 1;
        let _ = writeln!(out, "ete core {k}: PRGCTLR G#{was:X} -> G#{:X}", rd(b + TRCPRGCTLR));
    }
    for &(name, b) in &TMCS {
        let was = rd(b + TMC_CTL);
        if was & 1 == 0 {
            continue;
        }
        wr(b + TMC_LAR, CS_UNLOCK);
        wr(b + TMC_CTL, 0);
        let mut ready = false;
        for _ in 0..1 << 16 {
            if rd(b + TMC_STS) & (1 << 2) != 0 {
                ready = true;
                break;
            }
        }
        wr(b + TMC_LAR, 0);
        stopped += 1;
        let _ = writeln!(out, "{name}: CTL G#{was:X} -> G#{:X}, STS G#{:X}{}", rd(b + TMC_CTL), rd(b + TMC_STS), if ready { "" } else { " (never ready)" });
    }
    let miu = rd(ETR_MIU_ON);
    if miu != 0 {
        wr(ETR_MIU_ON, 0);
        stopped += 1;
        let _ = writeln!(out, "etr-miu: ON G#{miu:X} -> G#{:X}", rd(ETR_MIU_ON));
    }
    if stopped == 0 {
        out.push_str("trace: nothing was running\n");
    }
}
