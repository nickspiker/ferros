//! Ferros kernel — display test build.
//!
//! Code execution confirmed (PSCI reboot + red flash seen).
//! Now focusing on persistent display output via DPU trigger.
//!
//! Sequence:
//!   1. Fill framebuffer red (DRAM, in assembly — immediate)
//!   2. Set up exception handler + stack → enter Rust
//!   3. Fill entire FB with color pattern
//!   4. Try DPU splash handoff (flush all CTLs)
//!   5. Try full DPU pipeline setup
//!   6. Stay alive (WFE loop, no reboot)

#![no_std]
#![no_main]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;

use ferros_hal::dpu;

// ---------------------------------------------------------------------------
// Global allocator
// ---------------------------------------------------------------------------

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

const HEAP_SIZE: usize = 256 * 1024;

#[repr(align(4096))]
struct HeapMem(UnsafeCell<[u8; HEAP_SIZE]>);
unsafe impl Sync for HeapMem {}

static HEAP: HeapMem = HeapMem(UnsafeCell::new([0; HEAP_SIZE]));
static HEAP_POS: AtomicUsize = AtomicUsize::new(0);

struct BumpAlloc;

unsafe impl GlobalAlloc for BumpAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = HEAP.0.get() as *mut u8;
        loop {
            let pos = HEAP_POS.load(Ordering::Relaxed);
            let aligned = (pos + layout.align() - 1) & !(layout.align() - 1);
            let new_pos = aligned + layout.size();
            if new_pos > HEAP_SIZE {
                return core::ptr::null_mut();
            }
            if HEAP_POS.compare_exchange_weak(pos, new_pos, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                return unsafe { base.add(aligned) };
            }
        }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOC: BumpAlloc = BumpAlloc;

// ---------------------------------------------------------------------------
// Boot stub — minimal, no MMU
// ---------------------------------------------------------------------------

global_asm!(r#"
.section .text.boot, "ax"
.global _start

// ========================================================================
// ARM64 Image header with PE/COFF stub
//
// Qualcomm ABL (UEFI-based) requires PE/COFF format to find entry point.
// Stock FP5 kernel starts with "MZ" (PE/COFF). We replicate this.
//
// Layout:
//   0x000: DOS/ARM64 header (MZ + branch + ARM64 Image fields + e_lfanew)
//   0x040: PE header (PE\0\0 + COFF + Optional + Section table)
//   0x1000: _entry (page-aligned .text section start)
// ========================================================================

_start:
    // Offset 0x00: "MZ" magic — must be valid ARM64: `add x13, x18, #0x16`
    // Bytes: 4D 5A 00 91. Stock kernel uses this exact encoding.
    .long   0x91005A4D
    // Offset 0x04: branch to _entry (past all headers)
    b       _entry

    // Offset 0x08: text_offset (ARM64 Image header)
    .quad   0x80000                 // text_offset = 0x80000 (matches stock FP5 kernel)
    // Offset 0x10: image_size
    .quad   __kernel_size
    // Offset 0x18: flags — LE, 4K pages, can be placed anywhere
    .quad   0x0A
    // Offset 0x20: res2
    .quad   0
    // Offset 0x28: res3
    .quad   0
    // Offset 0x30: res4
    .quad   0
    // Offset 0x38: ARM64 magic
    .ascii  "ARM\x64"
    // Offset 0x3C: PE header offset (e_lfanew for PE/COFF)
    .long   .Lpe_header - _start

// ---------- PE/COFF Header (at offset 0x40) ----------
.balign 4
.Lpe_header:
    .ascii  "PE\0\0"               // PE magic

    // COFF header (20 bytes)
    .short  0xAA64                  // Machine: ARM64
    .short  1                       // NumberOfSections
    .long   0                       // TimeDateStamp
    .long   0                       // PointerToSymbolTable
    .long   0                       // NumberOfSymbols
    .short  .Lsection_table - .Loptional_header   // SizeOfOptionalHeader
    .short  0x206                   // Characteristics: EXEC | NO_LINE | NO_DEBUG

.Loptional_header:
    // PE32+ Optional Header
    .short  0x20B                   // Magic: PE32+
    .byte   0                       // MajorLinkerVersion
    .byte   0                       // MinorLinkerVersion
    .long   __kernel_size - 0x1000   // SizeOfCode (section data, excl headers)
    .long   0                       // SizeOfInitializedData
    .long   0                       // SizeOfUninitializedData
    .long   0x1000                  // AddressOfEntryPoint (RVA = page 1)
    .long   0x1000                  // BaseOfCode

    // PE32+ fields
    .quad   0                       // ImageBase (relocatable)
    .long   0x1000                  // SectionAlignment (4K)
    .long   0x200                   // FileAlignment (512 — matches stock)
    .short  0                       // MajorOperatingSystemVersion
    .short  0                       // MinorOperatingSystemVersion
    .short  0                       // MajorImageVersion
    .short  0                       // MinorImageVersion
    .short  0                       // MajorSubsystemVersion
    .short  0                       // MinorSubsystemVersion
    .long   0                       // Win32VersionValue
    .long   __kernel_size           // SizeOfImage
    .long   0x1000                  // SizeOfHeaders (one page)
    .long   0                       // CheckSum
    .short  10                      // Subsystem: EFI Application
    .short  0                       // DllCharacteristics
    .quad   0                       // SizeOfStackReserve
    .quad   0                       // SizeOfStackCommit
    .quad   0                       // SizeOfHeapReserve
    .quad   0                       // SizeOfHeapCommit
    .long   0                       // LoaderFlags
    .long   6                       // NumberOfRvaAndSizes (6, matches stock)

    // Data directories (6 entries, all empty — matches stock)
    .quad   0                       // Export Table
    .quad   0                       // Import Table
    .quad   0                       // Resource Table
    .quad   0                       // Exception Table
    .quad   0                       // Certificate Table
    .quad   0                       // Base Relocation Table

.Lsection_table:
    // Section header: ".text"
    .ascii  ".text\0\0\0"           // Name (8 bytes)
    .long   __kernel_size - 0x1000  // VirtualSize (section virtual extent, incl BSS)
    .long   0x1000                  // VirtualAddress (RVA, page-aligned)
    .long   __bss_start - _start - 0x1000  // SizeOfRawData (file-backed only, excl BSS)
    .long   0x1000                  // PointerToRawData (file offset)
    .long   0                       // PointerToRelocations
    .long   0                       // PointerToLinenumbers
    .short  0                       // NumberOfRelocations
    .short  0                       // NumberOfLinenumbers
    .long   0xE0000020              // Characteristics: CODE | EXECUTE | READ | WRITE

// ---------- Actual kernel entry (page-aligned at RVA 0x1000) ----------
.balign 0x1000
_entry:
    // Save DTB pointer, mask interrupts
    mov     x19, x0
    msr     daifset, #0xF

    // ================================================================
    // IMMEDIATE PROOF OF LIFE — before ANY other setup.
    // No stack, no BSS, no exception handler, no Rust.
    // ================================================================

    // -- Write red pixels to splash FB (DRAM, no MMIO needed) --
    // If DPU is still scanning from ABL, screen turns red.
    movz    x8, #0x0000
    movk    x8, #0xE100, lsl #16   // x8 = 0xE1000000 (splash FB base)
    movz    w9, #0x0000
    movk    w9, #0xFFFF, lsl #16   // w9 = 0xFFFF0000 (XRGB red)

    // Fill ALL scanlines (1224 * 2700 = 3304800 = 0x326BE0 pixels)
    movz    x10, #0x6BE0
    movk    x10, #0x32, lsl #16    // x10 = 0x326BE0
.Lfill_red:
    str     w9, [x8], #4
    subs    x10, x10, #1
    b.ne    .Lfill_red
    dsb     sy

    // Fall through to normal boot (set up exception handler, stack, Rust)

    mrs     x20, CurrentEL
    lsr     x20, x20, #2

    adr     x2, .Lvectors
    cmp     x20, #2
    b.ne    .Lvbar_el1
    msr     vbar_el2, x2
    b       .Lvbar_done
.Lvbar_el1:
    msr     vbar_el1, x2
.Lvbar_done:
    isb

    msr     spsel, #1
    adrp    x1, __stack_top
    add     x1, x1, :lo12:__stack_top
    mov     sp, x1

    adrp    x1, __bss_start
    add     x1, x1, :lo12:__bss_start
    adrp    x2, __bss_end
    add     x2, x2, :lo12:__bss_end
.Lbss_loop:
    cmp     x1, x2
    b.ge    .Lbss_done
    str     xzr, [x1], #8
    b       .Lbss_loop
.Lbss_done:

    adrp    x1, __boot_el
    add     x1, x1, :lo12:__boot_el
    str     x20, [x1]

    mov     x0, x19
    bl      kernel_main

.Lhalt:
    wfe
    b       .Lhalt

.balign 0x800
.Lvectors:
    b       .Lexc_recover
    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt

    .balign 0x80
    b       .Lexc_recover
    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt

    .balign 0x80
    b       .Lexc_recover
    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt

    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt

.balign 16
.Lexc_recover:
    stp     x0, x1, [sp, #-16]!
    adrp    x0, __exception_count
    add     x0, x0, :lo12:__exception_count
    ldr     x1, [x0]
    add     x1, x1, #1
    str     x1, [x0]
    mrs     x1, CurrentEL
    lsr     x1, x1, #2
    cmp     x1, #2
    b.eq    .Lexc_el2
    mrs     x0, elr_el1
    add     x0, x0, #4
    msr     elr_el1, x0
    b       .Lexc_ret
.Lexc_el2:
    mrs     x0, elr_el2
    add     x0, x0, #4
    msr     elr_el2, x0
.Lexc_ret:
    ldp     x0, x1, [sp], #16
    eret
"#);

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const FP5_FB_WIDTH: u32 = 1224;
const FP5_FB_HEIGHT: u32 = 2700;
const FP5_FB_STRIDE: u32 = FP5_FB_WIDTH * 4;
const FP5_SPLASH_ADDR: u64 = 0xE100_0000;

/// PS_HOLD register — writing 0 kills power (Qualcomm TCSR).
const PS_HOLD: usize = 0x0C26_4000;

// ---------------------------------------------------------------------------
// Statics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
static mut __exception_count: u64 = 0;
#[unsafe(no_mangle)]
static mut __boot_el: u64 = 0;

fn exception_count() -> u64 {
    unsafe { core::ptr::read_volatile(&raw const __exception_count) }
}

fn boot_el() -> u64 {
    unsafe { core::ptr::read_volatile(&raw const __boot_el) }
}

// ---------------------------------------------------------------------------
// Kernel entry — display test
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(_dtb_addr: u64) -> ! {
    let exc_before = exception_count();

    // ---- Phase 1: Read MDSS HW_VERSION to check if DPU MMIO is accessible ----
    let hw_ver = dpu::hw_version();
    let exc_after_hwver = exception_count();
    let dpu_accessible = exc_after_hwver == exc_before;

    // ---- Phase 2: Fill entire FB bright red (assembly already did this, ----
    // ---- but redo from Rust to be sure — full 1224x2700)              ----
    let fb_base = FP5_SPLASH_ADDR as *mut u32;
    let w = FP5_FB_WIDTH as usize;
    let h = FP5_FB_HEIGHT as usize;
    let stride_words = FP5_FB_STRIDE as usize / 4;
    let red: u32 = 0xFFFF_0000;

    for y in 0..h {
        for x in 0..w {
            unsafe { fb_base.add(y * stride_words + x).write_volatile(red) };
        }
    }
    unsafe { core::arch::asm!("dsb sy") };

    // ---- Phase 3: Try DPU splash handoff (flush + start all CTLs) ----
    // This is the simplest approach: ABL left the DPU pipeline configured,
    // we just tell it to push a new frame.
    if dpu_accessible {
        dpu::try_splash_handoff();
        unsafe { core::arch::asm!("dsb sy") };

        // Wait a bit for the frame to be sent
        spin_ms(100);

        // Try again (command mode may need repeated triggers)
        dpu::try_splash_handoff();
        unsafe { core::arch::asm!("dsb sy") };

        spin_ms(100);

        // ---- Phase 4: Try full pipeline setup (VIG0 path) ----
        dpu::setup_pipeline(
            FP5_SPLASH_ADDR as u32,
            FP5_FB_WIDTH,
            FP5_FB_HEIGHT,
            FP5_FB_STRIDE,
        );
        unsafe { core::arch::asm!("dsb sy") };

        spin_ms(200);

        // ---- Phase 5: Try DMA0 path instead ----
        dpu::setup_pipeline_dma0(
            FP5_SPLASH_ADDR as u32,
            FP5_FB_WIDTH,
            FP5_FB_HEIGHT,
            FP5_FB_STRIDE,
        );
        unsafe { core::arch::asm!("dsb sy") };

        // ---- Phase 6: Repeated flush loop — keep triggering DPU ----
        // Some command-mode panels need periodic triggers.
        // Also try re-triggering every 100ms for 30 seconds.
        for _ in 0..300 {
            dpu::try_splash_handoff();
            spin_ms(100);
        }
    }

    // ---- Write diagnostics to DRAM ----
    let exc_end = exception_count();
    let diag_addr = 0x8008_5000u64 as *mut u64;
    unsafe {
        diag_addr.write_volatile(0xFE00_D150_0000_0000u64 | boot_el());
        diag_addr.add(1).write_volatile(hw_ver as u64);
        diag_addr.add(2).write_volatile(if dpu_accessible { 1 } else { 0 });
        diag_addr.add(3).write_volatile(exc_end);
    }

    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}

/// Spin delay — approximately `ms` milliseconds on a ~1GHz core.
fn spin_ms(ms: u32) {
    // ~4 instructions per inner loop iteration, ~1GHz → ~250K iterations/ms
    for _ in 0..ms {
        for _ in 0..250_000u32 {
            unsafe { core::arch::asm!("nop") };
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop { unsafe { core::arch::asm!("wfe") }; }
}
