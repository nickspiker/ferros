//! zuma's CoreSight trace sinks (zuma-debug.dtsi; the exynos-etm driver's map), stopped after a jump from Android (2026-11-15, HV.md "The high bank after a jump").
//!
//! Trace's one DRAM writer is the TMC-ETR, whose buffer Linux would allocate (exynos-coresight-etm.c: 1 MiB, `dma_alloc_coherent`, circular): after a jump that buffer would lie in memory ferros owns, written through the CPUCL0 S2MPU, which ABL and Android leave open.
//! The ETFs keep trace in their own SRAM and write no DRAM.
//! Only the TMCs are touched: read live on the jumped Pro (b879) they answered, all four off and the ETR never given a buffer.
//! The per-core trace units (ETE frames at G#2Bn3_0000) and the ETR's bridge to USB (ETR_MIU, G#110D_0020) are not: a kernel that read them at the start of a jumped session (b880, 2026-11-15) stalled the bus there and the APC watchdog reset the device (cores 4 and 8's frames had answered the bridge; which of the others, or ETR_MIU, stalled is not known).
//! With the ETR off nothing a trace unit emits reaches DRAM.

use core::ptr::{read_volatile, write_volatile};

/// The TMCs: the ETR (the sink that writes DRAM), ETF0 and ETF1, and the BDU's ETF (zuma-debug.dtsi, exynos-etm).
const TMCS: [(&str, usize); 4] = [("etr", 0x2A00_A000), ("etf0", 0x2A00_4000), ("etf1", 0x2A00_5000), ("bdu-etf", 0x2A01_1000)];
/// TMC registers: RSZ, STS (bit 2 TMCReady), RWP and RWPHI (where it writes next), CTL (bit 0 TraceCaptEn), MODE, DBALO and DBAHI (the ETR's buffer), LAR (the software lock's key goes here).
const TMC_RSZ: usize = 0x004;
const TMC_STS: usize = 0x00C;
const TMC_RWP: usize = 0x018;
const TMC_CTL: usize = 0x020;
const TMC_MODE: usize = 0x028;
const TMC_RWPHI: usize = 0x03C;
const TMC_DBALO: usize = 0x118;
const TMC_DBAHI: usize = 0x11C;
const TMC_LAR: usize = 0xFB0;
const CS_UNLOCK: u32 = 0xC5AC_CE55;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) };
    unsafe { core::arch::asm!("dsb sy") };
}

/// The TMCs as they stand, read only: each one's control, status, mode, size and write pointer, and the ETR's buffer.
pub fn report(out: &mut alloc::string::String) {
    use core::fmt::Write;
    for &(name, b) in &TMCS {
        let _ = write!(out, "{name}: CTL G#{:X} STS G#{:X} MODE G#{:X} RSZ G#{:X} RWP G#{:X}", rd(b + TMC_CTL), rd(b + TMC_STS), rd(b + TMC_MODE), rd(b + TMC_RSZ), rd(b + TMC_RWP));
        if name == "etr" {
            let _ = write!(out, " RWPHI G#{:X} DBA G#{:X}_{:08X}", rd(b + TMC_RWPHI), rd(b + TMC_DBAHI), rd(b + TMC_DBALO));
        }
        out.push('\n');
    }
}

/// Each TMC found capturing disabled and waited on for TMCReady (a hardware handshake, at most 2^16 reads: a TMC that never drains is reported, not waited on); one found off is not written. The report says what each was and what it is now.
pub fn stop(out: &mut alloc::string::String) {
    use core::fmt::Write;
    let mut stopped = 0;
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
    if stopped == 0 {
        out.push_str("trace: no sink was capturing\n");
    }
}
