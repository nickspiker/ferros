/*
 * Linker script for ferros kernel — aarch64 bare metal.
 *
 * ABL loads our image at 0x80000 (standard Linux arm64 Image offset).
 * We set up our own stack and jump to Rust.
 *
 * Memory map (after ABL hands off):
 *   0x0000_0000 - 0x0007_FFFF : reserved (ABL, DTB, etc.)
 *   0x0008_0000 - ...          : ferros kernel (.text, .rodata, .data, .bss)
 *   ...         - STACK_TOP    : kernel stack (grows down)
 *   0x9C00_0000 - ~0x9D30_0000 : framebuffer (ABL's simplefb, ~13MB)
 *   DTB addr                   : passed in x0 by ABL
 */

ENTRY(_start)

SECTIONS
{
    . = 0x80000;

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

    /* Stack: 64KB, placed after kernel image */
    . = . + 0x10000;
    __stack_top = .;

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
