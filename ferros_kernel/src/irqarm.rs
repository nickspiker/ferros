//! Armed actions (SYS_IRQ_ARM; docs/KERNEL.md "Armed actions"; the encoding and its check: ferros_layout irqarm): a driver arms up to four register operations on its own granted line, inside one of its own hardware grants, checked once at the arm; `.Lirq` runs them when the line fires, before the line is masked and the driver woken, and never takes the kernel lock.
//! The first user is the camera (camera-full-rate, 2026-11-18): its next PDMA descriptor released inside the frame's blanking whatever holds the lock.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use ferros_layout::irqarm::{Op, ADVANCE, OPS_MOST, TEST_BITS, TEST_EQ, WRITE};

/// Lines, by `IRQS` index (process.rs `IRQ_GUARDED`: room for 2^5).
const LINES: usize = 1 << 5;

/// Per INTID, the line's slot plus one while it is armed (0 not): `.Lirq` tests it with one load and calls `ferros_irq_armed` only for an armed line.
#[unsafe(no_mangle)]
pub static mut __irq_armed: [u8; 1024] = [0; 1024];

/// Whether armed actions run at all (`diag irqarm on|off`, on): off, every line takes the path it took before them, for an A/B.
pub static ON: AtomicBool = AtomicBool::new(true);

/// Per slot: a sequence count, odd while the slot is being written (the interrupt copies the program and keeps it only if the count did not move); the operations armed (0 none); each operation as three words, resolved to absolute addresses at the arm: kind | a << 32, the register, and b (a second register for TEST_EQ, else the operand); the arming process's epoch.
static SEQ: [AtomicU32; LINES] = [const { AtomicU32::new(0) }; LINES];
static N: [AtomicU32; LINES] = [const { AtomicU32::new(0) }; LINES];
static OPS: [[[AtomicU64; 3]; OPS_MOST]; LINES] = [const { [const { [const { AtomicU64::new(0) }; 3] }; OPS_MOST] }; LINES];
static EPOCH: [AtomicU64; LINES] = [const { AtomicU64::new(0) }; LINES];
/// Per slot, for `diag irqs`: runs, runs whose every operation ran (acted), runs a test stopped, runs left because the slot was being rewritten.
static COUNTS: [[AtomicU64; 4]; LINES] = [const { [const { AtomicU64::new(0) }; 4] }; LINES];

fn rd(a: u64) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}
fn wr(a: u64, v: u32) {
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}

/// Arm line `slot` (INTID `intid`) with `ops`, checked already (ferros_layout irqarm `check`) against the grant at `base`, for the process of `epoch`. Under the kernel lock, as every arm and disarm is: two never race; the interrupt may, and the sequence count covers it.
pub fn arm(slot: usize, intid: u32, base: usize, ops: &[Op], epoch: u64) {
    SEQ[slot].fetch_add(1, Ordering::AcqRel);
    for (k, o) in ops.iter().enumerate() {
        let b = if o.op == TEST_EQ { (base + o.b as usize) as u64 } else { o.b as u64 };
        OPS[slot][k][0].store(o.op as u64 | (o.a as u64) << 32, Ordering::Relaxed);
        OPS[slot][k][1].store((base + o.off as usize) as u64, Ordering::Relaxed);
        OPS[slot][k][2].store(b, Ordering::Relaxed);
    }
    N[slot].store(ops.len() as u32, Ordering::Relaxed);
    EPOCH[slot].store(epoch, Ordering::Relaxed);
    SEQ[slot].fetch_add(1, Ordering::Release);
    unsafe { core::ptr::write_volatile(&raw mut __irq_armed[intid as usize], slot as u8 + 1) };
    for c in COUNTS[slot].iter() {
        c.store(0, Ordering::Relaxed);
    }
}

/// Line `slot` (INTID `intid`) disarmed: the byte first, so `.Lirq` stops calling, then the program.
pub fn disarm(slot: usize, intid: u32) {
    unsafe { core::ptr::write_volatile(&raw mut __irq_armed[intid as usize], 0) };
    SEQ[slot].fetch_add(1, Ordering::AcqRel);
    N[slot].store(0, Ordering::Relaxed);
    EPOCH[slot].store(0, Ordering::Relaxed);
    SEQ[slot].fetch_add(1, Ordering::Release);
}

/// Every line the process of `epoch` armed, disarmed: at its end and when it turns a power switch off (no action may touch a block whose clocks are going).
pub fn disarm_epoch(epoch: u64) {
    if epoch == 0 {
        return;
    }
    for (slot, e) in EPOCH.iter().enumerate() {
        if e.load(Ordering::Relaxed) == epoch {
            disarm(slot, crate::process::IRQS[slot].intid);
        }
    }
}

/// `.Lirq` for an armed line (main.rs `.Lirq_armed`, caller-saved registers saved): its program copied out, kept only if no arm or disarm moved it meanwhile, then run in order, a failed test ending the run.
#[unsafe(no_mangle)]
pub extern "C" fn ferros_irq_armed(intid: u64) {
    // `__irq_armed` holds only a slot plus one that `arm` wrote, under `LINES`, and the INTID is the GIC's, under 1020 (`.Lirq` sends nothing past it).
    let slot = unsafe { core::ptr::read_volatile(&raw const __irq_armed[intid as usize]) } as usize;
    if slot == 0 || !ON.load(Ordering::Relaxed) {
        return;
    }
    let slot = slot - 1;
    let c = &COUNTS[slot];
    c[0].fetch_add(1, Ordering::Relaxed);
    let s = SEQ[slot].load(Ordering::Acquire);
    // `arm` stores at most `OPS_MOST` (the check refused more).
    let n = N[slot].load(Ordering::Relaxed) as usize;
    let mut prog = [[0u64; 3]; OPS_MOST];
    for (k, p) in prog.iter_mut().take(n).enumerate() {
        *p = [OPS[slot][k][0].load(Ordering::Relaxed), OPS[slot][k][1].load(Ordering::Relaxed), OPS[slot][k][2].load(Ordering::Relaxed)];
    }
    core::sync::atomic::fence(Ordering::Acquire);
    if s & 1 != 0 || SEQ[slot].load(Ordering::Relaxed) != s {
        c[3].fetch_add(1, Ordering::Relaxed);
        return;
    }
    for &[w, reg, b] in prog.iter().take(n) {
        let a = (w >> 32) as u32;
        match w as u32 {
            TEST_BITS if rd(reg) & a == 0 => {
                c[2].fetch_add(1, Ordering::Relaxed);
                return;
            }
            TEST_EQ if rd(reg) != rd(b) => {
                c[2].fetch_add(1, Ordering::Relaxed);
                return;
            }
            // WHY wrapping: a ring index advanced and masked; the mask makes the wrap the ring's own.
            ADVANCE => wr(reg, rd(reg).wrapping_add(a) & b as u32),
            WRITE => wr(reg, b as u32),
            _ => {}
        }
    }
    c[1].fetch_add(1, Ordering::Relaxed);
}

/// For `diag irqs`: each armed line, its operations and its counts.
pub fn report(out: &mut alloc::vec::Vec<u8>) {
    out.extend_from_slice(alloc::format!("armed actions {}:", if ON.load(Ordering::Relaxed) { "on" } else { "off (diag irqarm on)" }).as_bytes());
    let mut any = false;
    // WHY the min: the table has room for `LINES`, the kernel names fewer lines; only those have a name to say.
    for slot in 0..LINES.min(crate::process::IRQS.len()) {
        let n = N[slot].load(Ordering::Relaxed);
        let c = &COUNTS[slot];
        if n == 0 && c[0].load(Ordering::Relaxed) == 0 {
            continue;
        }
        any = true;
        out.extend_from_slice(alloc::format!(" {} (A#{n} ops) runs A#{}, acted A#{}, stopped by a test A#{}, left mid-arm A#{};", crate::process::IRQS[slot].name, c[0].load(Ordering::Relaxed), c[1].load(Ordering::Relaxed), c[2].load(Ordering::Relaxed), c[3].load(Ordering::Relaxed)).as_bytes());
    }
    if !any {
        out.extend_from_slice(b" none armed");
    }
    out.push(b'\n');
}
