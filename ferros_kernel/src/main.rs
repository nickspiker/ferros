// FERROS SOURCE MAP — keep updated when pub items or files change
//
// ferros_kernel/ └── main.rs ── kernel entry, boot sequence, USB event loop, SD probe _start (asm), kernel_main() Boot: EL check, DTB parse, FB console, UART, pstore, DPU, SPMI GCC clocks, RPMh power, SDHCI probe, USB DWC3 init USB event loop: PT command dispatch, hot-reload handler SD probe: CMD0-CMD8-ACMD41-CMD2-CMD3-CMD9-CMD7-CMD16, R/W verify
//
// ferros_hal/ ── hardware abstraction (no_std, QCM6490/FP5) ├── lib.rs ── module re-exports ├── mmio.rs ── raw MMIO read/write (8/16/32-bit), cache ops ├── console.rs ── framebuffer text console, 8x16 VGA font, 2x scaling │   struct Console { fb_base, stride, x_off, y_off, col, row } │     ::new(), put_char(), scroll(), clear() ├── fb.rs ── raw framebuffer pixel ops ├── dtb.rs ── FDT/DTB parser (find nodes, read properties) ├── dpu.rs ── display processing unit register reads ├── uart.rs ── GENI UART TX (QUP1 SE5 at 0x994000) ├── pstore.rs ── ramoops/persistent_ram_buffer writer ├── spmi.rs ── SPMI arbiter v5 (observer reads, channel writes) │   find_apid(), read_byte(), write_byte(), ppid() ├── gcc.rs ── GCC clock controller (SDC2 branches + RCG) │   sdc2_set_400khz(), sdc2_set_25mhz(), sdc2_block_reset() ├── rpmh.rs ── RPMh TCS for LDO power enable via cmd-db │   enable_ldo(), cmd_db_lookup() ├── sdmmc.rs ── SD/MMC controller (SDHCI + Qualcomm vendor regs) │   struct SdmmcController { base, initialized, card_id, capacity, rca } │     ::new(), init(), probe_card() → ProbeResult │     ::read_block(), write_block(), read_blocks(), write_blocks() │   struct CsdInfo ::from_response(), max_freq_mhz() │   impl Device for SdmmcController (byte-range read_at/write_at) ├── pmic_glink.rs ── SMEM/GLINK transport to ADSP charger_pd │   probe_smem() → SmemProbe, probe_rtc() → Option<u32> │   struct PmicGlink — init(), open_channel(), bat_status(), property_get(), set_charge_limit() │   struct BatStatus — voltage_mv, capacity_pct, rate_ma, source, temp_tenths_k │   PROP_VOLT_NOW, PROP_CURR_NOW, PROP_CAPACITY, PROP_TEMP, PROP_CHG_CTRL └── usb.rs ── DWC3 USB device controller (1879 lines) struct Dwc3Dev { evt_read_idx, ep0_state, bulk_out/in state } ::new() → init, CSFTRST, PHY, endpoint config, Run/Stop ::poll_event() → UsbEvent (Reset, ConnectDone, TransferComplete) ::bulk_out_arm(), bulk_out_read() → &[u8] ::bulk_in_send(data) → bool (ISP_IMI for short packets) ::ep0_send(), ep0_status_in/out(), handle_setup() pub fn probe() → Dwc3Info, dump_diag() → Dwc3Diag pub fn phy_init(), smmu_bypass()
//
// ferros_pt/ ── Photon Transport (no_std, optional alloc) ├── lib.rs ── is_data_packet(), is_control_packet(), re-exports ├── packet.rs ── packet encode/decode │   TAG_SPEC='S', TAG_ACK='A', TAG_NAK='N', TAG_DONE='D', TAG_FIN='F' │   struct Spec, Ack, Nak, Complete { sid, fields... } │   encode_data(), decode_data() — per-chunk BLAKE3 hash ├── transfer.rs ── transfer state machines │   struct InboundTransfer { data, received bitmap, expected_count } │     ::new(), handle_data(), all_received(), finish() → COMPLETE/NAK │   struct OutboundTransfer { data, next_seq, count, psize } │     ::start(), start_vec(), next_data_packet(), encode_fin() │   BitmapWord = u64, outbound_bitmap_words() └── command.rs ── cap-addressed command protocol [cap:32][op:1][params...], dev caps via BLAKE3 caps: DIAG, MEM, RELOAD enum Op { Read, Write, Exec }
//
// ferros_ledger/ ── append-only event chain (no_std) ├── lib.rs ── re-exports ├── ewe.rs ── EWE variable-width integer encoding │   encode_u64(), decode_u64(), encode_lean(), decode_lean() │   encode_seq(), decode_seq(), seq_width() ├── chain.rs ── BLAKE3 hash chain ├── entry.rs ── ledger entry (VSF document structure) ├── event.rs ── typed boot/USB/SD events ├── category.rs ── log category tree └── preboot.rs ── pre-ledger ring buffer
//
// ferros_vault/ ── persistent object store (no_std) ├── lib.rs ── re-exports ├── device.rs ── Device trait, DeviceError, DeviceIoKind ├── hash.rs ── BLAKE3 hashing utilities ├── anchor.rs ── root anchor / superblock ├── boot.rs ── boot sequence validation ├── capability.rs ── capability tokens ├── commit.rs ── atomic commit protocol ├── failure.rs ── failure modes and recovery ├── mesh.rs ── object mesh topology ├── object.rs ── stored objects ├── platform.rs ── platform abstraction └── store.rs ── key-value store
//
// tools/ ├── ferros-bridge/ ── host-side USB tool (tokio + nusb) │   ├── main.rs ── CLI: diag, read, reload, reboot, status │   │   cmd_diag(), cmd_read(), cmd_reload() │   │   pt_send() — blast mode, 512-byte padded OUT, COMPLETE wait │   │   pt_recv() — receive outbound response, no SPEC ACK │   └── usb.rs ── UsbLink { interface, ep_out, ep_in } │       ::open(), send() (512-byte pad), recv() └── mkimg/ ── ELF → flat binary → boot.img v3 main.rs ── PE/COFF header, boot.img v3 packing
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

/// Mirror health state. Degrades on verify failure, never recovers at runtime. Recovery requires physical intervention: replace disk, rebuild, reflash.
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

/// Mirrored plow: writes to UFS and SD, verifies both, one source of truth. Degrades gracefully — if one disk fails, continues on the other with a persistent warning. Mode never recovers at runtime (requires physical intervention + reflash). Bounded mirrored block I/O. Owns a slice of the disk — writes and reads outside `[region_base, region_end)` are rejected. Same principle as Rust slices: the handle IS the capability.
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

    // ---- Disable the cluster watchdogs FIRST ---- ABL arms the Exynos CLUSTER0/1 NONCPU watchdogs so a hung kernel reboots (~60s, reset reason G#CBEA "APC Watchdog Early", confirmed 2026-08-14). Until we run a real timer + pet loop, stop them: Samsung s3c2410-style block, WTCON at base+0. Clearing WTCON.EN (bit 5) halts the counter and WTCON.RSTEN (bit 0) masks reset — writing 0 does both, so no reset is ever requested regardless of the PMU reset mask. Nodes from live DTB: watchdog_cl0@G#10060000, watchdog_cl1@G#10070000.
    const WATCHDOGS: [usize; 2] = [0x1006_0000, 0x1007_0000];
    for &wdt in &WATCHDOGS {
        unsafe {
            core::ptr::write_volatile((wdt + 0x00) as *mut u32, 0); // WTCON = 0: timer off, reset masked
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

    // ---- UFS command-path forensics (READ ONLY) ----
    // Our UFS commands have never completed on husky (UFS.md): boot write left the doorbell stuck, IS clean, no error. Capture the full standard-region + S2MPU state around ONE read_block(1) (GPT header — read-only, safe) so the failure mode is unambiguous instead of guessed. All reads here are known-accessible (UFS standard region, and the S2MPU we already write) — NOT the VS region (G#1320_1100) or SysMMU (G#131C_0000), which may hang the AP; those are a separate gated experiment only if this comes back clean.
    // ufs_diag layout, surfaced via DIAG "UFS_*" lines:
    //   0 link_up  1 read_ocs  2 IS  3 HCS  4 UTRLDBR  5 UTRLBA
    //   6 UECPA 7 UECDL 8 UECN 9 UECT 10 UECDME
    //   11 S2MPU_HSI2_CTRL0 (expect 0 = disabled)  12 S2MPU_HSI2+0x54
    //   13 databuf[0..4] ("EFI ")  14 databuf[4..8] ("PART")
    //   15 resp_upiu[0..4]  16 resp_upiu[4..8]  17 last_ocs
    let ufs = ferros_hal::ufs::UfsController::new(UFS_BASE);
    let ufs_up = ufs.link_is_up();
    let mut ufs_diag = [0u32; 24];
    ufs_diag[0] = ufs_up as u32;
    if ufs_up {
        let r = |off: usize| unsafe { core::ptr::read_volatile((UFS_BASE + off) as *const u32) };
        ufs.init_transfer_list();
        // Capture run-stop + auto-hibernate-timer BEFORE the command (post-init_transfer_list state).
        ufs_diag[18] = r(0x60); // UTRLRSR — is the transfer list actually running?
        ufs_diag[19] = r(0x18); // AHIT — auto-hibernate idle timer (nonzero = AH8 on; would gate the VS clock and explain the VS-region hang)
        ufs_diag[20] = r(0x00); // CAP
        ufs_diag[21] = r(0x54); // UTRLBAU
        let ocs = ufs.read_block(1);
        let le = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]); // big-endian display: bytes read left-to-right
        ufs_diag[1] = ocs as u32;
        ufs_diag[2] = r(0x20); // IS
        ufs_diag[3] = r(0x30); // HCS
        ufs_diag[4] = r(0x58); // UTRLDBR
        ufs_diag[5] = r(0x50); // UTRLBA
        ufs_diag[6] = r(0x38); // UECPA
        ufs_diag[7] = r(0x3C); // UECDL
        ufs_diag[8] = r(0x40); // UECN
        ufs_diag[9] = r(0x44); // UECT
        ufs_diag[10] = r(0x48); // UECDME
        ufs_diag[11] = unsafe { core::ptr::read_volatile((0x131F_0000 + 0x00) as *const u32) };
        ufs_diag[12] = unsafe { core::ptr::read_volatile((0x131F_0000 + 0x54) as *const u32) };
        let data = ufs.data_buffer();
        ufs_diag[13] = le(&data[0..4]);
        ufs_diag[14] = le(&data[4..8]);
        let rsp = ufs.response_upiu_head();
        ufs_diag[15] = le(&rsp[0..4]);
        ufs_diag[16] = le(&rsp[4..8]);
        ufs_diag[17] = ufs.last_ocs() as u32;
        // CMU_HSI2 UFS Q-channel gate (read only): QCH_CON_UFS_EMBD @ CMU_HSI2(G#1300_0000)+G#30C4. Bits [0]=ENABLE(HWACG on) [1]=CLOCK_REQ [2]=IGNORE_FORCE_PM. If ENABLE=1 and the clock isn't forced, auto-gating stops the vendor-region APB clock → explains why reg_hci (G#1320_1100) hangs. This confirms/refutes the clock-gate root cause; NO write. CMU is core infra behind the HSI2 S2MPU we already disabled, so the read is low-risk.
        ufs_diag[22] = unsafe { core::ptr::read_volatile((0x1300_30C4) as *const u32) };
        ufs_diag[23] = unsafe { core::ptr::read_volatile((0x1300_30C8) as *const u32) }; // QCH_CON_UFS_EMBD_FMP
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
                                                // UFS command-path forensics (read_block(1) at boot). See the ufs_diag layout comment.
                                                append_hex(&mut resp, b"UFS_LINK=", ufs_diag[0]);
                                                append_hex(&mut resp, b"UFS_READ_OCS=", ufs_diag[1]);
                                                append_hex(&mut resp, b"UFS_IS=", ufs_diag[2]);
                                                append_hex(&mut resp, b"UFS_HCS=", ufs_diag[3]);
                                                append_hex(&mut resp, b"UFS_DBR=", ufs_diag[4]);
                                                append_hex(&mut resp, b"UFS_UTRLBA=", ufs_diag[5]);
                                                append_hex(&mut resp, b"UFS_UECPA=", ufs_diag[6]);
                                                append_hex(&mut resp, b"UFS_UECDL=", ufs_diag[7]);
                                                append_hex(&mut resp, b"UFS_UECN=", ufs_diag[8]);
                                                append_hex(&mut resp, b"UFS_UECT=", ufs_diag[9]);
                                                append_hex(&mut resp, b"UFS_UECDME=", ufs_diag[10]);
                                                append_hex(&mut resp, b"UFS_S2MPU_CTRL0=", ufs_diag[11]);
                                                append_hex(&mut resp, b"UFS_S2MPU_54=", ufs_diag[12]);
                                                append_hex(&mut resp, b"UFS_DATA0=", ufs_diag[13]);
                                                append_hex(&mut resp, b"UFS_DATA1=", ufs_diag[14]);
                                                append_hex(&mut resp, b"UFS_RSP0=", ufs_diag[15]);
                                                append_hex(&mut resp, b"UFS_RSP1=", ufs_diag[16]);
                                                append_hex(&mut resp, b"UFS_LASTOCS=", ufs_diag[17]);
                                                append_hex(&mut resp, b"UFS_RSR=", ufs_diag[18]);
                                                append_hex(&mut resp, b"UFS_AHIT=", ufs_diag[19]);
                                                append_hex(&mut resp, b"UFS_CAP=", ufs_diag[20]);
                                                append_hex(&mut resp, b"UFS_UTRLBAU=", ufs_diag[21]);
                                                append_hex(&mut resp, b"UFS_QCH=", ufs_diag[22]);
                                                append_hex(&mut resp, b"UFS_QCH_FMP=", ufs_diag[23]);
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

    /* DISABLED: original FP5 boot sequence — needs Pixel 8 display + USB rework.

    // ---- Parse DTB early to find ramoops (fallback to known FP5 address) ----
    let ramoops = if dtb_addr != 0 {
        unsafe { Dtb::from_ptr(dtb_addr as *const u8) }
            .and_then(|dtb| parse_ramoops_config(&dtb))
            .map(|cfg| unsafe { Ramoops::from_config(&cfg) })
    } else {
        None
    };
    // Fallback: if DTB lookup failed, use known FP5 ramoops region directly. ramoops@0xBA800000, 2MB, default zone sizes (256KB each).
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

    // ---- Seed timing (if seed left a timing record) ----
    {
        const SEED_TIMING_ADDR: usize = 0x81FF_FFE0;
        let p = SEED_TIMING_ADDR as *const u64;
        let magic = unsafe { core::ptr::read_volatile(p.add(2)) };
        if magic == 0x454D_4954_4445_4553 { // "SEEDTIME"
            let t_start = unsafe { core::ptr::read_volatile(p) };
            let t_end = unsafe { core::ptr::read_volatile(p.add(1)) };
            let ticks = t_end.saturating_sub(t_start);
            // QCM6490 QTimer = 19.2 MHz → 1 tick = 52.08ns microseconds = ticks * 1000 / 19200
            let us = ticks * 1000 / 19200;
            let ms = us / 1000;
            let us_frac = us % 1000;
            log.screen = true;
            log.puts("seed: ");
            // Decimal ms output
            let mut dec = [b'0'; 8];
            let mut v = us;
            let mut i = 7;
            loop {
                dec[i] = b'0' + (v % 10) as u8;
                v /= 10;
                if v == 0 || i == 0 { break; }
                i -= 1;
            }
            // Insert decimal point: output digits with . before last 3
            let digits = &dec[i..8];
            if digits.len() <= 3 {
                log.puts("0.");
                for _ in 0..(3 - digits.len()) { log.putc(b'0'); }
                for &d in digits { log.putc(d); }
            } else {
                let split = digits.len() - 3;
                for &d in &digits[..split] { log.putc(d); }
                log.puts(".");
                for &d in &digits[split..] { log.putc(d); }
            }
            log.puts(" ms\n");
            log.screen = false;
            log.puts("seed timing: start=");
            log.put_hex(t_start);
            log.puts(" end=");
            log.put_hex(t_end);
            log.puts(" ticks=");
            log.put_hex(ticks);
            log.puts(" = ");
            log.put_hex(us);
            log.puts(" us\n");
        }
    }

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

    // ---- SD card power via RPMh mailbox ---- SPMI arbiter blocks direct LDO access (EE ownership). Use RPMh TCS to request LDO enable thru the proper power management channel.
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
        //    All three pads share one register at TLMM + 0xB4000 (SC7280 pinctrl) Bit layout: DATA drv [2:0], CMD drv [5:3], CLK drv [8:6], DATA pull [10:9], CMD pull [12:11], CLK pull [15:14] Drive: (mA/2)-1. Pull: 0=none, 1=down, 2=keeper, 3=up
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
        //    TLMM GPIO regs: base + gpio*0x1000, +0x00=CFG, +0x04=IN_OUT CFG: [3:2]=func(0=gpio), [1:0]=pull(3=up), [8:6]=drv
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

                // Write-verify test: write a pattern to a safe block, read back Use block 1048576 (4GB into the disk — well past Android partitions)
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

                // If any insert failed, the resumed tree is corrupt. Discard it, create a fresh root, re-insert everything.
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

        // Dump partition header + first two entry headers (each as u32 LE words). 32-byte partition header + 16-byte entry header × 2 = 64 bytes = 16 u32s
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

        // SMP2P readiness signaling probe (before GLINK init)
        {
            let smp = pmic_glink::probe_smp2p();
            log.puts("smp2p out 443: ");
            if smp.outbound_found {
                log.puts("found magic="); log.put_hex32(smp.outbound_magic);
                log.puts(" valid="); log.put_hex32(smp.outbound_valid as u32);
                log.puts(" flags="); log.put_hex32(smp.outbound_flags);
                log.puts("\n");
                if smp.master_kernel_idx != 0xFF {
                    log.puts("  master-kernel: idx="); log.put_hex32(smp.master_kernel_idx as u32);
                    log.puts(" val="); log.put_hex32(smp.master_kernel_val);
                    log.puts(if smp.master_kernel_val & 1 != 0 { " STOP SET\n" } else { " stop clear\n" });
                } else {
                    log.puts("  master-kernel: NOT FOUND\n");
                }
            } else {
                log.puts("NOT FOUND\n");
            }

            log.puts("smp2p in  429: ");
            if smp.inbound_found {
                log.puts("found magic="); log.put_hex32(smp.inbound_magic);
                log.puts(" valid="); log.put_hex32(smp.inbound_valid as u32);
                log.puts("\n");
                if smp.slave_kernel_idx != 0xFF {
                    log.puts("  slave-kernel:  idx="); log.put_hex32(smp.slave_kernel_idx as u32);
                    log.puts(" val="); log.put_hex32(smp.slave_kernel_val);
                    let ready = smp.slave_kernel_val & 2 != 0;
                    log.puts(if ready { " READY\n" } else { " not ready\n" });
                } else {
                    log.puts("  slave-kernel:  NOT FOUND\n");
                }
            } else {
                log.puts("NOT FOUND\n");
            }
        }

        // IPCC rev + config (safe offsets only — 0x10+ are TZ-protected)
        {
            let rev = unsafe { ferros_hal::mmio::read32(0x0040_8000) };
            let cfg = unsafe { ferros_hal::mmio::read32(0x0040_8004) };
            log.buf_only("IPCC:          rev="); log.buf_put_hex32(rev);
            log.buf_only(" cfg="); log.buf_put_hex32(cfg);
            log.buf_only("\n");
        }

        // GLINK handshake + BATTMGR query. init() will auto-allocate item 480 (APPS TX FIFO) if missing. We only need SMEM initialized + items 478/479 present (ADSP side).
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
        // Pre-init ADSP raw state — read BEFORE init() modifies anything. If th>0, ADSP already wrote a VERSION frame to item 479.
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
        // Scan APSS(7)↔ADSP(2) partition — SMP2P items may live here.
        {
            let mut items = [(0u16, 0u32); 16];
            let n = pmic_glink::scan_host_items(7, 2, &mut items);
            log.puts("apss↔adsp(7:2):");
            if n == 0 { log.puts(" (empty)"); }
            for i in 0..n {
                let (id, sz) = items[i];
                log.buf_only(" "); log.buf_put_hex32(id as u32);
                log.buf_only("("); log.buf_put_hex32(sz); log.buf_only(")");
            }
            log.puts("\n");
        }
        // Also scan APSS(7)↔Modem(1) for reference.
        {
            let mut items = [(0u16, 0u32); 16];
            let n = pmic_glink::scan_host_items(7, 1, &mut items);
            log.puts("apss↔mdm (7:1):");
            if n == 0 { log.puts(" (empty)"); }
            for i in 0..n {
                let (id, sz) = items[i];
                log.buf_only(" "); log.buf_put_hex32(id as u32);
                log.buf_only("("); log.buf_put_hex32(sz); log.buf_only(")");
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
                        // Dump raw descriptor (32 bytes) after timeout
                        {
                            log.buf_only("  desc raw: ");
                            for off in (0..32).step_by(4) {
                                let v = unsafe { ferros_hal::mmio::read32(glink.desc_addr() + off) };
                                log.buf_put_hex32(v); log.buf_only(" ");
                            }
                            log.buf_only("\n");
                        }
                        // Post-GLINK SMP2P re-probe — did our allocation work?
                        let smp = pmic_glink::probe_smp2p();
                        log.puts("  smp2p post:  out=");
                        log.put_hex32(smp.outbound_found as u32);
                        if smp.outbound_found {
                            log.puts(" magic="); log.put_hex32(smp.outbound_magic);
                            log.puts(" mk="); log.put_hex32(smp.master_kernel_val);
                        }
                        log.puts(" in="); log.put_hex32(smp.inbound_found as u32);
                        if smp.inbound_found {
                            log.puts(" magic="); log.put_hex32(smp.inbound_magic);
                            log.puts(" sk="); log.put_hex32(smp.slave_kernel_val);
                        }
                        log.puts("\n");
                    }
                }
            }
        } else {
            log.puts("glink:         SKIP (ADSP not ready)\n");
        }

        // Battery state from QG fuel gauge + SCHG charger (direct SPMI).
        {
            let bat = pmic_glink::probe_battery_qg();
            log.puts("battery:       ");
            log.put_hex32(bat.vbat_mv); log.puts(" mV (raw=");
            log.put_hex32(bat.vbat_raw as u32); log.puts(")\n");
            log.puts("  sdam valid:  "); log.put_hex32(bat.sdam_valid as u32); log.puts("\n");
            log.buf_only("  sdam[40-49]: ");
            for b in &bat.sdam_data { log.buf_put_hex32(*b as u32); log.buf_only(" "); }
            log.buf_only("\n");
            log.puts("  usb_rt_sts:  "); log.put_hex32(bat.usb_rt_sts as u32);
            log.puts("  chgr_rt_sts: "); log.put_hex32(bat.chgr_rt_sts as u32);
            log.puts("  misc_sts:    "); log.put_hex32(bat.misc_sts as u32); log.puts("\n");
        }

        // SID 8 SPMI probe — PM7250B BMS/charger peripherals on FP5.
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
        // Deep register dumps from key SID 8 PIDs. C8 = type 0x51/0x3F (QG/BMS — fuel gauge, battery voltage/SOC) CB = type 0x0B/0x01 (BAT_IF) CA = type 0x0A/0x01 (MBG/ADC)
        for pid_u8 in [0xC8u8, 0xCBu8, 0xCAu8] {
            let mut regs = [0xFFu8; 32];
            let n = pmic_glink::read_sid8_regs(pid_u8, 0x00, &mut regs);
            log.buf_only("  C"); log.buf_put_hex32(pid_u8 as u32 & 0xF);
            log.buf_only("[00]: ");
            for i in 0..n { log.buf_put_hex32(regs[i] as u32); log.buf_only(" "); }
            log.buf_only("\n");
            let mut regs2 = [0xFFu8; 16];
            let n2 = pmic_glink::read_sid8_regs(pid_u8, 0x10, &mut regs2);
            log.buf_only("  C"); log.buf_put_hex32(pid_u8 as u32 & 0xF);
            log.buf_only("[10]: ");
            for i in 0..n2 { log.buf_put_hex32(regs2[i] as u32); log.buf_only(" "); }
            log.buf_only("\n");
        }
        // Extended QG/BMS (PID C8) register dump — fuel gauge data regions. 0x40-0x5F: config/data, 0x60-0x7F: FIFO data, 0x80-0x9F: more data, 0xA0-0xBF: SDAM/scratch, 0xC0-0xDF: SOC/capacity, 0xE0-0xFF: cal data.
        {
            log.puts("  QG C8 ext:\n");
            for base in [0x40u8, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0, 0xE0] {
                let mut regs = [0xFFu8; 16];
                let n = pmic_glink::read_sid8_regs(0xC8, base, &mut regs);
                log.buf_only("    ["); log.buf_put_hex32(base as u32); log.buf_only("]: ");
                for i in 0..n { log.buf_put_hex32(regs[i] as u32); log.buf_only(" "); }
                log.buf_only("\n");
            }
        }
        // Extended reads from C9 (type 0x0B/0x02) — might be charger/SCHG peripheral.
        {
            log.puts("  C9 ext:\n");
            for base in [0x40u8, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0, 0xE0] {
                let mut regs = [0xFFu8; 16];
                let n = pmic_glink::read_sid8_regs(0xC9, base, &mut regs);
                log.buf_only("    ["); log.buf_put_hex32(base as u32); log.buf_only("]: ");
                for i in 0..n { log.buf_put_hex32(regs[i] as u32); log.buf_only(" "); }
                log.buf_only("\n");
            }
        }

        // SDAM (Shared Direct Access Memory) — PID G#70, G#71 on SID 8. QG firmware writes battery state (SOC, OCV, ESR) here for HLOS. SDAM data registers start at offset 0x40 (SDAM_MEM_0).
        {
            log.puts("  SDAM 70:\n");
            for base in [0x00u8, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0, 0xE0] {
                let mut regs = [0xFFu8; 16];
                let n = pmic_glink::read_sid8_regs(0x70, base, &mut regs);
                log.buf_only("    ["); log.buf_put_hex32(base as u32); log.buf_only("]: ");
                for i in 0..n { log.buf_put_hex32(regs[i] as u32); log.buf_only(" "); }
                log.buf_only("\n");
            }
            log.puts("  SDAM 71:\n");
            for base in [0x00u8, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0, 0xE0] {
                let mut regs = [0xFFu8; 16];
                let n = pmic_glink::read_sid8_regs(0x71, base, &mut regs);
                log.buf_only("    ["); log.buf_put_hex32(base as u32); log.buf_only("]: ");
                for i in 0..n { log.buf_put_hex32(regs[i] as u32); log.buf_only(" "); }
                log.buf_only("\n");
            }
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
                let cap_install = ferros_pt::command::dev_cap(ferros_pt::command::caps::INSTALL);
                // Staging area for hot-reload / install kernel image
                const RELOAD_STAGE: usize = 0xA000_0000;
                let mut reload_size: usize = 0;
                // Flag: outbound DATA pump needs to send next packet
                let mut _pt_out_pump = false;

                loop {
                    match usb.poll_event() {
                        ferros_hal::usb::UsbEvent::None => {
                            // Yield CPU briefly when no USB events pending. GIC + WFI attempted but GICD access may be TZ-protected. TODO: probe GIC safely, fall back to spin if protected.
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
                            usb.handle_disconnect();
                            // Clear PT state — session is dead
                            pt_inbound = None;
                            pt_outbound = None;
                            pt_seq_width = 0;
                            pt_complete_len = 0;
                            pt_out_spec_len = 0;
                            pt_out_data.clear();
                            pt_out_bitmap.clear();
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
                                                                // RELOAD Write: payload is the new kernel binary Copy into staging DRAM
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

                                                                            // Write kernel binary to kernel storage region
                                                                            let kernel_lba = ferros_layout::KERNEL_A_BASE;
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
                                                                                    eagle_time: ferros_hal::qtimer::read_qtimer(),
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

                                                                        // Shut down peripherals before jump
                                                                        usb.shutdown();
                                                                        ferros_hal::gcc::sdc2_block_reset();
                                                                        hot_reload(RELOAD_STAGE, dtb_addr);
                                                                    }
                                                                }
                                                            } else if cmd.cap == cap_install && cmd.op == ferros_pt::Op::Write {
                                                                // INSTALL Write: same staging as RELOAD
                                                                let chunk = cmd.params;
                                                                if !chunk.is_empty() {
                                                                    unsafe {
                                                                        core::ptr::copy_nonoverlapping(
                                                                            chunk.as_ptr(),
                                                                            (RELOAD_STAGE + reload_size) as *mut u8,
                                                                            chunk.len(),
                                                                        );
                                                                    }
                                                                    reload_size += chunk.len();
                                                                }
                                                                pt_out_data.clear();
                                                                pt_out_data.extend_from_slice(&(reload_size as u32).to_le_bytes());
                                                            } else if cmd.cap == cap_install && cmd.op == ferros_pt::Op::Exec {
                                                                // INSTALL Exec: persist to UFS + signed stem entry (no jump) Params: [size:4 LE][hash:32][sig:64] = 100 bytes
                                                                log.buf_only("INSTALL exec size=");
                                                                log.buf_put_hex32(reload_size as u32);
                                                                log.buf_only("\n");
                                                                if cmd.params.len() >= 100 && reload_size > 0x1000 {
                                                                    let inst_size = u32::from_le_bytes(
                                                                        cmd.params[0..4].try_into().unwrap()
                                                                    ) as usize;
                                                                    let mut inst_hash = [0u8; 32];
                                                                    inst_hash.copy_from_slice(&cmd.params[4..36]);
                                                                    let mut inst_sig = [0u8; 64];
                                                                    inst_sig.copy_from_slice(&cmd.params[36..100]);

                                                                    // Verify staged size matches
                                                                    if inst_size != reload_size {
                                                                        log.buf_only("INSTALL size mismatch\n");
                                                                        pt_out_data.clear();
                                                                        pt_out_data.extend_from_slice(b"ERR:SIZE");
                                                                    } else {
                                                                        let kernel_slice = unsafe {
                                                                            core::slice::from_raw_parts(
                                                                                RELOAD_STAGE as *const u8,
                                                                                reload_size,
                                                                            )
                                                                        };

                                                                        // Verify BLAKE3
                                                                        let computed = blake3::hash(kernel_slice);
                                                                        if computed.as_bytes() != &inst_hash {
                                                                            log.buf_only("INSTALL hash mismatch\n");
                                                                            pt_out_data.clear();
                                                                            pt_out_data.extend_from_slice(b"ERR:HASH");
                                                                        } else {
                                                                            // Write to UFS at KERNEL_A_BASE
                                                                            let ufs = ferros_hal::ufs::UfsController::resume();
                                                                            let kernel_lba = ferros_layout::KERNEL_A_BASE;
                                                                            let blocks = (reload_size + 4095) / 4096;
                                                                            let mut write_ok = blocks <= ferros_layout::KERNEL_MAX_BLOCKS as usize;

                                                                            if write_ok {
                                                                                for i in 0..blocks {
                                                                                    let buf = ufs.data_buffer_mut();
                                                                                    let offset = i * 4096;
                                                                                    let copy_len = core::cmp::min(4096, reload_size - offset);
                                                                                    buf[..copy_len].copy_from_slice(&kernel_slice[offset..offset + copy_len]);
                                                                                    for b in copy_len..4096 { buf[b] = 0; }
                                                                                    if ufs.write_block(kernel_lba + i as u32) != 0 {
                                                                                        write_ok = false;
                                                                                        break;
                                                                                    }
                                                                                }
                                                                            }

                                                                            if write_ok {
                                                                                // Read back and verify
                                                                                let mut verify_ok = true;
                                                                                let mut verify_hasher = blake3::Hasher::new();
                                                                                for i in 0..blocks {
                                                                                    if ufs.read_block(kernel_lba + i as u32) != 0 {
                                                                                        verify_ok = false;
                                                                                        break;
                                                                                    }
                                                                                    let buf = ufs.data_buffer();
                                                                                    let offset = i * 4096;
                                                                                    let copy_len = core::cmp::min(4096, reload_size - offset);
                                                                                    verify_hasher.update(&buf[..copy_len]);
                                                                                }
                                                                                let verify_hash = verify_hasher.finalize();
                                                                                if verify_hash.as_bytes() != &inst_hash {
                                                                                    verify_ok = false;
                                                                                }

                                                                                if verify_ok {
                                                                                    // Write signed stem entry
                                                                                    let scan = ferros_hal::ring::scan_kernel_ring(&ufs);
                                                                                    let new_gen = scan.generation + 1;
                                                                                    let entry = ferros_hal::ring::KernelRingEntry {
                                                                                        generation: new_gen,
                                                                                        kernel_lba,
                                                                                        kernel_size: reload_size as u32,
                                                                                        kernel_hash: inst_hash,
                                                                                        kernel_sig: inst_sig,
                                                                                        eagle_time: ferros_hal::qtimer::read_qtimer(),
                                                                                        hp_hash: [0u8; 32],
                                                                                    };
                                                                                    if ferros_hal::ring::write_kernel_entry(&ufs, &entry) {
                                                                                        log.buf_only("INSTALL gen=");
                                                                                        log.buf_put_hex32(new_gen as u32);
                                                                                        log.buf_only(" lba=G#");
                                                                                        log.buf_put_hex32(kernel_lba);
                                                                                        log.buf_only(" OK\n");
                                                                                        pt_out_data.clear();
                                                                                        pt_out_data.extend_from_slice(b"OK");
                                                                                    } else {
                                                                                        log.buf_only("INSTALL stem FAIL\n");
                                                                                        pt_out_data.clear();
                                                                                        pt_out_data.extend_from_slice(b"ERR:STEM");
                                                                                    }
                                                                                } else {
                                                                                    log.buf_only("INSTALL verify FAIL\n");
                                                                                    pt_out_data.clear();
                                                                                    pt_out_data.extend_from_slice(b"ERR:VERIFY");
                                                                                }
                                                                            } else {
                                                                                log.buf_only("INSTALL write FAIL\n");
                                                                                pt_out_data.clear();
                                                                                pt_out_data.extend_from_slice(b"ERR:WRITE");
                                                                            }
                                                                        }
                                                                    }
                                                                    reload_size = 0;
                                                                } else {
                                                                    pt_out_data.clear();
                                                                    pt_out_data.extend_from_slice(b"ERR:PARAMS");
                                                                }
                                                            } else if cmd.cap == cap_reboot && cmd.op == ferros_pt::Op::Exec {
                                                                // REBOOT Exec: param[0] selects mode 0x00 = normal reboot, 0x01 = fastboot
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
                                                                // BEAM Read: read ring entries Params: [ring_id:1][mode:1][offset:4 BE][count:4 BE]
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
                                            // SPEC — new inbound transfer (clears stale state) New session — log + cancel stale state
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
                                // All DATA sent — no FIN for outbound response. Bridge knows chunk count from SPEC. Sending FIN would leave a stale IN transfer if bridge already returned after all_received().
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
    */  // END DISABLED FP5 boot sequence
}

/// Hot-reload: jump to a new kernel image at the given DRAM address.
///
/// The new image is position-independent (uses adrp). We disable caches, flush the staging area, then branch to _start with x0 = DTB.
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
/// SDAM_2 is at SPMI SID 8 (not 0!), PID 0x71 → PPID 0x0871, APID 0x122. Direct SPMI write returns success but value doesn't stick — try SCM IO write to the SPMI arbiter channel registers instead (TZ privilege).
fn psci_reboot_fastboot() -> ! {
    // APID 0x122 write channel: CHNLS_BASE + 0x122 * 0x1000 = 0x0C722000
    const SDAM2_CH: usize = 0x0C60_0000 + 0x122 * 0x1000;

    // Method 1: SCM IO write thru SPMI arbiter channel (TZ privilege) Write WDATA0 = 0x04 (FASTBOOT_MODE=0x02 << 1)
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

/// SCM IO write: TrustZone-privileged write to a physical address. SMC64 fast call: SVC_IO(5), CMD_WRITE(2).
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
