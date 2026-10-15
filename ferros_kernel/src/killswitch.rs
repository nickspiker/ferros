//! Killswitch (KILLSWITCH.md): Vol Up + Vol Down held together → PSCI SYSTEM_OFF → the PMIC cuts the rails. No zeroing, no cleanup: the keys live in on-die flip-flops that stop existing when VCORE drops.
//!
//! It is the first interrupt ferros takes, and the key presses ARE the interrupt — nothing polls. Both volume keys sit on the always-on GPIO bank, where every pin can raise its own GIC SPI (Exynos wakeup EINT): vol-down = gpa4-1 → SPI 29, vol-up = gpa6-2 → SPI 32 (ABL's DTB). Each pin interrupts on both edges; the handler (`.Lirq` in main.rs, a few instructions of assembly with no Rust state) reads both keys and, if both are held, calls SYSTEM_OFF on the spot, whatever code was running — UFS bring-up, the vault, a RUN payload, a wedged loop.
//!
//! `arm()` runs first thing in kernel_main on both boot paths. ferros polls every other device, so every other interrupt is switched off here; one that still fires is switched off by the handler.

use core::ptr::{read_volatile, write_volatile};

// Always-on pin controller (GPIO_ALIVE). Bank gpa4 at +G#80, gpa6 at +G#A0: CON (4 bits/pin) at +0, DAT at +4. Wakeup EINT registers (Exynos7 layout): CON G#700, MASK G#900, PEND G#A00, each + the bank's EINT offset (gpa4 G#10, gpa6 G#14).
const ALIVE: usize = 0x154D_0000;
const GPA4_CON: usize = ALIVE + 0x80;
const GPA4_DAT: usize = ALIVE + 0x84;
const GPA6_CON: usize = ALIVE + 0xA0;
const GPA6_DAT: usize = ALIVE + 0xA4;
const EINT_CON: usize = ALIVE + 0x700;
const EINT_MASK: usize = ALIVE + 0x900;
const EINT_PEND: usize = ALIVE + 0xA00;
const GPA4_EINT: usize = 0x10;
const GPA6_EINT: usize = 0x14;
const VOL_DOWN_PIN: u32 = 1; // gpa4-1
const VOL_UP_PIN: u32 = 2; // gpa6-2
const PIN_FUNC_EINT: u32 = 0xF;
const EINT_BOTH_EDGES: u32 = 4;

// GICv3 (ABL's DTB: distributor G#1040_0000, redistributors from G#1044_0000, G#2_0000 per CPU).
const GICD: usize = 0x1040_0000;
const GICR: usize = 0x1044_0000;
const GICR_STRIDE: usize = 0x2_0000;
const GICR_SGI: usize = 0x1_0000;
pub const VOL_DOWN_INTID: u32 = 32 + 29;
pub const VOL_UP_INTID: u32 = 32 + 32;

const PSCI_SYSTEM_OFF: u64 = 0x8400_0008;

/// What `arm` found and did, for DIAG (KILL_*).
pub struct Armed {
    /// Index of this CPU's redistributor frame, G#FF = not found (nothing armed).
    pub gicr_frame: u32,
    pub gicd_ctlr: u32,
    pub hcr_before: u64,
    /// Both volume-key DAT bits right after the pins became EINTs: must read 1 (released) or the keys would read as held.
    pub keys_idle: u32,
}

/// The last `arm()` result, for DIAG: frame, GICD_CTLR, HCR_EL2 before (low 32), keys_idle.
pub static REPORT: [core::sync::atomic::AtomicU32; 4] = [const { core::sync::atomic::AtomicU32::new(0) }; 4];

fn record(a: Armed) -> Armed {
    use core::sync::atomic::Ordering::Relaxed;
    REPORT[0].store(a.gicr_frame, Relaxed);
    REPORT[1].store(a.gicd_ctlr, Relaxed);
    REPORT[2].store(a.hcr_before as u32, Relaxed);
    REPORT[3].store(a.keys_idle, Relaxed);
    a
}

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn set_nibble(a: usize, pin: u32, v: u32) {
    wr(a, (rd(a) & !(0xF << (pin * 4))) | (v << (pin * 4)));
}

fn both_held() -> bool {
    rd(GPA4_DAT) & (1 << VOL_DOWN_PIN) == 0 && rd(GPA6_DAT) & (1 << VOL_UP_PIN) == 0
}

/// Either volume key down right now (DAT bit 0 = pressed). The "Ready?" gate's dev timer reads it.
pub fn volume_key_down() -> bool {
    rd(GPA4_DAT) & (1 << VOL_DOWN_PIN) == 0 || rd(GPA6_DAT) & (1 << VOL_UP_PIN) == 0
}

fn system_off() -> ! {
    unsafe {
        core::arch::asm!("smc #0", in("x0") PSCI_SYSTEM_OFF, options(noreturn));
    }
}

/// The power key: gpa4-0 on the always-on bank (SPI 28, INTID 60), active low. `.Lirq` sets `__power_pressed` on its falling edge — the press, not the release (BOOT.md "Ready?" gate).
pub const POWER_INTID: u32 = 32 + 28;
const POWER_PIN: u32 = 0;
const EINT_FALLING: u32 = 2;

/// Arm the power key's press interrupt (after `arm`, which brought the GIC up).
pub fn arm_power_key() {
    set_nibble(GPA4_CON, POWER_PIN, PIN_FUNC_EINT);
    set_nibble(EINT_CON + GPA4_EINT, POWER_PIN, EINT_FALLING);
    wr(EINT_PEND + GPA4_EINT, 1 << POWER_PIN);
    wr(EINT_MASK + GPA4_EINT, rd(EINT_MASK + GPA4_EINT) & !(1 << POWER_PIN));
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
    let n = POWER_INTID as usize;
    unsafe { write_volatile((GICD + 0x6000 + n * 8) as *mut u64, (mpidr & 0xFF_FFFF) | (mpidr & 0xFF_0000_0000)) };
    let pr = GICD + 0x400 + (n & !3);
    wr(pr, (rd(pr) & !(0xFF << ((n & 3) * 8))) | (0x80 << ((n & 3) * 8)));
    let cfg = GICD + 0xC00 + (n / 16) * 4;
    wr(cfg, rd(cfg) & !(2 << ((n % 16) * 2)));
    wr(GICD + 0x100 + (n / 32) * 4, 1 << (n % 32));
}

/// Enable one private interrupt (PPI, INTID 16..31) on this CPU's redistributor at middle priority — e.g. the EL2 timer (26) that ends a process's time slice. No-op if `arm` found no redistributor.
pub fn enable_ppi(intid: u32) {
    let frame = REPORT[0].load(core::sync::atomic::Ordering::Relaxed);
    if frame == 0xFF || !(16..32).contains(&intid) {
        return;
    }
    let sgi = GICR + frame as usize * GICR_STRIDE + GICR_SGI;
    let pr = sgi + 0x400 + (intid as usize & !3);
    let sh = (intid as usize & 3) * 8;
    wr(pr, (rd(pr) & !(0xFF << sh)) | (0x80 << sh));
    wr(sgi + 0x100, 1 << intid);
}

/// Arm the killswitch. Safe to call again (RELOAD re-runs it).
pub fn arm() -> Armed {
    // Held at boot: die before anything else runs.
    if both_held() {
        system_off();
    }

    // The two pins become wakeup EINTs, both edges, pending cleared, unmasked.
    set_nibble(GPA4_CON, VOL_DOWN_PIN, PIN_FUNC_EINT);
    set_nibble(GPA6_CON, VOL_UP_PIN, PIN_FUNC_EINT);
    set_nibble(EINT_CON + GPA4_EINT, VOL_DOWN_PIN, EINT_BOTH_EDGES);
    set_nibble(EINT_CON + GPA6_EINT, VOL_UP_PIN, EINT_BOTH_EDGES);
    wr(EINT_PEND + GPA4_EINT, 1 << VOL_DOWN_PIN);
    wr(EINT_PEND + GPA6_EINT, 1 << VOL_UP_PIN);
    wr(EINT_MASK + GPA4_EINT, rd(EINT_MASK + GPA4_EINT) & !(1 << VOL_DOWN_PIN));
    wr(EINT_MASK + GPA6_EINT, rd(EINT_MASK + GPA6_EINT) & !(1 << VOL_UP_PIN));
    let keys_idle = (rd(GPA4_DAT) >> VOL_DOWN_PIN & 1) | (rd(GPA6_DAT) >> VOL_UP_PIN & 1) << 1;

    // This CPU's redistributor: the frame whose affinity (GICR_TYPER[63:32]) matches MPIDR.
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
    // GICR_TYPER[63:32] = Aff3.Aff2.Aff1.Aff0; MPIDR keeps Aff3 at [39:32] and flags (MT, U) at [31:24].
    let aff = (mpidr & 0xFF_FFFF) as u32 | (((mpidr >> 32) & 0xFF) as u32) << 24;
    let mut frame = None;
    for i in 0..16usize {
        let base = GICR + i * GICR_STRIDE;
        let typer_hi = rd(base + 0x0C);
        if typer_hi == aff {
            frame = Some(base);
            break;
        }
        if rd(base + 0x08) & (1 << 4) != 0 {
            break; // Last
        }
    }
    let hcr_before: u64;
    unsafe { core::arch::asm!("mrs {}, hcr_el2", out(reg) hcr_before) };
    let Some(gicr) = frame else {
        return record(Armed { gicr_frame: 0xFF, gicd_ctlr: rd(GICD), hcr_before, keys_idle });
    };

    // Redistributor awake (WAKER.ProcessorSleep = 0, wait ChildrenAsleep = 0); every SGI/PPI off.
    wr(gicr + 0x14, rd(gicr + 0x14) & !(1 << 1));
    for _ in 0..(1 << 20) {
        if rd(gicr + 0x14) & (1 << 2) == 0 {
            break;
        }
    }
    wr(gicr + GICR_SGI + 0x180, 0xFFFF_FFFF);

    // Distributor: every SPI off, then the two key SPIs on — routed to this CPU, middle priority, level (the EINT block holds the line until PEND is cleared).
    let lines = ((rd(GICD + 0x04) & 0x1F) + 1) as usize;
    for n in 1..lines {
        wr(GICD + 0x180 + n * 4, 0xFFFF_FFFF);
    }
    let gicd_wait = || {
        for _ in 0..(1 << 20) {
            if rd(GICD) & (1 << 31) == 0 {
                break;
            }
        }
    };
    gicd_wait();
    let route = (mpidr & 0xFF_FFFF) | (mpidr & 0xFF_0000_0000);
    for intid in [VOL_DOWN_INTID, VOL_UP_INTID] {
        let n = intid as usize;
        unsafe { write_volatile((GICD + 0x6000 + n * 8) as *mut u64, route) };
        let pr = GICD + 0x400 + (n & !3);
        wr(pr, (rd(pr) & !(0xFF << ((n & 3) * 8))) | (0x80 << ((n & 3) * 8)));
        let cfg = GICD + 0xC00 + (n / 16) * 4;
        wr(cfg, rd(cfg) & !(2 << ((n % 16) * 2)));
        wr(GICD + 0x100 + (n / 32) * 4, 1 << (n % 32));
    }
    // Non-secure group 1 on (ABL leaves it off; affinity routing is already on).
    wr(GICD, rd(GICD) | (1 << 1));
    gicd_wait();

    // CPU interface: system-register access, every priority unmasked, group 1 on. Physical IRQs to EL2 (HCR_EL2.IMO), then unmask.
    unsafe {
        core::arch::asm!(
            "mrs {t}, S3_4_C12_C9_5", // ICC_SRE_EL2
            "orr {t}, {t}, #1",
            "msr S3_4_C12_C9_5, {t}",
            "isb",
            "mov {t}, #0xF0",
            "msr S3_0_C4_C6_0, {t}", // ICC_PMR_EL1
            "msr S3_0_C12_C12_3, xzr", // ICC_BPR1_EL1
            "mov {t}, #1",
            "msr S3_0_C12_C12_7, {t}", // ICC_IGRPEN1_EL1
            "mrs {t}, hcr_el2",
            "orr {t}, {t}, #(1 << 4)",
            "msr hcr_el2, {t}",
            "isb",
            "msr daifclr, #2",
            t = out(reg) _,
        );
    }
    // Held while arming: the edge may already be gone.
    if both_held() {
        system_off();
    }
    record(Armed { gicr_frame: ((gicr - GICR) / GICR_STRIDE) as u32, gicd_ctlr: rd(GICD), hcr_before, keys_idle })
}
