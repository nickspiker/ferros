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
use ferros_ledger::event::Event;
use ferros_ledger::chain::Chain;
use ferros_pt::packet::{self, Spec, Ack, Complete};
use ferros_pt::transfer::{InboundTransfer, OutboundTransfer, bitmap_words};

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
const FP5_SPLASH_ADDR: u64 = 0xE100_0000;

/// PS_HOLD register — writing 0 kills power (Qualcomm TCSR).
const PS_HOLD: usize = 0x0C26_4000;
/// GENI SE UART base (QUPv3 SE3, from stock cmdline console=ttyMSM0).
const FP5_UART_BASE: usize = 0x0099_4000;
const FP5_SDC2_BASE: usize = 0x0880_4000; // QCM6490 SDHCI for microSD

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
    /// Boot log buffer — captured for USB retrieval.
    buf: alloc::vec::Vec<u8>,
    /// When false, puts/put_hex skip console output (buf + ramoops only).
    screen: bool,
}

impl Log {
    fn putc(&mut self, b: u8) {
        if self.screen { self.con.putc(b); }
        if let Some(ref mut r) = self.ram { r.putc(b); }
        self.buf.push(b);
    }
    fn puts(&mut self, s: &str) {
        if self.screen { self.con.puts(s); }
        if let Some(ref mut r) = self.ram { r.puts(s); }
        self.buf.extend_from_slice(s.as_bytes());
    }
    fn put_hex(&mut self, val: u64) {
        if self.screen { self.con.put_hex(val); }
        if let Some(ref mut r) = self.ram { r.put_hex(val); }
        let mut tmp = [0u8; 18];
        let n = fmt_hex64(val, &mut tmp);
        self.buf.extend_from_slice(&tmp[..n]);
    }
    fn put_hex32(&mut self, val: u32) {
        if self.screen { self.con.put_hex32(val); }
        if let Some(ref mut r) = self.ram { r.put_hex32(val); }
        let mut tmp = [0u8; 10];
        let n = fmt_hex32(val, &mut tmp);
        self.buf.extend_from_slice(&tmp[..n]);
    }
    /// Write to log buffer only (no console, no ramoops).
    fn buf_only(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
    }
    fn buf_put_hex32(&mut self, val: u32) {
        let mut tmp = [0u8; 10];
        let n = fmt_hex32(val, &mut tmp);
        self.buf.extend_from_slice(&tmp[..n]);
    }
}

/// Format u32 as hex into buffer, no leading zeros. Returns length written.
fn fmt_hex32(val: u32, buf: &mut [u8; 10]) -> usize {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    if val == 0 {
        buf[0] = b'0';
        return 1;
    }
    // Find first non-zero nibble
    let mut started = false;
    let mut pos = 0;
    for i in 0..8 {
        let nibble = ((val >> (28 - i * 4)) & 0xF) as usize;
        if nibble != 0 || started {
            buf[pos] = HEX[nibble];
            pos += 1;
            started = true;
        }
    }
    pos
}

/// Format u64 as hex into buffer, no leading zeros. Returns length written.
fn fmt_hex64(val: u64, buf: &mut [u8; 18]) -> usize {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    if val == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut started = false;
    let mut pos = 0;
    for i in 0..16 {
        let nibble = ((val >> (60 - i * 4)) & 0xF) as usize;
        if nibble != 0 || started {
            buf[pos] = HEX[nibble];
            pos += 1;
            started = true;
        }
    }
    pos
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

    let mut log = Log { con, ram: ramoops, buf: alloc::vec::Vec::with_capacity(8192), screen: false };
    log.con.clear();
    // Banner only — everything else goes to buf + ramoops
    log.screen = true;
    log.puts("ferros v0.0\n");
    log.screen = false;

    // ---- Ledger init ----
    let mut ledger = Chain::new();
    ledger.init();

    // ---- Banner ----
    log.puts("ferros v0.0 on Fairphone 5 (QCM6490)\n");
    log.puts("=====================================\n\n");

    // ---- Boot diagnostics ----
    ledger.post(&Event::BootStarted {
        el: boot_el() as u32,
        sctlr: boot_sctlr() as u32,
        dtb_addr,
    });
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
    if log.ram.is_some() {
        log.puts("OK (ramoops)\n");
        // TODO: get actual base/size from ramoops config
        ledger.post(&Event::BootPstoreFound { base: 0, size: 0 });
    } else {
        log.puts("not found\n");
    }

    // ---- DPU MMIO probe ----
    let exc_pre = exception_count();
    let hw_ver = dpu::hw_version();
    let exc_post = exception_count();
    let dpu_ok = exc_post == exc_pre;

    log.puts("\n-- DPU --\n");
    log.puts("MDSS HW_VER:   "); log.put_hex32(hw_ver);
    log.puts(if dpu_ok { " (OK)\n" } else { " (FAULT)\n" });
    ledger.post(&Event::BootDpuProbed { hw_ver, ok: dpu_ok });

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
    let uart_ok = uart.probe() && exception_count() == exc_pre;
    if uart_ok {
        uart.init();
        log.puts("UART:          OK\n");
        uart.puts("ferros v0.0 UART alive\r\n");
    } else {
        log.puts("UART:          FAIL\n");
    }
    ledger.post(&Event::BootUartProbed { ok: uart_ok });

    // ---- DTB parse ----
    log.puts("\n-- DTB --\n");
    if dtb_addr != 0 {
        let dtb = unsafe { Dtb::from_ptr(dtb_addr as *const u8) };
        match dtb {
            Some(dtb) => {
                log.puts("DTB valid:     "); log.put_hex32(dtb.total_size() as u32); log.puts(" bytes\n");
                ledger.post(&Event::BootDtbParsed { total_size: dtb.total_size() as u32 });

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

    // ---- GCC SDC2 clock init ----
    log.puts("\n-- GCC SDC2 clocks --\n");
    {
        // Pre-enable diagnostics
        log.puts("BCR pre:       "); log.put_hex32(ferros_hal::gcc::sdc2_bcr_raw()); log.puts("\n");
        let (ahb_raw, apps_raw) = ferros_hal::gcc::sdc2_cbcr_raw();
        log.puts("AHB_CBCR pre:  "); log.put_hex32(ahb_raw); log.puts("\n");
        log.puts("APPS_CBCR pre: "); log.put_hex32(apps_raw); log.puts("\n");
        log.puts("CMD_RCGR pre:  "); log.put_hex32(ferros_hal::gcc::sdc2_cmd_rcgr()); log.puts("\n");
        log.puts("CFG_RCGR pre:  "); log.put_hex32(ferros_hal::gcc::sdc2_cfg_rcgr()); log.puts("\n");

        // Full init: deassert reset, configure 400KHz, enable branches
        let (ahb_ok, apps_ok) = ferros_hal::gcc::sdc2_clock_init();
        log.puts("init 400KHz:   AHB="); log.put_hex32(ahb_ok as u32);
        log.puts(" APPS="); log.put_hex32(apps_ok as u32); log.puts("\n");

        // Post-enable diagnostics
        log.puts("BCR post:      "); log.put_hex32(ferros_hal::gcc::sdc2_bcr_raw()); log.puts("\n");
        let (ahb_raw, apps_raw) = ferros_hal::gcc::sdc2_cbcr_raw();
        log.puts("AHB_CBCR post: "); log.put_hex32(ahb_raw); log.puts("\n");
        log.puts("APPS_CBCR post:"); log.put_hex32(apps_raw); log.puts("\n");
        log.puts("CMD_RCGR post: "); log.put_hex32(ferros_hal::gcc::sdc2_cmd_rcgr()); log.puts("\n");
        log.puts("CFG_RCGR post: "); log.put_hex32(ferros_hal::gcc::sdc2_cfg_rcgr()); log.puts("\n");
    }

    // ---- SD card power via RPMh mailbox ----
    // SPMI arbiter blocks direct LDO access (EE ownership). Use RPMh TCS
    // to request LDO enable through the proper power management channel.
    log.puts("\n-- RPMh cmd-db --\n");
    {
        use ferros_hal::rpmh;

        // Look up LDO addresses in cmd-db
        let ldoc9_addr = rpmh::cmd_db_lookup(b"ldoc9");
        log.puts("ldoc9: ");
        match ldoc9_addr {
            Some(a) => { log.put_hex32(a); log.puts("\n"); }
            None => log.puts("NOT FOUND\n"),
        }

        let ldoc6_addr = rpmh::cmd_db_lookup(b"ldoc6");
        log.puts("ldoc6: ");
        match ldoc6_addr {
            Some(a) => { log.put_hex32(a); log.puts("\n"); }
            None => log.puts("NOT FOUND\n"),
        }

        // TCS pre-state diagnostics
        log.buf_only("TCS0 ctrl=");
        log.buf_put_hex32(rpmh::tcs0_control());
        log.buf_only(" irq=");
        log.buf_put_hex32(rpmh::tcs0_irq_status());
        log.buf_only(" cmd0_sts=");
        log.buf_put_hex32(rpmh::tcs0_cmd0_status());
        log.buf_only("\n");

        // If either found, enable via TCS
        if ldoc9_addr.is_some() || ldoc6_addr.is_some() {
            log.puts("RPMh TCS enable...\n");

            if let Some(addr) = ldoc9_addr {
                // vmmc — SD card power 2.95V
                let v_ok = rpmh::vrm_set_voltage(addr, 2950);
                log.buf_only("ldoc9 volt=");
                log.buf_put_hex32(v_ok as u32);
                let e_ok = rpmh::vrm_enable(addr);
                log.buf_only(" en=");
                log.buf_put_hex32(e_ok as u32);
                log.buf_only(" cmd0_sts=");
                log.buf_put_hex32(rpmh::tcs0_cmd0_status());
                log.buf_only("\n");
                if v_ok && e_ok {
                    log.puts("ldoc9 (vmmc): OK\n");
                } else {
                    log.puts("ldoc9 (vmmc): FAIL\n");
                }
            }

            if let Some(addr) = ldoc6_addr {
                // vqmmc — SD I/O voltage 1.8V
                let v_ok = rpmh::vrm_set_voltage(addr, 1800);
                log.buf_only("ldoc6 volt=");
                log.buf_put_hex32(v_ok as u32);
                let e_ok = rpmh::vrm_enable(addr);
                log.buf_only(" en=");
                log.buf_put_hex32(e_ok as u32);
                log.buf_only(" cmd0_sts=");
                log.buf_put_hex32(rpmh::tcs0_cmd0_status());
                log.buf_only("\n");
                if v_ok && e_ok {
                    log.puts("ldoc6 (vqmmc): OK\n");
                } else {
                    log.puts("ldoc6 (vqmmc): FAIL\n");
                }
            }

            // Delay for power rail stabilization
            for _ in 0..1_000_000u32 { unsafe { core::arch::asm!("nop") }; }
        }
    }

    // ---- SDHCI (microSD) card probe ----
    log.puts("\n-- SDHCI SDC2 --\n");
    {
        let tlmm = 0x0F10_0000usize;

        // 1. Configure TLMM SDC2 pads BEFORE probing
        //    All three pads share one register at TLMM + 0xB4000 (SC7280 pinctrl)
        //    Bit layout: DATA drv [2:0], CMD drv [5:3], CLK drv [8:6],
        //                DATA pull [10:9], CMD pull [12:11], CLK pull [15:14]
        //    Drive: (mA/2)-1. Pull: 0=none, 1=down, 2=keeper, 3=up
        let pack_pre = unsafe { ferros_hal::mmio::read32(tlmm + 0xB4000) };
        log.puts("TLMM pre:  "); log.put_hex32(pack_pre); log.puts("\n");

        let sdc2_cfg: u32 = (4 << 0)   // DATA drv = 10mA
                          | (4 << 3)    // CMD drv = 10mA
                          | (7 << 6)    // CLK drv = 16mA
                          | (3 << 9)    // DATA pull-up
                          | (3 << 11)   // CMD pull-up
                          | (0 << 13);  // CLK no-pull
        unsafe { ferros_hal::mmio::write32(tlmm + 0xB4000, sdc2_cfg) };
        let pack_post = unsafe { ferros_hal::mmio::read32(tlmm + 0xB4000) };
        log.puts("TLMM post: "); log.put_hex32(pack_post); log.puts("\n");

        // 2. Card detect — GPIO 91, active-low
        //    TLMM GPIO regs: base + gpio*0x1000, +0x00=CFG, +0x04=IN_OUT
        //    CFG: [3:2]=func(0=gpio), [1:0]=pull(3=up), [8:6]=drv
        let gpio91_cfg_addr = tlmm + 91 * 0x1000;
        let gpio91_cfg_pre = unsafe { ferros_hal::mmio::read32(gpio91_cfg_addr) };
        log.buf_only("GPIO91 CFG pre: "); log.buf_put_hex32(gpio91_cfg_pre); log.buf_only("\n");
        // Configure as GPIO input with pull-up: func=0, pull=3(up), drv=2mA(0)
        unsafe { ferros_hal::mmio::write32(gpio91_cfg_addr, 0x03) }; // pull-up, func=gpio
        // Read IN_OUT register bit 0 = input value
        let gpio91_val = unsafe { ferros_hal::mmio::read32(gpio91_cfg_addr + 0x04) };
        let card_detect = gpio91_val & 1 == 0; // active-low
        log.puts("CD GPIO91: "); log.put_hex32(gpio91_val);
        log.puts(if card_detect { " (CARD)\n" } else { " (EMPTY)\n" });

        // 3. Delay for pad config to stabilize
        for _ in 0..200_000u32 { unsafe { core::arch::asm!("nop") }; }

        // 4. Now probe with pads configured
        let sdc = ferros_hal::sdmmc::SdmmcController::new(FP5_SDC2_BASE);
        let probe = sdc.probe_card();

        log.puts("reset:  "); log.puts(if probe.reset_ok { "OK\n" } else { "FAIL\n" });
        log.puts("PWRCTL: pre="); log.put_hex32(probe.pwrctl_status_pre);
        log.puts(" post="); log.put_hex32(probe.pwrctl_status_post);
        log.puts(if probe.pwrctl_ack_ok { " ACK\n" } else { " NONE\n" });
        log.puts("PRESENT:"); log.put_hex32(probe.present_state); log.puts("\n");
        log.puts("CLK_CTL:"); log.put_hex32(probe.clock_ctrl as u32); log.puts("\n");
        log.puts("CAPS:   "); log.put_hex32(probe.caps); log.puts("\n");

        // Raw SDHCI register dump (buf-only for diag)
        log.buf_only("\nSDHCI raw regs (base=G#08804000):\n");
        {
            let sdc_base = FP5_SDC2_BASE;
            let std_offsets: [(usize, &str); 16] = [
                (0x00, "SDMA_ADDR   "),
                (0x04, "BLOCK_SZ/CNT"),
                (0x08, "ARGUMENT    "),
                (0x0C, "XFER/CMD    "),
                (0x10, "RESP0       "),
                (0x14, "RESP1       "),
                (0x18, "RESP2       "),
                (0x1C, "RESP3       "),
                (0x20, "BUF_DATA    "),
                (0x24, "PRESENT_ST  "),
                (0x28, "HOST_CTRL   "),
                (0x2C, "CLK/TIMEOUT "),
                (0x30, "INT_STATUS  "),
                (0x34, "INT_STAT_EN "),
                (0x38, "INT_SIG_EN  "),
                (0x3C, "AUTO_CMD_ERR"),
            ];
            for &(off, name) in std_offsets.iter() {
                let exc_pre = exception_count();
                let val = unsafe { ferros_hal::mmio::read32(sdc_base + off) };
                let exc_post = exception_count();
                log.buf_only("  "); log.buf_only(name); log.buf_only(" [");
                log.buf_put_hex32(off as u32); log.buf_only("]: ");
                if exc_post > exc_pre {
                    log.buf_only("FAULT\n");
                } else {
                    log.buf_put_hex32(val); log.buf_only("\n");
                }
            }
            for off in [0x40usize, 0x44, 0x48, 0x4C] {
                let exc_pre = exception_count();
                let val = unsafe { ferros_hal::mmio::read32(sdc_base + off) };
                let exc_post = exception_count();
                log.buf_only("  CAPS        [");
                log.buf_put_hex32(off as u32); log.buf_only("]: ");
                if exc_post > exc_pre {
                    log.buf_only("FAULT\n");
                } else {
                    log.buf_put_hex32(val); log.buf_only("\n");
                }
            }
            log.buf_only("  -- QC vendor --\n");
            for off_idx in 0..13u32 {
                let off = 0x200 + (off_idx as usize) * 4;
                let exc_pre = exception_count();
                let val = unsafe { ferros_hal::mmio::read32(sdc_base + off) };
                let exc_post = exception_count();
                log.buf_only("  VENDOR      [");
                log.buf_put_hex32(off as u32); log.buf_only("]: ");
                if exc_post > exc_pre {
                    log.buf_only("FAULT\n");
                } else {
                    log.buf_put_hex32(val); log.buf_only("\n");
                }
            }
        }

        log.puts("CMD0:   "); log.puts(if probe.cmd0_ok { "OK\n" } else { "FAIL\n" });
        log.puts("CMD8:   ");
        if probe.cmd8_ok {
            log.puts("OK resp="); log.put_hex32(probe.cmd8_resp); log.puts("\n");
        } else {
            log.puts("FAIL err="); log.put_hex32(probe.cmd8_err as u32); log.puts("\n");
        }
        log.puts("ACMD41: ");
        if probe.acmd41_ok {
            log.puts("OK tries="); log.put_hex32(probe.acmd41_tries);
            log.puts(" OCR="); log.put_hex32(probe.ocr); log.puts("\n");
        } else {
            log.puts("FAIL tries="); log.put_hex32(probe.acmd41_tries);
            log.puts(" OCR="); log.put_hex32(probe.ocr); log.puts("\n");
        }
        log.buf_only("pre-CMD2: PRESENT=");
        log.buf_put_hex32(probe.pre_cmd2_present);
        log.buf_only(" INT=");
        log.buf_put_hex32(probe.pre_cmd2_int as u32);
        log.buf_only(" PWRCTL=");
        log.buf_put_hex32(probe.pre_cmd2_pwrctl);
        log.buf_only("\n");
        log.puts("CMD2:   ");
        if probe.cmd2_ok {
            log.puts("OK\n");
            log.puts("CID:    ");
            log.put_hex32(probe.cid[3]); log.puts(" ");
            log.put_hex32(probe.cid[2]); log.puts(" ");
            log.put_hex32(probe.cid[1]); log.puts(" ");
            log.put_hex32(probe.cid[0]); log.puts("\n");
            let mid = ((probe.cid[3] >> 24) & 0xFF) as u8;
            log.puts("MFR:    "); log.put_hex32(mid as u32); log.puts("\n");
            let pn: [u8; 5] = [
                (probe.cid[3] & 0xFF) as u8,
                ((probe.cid[2] >> 24) & 0xFF) as u8,
                ((probe.cid[2] >> 16) & 0xFF) as u8,
                ((probe.cid[2] >> 8) & 0xFF) as u8,
                (probe.cid[2] & 0xFF) as u8,
            ];
            log.puts("NAME:   ");
            for &b in pn.iter() {
                if b >= 0x20 && b <= 0x7E {
                    log.putc(b);
                } else {
                    log.puts("?");
                }
            }
            log.puts("\n");
        } else {
            log.puts("FAIL err="); log.put_hex32(probe.cmd2_err as u32); log.puts("\n");
        }
        log.puts("CMD3:   ");
        if probe.cmd3_ok {
            log.puts("OK RCA="); log.put_hex32(probe.cmd3_resp >> 16); log.puts("\n");
        } else {
            log.puts("FAIL err="); log.put_hex32(probe.cmd3_err as u32); log.puts("\n");
        }
        // CSD register (speed, voltage, capacity)
        if probe.cmd9_ok {
            log.puts("CSD:    ");
            log.put_hex32(probe.csd[3]); log.puts(" ");
            log.put_hex32(probe.csd[2]); log.puts(" ");
            log.put_hex32(probe.csd[1]); log.puts(" ");
            log.put_hex32(probe.csd[0]); log.puts("\n");
            // SDHCI shifts R2 left 8 bits; CSD[3] bits [31:30] = CSD_STRUCTURE
            let csd_ver = (probe.csd[3] >> 30) & 0x3;
            // TRAN_SPEED is CSD byte 3 (bits [103:96])
            // After SDHCI 8-bit shift: in csd[3] bits [7:0] or csd[2] bits [31:24]
            let tran_speed = ((probe.csd[2] >> 24) & 0xFF) as u8;
            log.puts("  ver="); log.put_hex32(csd_ver);
            log.puts(" TRAN_SPEED="); log.put_hex32(tran_speed as u32);
            // Decode TRAN_SPEED: [2:0]=time_unit, [6:3]=time_value
            let unit = match tran_speed & 0x7 {
                0 => "100Kbit/s",
                1 => "1Mbit/s",
                2 => "10Mbit/s",
                3 => "100Mbit/s",
                _ => "?",
            };
            let mult = match (tran_speed >> 3) & 0xF {
                1 => "1.0",  6 => "2.5",
                2 => "1.2",  7 => "3.0",
                3 => "1.3",  8 => "3.5",
                4 => "1.5",  9 => "4.0",
                5 => "2.0", 10 => "4.5",
                11 => "5.0", 12 => "5.5",
                13 => "6.0", 14 => "7.0",
                15 => "8.0", _ => "?",
            };
            log.puts(" ("); log.puts(mult); log.puts("x"); log.puts(unit); log.puts(")\n");
            // CSD v2: C_SIZE is bits [69:48] = 22 bits
            if csd_ver >= 1 {
                // After 8-bit shift: C_SIZE spans csd[1] bits [29:8] and csd[0]
                let c_size = ((probe.csd[1] & 0x3FFFFF00) >> 8)
                           | ((probe.csd[0] >> 24) & 0xFF);
                // Capacity = (C_SIZE + 1) * 512KB
                let cap_mb = ((c_size as u64) + 1) / 2; // in MB
                log.puts("  C_SIZE="); log.put_hex32(c_size);
                log.puts(" cap="); log.put_hex32(cap_mb as u32); log.puts("MB\n");
            }
        }

        log.puts("CMD7:   ");
        if probe.cmd7_ok {
            log.puts("OK (selected)\n");
        } else {
            log.puts("FAIL err="); log.put_hex32(probe.cmd7_err as u32); log.puts("\n");
        }
        if probe.cmd7_ok {
            log.puts("CMD16:  "); log.puts(if probe.cmd16_ok { "OK\n" } else { "FAIL\n" });
            log.puts("BUS4:   "); log.puts(if probe.bus4_ok { "OK (4-bit)\n" } else { "FAIL (1-bit)\n" });
            log.puts("CLK25:  "); log.puts(if probe.clk25_ok { "OK (25MHz)\n" } else { "FAIL\n" });
            log.puts("READ0:  ");
            if probe.read_ok {
                log.puts("OK sig=");
                log.put_hex32(probe.block0_sig[0] as u32); log.puts(" ");
                log.put_hex32(probe.block0_sig[1] as u32); log.puts("\n");
                log.puts("BLK0:   ");
                for i in 0..16 {
                    log.put_hex32(probe.block0_head[i] as u32);
                    if i < 15 { log.puts(" "); }
                }
                log.puts("\n");
            } else {
                log.puts("FAIL err="); log.put_hex32(probe.read_err as u32); log.puts("\n");
            }
            log.puts("WRITE1: ");
            if probe.write_ok {
                log.puts("OK\n");
                log.puts("VERIFY: ");
                if probe.verify_ok {
                    log.puts("OK (FERROS pattern)\n");
                } else {
                    log.puts("MISMATCH err="); log.put_hex32(probe.verify_err as u32); log.puts("\n");
                }
            } else {
                log.puts("FAIL err="); log.put_hex32(probe.write_err as u32); log.puts("\n");
            }
        }
    }

    // ---- SPMI full APID map dump (find ALL peripherals) ----
    log.buf_only("\n-- SPMI ALL APIDs --\n");
    {
        let apid_map = 0x0C44_0000usize + 0x2000;
        for n in 0..1024u32 {
            let exc_pre = exception_count();
            let entry = unsafe { ferros_hal::mmio::read32(apid_map + 4 * n as usize) };
            let exc_post = exception_count();
            if exc_post > exc_pre { break; }
            if entry == 0 { continue; }
            let ppid = ((entry >> 8) & 0xFFF) as u16;
            let sid = (ppid >> 8) as u8;
            let pid = (ppid & 0xFF) as u8;
            log.buf_only("  A"); log.buf_put_hex32(n);
            log.buf_only(" S"); log.buf_put_hex32(sid as u32);
            log.buf_only(" P"); log.buf_put_hex32(pid as u32);
            log.buf_only(" "); log.buf_put_hex32(entry);
            log.buf_only("\n");
        }
    }

    // ---- IMEM probe (reboot reason) ----
    log.buf_only("\n-- IMEM G#146AA000 --\n");
    {
        let imem_base = 0x146A_A000usize;
        for &off in [0x0usize, 0x4, 0x65C, 0x660, 0x664].iter() {
            let exc_pre = exception_count();
            let val = unsafe { ferros_hal::mmio::read32(imem_base + off) };
            let exc_post = exception_count();
            log.buf_only("  ["); log.buf_put_hex32(off as u32); log.buf_only("]: ");
            if exc_post > exc_pre {
                log.buf_only("FAULT\n");
            } else {
                log.buf_put_hex32(val); log.buf_only("\n");
            }
        }
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
        log.puts("mode:          "); log.put_hex32((usb_info.gctl >> 12) & 0x3); log.puts("\n");
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
        ledger.post(&Event::BootUsbInitStarted);
        match ferros_hal::usb::Dwc3Dev::init() {
            Some(mut usb) => {
                log.puts("OK\n");
                ledger.post(&Event::BootUsbInitDone { ok: true });

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

                let mut evt_count = 0u32;
                let mut poll_count = 0u32;
                // PT inbound state — allocated on SPEC arrival, freed on COMPLETE
                let mut pt_data_buf: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                let mut pt_bitmap_buf: alloc::vec::Vec<ferros_pt::BitmapWord> = alloc::vec::Vec::new();
                let mut pt_inbound: Option<InboundTransfer<'_>> = None;
                // Current seq_width for DATA parsing (0 = no active transfer)
                let mut pt_seq_width: usize = 0;
                // Pending COMPLETE packet to send after last ACK flushes
                let mut pt_complete_pending = [0u8; 128];
                let mut pt_complete_len: usize = 0;
                // PT outbound state — device→host response transfer
                let mut pt_out_data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                let mut pt_out_bitmap: alloc::vec::Vec<ferros_pt::BitmapWord> = alloc::vec::Vec::new();
                let mut pt_outbound: Option<OutboundTransfer<'_>> = None;
                // Outbound SPEC queued after inbound COMPLETE (sent on second ep3)
                let mut pt_out_spec_pending = [0u8; 512];
                let mut pt_out_spec_len: usize = 0;
                // Precomputed dev cap hashes
                let cap_diag = ferros_pt::command::dev_cap(ferros_pt::command::caps::DIAG);
                let cap_mem = ferros_pt::command::dev_cap(ferros_pt::command::caps::MEM);
                let cap_reboot = ferros_pt::command::dev_cap(ferros_pt::command::caps::REBOOT);
                let cap_reload = ferros_pt::command::dev_cap(ferros_pt::command::caps::RELOAD);
                // Staging area for hot-reload kernel image
                const RELOAD_STAGE: usize = 0xA000_0000;
                let mut reload_size: usize = 0;
                // Flag: outbound DATA pump needs to send next packet
                let mut pt_out_pump = false;

                loop {
                    match usb.poll_event() {
                        ferros_hal::usb::UsbEvent::None => {}
                        ferros_hal::usb::UsbEvent::Reset => {
                            log.puts("  USB reset\n");
                            usb.handle_reset();
                            // Clear all PT state on USB reset
                            pt_inbound = None;
                            pt_outbound = None;
                            pt_seq_width = 0;
                            pt_complete_len = 0;
                            pt_out_spec_len = 0;
                            pt_out_data.clear();
                            pt_out_bitmap.clear();
                            ledger.post(&Event::UsbReset);
                            evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::ConnectDone { speed } => {
                            log.puts("  connected: ");
                            log.put_hex32(speed);
                            log.puts(" DSTS=");
                            log.put_hex32(unsafe { ferros_hal::mmio::read32(0x0A60C70C) });
                            log.puts(" PHY=");
                            log.put_hex32(unsafe { ferros_hal::mmio::read32(0x0A60C200) });
                            log.puts("\n");
                            usb.handle_connect_done();
                            usb.ep0_start_setup();
                            usb.bulk_out_arm(); // Initialize TRB ring
                            ledger.post(&Event::UsbConnectDone { speed });
                            evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::Disconnect => {
                            log.puts("  disconnected\n");
                            ledger.post(&Event::UsbDisconnect);
                            evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::Ep0Setup { request } => {
                            log.puts("  SETUP: ");
                            for i in 0..8 {
                                log.put_hex32(request[i] as u32);
                                log.puts(" ");
                            }
                            log.puts("\n");
                            ledger.post(&Event::UsbSetupPacket {
                                bm_request_type: request[0],
                                b_request: request[1],
                                w_value: (request[3] as u16) << 8 | request[2] as u16,
                                w_index: (request[5] as u16) << 8 | request[4] as u16,
                                w_length: (request[7] as u16) << 8 | request[6] as u16,
                            });
                            if !usb.handle_setup(&request) {
                                log.puts("  (stall)\n");
                                usb.ep0_stall();
                                ledger.post(&Event::UsbSetupStall);
                            } else {
                                // Log diagnostic readback for data transfers
                                if usb.last_send_len > 0 {
                                    log.puts("  -> len=");
                                    log.put_hex32(usb.last_send_len as u32);
                                    log.puts(" src@");
                                    log.put_hex32(usb.last_send_src_addr);
                                    log.puts("=");
                                    log.put_hex32(usb.last_send_src_preview);
                                    log.puts(" dst@");
                                    log.put_hex32(usb.last_send_buf_addr);
                                    log.puts("=");
                                    log.put_hex32(usb.last_send_preview);
                                    log.puts("\n");
                                }
                            }
                            evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::TransferComplete { ep } => {
                            if ep == 2 {
                                // Bulk OUT complete — PT packet handler
                                let mut tmp = [0u8; 512];
                                let mut n = 0usize;
                                if let Some(data) = usb.bulk_out_read() {
                                    n = data.len().min(512);
                                    tmp[..n].copy_from_slice(&data[..n]);
                                }
                                if n > 0 {
                                    ledger.post(&Event::UsbBulkRx { len: n as u32 });
                                    let mut resp = [0u8; 512];

                                    // Debug: force to screen
                                    log.screen = false; // was true
                                    log.puts("RX[");
                                    log.put_hex32(tmp[0] as u32);
                                    log.puts("] n=");
                                    log.put_hex32(n as u32);
                                    log.puts(" ctrl=");
                                    log.put_hex32(ferros_pt::is_control_packet(tmp[0]) as u32);
                                    log.puts("\n");
                                    log.screen = false;

                                    // Debug: show first byte + length for first few packets
                                    static mut PKT_DBG: u32 = 0;
                                    unsafe { PKT_DBG += 1; }
                                    if unsafe { PKT_DBG } <= 5 {
                                        log.screen = true;
                                        log.puts("["); log.put_hex32(tmp[0] as u32);
                                        log.puts(" n="); log.put_hex32(n as u32);
                                        log.puts("]");
                                        log.screen = false;
                                    }

                                    if ferros_pt::is_data_packet(tmp[0]) {
                                        // DATA packet — silent receive, no ACK
                                        if let Some(ref mut xfer) = pt_inbound {
                                            if let Some((_sid, seq, chunk_hash, payload)) = packet::decode_data(&tmp[..n], pt_seq_width) {
                                                xfer.handle_data(seq, &chunk_hash, payload);
                                                ledger.post(&Event::PtDataRx {
                                                    sid: xfer.sid.0,
                                                    seq,
                                                    len: payload.len() as u16,
                                                });
                                                // Log first packet + near completion + bad chunks
                                                if xfer.chunks_received <= 1 || xfer.bad_chunks > 0 || xfer.chunks_received >= xfer.expected_count.saturating_sub(1) {
                                                    log.screen = true;
                                                    log.puts("R"); log.put_hex32(xfer.chunks_received as u32);
                                                    log.puts("/"); log.put_hex32(xfer.expected_count as u32);
                                                    log.puts("B"); log.put_hex32(xfer.bad_chunks as u32);
                                                    log.puts("\n");
                                                    log.screen = false;
                                                }
                                                if xfer.all_received() {
                                                    let mut complete_buf = [0u8; 512];
                                                    let clen = xfer.finish(&mut complete_buf);
                                                    if clen > 0 {
                                                        let ok = usb.bulk_in_send(&complete_buf[..clen]);
                                                        log.puts("  PT COMPLETE sid=");
                                                        log.put_hex32(xfer.sid.0 as u32);
                                                        log.puts(" total=");
                                                        log.put_hex32(xfer.data_len as u32);
                                                        log.puts("\n");
                                                        ledger.post(&Event::PtComplete {
                                                            sid: xfer.sid.0,
                                                            success: true,
                                                            total: xfer.data_len as u64,
                                                        });
                                                        // Command dispatch on completed payload
                                                        let payload = xfer.payload();
                                                        if let Some(cmd) = ferros_pt::command::parse(payload) {
                                                            if cmd.cap == cap_diag && cmd.op == ferros_pt::Op::Read {
                                                                // DIAG Read: send boot log back via PT
                                                                // Copy log into pt_out_data, then clear log
                                                                // to prevent unbounded heap growth
                                                                pt_out_data.clear();
                                                                pt_out_data.extend_from_slice(&log.buf);
                                                                log.buf.clear();
                                                            } else if cmd.cap == cap_mem && cmd.op == ferros_pt::Op::Read {
                                                                // MEM Read: params = [addr:8][len:4]
                                                                if cmd.params.len() >= 12 {
                                                                    let addr = u64::from_be_bytes([
                                                                        cmd.params[0], cmd.params[1],
                                                                        cmd.params[2], cmd.params[3],
                                                                        cmd.params[4], cmd.params[5],
                                                                        cmd.params[6], cmd.params[7],
                                                                    ]);
                                                                    let len = u32::from_be_bytes([
                                                                        cmd.params[8], cmd.params[9],
                                                                        cmd.params[10], cmd.params[11],
                                                                    ]) as usize;
                                                                    let len = len.min(4096);
                                                                    // Round to 4-byte aligned reads (MMIO requires word access)
                                                                    let aligned_addr = (addr as usize) & !3;
                                                                    let skip = (addr as usize) - aligned_addr;
                                                                    let word_count = (skip + len + 3) / 4;
                                                                    pt_out_data = alloc::vec![0u8; len];
                                                                    for w in 0..word_count {
                                                                        let val = unsafe {
                                                                            core::ptr::read_volatile((aligned_addr + w * 4) as *const u32)
                                                                        };
                                                                        let bytes = val.to_le_bytes();
                                                                        for b in 0..4 {
                                                                            let src_off = w * 4 + b;
                                                                            if src_off >= skip && src_off < skip + len {
                                                                                pt_out_data[src_off - skip] = bytes[b];
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            } else if cmd.cap == cap_reload && cmd.op == ferros_pt::Op::Write {
                                                                // RELOAD Write: payload is the new kernel binary
                                                                // Copy into staging DRAM
                                                                let data = cmd.params;
                                                                let dst = unsafe {
                                                                    core::slice::from_raw_parts_mut(
                                                                        (RELOAD_STAGE + reload_size) as *mut u8,
                                                                        data.len(),
                                                                    )
                                                                };
                                                                dst.copy_from_slice(data);
                                                                reload_size += data.len();
                                                                log.buf_only("RELOAD chunk ");
                                                                log.buf_put_hex32(data.len() as u32);
                                                                log.buf_only(" total=");
                                                                log.buf_put_hex32(reload_size as u32);
                                                                log.buf_only("\n");
                                                                // Response: current total size
                                                                pt_out_data.clear();
                                                                pt_out_data.extend_from_slice(&(reload_size as u32).to_le_bytes());
                                                            } else if cmd.cap == cap_reload && cmd.op == ferros_pt::Op::Exec {
                                                                // RELOAD Exec: jump to staged kernel
                                                                log.buf_only("RELOAD exec size=");
                                                                log.buf_put_hex32(reload_size as u32);
                                                                log.buf_only("\n");
                                                                if reload_size > 0x1000 {
                                                                    // Validate: check for ARM64 magic or MZ header
                                                                    let magic = unsafe { *(RELOAD_STAGE as *const u32) };
                                                                    if magic == 0x91005A4D { // MZ header
                                                                        hot_reload(RELOAD_STAGE, dtb_addr);
                                                                    }
                                                                }
                                                            } else if cmd.cap == cap_reboot && cmd.op == ferros_pt::Op::Exec {
                                                                // REBOOT Exec: param[0] selects mode
                                                                // 0x00 = normal reboot, 0x01 = fastboot
                                                                let mode = cmd.params.first().copied().unwrap_or(0);
                                                                log.buf_only("REBOOT mode=");
                                                                log.buf_put_hex32(mode as u32);
                                                                log.buf_only("\n");
                                                                if mode == 0x01 {
                                                                    psci_reboot_fastboot();
                                                                } else {
                                                                    psci_reboot();
                                                                }
                                                            }
                                                            // Start outbound PT if we have response data
                                                            pt_outbound = None; // release borrow on pt_out_bitmap
                                                            if !pt_out_data.is_empty() {
                                                                let sid = ferros_pt::StreamId(xfer.sid.0);
                                                                let mut spec_buf = [0u8; 128];
                                                                if let Some((out_xfer, spec_len)) =
                                                                    OutboundTransfer::start_vec(
                                                                        sid,
                                                                        &pt_out_data,
                                                                        &mut pt_out_bitmap,
                                                                        &mut spec_buf,
                                                                    )
                                                                {
                                                                    log.puts("  PT OUT SPEC count=");
                                                                    log.put_hex32(out_xfer.count as u32);
                                                                    log.puts(" total=");
                                                                    log.put_hex32(out_xfer.total as u32);
                                                                    log.puts("\n");
                                                                    pt_outbound = Some(out_xfer);
                                                                    // Queue outbound SPEC — sent after COMPLETE
                                                                    pt_out_spec_pending[..spec_len]
                                                                        .copy_from_slice(&spec_buf[..spec_len]);
                                                                    pt_out_spec_len = spec_len;
                                                                }
                                                            }
                                                        }
                                                        pt_seq_width = 0;
                                                    }
                                                }
                                            }
                                        }
                                    } else if ferros_pt::is_control_packet(tmp[0]) {
                                        // Control packet — single-byte tag dispatch
                                        if let Some(spec) = Spec::decode(&tmp[..n]) {
                                            // SPEC — new inbound transfer (clears stale state)
                                            // New session — log + cancel stale state
                                            log.screen = false; // was true
                                            log.puts("NEW idle="); log.put_hex32(usb.bulk_in_idle as u32);
                                            log.puts("\n");
                                            log.screen = false;
                                            if !usb.bulk_in_idle {
                                                usb.cancel_bulk_in();
                                            }
                                            pt_outbound = None;
                                            pt_out_data.clear();
                                            pt_out_bitmap.clear();
                                            pt_out_spec_len = 0;
                                            log.puts("  PT SPEC sid=");
                                            log.put_hex32(spec.sid.0 as u32);
                                            log.puts(" count=");
                                            log.put_hex32(spec.count as u32);
                                            log.puts(" total=");
                                            log.put_hex32(spec.total as u32);
                                            log.puts("\n");

                                            pt_data_buf = alloc::vec![0u8; spec.total as usize];
                                            pt_bitmap_buf = alloc::vec![0u64; bitmap_words(spec.count)];

                                            let data_slice = unsafe {
                                                core::slice::from_raw_parts_mut(
                                                    pt_data_buf.as_mut_ptr(),
                                                    pt_data_buf.len(),
                                                )
                                            };
                                            let bitmap_slice = unsafe {
                                                core::slice::from_raw_parts_mut(
                                                    pt_bitmap_buf.as_mut_ptr(),
                                                    pt_bitmap_buf.len(),
                                                )
                                            };

                                            match InboundTransfer::new(&spec, data_slice, bitmap_slice) {
                                                Some(xfer) => {
                                                    pt_seq_width = xfer.seq_width;
                                                    ledger.post(&Event::PtSpecRx {
                                                        sid: spec.sid.0,
                                                        count: spec.count,
                                                        total: spec.total,
                                                    });
                                                    pt_inbound = Some(xfer);
                                                    let ack = Ack {
                                                        sid: spec.sid,
                                                        seq: u64::MAX,
                                                    };
                                                    let ack_len = ack.encode(&mut resp);
                                                    usb.bulk_in_send(&resp[..ack_len]);
                                                }
                                                None => {
                                                    log.puts("  PT NAK: alloc failed\n");
                                                }
                                            }
                                        } else if let Some(_ack) = Ack::decode(&tmp[..n]) {
                                            // ACK — currently unused (outbound blast is auto)
                                        } else if let Some(fin_sid) = packet::decode_fin(&tmp[..n]) {
                                            // FIN from bridge — blast is done, check what we have
                                            if let Some(ref mut xfer) = pt_inbound {
                                                if xfer.sid == fin_sid {
                                                    let mut complete_buf = [0u8; 128];
                                                    let clen = xfer.finish(&mut complete_buf);
                                                    if clen > 0 {
                                                        usb.bulk_in_send(&complete_buf[..clen]);
                                                    }
                                                }
                                            }
                                        } else if let Some(complete) = Complete::decode(&tmp[..n]) {
                                            // COMPLETE from bridge — outbound transfer done
                                            if let Some(ref mut out) = pt_outbound {
                                                let ok = out.handle_complete(&complete);
                                                log.puts("  PT OUT DONE ok=");
                                                log.put_hex32(ok as u32);
                                                log.puts("\n");
                                                pt_outbound = None;
                                                pt_out_data.clear();
                                                pt_out_bitmap.clear();
                                            }
                                        }
                                    } else {
                                        // Raw echo — bounce back for testing
                                        if usb.bulk_in_idle {
                                            usb.bulk_in_send(&tmp[..n]);
                                        }
                                    }
                                }
                                // Only advance ring consumer if we actually got data
                                if n > 0 {
                                    usb.bulk_out_arm();
                                }
                            }
                            if ep == 3 {
                                // Bulk IN complete — chain next pending send
                                log.screen = false; // was true
                                log.puts("E3 c="); log.put_hex32(pt_complete_len as u32);
                                log.puts(" s="); log.put_hex32(pt_out_spec_len as u32);
                                log.puts(" o="); log.put_hex32(pt_outbound.is_some() as u32);
                                log.puts("\n");
                                log.screen = false;
                                if pt_complete_len > 0 {
                                    // Inbound COMPLETE
                                    usb.bulk_in_send(&pt_complete_pending[..pt_complete_len]);
                                    pt_complete_len = 0;
                                    pt_inbound = None;
                                } else if pt_out_spec_len > 0 {
                                    // Outbound SPEC
                                    usb.bulk_in_send(&pt_out_spec_pending[..pt_out_spec_len]);
                                    pt_out_spec_len = 0;
                                }
                                // Outbound DATA is handled by the main loop idle check
                                ledger.post(&Event::UsbBulkTxComplete);
                            }
                            evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::TransferNotReady { .. } => {
                            evt_count += 1;
                        }
                    }

                    // (periodic re-arm removed — interferes with active transfers)

                    // Outbound DATA pump — poll bulk_in_idle directly.
                    if usb.bulk_in_idle {
                        if let Some(ref mut out) = pt_outbound {
                            if !out.all_sent() {
                                let mut pkt = [0u8; 512];
                                let pkt_len = out.next_data_packet(&pt_out_data, &mut pkt);
                                if pkt_len > 0 {
                                    let ok = usb.bulk_in_send(&pkt[..pkt_len]);
                                    // Debug first few sends
                                    if out.next_seq <= 3 || !ok {
                                        log.screen = false; // was true
                                        log.puts("D"); log.put_hex32(out.next_seq as u32);
                                        if ok { log.puts("ok "); } else { log.puts("FAIL "); }
                                        log.screen = false;
                                    }
                                }
                            } else {
                                // All DATA sent — no FIN for outbound response.
                                // Bridge knows chunk count from SPEC. Sending FIN
                                // would leave a stale IN transfer if bridge already
                                // returned after all_received().
                                pt_outbound = None;
                            }
                        }
                    }

                    poll_count = poll_count.wrapping_add(1);
                    // Print status every ~10M polls
                    if poll_count & 0x00FF_FFFF == 0 {
                        log.puts("  SU "); log.put_hex32(usb.setup_count);
                        log.puts(" bR "); log.put_hex32(usb.last_setup_brequest as u32);
                        log.puts(" wV "); log.put_hex32(usb.last_setup_wvalue as u32);
                        log.puts(" X1 "); log.put_hex32(usb.ep1_xfer_complete);
                        log.puts(" OK "); log.put_hex32(usb.ep1_start_ok);
                        log.puts(" F "); log.put_hex32(usb.ep1_start_fail);
                        log.puts("\n  SA "); log.put_hex32(usb.ep0_setup_arm_ok);
                        log.puts("/"); log.put_hex32(usb.ep0_setup_arm_fail);
                        log.puts(" SO "); log.put_hex32(usb.ep0_status_out_arm_fail);
                        log.puts(" NR "); log.put_hex32(usb.ep0_xfer_notready);
                        log.puts(" EV "); log.put_hex32(usb.last_evt_raw);
                        if usb.cmd_status_fail > 0 {
                            log.puts("\n  CS "); log.put_hex32(usb.cmd_status_fail);
                            log.puts(" CMD="); log.put_hex32(usb.last_cmd_status);
                            log.puts(" ep="); log.put_hex32(usb.last_cmd_ep as u32);
                            log.puts(" ty="); log.put_hex32(usb.last_cmd_type);
                        }
                        log.puts("\n");
                    }
                }
            }
            None => {
                log.puts("FAIL (reset timeout)\n");
                ledger.post(&Event::BootUsbInitDone { ok: false });
            }
        }
    } else if usb_ok {
        log.puts("DWC3:          bad SNPSID "); log.put_hex32(usb_info.snpsid); log.puts("\n");
    } else {
        log.puts("DWC3:          FAULT (clocks gated?)\n");
    }

    // ---- Final ----
    ledger.post(&Event::BootHalted);
    log.puts("\nTotal exc:     "); log.put_hex(exception_count()); log.puts("\n");
    log.puts("Ledger:        "); log.put_hex32(ledger.total_entries() as u32);
    log.puts(" entries, seq "); log.put_hex32(ledger.global_seq() as u32); log.puts("\n");
    if let Some(ref r) = log.ram {
        log.con.puts("pstore bytes:  "); log.con.put_hex32(r.written() as u32); log.con.puts("\n");
    }

    log.puts("\n-- halted --\n");
    loop { unsafe { core::arch::asm!("wfe") }; }
}

/// Hot-reload: jump to a new kernel image at the given DRAM address.
///
/// The new image is position-independent (uses adrp). We disable caches,
/// flush the staging area, then branch to _start with x0 = DTB.
fn hot_reload(stage_addr: usize, dtb: u64) -> ! {
    unsafe {
        core::arch::asm!(
            // Disable interrupts
            "msr daifset, #0xF",
            // Clean + invalidate D-cache for staging area (1MB should cover it)
            "mov x2, {stage}",
            "mov x3, #0x100000",
            "add x3, x3, x2",
            "2:",
            "dc civac, x2",
            "add x2, x2, #64",
            "cmp x2, x3",
            "b.lo 2b",
            "dsb sy",
            "isb",
            // Invalidate I-cache
            "ic iallu",
            "dsb sy",
            "isb",
            // Set up args: x0 = DTB pointer (what ABL passes)
            "mov x0, {dtb}",
            "mov x1, xzr",
            "mov x2, xzr",
            "mov x3, xzr",
            // Jump to new image entry point
            "br {stage}",
            stage = in(reg) stage_addr as u64,
            dtb = in(reg) dtb,
            options(noreturn),
        );
    }
}

/// PSCI SYSTEM_RESET via SMC — warm reboot that preserves DRAM.
fn psci_reboot() -> ! {
    unsafe {
        core::arch::asm!(
            "movz x0, #0x0009",
            "movk x0, #0x8400, lsl #16",  // x0 = 0x84000009 (SYSTEM_RESET)
            "mov x1, xzr",
            "mov x2, xzr",
            "mov x3, xzr",
            "smc #0",
            options(noreturn)
        );
    }
}

/// Reboot into fastboot via PMK8350 SDAM_2 restart reason register.
///
/// SDAM_2 is at SPMI SID 8 (not 0!), PID 0x71 → PPID 0x0871, APID 0x122.
/// Direct SPMI write returns success but value doesn't stick — try SCM IO
/// write to the SPMI arbiter channel registers instead (TZ privilege).
fn psci_reboot_fastboot() -> ! {
    // APID 0x122 write channel: CHNLS_BASE + 0x122 * 0x1000 = 0x0C722000
    const SDAM2_CH: usize = 0x0C60_0000 + 0x122 * 0x1000;

    // Method 1: SCM IO write through SPMI arbiter channel (TZ privilege)
    // Write WDATA0 = 0x04 (FASTBOOT_MODE=0x02 << 1)
    scm_io_write(SDAM2_CH + 0x10, 0x04);
    // Write CMD: EXT_WRITEL opcode=0, reg_offset=0x48, 1 byte
    scm_io_write(SDAM2_CH + 0x00, (0x48u32 << 4) | 0);
    // Small delay for SPMI transaction
    for _ in 0..100_000u32 { unsafe { core::arch::asm!("nop") }; }

    // Method 2: Direct SPMI write with SEC_ACCESS unlock
    use ferros_hal::spmi;
    let sdam2_ppid = spmi::ppid(8, 0x71);
    if let Some(apid) = spmi::find_apid(sdam2_ppid) {
        // Unlock protected registers: write 0xA5 to SEC_ACCESS (0xD0)
        spmi::write_byte(apid, 0xD0, 0xA5);
        // Write fastboot reason
        spmi::write_byte(apid, 0x48, 0x04);
        // Also try unshifted value
        spmi::write_byte(apid, 0xD0, 0xA5);
        spmi::write_byte(apid, 0x48, 0x02);
    }

    // Method 3: IMEM (probably won't help but costs nothing)
    unsafe {
        let imem = 0x146A_A65C as *mut u32;
        core::ptr::write_volatile(imem, 0x7766_5500u32);
        core::arch::asm!("dsb sy");
    }

    psci_reboot();
}

/// SCM IO write: TrustZone-privileged write to a physical address.
/// SMC64 fast call: SVC_IO(5), CMD_WRITE(2).
fn scm_io_write(addr: usize, val: u32) {
    unsafe {
        core::arch::asm!(
            "mov x0, {func}",
            "mov x1, #2",
            "mov x2, {addr}",
            "mov x3, {val}",
            "smc #0",
            func = in(reg) 0xC200_0502u64,
            addr = in(reg) addr as u64,
            val = in(reg) val as u64,
            out("x0") _, out("x1") _, out("x2") _, out("x3") _,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
        );
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop { unsafe { core::arch::asm!("wfe") }; }
}
