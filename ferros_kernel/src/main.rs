// FERROS SOURCE MAP — keep updated when pub items or files change
//
// ferros_kernel/
// └── main.rs ── kernel entry, boot sequence, USB event loop, SD probe
//       _start (asm), kernel_main()
//       Boot: EL check, DTB parse, FB console, UART, pstore, DPU, SPMI
//       GCC clocks, RPMh power, SDHCI probe, USB DWC3 init
//       USB event loop: PT command dispatch, hot-reload handler
//       SD probe: CMD0-CMD8-ACMD41-CMD2-CMD3-CMD9-CMD7-CMD16, R/W verify
//
// ferros_hal/ ── hardware abstraction (no_std, QCM6490/FP5)
// ├── lib.rs ── module re-exports
// ├── mmio.rs ── raw MMIO read/write (8/16/32-bit), cache ops
// ├── console.rs ── framebuffer text console, 8x16 VGA font, 2x scaling
// │   struct Console { fb_base, stride, x_off, y_off, col, row }
// │     ::new(), put_char(), scroll(), clear()
// ├── fb.rs ── raw framebuffer pixel ops
// ├── dtb.rs ── FDT/DTB parser (find nodes, read properties)
// ├── dpu.rs ── display processing unit register reads
// ├── uart.rs ── GENI UART TX (QUP1 SE5 at 0x994000)
// ├── pstore.rs ── ramoops/persistent_ram_buffer writer
// ├── spmi.rs ── SPMI arbiter v5 (observer reads, channel writes)
// │   find_apid(), read_byte(), write_byte(), ppid()
// ├── gcc.rs ── GCC clock controller (SDC2 branches + RCG)
// │   sdc2_set_400khz(), sdc2_set_25mhz(), sdc2_block_reset()
// ├── rpmh.rs ── RPMh TCS for LDO power enable via cmd-db
// │   enable_ldo(), cmd_db_lookup()
// ├── sdmmc.rs ── SD/MMC controller (SDHCI + Qualcomm vendor regs)
// │   struct SdmmcController { base, initialized, card_id, capacity, rca }
// │     ::new(), init(), probe_card() → ProbeResult
// │     ::read_block(), write_block(), read_blocks(), write_blocks()
// │   struct CsdInfo ::from_response(), max_freq_mhz()
// │   impl Device for SdmmcController (byte-range read_at/write_at)
// ├── pmic_glink.rs ── SMEM/GLINK transport to ADSP charger_pd
// │   probe_smem() → SmemProbe, probe_rtc() → Option<u32>
// │   struct PmicGlink — init(), open_channel(), bat_status(), property_get(), set_charge_limit()
// │   struct BatStatus — voltage_mv, capacity_pct, rate_ma, source, temp_tenths_k
// │   PROP_VOLT_NOW, PROP_CURR_NOW, PROP_CAPACITY, PROP_TEMP, PROP_CHG_CTRL
// └── usb.rs ── DWC3 USB device controller (1879 lines)
//     struct Dwc3Dev { evt_read_idx, ep0_state, bulk_out/in state }
//       ::new() → init, CSFTRST, PHY, endpoint config, Run/Stop
//       ::poll_event() → UsbEvent (Reset, ConnectDone, TransferComplete)
//       ::bulk_out_arm(), bulk_out_read() → &[u8]
//       ::bulk_in_send(data) → bool (ISP_IMI for short packets)
//       ::ep0_send(), ep0_status_in/out(), handle_setup()
//     pub fn probe() → Dwc3Info, dump_diag() → Dwc3Diag
//     pub fn phy_init(), smmu_bypass()
//
// ferros_pt/ ── Photon Transport (no_std, optional alloc)
// ├── lib.rs ── is_data_packet(), is_control_packet(), re-exports
// ├── packet.rs ── packet encode/decode
// │   TAG_SPEC='S', TAG_ACK='A', TAG_NAK='N', TAG_DONE='D', TAG_FIN='F'
// │   struct Spec, Ack, Nak, Complete { sid, fields... }
// │   encode_data(), decode_data() — per-chunk BLAKE3 hash
// ├── transfer.rs ── transfer state machines
// │   struct InboundTransfer { data, received bitmap, expected_count }
// │     ::new(), handle_data(), all_received(), finish() → COMPLETE/NAK
// │   struct OutboundTransfer { data, next_seq, count, psize }
// │     ::start(), start_vec(), next_data_packet(), encode_fin()
// │   BitmapWord = u64, outbound_bitmap_words()
// └── command.rs ── cap-addressed command protocol
//     [cap:32][op:1][params...], dev caps via BLAKE3
//     caps: DIAG, MEM, RELOAD
//     enum Op { Read, Write, Exec }
//
// ferros_ledger/ ── append-only event chain (no_std)
// ├── lib.rs ── re-exports
// ├── ewe.rs ── EWE variable-width integer encoding
// │   encode_u64(), decode_u64(), encode_lean(), decode_lean()
// │   encode_seq(), decode_seq(), seq_width()
// ├── chain.rs ── BLAKE3 hash chain
// ├── entry.rs ── ledger entry (VSF document structure)
// ├── event.rs ── typed boot/USB/SD events
// ├── category.rs ── log category tree
// └── preboot.rs ── pre-ledger ring buffer
//
// ferros_vault/ ── persistent object store (no_std)
// ├── lib.rs ── re-exports
// ├── device.rs ── Device trait, DeviceError, DeviceIoKind
// ├── hash.rs ── BLAKE3 hashing utilities
// ├── anchor.rs ── root anchor / superblock
// ├── boot.rs ── boot sequence validation
// ├── capability.rs ── capability tokens
// ├── commit.rs ── atomic commit protocol
// ├── failure.rs ── failure modes and recovery
// ├── mesh.rs ── object mesh topology
// ├── object.rs ── stored objects
// ├── platform.rs ── platform abstraction
// └── store.rs ── key-value store
//
// tools/
// ├── ferros-bridge/ ── host-side USB tool (tokio + nusb)
// │   ├── main.rs ── CLI: diag, read, reload, reboot, status
// │   │   cmd_diag(), cmd_read(), cmd_reload()
// │   │   pt_send() — blast mode, 512-byte padded OUT, COMPLETE wait
// │   │   pt_recv() — receive outbound response, no SPEC ACK
// │   └── usb.rs ── UsbLink { interface, ep_out, ep_in }
// │       ::open(), send() (512-byte pad), recv()
// └── mkimg/ ── ELF → flat binary → boot.img v3
//     main.rs ── PE/COFF header, boot.img v3 packing
//
//! Ferros kernel — bare-metal aarch64 on Fairphone 5 (QCM6490).

#![no_std]
#![no_main]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;

use ferros_hal::console::Console;
use ferros_hal::dpu;
use ferros_hal::dtb::Dtb;
use ferros_hal::pstore::{Ramoops, RamoopsConfig};
#[allow(unused_imports)] use ferros_hal::spmi;
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

// SError (asynchronous): increment count, do NOT advance ELR (fault is async),
// just eret — the exception entry consumed the pending SError.
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
// Constants
// ---------------------------------------------------------------------------

const FP5_FB_WIDTH: u32 = 1224;
const FP5_FB_HEIGHT: u32 = 2700;
const FP5_SPLASH_ADDR: u64 = 0xE100_0000;

/// PS_HOLD register — writing 0 kills power (Qualcomm TCSR).
#[allow(dead_code)] const PS_HOLD: usize = 0x0C26_4000;
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

// ---------------------------------------------------------------------------
// MirrorIO — BlockIO adapter: UFS + SD mirrored, write-verify both
// ---------------------------------------------------------------------------

use ferros_hal::hamt::BlockIO;

// ---------------------------------------------------------------------------
// Mirror mode — tracks disk health, degrades instead of halting
// ---------------------------------------------------------------------------

/// Mirror health state. Degrades on verify failure, never recovers at runtime.
/// Recovery requires physical intervention: replace disk, rebuild, reflash.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MirrorMode {
    /// Both disks healthy. Normal operation.
    Both,
    /// SD failed or absent. UFS only. Data at risk — no redundancy.
    UfsOnly,
    /// UFS failed. SD only. Reads from SD, writes to SD. Data at risk.
    SdOnly,
}

impl MirrorMode {
    fn has_ufs(self) -> bool { matches!(self, MirrorMode::Both | MirrorMode::UfsOnly) }
    fn has_sd(self) -> bool { matches!(self, MirrorMode::Both | MirrorMode::SdOnly) }

    /// Status string for display.
    fn label(self) -> &'static str {
        match self {
            MirrorMode::Both => "UFS+SD",
            MirrorMode::UfsOnly => "UFS ONLY — SD FAILED",
            MirrorMode::SdOnly => "SD ONLY — UFS FAILED",
        }
    }

    /// Warning banner for persistent on-screen display. None if healthy.
    fn warning(self) -> Option<&'static str> {
        match self {
            MirrorMode::Both => None,
            MirrorMode::UfsOnly => Some("WARNING: SD MIRROR FAILED — DATA NOT REDUNDANT"),
            MirrorMode::SdOnly => Some("WARNING: UFS PRIMARY FAILED — RUNNING FROM SD"),
        }
    }
}

/// Mirrored plow: writes to UFS and SD, verifies both, one source of truth.
/// Degrades gracefully — if one disk fails, continues on the other with a
/// persistent warning. Mode never recovers at runtime (requires physical
/// intervention + reflash).
/// Bounded mirrored block I/O. Owns a slice of the disk — writes and reads
/// outside `[region_base, region_end)` are rejected. Same principle as Rust
/// slices: the handle IS the capability.
struct MirrorIO<'a> {
    ufs: &'a ferros_hal::ufs::UfsController,
    sdc: Option<&'a mut ferros_hal::sdmmc::SdmmcController>,
    plow: u32,
    mode: MirrorMode,
    /// Owned region: first valid block (inclusive).
    region_base: u32,
    /// Owned region: last valid block (exclusive).
    region_end: u32,
    /// Count of failed operations since degradation.
    fail_count: u32,
}

impl<'a> MirrorIO<'a> {
    fn new(
        ufs: &'a ferros_hal::ufs::UfsController,
        sdc: Option<&'a mut ferros_hal::sdmmc::SdmmcController>,
    ) -> Self {
        let mode = if sdc.is_some() { MirrorMode::Both } else { MirrorMode::UfsOnly };
        MirrorIO {
            ufs,
            sdc,
            plow: ferros_layout::TRACT_BASE,
            mode,
            region_base: ferros_layout::TRACT_BASE,
            region_end: ferros_layout::TRACT_END,
            fail_count: 0,
        }
    }

    /// True if `lba` falls within this handle's owned region.
    fn in_bounds(&self, lba: u32) -> bool {
        lba >= self.region_base && lba < self.region_end
    }

    /// Convert a 4KB block LBA to an SD sector address (8 sectors per block).
    fn block_to_sector(lba: u32) -> u32 {
        lba * 8
    }

    /// Write to UFS, verify. Returns false on failure.
    fn ufs_write_verify(&self, lba: u32, data: &[u8; 4096]) -> bool {
        self.ufs.data_buffer_mut().copy_from_slice(data);
        if self.ufs.write_block(lba) != 0 { return false; }
        if self.ufs.read_block(lba) != 0 { return false; }
        self.ufs.data_buffer()[..] == data[..]
    }

    /// Read from UFS. Returns None on failure.
    fn ufs_read(&self, lba: u32) -> Option<[u8; 4096]> {
        if self.ufs.read_block(lba) != 0 { return None; }
        let mut blk = [0u8; 4096];
        blk.copy_from_slice(self.ufs.data_buffer());
        Some(blk)
    }

    /// Write to SD, verify. Returns false on failure.
    fn sd_write_verify(sdc: &mut ferros_hal::sdmmc::SdmmcController, lba: u32, data: &[u8; 4096]) -> bool {
        let sector = Self::block_to_sector(lba);
        if sdc.write_blocks(sector, data, 8).is_err() { return false; }
        let mut readback = [0u8; 4096];
        if sdc.read_blocks(sector, &mut readback, 8).is_err() { return false; }
        readback == *data
    }

    /// Read from SD. Returns None on failure.
    fn sd_read(sdc: &ferros_hal::sdmmc::SdmmcController, lba: u32) -> Option<[u8; 4096]> {
        let sector = Self::block_to_sector(lba);
        let mut blk = [0u8; 4096];
        if sdc.read_blocks(sector, &mut blk, 8).is_ok() {
            Some(blk)
        } else {
            None
        }
    }

    /// Degrade from Both to single-disk mode. Returns the new mode.
    fn degrade_ufs(&mut self) {
        if self.mode == MirrorMode::Both {
            self.mode = MirrorMode::SdOnly;
            self.fail_count = 1;
        } else {
            self.fail_count += 1;
        }
    }

    fn degrade_sd(&mut self) {
        if self.mode == MirrorMode::Both {
            self.mode = MirrorMode::UfsOnly;
            self.fail_count = 1;
        } else {
            self.fail_count += 1;
        }
    }
}

impl ferros_hal::hamt::BlockIO for MirrorIO<'_> {
    fn read_block(&self, lba: u32) -> Option<[u8; 4096]> {
        // Bounds check — reject reads outside our owned region
        if !self.in_bounds(lba) {
            return None;
        }
        match self.mode {
            MirrorMode::Both | MirrorMode::UfsOnly => {
                // Primary: UFS
                if let Some(blk) = self.ufs_read(lba) {
                    return Some(blk);
                }
                // Fallback: SD (if available)
                if let Some(ref sdc) = self.sdc {
                    return MirrorIO::sd_read(sdc, lba);
                }
                None
            }
            MirrorMode::SdOnly => {
                if let Some(ref sdc) = self.sdc {
                    MirrorIO::sd_read(sdc, lba)
                } else {
                    None
                }
            }
        }
    }

    fn write_block(&mut self, data: &[u8; 4096]) -> Option<u32> {
        let lba = self.plow;

        // Bounds check — plow must be within our owned region
        if !self.in_bounds(lba) {
            return None;
        }

        let result = match self.mode {
            MirrorMode::Both => {
                let ufs_ok = self.ufs_write_verify(lba, data);
                let sd_ok = if let Some(ref mut sdc) = self.sdc {
                    MirrorIO::sd_write_verify(sdc, lba, data)
                } else {
                    false
                };

                match (ufs_ok, sd_ok) {
                    (true, true) => Some(lba),
                    (true, false) => {
                        self.degrade_sd();
                        Some(lba)
                    }
                    (false, true) => {
                        self.degrade_ufs();
                        Some(lba)
                    }
                    (false, false) => None,
                }
            }
            MirrorMode::UfsOnly => {
                if self.ufs_write_verify(lba, data) {
                    Some(lba)
                } else {
                    None
                }
            }
            MirrorMode::SdOnly => {
                if let Some(ref mut sdc) = self.sdc {
                    if MirrorIO::sd_write_verify(sdc, lba, data) {
                        Some(lba)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        };

        // Advance plow on success, wrapping at region boundary
        if result.is_some() {
            self.plow = self.region_base
                + ((lba - self.region_base + 1) % (self.region_end - self.region_base));
        }

        result
    }
}

/// Compute BLAKE3 hash of a serialized HAMT block (with hp field zeroed).
fn hamt_block_hash(blk: &[u8; 4096]) -> [u8; 32] {
    // Find hp position and zero it for hashing
    let mut tmp = *blk;
    // hp format: 'h' 'p' '3' 0x1F [32 bytes]
    for i in 4..tmp.len().saturating_sub(36) {
        if tmp[i] == b'h' && tmp[i + 1] == b'p'
            && tmp[i + 2] == b'3' && tmp[i + 3] == 0x1F
        {
            tmp[i + 4..i + 36].fill(0);
            break;
        }
    }
    *blake3::hash(&tmp).as_bytes()
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
    let mut sdc = ferros_hal::sdmmc::SdmmcController::new(FP5_SDC2_BASE);
    let mut sd_ready = false;
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
            let csd = ferros_hal::sdmmc::CsdInfo::from_response(&probe.csd);
            log.puts("CSD v"); log.put_hex32(csd.csd_ver as u32);
            log.puts(" max="); log.put_hex32(csd.max_freq_mhz()); log.puts("MHz");
            log.puts(" cap="); log.put_hex32((csd.capacity_bytes >> 30) as u32); log.puts("GB");
            log.puts(" blk="); log.put_hex32(1u32 << csd.read_bl_len);
            log.puts(" erase="); log.put_hex32(if csd.erase_blk_en { 512 } else { (csd.sector_size as u32 + 1) * 512 });
            log.puts(" ccc="); log.put_hex32(csd.ccc as u32);
            log.puts("\n");
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

            // Multi-block test: write 8 blocks at block 256, read back, verify
            if probe.write_ok && probe.verify_ok {
                let test_start: u32 = 2;
                let test_count: u16 = 8;
                let mut write_buf = [0u8; 4096]; // 8 * 512
                // Fill with pattern: block N byte M = (N ^ M) & 0xFF
                for blk in 0..test_count as usize {
                    for byte in 0..512 {
                        write_buf[blk * 512 + byte] = ((blk ^ byte) & 0xFF) as u8;
                    }
                }
                // Test: 8 single-block writes + multi-block read back
                log.puts("8xW: ");
                let mut write_ok = true;
                for blk in 0..test_count as u32 {
                    let off = blk as usize * 512;
                    let mut blk_buf = [0u8; 512];
                    blk_buf.copy_from_slice(&write_buf[off..off + 512]);
                    if sdc.write_block(test_start + blk, &blk_buf).is_err() {
                        log.puts("F"); log.put_hex32(blk);
                        write_ok = false;
                        break;
                    }
                }
                if write_ok {
                    log.puts("OK ");
                    // Multi-block read back
                    log.puts("MR: ");
                    let mut read_buf = [0u8; 4096];
                    match sdc.read_blocks(test_start, &mut read_buf, test_count) {
                        Ok(()) => {
                            if read_buf == write_buf {
                                log.puts("OK 8blk verified\n");
                            } else {
                                // Try single-block read to isolate
                                log.puts("MBLK mismatch, trying single: ");
                                let mut single_ok = true;
                                for blk in 0..test_count as u32 {
                                    let off = blk as usize * 512;
                                    let mut blk_buf = [0u8; 512];
                                    if sdc.read_block(test_start + blk, &mut blk_buf).is_ok() {
                                        if blk_buf != write_buf[off..off + 512] {
                                            log.puts("X"); log.put_hex32(blk);
                                            single_ok = false;
                                        }
                                    } else {
                                        log.puts("F"); log.put_hex32(blk);
                                        single_ok = false;
                                    }
                                }
                                if single_ok {
                                    log.puts("OK (single read works, multi broken)\n");
                                } else {
                                    log.puts("FAIL\n");
                                }
                            }
                        }
                        Err(_) => {
                            log.puts("FAIL, single: ");
                            // Fallback to single reads
                            let mut ok_count = 0u32;
                            for blk in 0..test_count as u32 {
                                let mut blk_buf = [0u8; 512];
                                if sdc.read_block(test_start + blk, &mut blk_buf).is_ok() {
                                    let off = blk as usize * 512;
                                    if blk_buf == write_buf[off..off + 512] {
                                        ok_count += 1;
                                    }
                                }
                            }
                            log.put_hex32(ok_count); log.puts("/8 match\n");
                        }
                    }
                }
            }
        }
        sd_ready = probe.cmd7_ok && probe.bus4_ok;
    }

    // ---- UFS probe ----
    log.puts("\n-- UFS --\n");
    {
        let ufs = ferros_hal::ufs::UfsController::new(0x1D8_4000);
        let p = ufs.probe();
        log.puts("HCE="); log.put_hex32(p.hce);
        log.puts(" HCS="); log.put_hex32(p.hcs);
        log.puts(" VER="); log.put_hex32(p.ver);
        log.puts(" link="); log.puts(if p.link_up { "UP" } else { "DOWN" });
        log.puts("\n");
        if p.nop_ok {
            log.puts("NOP:    OK\n");
        } else {
            log.puts("NOP:    FAIL ocs="); log.put_hex32(p.nop_ocs as u32);
            log.puts(" rsp="); log.put_hex32(p.nop_rsp as u32);
            log.puts(" raw=[");
            for i in 0..4 { log.put_hex32(p.nop_rsp_raw[i] as u32); if i < 3 { log.puts(" "); } }
            log.puts("]\n");
        }
        // Raw descriptor dumps
        log.buf_only("GEO raw: ");
        for i in 0..p.geo_len.min(32) {
            log.buf_put_hex32(p.geo_raw[i] as u32); log.buf_only(" ");
        }
        log.buf_only("\n");
        log.buf_only("LUN0 raw: ");
        for i in 0..p.unit0_len.min(32) {
            log.buf_put_hex32(p.unit0_raw[i] as u32); log.buf_only(" ");
        }
        log.buf_only("\n");

        if p.geo_ok {
            let cap_gb = (p.total_raw_capacity_sectors * 512) >> 30;
            let blk = 1u32 << p.min_block_size_exp; // 2^exp bytes (NOT 512 * 2^exp)
            log.puts("GEO:    cap="); log.put_hex32(cap_gb as u32); log.puts("GB");
            log.puts(" seg="); log.put_hex32(p.segment_size);
            log.puts(" blk="); log.put_hex32(blk);
            log.puts(" LUNs="); log.put_hex32(p.num_lun as u32);
            log.puts("\n");
        }
        if p.unit0_ok {
            let blk = 1u32 << p.unit0_block_size_exp; // 2^exp bytes
            let cap_gb = (p.unit0_block_count * blk as u64) >> 30;
            log.puts("LUN0:   cap="); log.put_hex32(cap_gb as u32); log.puts("GB");
            log.puts(" blk="); log.put_hex32(blk);
            log.puts(" blocks="); log.put_hex32(p.unit0_block_count as u32);
            log.puts(" erase="); log.put_hex32(p.unit0_erase_block_size);
            log.puts("\n");

            // Test SCSI READ(10) — read block 0 from LUN 0
            if p.nop_ok {
                // Read block 0
                let read_ocs = ufs.read_block(0);
                log.puts("READ0:  ocs="); log.put_hex32(read_ocs as u32);
                log.puts(" sts="); log.put_hex32(ufs.last_response_status() as u32);
                if read_ocs == 0 {
                    let data = ufs.data_buffer();
                    log.puts(" [");
                    for i in 0..4 { log.put_hex32(data[i] as u32); log.puts(" "); }
                    log.puts("...]");
                }
                log.puts("\n");

                // Write-verify test: write a pattern to a safe block, read back
                // Use block 1048576 (4GB into the disk — well past Android partitions)
                let test_lba: u32 = 1 << 20;
                {
                    let buf = ufs.data_buffer_mut();
                    for i in 0..4096 {
                        buf[i] = ((i * 37 + 13) & 0xFF) as u8; // deterministic pattern
                    }
                }
                let write_ocs = ufs.write_block(test_lba);
                log.puts("WRITE:  ocs="); log.put_hex32(write_ocs as u32);
                log.puts(" sts="); log.put_hex32(ufs.last_response_status() as u32);
                log.puts(" lba="); log.put_hex32(test_lba);

                if write_ocs == 0 {
                    // Read back and verify
                    let verify_ocs = ufs.read_block(test_lba);
                    log.puts(" R:"); log.put_hex32(verify_ocs as u32);
                    if verify_ocs == 0 {
                        let data = ufs.data_buffer();
                        let mut ok = true;
                        for i in 0..4096 {
                            let expected = ((i * 37 + 13) & 0xFF) as u8;
                            if data[i] != expected {
                                log.puts(" MISMATCH@"); log.put_hex32(i as u32);
                                ok = false;
                                break;
                            }
                        }
                        if ok { log.puts(" VERIFIED"); }
                    }
                }
                log.puts("\n");
            }
        }
    }

    // ---- Hypervisor probe ----
    log.puts("\n-- HYP --\n");
    {
        let hp = ferros_hal::hyp::probe(exception_count);
        log.puts("present="); log.put_hex32(hp.present as u32);
        log.puts(" gunyah="); log.put_hex32(hp.gunyah as u32);
        log.puts(" exc="); log.put_hex32(hp.exceptions as u32);
        log.puts("\nGH x0="); log.put_hex32(hp.gh_identify_x0 as u32);
        log.puts(" x1="); log.put_hex32(hp.gh_identify_x1 as u32);
        log.puts("\nQCOM x0="); log.put_hex32(hp.qcom_x0 as u32);
        log.puts("\n");
    }

    // ---- Kernel Ring scan ----
    log.puts("\n-- KERNEL RING --\n");
    {
        let ufs = ferros_hal::ufs::UfsController::new(0x1D8_4000);
        if ufs.link_is_up() {
            ufs.init_transfer_list();
            let scan = ferros_hal::ring::scan_kernel_ring(&ufs);
            log.puts("kring: gen="); log.put_hex32(scan.generation as u32);
            log.puts(" pos="); log.put_hex32(scan.position);
            log.puts(" reads="); log.put_hex32(scan.reads);
            log.puts("\n");
            if scan.generation > 0 {
                let lba = ferros_layout::KERNEL_RING_BASE + scan.position;
                let ocs = ufs.read_block(lba);
                if ocs == 0 {
                    let data = ufs.data_buffer();
                    let mut blk = [0u8; 4096];
                    blk.copy_from_slice(&data[..4096]);
                    if let Some(ke) = ferros_hal::ring::KernelRingEntry::from_block(&blk) {
                        log.puts("  lba="); log.put_hex32(ke.kernel_lba);
                        log.puts(" size="); log.put_hex32(ke.kernel_size);
                        log.puts(" hash=");
                        for i in 0..4 { log.put_hex32(ke.kernel_hash[i] as u32); }
                        log.puts("...\n");
                    }
                }
            } else {
                log.puts("  (empty)\n");
            }
        }
    }

    // ---- Vault: spine scan → HAMT → spine commit ----
    log.screen = true;
    log.puts("\n-- VAULT --\n");
    let mirror_mode;
    {
        let ufs = ferros_hal::ufs::UfsController::resume();
        if ufs.link_is_up() {
            // 1. Scan spine for current state
            let scan = ferros_hal::ring::scan_ring(&ufs);
            let spine_entry = if scan.generation == 0 {
                log.puts("spine: genesis\n");
                ferros_hal::ring::RingEntry::genesis()
            } else {
                let e = scan.entry.as_ref().unwrap();
                log.puts("spine: gen=");
                log.put_hex32(e.generation as u32);
                log.puts(" plow=G#");
                log.put_hex32(e.plow_position);
                log.puts(" root=G#");
                log.put_hex32(e.hamt_root_lba);
                log.puts("\n");
                e.next()
            };

            // 2. Set up MirrorIO with plow resumed from spine
            let mut tio = if sd_ready {
                MirrorIO::new(&ufs, Some(&mut sdc))
            } else {
                MirrorIO::new(&ufs, None)
            };
            // Resume plow from spine. Clamp to region bounds (old entries may have 0).
            tio.plow = if tio.in_bounds(spine_entry.plow_position) {
                spine_entry.plow_position
            } else {
                tio.region_base
            };
            log.puts(tio.mode.label());
            log.puts(" plow=G#");
            log.put_hex32(tio.plow);
            log.puts("\n");

            // 3. Resume or create HAMT root
            let has_root = spine_entry.hamt_root_lba != 0
                && spine_entry.hamt_root_hash != [0u8; 32];

            let cur_root = if has_root {
                let r = ferros_hal::hamt::BlockRef {
                    hash: spine_entry.hamt_root_hash,
                    lba: spine_entry.hamt_root_lba,
                };
                // Validate: read root block and verify BLAKE3
                if let Some(blk) = tio.read_block(r.lba) {
                    let computed = hamt_block_hash(&blk);
                    if computed == r.hash {
                        log.puts("hamt: resume root=G#");
                        log.put_hex32(r.lba);
                        log.puts("\n");
                        Some(r)
                    } else {
                        log.puts("hamt: root corrupt, fresh genesis\n");
                        None
                    }
                } else {
                    log.puts("hamt: root unreadable, fresh genesis\n");
                    None
                }
            } else {
                let root_blk = ferros_hal::hamt::InternalNode::empty().to_block();
                let root_hash = hamt_block_hash(&root_blk);
                match tio.write_block(&root_blk) {
                    Some(root_lba) => {
                        log.puts("hamt: new root=G#");
                        log.put_hex32(root_lba);
                        log.puts("\n");
                        Some(ferros_hal::hamt::BlockRef {
                            hash: root_hash,
                            lba: root_lba,
                        })
                    }
                    None => {
                        log.puts("hamt: root write FAIL\n");
                        None
                    }
                }
            };

            // 4. Insert test objects (skip if no root)
            if let Some(cur_root) = cur_root {
                let mut live_root = cur_root;
                let test_keys: [u8; 4] = [0x42, 0x99, 0xDE, 0x07];
                let mut any_fail = false;
                for &k in test_keys.iter() {
                    let mut prov = [0u8; 32];
                    prov[0] = k;
                    // Only insert if not already present
                    if ferros_hal::hamt::lookup(&tio, &live_root, &prov).is_some() {
                        log.puts("  =G#");
                        log.put_hex32(k as u32);
                        log.puts("\n");
                        continue;
                    }
                    let content = [k; 64];
                    if let Some(leaf_blk) =
                        ferros_hal::hamt::lone_leaf_to_block(&prov, &content)
                    {
                        let leaf_hash = hamt_block_hash(&leaf_blk);
                        if let Some(leaf_lba) = tio.write_block(&leaf_blk) {
                            let leaf_ref = ferros_hal::hamt::BlockRef {
                                hash: leaf_hash,
                                lba: leaf_lba,
                            };
                            match ferros_hal::hamt::insert(
                                &mut tio, &live_root, leaf_ref, &prov,
                            ) {
                                Some(new_root) => {
                                    live_root = new_root;
                                    log.puts("  +G#");
                                    log.put_hex32(k as u32);
                                    log.puts(" root=G#");
                                    log.put_hex32(new_root.lba);
                                    log.puts("\n");
                                }
                                None => {
                                    log.puts("  +G#");
                                    log.put_hex32(k as u32);
                                    log.puts(" FAIL\n");
                                    any_fail = true;
                                }
                            }
                        }
                    }
                }

                // If any insert failed, the resumed tree is corrupt.
                // Discard it, create a fresh root, re-insert everything.
                if any_fail {
                    log.puts("hamt: corrupt tree, rebuild\n");
                    let root_blk = ferros_hal::hamt::InternalNode::empty().to_block();
                    let root_hash = hamt_block_hash(&root_blk);
                    if let Some(root_lba) = tio.write_block(&root_blk) {
                        live_root = ferros_hal::hamt::BlockRef {
                            hash: root_hash,
                            lba: root_lba,
                        };
                        for &k in test_keys.iter() {
                            let mut prov = [0u8; 32];
                            prov[0] = k;
                            let content = [k; 64];
                            if let Some(leaf_blk) =
                                ferros_hal::hamt::lone_leaf_to_block(&prov, &content)
                            {
                                let leaf_hash = hamt_block_hash(&leaf_blk);
                                if let Some(leaf_lba) = tio.write_block(&leaf_blk) {
                                    let leaf_ref = ferros_hal::hamt::BlockRef {
                                        hash: leaf_hash,
                                        lba: leaf_lba,
                                    };
                                    if let Some(new_root) = ferros_hal::hamt::insert(
                                        &mut tio, &live_root, leaf_ref, &prov,
                                    ) {
                                        live_root = new_root;
                                        log.puts("  +G#");
                                        log.put_hex32(k as u32);
                                        log.puts(" root=G#");
                                        log.put_hex32(new_root.lba);
                                        log.puts("\n");
                                    }
                                }
                            }
                        }
                    }
                }

                // 5. Verify lookups
                let mut found = 0u32;
                for &k in test_keys.iter() {
                    let mut prov = [0u8; 32];
                    prov[0] = k;
                    if ferros_hal::hamt::lookup(&tio, &live_root, &prov).is_some() {
                        found += 1;
                    }
                }
                log.puts("lookup ");
                log.put_hex32(found);
                log.puts("/");
                log.put_hex32(test_keys.len() as u32);

                let mut missing = [0u8; 32];
                missing[0] = 0xFF;
                if ferros_hal::hamt::lookup(&tio, &live_root, &missing).is_none() {
                    log.puts(" miss=OK");
                } else {
                    log.puts(" miss=BAD");
                }
                log.puts("\n");

                // 6. Commit spine entry with new HAMT root + plow position
                let mut commit = spine_entry;
                commit.hamt_root_hash = live_root.hash;
                commit.hamt_root_lba = live_root.lba;
                commit.plow_position = tio.plow;

                if ferros_hal::ring::write_entry(&ufs, &commit) {
                    log.puts("spine: gen=");
                    log.put_hex32(commit.generation as u32);
                    log.puts(" plow=G#");
                    log.put_hex32(commit.plow_position);
                    log.puts(" root=G#");
                    log.put_hex32(commit.hamt_root_lba);
                    log.puts(" OK\n");
                } else {
                    log.puts("spine: COMMIT FAIL\n");
                }
            }

            // Report degradation
            if tio.mode != MirrorMode::Both {
                log.puts("DEGRADED: ");
                log.puts(tio.mode.label());
                log.puts(" fails=");
                log.put_hex32(tio.fail_count);
                log.puts("\n");
            }

            mirror_mode = tio.mode;
        } else {
            log.puts("UFS link down\n");
            mirror_mode = MirrorMode::UfsOnly;
        }
    }

    // Persistent degradation warning — stays on screen forever
    if let Some(warning) = mirror_mode.warning() {
        log.puts("\n");
        log.puts(warning);
        log.puts("\n");
    }
    log.screen = false;

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

    // ---- pmic-glink (ADSP charger service) probe ----
    log.puts("\n-- pmic-glink --\n");
    {
        use ferros_hal::pmic_glink;

        // SMEM state — read-only, safe even before GLINK handshake.
        let smem = pmic_glink::probe_smem();
        log.puts("SMEM init:     "); log.put_hex32(smem.initialized);    log.puts("\n");
        log.puts("version[7]:    "); log.put_hex32(smem.version7);      log.puts("\n");
        log.puts("ptable magic:  "); log.put_hex32(smem.ptable_magic);  log.puts("\n");
        log.puts("ptable entries:"); log.put_hex32(smem.ptable_entries); log.puts("\n");
        log.puts("adsp part off: "); log.put_hex32(smem.adsp_part_off); log.puts("\n");
        log.puts("adsp part size:"); log.put_hex32(smem.adsp_part_size); log.puts("\n");
        log.puts("part magic:    "); log.put_hex32(smem.part_magic);    log.puts("\n");
        log.puts("part free unc: "); log.put_hex32(smem.part_free_off); log.puts("\n");
        log.puts("part free cac: "); log.put_hex32(smem.part_free_cac); log.puts("\n");
        log.puts("priv items:    desc="); log.put_hex32(smem.priv_desc_found as u32);
        log.puts(" tx=");             log.put_hex32(smem.priv_tx_found as u32);
        log.puts(" rx=");             log.put_hex32(smem.priv_rx_found as u32); log.puts("\n");
        log.puts("desc alloc:    "); log.put_hex32(smem.desc_alloc);    log.puts(" (global heap)\n");
        log.puts("tx tail/head:  "); log.put_hex32(smem.tx_tail);
        log.puts(" / ");              log.put_hex32(smem.tx_head);      log.puts("\n");
        log.puts("rx tail/head:  "); log.put_hex32(smem.rx_tail);
        log.puts(" / ");              log.put_hex32(smem.rx_head);      log.puts("\n");

        // Dump partition header + first two entry headers (each as u32 LE words).
        // 32-byte partition header + 16-byte entry header × 2 = 64 bytes = 16 u32s
        {
            let mut buf = [0u8; 64];
            let n = pmic_glink::dump_adsp_partition(0, &mut buf);
            if n >= 32 {
                log.buf_only("part u32s [0..40]:\n");
                let words = n / 4;
                for i in 0..words.min(16) {
                    let off = i * 4;
                    let w = (buf[off] as u32)
                        | ((buf[off+1] as u32) << 8)
                        | ((buf[off+2] as u32) << 16)
                        | ((buf[off+3] as u32) << 24);
                    log.buf_only("  ["); log.buf_put_hex32((off as u32));
                    log.buf_only("] "); log.buf_put_hex32(w); log.buf_only("\n");
                }
            }
        }

        // GLINK handshake + BATTMGR query.
        // init() will auto-allocate item 480 (APPS TX FIFO) if missing.
        // We only need SMEM initialized + items 478/479 present (ADSP side).
        let adsp_ready = smem.initialized == 1
            && (smem.priv_desc_found || smem.desc_alloc == 1)
            && (smem.priv_tx_found   || smem.tx_alloc   == 1);

        // Dump all ptable entries to see if there's a non-ADSP partition we're missing.
        {
            let mut entries = [(0u16, 0u16, 0u32, 0u32); 16];
            let n = pmic_glink::dump_ptable_entries(&mut entries);
            log.puts("ptable hosts:  ");
            for i in 0..n {
                let (h0, h1, _, _) = entries[i];
                log.buf_put_hex32(h0 as u32); log.buf_only(":"); log.buf_put_hex32(h1 as u32);
                log.buf_only(" ");
            }
            log.puts("\n");
        }
        // Pre-init ADSP raw state — read BEFORE init() modifies anything.
        // If th>0, ADSP already wrote a VERSION frame to item 479.
        {
            let (desc_a, th_a, rx_a, rx_b) = pmic_glink::probe_adsp_raw();
            log.buf_only("adsp raw:      desc="); log.buf_put_hex32(desc_a);
            log.buf_only(" th="); log.buf_put_hex32(th_a);
            log.buf_only(" rx="); log.buf_put_hex32(rx_a);
            log.buf_only(" rx[0..16]=[");
            for b in &rx_b { log.buf_put_hex32(*b as u32); log.buf_only(" "); }
            log.buf_only("]\n");
        }
        // Scan all items in APPS↔ADSP private partition.
        {
            let mut items = [(0u16, 0u32); 16];
            let n = pmic_glink::scan_adsp_items(&mut items);
            log.puts("adsp items:    ");
            for i in 0..n {
                let (id, sz) = items[i];
                log.buf_put_hex32(id as u32); log.buf_only("(");
                log.buf_put_hex32(sz); log.buf_only(") ");
            }
            log.puts("\n");
        }
        // Scan all items in APPS↔CDSP private partition (host 5).
        {
            let mut items = [(0u16, 0u32); 16];
            let n = pmic_glink::scan_cdsp_items(&mut items);
            log.puts("cdsp items:    ");
            for i in 0..n {
                let (id, sz) = items[i];
                log.buf_put_hex32(id as u32); log.buf_only("(");
                log.buf_put_hex32(sz); log.buf_only(") ");
            }
            log.puts("\n");
        }

        if adsp_ready {
            match pmic_glink::PmicGlink::init() {
                None => log.puts("glink init:    FAIL (alloc/items)\n"),
                Some(mut glink) => {
                    log.puts("glink init:    OK\n");
                    {
                        let (desc, tx, rx) = glink.addresses();
                        log.buf_only("  desc="); log.buf_put_hex32(desc as u32);
                        log.buf_only(" tx=");   log.buf_put_hex32(tx as u32);
                        log.buf_only(" rx=");   log.buf_put_hex32(rx as u32);
                        log.buf_only("\n");
                        let (tt, th, rt, rh) = glink.desc_snapshot();
                        log.buf_only("  desc: tt="); log.buf_put_hex32(tt);
                        log.buf_only(" th="); log.buf_put_hex32(th);
                        log.buf_only(" rt="); log.buf_put_hex32(rt);
                        log.buf_only(" rh="); log.buf_put_hex32(rh);
                        log.buf_only("\n");
                        // Dump first 32 bytes of tx_fifo (item 479) — ADSP may have written VERSION here
                        let mut fv = [0u8; 32];
                        glink.dump_tx(&mut fv);
                        log.buf_only("  tx[0..32]: ");
                        for b in &fv { log.buf_put_hex32(*b as u32); log.buf_only(" "); }
                        log.buf_only("\n");
                        // Dump first 32 bytes of rx_fifo (item 480, just allocated)
                        glink.dump_rx(&mut fv);
                        log.buf_only("  rx[0..32]: ");
                        for b in &fv { log.buf_put_hex32(*b as u32); log.buf_only(" "); }
                        log.buf_only("\n");
                    }
                    if glink.open_channel() {
                        log.puts("channel:       open (rcid=");
                        log.put_hex32(glink.rcid() as u32);
                        log.puts(")\n");

                        // Full battery snapshot.
                        match glink.bat_status() {
                            None => log.puts("bat status:    timeout\n"),
                            Some(bst) => {
                                log.puts("voltage:       "); log.put_hex32(bst.voltage_mv);  log.puts(" mV\n");
                                log.puts("capacity:      "); log.put_hex32(bst.capacity_pct); log.puts("%\n");
                                log.puts("rate:          "); log.put_hex32(bst.rate_ma);     log.puts(" mA\n");
                                log.puts("source:        "); log.put_hex32(bst.source);      log.puts("\n");
                                let tc = bst.temp_tenths_c();
                                log.puts("temperature:   ");
                                if tc < 0 {
                                    log.puts("-");
                                    log.put_hex32((-tc) as u32);
                                } else {
                                    log.put_hex32(tc as u32);
                                }
                                log.puts(" (tenths C)\n");
                                log.puts("charging:      ");
                                log.puts(if bst.is_charging() { "yes\n" } else { "no\n" });
                            }
                        }

                        // Individual properties for cross-check.
                        if let Some(v) = glink.property_get(pmic_glink::PROP_VOLT_NOW) {
                            log.puts("volt_now:      "); log.put_hex32(v); log.puts(" uV\n");
                        }
                        if let Some(v) = glink.property_get(pmic_glink::PROP_CURR_NOW) {
                            log.puts("curr_now:      "); log.put_hex32(v); log.puts(" uA\n");
                        }
                    } else {
                        log.puts("channel:       OPEN timeout\n");
                        // Dump post-timeout state to diagnose which FIFO ADSP used
                        let (tt, th, rt, rh) = glink.desc_snapshot();
                        log.buf_only("  post-to desc: tt="); log.buf_put_hex32(tt);
                        log.buf_only(" th="); log.buf_put_hex32(th);
                        log.buf_only(" rt="); log.buf_put_hex32(rt);
                        log.buf_only(" rh="); log.buf_put_hex32(rh);
                        log.buf_only("\n");
                        let mut fv = [0u8; 32];
                        glink.dump_tx(&mut fv);
                        log.buf_only("  post-to tx[0..32]: ");
                        for b in &fv { log.buf_put_hex32(*b as u32); log.buf_only(" "); }
                        log.buf_only("\n");
                        glink.dump_rx(&mut fv);
                        log.buf_only("  post-to rx[0..32]: ");
                        for b in &fv { log.buf_put_hex32(*b as u32); log.buf_only(" "); }
                        log.buf_only("\n");
                        // Dump all IPCC registers (incl. tentative per-source banks).
                        {
                            let mut ipcc = [(0u32, 0u32); 32];
                            let n = pmic_glink::PmicGlink::dump_ipcc(&mut ipcc);
                            log.buf_only("  IPCC regs: ");
                            for i in 0..n {
                                let (off, val) = ipcc[i];
                                if val != 0 {
                                    log.buf_only("["); log.buf_put_hex32(off);
                                    log.buf_only("]="); log.buf_put_hex32(val);
                                    log.buf_only(" ");
                                }
                            }
                            log.buf_only("(nz only)\n");
                        }
                    }
                }
            }
        } else {
            log.puts("glink:         SKIP (ADSP not ready)\n");
        }

        // SID 8 SPMI probe — PM7250B BMS/charger peripherals on FP5.
        // Reads PERPH_TYPE (0x40), PERPH_SUBTYPE (0x41), INT_LATCHED_STS (0x08).
        {
            let mut sid8 = [(0u8, 0u8, 0u8, 0u8); 16];
            let n = pmic_glink::probe_sid8_spmi(&mut sid8);
            log.puts("sid8 perph:    ");
            for i in 0..n {
                let (pid, ptype, psub, s0) = sid8[i];
                log.buf_put_hex32(pid as u32); log.buf_only(":");
                log.buf_put_hex32(ptype as u32); log.buf_only("/");
                log.buf_put_hex32(psub as u32);
                if s0 != 0xFF { log.buf_only("="); log.buf_put_hex32(s0 as u32); }
                log.buf_only(" ");
            }
            if n == 0 { log.buf_only("none"); }
            log.puts("\n");
        }
        // Deep register dumps from key SID 8 PIDs.
        // C8 = type 0x51/0x3F (FG or charger main — target for SOC/voltage)
        // CB = type 0x0B/0x01 (BAT_IF — offset 7 = 0x21 = 33, possible SOC%)
        // CA = type 0x0A/0x01 (VADC/ADC — target for voltage/temp measurements)
        // Read 32 bytes at 0x00 and 16 bytes at 0x10 for each.
        for pid_u8 in [0xC8u8, 0xCBu8, 0xCAu8] {
            let mut regs = [0xFFu8; 32];
            let n = pmic_glink::read_sid8_regs(pid_u8, 0x00, &mut regs);
            log.buf_only("  C"); log.buf_put_hex32(pid_u8 as u32 & 0xF);
            log.buf_only("[00]: ");
            for i in 0..n { log.buf_put_hex32(regs[i] as u32); log.buf_only(" "); }
            log.buf_only("\n");
            // Also read 0x10-0x1F region (STATUS/control/data on many charger peripherals).
            let mut regs2 = [0xFFu8; 16];
            let n2 = pmic_glink::read_sid8_regs(pid_u8, 0x10, &mut regs2);
            log.buf_only("  C"); log.buf_put_hex32(pid_u8 as u32 & 0xF);
            log.buf_only("[10]: ");
            for i in 0..n2 { log.buf_put_hex32(regs2[i] as u32); log.buf_only(" "); }
            log.buf_only("\n");
        }

        // RTC — direct SPMI probe, independent of GLINK.
        log.puts("RTC probe:     ");
        match pmic_glink::probe_rtc() {
            Some(t) => { log.put_hex32(t); log.puts(" (Unix seconds)\n"); }
            None    => log.puts("DENIED (SPMI arbiter)\n"),
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

                let mut _evt_count = 0u32;
                let mut poll_count = 0u32;
                // PT inbound state — allocated on SPEC arrival, freed on COMPLETE
                #[allow(unused_assignments)] let mut pt_data_buf: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                #[allow(unused_assignments)] let mut pt_bitmap_buf: alloc::vec::Vec<ferros_pt::BitmapWord> = alloc::vec::Vec::new();
                let mut pt_inbound: Option<InboundTransfer<'_>> = None;
                // Current seq_width for DATA parsing (0 = no active transfer)
                let mut pt_seq_width: usize = 0;
                // Pending COMPLETE packet to send after last ACK flushes
                let pt_complete_pending = [0u8; 128];
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
                let cap_beam = ferros_pt::command::dev_cap(ferros_pt::command::caps::BEAM);
                // Staging area for hot-reload kernel image
                const RELOAD_STAGE: usize = 0xA000_0000;
                let mut reload_size: usize = 0;
                // Flag: outbound DATA pump needs to send next packet
                let mut _pt_out_pump = false;

                loop {
                    match usb.poll_event() {
                        ferros_hal::usb::UsbEvent::None => {
                            // Yield CPU briefly when no USB events pending.
                            // GIC + WFI attempted but GICD access may be TZ-protected.
                            // TODO: probe GIC safely, fall back to spin if protected.
                            for _ in 0..64u32 { core::hint::spin_loop(); }
                        }
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
                            _evt_count += 1;
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
                            _evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::Disconnect => {
                            log.puts("  disconnected\n");
                            ledger.post(&Event::UsbDisconnect);
                            _evt_count += 1;
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
                            _evt_count += 1;
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
                                                        let _ok = usb.bulk_in_send(&complete_buf[..clen]);
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
                                                                pt_out_data.clear();
                                                                // Prepend degradation warning if applicable
                                                                if let Some(w) = mirror_mode.warning() {
                                                                    pt_out_data.extend_from_slice(w.as_bytes());
                                                                    pt_out_data.extend_from_slice(b"\n\n");
                                                                }
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
                                                                // RELOAD Exec: persist to UFS, update kernel ring, jump
                                                                log.buf_only("RELOAD exec size=");
                                                                log.buf_put_hex32(reload_size as u32);
                                                                log.buf_only("\n");
                                                                if reload_size > 0x1000 {
                                                                    let magic = unsafe { *(RELOAD_STAGE as *const u32) };
                                                                    if magic == 0x91005A4D { // MZ header
                                                                        // Persist kernel to UFS + update kernel ring
                                                                        let ufs = ferros_hal::ufs::UfsController::resume();
                                                                        if ufs.link_is_up() {
                                                                            let kernel_slice = unsafe {
                                                                                core::slice::from_raw_parts(
                                                                                    RELOAD_STAGE as *const u8,
                                                                                    reload_size,
                                                                                )
                                                                            };

                                                                            // Write kernel binary to HAMT region
                                                                            let kernel_lba = ferros_layout::HAMT_BASE;
                                                                            let blocks = (reload_size + 4095) / 4096;
                                                                            let mut write_ok = true;
                                                                            for i in 0..blocks {
                                                                                let buf = ufs.data_buffer_mut();
                                                                                let offset = i * 4096;
                                                                                let copy_len = core::cmp::min(4096, reload_size - offset);
                                                                                buf[..copy_len].copy_from_slice(&kernel_slice[offset..offset + copy_len]);
                                                                                // Zero padding
                                                                                for b in copy_len..4096 { buf[b] = 0; }
                                                                                if ufs.write_block(kernel_lba + i as u32) != 0 {
                                                                                    write_ok = false;
                                                                                    break;
                                                                                }
                                                                            }

                                                                            if write_ok {
                                                                                // Compute BLAKE3 of kernel
                                                                                let kernel_hash = blake3::hash(kernel_slice);

                                                                                // Scan kernel ring for current generation
                                                                                let scan = ferros_hal::ring::scan_kernel_ring(&ufs);
                                                                                let new_gen = scan.generation + 1;

                                                                                let entry = ferros_hal::ring::KernelRingEntry {
                                                                                    generation: new_gen,
                                                                                    kernel_lba,
                                                                                    kernel_size: reload_size as u32,
                                                                                    kernel_hash: *kernel_hash.as_bytes(),
                                                                                    kernel_sig: [0u8; 64], // unsigned dev build
                                                                                    eagle_time: ferros_hal::vsf_mini::read_qtimer(),
                                                                                    hp_hash: [0u8; 32], // computed in to_block()
                                                                                };

                                                                                if ferros_hal::ring::write_kernel_entry(&ufs, &entry) {
                                                                                    log.buf_only("KRING gen=");
                                                                                    log.buf_put_hex32(new_gen as u32);
                                                                                    log.buf_only(" lba=");
                                                                                    log.buf_put_hex32(kernel_lba);
                                                                                    log.buf_only(" OK\n");
                                                                                } else {
                                                                                    log.buf_only("KRING WRITE FAIL\n");
                                                                                }
                                                                            } else {
                                                                                log.buf_only("KERNEL UFS WRITE FAIL\n");
                                                                            }
                                                                        }

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
                                                            } else if cmd.cap == cap_beam && cmd.op == ferros_pt::Op::Read {
                                                                // BEAM Read: read ring entries
                                                                // Params: [ring_id:1][mode:1][offset:4 BE][count:4 BE]
                                                                if cmd.params.len() >= 10 {
                                                                    let ring_id = cmd.params[0];
                                                                    let mode = cmd.params[1];
                                                                    let offset = u32::from_be_bytes([
                                                                        cmd.params[2], cmd.params[3],
                                                                        cmd.params[4], cmd.params[5],
                                                                    ]);
                                                                    let count = u32::from_be_bytes([
                                                                        cmd.params[6], cmd.params[7],
                                                                        cmd.params[8], cmd.params[9],
                                                                    ]).max(1).min(256); // clamp 1..256

                                                                    let ufs = ferros_hal::ufs::UfsController::resume();
                                                                    if ufs.link_is_up() {
                                                                        // Determine ring base, size, and resolve latest
                                                                        let (base, ring_size) = match ring_id {
                                                                            0 => (ferros_layout::KERNEL_RING_BASE, ferros_layout::KERNEL_RING_SIZE),
                                                                            1 => (ferros_layout::VAULT_ROOT_RING_BASE, ferros_layout::VAULT_ROOT_RING_SIZE),
                                                                            2 => (ferros_layout::LEDGER_RING_BASE, ferros_layout::LEDGER_RING_SIZE),
                                                                            3 => (ferros_layout::STATE_RING_BASE, ferros_layout::STATE_RING_SIZE),
                                                                            _ => (0, 0),
                                                                        };

                                                                        if ring_size > 0 {
                                                                            // Find latest generation via scan
                                                                            let scan = match ring_id {
                                                                                0 => ferros_hal::ring::scan_kernel_ring(&ufs),
                                                                                _ => ferros_hal::ring::scan_ring(&ufs),
                                                                            };

                                                                            let start_pos = match mode {
                                                                                0 => {
                                                                                    // Latest minus offset
                                                                                    if scan.generation == 0 { 0 }
                                                                                    else {
                                                                                        let target_gen = scan.generation.saturating_sub(offset as u64);
                                                                                        if target_gen == 0 { 0 }
                                                                                        else { ((target_gen - 1) % ring_size as u64) as u32 }
                                                                                    }
                                                                                }
                                                                                1 => {
                                                                                    // Absolute generation
                                                                                    if offset == 0 { 0 }
                                                                                    else { ((offset as u64 - 1) % ring_size as u64) as u32 }
                                                                                }
                                                                                _ => 0,
                                                                            };

                                                                            // Read blocks into response
                                                                            pt_out_data.clear();
                                                                            for i in 0..count {
                                                                                let pos = (start_pos + ring_size - i) % ring_size;
                                                                                let lba = base + pos;
                                                                                let ocs = ufs.read_block(lba);
                                                                                if ocs == 0 {
                                                                                    let data = ufs.data_buffer();
                                                                                    pt_out_data.extend_from_slice(&data[..4096]);
                                                                                } else {
                                                                                    // Pad with zeros for failed reads
                                                                                    pt_out_data.resize(pt_out_data.len() + 4096, 0);
                                                                                }
                                                                            }

                                                                            log.buf_only("BEAM ring=");
                                                                            log.buf_put_hex32(ring_id as u32);
                                                                            log.buf_only(" gen=");
                                                                            log.buf_put_hex32(scan.generation as u32);
                                                                            log.buf_only(" pos=");
                                                                            log.buf_put_hex32(start_pos);
                                                                            log.buf_only(" n=");
                                                                            log.buf_put_hex32(count);
                                                                            log.buf_only(" bytes=");
                                                                            log.buf_put_hex32(pt_out_data.len() as u32);
                                                                            log.buf_only("\n");
                                                                        }
                                                                    }
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
                                    // Outbound SPEC — only clear if send succeeds
                                    if usb.bulk_in_send(&pt_out_spec_pending[..pt_out_spec_len]) {
                                        pt_out_spec_len = 0;
                                    }
                                    // else: keep pt_out_spec_len set, idle pump retries
                                }
                                // Outbound DATA is handled by the main loop idle check
                                ledger.post(&Event::UsbBulkTxComplete);
                            }
                            _evt_count += 1;
                        }
                        ferros_hal::usb::UsbEvent::TransferNotReady { .. } => {
                            _evt_count += 1;
                        }
                    }

                    // (periodic re-arm removed — interferes with active transfers)

                    // Outbound pump — SPEC first (retry if E3 send failed), then DATA.
                    if usb.bulk_in_idle {
                        if pt_out_spec_len > 0 {
                            // SPEC retry — E3 handler may have failed due to dirty endpoint
                            if usb.bulk_in_send(&pt_out_spec_pending[..pt_out_spec_len]) {
                                pt_out_spec_len = 0;
                            }
                        } else if let Some(ref mut out) = pt_outbound {
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
