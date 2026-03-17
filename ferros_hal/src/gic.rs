//! ARM GICv3 — minimal init for WFI wakeup on specific SPIs.
//!
//! QCM6490 GIC layout (from DTS):
//!   GICD (Distributor):    G#17A00000 (64KB)
//!   GICR (Redistributor): G#17A60000 (1MB, 8 CPUs)
//!   GICR CPU0 SGI_base:   G#17A70000
//!
//! ## Approach
//!
//! ABL/TF-A already initialized the GIC. We just need to:
//! 1. Enable Group 1 NS interrupts (ICC system registers)
//! 2. Enable specific SPIs (DWC3 = SPI 133)
//! 3. Use WFI to sleep — CPU wakes on any enabled interrupt
//! 4. Keep PSTATE.I masked — no exception handler needed, just poll after wake

const GICD_BASE: usize = 0x17A0_0000;
#[allow(dead_code)]
const GICR_BASE: usize = 0x17A6_0000;
#[allow(dead_code)]
const GICR_SGI_BASE: usize = 0x17A7_0000;

// GICD register offsets
const GICD_CTLR: usize = 0x000;
const GICD_ISENABLER: usize = 0x100; // + 4 * (n/32)
#[allow(dead_code)]
const GICD_IPRIORITYR: usize = 0x400; // + n (byte-addressable)

// GICD_CTLR bits
const GICD_CTLR_ENABLE_GRP1_NS: u32 = 1 << 1;

// GICR register offsets (from GICR_BASE for CPU 0)
const GICR_WAKER: usize = 0x014;
const GICR_WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const GICR_WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;

/// DWC3 USB controller interrupt: SPI 133 (INTID 165).
pub const SPI_DWC3: u32 = 133;

/// ARM non-secure physical timer: PPI 14 (INTID 30).
#[allow(dead_code)]
pub const PPI_NS_PHYS_TIMER: u32 = 14;

/// Full GIC init including GICD/GICR MMIO. Returns true if successful.
/// WARNING: GICD is TZ-protected on QCM6490 — causes exception. Do not use.
#[allow(dead_code)]
pub fn init() -> bool {
    unsafe {
        let sre: u64;
        core::arch::asm!("mrs {}, ICC_SRE_EL1", out(reg) sre);
        if sre & 1 == 0 { return false; }

        let waker = crate::mmio::read32(GICR_BASE + GICR_WAKER);
        if waker & GICR_WAKER_PROCESSOR_SLEEP != 0 {
            crate::mmio::write32(GICR_BASE + GICR_WAKER, waker & !GICR_WAKER_PROCESSOR_SLEEP);
            for _ in 0..1_000_000u32 {
                if crate::mmio::read32(GICR_BASE + GICR_WAKER) & GICR_WAKER_CHILDREN_ASLEEP == 0 { break; }
            }
        }

        let ctlr = crate::mmio::read32(GICD_BASE + GICD_CTLR);
        if ctlr & GICD_CTLR_ENABLE_GRP1_NS == 0 {
            crate::mmio::write32(GICD_BASE + GICD_CTLR, ctlr | GICD_CTLR_ENABLE_GRP1_NS);
        }

        core::arch::asm!("msr ICC_PMR_EL1, {}", in(reg) 0xFFu64);
        core::arch::asm!("msr ICC_IGRPEN1_EL1, {}", in(reg) 1u64);
        core::arch::asm!("isb");
    }
    true
}

/// Lightweight GIC init — EL1 system registers only, no MMIO.
///
/// ABL/QHEE already configured GICD and enabled the DWC3 SPI.
/// We just open the CPU interface so WFI wakes on pending interrupts.
/// No GICD/GICR MMIO access — safe on TZ-locked QCM6490.
pub fn init_el1_only() -> bool {
    unsafe {
        // Verify ICC system register interface is available
        let sre: u64;
        core::arch::asm!("mrs {}, ICC_SRE_EL1", out(reg) sre);
        if sre & 1 == 0 { return false; }

        // Open priority mask — allow all interrupt priorities
        core::arch::asm!("msr ICC_PMR_EL1, {}", in(reg) 0xFFu64);

        // Enable Group 1 NS interrupts at CPU interface
        core::arch::asm!("msr ICC_IGRPEN1_EL1, {}", in(reg) 1u64);

        // Barrier: ensure config takes effect before WFI
        core::arch::asm!("isb");
    }
    true
}

/// Enable a specific SPI in the GIC distributor.
pub fn enable_spi(spi: u32) {
    let reg_idx = spi / 32;
    let bit = 1u32 << (spi % 32);
    unsafe {
        crate::mmio::write32(GICD_BASE + GICD_ISENABLER + 4 * reg_idx as usize, bit);
    }
}

/// Execute WFI — CPU halts until an interrupt fires.
/// Returns immediately if an interrupt is already pending.
#[inline]
pub fn wfi() {
    unsafe {
        core::arch::asm!("wfi");
    }
}
