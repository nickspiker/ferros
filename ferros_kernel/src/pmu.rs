//! The performance monitors open to EL0 (owner, 2026-10-24): processes read the cycle counter and program and read the event counters themselves (`ferros_sys::pmu`), so a hot loop is measured where it runs. Per core, as each one comes up (boot core, the X3 after its hop, each worker), since the PMU's state is per core.
//!
//! What this hands EL0 is a timer finer than CNTVCT (the CPU's own clock) and the core-wide event counts, which include whatever else ran there; it is a development tool on this unit. `FERROS_PMU_EL0=0` at build time leaves the PMU closed.

/// Open this core's PMU to EL0:
/// - MDCR_EL2: HPMN = PMCR_EL0.N (every event counter in the range EL0 may use), TPM and TPMCR clear (no trap of PMU access to EL2);
/// - PMCR_EL0: E (counters on) and LC (the cycle counter overflows at 64 bits);
/// - PMCCFILTR_EL0: count EL0 and EL1 as the reset value does, and EL2 (NSH), as kohi's jitter source already sets it;
/// - PMUSERENR_EL0: EN, SW, CR, ER (EL0 may program, increment, and read the cycle and event counters);
/// - PMCNTENSET_EL0: the cycle counter and every event counter enabled (events are idle until a process programs PMEVTYPER).
pub fn el0_on() {
    if option_env!("FERROS_PMU_EL0") == Some("0") {
        return;
    }
    unsafe {
        core::arch::asm!(
            "mrs {n}, pmcr_el0",
            "ubfx {n}, {n}, #11, #5",
            "mrs {t}, mdcr_el2",
            "bic {t}, {t}, #0x7F",
            "orr {t}, {t}, {n}",
            "msr mdcr_el2, {t}",
            "mrs {t}, pmcr_el0",
            "orr {t}, {t}, #1",
            "orr {t}, {t}, #0x40",
            "msr pmcr_el0, {t}",
            "mov {t}, #(1 << 27)",
            "msr pmccfiltr_el0, {t}",
            "mov {t}, #0xF",
            "msr pmuserenr_el0, {t}",
            "mov {t}, #1",
            "lsl {t}, {t}, {n}",
            "sub {t}, {t}, #1",
            "orr {t}, {t}, #(1 << 31)",
            "msr pmcntenset_el0, {t}",
            "isb",
            n = out(reg) _,
            t = out(reg) _,
            options(nostack),
        );
    }
}
