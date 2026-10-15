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

// Pixel 8 Pro (husky) modules shared by both boot paths: platform services, the USB/PT service, display adoption, the ira/wairua sources and the vault probe.
#[cfg(feature = "pixel8")]
mod platform;
#[cfg(feature = "pixel8")]
mod service;
#[cfg(feature = "pixel8")]
mod display;
// The ira-rooted vault probe and its ira/wairua sources (both boot paths now bring UFS up).
#[cfg(feature = "pixel8")]
mod vault_boot;
#[cfg(feature = "pixel8")]
mod ira;
#[cfg(feature = "pixel8")]
mod wairua;
// Post-handoff entry (ferros chainloaded from a running Linux shim; FERROS_SHIM_HANDOFF=1 → cfg shim_handoff).
#[cfg(all(feature = "pixel8", shim_handoff))]
mod shim;
// Cold boot from ABL: the UFS link-startup probe behind `bridge diag ufs`.
#[cfg(all(feature = "pixel8", not(shim_handoff)))]
mod cold_ufs;
// Cold boot from ABL: mark the booted A/B slot successful in devinfo, so ABL keeps booting it.
#[cfg(all(feature = "pixel8", not(shim_handoff)))]
mod slot;

// UFS-backed vault Device (windowed onto the ferros partition), used by shim::entry_genesis.
mod ufs_device;


// ---------------------------------------------------------------------------
// Global allocator
// ---------------------------------------------------------------------------

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// DRAM-based bump allocator. Base address set at startup from __stack_top linker symbol (right after kernel image + 64KB stack). Keeps binary small — no BSS heap array — so PT transfers don't grow circularly.
const HEAP_SIZE: usize = 4 * 1024 * 1024;

#[cfg_attr(feature = "pixel8", allow(dead_code))]
static HEAP_BASE: AtomicUsize = AtomicUsize::new(0);
#[cfg_attr(feature = "pixel8", allow(dead_code))]
static HEAP_POS: AtomicUsize = AtomicUsize::new(0);

#[cfg_attr(feature = "pixel8", allow(dead_code))]
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

// m1 keeps the lean leak-forever bump: its allocation is bounded and set up once.
#[cfg(not(feature = "pixel8"))]
#[global_allocator]
static ALLOC: BumpAlloc = BumpAlloc;

// Both Pixel boot paths run the long-lived PT service (and the handoff the vault, whose commits do many alloc/free cycles); a leak-forever bump would exhaust the 4 MiB heap, so they get the freeing free-list allocator.
#[cfg(feature = "pixel8")]
#[global_allocator]
static KERNEL_HEAP: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

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
    // Handoff diagnostic breadcrumb: prove we reached ferros's first instruction.
    // Written to the shim scratch (0x92F00100) so it survives a later crash and the kernel readback can see the EL2 jump landed.
    // Harmless on cold boot — 0x92F00100 is DRAM the cold-boot image (at 0x80000) never touches.
    // MMU is off on all our entry paths (shim jump + Tensor ABL), so this hits DRAM.
    movz    x9, #0x92F0, lsl #16    // x9 = 0x92F00000
    add     x9, x9, #0x100          // x9 = 0x92F00100 (entry breadcrumb slot)
    movz    x10, #0xBEE1            // "reached _entry"
    str     x10, [x9]
    dc      civac, x9               // push the breadcrumb to PoC so it survives a reset
    dsb     sy
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
    // Fault sink (shim path): if __exc_sink is armed (a ramoops result block), record the FIRST sync fault into the block's hardcoded print slots — ELR@+0xA0, FAR@+0xA8 (the 16-byte hexdump), ESR@+0x20 (the r[4] byte fields), count@+0x28 (r[5]) — stamp the magic, and warm-reset via PSCI so DRAM survives and the next Linux boot prints it. Skipping the faulting instruction (the default below) only produces garbage state, so on the shim path a fault is terminal by design. Cold/m1 builds leave the sink at 0 and keep the skip-and-count behaviour.
    stp     x2, x3, [sp, #-16]!
    adrp    x0, __exc_sink
    add     x0, x0, :lo12:__exc_sink
    ldr     x0, [x0]
    cbz     x0, .Lexc_nosink
    ldr     x1, [x0, #0xA0]
    cbnz    x1, .Lexc_nosink
    mrs     x1, CurrentEL
    lsr     x1, x1, #2
    cmp     x1, #2
    b.eq    .Lexc_sink_el2
    mrs     x1, elr_el1
    mrs     x2, far_el1
    mrs     x3, esr_el1
    b       .Lexc_sink_store
.Lexc_sink_el2:
    mrs     x1, elr_el2
    mrs     x2, far_el2
    mrs     x3, esr_el2
.Lexc_sink_store:
    str     x1, [x0, #0xA0]
    str     x2, [x0, #0xA8]
    str     x3, [x0, #0x20]
    adrp    x1, __exception_count
    add     x1, x1, :lo12:__exception_count
    ldr     x1, [x1]
    str     x1, [x0, #0x28]
    movz    x1, #0x6546
    movk    x1, #0x6F52, lsl #16
    movk    x1, #0x6655, lsl #32
    movk    x1, #0x3153, lsl #48
    str     x1, [x0]
    dc      cvac, x0
    add     x1, x0, #0x20
    dc      cvac, x1
    add     x1, x0, #0xA0
    dc      cvac, x1
    dsb     sy
    mov     w0, #0x0009
    movk    w0, #0x8400, lsl #16
    smc     #0
.Lexc_nosink:
    ldp     x2, x3, [sp], #16
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
/// Fault sink for the sync exception handler: 0 = skip-and-count (cold/m1 boots); nonzero = physical address of a ramoops result block that the FIRST fault is recorded into before a PSCI warm reset (shim path, see `.Lexc_recover`).
#[unsafe(no_mangle)]
pub static mut __exc_sink: u64 = 0;
#[unsafe(no_mangle)]
static mut __boot_sctlr: u64 = 0;

fn exception_count() -> u64 {
    unsafe { core::ptr::read_volatile(&raw const __exception_count) }
}

/// Exception level the entry asm found at boot (2 = bare EL2, as both husky paths need).
#[cfg_attr(not(feature = "pixel8"), allow(dead_code))]
fn boot_el() -> u64 {
    unsafe { core::ptr::read_volatile(&raw const __boot_el) }
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

// Post-handoff entry: ferros was chainloaded by the Linux shim via cpu_soft_restart into the 0x92400000 carveout, MMU off at EL2, with the UFS link already alive.
// This kernel_main replaces the cold-boot one for the shim-linked image; it must NOT run the ABL-state-dependent cold-boot setup (watchdog reuse, S2MPU/eUSB assumptions), so it dispatches straight to shim.
/// eUSB2 PHY + USB link bring-up on Tensor G3. Faithful port of phy_exynos_eusb_initiate (tools/pixel8/phy-ref/phy-exynos-eusb.c), 19.2MHz / 4nm; bit positions from eusb-con-reg.h, link init from exynos-usb-blkcon.c. Re-runnable: the enumeration watchdog re-inits the PHY when the host never talks to us. Shared by the cold-boot path and the shim (post-handoff) USB path.
#[cfg(feature = "pixel8")]
pub(crate) fn pixel8_eusb_phy_init() {
    const USBCON: usize = 0x1110_0000;
    const EUSB_PHY: usize = 0x1111_0000;
    unsafe {
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

            // UTMI_CTRL(G#10): force B-valid (b0), VBUS-valid (b1) and UTMI TX bit-stuff enable (b2), the live value under Linux (7). The link's own session detection is disabled above (dis_bvalid), so b0|b1 ARE the session the controller sees (exynos-usb-blkcon.c); this used to set b1|b2 and so never forced B-valid. Register parity with Linux, from diffing USBCON cold vs handoff — it was not the cause of the flaky cold enumeration (that was the EP0 status stage, ferros_hal usb.rs StatusInWait).
            reg = core::ptr::read_volatile((USBCON + 0x0010) as *const u32);
            reg |= (1 << 0) | (1 << 1) | (1 << 2);
            core::ptr::write_volatile((USBCON + 0x0010) as *mut u32, reg);

            // HSP_MISC(G#114): resistance-tune calibration chain = eUSB PHY only (sel_res_tune_mux b1:0 = 2, set_req_in2 b4 = 1, set_ack_in2/req_in1/ack_in1 b5/b6/b7 = 0), as exynos_usbcon_disable_pipe3_phy does when the USB3/DP combo PHY is down, which it always is here. ABL leaves 0 (Phy2 → Phy1); Linux, with the combo PHY up, uses 1.
            reg = core::ptr::read_volatile((USBCON + 0x0114) as *const u32);
            reg &= !(0x3 | (0xF << 4));
            reg |= 2 | (1 << 4);
            core::ptr::write_volatile((USBCON + 0x0114) as *mut u32, reg);

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

            // TXTUNE(G#14): fsls_vref_tune (b2:1) = 0, as phy_exynos_eusb_initiate does right before releasing IDDQ (it then applies the DT tune table; on husky the rest of TXTUNE already matches Linux's tuned value G#9030 — cold ABL leaves G#9034).
            reg = core::ptr::read_volatile((EUSB_PHY + 0x0014) as *const u32);
            reg &= !(0x3 << 1);
            core::ptr::write_volatile((EUSB_PHY + 0x0014) as *mut u32, reg);

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
    }
}

#[cfg(all(feature = "pixel8", shim_handoff))]
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(dtb_addr: u64) -> ! {
    // Init the DRAM bump heap (same as cold boot; __stack_top is inside the carveout for the shim link, so this stays in reserved DRAM).
    unsafe extern "C" { static __stack_top: u8; }
    let stack_top = unsafe { &__stack_top as *const u8 as usize };
    let heap_base = (stack_top + 0xFFF) & !0xFFF;
    unsafe { KERNEL_HEAP.init(heap_base, HEAP_SIZE) };
    platform::enable_icache();

    // Default: the USB service (PT over the inherited controller, disk live). FERROS_SHIM_ENTRY=vault builds the one-shot vault probe that reports and resets.
    if option_env!("FERROS_SHIM_ENTRY") == Some("vault") {
        shim::entry_vault(dtb_addr)
    } else {
        shim::entry_usb(dtb_addr)
    }
}

#[cfg(all(feature = "pixel8", not(shim_handoff)))]
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(dtb_addr: u64) -> ! {
    // Cold boot from ABL (slot b): ABL loads the image at its link base G#8008_0000 (aarch64-pixel8.ld) with the MMU and caches off, x0 = DTB pointer. Heap right after the image + 64 KB stack.
    unsafe extern "C" { static __stack_top: u8; static _start: u8; }
    let stack_top = unsafe { &__stack_top as *const u8 as usize };
    let heap_base = (stack_top + 0xFFF) & !0xFFF;
    unsafe { KERNEL_HEAP.init(heap_base, HEAP_SIZE) };
    platform::enable_icache();

    // The previous session's report (ramoops DRAM survives warm resets), then the safety nets: fault sink + panic reporter into the report block, cluster watchdogs.
    let prev = platform::read_report();
    platform::arm_safety();
    platform::crumb(0xC0);

    // eUSB2 repeater: snapshot what ABL left, then apply Google's calibrated tune (ABL never does; only the Android driver does).
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

    platform::crumb(0xC1);

    // UFS: ABL leaves the flash unpowered (cold_ufs::power_on_device); bring the link up, then open the ira-rooted vault on the ferros partition exactly as the handoff path does.
    let t_ufs = platform::cntpct();
    let link_up = cold_ufs::bring_up();
    let ufs_ms = (platform::cntpct().wrapping_sub(t_ufs) / (platform::cntfrq() / 1000)) as u32;
    platform::crumb(0xC2);
    let t_vault = platform::cntpct();
    let (probe, store) = vault_boot::vault_probe(link_up);
    let vault_ms = (platform::cntpct().wrapping_sub(t_vault) / (platform::cntfrq() / 1000)) as u32;

    // Tell ABL this slot boots (devinfo "successful"), so its retry countdown stops and a power cycle lands here again. Only on a real ABL boot (x0 = DTB pointer, not a RELOAD tag) with the vault verified: a build that dies before this point still falls back to the other slot.
    let mark = if dtb_addr != 0 && dtb_addr & 1 == 0 && store.is_some() {
        slot::mark_booted_slot(dtb_addr)
    } else {
        slot::SlotMark { slot: 0xFF, before: 0, after: 0, result: 0xFF }
    };

    let own_base = &raw const _start as usize;
    let extras = alloc::vec![
        (b"COLD_UFS_MS=".as_slice(), ufs_ms),
        (b"COLD_VAULT_MS=".as_slice(), vault_ms),
        (b"REP_REV=".as_slice(), rep_snap[0]),
        (b"REP_CFG=".as_slice(), rep_snap[1]),
        (b"REP_T70=".as_slice(), rep_snap[2]),
        (b"REP_T71=".as_slice(), rep_snap[3]),
        (b"REP_T72=".as_slice(), rep_snap[4]),
        (b"REP_T73=".as_slice(), rep_snap[5]),
        (b"REP_T77=".as_slice(), rep_snap[6]),
        (b"REP_T78=".as_slice(), rep_snap[7]),
        (b"REP_T79=".as_slice(), rep_snap[8]),
        (b"REP_TUNED=".as_slice(), rep_tuned),
        (b"DTB=".as_slice(), dtb_addr as u32),
        (b"SLOT=".as_slice(), mark.slot),
        (b"SLOT_WORD_BEFORE=".as_slice(), mark.before),
        (b"SLOT_WORD_AFTER=".as_slice(), mark.after),
        (b"SLOT_MARK=".as_slice(), mark.result),
    ];
    // RELOAD stages at base+32 MiB (the slot the old ping-pong reload proved on hardware; base+64 MiB was fatal) and runs its copy stub 64 KiB below it, far past this image + stack + 4 MiB heap.
    service::run(service::Boot {
        kind: service::BootKind::Cold,
        link_base: own_base,
        stage: own_base + (32 << 20),
        stub: own_base + (32 << 20) - 0x1_0000,
        x0: if dtb_addr & 1 == 1 { dtb_addr } else { 0 },
        probe: Some(probe),
        store,
        prev,
        extras,
        ufs_probe: Some(cold_ufs::probe),
    })
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

#[cfg(not(feature = "pixel8"))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop { unsafe { core::arch::asm!("wfe") }; }
}

// Both Pixel boot paths: a silent wfe loop is indistinguishable from a hang and only the watchdog gets us out. Report the panic (message head, file, line, last crumb) into the ramoops result block and warm-reset, so the next boot (Linux on slot a, or the next cold ferros's DIAG) shows it.
#[cfg(feature = "pixel8")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    platform::report_panic(info)
}
