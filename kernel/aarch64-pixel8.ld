/*
 * Linker script for ferros kernel — Pixel 8 Pro (husky) COLD boot from ABL.
 *
 * Identical to aarch64.ld EXCEPT the link base: ABL loads the boot.img kernel
 * at DRAM base + text_offset = 0x8008_0000 (measured on husky). The image is
 * statically linked, so absolute addresses in its data (vtables, function
 * pointers) are only right if it runs where it is linked; aarch64.ld's 0x80000
 * stays for m1 (m1n1 places the raw image itself).
 *
 * Memory map (after ABL hands off):
 *   0x8008_0000 - ...          : ferros kernel (.text, .rodata, .data, .bss)
 *   ...         - __stack_top  : 512 KiB kernel stack, then the heap
 *   0x8208_0000                : RELOAD/RUN staging slot (base + 32 MiB)
 *   0xFD60_0000                : ramoops result block (survives warm resets)
 */

ENTRY(_start)

SECTIONS
{
    . = 0x80080000;

    .text : {
        KEEP(*(.text.boot))    /* boot stub must be first */
        *(.text .text.*)
    }

    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4096) {
        *(.data .data.*)
    }

    .bss : ALIGN(4096) {
        __bss_start = .;
        /* Device DMA buffers (ferros_hal: UFS descriptors + data, DWC3 events, TRBs, EP0 and bulk buffers), page-aligned so the static cache map can keep them uncached (cachemap.rs). Zeroed with the rest of .bss. */
        . = ALIGN(4096);
        __dma_start = .;
        /* UFS's own buffers first, on their own pages: the HSI2 S2MPU lets VID 0 (UFS and the WiFi chip share it) reach exactly these, not the USB/PT buffers beside them (s2mpu.rs). */
        __dma_ufs_start = .;
        *(.bss.dma.ufs)
        . = ALIGN(4096);
        __dma_ufs_end = .;
        *(.bss.dma .bss.dma.*)
        . = ALIGN(4096);
        __dma_end = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    . = ALIGN(4096);

    /* Page tables for identity mapping (2 × 4KB: L0 + L1) */
    __pagetable = .;
    . = . + 0x2000;

    __kernel_end = .;
    __kernel_size = __kernel_end - _start;

    /* Stack: 512 KiB, placed after the kernel image. 64 KiB overflowed into .bss on reloads: a reload runs the service loop on this stack under main's whole frame (a cold boot runs it on the X3's own 256 KiB stack), and a vault write or a spawn under load walked 22 KB past the bottom (the .bss canary, 2026-10-23). DIAG's STACK_USED_MAX is the measured depth. */
    __stack_bottom = .;
    . = . + 0x80000;
    __stack_top = .;

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
