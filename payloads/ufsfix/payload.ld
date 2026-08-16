/* ferros payload linker script — entry pinned at offset 0, everything PC-relative. The kernel calls blob offset 0 with the RUN ABI, so .text.entry MUST come first. */
/* .bss is NOT in the flat blob (objcopy drops it) — the entry stub must zero [__bss_start, __bss_end) itself before touching any static. */
ENTRY(_entry)
SECTIONS {
    . = 0;
    .text : {
        KEEP(*(.text.entry))
        *(.text*)
    }
    .rodata : { *(.rodata*) }
    .data : { *(.data*) }
    .bss : {
        . = ALIGN(1024);
        __bss_start = .;
        *(.bss*)
        *(COMMON)
        __bss_end = .;
    }
    /DISCARD/ : { *(.comment) *(.eh_frame*) }
}
