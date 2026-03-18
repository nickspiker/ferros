/*
 * Linker script for ferros seed — aarch64 bare metal.
 *
 * ABL loads the seed at 0x80000 (standard Linux arm64 Image offset).
 * The seed verifies itself and the kernel, then jumps to the kernel
 * at KERNEL_DRAM_BASE (0x80080000).
 *
 * The seed is small (<16KB). No page tables, minimal stack.
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

        /* Ed25519 public key — 32 bytes, patched at build time */
        . = ALIGN(32);
        __pubkey_start = .;
        KEEP(*(.rodata.pubkey))
        __pubkey_end = .;

        /* Ed25519 signature — 64 bytes, zeroed at build, patched by mkimg sign */
        . = ALIGN(64);
        __sig_start = .;
        KEEP(*(.rodata.sig))
        __sig_end = .;
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
    __seed_end = .;
    __seed_size = __seed_end - _start;

    /* Stack: 16KB (seed doesn't need much) */
    . = . + 0x4000;
    __stack_top = .;

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
