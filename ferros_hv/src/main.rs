//! ferros hv — the thin EL2 layer under Android on husky (HV.md).
//!
//! Milestone 0, what this file does: ABL loads `boot_a`'s "kernel", which is this binary padded to G#18_0000 with the stock Linux Image appended, at G#8008_0000 and jumps here at EL2 with the MMU off and the DTB in x0. The hv copies itself to 64 MiB of the low DRAM bank (HV_BASE), zeroes its bss, copies the DTB next to itself with one node added (`/reserved-memory/hv@b2000000`, no-map, so Linux never allocates over it), programs EL2 so that EL1 sees a plain machine (timers, GIC system registers, pointer authentication, SVE, PMU, MTE all through; only SMC traps), and erets into the Linux Image at G#8020_0000 as EL1h.
//! From then on the hv runs only when Linux issues an SMC: PSCI calls that name a resume address (CPU_ON, CPU_SUSPEND, SYSTEM_SUSPEND) get that address swapped for `_secondary`, which brings the new or woken core up through the same EL2 setup and erets to where Linux wanted it; every other SMC is forwarded to EL3 unchanged.
//! Milestone 0: Android boots, runs and sleeps under a dormant hv (stage 2 off). Milestone 1 adds stage 2 behind the `hv_stage2` knob — a 40-bit IPA identity map, HCR_EL2.VM = 1 — proven transparent on hardware 2026-10-19; the register oracle (`hv_oracle`) unmaps a target's pages to trap and log them, proven on the touch SPI bus 2026-10-19; the UFS command firewall and the EL2 kill chord build on the same trap next.
//! Crumbs go to the ramoops result block at G#FD60_0000 from offset G#40 (ferros's own words are below it), where the next ferros boot reads them: +G#40 stage, +G#48 SMC traps, +G#50 CPU_ON count, +G#58 ESR of the last unexpected trap, +G#60 its ELR, +G#68 its FAR, +G#70 EL2 setup sub-stage, +G#78 ACTLR_EL2 as found, +G#80 trapped system-register accesses, +G#100 + core*16 the first such trap per core (ESR, ELR).

#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

use core::arch::{asm, global_asm};
use core::ptr::{read_volatile, write_volatile};

/// 64 MiB in the LOW DRAM bank, just under the secure carveout at G#B620_0000 and clear of where ABL stages the boot images (G#A000_0000..G#AA00_0000) and keeps its heap (G#F8B3_C000). It was the last 64 MiB of DRAM (G#A_FC00_0000) first: the high bank reads as decayed cells after every reset, and Android under the hv lost cores to silent hangs after a minute of idling with nothing in the hv's log — consistent with that bank, or the hv's part of it, not being kept refreshed under deep idle either (2026-10-17).
const HV_BASE: u64 = 0xA_FC00_0000;
const HV_SIZE: u64 = 0x400_0000;
/// Where the patched DTB goes: the megabyte below the reserved region, ordinary RAM that Linux reserves itself once it has read the tree from there.
const DTB_COPY: u64 = HV_BASE - 0x10_0000;
const DTB_COPY_MAX: usize = 0x10_0000;
/// The Linux Image: right after the hv's pad in the blob ABL loaded, 2 MiB aligned as its header demands (its flags say "place anywhere").
const LINUX_ENTRY: u64 = 0x8020_0000;
/// The hv's report page: the last 64 KiB of its own reserved region. It sat at G#B610_0000 for a day, a page right under the secure DRAM carveout at G#B620_0000 — and the freezes under real use resolved into an asynchronous SError (bus error, syndrome G#BE000011) delivered to whichever core trapped next, which is what a write into a protected window looks like when the hv writes this page on every trap (2026-10-19). Nothing needs this page to survive a reset any more: the hv's words go into Android's pstore ring.
const REPORT: usize = 0xA_FFFF_0000;
const REPORT_RESERVE: u64 = 0x10_0000;

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
    // Band in splash slot 3 (rows 288..384, grey A0) before anything else: ABL jumped into the hv. PC-relative, no stack, no memory of ours.
    mov     x9, #3
    movz    x10, #0xA0A0
    movk    x10, #0xFFA0, lsl #16
    bl      asm_band
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
    // Band in slot 2 (rows 192..288, grey 60): the copy landed and the jump into it worked.
    mov     x9, #2
    movz    x10, #0x6060
    movk    x10, #0xFF60, lsl #16
    bl      asm_band
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
    sub     sp, sp, #0x100
    stp     x0, x1, [sp, #0x00]
    stp     x2, x3, [sp, #0x10]
    stp     x4, x5, [sp, #0x20]
    stp     x6, x7, [sp, #0x30]
    stp     x8, x9, [sp, #0x40]
    stp     x10, x11, [sp, #0x50]
    stp     x12, x13, [sp, #0x60]
    stp     x14, x15, [sp, #0x70]
    stp     x16, x17, [sp, #0x80]
    stp     x18, x19, [sp, #0x90]
    stp     x20, x21, [sp, #0xA0]
    stp     x22, x23, [sp, #0xB0]
    stp     x24, x25, [sp, #0xC0]
    stp     x26, x27, [sp, #0xD0]
    stp     x28, x29, [sp, #0xE0]
    str     x30, [sp, #0xF0]
    mov     x0, sp
    bl      hv_sync                     // frame index = register number; may rewrite any of x0..x30
    ldp     x0, x1, [sp, #0x00]
    ldp     x2, x3, [sp, #0x10]
    ldp     x4, x5, [sp, #0x20]
    ldp     x6, x7, [sp, #0x30]
    ldp     x8, x9, [sp, #0x40]
    ldp     x10, x11, [sp, #0x50]
    ldp     x12, x13, [sp, #0x60]
    ldp     x14, x15, [sp, #0x70]
    ldp     x16, x17, [sp, #0x80]
    ldp     x18, x19, [sp, #0x90]
    ldp     x20, x21, [sp, #0xA0]
    ldp     x22, x23, [sp, #0xB0]
    ldp     x24, x25, [sp, #0xC0]
    ldp     x26, x27, [sp, #0xD0]
    ldp     x28, x29, [sp, #0xE0]
    ldr     x30, [sp, #0xF0]
    add     sp, sp, #0x100
    eret

unexpected:
    mrs     x0, esr_el2
    mrs     x1, elr_el2
    mrs     x2, far_el2
    bl      hv_unexpected
6:  wfe
    b       6b

// Leaf, no stack: paint 88 rows of colour x10 across the layer DMA0 scans, at slot x9 (96 rows each). Clobbers x11-x16. Same registers and guards as splash::band.
.global asm_band
asm_band:
    movz    x11, #0x1990, lsl #16       // DMA0
    ldr     w12, [x11, #0x40]           // base (physical: translation off)
    ldr     w13, [x11, #0x10]           // stride, pixels
    ldr     w14, [x11, #0x1C]           // height:16 | width:16
    and     x15, x14, #0xFFFF           // width
    lsr     x14, x14, #16               // height
    lsr     x16, x12, #31               // base below G#8000_0000 is not DRAM
    cbz     x16, 9f
    cbz     x13, 9f
    cmp     x13, #8192
    b.hi    9f
    cbz     x15, 9f
    add     x16, x9, #1
    lsl     x16, x16, #5
    add     x16, x16, x16, lsl #1       // (slot+1)*96
    cmp     x14, x16
    b.lo    9f
    lsl     x16, x9, #5
    add     x16, x16, x16, lsl #1       // slot*96 = first row
    lsl     x13, x13, #2                // stride in bytes
    madd    x12, x16, x13, x12          // row address
    mov     x14, #88
7:  mov     x16, x15                    // x pixels left
    mov     x11, x12
8:  str     w10, [x11], #4
    subs    x16, x16, #1
    b.ne    8b
    add     x12, x12, x13
    subs    x14, x14, #1
    b.ne    7b
    dsb     sy
9:  ret

// Leaf: x1 = MPIDR → x0 = index into the per-core tables (Aff1 is the core number on this SoC: cluster-level affinity sits in Aff2 and the nine cores are numbered 0-8 in Aff1).
.global core_index
core_index:
    ubfx    x0, x1, #8, #4
    ret
"#
);

fn report(off: usize, v: u64) {
    if cfg!(hv_no_report) && off >= 0x48 {
        return;
    }
    unsafe {
        write_volatile((REPORT + off) as *mut u64, v);
        asm!("dc civac, {a}", "dsb sy", a = in(reg) (REPORT + off) as u64);
    }
}

fn stage(s: u64) {
    report(0x40, s);
    // Slot 0 of the splash: a grey that climbs with the stage (A1 dark … A4 white); errors in slot 1 (magenta: DTB rebuild failed; green: an unexpected trap parked a core; yellow: panic).
    match s {
        STAGE_ENTERED => splash::band(0, 0xFF40_4040),
        STAGE_DTB_DONE => splash::band(0, 0xFF80_8080),
        STAGE_EL2_DONE => splash::band(0, 0xFFC0_C0C0),
        STAGE_ERET => splash::band(0, 0xFFFF_FFFF),
        STAGE_DTB_FAILED => splash::band(1, 0xFFFF_00FF),
        STAGE_UNEXPECTED_TRAP => splash::band(1, 0xFF00_FF00),
        STAGE_PANIC => splash::band(1, 0xFFFF_FF00),
        _ => {}
    }
}

fn bump(off: usize) {
    if cfg!(hv_no_report) && off != 0x50 {
        return;
    }
    unsafe {
        let p = (REPORT + off) as *mut u64;
        write_volatile(p, read_volatile(p).wrapping_add(1));
    }
}

/// Stage 2, identity: a 40-bit IPA space in 1 GiB blocks (level 1, two concatenated 4 KiB tables = 1024 entries), every block Normal write-back inner-shareable, read/write, access flag set; stage 1's attributes win for Device memory under the combining rules, so MMIO stays MMIO. What pKVM gives its host; the oracle and the M1 firewall unmap a block's pages from here so every access traps.
#[repr(C, align(8192))]
struct Stage2([u64; 1024]);
static mut STAGE2: Stage2 = Stage2([0; 1024]);

/// Descriptor low bits. Block (L1/L2): [1:0] = 0b01. Page (L3): [1:0] = 0b11. Both carry MemAttr[5:2] = 0b1111 (Normal WB in/out), S2AP[7:6] = 0b11 (RW), SH[9:8] = 0b11 (inner), AF[10] = 1. A table pointer is next-level-address | 0b11.
const S2_ATTR: u64 = (0b1111 << 2) | (0b11 << 6) | (0b11 << 8) | (1 << 10);
const S2_BLOCK: u64 = 0b01 | S2_ATTR;
const S2_PAGE: u64 = 0b11 | S2_ATTR;
const S2_ADDR: u64 = 0x0000_FFFF_FFFF_F000;

/// A next-level page table (512 × 8 bytes, 4 KiB, naturally aligned). Pools for splitting blocks down to pages: eight 1 GiB blocks and 64 2 MiB blocks can be refined; a target list that needs more stops at the pool's end (stage2_unmap refuses rather than overrun).
#[repr(C, align(4096))]
struct Tbl([u64; 512]);
static mut L2POOL: [Tbl; 8] = [const { Tbl([0; 512]) }; 8];
static mut L3POOL: [Tbl; 64] = [const { Tbl([0; 512]) }; 64];
static mut L2N: usize = 0;
static mut L3N: usize = 0;
static mut S2_READY: bool = false;

// The MMIO pages the oracle unmaps so their accesses trap and are logged, from FERROS_HV_ORACLE_PAGES at build time (build.rs); the default is the touch SPI controller (spi@111D0000).
include!(concat!(env!("OUT_DIR"), "/oracle_targets.rs"));

/// Next free table from a pool, or null when it is used up (the caller leaves that target mapped).
unsafe fn take_l2() -> *mut u64 {
    let n = read_volatile(&raw const L2N);
    if n >= (*(&raw const L2POOL)).len() {
        return core::ptr::null_mut();
    }
    write_volatile(&raw mut L2N, n + 1);
    (&raw mut L2POOL[n]) as *mut u64
}
unsafe fn take_l3() -> *mut u64 {
    let n = read_volatile(&raw const L3N);
    if n >= (*(&raw const L3POOL)).len() {
        return core::ptr::null_mut();
    }
    write_volatile(&raw mut L3N, n + 1);
    (&raw mut L3POOL[n]) as *mut u64
}

/// Refine the map down to the 4 KiB page holding `ipa` and mark it invalid, so an EL1&0 access to it faults to EL2 (EC G#24). Splits the covering 1 GiB block into 2 MiB blocks and the covering 2 MiB block into 4 KiB pages, both identity, on the way down. Called once, on the primary, before any secondary starts.
unsafe fn stage2_unmap(ipa: u64) {
    let s2 = (&raw mut STAGE2) as *mut u64;
    let l1i = (ipa >> 30) as usize;
    let e1 = read_volatile(s2.add(l1i));
    let l2 = if e1 & 0b11 == 0b11 {
        (e1 & S2_ADDR) as *mut u64
    } else {
        let t = take_l2();
        if t.is_null() {
            return;
        }
        for j in 0..512u64 {
            write_volatile(t.add(j as usize), (((l1i as u64) << 30) | (j << 21)) | S2_BLOCK);
        }
        write_volatile(s2.add(l1i), (t as u64 & S2_ADDR) | 0b11);
        t
    };
    let l2i = ((ipa >> 21) & 0x1FF) as usize;
    let e2 = read_volatile(l2.add(l2i));
    let l3 = if e2 & 0b11 == 0b11 {
        (e2 & S2_ADDR) as *mut u64
    } else {
        let t = take_l3();
        if t.is_null() {
            return;
        }
        let base2 = (ipa >> 21) << 21;
        for k in 0..512u64 {
            write_volatile(t.add(k as usize), (base2 | (k << 12)) | S2_PAGE);
        }
        write_volatile(l2.add(l2i), (t as u64 & S2_ADDR) | 0b11);
        t
    };
    let l3i = ((ipa >> 12) & 0x1FF) as usize;
    write_volatile(l3.add(l3i), 0);
}

/// Build the shared stage-2 table once (the primary boots and enables it before any secondary is started, so the flag needs no lock), unmap the oracle's targets into it, and return its base for VTTBR_EL2. Secondaries call this too and get the finished table.
unsafe fn stage2_base() -> u64 {
    let s2 = (&raw mut STAGE2) as *mut u64;
    if !read_volatile(&raw const S2_READY) {
        for i in 0..1024u64 {
            write_volatile(s2.add(i as usize), (i << 30) | S2_BLOCK);
        }
        if cfg!(hv_oracle) {
            let mut i = 0;
            while i < ORACLE_TARGETS.len() {
                stage2_unmap(ORACLE_TARGETS[i]);
                i += 1;
            }
        }
        asm!("dsb sy", "tlbi alle1", "dsb sy", "isb");
        write_volatile(&raw mut S2_READY, true);
    }
    s2 as u64
}

/// One compact line per trapped oracle access, capped so a touch burst cannot flood the ring. Read from the next boot's /sys/fs/pstore/console-ramoops-0, the way every other hv line is.
fn oracle_log(pa: u64, write: bool, bytes: u8, val: u64) {
    if unsafe { read_volatile((REPORT + 0xB0) as *const u64) } > 2048 {
        return;
    }
    let (mut hp, mut hv) = ([0u8; 16], [0u8; 16]);
    pstore::log(&[
        b"\nferros hv: S2 ",
        pstore::hex(pa, &mut hp),
        if write { b" W" } else { b" R" },
        &[b'0' + bytes],
        b" ",
        pstore::hex(val, &mut hv),
        b"\n",
    ]);
}

/// Where each core should go after the hv brings it up: the entry and context Linux passed to PSCI, by core index.
static mut RESUME: [[u64; 2]; 16] = [[0; 2]; 16];
/// Per-core counts: resumes through `_secondary`, SMCs trapped. Written by one core each, read for the log.
static mut RESUMES: [u64; 16] = [0; 16];
static mut SMCS: [u64; 16] = [0; 16];
/// The distinct SMC function IDs seen (first occurrence logged with its arguments), 32 slots.
static mut FIDS: [u64; 32] = [0; 32];
/// Per core: the last trapped SMC's function ID and the counter value at the last trap; the counter value of the last stats line (core-shared, racy, fine).
static mut LAST_FID: [u64; 16] = [0; 16];
static mut LAST_T: [u64; 16] = [0; 16];
static mut STATS_T: u64 = 0;

/// Once a second, from whichever core traps next: a line per core into the pstore ring — traps, resumes, last SMC, milliseconds since its last trap. The timeline a freeze leaves behind: the core that falls silent first and its last words.
fn stats_tick(now: u64, freq: u64) {
    unsafe {
        let last = read_volatile(&raw const STATS_T);
        if now.wrapping_sub(last) < freq {
            return;
        }
        write_volatile(&raw mut STATS_T, now);
        let mut line = [0u8; 9 * 44];
        for c in 0..9 {
            let (mut h1, mut h2, mut h3) = ([0u8; 16], [0u8; 16], [0u8; 16]);
            let smcs = read_volatile(&raw const SMCS[c]);
            let res = read_volatile(&raw const RESUMES[c]);
            let fid = read_volatile(&raw const LAST_FID[c]);
            let lt = read_volatile(&raw const LAST_T[c]);
            let ago_ms = if lt == 0 { 0xFFFF_FFFF } else { now.wrapping_sub(lt) / (freq / 1000) };
            pstore::hex((smcs << 32) | (res & 0xFFFF_FFFF), &mut h1);
            pstore::hex(fid, &mut h2);
            pstore::hex(ago_ms, &mut h3);
            let o = c * 44;
            line[o] = b' ';
            line[o + 1] = b'0' + c as u8;
            line[o + 2] = b':';
            line[o + 3..o + 19].copy_from_slice(&h1);
            line[o + 19] = b'/';
            line[o + 20..o + 28].copy_from_slice(&h2[8..16]);
            line[o + 28] = b'/';
            line[o + 29..o + 37].copy_from_slice(&h3[8..16]);
            // The window column went with the native-SMC window (removed 2026-10-19): the stride stays 44 bytes and these trailing bytes are a fixed marker, so the per-core record and the silent-core detector (ms-since-last-trap, above) are unchanged.
            line[o + 37] = b'-';
            line[o + 38..o + 44].copy_from_slice(b"------");
        }
        let mut hn = [0u8; 16];
        pstore::hex(now / (freq / 1000), &mut hn);
        // The display's two S2MPUs: CTRL0, the per-VID protection enable as it reads back, and FAULT_STATUS — a denied DMA in stall mode would hold DECON's frame forever, which is what the compositor's unreleased retire fence looked like (2026-10-17 21:20). Read only when this core's last SMC was a DQE histogram call (the display driver is touching the DQE, so the domain is on): a read into a powered-down block hangs the core, and the first version of this, reading on every tick, did exactly that during early boot and burned slot a's retries (21:27–22:17).
        let mut s2 = [0u8; 3 * 40];
        // The display domain is on when any core issued a DQE histogram call within the last second; the GPU domain is on when the PMU's G3D_STATUS (read through EL3, as ferros's kernel reads the PMU) says so — the GPU's S2MPU (G3D) is then read too: Mahere's first frames stop the whole SoC with nothing in the kernel log, the shape of a DMA master stalled by its S2MPU (2026-10-19).
        // No S2MPU readback at all any more: the display domain is off more often than the histogram gate assumed (the display driver keeps issuing those calls and EL3 answers error 3 while the domain is down), and a read into the powered-down block came back as an asynchronous SError on whichever core trapped next — the bootloader decoded it as "BOOKER CPU8 to DPUF0" (2026-10-19). Every SError-class freeze dates from the build that added the readback.
        let dpu_on = false;
        let mut g3d_status: u64 = 0;
        if !cfg!(hv_no_pmu_read) {
        asm!("smc #0", inout("x0") 0x8200_0504u64 => g3d_status, inout("x1") 0x1546_1A84u64 => _, inout("x2") 0u64 => _, inout("x3") 0u64 => _,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _, out("x10") _, out("x11") _, out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x16") _, out("x17") _);
        }
        let g3d_on = g3d_status & 1 == 1 && g3d_status < 0x1_0000;
        for (k, base) in [0x1987_0000usize, 0x19C7_0000, 0x1EE7_0000].iter().enumerate() {
            let on = if k < 2 { dpu_on } else { g3d_on };
            if !on {
                let o = k * 40;
                s2[o..o + 40].copy_from_slice(if k < 2 { b" dpu?  --------/--------/--------       " } else { b" g3d?  --------/--------/--------       " });
                continue;
            }
            let (mut a, mut b, mut c) = ([0u8; 16], [0u8; 16], [0u8; 16]);
            pstore::hex(read_volatile(*base as *const u32) as u64, &mut a);
            pstore::hex(read_volatile((*base + 0x50) as *const u32) as u64, &mut b);
            pstore::hex(read_volatile((*base + 0x2000) as *const u32) as u64, &mut c);
            let o = k * 40;
            s2[o..o + 6].copy_from_slice(match k { 0 => b" dpuf0", 1 => b" dpuf1", _ => b" g3d  " });
            s2[o + 6] = b' ';
            s2[o + 7..o + 15].copy_from_slice(&a[8..16]);
            s2[o + 15] = b'/';
            s2[o + 16..o + 24].copy_from_slice(&b[8..16]);
            s2[o + 24] = b'/';
            s2[o + 25..o + 33].copy_from_slice(&c[8..16]);
            s2[o + 33..o + 40].copy_from_slice(b"       ");
        }
        pstore::log(&[b"\nferros hv stats t=", &hn[6..16], b" ms;", &line, b" s2mpu", &s2, b"\n"]);
    }
}

fn me_core() -> u64 {
    let m: u64;
    unsafe { asm!("mrs {v}, mpidr_el1", v = out(reg) m) };
    m >> 8
}


/// EL2 as the guest should find it. Called on every core before its first eret.
unsafe fn configure_el2() {
    // The hv's own instruction fetches are cacheable with SCTLR_EL2.I set even with its MMU off (data stays Device); without it every trap executes a few hundred instructions straight from DRAM, tens of microseconds with interrupts masked under GPU memory traffic.
    let sctlr2: u64;
    asm!("mrs {v}, sctlr_el2", v = out(reg) sctlr2);
    asm!("msr sctlr_el2, {v}", "ic iallu", "dsb sy", "isb", v = in(reg) sctlr2 | (1 << 12));
    // Vectors first, so anything below that traps lands in `unexpected` with its ESR instead of in whatever VBAR_EL2 held. Sub-stage crumbs at +G#70 name the register being written (the first flash, 2026-10-17, stopped between A2 and A3 with no vector installed).
    let vbar = hv_vectors as *const () as u64;
    asm!("msr vbar_el2, {v}", "isb", v = in(reg) vbar);
    report(0x70, 0xB0);
    // RW (EL1 is AArch64) | TSC (SMC traps here) | APK + API (pointer authentication keys and instructions untrapped) | ATA (MTE tag accesses untrapped where implemented). VM = 0: no stage 2 yet. IMO/FMO/AMO = 0: interrupts go to EL1 directly. E2H = 0, TGE = 0.
    // (Bits 52..55 are TOCU / EnSCXT / TTLBIS / TTLBOS — setting them "for PBHA" on 2026-10-19 trapped the kernel's TLB invalidations into the skip path and slot a stopped booting. The page-table hardware-use bits are gated elsewhere; left alone.)
    let hcr: u64 = (1 << 31) | (1 << 19) | (1 << 40) | (1 << 41) | if cfg!(hv_no_ata) { 0 } else { 1 << 56 };
    // The ISB matters: without it the CPTR_EL2 write below used the E2H = 1 register layout the core was still in (measured 2026-10-17: ZEN read as 0, the ZCR_EL2 write trapped with EC G#19).
    asm!("msr hcr_el2, {h}", "isb", h = in(reg) hcr);
    report(0x70, 0xB1);
    // CPTR_EL2 (E2H = 0 layout): RES1 bits with TFP (10) and TZ (8) clear, so FP/SIMD and SVE run at EL1 untrapped; TAM (30) clear for the activity monitors.
    asm!("msr cptr_el2, {v}", "isb", v = in(reg) 0x32FFu64);
    report(0x70, 0xB2);
    // SVE present: let EL1 use the full vector length.
    let pfr0: u64;
    asm!("mrs {v}, id_aa64pfr0_el1", v = out(reg) pfr0);
    if (pfr0 >> 32) & 0xF != 0 {
        asm!("msr s3_4_c1_c2_0, {v}", v = in(reg) 0xFu64); // ZCR_EL2
    }
    report(0x70, 0xB3);
    // MDCR_EL2: every PMU counter visible to EL1 (HPMN = PMCR_EL0.N), the SPE buffer (E2PB) and trace buffer (E2TB) owned by EL1, no debug traps.
    let pmcr: u64;
    asm!("mrs {v}, pmcr_el0", v = out(reg) pmcr);
    let mdcr: u64 = ((pmcr >> 11) & 0x1F) | (3 << 12) | (3 << 24);
    asm!("msr mdcr_el2, {v}", v = in(reg) mdcr);
    report(0x70, 0xB4);
    // Timers: EL1 reads the physical counter and owns the physical timer; no virtual offset.
    asm!("msr cnthctl_el2, {v}", v = in(reg) 3u64);
    asm!("msr cntvoff_el2, {v}", v = in(reg) 0u64);
    report(0x70, 0xB5);
    // GICv3: system-register interface at EL2, and EL1 allowed to enable its own (ICC_SRE_EL2.Enable); no virtual CPU interface.
    // ICC_SRE_EL2 as Linux's init_el2 writes it: SRE | Enable, the bypass-disable bits left clear (an earlier 0xF set DIB/DFB too).
    asm!("msr s3_4_c12_c9_5, {v}", v = in(reg) 0x9u64); // ICC_SRE_EL2
    asm!("isb");
    // The EL2 physical timer is UNKNOWN at reset and after every power-down; nothing at EL2 uses it, so it is off (a kernel at EL2 would own it).
    asm!("msr cnthp_ctl_el2, xzr", "isb");
    report(0x70, 0xB6);
    asm!("msr s3_4_c12_c11_0, {v}", v = in(reg) 0u64); // ICH_HCR_EL2
    report(0x70, 0xB7);
    // The guest sees the real core.
    let midr: u64;
    let mpidr: u64;
    asm!("mrs {a}, midr_el1", "mrs {b}, mpidr_el1", a = out(reg) midr, b = out(reg) mpidr);
    asm!("msr vpidr_el2, {a}", "msr vmpidr_el2, {b}", a = in(reg) midr, b = in(reg) mpidr);
    asm!("msr hstr_el2, {v}", v = in(reg) 0u64);
    // What Linux's own init_el2 zeroes and the hv did not (2026-10-17): registers that are UNKNOWN at reset and again after every core power-down, each able to change EL1's behaviour per core — fine-grained traps (HFGRTR/HFGWTR/HFGITR/HDFGRTR/HDFGWTR/HAFGRTR_EL2), HCRX_EL2 (FEAT_HCX: instruction enables and extra traps), MPAM2_EL2 and MPAMHCR_EL2 (partition IDs and MPAM traps), PMSCR_EL2 (statistical profiling at EL2), TRFCR_EL2 (trace filter). Guarded by the ID registers.
    let (mmfr0, mmfr1, pfr0, dfr0): (u64, u64, u64, u64);
    asm!("mrs {a}, id_aa64mmfr0_el1", "mrs {b}, id_aa64mmfr1_el1", "mrs {c}, id_aa64pfr0_el1", "mrs {d}, id_aa64dfr0_el1", a = out(reg) mmfr0, b = out(reg) mmfr1, c = out(reg) pfr0, d = out(reg) dfr0);
    if (mmfr0 >> 56) & 0xF != 0 {
        asm!("msr s3_4_c1_c1_4, xzr", "msr s3_4_c1_c1_5, xzr", "msr s3_4_c1_c1_6, xzr", "msr s3_4_c3_c1_4, xzr", "msr s3_4_c3_c1_5, xzr");
        if (pfr0 >> 44) & 0xF != 0 {
            asm!("msr s3_4_c3_c1_6, xzr"); // HAFGRTR_EL2, AMU present
        }
    }
    if (mmfr1 >> 40) & 0xF != 0 {
        asm!("msr s3_4_c1_c2_2, xzr"); // HCRX_EL2
    }
    if (pfr0 >> 40) & 0xF != 0 {
        asm!("msr s3_4_c10_c5_0, xzr", "msr s3_4_c10_c4_0, xzr"); // MPAM2_EL2, MPAMHCR_EL2
    }
    if (dfr0 >> 32) & 0xF != 0 {
        asm!("msr s3_4_c9_c9_0, xzr"); // PMSCR_EL2
    }
    if (dfr0 >> 40) & 0xF != 0 {
        asm!("msr s3_4_c1_c2_1, xzr"); // TRFCR_EL2
    }
    asm!("isb");
    report(0x70, 0xBA);
    // The VMID. With EL2 present, every EL1&0 TLB entry is tagged with VTTBR_EL2.VMID even while stage 2 is off, and VTTBR_EL2 is UNKNOWN at reset — a different value per core, and again after every power-down. Linux's broadcast TLB invalidations (VMALLE1IS and friends) only reach entries with the issuing core's VMID, so without one fixed VMID the other cores keep stale translations: the load-dependent, minutes-later hangs of 2026-10-17, worst under GPU mapping churn. VMID 0 everywhere, and the EL1 TLB flushed on the way in.
    // The flush is local: this core's EL1 TLB is empty on a first boot and after a power-down anyway, and a broadcast (ALLE1IS) on every resume of nine cores would be a DVM storm.
    // VTCR_EL2: T0SZ 24 (40-bit IPA), SL0 1 (start at level 1), IRGN/ORGN write-back, SH inner, TG0 4 KiB, PS 40-bit, RES1 bit 31.
    let vttbr = if cfg!(hv_stage2) { stage2_base() } else { 0 };
    asm!("msr vttbr_el2, {v}", "msr vtcr_el2, {t}", "isb", "tlbi alle1", "dsb nsh", "isb", v = in(reg) vttbr, t = in(reg) 0x8000_0000u64 | (24 << 0) | (1 << 6) | (3 << 8) | (3 << 10) | (3 << 12) | (2 << 16));
    if cfg!(hv_stage2) {
        let hcr: u64;
        asm!("mrs {h}, hcr_el2", h = out(reg) hcr);
        asm!("msr hcr_el2, {h}", "isb", h = in(reg) hcr | 1); // VM
    }
    report(0x70, 0xB8);
    // ACTLR_EL2 gates EL1 access to the core's implementation-defined registers (CPUECTLR_EL1, CPUACTLR_EL1, CPUPWRCTLR_EL1 and friends on Cortex-A/X cores); a kernel that ran at EL2 never met the gate. All permission bits on (RES0 bits ignore the write); the value found is kept at +G#78 for the record.
    let actlr: u64;
    asm!("mrs {v}, actlr_el2", v = out(reg) actlr);
    report(0x78, actlr);
    if !cfg!(hv_actlr_keep) {
        asm!("msr actlr_el2, {v}", "isb", v = in(reg) 0xFFFF_FFFFu64);
    }
    // EL1 starts with its MMU off, little-endian, as a kernel entry requires.
    // SCTLR_EL1 as Linux's init_el2 hands it to EL1 (INIT_SCTLR_EL1_MMU_OFF): RES1 plus the pointer-authentication enables EnIA/EnIB/EnDA/EnDB.
    asm!("msr sctlr_el1, {v}", v = in(reg) 0x30D0_0800u64 | (1 << 31) | (1 << 30) | (1 << 27) | (1 << 13));
    asm!("isb");
    report(0x70, 0xB9);
}

unsafe extern "C" {
    fn hv_vectors();
    fn _secondary();
}

/// EL1h with every interrupt masked, as a kernel expects at entry.
const SPSR_EL1H_MASKED: u64 = 0x3C5;

unsafe fn eret_to(entry: u64, x0: u64, paint: bool) -> ! {
    if paint {
        stage(STAGE_ERET);
    }
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
        write_volatile(REPORT as *mut u64, 0x4645_5252_4F53_5F48); // "FERROS_H"
    }
    stage(STAGE_ENTERED);
    chord::arm();
    chord::early_to_fastboot_if_held();
    report(0x48, 0);
    report(0x50, 0);
    report(0x80, 0);
    report(0x88, 0);
    report(0xA0, 0);
    for core in 0..16 {
        report(0x100 + core * 16, 0);
        report(0x108 + core * 16, 0);
        report(0x200 + core * 8, 0);
    }
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
        if cfg!(hv_linux_at_el2) {
            // The memory-map-only variant: Linux gets EL2 as from ABL (HCR_EL2 reset, SCTLR_EL2 off) and the hv is never entered again. Only the reservation and the DTB copy remain in effect.
            pstore::log(&[b"\nferros hv: handing Linux EL2 (memory-map test variant)\n"]);
            asm!("msr hcr_el2, {h}", "isb", h = in(reg) (1u64 << 31));
            stage(STAGE_ERET);
            asm!("msr elr_el2, {e}", "msr spsr_el2, {s}", "mov x1, xzr", "mov x2, xzr", "mov x3, xzr", "dsb sy", "isb", "eret",
                e = in(reg) LINUX_ENTRY, s = in(reg) 0x3C9u64, in("x0") new_dtb, options(noreturn));
        }
        configure_el2();
        stage(STAGE_EL2_DONE);
        let mut h = [0u8; 16];
        let actlr = read_volatile((REPORT + 0x78) as *const u64);
        pstore::log(&[b"\nferros hv: EL2 up, Linux at EL1; build ", env!("HV_BUILD_TAG").as_bytes(), b"; ACTLR_EL2 was ", pstore::hex(actlr, &mut h), b"\n"]);
        eret_to(LINUX_ENTRY, new_dtb, true);
    }
}

#[unsafe(no_mangle)]
extern "C" fn hv_secondary(ctx: u64) -> ! {
    let mpidr: u64;
    unsafe { asm!("mrs {v}, mpidr_el1", v = out(reg) mpidr) };
    let idx = ((mpidr >> 8) & 0xF) as usize;
    let (entry, saved_ctx) = unsafe { (RESUME[idx][0], RESUME[idx][1]) };
    if cfg!(hv_lean_resume) {
        // The wake path stripped to what EL1 needs: no chord poll, no counters, no log, no ACTLR write, no TLB flush — the bisection variant for the resume path (2026-10-17).
        unsafe {
            let vbar = hv_vectors as *const () as u64;
            asm!("msr vbar_el2, {v}", "isb", v = in(reg) vbar);
            asm!("msr hcr_el2, {h}", "isb", h = in(reg) (1u64 << 31) | (1 << 19) | (1 << 40) | (1 << 41) | (1 << 56));
            asm!("msr cptr_el2, {v}", "isb", v = in(reg) 0x32FFu64);
            let pmcr: u64;
            asm!("mrs {v}, pmcr_el0", v = out(reg) pmcr);
            asm!("msr mdcr_el2, {v}", v = in(reg) ((pmcr >> 11) & 0x1F) | (3 << 12) | (3 << 24));
            asm!("msr cnthctl_el2, {v}", "msr cntvoff_el2, xzr", v = in(reg) 3u64);
            asm!("msr s3_4_c12_c9_5, {v}", "isb", "msr s3_4_c12_c11_0, xzr", v = in(reg) 0xFu64);
            let (midr, mp): (u64, u64);
            asm!("mrs {a}, midr_el1", "mrs {b}, mpidr_el1", a = out(reg) midr, b = out(reg) mp);
            asm!("msr vpidr_el2, {a}", "msr vmpidr_el2, {b}", "msr hstr_el2, xzr", "msr vttbr_el2, xzr", a = in(reg) midr, b = in(reg) mp);
            asm!("msr sctlr_el1, {v}", "isb", v = in(reg) 0x30D0_0800u64);
            if entry == 0 {
                loop {
                    asm!("wfe");
                }
            }
            asm!("msr elr_el2, {e}", "msr spsr_el2, {s}", "mov x1, xzr", "mov x2, xzr", "mov x3, xzr", "dsb sy", "isb", "eret",
                e = in(reg) entry, s = in(reg) SPSR_EL1H_MASKED, in("x0") if ctx != 0 { ctx } else { saved_ctx }, options(noreturn));
        }
    }
    if chord::held() {
        chord::power_off();
    }
    unsafe {
        configure_el2();
        if entry == 0 {
            // A wake with nothing to resume to: EL3 started this core at the hv without a CPU_ON / CPU_SUSPEND having passed through (or the record was lost). Say so and park rather than eret to 0.
            let mut h = [0u8; 16];
            pstore::log(&[b"\nferros hv: core ", &[b'0' + idx as u8], b" woke with NO resume entry, ctx ", pstore::hex(ctx, &mut h), b" -- parked\n"]);
            stage(STAGE_UNEXPECTED_TRAP);
            loop {
                asm!("wfe");
            }
        }
        write_volatile(&raw mut RESUMES[idx], read_volatile(&raw const RESUMES[idx]) + 1);
        // First time through for this core after Linux is up: one line, so the log shows resume reaching the hv at all.
        let seen = (REPORT + 0x200 + idx * 8) as *mut u64;
        if read_volatile(seen) < 2 {
            write_volatile(seen, read_volatile(seen) + 1);
            let (mut h1, mut h2) = ([0u8; 16], [0u8; 16]);
            pstore::log(&[b"\nferros hv: core ", &[b'0' + idx as u8], b" up via hv, entry ", pstore::hex(entry, &mut h1), b" ctx ", pstore::hex(ctx, &mut h2), b"\n"]);
        }
        // CPU_ON hands the context in x0; a resume from CPU_SUSPEND / SYSTEM_SUSPEND does too. Prefer what EL3 gave us, fall back to the recorded one.
        eret_to(entry, if ctx != 0 { ctx } else { saved_ctx }, false);
    }
}

const PSCI_CPU_SUSPEND: u64 = 0xC400_0001;
const PSCI_CPU_ON: u64 = 0xC400_0003;
const PSCI_SYSTEM_SUSPEND: u64 = 0xC400_000E;
const PSCI_CPU_SUSPEND_32: u64 = 0x8400_0001;
/// Exynos EL3: save / restore a power domain's TrustZone state (TZPC group, S2MPU) around its power cycle (exynos-pd_el3.c).
const SMC_CMD_PREPARE_PD_ONOFF: u64 = 0x8200_0410;
const PSCI_CPU_ON_32: u64 = 0x8400_0003;

/// The trapped synchronous exception from EL1. `frame` holds x0..x17 (then x18, x30) as the guest left them; results go back through it.
#[unsafe(no_mangle)]
extern "C" fn hv_sync(frame: &mut [u64; 32]) {
    if chord::held() {
        chord::power_off();
    }
    let esr: u64;
    unsafe { asm!("mrs {v}, esr_el2", v = out(reg) esr) };
    let ec = (esr >> 26) & 0x3F;
    // The inert-hv experiment: with all secondaries up, this core stops trapping SMCs for good (Android must keep deep idle off: a power-down wake would land in Linux at EL2).
    if cfg!(hv_tsc_off_after_boot) && ec == 0x17 && unsafe { read_volatile((REPORT + 0x50) as *const u64) } >= 8 {
        unsafe {
            let hcr: u64;
            asm!("mrs {h}, hcr_el2", h = out(reg) hcr);
            asm!("msr hcr_el2, {h}", "isb", h = in(reg) hcr & !(1 << 19));
        }
        bump(0x98);
        // This SMC still has to happen: forward it as the trapped one it is.
        forward_smc(frame);
        unsafe {
            let elr: u64;
            asm!("mrs {v}, elr_el2", v = out(reg) elr);
            asm!("msr elr_el2, {v}", v = in(reg) elr + 4);
        }
        return;
    }
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
            let core = ((me >> 8) & 0xF) as usize;
            unsafe {
                write_volatile(&raw mut SMCS[core], read_volatile(&raw const SMCS[core]) + 1);
                let (now, freq): (u64, u64);
                asm!("mrs {a}, cntpct_el0", "mrs {b}, cntfrq_el0", a = out(reg) now, b = out(reg) freq);
                write_volatile(&raw mut LAST_FID[core], fid);
                write_volatile(&raw mut LAST_T[core], now);
                stats_tick(now, freq);
            }
            // Every trapped SMC is forwarded to EL3 from here and the guest resumed past it. The three PSCI calls that carry a re-entry point have it rewritten to `_secondary`; everything else passes through untouched. (The native-SMC re-execution window, breakpoint-stepped from EL1, was removed 2026-10-19: it existed to chase a caller-EL theory that the memory-corruption root cause disproved, and plain forwarding from EL2 booted Android before the window was ever written.)
            match fid {
                SMC_CMD_PREPARE_PD_ONOFF => {
                    bump(0x90);
                }
                PSCI_CPU_ON | PSCI_CPU_ON_32 => {
                    bump(0x50);
                    let idx = ((frame[1] >> 8) & 0xF) as usize;
                    unsafe {
                        RESUME[idx] = [frame[2], frame[3]];
                        asm!("dc civac, {a}", "dsb sy", a = in(reg) (&raw const RESUME[idx]) as u64);
                    }
                    frame[2] = _secondary as *const () as u64;
                }
                PSCI_CPU_SUSPEND | PSCI_CPU_SUSPEND_32 => {
                    unsafe {
                        RESUME[core] = [frame[2], frame[3]];
                        asm!("dc civac, {a}", "dsb sy", a = in(reg) (&raw const RESUME[core]) as u64);
                    }
                    frame[2] = _secondary as *const () as u64;
                }
                PSCI_SYSTEM_SUSPEND => {
                    unsafe {
                        RESUME[core] = [frame[1], frame[2]];
                        asm!("dc civac, {a}", "dsb sy", a = in(reg) (&raw const RESUME[core]) as u64);
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
        0x24 => {
            // Stage-2 data abort from EL1&0: an access to a page the oracle unmapped. The hv runs at EL2, which is not under stage 2, so it performs the real access against the device register, hands the guest the result, logs it, and steps past the instruction. Only the single-register syndrome form (ISV = 1, what readl/writel compile to) is emulated; a pair or other ISV = 0 access is logged and skipped (the driver may miss it, but the hv does not guess).
            let (far, hpfar): (u64, u64);
            unsafe { asm!("mrs {a}, far_el2", "mrs {b}, hpfar_el2", a = out(reg) far, b = out(reg) hpfar) };
            let pa = ((hpfar >> 4) << 12) | (far & 0xFFF); // identity map: IPA == PA
            let isv = (esr >> 24) & 1;
            let wnr = (esr >> 6) & 1;
            let sas = ((esr >> 22) & 3) as u32;
            let sse = (esr >> 21) & 1;
            let srt = ((esr >> 16) & 0x1F) as usize;
            let sf = (esr >> 15) & 1;
            bump(0xB0);
            if isv == 1 {
                let bytes = 1u8 << sas;
                let logval;
                unsafe {
                    if wnr == 1 {
                        let v = if srt == 31 { 0 } else { frame[srt] };
                        match sas {
                            0 => write_volatile(pa as *mut u8, v as u8),
                            1 => write_volatile(pa as *mut u16, v as u16),
                            2 => write_volatile(pa as *mut u32, v as u32),
                            _ => write_volatile(pa as *mut u64, v),
                        }
                        logval = v;
                    } else {
                        let raw = match sas {
                            0 => read_volatile(pa as *const u8) as u64,
                            1 => read_volatile(pa as *const u16) as u64,
                            2 => read_volatile(pa as *const u32) as u64,
                            _ => read_volatile(pa as *const u64),
                        };
                        let bits = 8u32 << sas;
                        let mut v = raw;
                        if sse == 1 && bits < 64 {
                            let sh = 64 - bits;
                            v = (((v << sh) as i64) >> sh) as u64;
                        }
                        if sf == 0 {
                            v &= 0xFFFF_FFFF;
                        }
                        if srt != 31 {
                            frame[srt] = v;
                        }
                        logval = v;
                    }
                }
                oracle_log(pa, wnr == 1, bytes, logval);
            } else {
                oracle_log(pa, wnr == 1, 0, esr);
            }
            unsafe {
                let elr: u64;
                asm!("mrs {v}, elr_el2", v = out(reg) elr);
                asm!("msr elr_el2, {v}", v = in(reg) elr + 4);
            }
        }
        0x16 => {
            // HVC from EL1: nothing at EL2 answers calls yet. SMCCC NOT_SUPPORTED; ELR already points past the HVC. The first eight are logged: a driver expecting pKVM would show up here.
            bump(0x88);
            unsafe {
                let n = read_volatile((REPORT + 0x88) as *const u64);
                if n <= 8 {
                    let (mut h0, mut h1) = ([0u8; 16], [0u8; 16]);
                    pstore::log(&[b"\nferros hv: HVC ", pstore::hex(frame[0], &mut h0), b" x1 ", pstore::hex(frame[1], &mut h1), b" -- NOT_SUPPORTED\n"]);
                }
            }
            frame[0] = u64::MAX;
        }
        0x18 => {
            // A system register access EL2 traps (one the gates above did not open). Recorded once per core (+G#100 + core*16: ESR, ELR) and counted (+G#80), then skipped: a write is dropped, a read returns 0. Milestone 0 survives it; the record names the register for the gate to open.
            let (me, elr): (u64, u64);
            unsafe { asm!("mrs {a}, mpidr_el1", "mrs {b}, elr_el2", a = out(reg) me, b = out(reg) elr) };
            let core = ((me >> 8) & 0xF) as usize;
            unsafe {
                let slot = (REPORT + 0x100 + core * 16) as *mut u64;
                if read_volatile(slot) == 0 {
                    write_volatile(slot, esr);
                    write_volatile(slot.add(1), elr);
                    asm!("dc civac, {a}", "dsb sy", a = in(reg) slot as u64);
                    let (mut h1, mut h2) = ([0u8; 16], [0u8; 16]);
                    pstore::log(&[b"\nferros hv: sysreg trap, core ", &[b'0' + core as u8], b" ESR ", pstore::hex(esr, &mut h1), b" ELR ", pstore::hex(elr, &mut h2), b" -- skipped\n"]);
                }
            }
            bump(0x80);
            if esr & 1 != 0 {
                // Direction bit set: a read returns 0. Rt is ISS[9:5]; the frame is indexed by register number (x0..x30), 31 = XZR (discard).
                let rt = ((esr >> 5) & 0x1F) as usize;
                if rt != 31 {
                    frame[rt] = 0;
                }
            }
            unsafe { asm!("msr elr_el2, {v}", v = in(reg) elr + 4) };
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
fn forward_smc(f: &mut [u64; 32]) {
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
    let (me, spsr): (u64, u64);
    unsafe { asm!("mrs {a}, mpidr_el1", "mrs {b}, spsr_el2", a = out(reg) me, b = out(reg) spsr) };
    let (mut h1, mut h2, mut h3, mut h4) = ([0u8; 16], [0u8; 16], [0u8; 16], [0u8; 16]);
    pstore::log(&[b"\nferros hv: UNEXPECTED trap, core ", &[b'0' + ((me >> 8) & 0xF) as u8], b" ESR ", pstore::hex(esr, &mut h1), b" ELR ", pstore::hex(elr, &mut h2), b" FAR ", pstore::hex(far, &mut h3), b" SPSR ", pstore::hex(spsr, &mut h4), b" -- core parked\n"]);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    stage(STAGE_PANIC);
    loop {
        unsafe { asm!("wfe") };
    }
}

/// The stage painted onto ABL's splash, which is still on the panel when the hv runs: a band across the top of the layer DMA0 is scanning (base, stride and size read from its registers, translation off on this path so the base is physical). Needs no memory that survives a reset and no host: the owner reads it off the glass. Greys are the same whichever byte order the surface has; magenta and green survive a red/blue swap too.
mod splash {
    use core::ptr::{read_volatile, write_volatile};
    const DMA0: usize = 0x1990_0000;
    pub const BAND_ROWS: usize = 96;

    pub fn band(slot: usize, argb: u32) {
        unsafe {
            let base = read_volatile((DMA0 + 0x40) as *const u32) as usize;
            let stride = read_volatile((DMA0 + 0x10) as *const u32) as usize;
            let img = read_volatile((DMA0 + 0x1C) as *const u32);
            let (w, h) = ((img & 0xFFFF) as usize, (img >> 16) as usize);
            if base < 0x8000_0000 || stride == 0 || stride > 8192 || w == 0 || h < BAND_ROWS * (slot + 1) {
                return;
            }
            let top = slot * BAND_ROWS;
            for y in top..top + BAND_ROWS - 8 {
                let row = base + y * stride * 4;
                for x in 0..w {
                    write_volatile((row + x * 4) as *mut u32, argb);
                }
            }
            core::arch::asm!("dsb sy");
        }
    }
}

/// The kill chord at EL2: Vol Up + Vol Down held together, read off the always-on GPIO banks on every trap the hv takes (thousands a second while Linux runs, and continuously in any early-boot livelock), so no GIC work is needed yet; the GIC group-0 FIQ hook (HV.md M1) makes it instant to the microsecond. Action: power off, the kill switch proper. A fastboot variant (stamp the mode through EL3, fire the cluster watchdogs, ~20 s) was measured working on 2026-10-17 and dropped in favour of this; it lives in git history if the dev loop ever wants a gesture for it.
mod chord {
    use core::arch::asm;
    use core::ptr::{read_volatile, write_volatile};
    const ALIVE: usize = 0x154D_0000;
    const GPA4_DAT: usize = ALIVE + 0x84;
    const GPA6_DAT: usize = ALIVE + 0xA4;
    const VOL_DOWN_PIN: u32 = 1;
    const VOL_UP_PIN: u32 = 2;

    const GPA4_CON: usize = ALIVE + 0x80;
    const GPA6_CON: usize = ALIVE + 0xA0;
    const GPA4_PUD: usize = ALIVE + 0x88;
    const GPA6_PUD: usize = ALIVE + 0xA8;
    static mut HITS: u32 = 0;

    /// Both pins as pulled-up inputs, so DAT reads the key (ABL hands them over in a state where DAT read 0 — "pressed" — and the chord fired on the first trap, 2026-10-17). Linux reconfigures them as EINTs later and reads the same bits.
    pub fn arm() {
        unsafe {
            let set = |con: usize, pud: usize, pin: u32| {
                let c = read_volatile(con as *const u32) & !(0xF << (pin * 4));
                write_volatile(con as *mut u32, c);
                let p = (read_volatile(pud as *const u32) & !(0xF << (pin * 4))) | (0x3 << (pin * 4));
                write_volatile(pud as *mut u32, p);
            };
            set(GPA4_CON, GPA4_PUD, VOL_DOWN_PIN);
            set(GPA6_CON, GPA6_PUD, VOL_UP_PIN);
            asm!("dsb sy");
        }
    }

    /// Both keys down right now.
    pub fn held_now() -> bool {
        unsafe { read_volatile(GPA4_DAT as *const u32) & (1 << VOL_DOWN_PIN) == 0 && read_volatile(GPA6_DAT as *const u32) & (1 << VOL_UP_PIN) == 0 }
    }

    /// Both keys down on two consecutive polls (the polls are traps, so a few microseconds to milliseconds apart).
    pub fn held() -> bool {
        if cfg!(hv_no_chord_poll) {
            return false;
        }
        unsafe {
            let both = read_volatile(GPA4_DAT as *const u32) & (1 << VOL_DOWN_PIN) == 0 && read_volatile(GPA6_DAT as *const u32) & (1 << VOL_UP_PIN) == 0;
            let n = if both { read_volatile(&raw const HITS) + 1 } else { 0 };
            write_volatile(&raw mut HITS, n);
            n >= 2
        }
    }

    const DSS_HEADER: usize = 0xD810_0000;
    const WATCHDOGS: [usize; 2] = [0x1006_0000, 0x1007_0000];
    const SMC_CMD_PRIV_REG: u64 = 0x8200_0504;
    const PMU_REBOOT_CMD: u64 = 0x1546_0810;
    const REBOOT_MODE_BOOTLOADER: u64 = 0xFC;

    fn ms(n: u64) {
        unsafe {
            let (f, t0): (u64, u64);
            asm!("mrs {f}, cntfrq_el0", "mrs {t}, cntpct_el0", f = out(reg) f, t = out(reg) t0);
            loop {
                let t: u64;
                asm!("mrs {t}, cntpct_el0", t = out(reg) t);
                if t.wrapping_sub(t0) >= f / 1000 * n {
                    break;
                }
            }
        }
    }

    /// The dev escape: both volume keys held while the hv starts (before Linux) send the phone to fastboot — the fastboot reboot mode stamped through EL3 and the cluster watchdogs fired, the path measured to land there (~20 s). A broken slot-a build otherwise retries forever (the slot is marked successful). Never returns.
    pub fn early_to_fastboot_if_held() {
        // The inputs can read "pressed" for a moment after a reset (watchdog reboots went to fastboot with nobody touching the phone, 2026-10-17 21:50): let the pull-ups settle, then demand both keys down on every one of 32 polls over a third of a second.
        ms(100);
        for _ in 0..32 {
            if !held_now() {
                return;
            }
            ms(10);
        }
        super::pstore::log(&[b"\nferros hv: volume keys held at start -- fastboot\n"]);
        unsafe {
            write_volatile((DSS_HEADER + 0x300) as *mut u32, 0xCAFE);
            write_volatile((DSS_HEADER + 0x100) as *mut u32, 0);
            for cpu in 0..9 {
                write_volatile((DSS_HEADER + 0x500 + cpu * 4) as *mut u32, 0);
            }
            asm!("dsb sy");
            asm!("smc #0", inout("x0") SMC_CMD_PRIV_REG => _, inout("x1") PMU_REBOOT_CMD => _, inout("x2") 1u64 => _, inout("x3") REBOOT_MODE_BOOTLOADER => _,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _, out("x10") _, out("x11") _, out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x16") _, out("x17") _);
            for &wdt in &WATCHDOGS {
                write_volatile(wdt as *mut u32, 0);
                ms(1);
                write_volatile((wdt + 0x04) as *mut u32, 1);
                ms(1);
                write_volatile((wdt + 0x08) as *mut u32, 1);
                ms(1);
                write_volatile(wdt as *mut u32, (1 << 5) | (1 << 0));
                ms(1);
            }
        }
        loop {
            unsafe { asm!("wfe") };
        }
    }

    /// Never returns: PSCI SYSTEM_OFF, the PMIC cuts the rails. The owner's test (2026-10-17): "watch that second vol button go down and if the next frame ain't black cuz the power is gone, you did it wrong." No stamp, no watchdog, no fastboot; the dev's fastboot is adb's problem.
    pub fn power_off() -> ! {
        super::pstore::log(&[b"\nferros hv: kill chord -- SYSTEM_OFF\n"]);
        unsafe {
            asm!("dsb sy");
            asm!("smc #0", in("x0") 0x8400_0008u64);
        }
        loop {
            unsafe { asm!("wfe") };
        }
    }
}

/// Lines into Android's pstore console ring (ramoops at G#FD3F_F000, 2 MiB, `persistent_ram_buffer`: u32 sig "DBGC", u32 start, u32 size, then data). pstore saves the ring to /sys/fs/pstore/console-ramoops-0 on the next Android boot, so the hv's words land in the kernel log timeline — the only RAM measured to survive this phone's resets (2026-10-17: everything outside the bootloader's preserved regions reads as decayed cells after any reset, and the pmsg half of ramoops too). The kernel appends to the same ring with atomics; the hv's plain read-modify-write from EL2 can clip a kernel line now and then, which is acceptable for a few lines per boot.
mod pstore {
    use core::ptr::{read_volatile, write_volatile};
    const ZONE: usize = 0xFD3F_F000;
    const ZONE_SIZE: usize = 0x20_0000;
    const DATA: usize = ZONE + 12;
    const BUF: u32 = (ZONE_SIZE - 12) as u32;

    pub fn log(parts: &[&[u8]]) {
        unsafe {
            if read_volatile(ZONE as *const u32) != 0x4347_4244 {
                return;
            }
            let mut start = read_volatile((ZONE + 4) as *const u32);
            let mut size = read_volatile((ZONE + 8) as *const u32);
            for part in parts {
                for &b in part.iter() {
                    write_volatile((DATA + start as usize) as *mut u8, b);
                    start = if start + 1 >= BUF { 0 } else { start + 1 };
                    if size < BUF {
                        size += 1;
                    }
                }
            }
            write_volatile((ZONE + 4) as *mut u32, start);
            write_volatile((ZONE + 8) as *mut u32, size);
            core::arch::asm!("dsb sy");
        }
    }

    /// 16 hex digits.
    pub fn hex(v: u64, out: &mut [u8; 16]) -> &[u8] {
        for (k, c) in out.iter_mut().enumerate() {
            *c = b"0123456789ABCDEF"[((v >> (60 - 4 * k)) & 0xF) as usize];
        }
        out
    }
}

/// The one DTB edit milestone 0 needs: a `hv@b2000000 { reg = <0x0 0xb2000000 0x0 0x4000000>; no-map; };` child under `/reserved-memory`, written as a fresh copy of the tree at `dst`. Big-endian flattened tree, standard token stream; the strings "reg" and "no-map" are taken from the existing strings block (every reserved-memory tree has them).
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
        // One node: BEGIN_NODE + 16-byte name, PROP reg (len 12: 2 address cells + 1 size cell), PROP no-map (len 0), END_NODE = 60 bytes.
        // The size is ONE cell because husky's /reserved-memory carries #address-cells = 2 and #size-cells = 1 (every sibling node's reg is 12 bytes).
        // Until 2026-10-19 this wrote four cells: Linux read the first size cell — 0 — and reserved nothing, so the hv's 64 MiB sat in the page allocator; Mahere's buffers and kernel structures landed in the hv's statics and stacks, and the hv's writes landed in theirs (HV.md, "the reservation that never was").
        let node_len = 4 + 16 + (12 + 12) + 12 + 4;
        let new_total = total + node_len;
        if new_total > dst_max {
            return None;
        }
        // Everything up to the insertion point, byte for byte (header fixed up after).
        let head = insert_at - src;
        copy(dst, src, head);
        let mut q = dst + head;
        for (name, base, size) in [(b"hv@afc000000\0\0\0\0", super::HV_BASE, super::HV_SIZE)] {
            wr32(q, BEGIN_NODE);
            q += 4;
            for (k, &c) in name.iter().enumerate() {
                wr8(q + k, c);
            }
            q += 16;
            wr32(q, PROP);
            wr32(q + 4, 12);
            wr32(q + 8, reg_off);
            wr32(q + 12, (base >> 32) as u32);
            wr32(q + 16, base as u32);
            wr32(q + 20, size as u32);
            q += 24;
            wr32(q, PROP);
            wr32(q + 4, 0);
            wr32(q + 8, nomap_off);
            q += 12;
            wr32(q, END_NODE);
            q += 4;
        }
        // The rest of the structure block and the strings block that follows it.
        copy(q, insert_at, total - head);
        wr32(dst + 4, new_total as u32);
        wr32(dst + 12, (off_strings + node_len) as u32);
        wr32(dst + 36, (size_struct + node_len) as u32);
        Some(new_total)
    }
}
