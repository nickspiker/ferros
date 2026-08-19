/*
 * Linker script for ferros kernel — aarch64 bare metal, SHIM HANDOFF variant.
 *
 * Identical to aarch64.ld EXCEPT the load base.
 *
 * The cold-boot image links at 0x80000 (ABL's Linux Image offset). That low
 * region is where the running shim kernel's own text lives, so we cannot drop
 * ferros there while Linux is still executing.
 *
 * This variant links ferros at 0x92400000 — the ferros_handoff carveout
 * declared no-map in zuma-rmem.dtsi (12 MB hole between gxp_mcu_fw and tpu_fw).
 * The shim kernel copies ferros.bin here and jumps via cpu_soft_restart, so:
 *   - the running kernel is never overwritten (no relocation trampoline needed),
 *   - ferros's image, stack, page tables, and static UFS DMA buffers (UFS_BUF)
 *     all fall inside the reserved region, so the UFS master DMAs only into
 *     DRAM the kernel guaranteed it never mapped.
 *
 * Memory map (after the shim hands off):
 *   0x9240_0000 - ...          : ferros kernel (.text, .rodata, .data, .bss)
 *   ...         - __stack_top  : kernel stack (grows down)
 *   0x92F0_0000                : result scratch (ferros writes, next boot reads)
 *   0x9300_0000                : tpu_fw_reserved (do not cross)
 */

ENTRY(_start)

SECTIONS
{
    . = 0x92400000;

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
