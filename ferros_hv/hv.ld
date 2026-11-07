/* ferros hv (HV.md). ABL loads the blob [hv | Linux Image] at G#8008_0000 and jumps to its first instruction with the MMU off; the hv copies itself to HV_BASE, 64 MiB of the LOW DRAM bank at G#B200_0000 (the high bank, G#8_8000_0000 up, is not kept refreshed across resets and is suspected of the same under deep idle), which it also reserves in the DTB it hands Linux. Everything is linked at HV_BASE; the few instructions that run before the copy are PC-relative. */
INCLUDE hv_base.ld
ENTRY(_start)
SECTIONS
{
    . = HV_BASE;
    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }
    .rodata : ALIGN(4096) { *(.rodata .rodata.*) }
    .data : ALIGN(4096) { *(.data .data.*) }
    __hv_copy_end = ALIGN(16);
    .bss : ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }
    . = ALIGN(4096);
    __stacks = .;
    . = . + 0x40000;        /* 16 KiB per core, cores 0-15 */
    __stacks_end = .;
    __hv_end = .;
    /* The ARM64 header's image_size (__hv_image_size): hv pad plus the Linux Image's own, per board, in hv_base.ld (build.rs). */
    __hv_copy_size = __hv_copy_end - HV_BASE;
    /DISCARD/ : { *(.eh_frame*) *(.note*) *(.comment) }
}
