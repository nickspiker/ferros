//! Ferros kernel — framebuffer console build.
//!
//! Boots on FP5, fills framebuffer, renders text diagnostic console.

#![no_std]
#![no_main]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;

use ferros_hal::console::Console;
use ferros_hal::dpu;
use ferros_hal::dtb::Dtb;
use ferros_hal::pstore::{Ramoops, RamoopsConfig};
use ferros_hal::spmi;
use ferros_hal::uart::{Uart, UartBackend};

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

    // ================================================================
    // Save SCTLR and disable MMU + caches.
    // ABL (UEFI) may leave caches enabled. With write-back caches,
    // DMA masters (DWC3 USB) read stale DRAM, not the CPU cache.
    // We must clean+disable caches before any DMA.
    // ================================================================
    cmp     x20, #2
    b.ne    .Lsctlr_el1

    // EL2 path
    mrs     x21, sctlr_el2     // x21 = original SCTLR (saved for diagnostics)
    // Clean + invalidate data caches before disabling
    // (otherwise dirty lines are lost)
    mrs     x0, ctr_el0
    ubfx    x0, x0, #16, #4    // DminLine (log2 words)
    mov     x1, #4
    lsl     x1, x1, x0         // x1 = cache line size in bytes
    sub     x3, x1, #1          // line mask

    // Clean entire BSS + data region conservatively (0 to 8MB from load addr)
    adrp    x4, _start
    mov     x5, #0x800000       // 8MB
    add     x5, x5, x4
.Lclean_el2:
    dc      civac, x4
    add     x4, x4, x1
    cmp     x4, x5
    b.lo    .Lclean_el2
    dsb     sy
    isb

    bic     x0, x21, #(1 << 0)  // Clear M (MMU)
    bic     x0, x0, #(1 << 2)   // Clear C (data cache)
    bic     x0, x0, #(1 << 12)  // Clear I (instruction cache)
    msr     sctlr_el2, x0
    isb
    // Invalidate TLBs + I-cache after disabling
    tlbi    alle2
    ic      iallu
    dsb     sy
    isb
    b       .Lsctlr_done

.Lsctlr_el1:
    // EL1 path
    mrs     x21, sctlr_el1
    mrs     x0, ctr_el0
    ubfx    x0, x0, #16, #4
    mov     x1, #4
    lsl     x1, x1, x0
    sub     x3, x1, #1

    adrp    x4, _start
    mov     x5, #0x800000
    add     x5, x5, x4
.Lclean_el1:
    dc      civac, x4
    add     x4, x4, x1
    cmp     x4, x5
    b.lo    .Lclean_el1
    dsb     sy
    isb

    bic     x0, x21, #(1 << 0)
    bic     x0, x0, #(1 << 2)
    bic     x0, x0, #(1 << 12)
    msr     sctlr_el1, x0
    isb
    tlbi    vmalle1
    ic      iallu
    dsb     sy
    isb

.Lsctlr_done:

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

    adrp    x1, __boot_sctlr
    add     x1, x1, :lo12:__boot_sctlr
    str     x21, [x1]

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
/// GENI SE UART base (QUPv3 SE3, from stock cmdline console=ttyMSM0).
const FP5_UART_BASE: usize = 0x0099_4000;

// ---------------------------------------------------------------------------
// Statics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
static mut __exception_count: u64 = 0;
#[unsafe(no_mangle)]
static mut __boot_el: u64 = 0;
#[unsafe(no_mangle)]
static mut __boot_sctlr: u64 = 0;

fn exception_count() -> u64 {
    unsafe { core::ptr::read_volatile(&raw const __exception_count) }
}

fn boot_el() -> u64 {
    unsafe { core::ptr::read_volatile(&raw const __boot_el) }
}

fn boot_sctlr() -> u64 {
    unsafe { core::ptr::read_volatile(&raw const __boot_sctlr) }
}

// ---------------------------------------------------------------------------
// Log tee — writes to console + ramoops simultaneously
// ---------------------------------------------------------------------------

struct Log {
    con: Console,
    ram: Option<Ramoops>,
}

impl Log {
    fn putc(&mut self, b: u8) {
        self.con.putc(b);
        if let Some(ref mut r) = self.ram { r.putc(b); }
    }
    fn puts(&mut self, s: &str) {
        self.con.puts(s);
        if let Some(ref mut r) = self.ram { r.puts(s); }
    }
    fn put_hex(&mut self, val: u64) {
        self.con.put_hex(val);
        if let Some(ref mut r) = self.ram { r.put_hex(val); }
    }
    fn put_hex32(&mut self, val: u32) {
        self.con.put_hex32(val);
        if let Some(ref mut r) = self.ram { r.put_hex32(val); }
    }
}

// ---------------------------------------------------------------------------
// Kernel entry — framebuffer console + ramoops log
// ---------------------------------------------------------------------------

/// Parse ramoops zone config from DTB.
fn parse_ramoops_config(dtb: &Dtb) -> Option<RamoopsConfig> {
    // Find ramoops reg (base + size)
    let reg = dtb.find_node_prop(b"ramoops", b"reg")?;
    if reg.data.len() < 16 { return None; }
    let base = u64::from_be_bytes([
        reg.data[0], reg.data[1], reg.data[2], reg.data[3],
        reg.data[4], reg.data[5], reg.data[6], reg.data[7],
    ]);
    let size = u64::from_be_bytes([
        reg.data[8], reg.data[9], reg.data[10], reg.data[11],
        reg.data[12], reg.data[13], reg.data[14], reg.data[15],
    ]) as usize;

    let record_size = dtb.find_node_prop(b"ramoops", b"record-size")
        .and_then(|p| p.as_u32()).unwrap_or(0x40000) as usize;
    let console_size = dtb.find_node_prop(b"ramoops", b"console-size")
        .and_then(|p| p.as_u32()).unwrap_or(0x40000) as usize;
    let ftrace_size = dtb.find_node_prop(b"ramoops", b"ftrace-size")
        .and_then(|p| p.as_u32()).unwrap_or(0x40000) as usize;
    let pmsg_size = dtb.find_node_prop(b"ramoops", b"pmsg-size")
        .and_then(|p| p.as_u32()).unwrap_or(0x40000) as usize;

    Some(RamoopsConfig { base, size, record_size, console_size, ftrace_size, pmsg_size })
}

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(dtb_addr: u64) -> ! {
    let exc_start = exception_count();

    // ---- Set up framebuffer console ----
    let con = unsafe {
        Console::new(
            FP5_SPLASH_ADDR as *mut u32,
            FP5_FB_WIDTH as usize,
            FP5_FB_HEIGHT as usize,
            FP5_FB_WIDTH as usize,
            0xFFFF_FFFF,           // white text
            0xFF00_0000,           // black background
            (120, 48, 80, 48),     // margins: top (notch), right, bottom, left
        )
    };

    // ---- Parse DTB early to find ramoops (fallback to known FP5 address) ----
    let ramoops = if dtb_addr != 0 {
        unsafe { Dtb::from_ptr(dtb_addr as *const u8) }
            .and_then(|dtb| parse_ramoops_config(&dtb))
            .map(|cfg| unsafe { Ramoops::from_config(&cfg) })
    } else {
        None
    };
    // Fallback: if DTB lookup failed, use known FP5 ramoops region directly.
    // ramoops@0xBA800000, 2MB, default zone sizes (256KB each).
    let ramoops = ramoops.or_else(|| {
        let cfg = RamoopsConfig {
            base: 0xBA80_0000,
            size: 0x20_0000,
            record_size: 0x40000,
            console_size: 0x40000,
            ftrace_size: 0x40000,
            pmsg_size: 0x40000,
        };
        Some(unsafe { Ramoops::from_config(&cfg) })
    });

    let mut log = Log { con, ram: ramoops };
    log.con.clear();

    // ---- Banner ----
    log.puts("ferros v0.0 on Fairphone 5 (QCM6490)\n");
    log.puts("=====================================\n\n");

    // ---- Boot diagnostics ----
    log.puts("Boot EL:       "); log.put_hex32(boot_el() as u32); log.puts("\n");
    let sctlr = boot_sctlr();
    log.puts("SCTLR (orig):  "); log.put_hex32(sctlr as u32); log.puts("\n");
    log.puts("  MMU="); log.put_hex32((sctlr & 1) as u32);
    log.puts(" D$="); log.put_hex32(((sctlr >> 2) & 1) as u32);
    log.puts(" I$="); log.put_hex32(((sctlr >> 12) & 1) as u32);
    log.puts("\n");
    log.puts("DTB addr:      "); log.put_hex(dtb_addr); log.puts("\n");
    log.puts("Exceptions:    "); log.put_hex(exception_count() - exc_start); log.puts("\n");
    log.puts("pstore:        ");
    if log.ram.is_some() { log.puts("OK (ramoops)\n"); } else { log.puts("not found\n"); }

    // ---- DPU MMIO probe ----
    let exc_pre = exception_count();
    let hw_ver = dpu::hw_version();
    let exc_post = exception_count();
    let dpu_ok = exc_post == exc_pre;

    log.puts("\n-- DPU --\n");
    log.puts("MDSS HW_VER:   "); log.put_hex32(hw_ver);
    log.puts(if dpu_ok { " (OK)\n" } else { " (FAULT)\n" });

    if dpu_ok {
        log.puts("CTL0_TOP:      "); log.put_hex32(dpu::read_reg(dpu::MDP_BASE + 0x15014)); log.puts("\n");
        log.puts("CTL0_FLUSH:    "); log.put_hex32(dpu::read_reg(dpu::MDP_BASE + 0x15018)); log.puts("\n");
        log.puts("INTF1_EN:      "); log.put_hex32(dpu::read_reg(dpu::MDP_BASE + 0x35000)); log.puts("\n");
        log.puts("VIG0 SRC_FMT:  "); log.put_hex32(dpu::read_reg(dpu::SSPP_VIG0 + 0x30)); log.puts("\n");
        log.puts("VIG0 SRC_ADDR: "); log.put_hex32(dpu::read_reg(dpu::SSPP_VIG0 + 0x14)); log.puts("\n");
        log.puts("VIG0 STRIDE:   "); log.put_hex32(dpu::read_reg(dpu::SSPP_VIG0 + 0x24)); log.puts("\n");
        log.puts("VIG0 SRC_SIZE: "); log.put_hex32(dpu::read_reg(dpu::SSPP_VIG0 + 0x00)); log.puts("\n");

        log.puts("DMA0 SRC_ADDR: "); log.put_hex32(dpu::read_reg(dpu::SSPP_DMA0 + 0x14)); log.puts("\n");
        log.puts("DMA0 STRIDE:   "); log.put_hex32(dpu::read_reg(dpu::SSPP_DMA0 + 0x24)); log.puts("\n");
        log.puts("DMA0 SRC_SIZE: "); log.put_hex32(dpu::read_reg(dpu::SSPP_DMA0 + 0x00)); log.puts("\n");
    }

    // ---- UART ----
    log.puts("\n-- UART --\n");
    let uart = Uart::new(UartBackend::GeniSe { base: FP5_UART_BASE });
    let exc_pre = exception_count();
    if uart.probe() && exception_count() == exc_pre {
        uart.init();
        log.puts("UART:          OK\n");
        uart.puts("ferros v0.0 UART alive\r\n");
    } else {
        log.puts("UART:          FAIL\n");
    }

    // ---- DTB parse ----
    log.puts("\n-- DTB --\n");
    if dtb_addr != 0 {
        let dtb = unsafe { Dtb::from_ptr(dtb_addr as *const u8) };
        match dtb {
            Some(dtb) => {
                log.puts("DTB valid:     "); log.put_hex32(dtb.total_size() as u32); log.puts(" bytes\n");

                if let Some(bootargs) = dtb.find_node_prop(b"chosen", b"bootargs") {
                    log.puts("bootargs:\n");
                    if let Some(s) = bootargs.as_str() {
                        let bytes = s.as_bytes();
                        let mut i = 0;
                        while i < bytes.len() {
                            let end = if i + 100 < bytes.len() { i + 100 } else { bytes.len() };
                            log.puts("  ");
                            log.puts(&s[i..end]);
                            log.puts("\n");
                            i = end;
                        }
                    }
                }

                match dtb.parse_simplefb() {
                    Some(fb) => {
                        log.puts("simplefb addr: "); log.put_hex(fb.phys_base); log.puts("\n");
                        log.puts("simplefb size: "); log.put_hex32(fb.width); log.puts("x"); log.put_hex32(fb.height); log.puts("\n");
                        log.puts("simplefb strd: "); log.put_hex32(fb.stride); log.puts("\n");
                    }
                    None => log.puts("simplefb:      not found\n"),
                }

                if let Some(reg) = dtb.find_node_prop(b"memory", b"reg") {
                    log.puts("memory reg:    ");
                    let mut off = 0;
                    while off + 16 <= reg.data.len() {
                        let addr = u64::from_be_bytes([
                            reg.data[off], reg.data[off+1], reg.data[off+2], reg.data[off+3],
                            reg.data[off+4], reg.data[off+5], reg.data[off+6], reg.data[off+7],
                        ]);
                        let size = u64::from_be_bytes([
                            reg.data[off+8], reg.data[off+9], reg.data[off+10], reg.data[off+11],
                            reg.data[off+12], reg.data[off+13], reg.data[off+14], reg.data[off+15],
                        ]);
                        if off > 0 { log.puts("               "); }
                        log.put_hex(addr); log.puts(" +"); log.put_hex(size); log.puts("\n");
                        off += 16;
                    }
                }

                log.puts("\n-- reserved-memory --\n");
                let mut rmem_count = 0u32;
                dtb.for_each_child_reg(b"reserved-memory", |name, reg| {
                    log.puts("  ");
                    for &b in name {
                        if b == b'@' { break; }
                        log.putc(b);
                    }
                    log.puts(": ");
                    if reg.len() >= 16 {
                        let addr = u64::from_be_bytes([
                            reg[0], reg[1], reg[2], reg[3],
                            reg[4], reg[5], reg[6], reg[7],
                        ]);
                        let size = u64::from_be_bytes([
                            reg[8], reg[9], reg[10], reg[11],
                            reg[12], reg[13], reg[14], reg[15],
                        ]);
                        log.put_hex(addr); log.puts(" +"); log.put_hex(size);
                    } else if reg.len() >= 8 {
                        let addr = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]);
                        let size = u32::from_be_bytes([reg[4], reg[5], reg[6], reg[7]]);
                        log.put_hex32(addr); log.puts(" +"); log.put_hex32(size);
                    }
                    log.puts("\n");
                    rmem_count += 1;
                });
                if rmem_count == 0 { log.puts("  (none found)\n"); }

                log.puts("\nnodes:         ");
                let mut count = 0u32;
                dtb.for_each_node(|name, depth| {
                    if depth == 2 && !name.is_empty() {
                        if count > 0 { log.puts(" "); }
                        if count < 16 {
                            for &b in name {
                                if b == b'@' { break; }
                                log.putc(b);
                            }
                        }
                        count += 1;
                    }
                });
                if count >= 16 { log.puts(" ..."); }
                log.puts(" ("); log.put_hex32(count); log.puts(" total)\n");
            }
            None => {
                log.puts("DTB invalid at "); log.put_hex(dtb_addr); log.puts("\n");
            }
        }
    } else {
        log.puts("DTB addr:      NULL\n");
    }

    // ---- USB DWC3 probe ----
    log.puts("\n-- USB DWC3 --\n");
    let exc_pre = exception_count();
    let usb_info = ferros_hal::usb::probe();
    let exc_post = exception_count();
    let usb_ok = exc_post == exc_pre;

    if usb_ok && usb_info.is_valid() {
        log.puts("GSNPSID:       "); log.put_hex32(usb_info.snpsid); log.puts("\n");
        log.puts("revision:      "); log.put_hex32(usb_info.revision() as u32); log.puts("\n");
        log.puts("GCTL:          "); log.put_hex32(usb_info.gctl); log.puts("\n");
        log.puts("mode:          "); log.puts(usb_info.port_cap()); log.puts("\n");
        log.puts("endpoints:     "); log.put_hex32(usb_info.num_eps()); log.puts("\n");
        log.puts("DSTS:          "); log.put_hex32(usb_info.dsts); log.puts("\n");

        // Dump pre-init state
        log.puts("Pre-init DCTL: "); log.put_hex32(unsafe { ferros_hal::mmio::read32(0x0A60C704) }); log.puts("\n");
        log.puts("Pre-init GEVNTCOUNT: "); log.put_hex32(unsafe { ferros_hal::mmio::read32(0x0A60C40C) }); log.puts("\n");

        // Bypass SMMU so DWC3 DMA uses physical addresses directly
        ferros_hal::usb::smmu_bypass();
        log.puts("SMMU:          bypass\n");

        // Initialize USB PHY (SNPS Femto v2 — ABL tears it down at handoff)
        log.puts("PHY init:      ");
        if ferros_hal::usb::phy_init() {
            log.puts("OK\n");
        } else {
            log.puts("FAIL\n");
        }

        // Initialize USB device mode
        log.puts("USB init:      ");
        match ferros_hal::usb::Dwc3Dev::init() {
            Some(mut usb) => {
                log.puts("OK\n");

                // Halt diagnostics
                log.puts("halt_ok:       ");
                if usb.halt_ok { log.puts("YES\n"); } else { log.puts("NO\n"); }
                log.puts("DSTS@halt:     "); log.put_hex32(usb.dsts_at_halt); log.puts("\n");

                // Print computed DMA buffer addresses vs what DWC3 has
                let (evt_a, trb_a, setup_a, data_a) = ferros_hal::usb::dma_buffer_addrs();
                log.puts("EVT_BUF @:     "); log.put_hex(evt_a); log.puts("\n");
                log.puts("EP0_TRBS @:    "); log.put_hex(trb_a); log.puts("\n");
                log.puts("SETUP_BUF @:   "); log.put_hex(setup_a); log.puts("\n");
                log.puts("DATA_BUF @:    "); log.put_hex(data_a); log.puts("\n");

                // Dump post-init register state
                let diag = ferros_hal::usb::dump_diag(exception_count);
                log.puts("DCTL:          "); log.put_hex32(diag.dctl); log.puts("\n");
                log.puts("DSTS:          "); log.put_hex32(diag.dsts); log.puts("\n");
                log.puts("DCFG:          "); log.put_hex32(diag.dcfg); log.puts("\n");
                log.puts("DEVTEN:        "); log.put_hex32(diag.devten); log.puts("\n");
                log.puts("GCTL:          "); log.put_hex32(diag.gctl); log.puts("\n");
                log.puts("GSTS:          "); log.put_hex32(diag.gsts); log.puts("\n");
                log.puts("EVT_ADRLO:     "); log.put_hex32(diag.gevntadrlo); log.puts("\n");
                log.puts("EVT_ADRHI:     "); log.put_hex32(diag.gevntadrhi); log.puts("\n");
                log.puts("EVT_SIZ:       "); log.put_hex32(diag.gevntsiz); log.puts("\n");
                log.puts("EVT_COUNT:     "); log.put_hex32(diag.gevntcount); log.puts("\n");
                log.puts("QCOM_GEN_CFG:  "); log.put_hex32(diag.qcom_general_cfg); log.puts("\n");
                log.puts("QCOM_HS_PHY:   "); log.put_hex32(diag.qcom_hs_phy_ctrl); log.puts("\n");
                log.puts("QCOM_SS_PHY:   "); log.put_hex32(diag.qcom_ss_phy_ctrl); log.puts("\n");
                log.puts("QUSB2_PWR:     "); log.put_hex32(diag.qusb2_pwr_ctrl); log.puts("\n");
                log.puts("QMP_PWR:       "); log.put_hex32(diag.qmp_pwr_ctrl); log.puts("\n");
                log.puts("USB2_UTMI:     "); log.put_hex32(diag.usb2_phy_utmi_ctrl); log.puts("\n");
                log.puts("PHY exc:       "); log.put_hex32(diag.exc_during_phy); log.puts("\n");
                log.puts("GUSB2PHYCFG:   "); log.put_hex32(diag.gusb2phycfg); log.puts("\n");
                log.puts("GUSB3PIPECTL:  "); log.put_hex32(diag.gusb3pipectl); log.puts("\n");
                log.puts("DALEPENA:      "); log.put_hex32(diag.dalepena); log.puts("\n");
                log.puts("EVT_RAW[0-3]:  ");
                log.put_hex32(diag.evt_buf_raw[0]); log.puts(" ");
                log.put_hex32(diag.evt_buf_raw[1]); log.puts(" ");
                log.put_hex32(diag.evt_buf_raw[2]); log.puts(" ");
                log.put_hex32(diag.evt_buf_raw[3]); log.puts("\n");

                // Check if Run/Stop is actually set
                if diag.dctl & (1 << 31) != 0 {
                    log.puts("Run/Stop:      SET\n");
                } else {
                    log.puts("Run/Stop:      CLEARED!\n");
                }

                log.puts("Waiting for host...\n");

                // Poll for USB events indefinitely
                let mut evt_count = 0u32;
                let mut poll_count = 0u32;

                loop {
                    match usb.poll_event() {
                        ferros_hal::usb::UsbEvent::None => {}
                        ferros_hal::usb::UsbEvent::Reset => {
                            log.puts("  USB reset\n");
                            usb.handle_reset();
                            evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::ConnectDone { speed } => {
                            log.puts("  connected: ");
                            log.puts(ferros_hal::usb::speed_string(speed));
                            log.puts(" DSTS=");
                            log.put_hex32(unsafe { ferros_hal::mmio::read32(0x0A60C70C) });
                            log.puts(" PHY=");
                            log.put_hex32(unsafe { ferros_hal::mmio::read32(0x0A60C200) });
                            log.puts("\n");
                            usb.handle_connect_done();
                            usb.ep0_start_setup();
                            evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::Disconnect => {
                            log.puts("  disconnected\n");
                            evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::Ep0Setup { request } => {
                            log.puts("  SETUP: ");
                            for i in 0..8 {
                                log.put_hex32(request[i] as u32);
                                log.puts(" ");
                            }
                            log.puts("\n");
                            if !usb.handle_setup(&request) {
                                log.puts("  (stall)\n");
                                usb.ep0_stall();
                            }
                            evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::TransferComplete { ep } => {
                            // EP0 status stage is handled internally by the driver.
                            // This arm now only fires for non-EP0 endpoints.
                            evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::TransferNotReady { .. } => {
                            evt_count += 1;
                        }
                    }

                    poll_count = poll_count.wrapping_add(1);
                    // Print status every ~10M polls
                    if poll_count & 0x00FF_FFFF == 0 {
                        log.puts("  X1 "); log.put_hex32(usb.ep1_xfer_complete);
                        log.puts(" CS "); log.put_hex32(usb.cmd_status_fail);
                        log.puts(" D1 "); log.put_hex32(usb.ep1_data_notready);
                        if usb.cmd_status_fail > 0 {
                            log.puts("\n  CMD="); log.put_hex32(usb.last_cmd_status);
                            log.puts(" ep="); log.put_hex32(usb.last_cmd_ep as u32);
                            log.puts(" ty="); log.put_hex32(usb.last_cmd_type);
                        }
                        log.puts("\n");
                    }
                }
            }
            None => {
                log.puts("FAIL (reset timeout)\n");
            }
        }
    } else if usb_ok {
        log.puts("DWC3:          bad SNPSID "); log.put_hex32(usb_info.snpsid); log.puts("\n");
    } else {
        log.puts("DWC3:          FAULT (clocks gated?)\n");
    }

    // ---- Final ----
    log.puts("\nTotal exc:     "); log.put_hex(exception_count()); log.puts("\n");
    if let Some(ref r) = log.ram {
        log.con.puts("pstore bytes:  "); log.con.put_hex32(r.written() as u32); log.con.puts("\n");
    }

    // ---- Halt (WFE loop) — do NOT reboot to avoid boot loops ----
    log.puts("\n-- halted (hold power to reboot) --\n");
    unsafe { core::arch::asm!("dsb sy"); }
    loop { unsafe { core::arch::asm!("wfe"); } }
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
