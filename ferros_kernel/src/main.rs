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
mod settings;
// Vol Up + Vol Down → power off, as the key-press interrupt itself (KILLSWITCH.md).
#[cfg(feature = "pixel8")]
mod killswitch;
#[cfg(feature = "pixel8")]
mod service;
#[cfg(feature = "pixel8")]
mod display;
#[cfg(feature = "pixel8")]
mod dsim;
#[cfg(feature = "pixel8")]
mod hk3;
#[cfg(feature = "pixel8")]
mod dpu;
#[cfg(feature = "pixel8")]
mod hsi2c;
#[cfg(feature = "pixel8")]
mod acpm;
mod odpm;
mod pd;
#[cfg(feature = "pixel8")]
mod camera;
#[cfg(feature = "pixel8")]
mod camera_events;
#[cfg(feature = "pixel8")]
mod camera_snap;
mod x3;
mod smp;
mod rtc;
mod aoc;
mod usf;
mod haptics;
mod gnss;
mod nfc;
mod bt;
mod tpu;
mod gsa;
mod ffa;
mod trusty;
#[cfg(feature = "pixel8")]
mod camera_close;
mod camera_rx;
#[cfg(feature = "pixel8")]
mod camera_rx_uw;
#[cfg(feature = "pixel8")]
mod spi;
mod fp;
#[cfg(feature = "pixel8")]
mod touch;
#[cfg(feature = "pixel8")]
mod amp;
#[cfg(feature = "pixel8")]
mod partfs;
#[cfg(feature = "pixel8")]
mod flashlog;
#[cfg(feature = "pixel8")]
mod kvault;
#[cfg(feature = "pixel8")]
mod modem;
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
// Cold boot: the HSI2 S2MPU fence (what UFS and the WiFi chip may DMA into).
#[cfg(all(feature = "pixel8", not(shim_handoff)))]
mod s2mpu;
// The static cache map: one identity table at boot so ferros's own memory is cacheable (KERNEL.md, Execution Model).
#[cfg(feature = "pixel8")]
mod cachemap;
// KOHI: PIPE's entropy gathering + health stack in software (the wairua's source).
#[cfg(feature = "pixel8")]
mod kohi;
// The performance monitors, open to EL0 on every core (pmu.rs).
#[cfg(feature = "pixel8")]
mod pmu;
// Where the ira and wairua could live off DRAM: a probe of the unused CPU registers (keyreg.rs, KEY_REGISTERS.md).
#[cfg(feature = "pixel8")]
mod keyreg;
// The ira and wairua held in CPU registers, never DRAM (keys.rs; the code that touches them is lifted from keyreg/keys.rs).
#[cfg(feature = "pixel8")]
mod keys;
// The gate and the oracle: signed capsules, ihi at spawn (ORACLE.md).
#[cfg(feature = "pixel8")]
mod oracle;
// Processes: EL0 in an arc, capabilities, the syscall boundary.
#[cfg(feature = "pixel8")]
mod process;
// The "Ready?" gate: the boot proper starts at the power key's press (BOOT.md).
#[cfg(all(feature = "pixel8", not(shim_handoff)))]
mod ready;
// Bring-up checklist on the panel: red until done, green once done.
#[cfg(feature = "pixel8")]
mod checklist;
mod text;

// UFS-backed vault Device (windowed onto the ferros partition), used by shim::entry_genesis.
mod ufs_device;


// ---------------------------------------------------------------------------
// Global allocator
// ---------------------------------------------------------------------------

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// DRAM-based bump allocator. Base address set at startup from __stack_top linker symbol (right after kernel image + the kernel stack). Keeps binary small — no BSS heap array — so PT transfers don't grow circularly.
// Cold boot: 12 MiB (room for multi-MiB PT transfers — WiFi firmware, a display capsule with 256-px icons, 2.2 MB, which with the store's copies ran 8 MiB out, 2026-10-23; image + stack + heap end below G#8100_0000, where the arc pool starts: G#80F3_4000 today). The handoff carveout is 12 MiB with its staging slot 6 MiB in, so it keeps 4 MiB.
#[cfg(not(shim_handoff))]
const HEAP_SIZE: usize = 12 * 1024 * 1024;
#[cfg(shim_handoff)]
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
/// Whether the cache map is on, so the heap's second region can join (it must be normal memory: an allocation's unaligned copies fault on Device memory).
static HEAP2_CACHED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static HEAP2_JOINED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// From the service loop, once, a couple of seconds after it starts: the heap's second region (cachemap HEAP2) prepared and added to the kernel heap. The first 12 MiB carry the boot until then.
#[cfg(not(shim_handoff))]
pub fn heap2_join() {
    if !HEAP2_CACHED.load(Ordering::Relaxed) || HEAP2_JOINED.swap(true, Ordering::Relaxed) {
        return;
    }
    cachemap::heap2_prepare();
    unsafe { KERNEL_HEAP.extend(cachemap::HEAP2.0, cachemap::HEAP2.1 - cachemap::HEAP2.0) };
}

/// The kernel heap's (free bytes, free blocks, largest free block), for DIAG; None if the heap is busy.
pub fn heap_stats() -> Option<(usize, usize, usize)> {
    KERNEL_HEAP.try_lock().map(|h| (h.free_bytes(), h.free_blocks(), h.largest_free()))
}

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
    b       .Lirq                  // EL1t IRQ
    .balign 0x80
    b .Lhalt                       // EL1t FIQ
    .balign 0x80
    b       .Lexc_serror           // EL1t SError

    .balign 0x80
    b       .Lexc_recover          // EL1h sync
    .balign 0x80
    b       .Lirq                  // EL1h IRQ (at EL2: current EL, SP_EL2)
    .balign 0x80
    b .Lhalt                       // EL1h FIQ
    .balign 0x80
    b       .Lexc_serror           // EL1h SError

    .balign 0x80
    b       .Lel0_sync             // Lower AArch64 sync (a process: syscall or fault)
    .balign 0x80
    b       .Lirq                  // Lower AArch64 IRQ (the killswitch preempts processes too)
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
// IRQ = the killswitch (ferros_kernel/src/killswitch.rs). Whatever fired, both volume keys held (gpa4-1 and gpa6-2 low, G#154D_0084 / G#154D_00A4) → PSCI SYSTEM_OFF at once: no zeroing, no cleanup (KILLSWITCH.md). Otherwise a key edge clears its EINT pending bits; any other interrupt is switched off at the distributor (ferros polls everything else); then EOI and return. Spurious (INTID >= 1020) returns without EOI. If SYSTEM_OFF ever returns, sit with IRQs masked until the unpetted watchdog resets the SoC.
.Lirq:
    stp     x0, x1, [sp, #-32]!
    stp     x2, x3, [sp, #16]
    // Stage 1: entered (x1 = this core's stage word, kept in x1 only until the first ldr below).
    adrp    x1, __irq_stage
    add     x1, x1, :lo12:__irq_stage
    mrs     x2, mpidr_el1
    ubfx    x2, x2, #8, #4
    mov     x3, #1
    str     x3, [x1, x2, lsl #3]
    mrs     x0, S3_0_C12_C12_0     // ICC_IAR1_EL1
    movz    x1, #0x0084
    movk    x1, #0x154D, lsl #16
    ldr     w2, [x1]
    ldr     w3, [x1, #0x20]
    tst     w2, #2
    b.ne    .Lirq_live
    tst     w3, #4
    b.ne    .Lirq_live
    movz    x0, #0x0008
    movk    x0, #0x8400, lsl #16
    smc     #0
.Lirq_dead:
    adrp    x1, __irq_stage
    add     x1, x1, :lo12:__irq_stage
    mrs     x2, mpidr_el1
    ubfx    x2, x2, #8, #4
    mov     x3, #3
    str     x3, [x1, x2, lsl #3]
    wfi
    b       .Lirq_dead
.Lirq_live:
    adrp    x1, __irq_stage
    add     x1, x1, :lo12:__irq_stage
    mrs     x2, mpidr_el1
    ubfx    x2, x2, #8, #4
    mov     x3, #2
    str     x3, [x1, x2, lsl #3]
    adrp    x1, __irq_count
    add     x1, x1, :lo12:__irq_count
    ldr     x2, [x1]
    add     x2, x2, #1
    str     x2, [x1]
    cmp     x0, #1020
    b.hs    .Lirq_ret
    cmp     x0, #2
    b.eq    .Lirq_dump
    cmp     x0, #26
    b.eq    .Lirq_tick
    cmp     x0, #60
    b.eq    .Lirq_power
    cmp     x0, #61
    b.eq    .Lirq_key
    cmp     x0, #64
    b.eq    .Lirq_key
    cmp     x0, #32
    b.lo    .Lirq_eoi
    lsr     x2, x0, #5
    and     x3, x0, #31
    mov     w1, #1
    lsl     w1, w1, w3
    movz    x3, #0x0180
    movk    x3, #0x1040, lsl #16
    str     w1, [x3, x2, lsl #2]   // GICD_ICENABLER<n>: masked until a driver's irq_ack (or for good, if no one holds the line)
    adrp    x1, __irq_stage
    add     x1, x1, :lo12:__irq_stage
    mrs     x2, mpidr_el1
    ubfx    x2, x2, #8, #4
    mov     x3, #4
    str     x3, [x1, x2, lsl #3]
    // The line is pending for whoever holds its grant (process.rs IRQS); from EL0 the running thread is parked so the scheduler can run the waiter now, from EL2 the service loop's next slice does.
    adrp    x1, __irq_pending
    add     x1, x1, :lo12:__irq_pending
    mov     w2, #1
    strb    w2, [x1, x0]
    adrp    x1, __irq_any
    add     x1, x1, :lo12:__irq_any
    str     x2, [x1]
    dsb     sy
    sev                            // the kernel's core, if in wfe, looks for the waiter
    mov     x1, #(1 << 24)
    orr     x1, x1, #(1 << 40)
    msr     S3_0_C12_C11_5, x1     // ICC_SGI1R_EL1: SGI 1 to every other core — the workers sleep in wfi
    isb
    msr     S3_0_C12_C12_1, x0     // ICC_EOIR1_EL1
    isb
    adrp    x1, __irq_stage
    add     x1, x1, :lo12:__irq_stage
    mrs     x2, mpidr_el1
    ubfx    x2, x2, #8, #4
    mov     x3, #5
    str     x3, [x1, x2, lsl #3]
    mrs     x1, spsr_el2
    and     x1, x1, #0xF
    cbnz    x1, .Lirq_ret
    adrp    x1, __irq_stage
    add     x1, x1, :lo12:__irq_stage
    mov     x3, #6
    str     x3, [x1, x2, lsl #3]
    ldp     x2, x3, [sp, #16]
    ldp     x0, x1, [sp], #32
    b       .Lel0_preempt
// The power key was pressed (gpa4-0 falling edge, killswitch.rs arm_power_key): record it for the "Ready?" gate and clear its pending bit.
.Lirq_power:
    adrp    x1, __power_pressed
    add     x1, x1, :lo12:__power_pressed
    mov     x2, #1
    str     x2, [x1]
    movz    x1, #0x0A10
    movk    x1, #0x154D, lsl #16
    mov     w2, #1
    str     w2, [x1]               // EINT_PEND gpa4: power
    dsb     sy
    b       .Lirq_eoi
// The EL2 timer (PPI 26): a process's time slice is up. Stop the timer and EOI; from EL2 there is nothing to preempt, from EL0 park the process (.Lel0_preempt) and give the CPU back to the kernel.
.Lirq_tick:
    msr     cnthp_ctl_el2, xzr
    msr     S3_0_C12_C12_1, x0     // ICC_EOIR1_EL1
    isb
    mrs     x1, spsr_el2
    and     x1, x1, #0xF
    cbnz    x1, .Lirq_ret
    ldp     x2, x3, [sp, #16]
    ldp     x0, x1, [sp], #32
    b       .Lel0_preempt
// SGI 2: dump where this core is (process.rs `diag core`): the interrupted ELR/SPSR, both stack pointers, and x0-x3 as saved at entry.
.Lirq_dump:
    adrp    x1, __core_dump
    add     x1, x1, :lo12:__core_dump
    mrs     x2, mpidr_el1
    ubfx    x2, x2, #8, #4
    add     x1, x1, x2, lsl #6
    mrs     x2, elr_el2
    str     x2, [x1, #0]
    mrs     x2, spsr_el2
    str     x2, [x1, #8]
    add     x2, sp, #32
    str     x2, [x1, #16]
    mrs     x2, sp_el0
    str     x2, [x1, #24]
    ldp     x2, x3, [sp]
    stp     x2, x3, [x1, #32]
    ldp     x2, x3, [sp, #16]
    stp     x2, x3, [x1, #48]
    dsb     sy
    b       .Lirq_eoi
.Lirq_key:
    movz    x1, #0x0A10
    movk    x1, #0x154D, lsl #16
    mov     w2, #2
    str     w2, [x1]               // EINT_PEND gpa4: vol-down
    mov     w2, #4
    str     w2, [x1, #4]           // EINT_PEND gpa6: vol-up
    dsb     sy
.Lirq_eoi:
    msr     S3_0_C12_C12_1, x0     // ICC_EOIR1_EL1
    isb
.Lirq_ret:
    adrp    x1, __irq_stage
    add     x1, x1, :lo12:__irq_stage
    mrs     x2, mpidr_el1
    ubfx    x2, x2, #8, #4
    mov     x3, #7
    str     x3, [x1, x2, lsl #3]
    ldp     x2, x3, [sp, #16]
    ldp     x0, x1, [sp], #32
    eret

.balign 16
// Enter a process at EL0 (process.rs `spawn`): x0 = entry, x1 = SP_EL0, x2 = arc base → x28, x3 = arc mask → x27, x4..x6 = the process's x0..x2. Saves the kernel's callee-saved registers and SP in __el0_kctx; `.Lel0_leave` returns from this call with the exit value. Every other register — general and SIMD — is cleared so no kernel value leaks into the process. SPSR = EL0t with interrupts unmasked: the killswitch keeps preempting.
.global ferros_el0_enter
ferros_el0_enter:
    msr     daifset, #2            // as in ferros_el0_resume
    adrp    x9, __el0_kctx
    add     x9, x9, :lo12:__el0_kctx
    mrs     x10, mpidr_el1          // this core's slot: MPIDR Aff1 (husky: 0-8), 192 bytes each
    ubfx    x10, x10, #8, #4
    add     x9, x9, x10, lsl #7
    add     x9, x9, x10, lsl #6
    stp     x19, x20, [x9, #0]
    stp     x21, x22, [x9, #16]
    stp     x23, x24, [x9, #32]
    stp     x25, x26, [x9, #48]
    stp     x27, x28, [x9, #64]
    stp     x29, x30, [x9, #80]
    mov     x10, sp
    str     x10, [x9, #96]
    stp     d8, d9, [x9, #104]
    stp     d10, d11, [x9, #120]
    stp     d12, d13, [x9, #136]
    stp     d14, d15, [x9, #152]
    msr     elr_el2, x0
    msr     sp_el0, x1
    msr     spsr_el2, xzr
    mov     x28, x2
    mov     x27, x3
    mov     x0, x4
    mov     x1, x5
    mov     x2, x6
    mov     x3, xzr
    mov     x4, xzr
    mov     x5, xzr
    mov     x6, xzr
    mov     x7, xzr
    mov     x8, xzr
    mov     x9, xzr
    mov     x10, xzr
    mov     x11, xzr
    mov     x12, xzr
    mov     x13, xzr
    mov     x14, xzr
    mov     x15, xzr
    mov     x16, xzr
    mov     x17, xzr
    mov     x18, xzr
    mov     x19, xzr
    mov     x20, xzr
    mov     x21, xzr
    mov     x22, xzr
    mov     x23, xzr
    mov     x24, xzr
    mov     x25, xzr
    mov     x26, xzr
    mov     x29, xzr
    mov     x30, xzr
    movi    v0.16b, #0
    movi    v1.16b, #0
    movi    v2.16b, #0
    movi    v3.16b, #0
    movi    v4.16b, #0
    movi    v5.16b, #0
    movi    v6.16b, #0
    movi    v7.16b, #0
    movi    v8.16b, #0
    movi    v9.16b, #0
    movi    v10.16b, #0
    movi    v11.16b, #0
    movi    v12.16b, #0
    movi    v13.16b, #0
    movi    v14.16b, #0
    movi    v15.16b, #0
    movi    v16.16b, #0
    movi    v17.16b, #0
    movi    v18.16b, #0
    movi    v19.16b, #0
    movi    v20.16b, #0
    movi    v21.16b, #0
    movi    v22.16b, #0
    movi    v23.16b, #0
    movi    v24.16b, #0
    movi    v25.16b, #0
    movi    v26.16b, #0
    movi    v27.16b, #0
    movi    v28.16b, #0
    movi    v29.16b, #0
    movi    v30.16b, #0
    movi    v31.16b, #0
    eret

.balign 16
// Resume a parked process (process.rs `run_slice`): x0 = its saved frame (816 bytes, the .Lel0_sync layout: x0–x30, SP_EL0, ELR, SPSR, ESR, FAR, then v0–v31, FPCR, FPSR). Saves the kernel context like ferros_el0_enter, copies the frame onto the stack and erets through the common restore path.
.global ferros_el0_resume
ferros_el0_resume:
    // IRQs masked from here to the eret (the eret restores the thread's own PSTATE). Without this, an interrupt taken between `msr elr_el2`/`msr spsr_el2` in .Lel0_restore and the eret overwrote both with the eret's own address and EL2h, and the eret then returned to itself at EL2 forever — a core parked on one instruction with interrupts open, no slice ticks, no traps (2026-10-23; with SGI kicks at kilohertz it happened within seconds of a drag). The trap-return path never had this: exception entry masks IRQs.
    msr     daifset, #2
    adrp    x9, __el0_kctx
    add     x9, x9, :lo12:__el0_kctx
    mrs     x10, mpidr_el1          // this core's slot: MPIDR Aff1 (husky: 0-8), 192 bytes each
    ubfx    x10, x10, #8, #4
    add     x9, x9, x10, lsl #7
    add     x9, x9, x10, lsl #6
    stp     x19, x20, [x9, #0]
    stp     x21, x22, [x9, #16]
    stp     x23, x24, [x9, #32]
    stp     x25, x26, [x9, #48]
    stp     x27, x28, [x9, #64]
    stp     x29, x30, [x9, #80]
    mov     x10, sp
    str     x10, [x9, #96]
    stp     d8, d9, [x9, #104]
    stp     d10, d11, [x9, #120]
    stp     d12, d13, [x9, #136]
    stp     d14, d15, [x9, #152]
    sub     sp, sp, #832
    mov     x10, #0
1:  ldr     x11, [x0, x10]
    str     x11, [sp, x10]
    add     x10, x10, #8
    cmp     x10, #832
    b.lo    1b
    b       .Lel0_restore

.balign 16
// Synchronous exception from EL0 (a syscall or a fault): save the process's registers in a frame on SP_EL2, let process.rs decide (ferros_el0_trap: 0 = resume, 1 = the process ends).
.Lel0_sync:
    sub     sp, sp, #832
    stp     x0, x1, [sp, #0]
    stp     x2, x3, [sp, #16]
    stp     x4, x5, [sp, #32]
    stp     x6, x7, [sp, #48]
    stp     x8, x9, [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    str     x30, [sp, #240]
    mrs     x9, sp_el0
    str     x9, [sp, #248]
    mrs     x9, elr_el2
    str     x9, [sp, #256]
    mrs     x9, spsr_el2
    str     x9, [sp, #264]
    mrs     x9, esr_el2
    str     x9, [sp, #272]
    mrs     x9, far_el2
    str     x9, [sp, #280]
    stp     q0, q1, [sp, #288]
    stp     q2, q3, [sp, #320]
    stp     q4, q5, [sp, #352]
    stp     q6, q7, [sp, #384]
    stp     q8, q9, [sp, #416]
    stp     q10, q11, [sp, #448]
    stp     q12, q13, [sp, #480]
    stp     q14, q15, [sp, #512]
    stp     q16, q17, [sp, #544]
    stp     q18, q19, [sp, #576]
    stp     q20, q21, [sp, #608]
    stp     q22, q23, [sp, #640]
    stp     q24, q25, [sp, #672]
    stp     q26, q27, [sp, #704]
    stp     q28, q29, [sp, #736]
    stp     q30, q31, [sp, #768]
    mrs     x9, fpcr
    str     x9, [sp, #800]
    mrs     x9, fpsr
    str     x9, [sp, #808]
    mrs     x9, tpidr_el0
    str     x9, [sp, #816]
    mov     x0, sp
    bl      ferros_el0_trap
    b       .Lel0_tail
// The time slice ran out while a process was at EL0 (the timer tick in .Lirq, scratch registers already restored): save its frame the same way and let process.rs park it runnable.
.Lel0_preempt:
    sub     sp, sp, #832
    stp     x0, x1, [sp, #0]
    stp     x2, x3, [sp, #16]
    stp     x4, x5, [sp, #32]
    stp     x6, x7, [sp, #48]
    stp     x8, x9, [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    str     x30, [sp, #240]
    mrs     x9, sp_el0
    str     x9, [sp, #248]
    mrs     x9, elr_el2
    str     x9, [sp, #256]
    mrs     x9, spsr_el2
    str     x9, [sp, #264]
    mrs     x9, esr_el2
    str     x9, [sp, #272]
    mrs     x9, far_el2
    str     x9, [sp, #280]
    stp     q0, q1, [sp, #288]
    stp     q2, q3, [sp, #320]
    stp     q4, q5, [sp, #352]
    stp     q6, q7, [sp, #384]
    stp     q8, q9, [sp, #416]
    stp     q10, q11, [sp, #448]
    stp     q12, q13, [sp, #480]
    stp     q14, q15, [sp, #512]
    stp     q16, q17, [sp, #544]
    stp     q18, q19, [sp, #576]
    stp     q20, q21, [sp, #608]
    stp     q22, q23, [sp, #640]
    stp     q24, q25, [sp, #672]
    stp     q26, q27, [sp, #704]
    stp     q28, q29, [sp, #736]
    stp     q30, q31, [sp, #768]
    mrs     x9, fpcr
    str     x9, [sp, #800]
    mrs     x9, fpsr
    str     x9, [sp, #808]
    mrs     x9, tpidr_el0
    str     x9, [sp, #816]
    mov     x0, sp
    bl      ferros_el0_preempt
.Lel0_tail:
    cbnz    x0, .Lel0_leave
.Lel0_restore:
    ldr     x9, [sp, #256]
    msr     elr_el2, x9
    ldr     x9, [sp, #264]
    msr     spsr_el2, x9
    ldr     x9, [sp, #248]
    msr     sp_el0, x9
    ldr     x9, [sp, #800]
    msr     fpcr, x9
    ldr     x9, [sp, #808]
    msr     fpsr, x9
    ldr     x9, [sp, #816]
    msr     tpidr_el0, x9
    ldp     q0, q1, [sp, #288]
    ldp     q2, q3, [sp, #320]
    ldp     q4, q5, [sp, #352]
    ldp     q6, q7, [sp, #384]
    ldp     q8, q9, [sp, #416]
    ldp     q10, q11, [sp, #448]
    ldp     q12, q13, [sp, #480]
    ldp     q14, q15, [sp, #512]
    ldp     q16, q17, [sp, #544]
    ldp     q18, q19, [sp, #576]
    ldp     q20, q21, [sp, #608]
    ldp     q22, q23, [sp, #640]
    ldp     q24, q25, [sp, #672]
    ldp     q26, q27, [sp, #704]
    ldp     q28, q29, [sp, #736]
    ldp     q30, q31, [sp, #768]
    ldp     x0, x1, [sp, #0]
    ldp     x2, x3, [sp, #16]
    ldp     x4, x5, [sp, #32]
    ldp     x6, x7, [sp, #48]
    ldp     x8, x9, [sp, #64]
    ldp     x10, x11, [sp, #80]
    ldp     x12, x13, [sp, #96]
    ldp     x14, x15, [sp, #112]
    ldp     x16, x17, [sp, #128]
    ldp     x18, x19, [sp, #144]
    ldp     x20, x21, [sp, #160]
    ldp     x22, x23, [sp, #176]
    ldp     x24, x25, [sp, #192]
    ldp     x26, x27, [sp, #208]
    ldp     x28, x29, [sp, #224]
    ldr     x30, [sp, #240]
    add     sp, sp, #832
    eret
// The process ended: return from ferros_el0_enter with its x0 (the exit value), kernel registers and SP restored.
.Lel0_leave:
    ldr     x0, [sp, #0]
    adrp    x9, __el0_kctx
    add     x9, x9, :lo12:__el0_kctx
    mrs     x10, mpidr_el1          // this core's slot: MPIDR Aff1 (husky: 0-8), 192 bytes each
    ubfx    x10, x10, #8, #4
    add     x9, x9, x10, lsl #7
    add     x9, x9, x10, lsl #6
    ldp     x19, x20, [x9, #0]
    ldp     x21, x22, [x9, #16]
    ldp     x23, x24, [x9, #32]
    ldp     x25, x26, [x9, #48]
    ldp     x27, x28, [x9, #64]
    ldp     x29, x30, [x9, #80]
    ldr     x10, [x9, #96]
    mov     sp, x10
    ldp     d8, d9, [x9, #104]
    ldp     d10, d11, [x9, #120]
    ldp     d12, d13, [x9, #136]
    ldp     d14, d15, [x9, #152]
    // Back in the kernel from exception context: taking the EL0 exception masked IRQs (PSTATE.I), and `ret` doesn't restore them. Left masked, the killswitch's key interrupts are never taken again — every boot lost it the moment `init` ran (seen 2026-10-15: DAIF G#3C0 in the service loop).
    msr     daifclr, #2
    ret

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
/// M1 has no processes yet; the shared vector table still names the EL0 trap handler, so a stand-in ends anything that could ever reach it.
#[cfg(not(feature = "pixel8"))]
#[unsafe(no_mangle)]
pub extern "C" fn ferros_el0_trap(_frame: *mut u64) -> u64 {
    1
}
#[cfg(not(feature = "pixel8"))]
#[unsafe(no_mangle)]
pub extern "C" fn ferros_el0_preempt(_frame: *mut u64) -> u64 {
    1
}

/// Set by `.Lirq_power` when the power key is pressed.
#[unsafe(no_mangle)]
pub static mut __power_pressed: u64 = 0;
/// Kernel context saved by `ferros_el0_enter` (x19..x30, SP) and restored when the process ends.
#[unsafe(no_mangle)]
/// The kernel's callee-saved registers, SP and d8-d15 around a process run, per core (24 words = 192 bytes a slot, MPIDR Aff1 picks it): two cores enter processes at once.
pub static mut __el0_kctx: [[u64; 24]; 16] = [[0; 24]; 16];
/// Whether the cold boot armed the UFS power-on write protect: 0 not attempted, 1 armed, 2 the device refused (DIAG UFS_POWER_ON_WP).
pub static UFS_WP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// The flag as the boot found it (DIAG UFS_POWER_ON_WP_BEFORE): 0 clear, 1 set — it survived the reset — 2 unreadable.
pub static UFS_WP_BEFORE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Interrupts taken that were not the killswitch firing (key edges with one key up, spurious, stray lines switched off): `.Lirq`.
#[unsafe(no_mangle)]
pub static mut __irq_count: u64 = 0;
/// Per core (MPIDR Aff1), where its IRQ handler last was: 1 entered, 2 past the chord check, 3 the dead loop (both volume keys read pressed and SYSTEM_OFF returned), 4 a device line masked and marked pending, 5 after EOI, 6 leaving for the preempt path, 7 returned to the interrupted code. DIAG IRQ_STAGE_Ck; a core stuck in the handler shows where (2026-10-23).
#[unsafe(no_mangle)]
pub static mut __irq_stage: [u64; 16] = [0; 16];
/// Per core, what SGI 2 (`diag core <k>`) found when it interrupted it: ELR, SPSR, SP (the handler's), SP_EL0, then x0-x3 as they were — a stuck core's exact position (2026-10-23).
#[unsafe(no_mangle)]
pub static mut __core_dump: [[u64; 8]; 16] = [[0; 8]; 16];
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
            // Delays on the generic timer, not a spin count: the old "~4000 spin iterations per us" turned the 3.56 ms these steps ask for into 230 ms of the press-to-ready time (measured 2026-10-22, `diag boot`), the core running far below the clock it assumed. The timer gives exactly what each step specifies whatever the CPU clock.
            let udelay = |us: u32| {
                let t0 = platform::cntpct();
                let ticks = us as u64 * platform::cntfrq() / 1_000_000 + 1;
                while platform::cntpct().wrapping_sub(t0) < ticks {
                    core::hint::spin_loop();
                }
            };

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
    // Killswitch before anything else, then the cache map.
    killswitch::arm();
    cachemap::enable();
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
    // FERROS_TEST_HANG=1 builds: stop here, before the watchdog is petted or the slot marked, as a broken build would. ABL's watchdog then resets the phone, the slot's retries count down, and ABL should fall back to the other slot (the A/B fallback test, BOOT.md). Never in a build that ships.
    if option_env!("FERROS_TEST_HANG") == Some("1") {
        loop {
            unsafe { core::arch::asm!("wfe") };
        }
    }
    // Killswitch before anything else, then the cache map.
    killswitch::arm();
    // Boot timing starts here: everything after this is ferros's own time (`diag boot`).
    checklist::ENTRY.store(platform::cntpct(), core::sync::atomic::Ordering::Relaxed);
    // The I-cache before the cache map: with the MMU off and the I-cache off every instruction is a DRAM read, and building the tables plus discarding the window's stale lines ran 317 ms that way (`diag boot`, 2026-10-27). With SCTLR.I set and the MMU off, instruction fetches are cacheable; `enable_icache` invalidates first.
    platform::enable_icache();
    let cached = cachemap::enable();
    checklist::at("cache map on");
    // Cold boot from ABL (slot b): ABL loads the image at its link base G#8008_0000 (aarch64-pixel8.ld) with the MMU and caches off, x0 = DTB pointer. Heap right after the image + 64 KB stack.
    unsafe extern "C" { static __stack_top: u8; static _start: u8; }
    let stack_top = unsafe { &__stack_top as *const u8 as usize };
    let heap_base = (stack_top + 0xFFF) & !0xFFF;
    unsafe { KERNEL_HEAP.init(heap_base, HEAP_SIZE) };
    // The heap's second region joins later, from the service loop (`heap2_join`): its stale lines must be discarded first, and that takes ~235 ms the boot should not wait for.
    HEAP2_CACHED.store(cached, Ordering::Relaxed);
    platform::canary_init();
    platform::stack_paint();
    platform::notes_rotate();
    // The big core, started now so its power-up overlaps the boot; the service moves to it (x3.rs) if it is parked by then.
    if option_env!("FERROS_X3") != Some("0") {
        x3::launch();
    }

    // The previous session's report (ramoops DRAM survives warm resets), then the safety nets: fault sink + panic reporter into the report block, cluster watchdogs.
    let prev = platform::read_report();
    platform::arm_safety();
    platform::crumb(0xC0);
    checklist::at("safety armed");

    // The "Ready?" gate, on a real ABL boot (a RELOAD swaps code on a device that has already booted, it is not a boot). Everything above it is plumbing; ferros's boot starts at the press.
    // Started by the hv's boot gate (slot a, "ferros" already chosen there): no second gate.
    let hv_answered = ready::answered_by_hv();
    let real_boot = dtb_addr != 0 && dtb_addr & 1 == 0;
    // The hv's gate cleared the panel and drew only the gate's picture: adopting it need not clear it again.
    if hv_answered {
        display::KEEP_PIXELS.store(true, core::sync::atomic::Ordering::Relaxed);
    }
    // Dev builds bring USB up before the gate, so the host can answer it (`ferros-bridge gate ferros|android|hold`) as the keys do; the service takes the controller over after the press. Release builds have no USB until the press (owner, 2026-10-22).
    let mut gate_usb: Option<ferros_hal::usb::Dwc3Dev> = None;
    if cfg!(not(ferros_production)) && !hv_answered && real_boot {
        pixel8_eusb_phy_init();
        ferros_hal::usb::set_dwc3_base(service::DWC3);
        gate_usb = ferros_hal::usb::Dwc3Dev::init();
        checklist::at(if gate_usb.is_some() { "gate: usb up" } else { "gate: usb failed" });
    }
    let (pressed_at, gate_choice) = if hv_answered {
        (platform::cntpct(), ready::Choice::Ferros)
    } else if real_boot {
        ready::wait(gate_usb.as_mut())
    } else {
        (0, ready::Choice::Ferros)
    };
    let _ = pressed_at;
    checklist::PRESS.store(pressed_at, core::sync::atomic::Ordering::Relaxed);

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
    checklist::at("repeater read + tuned");

    // The road map on the panel (ABL's splash surface): what this boot has done so far is green.
    checklist::start(platform::cntfrq(), &[checklist::Item::Killswitch]);
    checklist::at("checklist drawn");
    // The board's CDT and MLB for the sensor registry scripts (usf.rs), while ABL's device tree is still where it put it: after the ring's first frame, so the walk (~120 ms before, uncached) is not between the press and the glass.
    if real_boot {
        usf::read_board(dtb_addr);
        modem::read_board(dtb_addr);
    }
    checklist::at("board read");
    if cached {
        checklist::mark(checklist::Item::CacheMap);
    }
    // KOHI ran during the "Ready?" wait: light the wairua line now that the checklist exists.
    if let Some(fp) = wairua::peek().filter(|s| s.0).and_then(|_| wairua::fingerprint()) {
        let hex = b"0123456789ABCDEF";
        checklist::mark_interim(checklist::Item::Wairua);
        checklist::suffix(checklist::Item::Wairua, &[hex[(fp[0] >> 4) as usize], hex[(fp[0] & 15) as usize], hex[(fp[1] >> 4) as usize], hex[(fp[1] & 15) as usize]]);
    }

    // UFS: ABL leaves the flash unpowered (cold_ufs::power_on_device); bring the link up, then open the ira-rooted vault on the ferros partition exactly as the handoff path does.
    let t_ufs = platform::cntpct();
    let link_up = cold_ufs::bring_up();
    let ufs_ms = (platform::cntpct().wrapping_sub(t_ufs) / (platform::cntfrq() / 1000)) as u32;
    if link_up {
        checklist::mark(checklist::Item::Disk);
    }
    // The gate chose Android: the moment the disk answers (set-active needs devinfo and the boot LU attribute, nothing of the vault) — slot a active, then a plain reset; the bootloader boots slot a, whose hv gate takes the one-shot marker and goes to Android. The way back is a reboot: slot a's gate asks again (1 s on dev builds) and boots the ferros packed into it (tools/husky-cold.sh keeps that copy current). Android cannot boot from slot b's own partitions (product_b is empty), which is why the choice is a slot switch and not a chainload.
    if gate_choice == ready::Choice::Android && link_up {
        // Tell slot a's gate (ferros_hv/src/gate.rs) to answer "Android" this once instead of asking again and auto-choosing ferros after a second, as `diag android` does; cleaned to DRAM, since the gate reads it with the caches off. Every later boot of slot a asks again, so a reboot from Android lands on the gate and, untouched, on ferros (owner, 2026-10-22: never stuck in Android).
        unsafe {
            let p = 0xFD60_0FF8usize as *mut u64;
            core::ptr::write_volatile(p, 0x414E_4452_4F49_4431);
            core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) p as u64);
        }
        let r = slot::set_active(0);
        if r == slot::MARKED {
            platform::crumb(0xC7);
            platform::reboot();
        }
        checklist::suffix(checklist::Item::Slot, b"android: set-active failed");
    }
    platform::crumb(0xC2);
    let t_vault = platform::cntpct();
    if link_up {
        let ok = flashlog::init();
        flashlog::line(&alloc::format!("boot cold, flash log init {ok}"));
    }
    let (probe, mut store) = vault_boot::vault_probe(link_up);
    let vault_ms = (platform::cntpct().wrapping_sub(t_vault) / (platform::cntfrq() / 1000)) as u32;
    if probe.opened == 1 {
        checklist::mark(checklist::Item::VaultOpen);
    }
    if probe.verified == 1 {
        checklist::mark(checklist::Item::VaultVerified);
    }
    // ira: yellow once derived — up, but a stand-in until PIPE (SoC-sourced and held in RAM, not on-die registers, KEY_REGISTERS.md) — with the first two bytes of its one-way fingerprint after the label (256-bit secret, so plain BLAKE3 is already one-way; spaghettify only matters for guessable inputs like handles). The wairua line is lit by wairua.rs when KOHI first runs: it is lazy, nothing at boot needs it.
    if probe.ira_fail == 0 {
        checklist::mark_interim(checklist::Item::Ira);
        let hex = b"0123456789ABCDEF";
        let f = &probe.ira_fp;
        checklist::suffix(checklist::Item::Ira, &[hex[(f[0] >> 4) as usize], hex[(f[0] & 15) as usize], hex[(f[1] >> 4) as usize], hex[(f[1] & 15) as usize]]);
    }

    // The flash's power-on write protect, armed now that the link is up (BOOT.md "Dual Boot", owner's yes 2026-10-17): the bootloader's own LUs become read-only until the next reset. The bootloader does not arm it itself; fastboot runs before anything could, so flashing is untouched. Reported in DIAG as UFS_POWER_ON_WP (1 armed, 2 refused, 0 no link).
    let ufs_wp = if dtb_addr != 0 && dtb_addr & 1 == 0 && store.is_some() {
        let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
        // As the boot found it: whether the flag survived the reset that brought us here (a warm reset may keep it; DIAG UFS_POWER_ON_WP_BEFORE: 0/1, 2 unreadable).
        UFS_WP_BEFORE.store(match ufs.read_flag(0x03) { Some(true) => 1, Some(false) => 0, None => 2 }, core::sync::atomic::Ordering::Relaxed);
        if cfg!(ferros_no_ufs_wp) {
            3
        } else if ufs.set_flag(0x03) {
            1
        } else {
            2
        }
    } else {
        0
    };
    UFS_WP.store(ufs_wp, core::sync::atomic::Ordering::Relaxed);

    platform::crumb(0xE4);

    // Tell ABL this slot boots (devinfo "successful"), so its retry countdown stops and a power cycle lands here again. Only on a real ABL boot (x0 = DTB pointer, not a RELOAD tag) with the vault verified: a build that dies before this point still falls back to the other slot.
    let mark = if dtb_addr != 0 && dtb_addr & 1 == 0 && store.is_some() {
        let m = slot::mark_booted_slot(dtb_addr);
        if m.result == slot::MARKED || m.result == slot::ALREADY {
            checklist::mark(checklist::Item::Slot);
        }
        m
    } else {
        slot::SlotMark { slot: 0xFF, before: 0, after: 0, result: 0xFF }
    };

    // The device's settings (SETTINGS.md, settings.rs): one VSF document under `device`, loaded here so everything drawn from now on reads in them. Defaults first (DMS, tatau's house base; brightness 2, white about 147 cd/m²; Dvorak; everything else 0), then whatever the document carries. An identity's preferences replace them when that identity attests; nothing here ever writes them back to an identity.
    process::set_pref(process::PREF_BASE, tatau::base().code());
    process::set_pref(process::PREF_BRIGHTNESS, 2);
    if let Some(doc) = store.as_mut().and_then(|s| s.get_by_key(settings::KEY).ok().flatten()) {
        let _ = settings::apply(&doc, false);
    }
    platform::crumb(0xE5);
    checklist::base_known();

    // The first process is no longer started here: the vault's `init` starts when the boot ring completes (service.rs `start_home`), holding the shell's capabilities, and starts the shell itself (owner, 2026-10-22).
    let init_exit = 0u32;

    let own_base = &raw const _start as usize;
    let extras = alloc::vec![
        (b"COLD_UFS_MS=".as_slice(), ufs_ms),
        (b"UFS_POWER_ON_WP=".as_slice(), UFS_WP.load(core::sync::atomic::Ordering::Relaxed)),
        (b"UFS_POWER_ON_WP_BEFORE=".as_slice(), UFS_WP_BEFORE.load(core::sync::atomic::Ordering::Relaxed)),
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
        (b"INIT=".as_slice(), init_exit),
    ];
    // RELOAD stages at base+32 MiB (the slot the old ping-pong reload proved on hardware; base+64 MiB was fatal) and runs its copy stub 64 KiB below it, far past this image + stack + 4 MiB heap.
    platform::crumb(0xE6);
    checklist::at("service start");
    let boot = service::Boot {
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
        usb: gate_usb,
    };
    // On the X3 if it is parked by now (every build since 2026-10-22, proven on a cold boot; FERROS_X3=0 keeps CPU0); otherwise here.
    // CPU0 is about to park: the ira and wairua it holds go sealed to the X3 and the workers, and its own registers are cleared (keys.rs).
    if option_env!("FERROS_X3") != Some("0") && x3::parked() {
        keys::leave(keys::AFTER_HOP);
    }
    let boot = if option_env!("FERROS_X3") != Some("0") { x3::hand_over(boot) } else { boot };
    platform::crumb(0xE7);
    service::run(boot)
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
