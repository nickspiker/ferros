//! ferros hv — the thin EL2 layer under Android on husky (HV.md).
//!
//! Milestone 0, what this file does: ABL loads `boot_a`'s "kernel", which is this binary padded to G#18_0000 with the stock Linux Image appended, at G#8008_0000 and jumps here at EL2 with the MMU off and the DTB in x0. The hv copies itself to the last 64 MiB of DRAM (HV_BASE), zeroes its bss, copies the DTB next to itself with one node added (`/reserved-memory/hv@afc000000`, no-map, so Linux never allocates over it), programs EL2 so that EL1 sees a plain machine (timers, GIC system registers, pointer authentication, SVE, PMU, MTE all through; only SMC traps), and erets into the Linux Image at G#8020_0000 as EL1h.
//! From then on the hv runs only when Linux issues an SMC: PSCI calls that name a resume address (CPU_ON, CPU_SUSPEND, SYSTEM_SUSPEND) get that address swapped for `_secondary`, which brings the new or woken core up through the same EL2 setup and erets to where Linux wanted it; every other SMC is forwarded to EL3 unchanged.
//! There is no stage 2 yet (HCR_EL2.VM = 0): milestone 0 is Android booting, running and sleeping under a dormant hv. Stage 2 and the UFS command firewall come next.
//! Crumbs go to the ramoops result block at G#FD60_0000 from offset G#40 (ferros's own words are below it), where the next ferros boot reads them: +G#40 stage, +G#48 SMC traps, +G#50 CPU_ON count, +G#58 ESR of the last unexpected trap, +G#60 its ELR, +G#68 its FAR.

#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

use core::arch::{asm, global_asm};
use core::ptr::{read_volatile, write_volatile};

const HV_BASE: u64 = 0xA_FC00_0000;
const HV_SIZE: u64 = 0x400_0000;
/// Where the patched DTB goes: the megabyte below the reserved region, ordinary RAM that Linux reserves itself once it has read the tree from there.
const DTB_COPY: u64 = HV_BASE - 0x10_0000;
const DTB_COPY_MAX: usize = 0x10_0000;
/// The Linux Image: right after the hv's pad in the blob ABL loaded, 2 MiB aligned as its header demands (its flags say "place anywhere").
const LINUX_ENTRY: u64 = 0x8020_0000;
const REPORT: usize = 0xFD60_0000;

const STAGE_ENTERED: u64 = 0xA1;
const STAGE_DTB_DONE: u64 = 0xA2;
const STAGE_EL2_DONE: u64 = 0xA3;
const STAGE_ERET: u64 = 0xA4;
const STAGE_DTB_FAILED: u64 = 0xE1;
const STAGE_UNEXPECTED_TRAP: u64 = 0xE2;
const STAGE_PANIC: u64 = 0xE3;

global_asm!(include_str!("header.s"));

global_asm!(
    r#"
.section .text.boot, "ax"
.global _entry
_entry:
    msr     daifset, #0xF
    mov     x19, x0                     // DTB from ABL
    // Copy [_start, __hv_copy_end) to HV_BASE. PC-relative source, absolute destination; the MMU is off so these are plain DRAM writes.
    adrp    x1, _start
    add     x1, x1, :lo12:_start
    ldr     x2, =HV_BASE
    ldr     x3, =__hv_copy_size
    add     x3, x3, x2
1:  ldp     x4, x5, [x1], #16
    stp     x4, x5, [x2], #16
    cmp     x2, x3
    b.lo    1b
    dsb     sy
    ic      iallu
    dsb     sy
    isb
    ldr     x4, =.Lrelocated
    br      x4
.Lrelocated:
    // Zero bss, take the primary stack, enter Rust.
    ldr     x1, =__bss_start
    ldr     x2, =__bss_end
2:  cmp     x1, x2
    b.hs    3f
    str     xzr, [x1], #8
    b       2b
3:  ldr     x1, =__stacks
    add     sp, x1, #0x4000
    mov     x0, x19
    bl      hv_primary
4:  wfe
    b       4b

// A core PSCI started or woke at our address: x0 = the context Linux passed. Stack by the core's index in the table (hv_secondary looks it up and never returns).
.global _secondary
_secondary:
    msr     daifset, #0xF
    mov     x19, x0
    mrs     x1, mpidr_el1
    bl      core_index                  // x0 = 0..15 (leaf, no stack use)
    ldr     x1, =__stacks
    add     x0, x0, #1
    lsl     x0, x0, #14
    add     sp, x1, x0
    mov     x0, x19
    bl      hv_secondary
5:  wfe
    b       5b

// Exception vectors (VBAR_EL2). Only "lower EL, AArch64, synchronous" does real work; everything else records itself and parks the core.
.balign 0x800
.global hv_vectors
hv_vectors:
    .org hv_vectors + 0x000
    b       unexpected
    .org hv_vectors + 0x080
    b       unexpected
    .org hv_vectors + 0x100
    b       unexpected
    .org hv_vectors + 0x180
    b       unexpected
    .org hv_vectors + 0x200
    b       unexpected
    .org hv_vectors + 0x280
    b       unexpected
    .org hv_vectors + 0x300
    b       unexpected
    .org hv_vectors + 0x380
    b       unexpected
    .org hv_vectors + 0x400
    b       lower_sync
    .org hv_vectors + 0x480
    b       unexpected
    .org hv_vectors + 0x500
    b       unexpected
    .org hv_vectors + 0x580
    b       unexpected
    .org hv_vectors + 0x600
    b       unexpected
    .org hv_vectors + 0x680
    b       unexpected
    .org hv_vectors + 0x700
    b       unexpected
    .org hv_vectors + 0x780
    b       unexpected

// x0..x17 are the SMC's arguments and results; the guest's x18..x30 are callee-saved by the Rust handler, sp_el1 and the system state are untouched.
lower_sync:
    sub     sp, sp, #0xA0
    stp     x0, x1, [sp, #0x00]
    stp     x2, x3, [sp, #0x10]
    stp     x4, x5, [sp, #0x20]
    stp     x6, x7, [sp, #0x30]
    stp     x8, x9, [sp, #0x40]
    stp     x10, x11, [sp, #0x50]
    stp     x12, x13, [sp, #0x60]
    stp     x14, x15, [sp, #0x70]
    stp     x16, x17, [sp, #0x80]
    stp     x18, x30, [sp, #0x90]
    mov     x0, sp
    bl      hv_sync                     // rewrites x0..x17 in the frame
    ldp     x0, x1, [sp, #0x00]
    ldp     x2, x3, [sp, #0x10]
    ldp     x4, x5, [sp, #0x20]
    ldp     x6, x7, [sp, #0x30]
    ldp     x8, x9, [sp, #0x40]
    ldp     x10, x11, [sp, #0x50]
    ldp     x12, x13, [sp, #0x60]
    ldp     x14, x15, [sp, #0x70]
    ldp     x16, x17, [sp, #0x80]
    ldp     x18, x30, [sp, #0x90]
    add     sp, sp, #0xA0
    eret

unexpected:
    mrs     x0, esr_el2
    mrs     x1, elr_el2
    mrs     x2, far_el2
    bl      hv_unexpected
6:  wfe
    b       6b

// Leaf: x1 = MPIDR → x0 = index into the per-core tables (Aff1 is the core number on this SoC: cluster-level affinity sits in Aff2 and the nine cores are numbered 0-8 in Aff1).
.global core_index
core_index:
    ubfx    x0, x1, #8, #4
    ret
"#
);

fn report(off: usize, v: u64) {
    unsafe {
        write_volatile((REPORT + off) as *mut u64, v);
        asm!("dc civac, {a}", "dsb sy", a = in(reg) (REPORT + off) as u64);
    }
}

fn stage(s: u64) {
    report(0x40, s);
}

fn bump(off: usize) {
    unsafe {
        let p = (REPORT + off) as *mut u64;
        write_volatile(p, read_volatile(p).wrapping_add(1));
    }
}

/// Where each core should go after the hv brings it up: the entry and context Linux passed to PSCI, by core index.
static mut RESUME: [[u64; 2]; 16] = [[0; 2]; 16];

/// EL2 as the guest should find it. Called on every core before its first eret.
unsafe fn configure_el2() {
    // RW (EL1 is AArch64) | TSC (SMC traps here) | APK + API (pointer authentication keys and instructions untrapped) | ATA (MTE tag accesses untrapped where implemented). VM = 0: no stage 2 yet. IMO/FMO/AMO = 0: interrupts go to EL1 directly. E2H = 0, TGE = 0.
    let hcr: u64 = (1 << 31) | (1 << 19) | (1 << 40) | (1 << 41) | (1 << 56);
    asm!("msr hcr_el2, {h}", h = in(reg) hcr);
    // CPTR_EL2 (E2H = 0 layout): RES1 bits with TFP (10) and TZ (8) clear, so FP/SIMD and SVE run at EL1 untrapped; TAM (30) clear for the activity monitors.
    asm!("msr cptr_el2, {v}", v = in(reg) 0x32FFu64);
    // SVE present: let EL1 use the full vector length.
    let pfr0: u64;
    asm!("mrs {v}, id_aa64pfr0_el1", v = out(reg) pfr0);
    if (pfr0 >> 32) & 0xF != 0 {
        asm!("msr s3_4_c1_c2_0, {v}", v = in(reg) 0xFu64); // ZCR_EL2
    }
    // MDCR_EL2: every PMU counter visible to EL1 (HPMN = PMCR_EL0.N), the SPE buffer (E2PB) and trace buffer (E2TB) owned by EL1, no debug traps.
    let pmcr: u64;
    asm!("mrs {v}, pmcr_el0", v = out(reg) pmcr);
    let mdcr: u64 = ((pmcr >> 11) & 0x1F) | (3 << 12) | (3 << 24);
    asm!("msr mdcr_el2, {v}", v = in(reg) mdcr);
    // Timers: EL1 reads the physical counter and owns the physical timer; no virtual offset.
    asm!("msr cnthctl_el2, {v}", v = in(reg) 3u64);
    asm!("msr cntvoff_el2, {v}", v = in(reg) 0u64);
    // GICv3: system-register interface at EL2, and EL1 allowed to enable its own (ICC_SRE_EL2.Enable); no virtual CPU interface.
    asm!("msr s3_4_c12_c9_5, {v}", v = in(reg) 0xFu64); // ICC_SRE_EL2
    asm!("isb");
    asm!("msr s3_4_c12_c11_0, {v}", v = in(reg) 0u64); // ICH_HCR_EL2
    // The guest sees the real core.
    let midr: u64;
    let mpidr: u64;
    asm!("mrs {a}, midr_el1", "mrs {b}, mpidr_el1", a = out(reg) midr, b = out(reg) mpidr);
    asm!("msr vpidr_el2, {a}", "msr vmpidr_el2, {b}", a = in(reg) midr, b = in(reg) mpidr);
    asm!("msr hstr_el2, {v}", v = in(reg) 0u64);
    // EL1 starts with its MMU off, little-endian, as a kernel entry requires.
    asm!("msr sctlr_el1, {v}", v = in(reg) 0x30D0_0800u64);
    let vbar = hv_vectors as *const () as u64;
    asm!("msr vbar_el2, {v}", v = in(reg) vbar);
    asm!("isb");
}

unsafe extern "C" {
    fn hv_vectors();
    fn _secondary();
}

/// EL1h with every interrupt masked, as a kernel expects at entry.
const SPSR_EL1H_MASKED: u64 = 0x3C5;

unsafe fn eret_to(entry: u64, x0: u64) -> ! {
    stage(STAGE_ERET);
    asm!(
        "msr elr_el2, {e}",
        "msr spsr_el2, {s}",
        "mov x1, xzr",
        "mov x2, xzr",
        "mov x3, xzr",
        "dsb sy",
        "isb",
        "eret",
        e = in(reg) entry,
        s = in(reg) SPSR_EL1H_MASKED,
        in("x0") x0,
        options(noreturn)
    );
}

#[unsafe(no_mangle)]
extern "C" fn hv_primary(dtb: u64) -> ! {
    unsafe {
        write_volatile(REPORT as *mut u64, 0x4645_5252_4F53_5F48); // "FERROS_H": the block is ours too
    }
    stage(STAGE_ENTERED);
    report(0x48, 0);
    report(0x50, 0);
    let new_dtb = match fdt::patch(dtb as usize, DTB_COPY as usize, DTB_COPY_MAX) {
        Some(len) => {
            unsafe {
                let mut a = DTB_COPY;
                while a < DTB_COPY + len as u64 {
                    asm!("dc civac, {a}", a = in(reg) a);
                    a += 64;
                }
                asm!("dsb sy");
            }
            stage(STAGE_DTB_DONE);
            DTB_COPY
        }
        None => {
            // Without the reservation Linux would allocate over the hv; better to stop here where the crumb says why.
            stage(STAGE_DTB_FAILED);
            loop {
                unsafe { asm!("wfe") };
            }
        }
    };
    unsafe {
        configure_el2();
        stage(STAGE_EL2_DONE);
        eret_to(LINUX_ENTRY, new_dtb);
    }
}

#[unsafe(no_mangle)]
extern "C" fn hv_secondary(ctx: u64) -> ! {
    let mpidr: u64;
    unsafe { asm!("mrs {v}, mpidr_el1", v = out(reg) mpidr) };
    let idx = ((mpidr >> 8) & 0xF) as usize;
    let (entry, saved_ctx) = unsafe { (RESUME[idx][0], RESUME[idx][1]) };
    unsafe {
        configure_el2();
        // CPU_ON hands the context in x0; a resume from CPU_SUSPEND / SYSTEM_SUSPEND does too. Prefer what EL3 gave us, fall back to the recorded one.
        eret_to(entry, if ctx != 0 { ctx } else { saved_ctx });
    }
}

const PSCI_CPU_SUSPEND: u64 = 0xC400_0001;
const PSCI_CPU_ON: u64 = 0xC400_0003;
const PSCI_SYSTEM_SUSPEND: u64 = 0xC400_000E;
const PSCI_CPU_SUSPEND_32: u64 = 0x8400_0001;
const PSCI_CPU_ON_32: u64 = 0x8400_0003;

/// The trapped synchronous exception from EL1. `frame` holds x0..x17 (then x18, x30) as the guest left them; results go back through it.
#[unsafe(no_mangle)]
extern "C" fn hv_sync(frame: &mut [u64; 20]) {
    let esr: u64;
    unsafe { asm!("mrs {v}, esr_el2", v = out(reg) esr) };
    let ec = (esr >> 26) & 0x3F;
    match ec {
        0x17 => {
            // SMC from AArch64 EL1. ELR points at the SMC itself for this class; step over it.
            bump(0x48);
            let fid = frame[0];
            let me = unsafe {
                let m: u64;
                asm!("mrs {v}, mpidr_el1", v = out(reg) m);
                m
            };
            match fid {
                PSCI_CPU_ON | PSCI_CPU_ON_32 => {
                    bump(0x50);
                    let idx = ((frame[1] >> 8) & 0xF) as usize;
                    unsafe {
                        RESUME[idx] = [frame[2], frame[3]];
                        asm!("dc civac, {a}", "dsb sy", a = in(reg) (&raw const RESUME[idx]) as u64);
                    }
                    frame[2] = _secondary as *const () as u64;
                    // Context still travels through EL3 in x3 and arrives in x0 on the new core.
                }
                PSCI_CPU_SUSPEND | PSCI_CPU_SUSPEND_32 => {
                    let idx = ((me >> 8) & 0xF) as usize;
                    unsafe {
                        RESUME[idx] = [frame[2], frame[3]];
                        asm!("dc civac, {a}", "dsb sy", a = in(reg) (&raw const RESUME[idx]) as u64);
                    }
                    frame[2] = _secondary as *const () as u64;
                }
                PSCI_SYSTEM_SUSPEND => {
                    let idx = ((me >> 8) & 0xF) as usize;
                    unsafe {
                        RESUME[idx] = [frame[1], frame[2]];
                        asm!("dc civac, {a}", "dsb sy", a = in(reg) (&raw const RESUME[idx]) as u64);
                    }
                    frame[1] = _secondary as *const () as u64;
                }
                _ => {}
            }
            forward_smc(frame);
            unsafe {
                let elr: u64;
                asm!("mrs {v}, elr_el2", v = out(reg) elr);
                asm!("msr elr_el2, {v}", v = in(reg) elr + 4);
            }
        }
        0x16 => {
            // HVC from EL1: nothing at EL2 answers calls yet. SMCCC NOT_SUPPORTED; ELR already points past the HVC.
            frame[0] = u64::MAX;
        }
        _ => {
            let elr: u64;
            let far: u64;
            unsafe { asm!("mrs {a}, elr_el2", "mrs {b}, far_el2", a = out(reg) elr, b = out(reg) far) };
            hv_unexpected(esr, elr, far);
            loop {
                unsafe { asm!("wfe") };
            }
        }
    }
}

/// Issue the guest's SMC to EL3 with its arguments and bring every result register back.
fn forward_smc(f: &mut [u64; 20]) {
    unsafe {
        asm!(
            "smc #0",
            inout("x0") f[0], inout("x1") f[1], inout("x2") f[2], inout("x3") f[3],
            inout("x4") f[4], inout("x5") f[5], inout("x6") f[6], inout("x7") f[7],
            inout("x8") f[8], inout("x9") f[9], inout("x10") f[10], inout("x11") f[11],
            inout("x12") f[12], inout("x13") f[13], inout("x14") f[14], inout("x15") f[15],
            inout("x16") f[16], inout("x17") f[17],
            options(nostack)
        );
    }
}

#[unsafe(no_mangle)]
extern "C" fn hv_unexpected(esr: u64, elr: u64, far: u64) {
    report(0x58, esr);
    report(0x60, elr);
    report(0x68, far);
    stage(STAGE_UNEXPECTED_TRAP);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    stage(STAGE_PANIC);
    loop {
        unsafe { asm!("wfe") };
    }
}

/// The one DTB edit milestone 0 needs: a `hv@afc000000 { reg = <0xa 0xfc000000 0x0 0x4000000>; no-map; };` child under `/reserved-memory`, written as a fresh copy of the tree at `dst`. Big-endian flattened tree, standard token stream; the strings "reg" and "no-map" are taken from the existing strings block (every reserved-memory tree has them).
mod fdt {
    use core::ptr::{read_volatile, write_volatile};

    const MAGIC: u32 = 0xD00D_FEED;
    const BEGIN_NODE: u32 = 1;
    const END_NODE: u32 = 2;
    const PROP: u32 = 3;
    const NOP: u32 = 4;
    const END: u32 = 9;

    fn rd32(p: usize) -> u32 {
        u32::from_be(unsafe { read_volatile(p as *const u32) })
    }
    fn rd8(p: usize) -> u8 {
        unsafe { read_volatile(p as *const u8) }
    }
    fn wr8(p: usize, v: u8) {
        unsafe { write_volatile(p as *mut u8, v) }
    }
    fn wr32(p: usize, v: u32) {
        for (k, b) in v.to_be_bytes().iter().enumerate() {
            wr8(p + k, *b);
        }
    }
    fn copy(dst: usize, src: usize, n: usize) {
        for k in 0..n {
            wr8(dst + k, rd8(src + k));
        }
    }
    fn name_eq(p: usize, s: &[u8]) -> bool {
        s.iter().enumerate().all(|(k, &c)| rd8(p + k) == c) && rd8(p + s.len()) == 0
    }
    /// Offset of `s` as a whole string in the strings block, if present.
    fn find_string(strings: usize, len: usize, s: &[u8]) -> Option<u32> {
        let mut off = 0usize;
        while off < len {
            if name_eq(strings + off, s) {
                return Some(off as u32);
            }
            while off < len && rd8(strings + off) != 0 {
                off += 1;
            }
            off += 1;
        }
        None
    }

    /// Returns the new tree's total size, or None when the tree is not what this expects (then nothing has been written that matters).
    pub fn patch(src: usize, dst: usize, dst_max: usize) -> Option<usize> {
        if rd32(src) != MAGIC {
            return None;
        }
        let total = rd32(src + 4) as usize;
        let off_struct = rd32(src + 8) as usize;
        let off_strings = rd32(src + 12) as usize;
        let size_strings = rd32(src + 32) as usize;
        let size_struct = rd32(src + 36) as usize;
        // The usual layout, which the rebuild below assumes: header, memory reservation map, structure, strings, nothing after.
        if off_struct + size_struct > off_strings || off_strings + size_strings > total {
            return None;
        }
        let reg_off = find_string(src + off_strings, size_strings, b"reg")?;
        let nomap_off = find_string(src + off_strings, size_strings, b"no-map")?;
        // Walk the structure block to the END_NODE that closes /reserved-memory.
        let mut p = src + off_struct;
        let end = p + size_struct;
        let mut depth = 0u32;
        let mut target_depth: Option<u32> = None;
        let mut insert_at: Option<usize> = None;
        while p < end {
            let tok = rd32(p);
            p += 4;
            match tok {
                BEGIN_NODE => {
                    let name = p;
                    let mut n = 0usize;
                    while rd8(p + n) != 0 {
                        n += 1;
                    }
                    p = (p + n + 1 + 3) & !3;
                    depth += 1;
                    if depth == 2 && target_depth.is_none() && name_eq(name, b"reserved-memory") {
                        target_depth = Some(depth);
                    }
                }
                END_NODE => {
                    if target_depth == Some(depth) {
                        insert_at = Some(p - 4);
                        break;
                    }
                    depth -= 1;
                }
                PROP => {
                    let len = rd32(p) as usize;
                    p = (p + 8 + len + 3) & !3;
                }
                NOP => {}
                END => break,
                _ => return None,
            }
        }
        let insert_at = insert_at?;
        // The node: BEGIN_NODE "hv@afc000000" (13 bytes + pad to 16), PROP reg (len 16, 4 cells), PROP no-map (len 0), END_NODE.
        let node_len = 4 + 16 + 12 + 16 + 12 + 4;
        let new_total = total + node_len;
        if new_total > dst_max {
            return None;
        }
        // Everything up to the insertion point, byte for byte (header fixed up after).
        let head = insert_at - src;
        copy(dst, src, head);
        let mut q = dst + head;
        wr32(q, BEGIN_NODE);
        q += 4;
        for (k, &c) in b"hv@afc000000\0\0\0\0".iter().enumerate() {
            wr8(q + k, c);
        }
        q += 16;
        wr32(q, PROP);
        wr32(q + 4, 16);
        wr32(q + 8, reg_off);
        wr32(q + 12, (super::HV_BASE >> 32) as u32);
        wr32(q + 16, super::HV_BASE as u32);
        wr32(q + 20, (super::HV_SIZE >> 32) as u32);
        wr32(q + 24, super::HV_SIZE as u32);
        q += 28;
        wr32(q, PROP);
        wr32(q + 4, 0);
        wr32(q + 8, nomap_off);
        q += 12;
        wr32(q, END_NODE);
        q += 4;
        // The rest of the structure block and the strings block that follows it.
        copy(q, insert_at, total - head);
        wr32(dst + 4, new_total as u32);
        wr32(dst + 12, (off_strings + node_len) as u32);
        wr32(dst + 36, (size_struct + node_len) as u32);
        Some(new_total)
    }
}
