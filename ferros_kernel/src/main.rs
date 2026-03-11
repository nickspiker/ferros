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
/// GENI SE UART base (QUPv3 SE3, from stock cmdline console=ttyMSM0).
const FP5_UART_BASE: usize = 0x0099_4000;

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

    // ---- Final ----
    log.puts("\nTotal exc:     "); log.put_hex(exception_count()); log.puts("\n");
    if let Some(ref r) = log.ram {
        log.con.puts("pstore bytes:  "); log.con.put_hex32(r.written() as u32); log.con.puts("\n");
    }

    // ---- PSCI reboot (warm — preserves ramoops DRAM) ----
    log.puts("\n-- rebooting --\n");
    unsafe {
        core::arch::asm!("dsb sy");
        core::arch::asm!(
            "mov w0, #0x9",
            "movk w0, #0x8400, lsl #16",
            "mov x1, xzr",
            "mov x2, xzr",
            "mov x3, xzr",
            "smc #0",
            options(noreturn)
        );
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
