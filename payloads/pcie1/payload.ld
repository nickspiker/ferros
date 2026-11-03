/* ferros payload linker script — entry pinned at offset 0, everything PC-relative. The kernel calls blob offset 0 with the RUN ABI, so .text.entry MUST come first. */
ENTRY(_entry)
SECTIONS {
    . = 0;
    .text : {
        KEEP(*(.text.entry))
        *(.text*)
    }
    .rodata : { *(.rodata*) }
    .data : { *(.data*) }
    . = ALIGN(64);
    .bss : { *(.bss*) *(COMMON) }
    /DISCARD/ : { *(.comment) *(.eh_frame*) }
}
__ferros_tail = SIZEOF(.bss) + 16;
