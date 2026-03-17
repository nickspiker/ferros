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

/// Initialize GIC for WFI wakeup. Returns true if successful.
///
/// After this call, WFI will wake on any enabled SPI/PPI.
/// PSTATE.I stays masked — no exception handler needed.
pub fn init() -> bool {
    unsafe {
        // 1. Verify system register interface is enabled (TF-A should have done this)
        let sre: u64;
        core::arch::asm!("mrs {}, ICC_SRE_EL1", out(reg) sre);
        if sre & 1 == 0 {
            return false; // SRE not enabled, can't use system registers
        }

        // 2. Wake GICR for CPU 0 (ABL likely already did this)
        let waker = crate::mmio::read32(GICR_BASE + GICR_WAKER);
        if waker & GICR_WAKER_PROCESSOR_SLEEP != 0 {
            crate::mmio::write32(
                GICR_BASE + GICR_WAKER,
                waker & !GICR_WAKER_PROCESSOR_SLEEP,
            );
            // Poll until ChildrenAsleep clears
            for _ in 0..1_000_000u32 {
                if crate::mmio::read32(GICR_BASE + GICR_WAKER) & GICR_WAKER_CHILDREN_ASLEEP == 0 {
                    break;
                }
            }
        }

        // 3. Enable Group 1 NS in distributor (may already be enabled)
        let ctlr = crate::mmio::read32(GICD_BASE + GICD_CTLR);
        if ctlr & GICD_CTLR_ENABLE_GRP1_NS == 0 {
            crate::mmio::write32(GICD_BASE + GICD_CTLR, ctlr | GICD_CTLR_ENABLE_GRP1_NS);
        }

        // 4. Set priority mask to allow all priorities
        core::arch::asm!("msr ICC_PMR_EL1, {}", in(reg) 0xFFu64);

        // 5. Enable Group 1 interrupts at CPU interface
        core::arch::asm!("msr ICC_IGRPEN1_EL1, {}", in(reg) 1u64);

        // ISB to ensure all config takes effect before WFI
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
