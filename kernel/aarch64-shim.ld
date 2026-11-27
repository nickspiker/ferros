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

    /* Stack: 256 KiB, placed after the kernel image. 64 KiB overflowed into .bss: the service loop needs ~95 KiB and a vault write or a spawn under load spikes ~22 KiB more (the cold path's aarch64-pixel8.ld was raised to 512 KiB for this on 2026-10-23, but this handoff script was left at 64 KiB — the handoff then overflowed nondeterministically, which is the watchdog resets and flaky USB enumeration of 2026-10-29). 256 KiB clears it with margin inside the 12 MiB carveout (heap is 4 MiB above __stack_top, well below the result scratch at 0x92F00000). DIAG's STACK_USED_MAX reports the measured depth. */
    __stack_bottom = .;
    . = . + 0x40000;
    __stack_top = .;

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
