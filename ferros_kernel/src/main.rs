// FERROS SOURCE MAP — keep updated when pub items or files change
//
// ferros_kernel/ └── main.rs ── kernel entry, boot sequence, USB event loop _start (asm), kernel_main() Boot: EL check, DTB parse, FB console, USB DWC3 init USB event loop: PT command dispatch, hot-reload handler
//
// ferros_hal/ ── hardware abstraction (no_std, Pixel 8 Exynos + shared) ├── lib.rs ── module re-exports ├── mmio.rs ── raw MMIO read/write (8/16/32-bit), cache ops ├── console.rs ── framebuffer text console, 8x16 VGA font, 2x scaling │   struct Console { fb_base, stride, x_off, y_off, col, row } │     ::new(), put_char(), scroll(), clear() ├── fb.rs ── raw framebuffer pixel ops ├── dtb.rs ── FDT/DTB parser (find nodes, read properties) ├── uart.rs ── GENI UART TX ├── pstore.rs ── ramoops/persistent_ram_buffer writer ├── ufs.rs ── UFS host controller (UFSHCI v3.0) ├── hsi2c.rs ── Exynos HSI2C master (eUSB repeater) └── usb.rs ── DWC3 USB device controller struct Dwc3Dev { evt_read_idx, ep0_state, bulk_out/in state } ::new() → init, CSFTRST, PHY, endpoint config, Run/Stop ::poll_event() → UsbEvent (Reset, ConnectDone, TransferComplete) ::bulk_out_arm(), bulk_out_read() → &[u8] ::bulk_in_send(data) → bool (ISP_IMI for short packets) ::ep0_send(), ep0_status_in/out(), handle_setup() pub fn probe() → Dwc3Info, dump_diag() → Dwc3Diag pub fn phy_init(), smmu_bypass()
//
// ferros_pt/ ── Photon Transport (no_std, optional alloc) ├── lib.rs ── is_data_packet(), is_control_packet(), re-exports ├── packet.rs ── packet encode/decode │   TAG_SPEC='S', TAG_ACK='A', TAG_NAK='N', TAG_DONE='D', TAG_FIN='F' │   struct Spec, Ack, Nak, Complete { sid, fields... } │   encode_data(), decode_data() — per-chunk BLAKE3 hash ├── transfer.rs ── transfer state machines │   struct InboundTransfer { data, received bitmap, expected_count } │     ::new(), handle_data(), all_received(), finish() → COMPLETE/NAK │   struct OutboundTransfer { data, next_seq, count, psize } │     ::start(), start_vec(), next_data_packet(), encode_fin() │   BitmapWord = u64, outbound_bitmap_words() └── command.rs ── cap-addressed command protocol [cap:32][op:1][params...], dev caps via BLAKE3 caps: DIAG, MEM, RELOAD enum Op { Read, Write, Exec }
//
// ferros_ledger/ ── append-only event chain (no_std) ├── lib.rs ── re-exports ├── ewe.rs ── EWE variable-width integer encoding │   encode_u64(), decode_u64(), encode_lean(), decode_lean() │   encode_seq(), decode_seq(), seq_width() ├── chain.rs ── BLAKE3 hash chain ├── entry.rs ── ledger entry (VSF document structure) ├── event.rs ── typed boot/USB/SD events ├── category.rs ── log category tree └── preboot.rs ── pre-ledger ring buffer
//
// ferros_vault/ ── persistent object store (no_std) ├── lib.rs ── re-exports ├── device.rs ── Device trait, DeviceError, DeviceIoKind ├── hash.rs ── BLAKE3 hashing utilities ├── anchor.rs ── root anchor / superblock ├── boot.rs ── boot sequence validation ├── capability.rs ── capability tokens ├── commit.rs ── atomic commit protocol ├── failure.rs ── failure modes and recovery ├── mesh.rs ── object mesh topology ├── object.rs ── stored objects ├── platform.rs ── platform abstraction └── store.rs ── key-value store
//
// tools/ ├── ferros-bridge/ ── host-side USB tool (tokio + nusb) │   ├── main.rs ── CLI: diag, read, reload, reboot, status │   │   cmd_diag(), cmd_read(), cmd_reload() │   │   pt_send() — blast mode, 512-byte padded OUT, COMPLETE wait │   │   pt_recv() — receive outbound response, no SPEC ACK │   └── usb.rs ── UsbLink { interface, ep_out, ep_in } │       ::open(), send() (512-byte pad), recv() └── mkimg/ ── ELF → flat binary → boot.img v3 main.rs ── PE/COFF header, boot.img v3 packing
//
//! Ferros kernel — bare-metal aarch64. Targets: Pixel 8 (pixel8, default) and M1 MacBook (m1).

#![no_std]
#![no_main]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;

use ferros_pt::packet;
use ferros_pt::transfer::{InboundTransfer, OutboundTransfer};

// ---------------------------------------------------------------------------
// Global allocator
// ---------------------------------------------------------------------------

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// DRAM-based bump allocator. Base address set at startup from __stack_top linker symbol (right after kernel image + 64KB stack). Keeps binary small — no BSS heap array — so PT transfers don't grow circularly.
const HEAP_SIZE: usize = 4 * 1024 * 1024;

static HEAP_BASE: AtomicUsize = AtomicUsize::new(0);
static HEAP_POS: AtomicUsize = AtomicUsize::new(0);

struct BumpAlloc;

unsafe impl GlobalAlloc for BumpAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = HEAP_BASE.load(Ordering::Relaxed) as *mut u8;
        if base.is_null() {
            return core::ptr::null_mut();
        }
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
// Qualcomm ABL (UEFI-based) requires PE/COFF format to find entry point. Stock FP5 kernel starts with "MZ" (PE/COFF). We replicate this.
//
// Layout: 0x000: DOS/ARM64 header (MZ + branch + ARM64 Image fields + e_lfanew) 0x040: PE header (PE\0\0 + COFF + Optional + Section table) 0x1000: _entry (page-aligned .text section start)
// ========================================================================

_start:
    // Offset 0x00: "MZ" magic — must be valid ARM64: `add x13, x18, #0x16` Bytes: 4D 5A 00 91. Stock kernel uses this exact encoding.
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

    mrs     x20, CurrentEL
    lsr     x20, x20, #2

    // ================================================================
    // Check if MMU is already off. If so, skip the teardown entirely. ABL (Tensor G3, m1n1) disables MMU + caches before jumping. FP5/QCM6490 ABL leaves them enabled. Attempting cache clean + MMU disable when already off faults on Tensor G3 (Apple SPRR, Samsung SCTLR behavior).
    // ================================================================
    cmp     x20, #2
    b.ne    .Lcheck_el1_mmu
    mrs     x21, sctlr_el2
    tst     x21, #1             // test M bit (MMU enable)
    b.eq    .Lsctlr_done        // MMU already off — skip teardown
    b       .Ldo_cache_clean_el2

.Lcheck_el1_mmu:
    mrs     x21, sctlr_el1
    tst     x21, #1
    b.eq    .Lsctlr_done        // MMU already off — skip teardown
    b       .Ldo_cache_clean_el1

// ---------- M1 entry: explicit skip (kept for m1n1-boot.py compatibility) ----------
.global _m1_entry
_m1_entry:
    mov     x19, x0
    msr     daifset, #0xF
    mrs     x20, CurrentEL
    lsr     x20, x20, #2
    mov     x21, #0             // no saved SCTLR
    b       .Lsctlr_done

    // ================================================================
    // Cache clean + MMU disable — only reached if MMU was on.
    // ================================================================
.Ldo_cache_clean_el2:
    // EL2 path
    mrs     x21, sctlr_el2     // x21 = original SCTLR (saved for diagnostics)
    // Clean + invalidate data caches before disabling (otherwise dirty lines are lost)
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

.Ldo_cache_clean_el1:
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
    b       .Lexc_recover          // EL1t sync
    .balign 0x80
    b .Lhalt                       // EL1t IRQ
    .balign 0x80
    b .Lhalt                       // EL1t FIQ
    .balign 0x80
    b       .Lexc_serror           // EL1t SError

    .balign 0x80
    b       .Lexc_recover          // EL1h sync
    .balign 0x80
    b .Lhalt                       // EL1h IRQ
    .balign 0x80
    b .Lhalt                       // EL1h FIQ
    .balign 0x80
    b       .Lexc_serror           // EL1h SError

    .balign 0x80
    b       .Lexc_recover          // Lower AArch64 sync
    .balign 0x80
    b .Lhalt                       // Lower AArch64 IRQ
    .balign 0x80
    b .Lhalt                       // Lower AArch64 FIQ
    .balign 0x80
    b       .Lexc_serror           // Lower AArch64 SError

    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt
    .balign 0x80
    b .Lhalt

.balign 16
// Synchronous abort: increment count, advance ELR past faulting instruction, eret.
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

// SError (asynchronous): increment count, do NOT advance ELR (fault is async), just eret — the exception entry consumed the pending SError.
.Lexc_serror:
    stp     x0, x1, [sp, #-16]!
    adrp    x0, __exception_count
    add     x0, x0, :lo12:__exception_count
    ldr     x1, [x0]
    add     x1, x1, #1
    str     x1, [x0]
    ldp     x0, x1, [sp], #16
    eret
"#);

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

// ---------------------------------------------------------------------------
// M1 kernel entry
// ---------------------------------------------------------------------------

#[cfg(feature = "m1")]
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(dtb_addr: u64) -> ! {
    // Init DRAM heap.
    unsafe extern "C" { static __stack_top: u8; }
    let stack_top = unsafe { &__stack_top as *const u8 as usize };
    let heap_base = (stack_top + 0xFFF) & !0xFFF;
    HEAP_BASE.store(heap_base, Ordering::SeqCst);

    // Parse DTB for simplefb — m1n1 always provides one.
    let fb_cfg = if dtb_addr != 0 {
        unsafe { ferros_hal::dtb::Dtb::from_ptr(dtb_addr as *const u8) }
            .and_then(|dtb| dtb.parse_simplefb())
    } else {
        None
    };

    // Set up framebuffer console from DTB simplefb, or spin if not found. M1 DCP uses 10:10:10:2 pixel format (R10:G10:B10:X2, little-endian u32). White = G#FFFFFFFC, Black = G#00000000.
    let mut con = if let Some(cfg) = fb_cfg {
        unsafe {
            ferros_hal::console::Console::new(
                cfg.phys_base as *mut u32,
                cfg.width as usize,
                cfg.height as usize,
                (cfg.stride / 4) as usize, // stride in pixels
                0xFFFF_FFFC,               // white text (10:10:10:2)
                0x0000_0000,               // black background
                (32, 32, 32, 32),          // margins
            )
        }
    } else {
        // No framebuffer — spin. Connect serial via m1n1 proxy to debug.
        loop { core::hint::spin_loop(); }
    };

    con.clear();
    con.puts("ferros on M1\n");
    con.puts("============\n\n");
    // --- USB init --- TODO: read actual addresses from ADT. These are placeholders. Boot into m1n1 proxy and run m1n1-boot.py to dump real values. Probe DWC3 before full init — check if controller is powered
    let dwc3_base: usize = 0x3_8228_0000;
    con.puts("DWC3 probe:    ");
    let snpsid = unsafe { ferros_hal::mmio::read32(dwc3_base + 0xC120) };
    con.put_hex32(snpsid);
    con.puts("\n");

    con.puts("GCTL:          ");
    let gctl = unsafe { ferros_hal::mmio::read32(dwc3_base + 0xC110) };
    con.put_hex32(gctl);
    con.puts("\n");

    con.puts("DSTS:          ");
    let dsts = unsafe { ferros_hal::mmio::read32(dwc3_base + 0xC70C) };
    con.put_hex32(dsts);
    con.puts("\n");

    let usb_addrs = ferros_hal_m1::usb::M1UsbAddrs {
        dwc3:      0x3_8228_0000,
        pipe:      0x3_82A8_4000,
        atcphy:    0x3_82A9_0000,
        dart_base0: 0x3_82F8_0000,
        dart_base1: 0x3_82F0_0000,
        dart_sid:  0,
    };

    // DART page tables: use fixed addresses after the heap base. __stack_top is at ~kernel+0x23000. Heap starts there. We reserve the first 64KB of heap for DART page tables. L1 at heap+0, L2 at heap+16KB. Then bump HEAP_POS past them.
    con.puts("DART alloc:    ");
    let l1_base = (heap_base + 0x3FFF) & !0x3FFF; // 16KB align
    let l2_base = l1_base + 16384;
    // Bump heap position past the page tables
    HEAP_POS.store(l2_base + 16384 - heap_base, Ordering::SeqCst);
    con.puts("OK\n");

    con.puts("DART tables:   ");
    let min_page = ferros_hal_m1::usb::dma_buffer_min_page();
    let iova_base: usize = 0xF000_0000;
    let l1_idx = (iova_base >> 25) & 0x1FFF;
    unsafe {
        // Zero tables
        let l1 = l1_base as *mut u64;
        let l2 = l2_base as *mut u64;
        for i in 0..2048 { core::ptr::write_volatile(l1.add(i), 0); }
        for i in 0..2048 { core::ptr::write_volatile(l2.add(i), 0); }
        // L1 entry → L2
        let l1_entry = ((l2_base as u64) >> 14) << 14 | (0xFFF_u64 << 40) | (1 << 1) | 1;
        core::ptr::write_volatile(l1.add(l1_idx), l1_entry);
        // L2 entries → buffer pages
        for i in 0..32usize {
            let phys = min_page + i * 16384;
            let iova = iova_base + i * 16384;
            let l2_idx = (iova >> 14) & 0x7FF;
            let pte = ((phys as u64) >> 14) << 14 | (0xFFF_u64 << 40) | (1 << 1) | 1;
            core::ptr::write_volatile(l2.add(l2_idx), pte);
        }
        core::arch::asm!("dsb sy");
    }
    con.puts("OK\n");

    con.puts("DART hw:       ");
    let ttbr_val = (1u32 << 31) | ((l1_base >> 12) as u32 & 0x7FFF_FFFF);
    unsafe {
        for dart_base in [usb_addrs.dart_base0, usb_addrs.dart_base1] {
            ferros_hal::mmio::write32(dart_base + 0xFC, 0xFFFF); // enable streams
            for s in 0..16u32 {
                let off = 0x200 + (s as usize) * 16;
                ferros_hal::mmio::write32(dart_base + off, ttbr_val);
                for t in 1..4u32 { ferros_hal::mmio::write32(dart_base + off + (t as usize) * 4, 0); }
            }
            for s in 0..16u32 {
                ferros_hal::mmio::write32(dart_base + 0x100 + (s as usize) * 4, 0x80);
            }
            // TLB invalidate
            ferros_hal::mmio::write32(dart_base + 0x34, 0xFFFF);
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
            ferros_hal::mmio::write32(dart_base + 0x20, 1 << 20);
            for _ in 0..100_000u32 {
                if ferros_hal::mmio::read32(dart_base + 0x20) & (1 << 2) == 0 { break; }
            }
        }
    }
    let dma_offset: i64 = iova_base as i64 - min_page as i64;
    con.puts("OK\n");

    con.puts("USB init:      ");
    match ferros_hal_m1::usb::M1Usb::init(&usb_addrs, dma_offset) {
        Some(mut usb) => {
            con.puts("OK\n");
            // Post-init register dump
            unsafe {
                con.puts("DCTL:          ");
                con.put_hex32(ferros_hal::mmio::read32(dwc3_base + 0xC704));
                con.puts("\n");
                con.puts("DALEPENA:      ");
                con.put_hex32(ferros_hal::mmio::read32(dwc3_base + 0xC720));
                con.puts("\n");
                con.puts("GUSB2PHYCFG:   ");
                con.put_hex32(ferros_hal::mmio::read32(dwc3_base + 0xC200));
                con.puts("\n");
                con.puts("GUSB3PIPECTL:  ");
                con.put_hex32(ferros_hal::mmio::read32(dwc3_base + 0xC2C0));
                con.puts("\n");
                con.puts("GEVNTCOUNT:    ");
                con.put_hex32(ferros_hal::mmio::read32(dwc3_base + 0xC40C));
                con.puts("\n");
                con.puts("DART ERR:      ");
                con.put_hex32(ferros_hal::mmio::read32(usb_addrs.dart_base0 + 0x40));
                con.puts("\n");
                // Event buffer DMA address
                con.puts("GEVNTADR:      ");
                con.put_hex32(ferros_hal::mmio::read32(dwc3_base + 0xC404));
                con.puts("_");
                con.put_hex32(ferros_hal::mmio::read32(dwc3_base + 0xC400));
                con.puts("\n");
            }
            con.puts("DART cfg:      ");
            con.put_hex32(usb.dart_config);
            con.puts("\n");
            con.puts("DART TCR b/a:  ");
            con.put_hex32(usb.dart_tcr_before);
            con.puts(" -> ");
            con.put_hex32(usb.dart_tcr_after);
            con.puts("\n");
            con.puts("DART TTBR b/a: ");
            con.put_hex32(usb.dart_ttbr_before);
            con.puts(" -> ");
            con.put_hex32(usb.dart_ttbr_after);
            con.puts("\n");
            con.puts("dma_offset:    ");
            con.put_hex32((usb.dma_offset_val >> 32) as u32);
            con.puts("_");
            con.put_hex32(usb.dma_offset_val as u32);
            con.puts("\n");
            // Read back L1 entry for IOVA 0xF0000000 L1 index = (0xF0000000 >> 25) & 0x1FFF = 0x780 TTBR after has L1 phys = (ttbr & 0x7FFFFFFF) << 12
            let l1_phys = ((usb.dart_ttbr_after & 0x7FFF_FFFF) as usize) << 12;
            con.puts("\nL1 phys:       ");
            con.put_hex32((usb.l1_phys >> 32) as u32);
            con.puts("_");
            con.put_hex32(usb.l1_phys as u32);
            con.puts("\nmin_buf_page:  ");
            con.put_hex32((usb.min_buf_page >> 32) as u32);
            con.puts("_");
            con.put_hex32(usb.min_buf_page as u32);
            // L1 readback immediately after dart.map() (inside USB init)
            con.puts("\nL1 immed:      ");
            con.put_hex32((usb.l1_readback >> 32) as u32);
            con.puts("_");
            con.put_hex32(usb.l1_readback as u32);
            // Read back L1 entry now (from kernel_main)
            let l1_addr = usb.l1_phys as usize;
            if l1_addr != 0 {
                let l1_entry_addr = l1_addr + 0x780 * 8;
                let l1_lo = unsafe { core::ptr::read_volatile(l1_entry_addr as *const u32) };
                let l1_hi = unsafe { core::ptr::read_volatile((l1_entry_addr + 4) as *const u32) };
                con.puts("\nL1 now:        ");
                con.put_hex32(l1_hi);
                con.puts("_");
                con.put_hex32(l1_lo);
            }
            con.puts("\nSETUP buf DMA: ");
            con.put_hex32((usb.setup_buf_dma >> 32) as u32);
            con.puts("_");
            con.put_hex32(usb.setup_buf_dma as u32);
            con.puts("\nSETUP trb DMA: ");
            con.put_hex32((usb.setup_trb_dma >> 32) as u32);
            con.puts("_");
            con.put_hex32(usb.setup_trb_dma as u32);
            con.puts("\n");
            con.puts("ep_cmd ok=");
            con.put_hex32(usb.ep_cmd_ok);
            con.puts(" fail=");
            con.put_hex32(usb.ep_cmd_fail);
            con.puts("\n");
            if usb.ep_cmd_fail > 0 {
                con.puts("last_cmd:      ");
                con.put_hex32(usb.last_cmd_status);
                con.puts("\n");
            }
            con.puts("Waiting for host...\n");

            // Run event loop with diagnostics
            use ferros_hal::UsbBulk;
            let mut loop_count = 0u32;
            loop {
                match usb.poll_event() {
                    ferros_hal::UsbEvent::None => {
                        loop_count += 1;
                        if loop_count == 10_000_000 {
                            con.puts("\n--- 10M polls ---\n");
                            con.puts("evt_count:     ");
                            con.put_hex32(usb.evt_count);
                            con.puts("\n");
                            con.puts("setup_count:   ");
                            con.put_hex32(usb.setup_count);
                            con.puts("\n");
                            con.puts("GEVNTCOUNT:    ");
                            con.put_hex32(usb.read_reg(0xC40C));
                            con.puts("\n");
                            con.puts("DSTS:          ");
                            con.put_hex32(usb.read_reg(0xC70C));
                            con.puts("\n");
                            con.puts("DCTL:          ");
                            con.put_hex32(usb.read_reg(0xC704));
                            con.puts("\n");
                            // Dump raw events
                            con.puts("raw evts:      ");
                            for i in 0..8 {
                                con.put_hex32(usb.raw_evts[i]);
                                con.puts(" ");
                            }
                            con.puts("\n");
                            // Read DART error addr
                            con.puts("DART ERR_ADDR: ");
                            unsafe {
                                con.put_hex32(ferros_hal::mmio::read32(usb_addrs.dart_base0 + 0x54));
                                con.puts("_");
                                con.put_hex32(ferros_hal::mmio::read32(usb_addrs.dart_base0 + 0x50));
                            }
                            con.puts("\n");
                        }
                        for _ in 0..64u32 { core::hint::spin_loop(); }
                    }
                    ferros_hal::UsbEvent::Reset => {
                        con.puts("  USB reset\n");
                        usb.handle_reset();
                    }
                    ferros_hal::UsbEvent::ConnectDone { speed } => {
                        con.puts("  connected spd=");
                        con.put_hex32(speed);
                        con.puts("\n");
                        usb.handle_connect_done();
                    }
                    ferros_hal::UsbEvent::Disconnect => {
                        con.puts("  disconnected\n");
                        usb.handle_disconnect();
                    }
                    ferros_hal::UsbEvent::Ep0Setup { request } => {
                        con.puts("  SETUP: ");
                        for b in &request {
                            con.put_hex32(*b as u32);
                            con.puts(" ");
                        }
                        con.puts("\n");
                        if !usb.handle_setup(&request) {
                            usb.ep0_stall();
                        }
                    }
                    ferros_hal::UsbEvent::TransferComplete { ep } => {
                        con.puts("  XferDone ep=");
                        con.put_hex32(ep as u32);
                        con.puts("\n");
                    }
                    ferros_hal::UsbEvent::TransferNotReady { ep } => {
                        con.puts("  XferNRdy ep=");
                        con.put_hex32(ep as u32);
                        con.puts("\n");
                    }
                }
            }
        }
        None => {
            con.puts("FAIL\n");
            con.puts("USB init failed. Halted.\n");
        }
    }

    loop { core::hint::spin_loop(); }
}

/// M1 USB event loop with PT command dispatch.
#[cfg(feature = "m1")]
fn m1_usb_event_loop(
    usb: &mut ferros_hal_m1::usb::M1Usb,
    con: &mut ferros_hal::console::Console,
) {
    use ferros_hal::{UsbBulk, UsbEvent};
    use ferros_pt::{packet, transfer::InboundTransfer};

    // PT state
    let mut pt_data_buf: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut pt_bitmap_buf: alloc::vec::Vec<ferros_pt::BitmapWord> = alloc::vec::Vec::new();
    let mut pt_inbound: Option<InboundTransfer<'_>> = None;
    let mut pt_seq_width: usize = 0;
    let mut pt_out_data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();

    // Cap hashes
    let cap_diag = ferros_pt::command::dev_cap(ferros_pt::command::caps::DIAG);
    let cap_reload = ferros_pt::command::dev_cap(ferros_pt::command::caps::RELOAD);
    let cap_reboot = ferros_pt::command::dev_cap(ferros_pt::command::caps::REBOOT);

    // Staging area for hot-reload (use high DRAM, well above kernel)
    const RELOAD_STAGE: usize = 0x8_2000_0000;
    let mut reload_size: usize = 0;

    // Boot log buffer for DIAG command
    let mut boot_log: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    boot_log.extend_from_slice(b"ferros on M1\nUSB: connected\n");

    loop {
        match usb.poll_event() {
            UsbEvent::None => {
                // Pump outbound data if pending
                if !pt_out_data.is_empty() && usb.bulk_in_is_idle() {
                    let len = pt_out_data.len().min(512);
                    let chunk: alloc::vec::Vec<u8> = pt_out_data.drain(..len).collect();
                    usb.bulk_in_send(&chunk);
                }
                for _ in 0..64u32 { core::hint::spin_loop(); }
            }
            UsbEvent::Reset => {
                con.puts("  USB reset\n");
                usb.handle_reset();
                pt_inbound = None;
                pt_seq_width = 0;
                pt_out_data.clear();
            }
            UsbEvent::ConnectDone { speed } => {
                con.puts("  connected spd=");
                con.put_hex32(speed);
                con.puts("\n");
                usb.handle_connect_done();
                usb.ep0_start_setup();
                usb.bulk_out_arm();
            }
            UsbEvent::Disconnect => {
                con.puts("  disconnected\n");
                usb.handle_disconnect();
                pt_inbound = None;
                pt_seq_width = 0;
                pt_out_data.clear();
            }
            UsbEvent::Ep0Setup { request } => {
                if !usb.handle_setup(&request) {
                    usb.ep0_stall();
                }
            }
            UsbEvent::TransferComplete { ep } => {
                if ep == 2 {
                    // Bulk OUT — PT packet
                    let mut tmp = [0u8; 512];
                    let mut n = 0usize;
                    if let Some(data) = usb.bulk_out_read() {
                        n = data.len().min(512);
                        tmp[..n].copy_from_slice(&data[..n]);
                    }
                    if n > 0 {
                        if ferros_pt::is_data_packet(tmp[0]) {
                            // DATA packet
                            if let Some(ref mut xfer) = pt_inbound {
                                if let Some((_sid, seq, hash, payload)) = packet::decode_data(&tmp[..n], pt_seq_width) {
                                    xfer.handle_data(seq, &hash, payload);
                                    if xfer.all_received() {
                                        let mut complete_buf = [0u8; 512];
                                        let clen = xfer.finish(&mut complete_buf);
                                        if clen > 0 {
                                            usb.bulk_in_send(&complete_buf[..clen]);
                                        }
                                        // Command dispatch
                                        let payload = xfer.payload();
                                        if let Some(cmd) = ferros_pt::command::parse(payload) {
                                            if cmd.cap == cap_diag && cmd.op == ferros_pt::Op::Read {
                                                con.puts("  CMD: DIAG\n");
                                                pt_out_data.clear();
                                                pt_out_data.extend_from_slice(&boot_log);
                                            } else if cmd.cap == cap_reload && cmd.op == ferros_pt::Op::Write {
                                                let data = cmd.params;
                                                unsafe {
                                                    core::ptr::copy_nonoverlapping(
                                                        data.as_ptr(),
                                                        (RELOAD_STAGE + reload_size) as *mut u8,
                                                        data.len(),
                                                    );
                                                }
                                                reload_size += data.len();
                                                con.puts("  RELOAD +");
                                                con.put_hex32(data.len() as u32);
                                                con.puts(" total=");
                                                con.put_hex32(reload_size as u32);
                                                con.puts("\n");
                                            } else if cmd.cap == cap_reload && cmd.op == ferros_pt::Op::Exec {
                                                con.puts("  RELOAD EXEC ");
                                                con.put_hex32(reload_size as u32);
                                                con.puts(" bytes\n");
                                                // TODO: verify + jump For now, just acknowledge
                                            } else if cmd.cap == cap_reboot && cmd.op == ferros_pt::Op::Exec {
                                                con.puts("  REBOOT\n");
                                                // PSCI SYSTEM_RESET
                                                unsafe {
                                                    core::arch::asm!(
                                                        "mov x0, #0x84000000",
                                                        "movk x0, #0x0009, lsl #16",
                                                        "hvc #0",
                                                        options(noreturn)
                                                    );
                                                }
                                            }
                                        }
                                        pt_inbound = None;
                                        pt_seq_width = 0;
                                    }
                                }
                            }
                        } else if ferros_pt::is_control_packet(tmp[0]) {
                            // SPEC packet — start new inbound transfer
                            if let Some(spec) = packet::Spec::decode(&tmp[..n]) {
                                pt_seq_width = ferros_ledger::ewe::seq_width(spec.count);
                                let bitmap_words = ferros_pt::transfer::bitmap_words(spec.count);
                                pt_data_buf = alloc::vec![0u8; spec.total as usize];
                                pt_bitmap_buf = alloc::vec![0u64; bitmap_words];
                                // SAFETY: pt_data_buf and pt_bitmap_buf live as long as pt_inbound
                                let xfer = unsafe {
                                    InboundTransfer::new(
                                        &spec,
                                        core::slice::from_raw_parts_mut(pt_data_buf.as_mut_ptr(), pt_data_buf.len()),
                                        core::slice::from_raw_parts_mut(pt_bitmap_buf.as_mut_ptr(), pt_bitmap_buf.len()),
                                    )
                                };
                                if let Some(xfer) = xfer {
                                    // Send SPEC ACK (seq=MAX means "ready")
                                    let ack = packet::Ack { sid: spec.sid, seq: u64::MAX };
                                    let mut ack_buf = [0u8; 64];
                                    let ack_len = ack.encode(&mut ack_buf);
                                    if ack_len > 0 {
                                        usb.bulk_in_send(&ack_buf[..ack_len]);
                                    }
                                    pt_inbound = Some(xfer);
                                    con.puts("  SPEC sid=");
                                    con.put_hex32(spec.sid.0 as u32);
                                    con.puts(" cnt=");
                                    con.put_hex32(spec.count as u32);
                                    con.puts("\n");
                                }
                            }
                        }
                    }
                    usb.bulk_out_arm();
                } else if ep == 3 {
                    // Bulk IN complete — nothing to do, bulk_in_idle set by poll_event
                }
            }
            UsbEvent::TransferNotReady { .. } => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Pixel 8 / QCM6490 kernel entry
// ---------------------------------------------------------------------------

#[cfg(feature = "pixel8")]
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(dtb_addr: u64) -> ! {
    // Init DRAM heap — must happen before any allocation. __stack_top is right after kernel image + 64KB stack. Align to 4KB page boundary for clean start.
    unsafe extern "C" { static __stack_top: u8; }
    let stack_top = unsafe { &__stack_top as *const u8 as usize };
    let heap_base = (stack_top + 0xFFF) & !0xFFF; // page-align up
    HEAP_BASE.store(heap_base, Ordering::SeqCst);

    let exc_start = exception_count();

    // ---- Pixel 8 / Tensor G3 ----

    // ---- Recoverable cluster watchdog ---- We KEEP ABL's watchdogs armed and pet them in the main loop, instead of disabling them. Rationale: with the watchdogs off, a genuine hang (stuck payload, hung MMIO read) freezes the phone forever — needs a physical power-cycle. Kept armed + petted, a hang instead auto-resets in ~ABL's window (~60s, reset reason G#CBEA), and ABL's A/B retry brings ferros back. This is the safety net for every risky new-MMIO experiment on this device.
    // ABL already armed them (RSTEN + PMU int-enable — the CBEA reset proves it) and calibrated the reload for ~60s, so REUSE its WTDAT (read it) rather than recompute the WDT clock. Reload the counter now for a full first window; the main loop then pets continuously (it spins on USB poll + enum re-init, so normal operation — including slow enumeration — always pets; only a true hang stops the pets). Samsung s3c2410 block: WTCON base+0, WTDAT +4, WTCNT +8. Nodes: watchdog_cl0@G#10060000, watchdog_cl1@G#10070000.
    // We ARM explicitly (not just reuse ABL's WTCON) so this works identically on a cold boot AND on hot-reload — where the prior ferros kernel had DISABLED the watchdog (WTCON=0), so there's nothing to reuse. The PMU-level reset routing (CLUSTERx_NONCPU_INT_EN) that ABL set persists across the kernel→kernel jump (we never reset the PMU), so WTCON.RSTEN is enough to actually reset. Max prescaler + DIV128 + full 16-bit reload = the longest window (~1-2 min), safely clear of any false-fire during normal spinning while still recovering a hang quickly.
    // WTCON bits (s3c2410): RSTEN=b0, EN=b5, DIV128=3<<3, PRESCALE(0xFF)=0xFF<<8. INTEN left off (we want a reset, not an interrupt).
    const WATCHDOGS: [usize; 2] = [0x1006_0000, 0x1007_0000];
    const WTDAT: usize = 0x04;
    const WTCNT: usize = 0x08;
    const WDT_ARM: u32 = 0xFF00 | (3 << 3) | (1 << 5) | (1 << 0);
    let wdt_reload = [0xFFFFu32; 2];
    let mut wdt_con = [0u32; 2];
    for (i, &wdt) in WATCHDOGS.iter().enumerate() {
        unsafe {
            core::ptr::write_volatile((wdt + WTDAT) as *mut u32, wdt_reload[i]);
            core::ptr::write_volatile((wdt + WTCNT) as *mut u32, wdt_reload[i]);
            core::ptr::write_volatile((wdt + 0x00) as *mut u32, WDT_ARM);
            wdt_con[i] = core::ptr::read_volatile((wdt + 0x00) as *const u32);
        }
    }

    // GPIO registers for volume buttons (from DTB: pinctrl@G#154D0000)
    const GPA4_DAT: usize = 0x154D_0084;
    const GPA6_DAT: usize = 0x154D_00A4;
    const VOL_DOWN_BIT: u32 = 1 << 1;
    const VOL_UP_BIT: u32 = 1 << 2;

    // ---- Display: deferred ---- Display SYSMMU is write-protected (S2MPU). Writing to G#19840000 locks the CPU. Display output requires either: walking SYSMMU page tables (read-only) to find the FB physical address, or USB transport for all I/O. Prioritizing USB.

    // ---- S2MPU bypass (from pkvm_s2mpu.c:187) ---- Bypass ALL relevant S2MPUs so we can access USB PHY, UFS, and display. Each S2MPU: write G#FF to +G#54 (clear VID protection), 0 to +G#00 (disable)
    const S2MPUS: [usize; 2] = [
        0x1107_0000, // HSI0 — USB PHY + DWC3 DMA
        0x131F_0000, // HSI2 — UFS
    ];
    for &base in &S2MPUS {
        unsafe {
            core::ptr::write_volatile((base + 0x54) as *mut u32, 0xFF);
            core::ptr::write_volatile((base + 0x00) as *mut u32, 0x00);
        }
    }

    // ---- USB SYSMMU bypass (now accessible with S2MPU disabled) ---- SYSMMU at G#11040000 translates DWC3 DMA addresses. Without bypass, DWC3 can't read TRBs or write events to our DRAM buffers. Samsung SYSMMU v9: write 0 to CTRL (offset 0) to disable translation.
    const USB_SYSMMU: usize = 0x1104_0000;
    unsafe {
        core::ptr::write_volatile((USB_SYSMMU + 0x00) as *mut u32, 0); // disable SYSMMU
    }

    // ---- eUSB2 PHY init ---- Faithful port of phy_exynos_eusb_initiate (tools/pixel8/phy-ref/phy-exynos-eusb.c), 19.2MHz / 4nm. Bit positions from eusb-con-reg.h; link init from exynos-usb-blkcon.c. The old inline port had the register OFFSETS right but the BITS wrong (rptr_mode b1 not b10, pll_fb_div [11:0] not [19:8], pll_ref_div [3:0] not [11:8]) and — the killer — never cleared TESTSE.test_iddq, leaving the PHY analog powered DOWN, so nothing ever reached the wire. Assumes ABL left the eUSB repeater (I2C) + PMU isolation + PHY clocks up (they persist across the jump; the analog IDDQ/enable does not).
    const USBCON: usize = 0x1110_0000;
    const EUSB_PHY: usize = 0x1111_0000;

    // Re-runnable: the enumeration watchdog below re-inits the PHY if the host never talks to us (the PLL-lock race makes first-try enumeration flaky).
    let eusb_phy_init = || unsafe {
        // Tensor cores ~2GHz; ~4000 spin iters/us is generous (over-delay harmless, under-delay breaks PLL/REXT calibration).
        let udelay = |us: u32| { for _ in 0..us.saturating_mul(4000) { core::hint::spin_loop(); } };

        // --- BLKCON link init (exynos_usbcon_init_link) --- LINKCTRL(G#04): disable qact autogating, force qact, bypass bus filter.
        let mut reg = core::ptr::read_volatile((USBCON + 0x0004) as *const u32);
        reg |= (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7); // dis_id0/bvalid/vbusvalid/linkgate_qact
        reg &= !(1 << 8);                                  // force_qact = 0
        core::ptr::write_volatile((USBCON + 0x0004) as *mut u32, reg);
        reg |= 1 << 8;        // force_qact = 1
        reg |= 0xF << 12;     // bus_filter_bypass = G#F
        core::ptr::write_volatile((USBCON + 0x0004) as *mut u32, reg);

        // LINK_CLKRST(G#0C): pulse link software reset.
        reg = core::ptr::read_volatile((USBCON + 0x000C) as *const u32);
        reg |= 1 << 0;
        core::ptr::write_volatile((USBCON + 0x000C) as *mut u32, reg);
        udelay(10);
        reg &= !(1 << 0);
        core::ptr::write_volatile((USBCON + 0x000C) as *mut u32, reg);

        // UTMI_CTRL(G#10): force VBUS/BVALID valid (device mode, no VBUS-detect HW path).
        reg = core::ptr::read_volatile((USBCON + 0x0010) as *const u32);
        reg |= (1 << 1) | (1 << 2);
        core::ptr::write_volatile((USBCON + 0x0010) as *mut u32, reg);

        // --- PHY init (phy_exynos_eusb_initiate) ---
        // 1. Hold PHY + UTMI port in reset, override-enabled. RST_CTRL(G#00): phy_reset b0, phy_reset_ovrd_en b1, utmi_port_reset b4, utmi_port_reset_ovrd_en b5.
        reg = core::ptr::read_volatile((EUSB_PHY + 0x0000) as *const u32);
        reg |= (1 << 0) | (1 << 1) | (1 << 4) | (1 << 5);
        core::ptr::write_volatile((EUSB_PHY + 0x0000) as *mut u32, reg);

        // 2. Strapping. CMN_CTRL(G#04): rptr_mode b10 = 1, ref_freq_sel b6:4 = 0 (19.2MHz).
        reg = core::ptr::read_volatile((EUSB_PHY + 0x0004) as *const u32);
        reg |= 1 << 10;
        reg &= !(0x7 << 4);
        core::ptr::write_volatile((EUSB_PHY + 0x0004) as *mut u32, reg);

        // PLLCFG0(G#08): pll_fb_div b19:8 = 368, pll_cpbias_cntrl b6:0 = 0.
        reg = core::ptr::read_volatile((EUSB_PHY + 0x0008) as *const u32);
        reg &= !(0xFFF << 8);
        reg |= 368 << 8;
        reg &= !0x7F;
        core::ptr::write_volatile((EUSB_PHY + 0x0008) as *mut u32, reg);

        // PLLCFG1(G#0C): pll_ref_div b11:8 = 0.
        reg = core::ptr::read_volatile((EUSB_PHY + 0x000C) as *const u32);
        reg &= !(0xF << 8);
        core::ptr::write_volatile((EUSB_PHY + 0x000C) as *mut u32, reg);

        // 3. Clear analog IDDQ — TESTSE(G#20) test_iddq b6 = 0 — powers the PHY analog UP. (The step the old port missed.)
        reg = core::ptr::read_volatile((EUSB_PHY + 0x0020) as *const u32);
        reg &= !(1 << 6);
        core::ptr::write_volatile((EUSB_PHY + 0x0020) as *mut u32, reg);

        // Keep phy_enable(b0) = 0 during the power-up window.
        reg = core::ptr::read_volatile((EUSB_PHY + 0x0004) as *const u32);
        reg &= !(1 << 0);
        core::ptr::write_volatile((EUSB_PHY + 0x0004) as *mut u32, reg);

        udelay(10); // phy_reset held >=10us after test_iddq clear

        // 6. Release phy_reset (b0=0); KEEP utmi_port_reset asserted.
        reg = core::ptr::read_volatile((EUSB_PHY + 0x0000) as *const u32);
        reg &= !(1 << 0);
        core::ptr::write_volatile((EUSB_PHY + 0x0000) as *mut u32, reg);

        udelay(10); // REXT calibration

        // 7. Enable PHY. CMN_CTRL phy_enable(b0) = 1.
        reg = core::ptr::read_volatile((EUSB_PHY + 0x0004) as *const u32);
        reg |= 1 << 0;
        core::ptr::write_volatile((EUSB_PHY + 0x0004) as *mut u32, reg);

        udelay(1000); // REXT calibration
        udelay(28);   // T4: analog+digital powered, utmi_clk toggles
        udelay(2500); // T5: Port Reset (ESE1) transmit complete

        // Release UTMI port reset (RST_CTRL b4=0, b5=0).
        reg = core::ptr::read_volatile((EUSB_PHY + 0x0000) as *const u32);
        reg &= !((1 << 4) | (1 << 5));
        core::ptr::write_volatile((EUSB_PHY + 0x0000) as *mut u32, reg);
    };
    // eUSB2 repeater snapshot BEFORE PHY init (reads only — the bus is shared with the PMIC chain).
    // Answers, on every boot, "did ABL's repeater state survive the jump?" — DIAG reports it, so flaky boots self-document at the analog layer, not just the link layer.
    // Slot 0 = REV_ID (healthy G#3), slots 1..9 = CONFIG_PORT1 + the 8 tune registers (healthy baseline in REPEATER.md). Read failure = G#8000_0000 | TRANS_STATUS[15:0].
    let rep_snap: [u32; 9] = {
        let i2c = ferros_hal::hsi2c::Hsi2c::new(ferros_hal::hsi2c::PIXEL8_HSI2C11_BASE);
        i2c.init();
        let rep = |reg: u8| match i2c.read_reg(ferros_hal::hsi2c::PIXEL8_EUSB_REPEATER_ADDR, reg) {
            Ok(v) => v as u32,
            Err(trans) => 0x8000_0000 | (trans & 0xFFFF),
        };
        [rep(0xB0), rep(0x60), rep(0x70), rep(0x71), rep(0x72), rep(0x73), rep(0x77), rep(0x78), rep(0x79)]
    };

    // Apply Google's calibrated repeater tune (the DT repeater_tune* table) before PHY init.
    // ABL does NOT apply it — only the Android driver does — so without this every ferros boot runs the analog path untuned (the leading suspect for the enumeration coin flip).
    // Gated on a positive REV_ID identification: the bus is shared with the PMIC chain, so if the snapshot couldn't identify the repeater we write nothing.
    // rep_tuned: count of verified tune registers (8 = full success), G#FF = skipped (REV_ID gate).
    let rep_tuned: u32 = if rep_snap[0] == 0x3 {
        let i2c = ferros_hal::hsi2c::Hsi2c::new(ferros_hal::hsi2c::PIXEL8_HSI2C11_BASE);
        let mut ok = 0u32;
        for &(reg, val) in ferros_hal::hsi2c::PIXEL8_REPEATER_TUNE.iter() {
            let addr = ferros_hal::hsi2c::PIXEL8_EUSB_REPEATER_ADDR;
            if i2c.write_reg(addr, reg, val).is_ok() && i2c.read_reg(addr, reg) == Ok(val) {
                ok += 1;
            }
        }
        ok
    } else {
        0xFF
    };

    eusb_phy_init();

    // If we survived, S2MPU is bypassed and PHY is initialized. Now try UFS + USB.
    const DWC3: usize = 0x1121_0000;
    const UFS_BASE: usize = 0x1320_0000;
    const FERROS_PART_LBA: u32 = 21758972;

    fn hex_to_buf(buf: &mut [u8], off: &mut usize, label: &[u8], val: u32) {
        for &b in label { buf[*off] = b; *off += 1; }
        let hex = b"0123456789ABCDEF";
        for i in (0..8).rev() {
            buf[*off] = hex[((val >> (i * 4)) & 0xF) as usize]; *off += 1;
        }
        buf[*off] = b'\n'; *off += 1;
    }

    let _ = (hex_to_buf, FERROS_PART_LBA); // retained: hex_to_buf used elsewhere; LBA is the real write target once the command path works

    // ---- UFS full re-initialization ----
    // ABL's handoff state never processes host transfer requests (UFS.md, proven down to NOP-fails-on-clean-controller), so we re-own the controller from HCE reset: vendor config at enable time, M-PHY/UNIPRO calibration (ferros_hal::ufs_cal, the zuma ufs-cal-if port), DME_LINKSTARTUP, fDeviceInit, HS-G4 power-mode change.
    // The historical vendor-region/PMA hangs were the HCI_FORCE_HCS auto clock-stop gates; full_init clears them first and leaves them off.
    // Every wait in full_init is bounded (worst case a few seconds) — far inside the ~87s cluster watchdog, and a hang here auto-recovers via ABL A/B fallback anyway.
    // Validation: read_block(1) = GPT header (read-only) — success is OCS=0 and "EFI PART" in the first 8 bytes.
    // ufs_diag layout, surfaced via DIAG "UFS_*" lines (see the append_hex block).
    let ufs = ferros_hal::ufs::UfsController::new(UFS_BASE);
    let mut ufs_diag = [0u32; 28];
    let mut pmaw = [0u32; 12]; // ABL working-link PMA/PA snapshot (Phase A)
    let mut pmaf = [0u32; 12]; // post-full_init failed-link PMA/PA snapshot (Phase B)
    let mut rst_test = [0u32; 4]; // GPIO_OUT device-reset test: [HCS_before, MXGR_before, HCS_after, MXGR_after]
    let mut clkdiag = [0u32; 5]; // clock/refclk state at link-startup: [CLKSTOP_CTRL, FORCE_HCS, MPHY_REFCLK_SEL, CMU_QCH, CMU_UNIPRO_GATE]
    let mut reuse = [0u32; 14]; // pbl-style descriptor-reuse NOP: [utrlba, utrlbau, ucd_lo, ucd_hi, dbr_before, ocs, is, dbr_after, done, rsr, utrd_dw0, utrd_dw2, s2mpu_ctrl, s2mpu_cfg]
    let mut clean = [0u32; 4]; // absolute-first clean NOP: [ocs, rsp, IS, DBR]
    let mut hitest = [0u32; 4]; // high-DRAM reachability probe: [ocs, is, dbr_after, done]
    let mut iocc_fix = [0u32; 2]; // IOCC coherency clear: [before, after]
    let mut dma_dbg = [0u32; 6]; // Exynos DMA-engine state captured mid-stall: [fsm, dma0_state, dma0_cnt, dma0_doorbell, vendor_is, axi_if_ctrl]
    let mut h8 = [0u32; 6]; // hibern8-exit probe: [pa_ctrlstate_before, pwrmode_before, uic_result, upms_completed, pa_ctrlstate_after, hcs_after]
    let mut h8_nop = [0u32; 4]; // NOP after hibern8 exit: [ocs, rsp, is, dbr]
    let mut regfile = [0u32; 14]; // ABL pristine register file: CAP,VER,IS,IE,HCS,HCE,UTRLBA,UTRLBAU,UTRLDBR,UTRLCLR,UTRLRSR,UTMRLBA,UTMRLRSR,UICCMD
    let mut clr_nop = [0u32; 4]; // NOP with UTRLCLR forced to all-ones first: [ocs, rsp, is, dbr]
    let mut ext = [0u32; 8]; // external-block state at ABL handoff: [sysreg_iocc, pmu_phy_iso, ufsp_rsec, ufsp_wsec, s2mpu_ctrl0, gph5con, gpp0con, gpp0dat]
    let mut cmu = [0u32; 10]; // mclk clock tree at ABL handoff: [pll_shared0_con3, pll_shared2_con3, pll_spare_con3, top_mux, top_div, top_gate, hsi2_user_mux, leaf_aclk, leaf_unipro, leaf_fmp]
    let mut dbg_prd_pristine = 0u32; // pristine UNIPRO DBG_PRD (actual clock indicator: G#78=133MHz, G#59=178MHz)
    {
        let r = |off: usize| unsafe { core::ptr::read_volatile((UFS_BASE + off) as *const u32) };
        let le = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]); // big-endian display: bytes read left-to-right

        // External-block state as ABL left it, read before ANY UFS touch. These are the probe-time writes Linux does OUTSIDE the FWTRACE window: sysreg_ufs iocc G#1302_0710 = 3 (UFS DMA coherency/shareability), PMU ufs-phy-iso G#1546_3EC0 = 1 (PHY isolation bypass). Linux's running UFSP reads RSECURITY=G#FFFA6492 WSECURITY=0. Any delta here is a candidate for both the dead doorbell and the silent link startup.
        {
            let rd = |a: usize| unsafe { core::ptr::read_volatile(a as *const u32) };
            let wr = |a: usize, v: u32| unsafe { core::ptr::write_volatile(a as *mut u32, v) };
            // Always-on-domain reads first.
            ext[0] = rd(0x1302_0710); // sysreg_ufs iocc
            ext[5] = rd(0x1306_0000); // GPH5CON at handoff
            ext[1] = rd(0x1546_3EC0); // PMU ufs-phy-iso

            // UFS device VCC rail: fixed regulator switched by gpp0-1 (zuma-ufs.dtsi ufs_fixed_vcc, enable-active-high). Linux's regulator core CLAIMS this GPIO at probe and drives it — its "vcc en=1" may be Linux TURNING IT ON, not inheriting it. ABL parks the link in HIBERN8 where dropping VCC is legitimate power management; if ABL cuts VCC at handoff the device is silent at line-reset with zero PHY error — our exact signature.
            // HANG LESSON: reading gpp0 directly (PERIC0 GPIO G#1084_0000) bus-hung the AP — the PERIC0 domain/bridge isn't up at ABL handoff for ferros. This round reads ONLY safe always-on blocks: PMU domain status + the NOCL0-side PERIC0 bridge QCH states, to plan the ungate.
            ext[6] = rd(0x1546_2804); // PMU PERIC0 domain status (bit0 = powered, per PMUCAL cond refs)
            ext[7] = rd(0x2600_3290); // CMU_NOCL0 QCH_CON_SLH_AXI_SI_P_PERIC0 (the config-bus bridge into PERIC0)
            // UFSP is behind the auto-gated UFS clock domain (FORCE_HCS UFSP_DRCG_EN) — reading it with the gate armed bus-hangs the AP (proved by hot-reload: this block hung until these unlock writes were added). Clear the auto-stop enables and forced stops first, exactly like unlock_clocks().
            let found_b4 = rd(0x1320_11B4); // ABL leaves G#900
            let found_b0 = rd(0x1320_11B0); // ABL leaves G#10
            wr(0x1320_11B4, found_b4 & !0xFF0); // HCI_FORCE_HCS
            wr(0x1320_11B0, found_b0 & !0x1F);  // HCI_CLKSTOP_CTRL
            unsafe { core::arch::asm!("dsb sy") };
            ext[2] = rd(0x132A_0010); // UFSP RSECURITY
            ext[3] = rd(0x132A_0110); // UFSP WSECURITY
            ext[4] = rd(0x131F_0000); // S2MPU CTRL0
            // PRISTINE UNIPRO DBG_PRD (G#1328_0044) as ABL left it — the actual UNIPRO clock indicator. ABL writes this = 16e9/mclk. G#78 (120) = 133MHz (ABL's rate). G#59 (89) = 178MHz. If this reads 120, ferros runs at 133 but we calibrate for 178 = the mismatch (Linux's clk driver bumps 133->178; ferros never does). Read before full_init overwrites it.
            dbg_prd_pristine = rd(0x1328_0044);

            // PRISTINE mclk clock tree (CMU_TOP G#2604_0000 + CMU_HSI2 G#1300_0000), the registers cal-if programs for the UFS_EMBD vclk. PLL rate = 24.576MHz*M/(P*2^S) with M=[25:16] P=[13:8] S=[2:0] of CON3. TOP mux SELECT=[1:0] (0=OSC 1=SHARED0_D4 2=SHARED2_D2 3=SPARE_D1), TOP div DIVRATIO=[3:0] (÷N+1), gates: CG_VAL=[21] MANUAL=[20]. Linux gets mclk=178MHz from this tree; ABL leaves 133 (DBG_PRD=G#78). Reading both sides tells us the exact mux/div delta to replay.
            cmu[0] = rd(0x2604_014C); // PLL_CON3_PLL_SHARED0
            cmu[1] = rd(0x2604_01CC); // PLL_CON3_PLL_SHARED2
            cmu[2] = rd(0x2604_024C); // PLL_CON3_PLL_SPARE
            cmu[3] = rd(0x2604_10B8); // CLK_CON_MUX_MUX_CLKCMU_HSI2_UFS_EMBD
            cmu[4] = rd(0x2604_18B0); // CLK_CON_DIV_CLKCMU_HSI2_UFS_EMBD
            cmu[5] = rd(0x2604_20E0); // CLK_CON_GAT_GATE_CLKCMU_HSI2_UFS_EMBD
            cmu[6] = rd(0x1300_0630); // PLL_CON0_MUX_CLKCMU_HSI2_UFS_EMBD_USER (MUX_SEL=[4]: 0=OSC 1=TOP)
            cmu[7] = rd(0x1300_210C); // leaf gate I_ACLK
            cmu[8] = rd(0x1300_2110); // leaf gate I_CLK_UNIPRO
            cmu[9] = rd(0x1300_2114); // leaf gate I_FMP_CLK

            // FAITHFUL_REPLAY: restore the clock-gating regs to ABL's found values (B4=G#900, B0=G#10) so full_init sees the same pristine state Linux's probe does. Linux preserves B4 through HCE enable, arms ALL auto-stops (G#DE0) at link_startup PRE, and drops to G#9C0 only as the cal's first act — full_init now replays that choreography and needs the true starting point.
            if FAITHFUL_REPLAY {
                wr(0x1320_11B4, found_b4);
                wr(0x1320_11B0, found_b0);
                unsafe { core::arch::asm!("dsb sy") };
            }

            // CANDIDATE FIX for the dead doorbell: ABL sets sysreg_ufs iocc bits[1:0]=3 => the UFS AXI master issues COHERENT (inner/outer-shareable) transactions that must snoop the CPU caches. ABL/Linux run in the coherency domain so snoops resolve; ferros manages caches MANUALLY (ufs.rs does explicit clean/invalidate around DMA — the non-coherent model) and is not answering coherent snoops, so the master's coherent read STALLS forever = doorbell stuck, every address. Clear the coherency bits so the master does plain non-coherent DMA straight to DRAM, matching ferros's driver. Reversible on reboot.
            iocc_fix[0] = ext[0];                       // IOCC as ABL left it
            wr(0x1302_0710, ext[0] & !0x3);             // clear shareable/coherent bits
            unsafe { core::arch::asm!("dsb sy") };
            iocc_fix[1] = rd(0x1302_0710);              // IOCC after clear (should be ext[0] & !3)
        }

        // FAITHFUL_REPLAY: skip every mutating pre-probe (h8 exit, reuse doorbell, hitest, NOPs, reset test) so full_init runs on the same pristine ABL state Linux's probe sees, replaying Linux's exact combined core+vendor sequence. The pre-probes poisoned the state Linux never sees (the h8 exit alone drives UPMCRS to PWR_FATAL before full_init even starts).
        const FAITHFUL_REPLAY: bool = true;

        // ---- WAKE THE LINK FIRST: ABL parks UFS in HIBERN8 (pristine UICCMD reads G#17 = HIBERN8_ENTER). ----
        // Must run before any doorbell ring or IS clear, on the truly pristine hibernating link. Everything downstream (regfile snapshot, reuse, clean NOP, Phase A) then runs on the woken link.
        if !FAITHFUL_REPLAY {
            h8 = ufs.live_hibern8_exit();
        }

        // ---- ABL pristine register file (before ANY UFS write) ----
        // Characterize why the transfer manager sits idle on a doorbell. Reads only, so ABL's handoff state is untouched.
        regfile[0] = r(0x00);  // CAP
        regfile[1] = r(0x08);  // VER
        regfile[2] = r(0x20);  // IS
        regfile[3] = r(0x24);  // IE
        regfile[4] = r(0x30);  // HCS
        regfile[5] = r(0x34);  // HCE
        regfile[6] = r(0x50);  // UTRLBA
        regfile[7] = r(0x54);  // UTRLBAU
        regfile[8] = r(0x58);  // UTRLDBR
        regfile[9] = r(0x5C);  // UTRLCLR — must have slot bit SET (1) to run; 0 = slot cleared, silently ignored
        regfile[10] = r(0x60); // UTRLRSR
        regfile[11] = r(0x70); // UTMRLBA
        regfile[12] = r(0x80); // UTMRLRSR
        regfile[13] = r(0x90); // UICCMD

        // ---- Phase A0: pbl-style descriptor REUSE (the ABSOLUTE first UFS touch) ----
        // MUST run before init_transfer_list (clean-NOP) or the rebase clobbers ABL's UTRLBA and this tests ferros's own ring instead. pbl/ABL do transfers by reusing the BootROM's descriptor ring at the EXISTING UTRLBA (never rebasing). If that ring lives in an S2MPU/protected DMA region the UFS master can reach but our UFS_BUF (ferros load addr) can't, our rebased doorbell is accepted but the descriptor is never DMA'd (OCS=F). Test: build a NOP in ABL's OWN UCD (reachable region), ring the doorbell WITHOUT rebasing. OCS=0 here = the region was the whole problem.
        if !FAITHFUL_REPLAY {
            let rd8 = |a: usize| unsafe { core::ptr::read_volatile(a as *const u32) };
            let wr32 = |a: usize, v: u32| unsafe { core::ptr::write_volatile(a as *mut u32, v); };
            let utrlba = r(0x50);
            let utrlbau = r(0x54);
            reuse[0] = utrlba;
            reuse[1] = utrlbau;
            reuse[9] = r(0x60); // UTRLRSR (is ABL's list running?)
            let utrd = ((utrlbau as u64) << 32 | utrlba as u64) as usize;
            // Sanity: ABL's UTRD base must be a plausible DRAM address before we poke it. (ABL's ring lives high — G#F8C42000 — a reserved UFS DMA carveout, so allow the full 32-bit DRAM range.)
            // Confirm whether ABL's descriptor region is even CPU-readable (a protected UFS-DMA carveout reads 0 from the CPU) and dump the UFS S2MPU. HSI2 S2MPU at G#131F0000: CTRL0 (+0) and a config/whitelist word (+0x10).
            reuse[10] = rd8(utrd + 0);   // ABL UTRD DW0 (0 = region not CPU-readable)
            reuse[11] = rd8(utrd + 8);   // ABL UTRD DW2
            // V9 S2MPU: did our bypass take on HSI2? Read PROT_EN state (0x50 reads the current per-VID enable bitmap; 0 = protection off = bypassed). Then re-write the CLR and re-read to see if the write sticks (nonzero after = something is holding protection on, e.g. pKVM trapping our writes).
            reuse[12] = rd8(0x131F_0000 + 0x50); // PROT_EN_PER_VID state as boot-bypass left it
            unsafe { core::ptr::write_volatile((0x131F_0000 + 0x54) as *mut u32, 0xFF); core::arch::asm!("dsb sy"); }
            reuse[13] = rd8(0x131F_0000 + 0x50); // PROT_EN after re-clearing (still nonzero = write not sticking)
            if utrlbau == 0 && utrlba >= 0x8000_0000 && utrlba < 0xFFFF_0000 {
                let ucd_lo = rd8(utrd + 16); // UTRD DW4 = UCD base low
                let ucd_hi = rd8(utrd + 20); // UTRD DW5 = UCD base high
                reuse[2] = ucd_lo;
                reuse[3] = ucd_hi;
                let dw7 = rd8(utrd + 28); // keep response/PRDT offsets, zero PRDT count
                let ucd = ((ucd_hi as u64) << 32 | ucd_lo as u64) as usize;
                if ucd_hi == 0 && ucd_lo >= 0x8000_0000 && ucd_lo < 0xFFFF_0000 {
                    // NOP OUT UPIU at the command UPIU (UCD offset 0): transaction code 0, tag 0.
                    unsafe { core::ptr::write_bytes(ucd as *mut u8, 0, 32); }
                    wr32(utrd + 0, (1 << 24) | (1 << 28)); // DW0: interrupt | cmd_type=native UFS
                    wr32(utrd + 8, 0x0000_000F);           // DW2: OCS = INVALID sentinel
                    wr32(utrd + 28, dw7 & 0xFFFF_0000);    // DW7: keep PRDT offset, 0 entries
                    // NEXUS bit 0, clear IS, ring doorbell slot 0 — NO rebase, NO RSR toggle.
                    wr32(0x1320_1140, 0xFFFF_FFFF);
                    unsafe { core::arch::asm!("dsb sy"); }
                    reuse[4] = r(0x58); // DBR before
                    wr32(UFS_BASE + 0x20, 0xFFFF_FFFF); // clear IS
                    wr32(UFS_BASE + 0x58, 1);           // ring slot 0
                    unsafe { core::arch::asm!("dsb sy"); }
                    let mut done = false;
                    for _ in 0..4_000_000u32 {
                        if r(0x20) & 1 != 0 { done = true; break; } // IS.UTRCS
                        if rd8(utrd + 8) & 0xFF != 0x0F { done = true; break; } // OCS written
                    }
                    reuse[5] = rd8(utrd + 8) & 0xFF; // OCS
                    reuse[6] = r(0x20);              // IS
                    reuse[7] = r(0x58);              // DBR after
                    reuse[8] = done as u32;
                }
            }
        }

        // ---- HIGH-DRAM reachability probe (on ABL's live link, before any rebase settles) ----
        // The dead-doorbell hypothesis: ferros's UFS_BUF at G#8009_0400 sits in the low bootloader-reserved DRAM the UFS DMA master / SysMMU can't reach, while ABL (G#F8C4_2000) and Linux (G#8826_1000) both place descriptors in HIGH DRAM and their transfers work. Build a self-contained UTRD+UCD at a fixed high address and ring the doorbell WITHOUT using UFS_BUF. If OCS lands 0 here but not at G#8009_0400, the buffer region was the whole problem and the fix is to relocate UFS_BUF high. hitest = [ocs, is, dbr_after, done].
        if !FAITHFUL_REPLAY {
            let rd8 = |a: usize| unsafe { core::ptr::read_volatile(a as *const u32) };
            let wr32 = |a: usize, v: u32| unsafe { core::ptr::write_volatile(a as *mut u32, v) };
            const HI_UTRD: usize = 0x9000_0000; // 256MB into DRAM — above kernel/bootloader low carveout, below ABL/Linux rings
            const HI_UCD: usize = 0x9000_1000;
            // Zero UTRD (32B) and the UCD command+response region we use (1KB).
            unsafe {
                core::ptr::write_bytes(HI_UTRD as *mut u8, 0, 32);
                core::ptr::write_bytes(HI_UCD as *mut u8, 0, 1024);
            }
            // NOP OUT UPIU at UCD offset 0 (transaction code 0, tag 0) — already zeroed.
            wr32(HI_UTRD + 0, (1 << 24) | (1 << 28));   // DW0: interrupt | cmd_type=native UFS
            wr32(HI_UTRD + 8, 0x0000_000F);             // DW2: OCS = INVALID sentinel
            wr32(HI_UTRD + 16, HI_UCD as u32);          // DW4: UCD base low
            wr32(HI_UTRD + 20, 0);                      // DW5: UCD base high
            wr32(HI_UTRD + 24, (0x0080 << 16) | 0x0080); // DW6: response offset/len
            wr32(HI_UTRD + 28, (0x0100 << 16) | 0x0000); // DW7: PRDT offset, 0 entries
            unsafe { core::arch::asm!("dsb sy") };
            // Point the controller at the high UTRD (RSR=0 to sample the new base), nexus every tag, ring slot 0.
            wr32(UFS_BASE + 0x60, 0);                    // UTRLRSR = 0
            wr32(UFS_BASE + 0x50, HI_UTRD as u32);       // UTRLBA
            wr32(UFS_BASE + 0x54, 0);                    // UTRLBAU
            wr32(0x1320_1140, 0xFFFF_FFFF);              // HCI_UTRL_NEXUS_TYPE
            wr32(UFS_BASE + 0x20, 0xFFFF_FFFF);          // clear IS
            wr32(UFS_BASE + 0x60, 1);                    // UTRLRSR = 1
            unsafe { core::arch::asm!("dsb sy") };
            wr32(UFS_BASE + 0x58, 1);                    // ring doorbell slot 0
            unsafe { core::arch::asm!("dsb sy") };
            let mut done = false;
            for _ in 0..4_000_000u32 {
                if r(0x20) & 1 != 0 { done = true; break; }             // IS.UTRCS
                if rd8(HI_UTRD + 8) & 0xFF != 0x0F { done = true; break; } // OCS written back by DMA
            }
            hitest[0] = rd8(HI_UTRD + 8) & 0xFF; // OCS
            hitest[1] = r(0x20);                 // IS
            hitest[2] = r(0x58);                 // DBR after
            hitest[3] = done as u32;
        }

        // ---- CLEAN NOP: first touch AFTER the reuse test, using ferros's own rebased ring ----
        // Isolates whether Phase A's own writes (clock-unlock, GPIO reset test, NEXUS write, ABL-UCD scribble) perturb the live ABL link before the NOP. If this clean NOP completes (OCS=0) but the later ones don't, our own diagnostics were breaking the link.
        if !FAITHFUL_REPLAY {
            ufs.init_transfer_list();
            let (clean_ocs, clean_rsp) = ufs.live_nop();
            clean[0] = clean_ocs as u32;
            clean[1] = clean_rsp as u32;
            clean[2] = r(0x20); // IS
            clean[3] = r(0x58); // DBR
            // Exynos DMA-engine debug regs, captured right after the (stalled) doorbell. HCI vendor block base G#1320_1100. If the engine is stuck fetching the descriptor these show where: FSM_MONITOR G#C0, DMA0_MONITOR_STATE G#C8 / _CNT G#CC (nonzero cnt = AXI beats moved), DMA0_DOORBELL_DEBUG G#D8, HCI_VENDOR_SPECIFIC_IS G#38 (latches AXI/DMA errors), UFS_AXI_DMA_IF_CTRL G#F8.
            let hci = |off: usize| unsafe { core::ptr::read_volatile((0x1320_1100 + off) as *const u32) };
            dma_dbg[0] = hci(0xC0); // FSM_MONITOR
            dma_dbg[1] = hci(0xC8); // DMA0_MONITOR_STATE
            dma_dbg[2] = hci(0xCC); // DMA0_MONITOR_CNT
            dma_dbg[3] = hci(0xD8); // DMA0_DOORBELL_DEBUG
            dma_dbg[4] = hci(0x38); // HCI_VENDOR_SPECIFIC_IS
            dma_dbg[5] = hci(0xF8); // UFS_AXI_DMA_IF_CTRL
        }

        // ---- Post-wake NOP validation (link was already woken at the top of the block) ----
        if !FAITHFUL_REPLAY {
            ufs.init_transfer_list();
            let (ocs, rsp) = ufs.live_nop();
            h8_nop[0] = ocs as u32;
            h8_nop[1] = rsp as u32;
            h8_nop[2] = r(0x20); // IS
            h8_nop[3] = r(0x58); // DBR
        }

        // ---- UTRLCLR-forced NOP: set the slot-clear register to all-ones before ringing ----
        // If the transfer manager ignores the doorbell because UTRLCLR slot bit is 0 (slot considered cleared/aborted), forcing UTRLCLR=0xFFFFFFFF first unblocks it. RSR is toggled by init_transfer_list; we set UTRLCLR between rebase and doorbell.
        if !FAITHFUL_REPLAY {
            let wr32 = |a: usize, v: u32| unsafe { core::ptr::write_volatile(a as *mut u32, v) };
            ufs.init_transfer_list();
            wr32(UFS_BASE + 0x5C, 0xFFFF_FFFF); // UTRLCLR = all slots active
            unsafe { core::arch::asm!("dsb sy") };
            let (ocs, rsp) = ufs.live_nop();
            clr_nop[0] = ocs as u32;
            clr_nop[1] = rsp as u32;
            clr_nop[2] = r(0x20); // IS
            clr_nop[3] = r(0x58); // DBR
        }

        // ---- Phase A: transfers on ABL's LIVE link (no reset, no full_init) ----
        // This is the original problem, tested cleanly: on a fresh boot ABL leaves the link up (HCS=G#10F). If a NOP completes here, the transfer engine works and full_init is unnecessary — the whole saga was a transfer-setup bug, not a link bug.
        // The UTRD physical address the controller must DMA is reported (no MMU → CPU addr == phys); if the UFS master can't reach it, that alone explains OCS=F.
        let phys = ufs.utrd_phys();
        ufs_diag[24] = (phys >> 32) as u32;
        ufs_diag[25] = phys as u32;
        let hcs_pristine = r(0x30);
        ufs_diag[26] = hcs_pristine;
        // Register-diff baseline: snapshot ABL's WORKING-link PMA/PA state before we touch anything (only meaningful on a fresh flash+boot where HCS_PRISTINE=G#10F). pmaf is captured after full_init's failed startup; the diff reveals the enable/power-up step the cal table omits.
        pmaw = ufs.snapshot_pma();

        // DECISIVE device-reset test (non-cached): on ABL's live working link (HCS=G#10F, DP set), assert HCI_GPIO_OUT bit0=0 (the reference exynos_ufs_dev_hw_reset). If that reaches the device reset_n, the M-PHY link physically drops and the DL layer latches a REAL-TIME link-lost error (UECDL/UECN) and/or HCS.DP clears — unlike cached MXGR. If the UEC family stays clean and DP stays set after a 10ms assert, GPIO_OUT does NOT reach the device on husky → the device never leaves its ABL link state → it ignores every re-link (root cause). rst_test = [uec_before, hcs_before, uec_after, hcs_after]; uec packed UECDL|UECN<<8|UECT<<16|UECDME<<24.
        let mut live_ok = false;
        if !FAITHFUL_REPLAY {
            let uec = || {
                let dl = r(0x3C) & 0xFF; let n = r(0x40) & 0xFF; let t = r(0x44) & 0xFF; let dme = r(0x48) & 0xFF;
                dl | (n << 8) | (t << 16) | (dme << 24)
            };
            let _ = uec(); // clear-on-read: flush any stale latched errors first
            rst_test[0] = uec();          // baseline (expect 0 after flush)
            rst_test[1] = r(0x30);        // HCS before (expect G#10F)
            unsafe { core::ptr::write_volatile((0x1320_1170) as *mut u32, 0); core::arch::asm!("dsb sy"); } // assert reset_n
            ferros_hal::ufs_cal::udelay(10_000);
            rst_test[2] = uec();          // errors latched? (nonzero = reset reached the device)
            rst_test[3] = r(0x30);        // HCS after (DP cleared = reset reached the device)
            unsafe { core::ptr::write_volatile((0x1320_1170) as *mut u32, 1); core::arch::asm!("dsb sy"); } // deassert
            // Belt-and-suspenders: mark every tag a nexus at runtime (visible even if not latched).
            unsafe { core::ptr::write_volatile((0x1320_1140) as *mut u32, 0xFFFF_FFFF); core::arch::asm!("dsb sy"); }
            ufs.init_transfer_list();
            let (live_ocs, live_rsp) = ufs.live_nop();
            ufs_diag[0] = live_ocs as u32;
            ufs_diag[1] = live_rsp as u32;
            ufs_diag[2] = r(0x20); // IS
            ufs_diag[3] = r(0x58); // UTRLDBR
            ufs_diag[4] = r(0x50); // UTRLBA (what the controller actually holds)
            ufs_diag[5] = r(0x54); // UTRLBAU
            ufs_diag[6] = r(0x38); // UECPA
            ufs_diag[7] = unsafe { core::ptr::read_volatile((0x1320_1220) as *const u32) }; // HCI_DBR_DUPLICATION_INFO (VS+G#120)
            live_ok = live_ocs == 0x00;
        }

        if live_ok {
            // Transfer engine works on the live link — validate with a real read and stop.
            let read_ocs = ufs.read_block(1);
            ufs_diag[16] = read_ocs as u32;
            let data = ufs.data_buffer();
            ufs_diag[17] = le(&data[0..4]);
            ufs_diag[18] = le(&data[4..8]);
            ufs_diag[19] = r(0x20);
            ufs_diag[20] = r(0x30);
            ufs_diag[27] = 0xA11_00D; // sentinel: live path succeeded
        } else {
            // ---- Phase B: fall back to full controller re-init ----
            let rep = ufs.full_init();
            ufs_diag[8] = rep.steps;
            ufs_diag[9] = rep.fail_step;
            ufs_diag[10] = rep.linkstartup_res;
            ufs_diag[11] = rep.cal_timeouts;   // did the PHY PLL lock? (EmbCalWait timeouts)
            ufs_diag[12] = rep.dme_err;        // DME_INTR_ERROR_CODE at failure
            ufs_diag[13] = rep.pa_state;       // DBG_PA_CTRLSTATE
            ufs_diag[14] = rep.pa_tx_state;    // DBG_PA_TX_STATE
            ufs_diag[15] = rep.gph5_dat;       // reset_n pad readback after GPIO_OUT pulse
            ufs_diag[16] = rep.uec_pack;       // UECDL|UECN|UECT|UECDME low bytes
            ufs_diag[17] = rep.avail_rx;
            ufs_diag[18] = rep.ls_cnf;
            ufs_diag[19] = rep.nop_ocs as u32;
            ufs_diag[20] = r(0x30);
            ufs_diag[21] = rep.hcs_after_link;
            ufs_diag[22] = r(0x38);
            ufs_diag[23] = rep.linkstartup_tries;
            ufs_diag[27] = rep.uecpa;
            clkdiag[0] = rep.clkstop_ctrl;
            clkdiag[1] = rep.force_hcs;
            clkdiag[2] = rep.mphy_refclk_sel;
            clkdiag[3] = rep.cmu_qch;
            clkdiag[4] = rep.cmu_unipro_gate;
            ufs_diag[13] = rep.pcs_readback; // PCS cal readback (expect low bytes F6, 79, 02)
            ufs_diag[12] = rep.gph5_con_before; // pinmux ABL left (gph5-0 nibble != 2 = refclk de-routed)
            ufs_diag[14] = rep.gph5_con_after;  // after we route both to function 2
            // MAXRXHSGEAR after this init attempt (nonzero = the device finally advertised = refclk routing fixed it).
            ufs_diag[11] = unsafe { core::ptr::read_volatile((0x1328_0000 + 0x321C) as *const u32) };
            pmaf = ufs.snapshot_pma(); // failed-link PMA/PA state for the diff against pmaw
        }
    }

    // ---- DWC3 USB init ----
    ferros_hal::usb::set_dwc3_base(DWC3);
    let mut usb = ferros_hal::usb::Dwc3Dev::init();

    // ---- Killswitch + USB event loop with PT command dispatch ----
    // Both volume buttons held = SYSTEM_OFF (killswitch). PT commands: DIAG (live diagnostics over USB), RELOAD (stage kernel — Write counts, Exec/jump is TODO), REBOOT (PSCI SYSTEM_RESET via smc — Tensor has EL3). Mirrors the M1 dispatch loop (m1_usb_event_loop).
    use ferros_hal::usb::UsbEvent;

    // Killswitch: both volume buttons held → PSCI SYSTEM_OFF. Checked once per iteration.
    macro_rules! killswitch_check {
        () => {{
            let gpa4 = unsafe { core::ptr::read_volatile(GPA4_DAT as *const u32) };
            let gpa6 = unsafe { core::ptr::read_volatile(GPA6_DAT as *const u32) };
            if (gpa4 & VOL_DOWN_BIT) == 0 && (gpa6 & VOL_UP_BIT) == 0 {
                unsafe { core::arch::asm!("ldr x0, =0x84000008", "smc #0", options(noreturn)); }
            }
        }};
    }

    // If USB init failed, still honor the killswitch.
    let mut usb = match usb {
        Some(u) => u,
        None => loop {
            killswitch_check!();
            for _ in 0..1024u32 { core::hint::spin_loop(); }
        },
    };

    // ---- Enumeration watchdog ---- The eUSB2 PHY init has a PLL-lock race (blind udelays), so some boots come up marginal: either fully silent, or attach succeeds but EP0 wedges (host logs descriptor-read timeouts and gives up). Instead of forcing a reboot, re-run PHY + DWC3 init after 4 seconds of USB silence without reaching the configured state. The re-init drops us off the bus, so the host sees a fresh attach and restarts enumeration from scratch — this heals both failure modes. Disarm only on SET_CONFIGURATION (a bus reset is NOT proof the data path works — tonight's wedge attached fine and then died in EP0). Any USB event counts as progress and pushes the deadline, so an in-flight enumeration is never torn down mid-exchange. Generic timer: CNTPCT_EL0 counts at CNTFRQ_EL0 regardless of core clock.
    let cntfrq: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) cntfrq); }
    let cntpct = || {
        let v: u64;
        unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) v); }
        v
    };
    // Guard against unprogrammed CNTFRQ (would make the timeout zero and re-init every iteration): fall back to the Tensor G3 arch timer rate, 24.576 MHz.
    let cntfrq = if cntfrq == 0 { 24_576_000 } else { cntfrq };
    // 2s of total silence = dead attempt. Forensics (2026-08-15) showed failed inits produce ZERO events — the analog path never lights up (LTSTATE/LINKDBG identical to healthy, DSTS bit 17 never sets) — while a good attempt draws the host's bus reset well under a second after attach. Any event pushes the deadline, so slow-but-alive enumeration is never torn down.
    let enum_timeout = cntfrq * 2;
    // Second bound: the EP0-wedge failure mode keeps drawing host descriptor-read retries (each an event, each pushing the silence deadline), riding a doomed attempt for 15s+ while the host slowly gives up. A healthy handshake reaches SET_CONFIGURATION well under a second after attach, so 4s since the last init without configuring means the attempt is bad no matter how chatty it looks.
    let wedge_timeout = cntfrq * 4;
    let mut t_init = cntpct();
    let mut t_progress = cntpct();
    ferros_hal::usb::dbg_set(3, 1); // PHY init attempts (first one already done above)

    // PT inbound-transfer state.
    let mut pt_data_buf: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut pt_bitmap_buf: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    let mut pt_inbound: Option<InboundTransfer<'_>> = None;
    let mut pt_seq_width: usize = 0;
    let mut pt_out_data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();

    let cap_diag = ferros_pt::command::dev_cap(ferros_pt::command::caps::DIAG);
    let cap_reload = ferros_pt::command::dev_cap(ferros_pt::command::caps::RELOAD);
    let cap_reboot = ferros_pt::command::dev_cap(ferros_pt::command::caps::REBOOT);
    let cap_run = ferros_pt::command::dev_cap(ferros_pt::command::caps::RUN);
    let mut reload_size: usize = 0;
    // RELOAD and RUN share the staging slot (the free ping-pong slot is THE staging ground for the next executable thing). At most one of these flags is set at a time, so a staged payload can never be jumped-to as a kernel or vice versa.
    let mut reload_ok = false;
    let mut run_ok = false;
    let mut run_size: usize = 0;
    // Enumeration-failure forensics: (LTSTATE_HIS, LINK_DEBUG_L, DSTS) captured at the moment the watchdog declares an init attempt dead, right before re-init. DIAG dumps the collection — flaky boots document themselves. Capped at 32 attempts.
    let mut init_history: alloc::vec::Vec<(u32, u32, u32)> = alloc::vec::Vec::new();

    // Hot-reload staging: ping-pong between exactly two proven slots. The ABL load base is proven (every cold boot runs there) and base+32MiB is proven by the first hot reload; anything further up is NOT — a jump to base+64MiB died silently on hardware (staging writes verified by hash, execution never came back). So each generation stages into the slot its predecessor vacated. The handoff rides the jump's x0: ABL passes a DTB pointer (8-aligned) or 0, the reloader passes its own base with bit 0 set — the tag says which boot path we came from and where the free slot is. Kernel footprint is image (~300KB flat + bss) + 64KB stack + 4MiB heap, well under the 32MiB slot pitch. MMU is off on Tensor (ABL disables it before the jump), so staging writes are straight physical stores.
    unsafe extern "C" { static _start: u8; }
    let own_base = &raw const _start as usize;
    let reload_stage = if dtb_addr & 1 == 1 {
        (dtb_addr & !1) as usize // hot-reload handoff: predecessor's base is now the free slot
    } else {
        own_base + (32 << 20) // ABL cold boot: the +32MiB slot is free
    };

    // Append "LABEL=<8 hex digits>\n" to a byte vec.
    let append_hex = |log: &mut alloc::vec::Vec<u8>, label: &[u8], val: u32| {
        log.extend_from_slice(label);
        let hex = b"0123456789ABCDEF";
        for i in (0..8).rev() { log.push(hex[((val >> (i * 4)) & 0xF) as usize]); }
        log.push(b'\n');
    };

    // Frame a response as a PT outbound transfer: SPEC + DATA packets each padded to exactly 512 so the idle pump's 512-byte chunks align with packet boundaries. Blast mode — bridge pt_recv takes SPEC + DATA with no ACKs; no FIN, since USB bulk never drops packets and a trailing FIN would sit unread and poison the next command's first recv.
    let queue_pt_response = |out: &mut alloc::vec::Vec<u8>, payload: &[u8]| {
        out.clear();
        let mut ob_bitmap = alloc::vec![0u64; ferros_pt::transfer::outbound_bitmap_words(payload.len())];
        let mut spec_buf = [0u8; 512];
        if let Some((mut ob, spec_len)) = OutboundTransfer::start(ferros_pt::StreamId::FIRST, payload, &mut ob_bitmap, &mut spec_buf) {
            out.extend_from_slice(&spec_buf[..spec_len]);
            out.resize(512, 0);
            let mut pkt = [0u8; 512];
            while !ob.all_sent() {
                let plen = ob.next_data_packet(payload, &mut pkt);
                if plen == 0 { break; }
                let start = out.len();
                out.extend_from_slice(&pkt[..plen]);
                out.resize(start + 512, 0);
            }
        }
    };

    loop {
        // Pet the recoverable watchdog: reload both cluster counters every iteration. This loop spins continuously, so a genuine hang (a payload that never returns, a hung MMIO read) stops the pets → ~60s auto-reset instead of a frozen phone.
        for (i, &wdt) in WATCHDOGS.iter().enumerate() {
            unsafe { core::ptr::write_volatile((wdt + WTCNT) as *mut u32, wdt_reload[i]); }
        }
        killswitch_check!();

        match usb.poll_event() {
            UsbEvent::None => {
                // Enumeration watchdog: not configured + 4s without any USB event → full PHY + DWC3 re-init (fresh attach from the host's perspective).
                let now = cntpct();
                if !usb.is_configured()
                    && (now.wrapping_sub(t_progress) > enum_timeout || now.wrapping_sub(t_init) > wedge_timeout)
                {
                    if init_history.len() < 32 {
                        unsafe {
                            init_history.push((
                                core::ptr::read_volatile((USBCON + 0x80) as *const u32),  // LTSTATE_HIS
                                core::ptr::read_volatile((USBCON + 0x84) as *const u32),  // LINK_DEBUG_L
                                core::ptr::read_volatile((DWC3 + 0xC70C) as *const u32), // DSTS
                            ));
                        }
                    }
                    eusb_phy_init();
                    if let Some(nu) = ferros_hal::usb::Dwc3Dev::init() { usb = nu; }
                    ferros_hal::usb::dbg_bump(3);
                    pt_inbound = None;
                    pt_seq_width = 0;
                    pt_out_data.clear();
                    t_init = cntpct();
                    t_progress = t_init;
                }
                // Pump a pending outbound response (DIAG payload), 512 bytes per bulk IN.
                if !pt_out_data.is_empty() && usb.bulk_in_is_idle() {
                    let len = pt_out_data.len().min(512);
                    let chunk: alloc::vec::Vec<u8> = pt_out_data.drain(..len).collect();
                    usb.bulk_in_send(&chunk);
                }
                for _ in 0..64u32 { core::hint::spin_loop(); }
            }
            UsbEvent::Reset => {
                t_progress = cntpct();
                usb.handle_reset();
                pt_inbound = None;
                pt_seq_width = 0;
                pt_out_data.clear();
            }
            UsbEvent::ConnectDone { .. } => {
                t_progress = cntpct();
                usb.handle_connect_done();
                usb.ep0_start_setup();
                usb.bulk_out_arm();
            }
            UsbEvent::Disconnect => {
                usb.handle_disconnect();
                pt_inbound = None;
                pt_seq_width = 0;
                pt_out_data.clear();
            }
            UsbEvent::Ep0Setup { request } => {
                t_progress = cntpct();
                if !usb.handle_setup(&request) {
                    usb.ep0_stall();
                }
            }
            UsbEvent::TransferComplete { ep } => {
                t_progress = cntpct();
                if ep == 2 {
                    use ferros_hal::usb::{dbg_bump, dbg_set};
                    // Bulk OUT — a PT packet (SPEC opens a transfer, DATA fills it).
                    // Debug slots (VENDOR_REQ_DBG): 0=ep2 events, 1=read None, 2=(len<<8)|first_byte, 4=SPEC ok, 5=ACK send 1ok/2drop, 6=DATA seen, 7=decode ok, 8=chunk accepted, 9=all_received, 10=finish len, 11=COMPLETE send 1ok/2drop.
                    dbg_bump(0);
                    let mut tmp = [0u8; 512];
                    let mut n = 0usize;
                    if let Some(data) = usb.bulk_out_read() {
                        n = data.len().min(512);
                        tmp[..n].copy_from_slice(&data[..n]);
                    } else {
                        dbg_bump(1);
                    }
                    dbg_set(2, ((n as u32) << 8) | tmp[0] as u32);
                    if n > 0 {
                        if ferros_pt::is_data_packet(tmp[0]) {
                            dbg_bump(6);
                            if let Some(ref mut xfer) = pt_inbound {
                                if let Some((_sid, seq, hash, payload)) = packet::decode_data(&tmp[..n], pt_seq_width) {
                                    dbg_bump(7);
                                    if xfer.handle_data(seq, &hash, payload) {
                                        dbg_bump(8);
                                    }
                                    if xfer.all_received() {
                                        dbg_bump(9);
                                        let mut complete_buf = [0u8; 512];
                                        let clen = xfer.finish(&mut complete_buf);
                                        dbg_set(10, clen as u32);
                                        if clen > 0 {
                                            let ok = usb.bulk_in_send(&complete_buf[..clen]);
                                            dbg_set(11, if ok { 1 } else { 2 });
                                        }
                                        // Dispatch the completed command.
                                        let payload = xfer.payload();
                                        if let Some(cmd) = ferros_pt::command::parse(payload) {
                                            if cmd.cap == cap_diag && cmd.op == ferros_pt::Op::Read {
                                                // Live diagnostics: DWC3 state + exception count.
                                                let mut resp: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                                                resp.extend_from_slice(b"ferros on Pixel 8 Pro (Tensor G3)\nUSB: eUSB2 PHY up, enumerated\n");
                                                let snpsid = unsafe { core::ptr::read_volatile((DWC3 + 0xC120) as *const u32) };
                                                let gsts = unsafe { core::ptr::read_volatile((DWC3 + 0xC118) as *const u32) };
                                                let dsts = unsafe { core::ptr::read_volatile((DWC3 + 0xC70C) as *const u32) };
                                                append_hex(&mut resp, b"SNPSID=", snpsid);
                                                append_hex(&mut resp, b"GSTS=", gsts);
                                                append_hex(&mut resp, b"DSTS=", dsts);
                                                append_hex(&mut resp, b"EXC=", exception_count().wrapping_sub(exc_start) as u32);
                                                append_hex(&mut resp, b"BASE=", own_base as u32);
                                                append_hex(&mut resp, b"STAGE=", reload_stage as u32);
                                                for (i, &(lt, dbg, dsts)) in init_history.iter().enumerate() {
                                                    append_hex(&mut resp, b"FAIL_ATT=", i as u32);
                                                    append_hex(&mut resp, b"  LTSTATE=", lt);
                                                    append_hex(&mut resp, b"  LINKDBG=", dbg);
                                                    append_hex(&mut resp, b"  DSTS=", dsts);
                                                }
                                                // Repeater state as captured before PHY init this boot (REV_ID + CONFIG_PORT1 + tune set; G#8000_xxxx = I2C read failed). Baseline in REPEATER.md.
                                                append_hex(&mut resp, b"REP_REV=", rep_snap[0]);
                                                append_hex(&mut resp, b"REP_CFG=", rep_snap[1]);
                                                append_hex(&mut resp, b"REP_T70=", rep_snap[2]);
                                                append_hex(&mut resp, b"REP_T71=", rep_snap[3]);
                                                append_hex(&mut resp, b"REP_T72=", rep_snap[4]);
                                                append_hex(&mut resp, b"REP_T73=", rep_snap[5]);
                                                append_hex(&mut resp, b"REP_T77=", rep_snap[6]);
                                                append_hex(&mut resp, b"REP_T78=", rep_snap[7]);
                                                append_hex(&mut resp, b"REP_T79=", rep_snap[8]);
                                                append_hex(&mut resp, b"REP_TUNED=", rep_tuned);
                                                // UFS: Phase A = live-link NOP (no reset); Phase B (only if A fails) = full_init. Success (live) = UFS_LIVE_OCS=0 + UFS_DONE=00A1100D + UFS_DATA0/1="EFI PART".
                                                // pbl-style descriptor-reuse NOP (ABL's own UCD, no rebase). REUSE_OCS=0 = transfer completed → our UFS_BUF region was unreachable by the UFS DMA master (S2MPU/protected region) and rebasing was the bug.
                                                // External-block ABL-handoff state vs Linux's probe-time values: IOCC expect Linux=3, PHYISO expect 1, UFSP_RSEC Linux=G#FFFA6492, UFSP_WSEC Linux=0.
                                                append_hex(&mut resp, b"UFS_DBG_PRD=", dbg_prd_pristine);
                                                append_hex(&mut resp, b"UFS_EXT_IOCC=", ext[0]);
                                                append_hex(&mut resp, b"UFS_EXT_PHYISO=", ext[1]);
                                                append_hex(&mut resp, b"UFS_EXT_UFSP_RSEC=", ext[2]);
                                                append_hex(&mut resp, b"UFS_EXT_UFSP_WSEC=", ext[3]);
                                                append_hex(&mut resp, b"UFS_EXT_S2MPU_CTRL=", ext[4]);
                                                append_hex(&mut resp, b"UFS_EXT_GPH5CON=", ext[5]);
                                                append_hex(&mut resp, b"UFS_PD_PERIC0=", ext[6]);
                                                append_hex(&mut resp, b"UFS_QCH_P_PERIC0=", ext[7]);
                                                append_hex(&mut resp, b"UFS_CMU_PLL_SH0=", cmu[0]);
                                                append_hex(&mut resp, b"UFS_CMU_PLL_SH2=", cmu[1]);
                                                append_hex(&mut resp, b"UFS_CMU_PLL_SPARE=", cmu[2]);
                                                append_hex(&mut resp, b"UFS_CMU_TOP_MUX=", cmu[3]);
                                                append_hex(&mut resp, b"UFS_CMU_TOP_DIV=", cmu[4]);
                                                append_hex(&mut resp, b"UFS_CMU_TOP_GATE=", cmu[5]);
                                                append_hex(&mut resp, b"UFS_CMU_USER_MUX=", cmu[6]);
                                                append_hex(&mut resp, b"UFS_CMU_LEAF_ACLK=", cmu[7]);
                                                append_hex(&mut resp, b"UFS_CMU_LEAF_UNIPRO=", cmu[8]);
                                                append_hex(&mut resp, b"UFS_CMU_LEAF_FMP=", cmu[9]);
                                                // IOCC coherency clear (candidate fix): if CLEAN/HITEST/REUSE OCS now land 0, the coherent-DMA-snoop-stall was the dead-doorbell root cause.
                                                // HIBERN8-exit probe: was ABL parking the link in hibernate? H8_NOP_OCS=0 after exit ⇒ yes, that was the dead-doorbell cause. PA_CTRLSTATE/PWRMODE show the link state before/after.
                                                // ABL pristine register file — why the transfer manager sits idle. UTRLCLR (slot-clear) must read with slot0 bit SET to run.
                                                append_hex(&mut resp, b"UFS_RF_CAP=", regfile[0]);
                                                append_hex(&mut resp, b"UFS_RF_IS=", regfile[2]);
                                                append_hex(&mut resp, b"UFS_RF_IE=", regfile[3]);
                                                append_hex(&mut resp, b"UFS_RF_HCS=", regfile[4]);
                                                append_hex(&mut resp, b"UFS_RF_HCE=", regfile[5]);
                                                append_hex(&mut resp, b"UFS_RF_UTRLBA=", regfile[6]);
                                                append_hex(&mut resp, b"UFS_RF_UTRLDBR=", regfile[8]);
                                                append_hex(&mut resp, b"UFS_RF_UTRLCLR=", regfile[9]);
                                                append_hex(&mut resp, b"UFS_RF_UTRLRSR=", regfile[10]);
                                                append_hex(&mut resp, b"UFS_RF_UTMRLRSR=", regfile[12]);
                                                append_hex(&mut resp, b"UFS_RF_UICCMD=", regfile[13]);
                                                append_hex(&mut resp, b"UFS_CLR_NOP_OCS=", clr_nop[0]);
                                                append_hex(&mut resp, b"UFS_CLR_NOP_IS=", clr_nop[2]);
                                                append_hex(&mut resp, b"UFS_CLR_NOP_DBR=", clr_nop[3]);
                                                append_hex(&mut resp, b"UFS_H8_PACS_BEFORE=", h8[0]);
                                                append_hex(&mut resp, b"UFS_H8_PWRMODE_BEFORE=", h8[1]);
                                                append_hex(&mut resp, b"UFS_H8_UIC_RES=", h8[2]);
                                                append_hex(&mut resp, b"UFS_H8_UPMS_DONE=", h8[3]);
                                                append_hex(&mut resp, b"UFS_H8_PACS_AFTER=", h8[4]);
                                                append_hex(&mut resp, b"UFS_H8_HCS_AFTER=", h8[5]);
                                                append_hex(&mut resp, b"UFS_H8_NOP_OCS=", h8_nop[0]);
                                                append_hex(&mut resp, b"UFS_H8_NOP_RSP=", h8_nop[1]);
                                                append_hex(&mut resp, b"UFS_H8_NOP_IS=", h8_nop[2]);
                                                append_hex(&mut resp, b"UFS_H8_NOP_DBR=", h8_nop[3]);
                                                append_hex(&mut resp, b"UFS_IOCC_BEFORE=", iocc_fix[0]);
                                                append_hex(&mut resp, b"UFS_IOCC_AFTER=", iocc_fix[1]);
                                                // Exynos DMA-engine state captured right after the (possibly stalled) clean-NOP doorbell.
                                                append_hex(&mut resp, b"UFS_DMA_FSM=", dma_dbg[0]);
                                                append_hex(&mut resp, b"UFS_DMA0_STATE=", dma_dbg[1]);
                                                append_hex(&mut resp, b"UFS_DMA0_CNT=", dma_dbg[2]);
                                                append_hex(&mut resp, b"UFS_DMA0_DBELL=", dma_dbg[3]);
                                                append_hex(&mut resp, b"UFS_VENDOR_IS=", dma_dbg[4]);
                                                append_hex(&mut resp, b"UFS_AXI_IF_CTRL=", dma_dbg[5]);
                                                // High-DRAM reachability probe: does a doorbell against a G#9000_0000 descriptor complete on ABL's live link? HITEST_OCS=0 while CLEAN_OCS=F ⇒ ferros's low UFS_BUF was unreachable; relocate it high.
                                                append_hex(&mut resp, b"UFS_HITEST_OCS=", hitest[0]);
                                                append_hex(&mut resp, b"UFS_HITEST_IS=", hitest[1]);
                                                append_hex(&mut resp, b"UFS_HITEST_DBR=", hitest[2]);
                                                append_hex(&mut resp, b"UFS_HITEST_DONE=", hitest[3]);
                                                append_hex(&mut resp, b"UFS_CLEAN_OCS=", clean[0]);
                                                append_hex(&mut resp, b"UFS_CLEAN_RSP=", clean[1]);
                                                append_hex(&mut resp, b"UFS_CLEAN_IS=", clean[2]);
                                                append_hex(&mut resp, b"UFS_CLEAN_DBR=", clean[3]);
                                                append_hex(&mut resp, b"UFS_REUSE_UTRLBA=", reuse[0]);
                                                append_hex(&mut resp, b"UFS_REUSE_UTRD_DW0=", reuse[10]);
                                                append_hex(&mut resp, b"UFS_REUSE_UTRD_DW2=", reuse[11]);
                                                append_hex(&mut resp, b"UFS_REUSE_PROTEN=", reuse[12]);
                                                append_hex(&mut resp, b"UFS_REUSE_PROTEN2=", reuse[13]);
                                                append_hex(&mut resp, b"UFS_REUSE_UCD_LO=", reuse[2]);
                                                append_hex(&mut resp, b"UFS_REUSE_UCD_HI=", reuse[3]);
                                                append_hex(&mut resp, b"UFS_REUSE_DBR_B=", reuse[4]);
                                                append_hex(&mut resp, b"UFS_REUSE_OCS=", reuse[5]);
                                                append_hex(&mut resp, b"UFS_REUSE_IS=", reuse[6]);
                                                append_hex(&mut resp, b"UFS_REUSE_DBR_A=", reuse[7]);
                                                append_hex(&mut resp, b"UFS_REUSE_DONE=", reuse[8]);
                                                append_hex(&mut resp, b"UFS_REUSE_RSR=", reuse[9]);
                                                append_hex(&mut resp, b"UFS_LIVE_OCS=", ufs_diag[0]);
                                                append_hex(&mut resp, b"UFS_LIVE_RSP=", ufs_diag[1]);
                                                append_hex(&mut resp, b"UFS_LIVE_IS=", ufs_diag[2]);
                                                append_hex(&mut resp, b"UFS_LIVE_DBR=", ufs_diag[3]);
                                                append_hex(&mut resp, b"UFS_CTRL_UTRLBA=", ufs_diag[4]);
                                                append_hex(&mut resp, b"UFS_CTRL_UTRLBAU=", ufs_diag[5]);
                                                append_hex(&mut resp, b"UFS_LIVE_UECPA=", ufs_diag[6]);
                                                append_hex(&mut resp, b"UFS_DBR_DUP=", ufs_diag[7]);
                                                append_hex(&mut resp, b"UFS_BUF_PHYS_HI=", ufs_diag[24]);
                                                append_hex(&mut resp, b"UFS_BUF_PHYS_LO=", ufs_diag[25]);
                                                append_hex(&mut resp, b"UFS_HCS_PRISTINE=", ufs_diag[26]);
                                                append_hex(&mut resp, b"UFS_STEPS=", ufs_diag[8]);
                                                append_hex(&mut resp, b"UFS_FAIL=", ufs_diag[9]);
                                                append_hex(&mut resp, b"UFS_LS_RES=", ufs_diag[10]);
                                                append_hex(&mut resp, b"UFS_MXGR_AFTER=", ufs_diag[11]);
                                                append_hex(&mut resp, b"UFS_GPH5CON_BEFORE=", ufs_diag[12]);
                                                append_hex(&mut resp, b"UFS_PCS_READBACK=", ufs_diag[13]);
                                                append_hex(&mut resp, b"UFS_GPH5CON_AFTER=", ufs_diag[14]);
                                                append_hex(&mut resp, b"UFS_GPH5_DAT=", ufs_diag[15]);
                                                append_hex(&mut resp, b"UFS_UEC_PACK=", ufs_diag[16]);
                                                append_hex(&mut resp, b"UFS_AVAIL_RX=", ufs_diag[17]);
                                                append_hex(&mut resp, b"UFS_LS_CNF=", ufs_diag[18]);
                                                append_hex(&mut resp, b"UFS_NOP_OCS=", ufs_diag[19]);
                                                append_hex(&mut resp, b"UFS_HCS=", ufs_diag[20]);
                                                append_hex(&mut resp, b"UFS_HCS_LINK=", ufs_diag[21]);
                                                append_hex(&mut resp, b"UFS_UECPA=", ufs_diag[22]);
                                                append_hex(&mut resp, b"UFS_LS_TRIES=", ufs_diag[23]);
                                                append_hex(&mut resp, b"UFS_DONE=", ufs_diag[27]);
                                                // Clock/refclk state at link-startup. REFCLK is what the device needs to respond. CLKSTOP bit4=REFCLKOUT_STOP (want 0). CMU gates want nonzero/enabled.
                                                append_hex(&mut resp, b"UFS_CLKSTOP=", clkdiag[0]);
                                                append_hex(&mut resp, b"UFS_FORCEHCS=", clkdiag[1]);
                                                append_hex(&mut resp, b"UFS_MPHY_REFCLK_SEL=", clkdiag[2]);
                                                append_hex(&mut resp, b"UFS_CMU_QCH=", clkdiag[3]);
                                                append_hex(&mut resp, b"UFS_CMU_UNIPRO_GATE=", clkdiag[4]);
                                                // Device-reset test on ABL's live link (non-cached). If RST_UEC_A latches nonzero OR RST_HCS_A drops DP (G#10E/G#010E vs G#10F), GPIO_OUT reset reached the device (good). If RST_UEC_A=0 and RST_HCS_A=G#10F unchanged, GPIO_OUT does NOT reach the device reset_n — the device never drops its ABL link state = root cause.
                                                append_hex(&mut resp, b"UFS_RST_UEC_B=", rst_test[0]);
                                                append_hex(&mut resp, b"UFS_RST_HCS_B=", rst_test[1]);
                                                append_hex(&mut resp, b"UFS_RST_UEC_A=", rst_test[2]);
                                                append_hex(&mut resp, b"UFS_RST_HCS_A=", rst_test[3]);
                                                // PMA/PA register diff: W=ABL working link (Phase A), F=post-full_init failed (Phase B). Order: PMA 000/140/150/19C/1A0/C74, PMA-lane0 9F0/9F4/A00, PA_CTRLSTATE, PA_TX_STATE, MAXRXHSGEAR.
                                                let pma_labels: [&[u8]; 12] = [b"P000", b"P140", b"P150", b"P19C", b"P1A0", b"PC74", b"P9F0", b"P9F4", b"PA00", b"PACS", b"PATX", b"MXGR"];
                                                for i in 0..12 {
                                                    let mut lw = [0u8; 16]; let mut n = 0;
                                                    for &b in b"UFS_W_" { lw[n] = b; n += 1; }
                                                    for &b in pma_labels[i] { lw[n] = b; n += 1; }
                                                    append_hex(&mut resp, &lw[..n], pmaw[i]);
                                                }
                                                for i in 0..12 {
                                                    let mut lf = [0u8; 16]; let mut n = 0;
                                                    for &b in b"UFS_F_" { lf[n] = b; n += 1; }
                                                    for &b in pma_labels[i] { lf[n] = b; n += 1; }
                                                    append_hex(&mut resp, &lf[..n], pmaf[i]);
                                                }
                                                // Recoverable watchdog state (ABL's config, reused). WTCON bit5=EN bit0=RSTEN; WTDAT = reload (~60s window).
                                                append_hex(&mut resp, b"WDT0_CON=", wdt_con[0]);
                                                append_hex(&mut resp, b"WDT0_DAT=", wdt_reload[0]);
                                                append_hex(&mut resp, b"WDT1_CON=", wdt_con[1]);
                                                append_hex(&mut resp, b"WDT1_DAT=", wdt_reload[1]);
                                                // ferros-side FWTRACE: every MMIO access full_init issued, in order, to diff against Linux's captured working trace. Addr bit31 set = READ (FRTRACE), clear = WRITE (FWTRACE).
                                                let n = ferros_hal::ufs_cal::trace_len().min(600);
                                                append_hex(&mut resp, b"UFS_TRACE_N=", ferros_hal::ufs_cal::trace_len() as u32);
                                                for i in 0..n {
                                                    let (a, v) = ferros_hal::ufs_cal::trace_get(i);
                                                    let mut lbl = [0u8; 16]; let mut ln = 0;
                                                    for &b in b"FTRACE_" { lbl[ln] = b; ln += 1; }
                                                    // encode addr as hex into the label so each line is unique
                                                    let hexd = b"0123456789ABCDEF";
                                                    for shift in [28,24,20,16,12,8,4,0] {
                                                        lbl[ln] = hexd[((a >> shift) & 0xF) as usize]; ln += 1;
                                                    }
                                                    lbl[ln] = b'='; ln += 1;
                                                    append_hex(&mut resp, &lbl[..ln], v);
                                                }
                                                // Full PMA analog capture (256 regs) at ferros's failed-linkstartup point, to diff against the debug kernel's FPMA dump. Label PMA_C<off>/PMA_T<off> mirrors the kernel's C/T tags. [0..128]=COMN 0x000-0x1FC, [128..256]=TRSV0 0x800-0x9FC.
                                                let hexd = b"0123456789ABCDEF";
                                                for i in 0..384usize {
                                                    let v = unsafe { ferros_hal::ufs_cal::PMA_FULL[i] };
                                                    let (tag, off) = if i < 128 { (b'C', (i * 4) as u32) } else { (b'T', (0x800 + (i - 128) * 4) as u32) };
                                                    let mut lbl = [0u8; 16]; let mut ln = 0;
                                                    for &b in b"PMA_" { lbl[ln] = b; ln += 1; }
                                                    lbl[ln] = tag; ln += 1;
                                                    for shift in [12,8,4,0] { lbl[ln] = hexd[((off >> shift) & 0xF) as usize]; ln += 1; }
                                                    lbl[ln] = b'='; ln += 1;
                                                    append_hex(&mut resp, &lbl[..ln], v);
                                                }
                                                resp.extend_from_slice(b"END\n");
                                                queue_pt_response(&mut pt_out_data, &resp);
                                            } else if cmd.cap == cap_reload && cmd.op == ferros_pt::Op::Write {
                                                // Stage the flat kernel image. The PT transfer already verified per-chunk + root BLAKE3; re-hash the staged copy against the source (write-verify — catches a bad copy, not a bad transfer). Sanity-gate on the ARM64 header MZ magic so a stray Write can't stage garbage.
                                                let params = cmd.params;
                                                let is_kernel = params.len() > 0x1000
                                                    && params[0] == 0x4D && params[1] == 0x5A && params[2] == 0x00 && params[3] == 0x91;
                                                if is_kernel {
                                                    let src_hash = blake3::hash(params);
                                                    unsafe {
                                                        core::ptr::copy_nonoverlapping(params.as_ptr(), reload_stage as *mut u8, params.len());
                                                    }
                                                    let staged = unsafe { core::slice::from_raw_parts(reload_stage as *const u8, params.len()) };
                                                    reload_ok = blake3::hash(staged) == src_hash;
                                                    reload_size = params.len();
                                                } else {
                                                    reload_ok = false;
                                                    reload_size = 0;
                                                }
                                                // Staged-size ack, PT-framed (bridge pt_recv reads 4 LE bytes; 0 = rejected).
                                                let ack = (if reload_ok { reload_size as u32 } else { 0 }).to_le_bytes();
                                                queue_pt_response(&mut pt_out_data, &ack);
                                            } else if cmd.cap == cap_reload && cmd.op == ferros_pt::Op::Exec {
                                                if reload_ok && reload_size > 0 {
                                                    // Flush this Exec's COMPLETE out the wire before tearing USB down: poll until the bulk IN that carries it finishes, then give the host a grace period to read it.
                                                    let mut spins = 0u32;
                                                    while !usb.bulk_in_is_idle() && spins < (1 << 20) {
                                                        let _ = usb.poll_event();
                                                        spins += 1;
                                                    }
                                                    for _ in 0..(1 << 24) { core::hint::spin_loop(); }
                                                    unsafe {
                                                        // Graceful detach: clear DCTL Run/Stop so the host sees a disconnect, not a dead device.
                                                        let dctl = core::ptr::read_volatile((DWC3 + 0xC704) as *const u32);
                                                        core::ptr::write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
                                                        // MMU + caches are off (Tensor ABL entry state), so the staged bytes are already in DRAM; invalidate the icache and jump to the staged image's ARM64 header (code0/code1 branch to _entry, fully PC-relative — it zeroes its own bss and sets its own stack at the new base). x0 = 0: no DTB.
                                                        core::arch::asm!(
                                                            "dsb sy",
                                                            "ic iallu",
                                                            "dsb sy",
                                                            "isb",
                                                            "br {stage}",
                                                            stage = in(reg) reload_stage,
                                                            in("x0") own_base | 1, // handoff tag: our base becomes the successor's free slot
                                                            options(noreturn)
                                                        );
                                                    }
                                                }
                                                // Nothing staged (or verify failed): fall through, COMPLETE already told the bridge the transfer landed; the 0-size ack from Write is the rejection signal.
                                            } else if cmd.cap == cap_run && cmd.op == ferros_pt::Op::Write {
                                                // Stage an arbitrary payload blob into the shared staging slot (overwrites any staged kernel — dev semantics: last Write wins). Size-capped to the 16MiB slot budget; write-verified like a kernel.
                                                let params = cmd.params;
                                                if !params.is_empty() && params.len() <= (16 << 20) {
                                                    let src_hash = blake3::hash(params);
                                                    unsafe {
                                                        core::ptr::copy_nonoverlapping(params.as_ptr(), reload_stage as *mut u8, params.len());
                                                    }
                                                    let staged = unsafe { core::slice::from_raw_parts(reload_stage as *const u8, params.len()) };
                                                    run_ok = blake3::hash(staged) == src_hash;
                                                    run_size = params.len();
                                                } else {
                                                    run_ok = false;
                                                    run_size = 0;
                                                }
                                                reload_ok = false;
                                                let ack = (if run_ok { run_size as u32 } else { 0 }).to_le_bytes();
                                                queue_pt_response(&mut pt_out_data, &ack);
                                            } else if cmd.cap == cap_run && cmd.op == ferros_pt::Op::Exec {
                                                // CALL the staged payload (unlike RELOAD Exec's one-way jump). ABI: extern "C" fn(in_ptr, in_len, out_ptr, out_cap) -> u64, entry at blob offset 0, PC-relative code only, same EL, full dev trust. Exec params are the payload's input. No preemption exists — a payload that never returns hangs the kernel (dev tool; recover via hard reboot).
                                                if run_ok && run_size > 0 {
                                                    unsafe {
                                                        core::arch::asm!("dsb sy", "ic iallu", "dsb sy", "isb");
                                                    }
                                                    let mut out = alloc::vec![0u8; 4096];
                                                    let entry: extern "C" fn(*const u8, usize, *mut u8, usize) -> u64 =
                                                        unsafe { core::mem::transmute(reload_stage) };
                                                    let ret = entry(cmd.params.as_ptr(), cmd.params.len(), out.as_mut_ptr(), out.len());
                                                    let n = (ret as usize).min(out.len());
                                                    // Response: [ret: 8 LE][out bytes up to ret, clamped].
                                                    let mut resp: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(8 + n);
                                                    resp.extend_from_slice(&ret.to_le_bytes());
                                                    resp.extend_from_slice(&out[..n]);
                                                    queue_pt_response(&mut pt_out_data, &resp);
                                                } else {
                                                    queue_pt_response(&mut pt_out_data, b"ERR:NOPAYLOAD");
                                                }
                                            } else if cmd.cap == cap_reboot && cmd.op == ferros_pt::Op::Exec {
                                                // Reboot. params[0] mode byte is accepted but NOT acted on yet: the fastboot-reason PMU write is pulled until a read-only probe (RUN payload) confirms the offset and that S2MPU passes the write. An earlier blind write of G#8000_00FC to G#1546_0810 left USB dead across warm resets — cold power cycle recovered it. Until proven, every reboot is a plain PSCI SYSTEM_RESET (Tensor: secure monitor at EL3 via smc).
                                                let _ = cmd.params.first().copied().unwrap_or(0);
                                                unsafe { core::arch::asm!("ldr x0, =0x84000009", "smc #0", options(noreturn)); }
                                            }
                                        }
                                        pt_inbound = None;
                                        pt_seq_width = 0;
                                    }
                                }
                            }
                        } else if ferros_pt::is_control_packet(tmp[0]) {
                            // SPEC — start a new inbound transfer, reply SPEC ACK.
                            if let Some(spec) = packet::Spec::decode(&tmp[..n]) {
                                dbg_bump(4);
                                pt_seq_width = ferros_ledger::ewe::seq_width(spec.count);
                                let bmw = ferros_pt::transfer::bitmap_words(spec.count);
                                pt_data_buf = alloc::vec![0u8; spec.total as usize];
                                pt_bitmap_buf = alloc::vec![0u64; bmw];
                                // SAFETY: pt_data_buf/pt_bitmap_buf live in this same scope as long as pt_inbound.
                                let xfer = unsafe {
                                    InboundTransfer::new(
                                        &spec,
                                        core::slice::from_raw_parts_mut(pt_data_buf.as_mut_ptr(), pt_data_buf.len()),
                                        core::slice::from_raw_parts_mut(pt_bitmap_buf.as_mut_ptr(), pt_bitmap_buf.len()),
                                    )
                                };
                                if let Some(xfer) = xfer {
                                    let ack = packet::Ack { sid: spec.sid, seq: u64::MAX };
                                    let mut ack_buf = [0u8; 64];
                                    let ack_len = ack.encode(&mut ack_buf);
                                    if ack_len > 0 {
                                        let ok = usb.bulk_in_send(&ack_buf[..ack_len]);
                                        dbg_set(5, if ok { 1 } else { 2 });
                                    }
                                    pt_inbound = Some(xfer);
                                } else {
                                    // Buffer sizing rejected the SPEC — no ACK goes out, so make the silence diagnosable.
                                    dbg_set(5, 3);
                                }
                            }
                        }
                    }
                    usb.bulk_out_arm();
                }
            }
            UsbEvent::TransferNotReady { .. } => {}
        }
    }
}

/// Hot-reload: jump to a new kernel image at the given DRAM address.
///
/// The new image is position-independent (uses adrp). We disable caches, flush the staging area, then branch to _start with x0 = DTB.
// kept: generic reload/reboot mechanism, not yet wired to the live path
#[allow(dead_code)]
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
// kept: generic reload/reboot mechanism, not yet wired to the live path
#[allow(dead_code)]
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

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop { unsafe { core::arch::asm!("wfe") }; }
}
