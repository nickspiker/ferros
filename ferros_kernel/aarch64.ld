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
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    . = ALIGN(4096);
    __kernel_end = .;

    /* Stack: 64KB, placed after kernel image */
    . = . + 0x10000;
    __stack_top = .;

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
