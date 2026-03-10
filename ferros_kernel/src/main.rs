//! Ferros kernel — bare metal entry point for aarch64.
//!
//! ## Boot flow
//!
//! ```text
//! ABL loads boot.img → _start (asm) → kernel_main (Rust)
//!   → UART init (serial debug)
//!   → DTB parse (find framebuffer)
//!   → SimpleFB init (proof of life: colored rectangles)
//!   → SD/MMC init (probe microSD)
//!   → Anchor ring scan (find latest committed state)
//!   → Ledger mount (ready for operations)
//! ```
//!
//! ABL passes the DTB physical address in x0 (standard arm64 boot protocol).

#![no_std]
#![no_main]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;

use ferros_hal::uart::{Uart, UartBackend};
use ferros_hal::fb::Framebuffer;
use ferros_hal::dtb::Dtb;

// ---------------------------------------------------------------------------
// Global allocator — simple bump allocator for early boot
// ---------------------------------------------------------------------------

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 256KB heap — plenty for boot. Will be replaced by the ring allocator.
const HEAP_SIZE: usize = 256 * 1024;

#[repr(align(4096))]
struct HeapMem(UnsafeCell<[u8; HEAP_SIZE]>);

// SAFETY: We use atomic operations to synchronize access to the heap.
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

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator never frees. This is fine for boot.
    }
}

#[global_allocator]
static ALLOC: BumpAlloc = BumpAlloc;

// ---------------------------------------------------------------------------
// Boot stub — aarch64 assembly
// ---------------------------------------------------------------------------

global_asm!(r#"
.section .text.boot, "ax"
.global _start

_start:
    // x0 = DTB physical address (from ABL)
    // Save it before we clobber registers.
    mov     x19, x0

    // Disable interrupts
    msr     daifset, #0xF

    // Check CPU ID — only core 0 boots, others park
    mrs     x1, mpidr_el1
    and     x1, x1, #0xFF
    cbz     x1, .Lprimary
.Lpark:
    wfe
    b       .Lpark

.Lprimary:
    // Set up stack
    ldr     x1, =__stack_top
    mov     sp, x1

    // Zero BSS
    ldr     x1, =__bss_start
    ldr     x2, =__bss_end
.Lbss_loop:
    cmp     x1, x2
    b.ge    .Lbss_done
    str     xzr, [x1], #8
    b       .Lbss_loop
.Lbss_done:

    // Jump to Rust — x19 = DTB address
    mov     x0, x19
    bl      kernel_main

    // If kernel_main returns, halt
.Lhalt:
    wfe
    b       .Lhalt
"#);

// ---------------------------------------------------------------------------
// Kernel entry point (Rust)
// ---------------------------------------------------------------------------

/// Main kernel entry — called from assembly with DTB address.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(dtb_addr: u64) -> ! {
    // -- Stage 1: UART --
    // QEMU virt PL011 at 0x0900_0000.
    // On real FP5, this would be the GENI SE UART base.
    let uart = Uart::new(UartBackend::Pl011 { base: 0x0900_0000 });
    uart.puts("\n\n");
    uart.puts("========================================\n");
    uart.puts("  ferros kernel alive\n");
    uart.puts("========================================\n");
    uart.puts("DTB at: ");
    uart.put_hex(dtb_addr);
    uart.puts("\n");

    // -- Stage 2: Parse DTB --
    let dtb_slice = unsafe {
        // ABL guarantees DTB is valid at this address.
        // Read totalsize from header to know how big it is.
        let header = dtb_addr as *const u8;
        let size_bytes = [
            *header.add(4), *header.add(5), *header.add(6), *header.add(7),
        ];
        let total_size = u32::from_be_bytes(size_bytes) as usize;
        core::slice::from_raw_parts(header, total_size)
    };

    if let Some(dtb) = Dtb::from_bytes(dtb_slice) {
        uart.puts("[dtb] parsed OK\n");

        // -- Stage 3: SimpleFB --
        if let Some(fb_config) = dtb.parse_simplefb() {
            uart.puts("[fb] found simplefb: ");
            uart.put_hex(fb_config.phys_base);
            uart.puts(" ");
            uart.put_hex(fb_config.width as u64);
            uart.puts("x");
            uart.put_hex(fb_config.height as u64);
            uart.puts("\n");

            // Identity-mapped: phys == virt on bare metal
            let fb_ptr = fb_config.phys_base as *mut u8;
            let mut fb = unsafe {
                Framebuffer::new(fb_config, fb_ptr)
            };

            // Proof of life: fill screen dark blue, draw ferros logo placeholder
            fb.clear(0x10, 0x10, 0x30);  // dark blue-gray

            // Draw a white rectangle in the center
            let cx = fb.config.width / 2;
            let cy = fb.config.height / 2;
            fb.fill_rect(cx - 100, cy - 40, 200, 80, 0xFF, 0xFF, 0xFF);

            // Draw ferros orange accent bar
            fb.fill_rect(cx - 100, cy + 50, 200, 8, 0xFF, 0x80, 0x00);

            uart.puts("[fb] proof of life displayed\n");
        } else {
            uart.puts("[fb] no simplefb node in DTB\n");
        }

        // -- Stage 4: SD/MMC (placeholder) --
        uart.puts("[sdmmc] TODO: probe microSD controller\n");

        // -- Stage 5: Anchor ring (placeholder) --
        uart.puts("[anchor] TODO: scan anchor ring on microSD\n");

        // -- Stage 6: Ledger mount (placeholder) --
        uart.puts("[ledger] TODO: mesh consensus and mount\n");
    } else {
        uart.puts("[dtb] FAILED to parse DTB!\n");
    }

    uart.puts("\n");
    uart.puts("ferros: boot complete, entering idle loop\n");

    loop {
        // WFE = Wait For Event — low power idle
        unsafe { core::arch::asm!("wfe") };
    }
}

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Try to print panic message via UART
    let uart = Uart::new(UartBackend::Pl011 { base: 0x0900_0000 });
    uart.puts("\n!!! KERNEL PANIC !!!\n");
    if let Some(location) = info.location() {
        uart.puts(location.file());
        uart.puts(":");
        // Print line number as decimal
        let mut line = location.line();
        let mut buf = [0u8; 10];
        let mut i = 0;
        if line == 0 {
            uart.putc(b'0');
        } else {
            while line > 0 {
                buf[i] = b'0' + (line % 10) as u8;
                line /= 10;
                i += 1;
            }
            while i > 0 {
                i -= 1;
                uart.putc(buf[i]);
            }
        }
        uart.puts("\n");
    }

    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}
